//! Encryption as the writer writes it: how a file is encrypted, a
//! reading's or a new one's ([`Encryptor`]), and what encrypts one file's
//! objects as pdf-objects' writer writes them ([`Encrypting`]), by the list
//! of what's left in the clear that the reader decrypts by.

use std::borrow::Cow;
use std::convert::Infallible;
use std::fmt;

use pdf_crypt::rand_core::CryptoRng;
use pdf_crypt::{Encoded, Fault, Handler, Method, Unusable};
use pdf_objects::Resolve;
use pdf_objects::object::{Dict, Object, ObjectId, Str, Value};
use pdf_objects::serialize::{self, Encrypt};

use super::Error;
use crate::bytes::{ByteSource, Reading};
use crate::crypt::{self, List, Stream};
use crate::filter;
use crate::source::Encryption;

/// How a written file is encrypted: a security handler and its key, a
/// reading's or a new file's; its encryption dictionary as it's written,
/// and where the file it was read from had it; the `/ID[0]` the key is
/// made from; and what it leaves in the clear, the list the reader
/// decrypts by (see [`bytes`](crate::bytes), "What's encrypted"). It gives
/// out nothing of the key, its `Debug` neither; a clone clones the handler,
/// each key wiped when it's dropped (pdf-crypt's `Handler`).
#[derive(Clone)]
pub struct Encryptor {
    handler: Handler,
    list: List,
    dict: Dict,
    place: Place,
    first_id: Option<Vec<u8>>,
}

/// Where an encryption dictionary was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// At its number, in the file it was read from.
    At(ObjectId),
    /// In that file's trailer.
    Direct,
    /// Nowhere: it's a new file's.
    New,
}

impl fmt::Debug for Encryptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encryptor")
            .field("handler", &self.handler)
            .field("place", &self.place)
            .finish_non_exhaustive()
    }
}

impl Encryptor {
    /// How `reading`'s file is encrypted, to write it encrypted as it is:
    /// its handler, as the password the reading was given opened it; its
    /// `/Encrypt` dictionary as the file stores it, its strings as written,
    /// at its number or in the trailer; its first `/ID` element, as the
    /// handler opened by it (empty where the trailer has none); and what it
    /// leaves in the clear, as the reading decrypts it. None for a file that
    /// isn't encrypted. Refused for one no password opened
    /// ([`Error::Locked`], whether its strings and streams are encrypted or
    /// not, [`Encryption::Plain`]), and for one whose security handler the
    /// reader can't open ([`Error::Unsupported`]): there's nothing to
    /// encrypt what's written by.
    pub fn of<S: ByteSource>(reading: &Reading<S>) -> Result<Option<Encryptor>, Error> {
        match (reading.encryption(), reading.handler(), reading.stored()) {
            (Encryption::None, ..) => Ok(None),
            (Encryption::Unsupported(why), ..) => Err(Error::Unsupported(why.clone())),
            (Encryption::Opened { .. }, Some(handler), Some(stored)) => Ok(Some(Encryptor {
                handler: handler.clone(),
                list: reading.list().clone(),
                dict: stored.dict.clone(),
                place: stored.at.map_or(Place::Direct, Place::At),
                first_id: Some(stored.id.clone()),
            })),
            _ => Err(Error::Locked),
        }
    }

    /// A new file's encryption, of revision 6 (AES-256) for `password`,
    /// prepared, as pdf-crypt's `Handler::create` makes it from `rng`'s
    /// draws, the document's metadata encrypted as `encrypt_metadata` says;
    /// its dictionary with its crypt filter's `/Length 32` (bytes, as qpdf
    /// writes it) and `/AuthEvent /DocOpen`. Its key isn't made from the
    /// file's `/ID` ([`Encryptor::first_id`] is none). Refused, as
    /// `Handler::create` refuses, for a password that isn't prepared, or is
    /// empty or too long.
    pub fn create(
        password: &Encoded,
        encrypt_metadata: bool,
        rng: &mut (impl CryptoRng + ?Sized),
    ) -> Result<Encryptor, Unusable> {
        let (handler, values) = Handler::create(password, encrypt_metadata, rng)?;
        let mut list = List {
            encrypts: true,
            ..List::default()
        };
        list.settle(&values, Ok(&handler));
        Ok(Encryptor {
            handler,
            list,
            dict: encryption(&values, Some(32)),
            place: Place::New,
            first_id: None,
        })
    }

    /// Whether objects may go in object streams under it: a container is
    /// encrypted as a stream, whole, its members' strings with it, so not
    /// where streams are left in the clear (`/StmF /Identity`) and strings
    /// aren't, whose strings would then be in the clear; nor where either's
    /// method isn't one.
    pub fn packs(&self) -> bool {
        let plain = |m: &Method| matches!(m, Method::Identity | Method::None);
        let known = |m: &Method| !matches!(m, Method::Unknown(_));
        let (streams, strings) = (self.handler.streams(), self.handler.strings());
        known(streams) && known(strings) && (!plain(streams) || plain(strings))
    }

    /// The encryption dictionary, as the file it was read from stores it,
    /// its strings as written; or a new file's.
    pub fn dictionary(&self) -> &Dict {
        &self.dict
    }

    /// Where the file it was read from has the dictionary: its number.
    /// None where it's in that file's trailer, and for a new file's, which
    /// has no number yet.
    pub fn at(&self) -> Option<ObjectId> {
        match self.place {
            Place::At(id) => Some(id),
            Place::Direct | Place::New => None,
        }
    }

    /// Whether the file it was read from has the dictionary in its
    /// trailer, direct, as a whole file and an update written under it
    /// keep it, where one at a number is written as an object.
    pub fn direct(&self) -> bool {
        self.place == Place::Direct
    }

    /// The first `/ID` element the key is made from: the file's it was read
    /// from, as its handler opened by it, empty where that file's trailer
    /// has none. None for a new file's, whose key isn't made from it.
    pub fn first_id(&self) -> Option<&[u8]> {
        self.first_id.as_deref()
    }

    /// What encrypts the objects of a file written under it: pdf-objects'
    /// encryptor, for its `indirect_with` and `indirect_stream_with`.
    /// `trailer` is the file's trailer, which says which stream is the
    /// document's metadata (its catalog's `/Metadata`), and `resolve` gives
    /// the objects a reference names: the metadata stream's way there, and a
    /// stream's `/Filter` and `/DecodeParms`, which say whether it has a
    /// crypt filter of its own.
    pub fn encrypting<'a>(
        &'a self,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
    ) -> Encrypting<'a> {
        let metadata = match self.list.unencrypted_metadata {
            true => {
                let get: crypt::Get<'_, '_, Infallible> = &|id| Ok(resolve.resolve(id));
                let Ok(metadata) = crypt::metadata_stream(trailer, get);
                metadata
            }
            false => None,
        };
        Encrypting {
            encryptor: self,
            resolve,
            metadata,
        }
    }
}

/// What encrypts the objects of one file written under an [`Encryptor`]
/// as pdf-objects' writer writes them: its `serialize::Encrypt`, which
/// every string and stream's data of an indirect object written by
/// `indirect_with` or `indirect_stream_with` passes through, encrypted by
/// the handler with the object's key, AES's IV derived from `n g obj` and
/// the data, as pdf-crypt's `Handler::encrypt_string` and `encrypt_stream`
/// derive it, so the same object encrypts to the same bytes on every save
/// and no IV serves two plaintexts. What it leaves in the clear is the list
/// the reader decrypts by (see [`bytes`](crate::bytes), "What's
/// encrypted"): an encryption dictionary and what its entries refer to, a
/// cross-reference stream, a signature's `/Contents`, a stream whose crypt
/// filter is `/Identity`, and the document's metadata stream's data under
/// `/EncryptMetadata false`. It gives out nothing of the key: it encrypts
/// as the reading that opened the file decrypts.
pub struct Encrypting<'a> {
    encryptor: &'a Encryptor,
    resolve: &'a (dyn Resolve<'a> + Sync),
    /// The document's metadata stream, where `/EncryptMetadata false`
    /// leaves its data in the clear.
    metadata: Option<ObjectId>,
}

impl fmt::Debug for Encrypting<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encrypting")
            .field("encryptor", &self.encryptor)
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

/// Why a string or a stream's data of a file written encrypted isn't
/// encrypted, which the writer refuses ([`Error::Encrypt`]), writing
/// nothing in the clear.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unencryptable {
    /// pdf-crypt's fault: a crypt filter's method that isn't one, or one
    /// the dictionary hasn't, or AES that the file's key isn't for.
    #[error("{0}")]
    Fault(Fault),
    /// It's a stream an encryption dictionary refers to, whose strings are
    /// left as written: its data is neither said to be plaintext nor to be
    /// encrypted.
    #[error(
        "it's a stream an encryption dictionary refers to, whose data isn't said to be encrypted or not"
    )]
    Dictionary,
    /// It's an embedded file's data, without a crypt filter of its own, and
    /// the encryption dictionary's `/EFF` isn't a name, so doesn't say how
    /// embedded files are encrypted.
    #[error(
        "it's an embedded file's data, and the encryption dictionary's /EFF, which says how that's encrypted, isn't a name"
    )]
    Unsaid,
}

impl Encrypt for Encrypting<'_> {
    type Error = Unencryptable;

    /// In the clear: every object of a file that doesn't encrypt; an
    /// encryption dictionary and what its entries refer to, by number
    /// (ISO 32000-2, 7.6.2); and a cross-reference stream (7.5.8.2).
    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        let list = &self.encryptor.list;
        !list.encrypts
            || list.dictionaries.contains(&id)
            || (stream && dict.is_some_and(crypt::is_xref))
    }

    /// A signature's `/Contents`, in a dictionary with `/ByteRange` and a
    /// string `/Contents`, of `/Type /Sig`, `/DocTimeStamp` or none (7.6.2).
    fn clear_key(&self, _: ObjectId, dict: &Dict) -> Option<&'static [u8]> {
        crypt::signature(dict).then_some(b"Contents")
    }

    /// Encrypted with the object's key as the handler's strings are, but
    /// where they're left in the clear (`/StrF /Identity`).
    fn string<'t>(&self, id: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Unencryptable> {
        if !self.encryptor.list.encrypted(id) {
            return Ok(Cow::Borrowed(text));
        }
        let handler = &self.encryptor.handler;
        (handler.encrypt_string(id, &context(id), text))
            .map(Cow::Owned)
            .map_err(Unencryptable::Fault)
    }

    /// Encrypted by its own crypt filter when it has one, else an embedded
    /// file's as `/EFF` says, else as the handler's streams are (7.6.6);
    /// in the clear where that's `/Identity`, and the document's metadata
    /// stream's under `/EncryptMetadata false` but where its own crypt
    /// filter encrypts it (7.6.4.2, 14.3.2).
    fn stream<'d>(
        &self,
        id: ObjectId,
        dict: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, Unencryptable> {
        let resolve = |id| self.resolve.resolve(id);
        let own = || Ok::<_, Infallible>(filter::crypt_filter(dict, Some(&resolve)));
        let Ok(stream) = self
            .encryptor
            .list
            .stream(id, dict, own, || Ok(self.metadata));
        let handler = &self.encryptor.handler;
        match stream {
            Stream::Clear => Ok(Cow::Borrowed(data)),
            Stream::Dictionary => Err(Unencryptable::Dictionary),
            Stream::Unsaid => Err(Unencryptable::Unsaid),
            Stream::By(name) => {
                let named = name.map(|name| handler.named(&name));
                let method = named.as_ref().unwrap_or(handler.streams());
                if matches!(method, Method::Identity | Method::None) {
                    return Ok(Cow::Borrowed(data));
                }
                (handler.encrypt_stream(id, &context(id), data, Some(method)))
                    .map(Cow::Owned)
                    .map_err(Unencryptable::Fault)
            }
        }
    }
}

/// What AES's IV for a string or stream's data of object `id` is derived
/// from beside the data: `n g obj`, as the object is written.
fn context((n, g): ObjectId) -> Vec<u8> {
    let mut context = Vec::with_capacity(20);
    serialize::decimal(&mut context, n.into());
    context.push(b' ');
    serialize::decimal(&mut context, g.into());
    context.extend_from_slice(b" obj");
    context
}

/// The encryption dictionary of `values`, each crypt filter's `/Length`
/// `length` where it's given, `/AuthEvent /DocOpen`, and `/StmF` and
/// `/StrF` its first crypt filter; a value `values` hasn't left out.
pub(crate) fn encryption(values: &pdf_crypt::Dictionary, length: Option<i64>) -> Dict {
    let s = |bytes: &[u8]| Object::new(Value::String(Str::Bytes(bytes.to_vec()), None));
    let mut dict = pdf_objects::dict! {
        "Filter" => "Standard",
        "V" => values.v,
        "R" => values.r,
    };
    for (key, value) in [(&b"O"[..], &values.o), (b"U", &values.u)] {
        if let Some(value) = value {
            dict.set(key, s(value));
        }
    }
    if let Some(p) = values.p {
        dict.set("P", p);
    }
    if let Some(length) = values.length {
        dict.set("Length", length);
    }
    if values.v >= 5 {
        let keys = [
            (&b"OE"[..], &values.oe),
            (b"UE", &values.ue),
            (b"Perms", &values.perms),
        ];
        for (key, value) in keys {
            if let Some(value) = value {
                dict.set(key, s(value));
            }
        }
    }
    if let Some(encrypt_metadata) = values.encrypt_metadata {
        dict.set("EncryptMetadata", encrypt_metadata);
    }
    if let Some(filter) = values.crypt_filters.first() {
        let method =
            String::from_utf8_lossy(filter.method.as_deref().unwrap_or_default()).into_owned();
        let mut std_cf = pdf_objects::dict! { "CFM" => method.as_str(), "AuthEvent" => "DocOpen" };
        if let Some(length) = length {
            std_cf.set("Length", length);
        }
        dict.set("CF", pdf_objects::dict! { "StdCF" => std_cf });
        dict.set("StmF", "StdCF");
        dict.set("StrF", "StdCF");
    }
    dict
}
