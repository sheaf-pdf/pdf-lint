//! Encryption as the writer writes it: how a file is encrypted, a
//! reading's or a new one's ([`Encryptor`]), and what encrypts one file's
//! objects as pdf-objects' writer writes them ([`Encrypting`]), by the list
//! of what's left in the clear that the reader decrypts by.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::fmt;

use pdf_crypt::rand_core::CryptoRng;
use pdf_crypt::{Encoded, Fault, Handler, Method, Unusable};
use pdf_objects::Resolve;
use pdf_objects::object::{Dict, Object, ObjectId, Str, Value};
use pdf_objects::serialize::{self, Encrypt, Renumber};

use super::{Error, Numbering};
use crate::bytes::{ByteSource, Reading, SourceId, XrefEntry};
use crate::crypt::{self, List, Stream};
use crate::filter;
use crate::source::Encryption;

/// How a written file is encrypted: a security handler and its key, a
/// reading's or a new file's; its encryption dictionary as it's written,
/// and where the file it was read from had it; the `/ID[0]` the key is
/// made from; and what it leaves in the clear, the list the reader
/// decrypts by (see [`bytes`](crate::bytes), "What's encrypted"); and,
/// where its keys are made per object, the numbers the file lists free,
/// at which nothing new is written under it ([`Encryptor::free`]). It
/// gives out nothing of the key, its `Debug` neither; a clone clones the
/// handler, each key wiped when it's dropped (pdf-crypt's `Handler`).
///
/// A caller that lays a file out itself, as [`full`](super::full) does,
/// writes it encrypted as `full` does: through [`Encryptor::whole`], then
/// [`Encryptor::encrypting`]; or, where it renumbers its objects, through
/// [`Encryptor::renumbered`], [`Encryptor::whole`], then
/// [`Encryptor::encrypting_renumbered`], given the renumbering, which finds
/// the document's metadata stream in the numbers held and keeps it as its
/// number written. It writes no object at a number [`Encryptor::free`]
/// gives, and lists each free at its generation
/// ([`Encryptor::free_numbers`]); puts in no object stream an object
/// [`Encryptor::dictionary_object`] says is the encryption dictionary's;
/// and numbers no object but that one there, as what's there is left in
/// the clear.
#[derive(Clone)]
pub struct Encryptor {
    handler: Handler,
    list: List,
    dict: Dict,
    place: Place,
    first_id: Option<Vec<u8>>,
    /// The numbers the file it was read from lists free, each with the
    /// generation its entry gives, where a method of its makes keys per
    /// object (RC4, AES-128), from the number and generation (ISO 32000-2,
    /// 7.6.3, Algorithm 1): a new object there would be encrypted with a
    /// deleted object's key. Empty otherwise.
    free: BTreeMap<u32, u16>,
    /// Whether a method of its makes keys per object (RC4, AES-128), from
    /// the object's number and generation: then what's encrypted under it
    /// is right at its own number alone. AES-256's key is the file's
    /// (Algorithm 1.A), its IV beside its ciphertext.
    per_object: bool,
    /// The reading it was made of, whose objects it encrypts as the
    /// reading's file has them: none for a new file's.
    source: Option<SourceId>,
}

/// Where an encryption dictionary was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
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
    ///
    /// Where any of its methods makes keys per object, from the object's
    /// number and generation (RC4 and AES-128: its strings', its streams',
    /// a crypt filter's its dictionary has, or `/EFF`'s), it keeps the
    /// numbers the reading lists free, each with the generation it's to be
    /// used at next ([`Encryptor::free`]): a whole file or an update written
    /// under it refuses an object at one ([`Error::Free`]), as its key
    /// would be a deleted object's, whose ciphertext may still be in the
    /// file, or in the earlier file a whole save replaces; and a whole file
    /// lists each free at that generation, so that a tool that follows its
    /// free list doesn't take one at a deleted object's key.
    pub fn of<S: ByteSource>(reading: &Reading<S>) -> Result<Option<Encryptor>, Error> {
        match (reading.encryption(), reading.handler(), reading.stored()) {
            (Encryption::None, ..) => Ok(None),
            (Encryption::Unsupported(why), ..) => Err(Error::Unsupported(why.clone())),
            (Encryption::Opened { .. }, Some(handler), Some(stored)) => {
                let per_object = per_object(handler, &stored.dict);
                let free = match per_object {
                    true => (reading.entries())
                        .filter(|&(n, _, entry)| n != 0 && entry == XrefEntry::Free)
                        .map(|(n, generation, _)| (n, generation))
                        .collect(),
                    false => BTreeMap::new(),
                };
                Ok(Some(Encryptor {
                    handler: handler.clone(),
                    list: reading.list().clone(),
                    dict: stored.dict.clone(),
                    place: stored.at.map_or(Place::Direct, Place::At),
                    first_id: Some(stored.id.clone()),
                    free,
                    per_object,
                    source: Some(reading.id()),
                }))
            }
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
            free: BTreeMap::new(),
            per_object: false,
            source: None,
        })
    }

    /// Whether objects may go in object streams under it: a container is
    /// encrypted as a stream, whole, its members' strings with it, so not
    /// where streams are left in the clear (`/StmF /Identity`) and strings
    /// aren't, whose strings would then be in the clear; nor where either's
    /// method isn't one.
    pub fn packs(&self) -> bool {
        crypt::packs(self.handler.streams(), self.handler.strings())
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

    /// Where the file it was read from has its dictionary.
    pub(super) fn place(&self) -> Place {
        self.place
    }

    /// It, for a file written whole that writes its dictionary at `at`, or
    /// direct where that's none, every section of it naming that one: an
    /// object at a number the file it was read from named `/Encrypt` by, in
    /// its trailer or an earlier section's, is encrypted as any is, as a
    /// reader of the file written decrypts it, where it would otherwise be
    /// left as written; what the dictionary refers to is still left in the
    /// clear. A file written whole under it is encrypted through this.
    pub fn whole(&self, at: Option<ObjectId>) -> Cow<'_, Encryptor> {
        let list = &self.list;
        let named = |id: &ObjectId| list.sections.contains(id) && Some(*id) != at;
        if !list.dictionaries.iter().any(named) {
            return Cow::Borrowed(self);
        }
        let mut whole = self.clone();
        whole.list.dictionaries.retain(|id| !named(id));
        whole.list.sections = at.into_iter().collect();
        Cow::Owned(whole)
    }

    /// It, for a file whose objects are written at the numbers `to` gives
    /// them ([`Options::renumber`](super::Options::renumber)), in those
    /// numbers: what it leaves in the clear by number, each encryption
    /// dictionary and what one refers to, at the number `to` gives it, one
    /// it gives none left out, as it isn't written; where its dictionary
    /// is, at the number `to` gives it, or, where it gives none, nowhere
    /// yet, as a new file's is, a whole file then writing it past every
    /// object; and its dictionary's references as `to` names them, null
    /// where it names none. Its free numbers stay as they are: they're
    /// numbers of the file written too, where nothing is written under
    /// keys made per object. Where `to` moves none of those, it's itself.
    ///
    /// A whole file written under a renumbering is encrypted through this,
    /// then [`Encryptor::whole`]; and so is an update after it, given
    /// objects at the numbers written ([`Written::base`](super::Written::base)),
    /// whose place and dictionary are then the file's, as an update's
    /// encryptor must have ([`Error::Unmatched`]), where the whole write
    /// put the dictionary where `to` puts its number: not where `to` names
    /// it none, or [`Options::encrypt_at`](super::Options::encrypt_at)
    /// moved it. An object's strings and data are encrypted under the number
    /// it's written at, the document's metadata stream found from the
    /// objects given and kept as its number written. An update's own
    /// renumbering doesn't go through this: the update doesn't write the
    /// dictionary again, so it encrypts by the list as it is, in its file's
    /// numbers, which its renumbering moves nothing of
    /// ([`Error::Moved`]).
    pub fn renumbered<N: Renumber + ?Sized>(&self, to: &N) -> Cow<'_, Encryptor> {
        let list = &self.list;
        let numbers = list.dictionaries.iter().chain(&list.sections);
        let place = match self.place {
            Place::At(at) => to.renumber(at).map_or(Place::New, Place::At),
            place => place,
        };
        let mut dict = Object::from(self.dict.clone());
        let named = dict.renumber(|id| to.renumber(id));
        if !named
            && place == self.place
            && numbers.into_iter().all(|&id| to.renumber(id) == Some(id))
        {
            return Cow::Borrowed(self);
        }
        let mut renumbered = self.clone();
        let translated =
            |ids: &HashSet<ObjectId>| ids.iter().filter_map(|&id| to.renumber(id)).collect();
        renumbered.list.dictionaries = translated(&list.dictionaries);
        renumbered.list.sections = translated(&list.sections);
        renumbered.place = place;
        if named {
            renumbered.dict = match dict.into_value() {
                Value::Dict(dict) => dict,
                _ => unreachable!("a dictionary renumbered is one"),
            };
        }
        Cow::Owned(renumbered)
    }

    /// The reading it was made of ([`Encryptor::of`]), whose objects a
    /// write under it may copy as their bytes: none for a new file's.
    pub(super) fn read_in(&self) -> Option<SourceId> {
        self.source
    }

    /// Whether any method of its makes keys per object (RC4, AES-128: its
    /// strings', its streams', a crypt filter's its dictionary has, or
    /// `/EFF`'s), so that what it encrypted is right at its own number
    /// alone, which a copy at another number isn't.
    pub(super) fn per_object(&self) -> bool {
        self.per_object
    }

    /// Where the file it was read from lists number `n` free, under keys
    /// made per object (RC4, AES-128): the generation its entry gives, the
    /// one the number is to be used at next. Nothing new is written there
    /// under it, as it would be encrypted with a deleted object's key
    /// ([`Error::Free`]), and a file written whole lists it free at that
    /// generation. None for any other number, and every number under keys
    /// that aren't made per object.
    pub fn free(&self, n: u32) -> Option<u16> {
        self.free.get(&n).copied()
    }

    /// Each number [`Encryptor::free`] gives a generation for, with it,
    /// ascending.
    pub fn free_numbers(&self) -> impl Iterator<Item = (u32, u16)> + '_ {
        self.free.iter().map(|(&n, &generation)| (n, generation))
    }

    /// Whether object `id` is an encryption dictionary, or what one's
    /// entries refer to, by number, which a reader reads before it has a
    /// key: left in the clear, and never put in an object stream, which is
    /// encrypted. [`packable`](super::packable) tells an encryption
    /// dictionary by its shape alone.
    pub fn dictionary_object(&self, id: ObjectId) -> bool {
        self.list.dictionaries.contains(&id)
    }

    /// For an update under the renumbering `to`, whose objects are each
    /// given at, and written at, as `written` pairs them: the least of the
    /// objects [`Encryptor::dictionary_object`] says are an encryption
    /// dictionary's, by number in the file, that `to` moves, or names none,
    /// or that `written` puts another object's number on. None where
    /// there's none, and with no renumbering. A whole file writes the
    /// dictionary again, in the numbers written ([`Encryptor::renumbered`]);
    /// an update doesn't, so the file names those where they are, and its
    /// reader reads what's there in the clear.
    pub(super) fn moved(
        &self,
        to: Numbering<'_>,
        written: impl IntoIterator<Item = (ObjectId, ObjectId)>,
    ) -> Option<ObjectId> {
        to.0?;
        let dictionaries = &self.list.dictionaries;
        let moved = (dictionaries.iter().copied()).filter(|&id| to.at(id) != Some(id));
        let mut numbers: BTreeMap<u32, ObjectId> = BTreeMap::new();
        for &id in dictionaries {
            let least = numbers.entry(id.0).or_insert(id);
            *least = id.min(*least);
        }
        let onto = (written.into_iter())
            .filter(|(given, at)| given != at)
            .filter_map(|(_, at)| numbers.get(&at.0).copied());
        moved.chain(onto).min()
    }

    /// For a whole file under the renumbering `to`, itself the encryptor as
    /// given and `now` it in the numbers written ([`Encryptor::renumbered`],
    /// then [`Encryptor::whole`]), whose objects are each given at, and
    /// written at, as `written` pairs them: the least of the objects
    /// [`Encryptor::dictionary_object`] says are an encryption dictionary's,
    /// by number in the file, at whose number written `written` puts an
    /// object that isn't one of those. `now` leaves what's there in the
    /// clear, as the reader of the file written reads it, which that
    /// object's strings would then be. None where there's none, and with
    /// no renumbering, which puts no object at another's number.
    pub(super) fn shared(
        &self,
        now: &Encryptor,
        to: Numbering<'_>,
        written: impl IntoIterator<Item = (ObjectId, ObjectId)>,
    ) -> Option<ObjectId> {
        to.0?;
        let (then, now) = (&self.list.dictionaries, &now.list.dictionaries);
        let shared = (written.into_iter())
            .filter(|(given, at)| now.contains(at) && !then.contains(given))
            .map(|(_, at)| at);
        // Each named by the least of those `to` puts there; as written,
        // where none is, which `now`, being them in the numbers written,
        // doesn't leave.
        let named = |at: ObjectId| {
            let there = then.iter().copied().filter(|&id| to.at(id) == Some(at));
            there.min().unwrap_or(at)
        };
        shared.map(named).min()
    }

    /// The numbers [`Encryptor::dictionary_object`] says are an encryption
    /// dictionary's, or what one refers to: what's at them is left in the
    /// clear, so the writer numbers no object of its own there.
    pub(super) fn dictionary_numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.list.dictionaries.iter().map(|id| id.0)
    }

    /// Whether `/EncryptMetadata false` leaves the document's metadata
    /// stream's data in the clear under it, which the catalog's
    /// `/Metadata` says.
    pub(super) fn plain_metadata(&self) -> bool {
        self.list.unencrypted_metadata
    }

    /// What encrypts the objects of a file written under it: pdf-objects'
    /// encryptor, for its `indirect_with` and `indirect_stream_with`.
    /// `trailer` is the file's trailer, which says which stream is the
    /// document's metadata (its catalog's `/Metadata`), and `resolve` gives
    /// the objects a reference names: the metadata stream's way there, and a
    /// stream's `/Filter` and `/DecodeParms`, which say whether it has a
    /// crypt filter of its own. Both are in the numbers the objects are
    /// written at: a file whose objects are written at other numbers than
    /// they're held at is encrypted through
    /// [`Encryptor::encrypting_renumbered`]. Refused where `/EncryptMetadata
    /// false` leaves the document's metadata stream in the clear and
    /// `resolve` doesn't give the catalog, which says which stream that is
    /// ([`Error::Catalog`]), rather than encrypt it.
    pub fn encrypting<'a>(
        &'a self,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
    ) -> Result<Encrypting<'a>, Error> {
        self.encrypting_at(trailer, resolve, Numbering::NONE)
    }

    /// [`Encryptor::encrypting`], for a file whose objects are written at
    /// the numbers `to` gives them, as [`full`](super::full) writes one
    /// under [`Options::renumber`](super::Options::renumber): itself in the
    /// numbers written ([`Encryptor::renumbered`], then
    /// [`Encryptor::whole`]); `trailer` and `resolve` in the numbers the
    /// objects are held at, as the dictionary of a stream pdf-objects'
    /// writer asks it to encrypt is, its references not renumbered
    /// (`serialize::Encrypt`), so that a stream's own crypt filter is found
    /// through them. The document's metadata stream is found there, and
    /// kept as the number `to` writes it at, which pdf-objects' writer asks
    /// by: so `/EncryptMetadata false` leaves that one in the clear, and
    /// not the object `to` writes at the number it's held at. Refused as
    /// [`Encryptor::encrypting`] is.
    pub fn encrypting_renumbered<'a, N: Renumber + ?Sized>(
        &'a self,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
        to: &N,
    ) -> Result<Encrypting<'a>, Error> {
        self.encrypting_by(trailer, resolve, |id| to.renumber(id))
    }

    /// [`Encryptor::encrypting_renumbered`] by the renumbering a write is
    /// given, or none.
    pub(super) fn encrypting_at<'a>(
        &'a self,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
        to: Numbering<'_>,
    ) -> Result<Encrypting<'a>, Error> {
        self.encrypting_by(trailer, resolve, |id| to.at(id))
    }

    /// [`Encryptor::encrypting`], `trailer` and `resolve` in the numbers the
    /// objects are held at, `at` giving the number each is written at.
    fn encrypting_by<'a>(
        &'a self,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
        at: impl Fn(ObjectId) -> Option<ObjectId>,
    ) -> Result<Encrypting<'a>, Error> {
        let metadata = match self.list.unencrypted_metadata {
            true => {
                let root = trailer.get(b"Root").and_then(Object::as_reference);
                if let Some(root) = root.filter(|&root| resolve.resolve(root).is_none()) {
                    return Err(Error::Catalog(root));
                }
                let get: crypt::Get<'_, '_, Infallible> = &|id| Ok(resolve.resolve(id));
                let Ok(metadata) = crypt::metadata_stream(trailer, get);
                metadata.and_then(at)
            }
            false => None,
        };
        Ok(Encrypting {
            encryptor: self,
            resolve,
            metadata,
        })
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
    /// leaves its data in the clear, at the number it's written at.
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

impl<'a> Encrypting<'a> {
    /// Whether object `obj`, which the reading `read` was made of read at
    /// `id`, written here at `at`, is encrypted by this as that reading
    /// decrypted it: its strings by the same rule, the list asked of each
    /// number in its own file, and its stream's data, its own crypt filter
    /// followed through what's written here and through `there`, the
    /// reading's objects, there, and the reading's document metadata
    /// stream `metadata`. So, where `read`'s key is this one's, its bytes,
    /// written at the number they were read at, read back as the reading
    /// read them.
    pub(super) fn keeps<'o>(
        &self,
        read: &Encryptor,
        there: &dyn Fn(ObjectId) -> Option<&'o Object>,
        metadata: Option<ObjectId>,
        (id, at): (ObjectId, ObjectId),
        obj: &'o Object,
    ) -> bool
    where
        'a: 'o,
    {
        let (now, then) = (&self.encryptor.list, &read.list);
        if now.encrypted(at) != then.encrypted(id) {
            return false;
        }
        let Value::Stream(stream) = obj.value() else {
            return true;
        };
        let dict = &stream.dict;
        let here = |id| -> Option<&'o Object> { self.resolve.resolve(id) };
        let own_here = || Ok::<_, Infallible>(filter::crypt_filter(dict, Some(&here)));
        let own_there = || Ok::<_, Infallible>(filter::crypt_filter(dict, Some(there)));
        let Ok(here) = now.stream(at, dict, own_here, || Ok(self.metadata));
        let Ok(there) = then.stream(id, dict, own_there, || Ok(metadata));
        here == there
    }
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

/// Whether any method `handler` encrypts by makes keys per object (RC4,
/// AES-128): its strings', its streams', those of the crypt filters `dict`,
/// its encryption dictionary, has in `/CF`, or `/EFF`'s.
fn per_object(handler: &Handler, dict: &Dict) -> bool {
    let keyed = |m: &Method| matches!(m, Method::Rc4 | Method::Aes128);
    let filters = dict.get(b"CF").and_then(Object::as_dict);
    let named = (filters
        .into_iter()
        .flat_map(|cf| cf.iter().map(|(name, _)| name)))
    .chain(dict.get(b"EFF").and_then(Object::as_name));
    keyed(handler.strings())
        || keyed(handler.streams())
        || named.into_iter().any(|name| keyed(&handler.named(name)))
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
