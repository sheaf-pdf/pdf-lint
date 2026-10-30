//! What an encrypted file leaves in the clear, each rule once, over plain
//! values: the rules the reader decrypts a file's strings and streams' data
//! by, and the writer encrypts them by ([`write::Encrypting`]). Each, with
//! the clause of ISO 32000-2 that says so and the tests that hold it, is
//! listed in [`bytes`](crate::bytes) ("What's encrypted").
//!
//! [`write::Encrypting`]: crate::write::Encrypting
//!
//! - **A signature's `/Contents`,** and all in it ([`signature`], 7.6.2).
//! - **A cross-reference stream,** its dictionary's strings and its data
//!   ([`is_xref`], 7.5.8.2).
//! - **The encryption dictionaries,** each section's `/Encrypt` and the
//!   objects their entries refer to, by number (7.6.2): their strings
//!   ([`List::encrypted`]); a stream among them is neither plaintext nor
//!   said to be encrypted ([`List::stream`]).
//! - **A stream's data,** by its own `/Crypt` filter when it has one, else
//!   an embedded file's by `/EFF` from `/V` 4, else as `/StmF` says; the
//!   catalog's metadata stream's in the clear under `/EncryptMetadata
//!   false`, but where its own crypt filter encrypts it (7.6.4.2, 7.6.6,
//!   14.3.2; [`List::stream`]).
//! - **`/Identity`** leaves strings or streams as they are (7.6.6 and Table
//!   20; [`List::settle`]).
//! - **An object stream's objects' strings,** encrypted with its data,
//!   whole, not each with its object's key: in the clear where its data is
//!   and strings aren't ([`bares_strings`], 7.5.7, 7.6.2), which the writer
//!   doesn't pack in ([`packs`]), and the reader records ([`List::bares`]).

use std::collections::HashSet;

use pdf_crypt::{Handler, Method, Refused};
use pdf_objects::object::{Dict, Object, ObjectId, Value};

/// What a file leaves in the clear, as its encryption dictionary says: kept
/// by the reader, which makes it as it opens the file, and settles it once
/// it knows how the file's strings and streams are encrypted; and by the
/// writer's encryptor, a reading's or a new file's. Nothing is encrypted
/// in a file without an encryption dictionary.
#[derive(Clone, Default)]
pub(crate) struct List {
    /// Whether the file has an encryption dictionary, so that its streams
    /// may be encrypted, whatever its strings are.
    pub(crate) encrypts: bool,
    /// Whether its strings are encrypted, once it's settled: none are while
    /// it's opened, as its encryption dictionary and what that refers to
    /// are read.
    pub(crate) strings: bool,
    /// Whether its streams are, once it's settled.
    pub(crate) streams: bool,
    /// Whether its metadata stream is left as written by `/EncryptMetadata
    /// false` itself, whatever its streams are.
    pub(crate) unencrypted_metadata: bool,
    /// The objects the sections' trailers name as `/Encrypt`, and those
    /// their values refer to: an earlier update's encryption dictionary's
    /// strings aren't encrypted either.
    pub(crate) dictionaries: HashSet<ObjectId>,
    /// Those the sections' trailers name as `/Encrypt` (or, where the file
    /// was scanned, the trailer's), among `dictionaries`: what a whole file
    /// written of it, with one section, no longer names so.
    pub(crate) sections: HashSet<ObjectId>,
    /// How its embedded files are encrypted, by its dictionary's `/EFF`.
    pub(crate) embedded_files: Eff,
}

/// How an encrypted file's embedded files are encrypted, as its
/// encryption dictionary's `/EFF` says (ISO 32000-2, 7.6.6 and Table 20).
#[derive(Clone, Default)]
pub(crate) enum Eff {
    /// As its streams are: it has no `/EFF`, or a null, or is of a `/V`
    /// below 4, which has no crypt filters.
    #[default]
    AsStreams,
    /// By the crypt filter of this name, but one with a crypt filter of its
    /// own.
    Named(Vec<u8>),
    /// Its `/EFF` isn't a name, so it doesn't say how.
    Unsaid,
}

/// How a stream's data is encrypted, by the [`List`].
#[derive(PartialEq, Eq)]
pub(crate) enum Stream {
    /// It isn't: it's in the clear.
    Clear,
    /// It's an encryption dictionary's, or what one refers to, whose strings
    /// are left as written: its data is neither plaintext nor said to be
    /// encrypted.
    Dictionary,
    /// By the crypt filter of this name, its own or `/EFF`'s; by none, as
    /// the file's streams are (`/StmF`).
    By(Option<Vec<u8>>),
    /// It's an embedded file without a crypt filter of its own, and `/EFF`
    /// isn't a name, which doesn't say how.
    Unsaid,
}

impl List {
    /// What the encryption dictionary whose values are `values` leaves in
    /// the clear, its security handler `opened` or refused: its strings and
    /// its streams where no crypt filter encrypts them, by the handler's
    /// methods, or, where it doesn't open, by the dictionary's `/StrF` and
    /// `/StmF`, `/Identity` by default from `/V` 4, below which there are no
    /// crypt filters (ISO 32000-2, 7.6.6 and Table 20); and its metadata
    /// stream by `/EncryptMetadata false`, which counts from `/V` 4 too
    /// (7.6.4.2). Opening a file and unlocking one both settle it.
    pub(crate) fn settle(
        &mut self,
        values: &pdf_crypt::Dictionary,
        opened: Result<&Handler, &Refused>,
    ) {
        // Neither, when no crypt filter encrypts them (from /V 4), whatever
        // the handler.
        let identity = |f: &Option<Vec<u8>>| f.as_deref().is_none_or(|f| f == b"Identity");
        self.strings = match opened {
            Ok(handler) => !plain(handler.strings()),
            Err(_) => !(values.v >= 4 && identity(&values.str_f)),
        };
        self.streams = match opened {
            Ok(handler) => !plain(handler.streams()),
            Err(_) => !(values.v >= 4 && identity(&values.stm_f)),
        };
        self.unencrypted_metadata = match opened {
            Ok(handler) => !handler.encrypts_metadata(),
            Err(_) => values.v >= 4 && values.encrypt_metadata == Some(false),
        };
    }

    /// Settled where its encryption dictionary isn't one, or doesn't read:
    /// nothing says what's in the clear, so its strings and streams are
    /// taken as encrypted.
    pub(crate) fn undescribed(&mut self) {
        self.strings = true;
        self.streams = true;
    }

    /// Whether object `id`'s strings are encrypted: the file's are, and it
    /// isn't an encryption dictionary or what one refers to (ISO 32000-2,
    /// 7.6.2).
    pub(crate) fn encrypted(&self, id: ObjectId) -> bool {
        self.strings && !self.dictionaries.contains(&id)
    }

    /// Whether the catalog's metadata stream's data is in the clear: in a
    /// file that doesn't encrypt, or whose streams aren't encrypted, or by
    /// `/EncryptMetadata false` (ISO 32000-2, 7.6.4.2 and 14.3.2).
    pub(crate) fn plain_metadata(&self) -> bool {
        !self.encrypts || !self.streams || self.unencrypted_metadata
    }

    /// How the data of stream `id`, whose dictionary is `dict`, is
    /// encrypted: as the crypt filter its own `/Crypt` filter names says,
    /// `own`, when it has one, else as the file's streams are (ISO 32000-2,
    /// 7.6.6); an embedded file's (`/Type /EmbeddedFile`) without one of its
    /// own, by the crypt filter the encryption dictionary's `/EFF` names
    /// (7.6.6 and Table 20). In the clear: in a file that doesn't encrypt; a
    /// cross-reference stream's (7.5.8.2); and the catalog's metadata
    /// stream's, the one `metadata` gives, under `/EncryptMetadata false`
    /// (7.6.4.2 and 14.3.2), but where its own crypt filter encrypts it,
    /// which then does as it does any stream's (7.6.6). `own` and
    /// `metadata` are asked only when the rule needs them, in that order,
    /// as each may read the file.
    pub(crate) fn stream<E>(
        &self,
        id: ObjectId,
        dict: &Dict,
        own: impl FnOnce() -> Result<Option<Vec<u8>>, E>,
        metadata: impl FnOnce() -> Result<Option<ObjectId>, E>,
    ) -> Result<Stream, E> {
        if !self.encrypts || is_xref(dict) {
            return Ok(Stream::Clear);
        }
        if self.dictionaries.contains(&id) {
            return Ok(Stream::Dictionary);
        }
        let own = own()?;
        // The metadata stream is left as written, but where its own crypt
        // filter encrypts it, as it does any stream's.
        if self.unencrypted_metadata
            && own.as_deref().is_none_or(|name| name == b"Identity")
            && metadata()? == Some(id)
        {
            return Ok(Stream::Clear);
        }
        let embedded = || dict.get(b"Type").and_then(Object::as_name) == Some(b"EmbeddedFile");
        Ok(match (own, &self.embedded_files) {
            (None, Eff::Named(name)) if embedded() => Stream::By(Some(name.clone())),
            (None, Eff::Unsaid) if embedded() => Stream::Unsaid,
            (own, _) => Stream::By(own),
        })
    }

    /// Whether an object stream whose own `/Crypt` filter names `own`, none
    /// where it has none, leaves the strings of the objects in it in the
    /// clear ([`bares_strings`]): its data encrypted as that crypt filter
    /// says, else as the file's streams are (7.6.6), by `handler`'s methods
    /// where the file opened. Where it didn't, as its encryption dictionary
    /// says, which settled the list, and as the reader reads the stream: in
    /// the clear by its own crypt filter `/Identity`, or without one where
    /// the file's streams are.
    pub(crate) fn bares(&self, handler: Option<&Handler>, own: Option<&[u8]>) -> bool {
        match handler {
            Some(handler) => {
                let named;
                let data = match own {
                    Some(name) => {
                        named = handler.named(name);
                        &named
                    }
                    None => handler.streams(),
                };
                bares_strings(data, handler.strings())
            }
            None => self.strings && own.map_or(!self.streams, |name| name == b"Identity"),
        }
    }
}

/// Whether what's encrypted by `method` is left in the clear: `/Identity`,
/// or a crypt filter whose method is `/None`, which leaves it as it is.
fn plain(method: &Method) -> bool {
    matches!(method, Method::Identity | Method::None)
}

/// Whether an object stream whose data is encrypted by `data` leaves its
/// objects' strings in the clear where strings are encrypted by
/// `strings`: an object in one isn't encrypted itself but with its
/// stream's data, whole (ISO 32000-2, 7.5.7, 7.6.2), so where that's in
/// the clear and strings aren't, theirs are.
pub(crate) fn bares_strings(data: &Method, strings: &Method) -> bool {
    plain(data) && !plain(strings)
}

/// Whether a writer may put objects in object streams in a file whose
/// streams are encrypted by `streams` and strings by `strings`
/// ([`write::Encryptor::packs`](crate::write::Encryptor::packs)): not where
/// a container it encrypts as a stream, whole, would leave its objects'
/// strings in the clear ([`bares_strings`]); nor where either's method
/// isn't one.
pub(crate) fn packs(streams: &Method, strings: &Method) -> bool {
    let known = |m: &Method| !matches!(m, Method::Unknown(_));
    known(streams) && known(strings) && !bares_strings(streams, strings)
}

/// Whether `dict` is a signature's, whose `/Contents`, and all in it, isn't
/// encrypted (ISO 32000-2, 7.6.2): a dictionary with `/ByteRange` and a
/// string `/Contents`, of `/Type /Sig`, `/DocTimeStamp` or none.
pub(crate) fn signature(dict: &Dict) -> bool {
    let contents = matches!(
        dict.get(b"Contents").map(|c| c.value()),
        Some(Value::String(..))
    );
    let typed =
        (dict.get(b"Type")).is_none_or(|t| matches!(t.as_name(), Some(b"Sig" | b"DocTimeStamp")));
    dict.has(b"ByteRange") && contents && typed
}

/// Whether `dict` is a cross-reference stream's, which is never encrypted,
/// its dictionary's strings nor its data (ISO 32000-2, 7.5.8.2).
pub(crate) fn is_xref(dict: &Dict) -> bool {
    dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef")
}

/// What a read of the objects on the way to the metadata gives: the object
/// a reference refers to, none for no object that reads, or why the file
/// doesn't read.
pub(crate) type Get<'g, 'o, E> = &'g dyn Fn(ObjectId) -> Result<Option<&'o Object>, E>;

/// `o` with its references followed, as many as an object's way to the
/// catalog may take.
pub(crate) fn follow<'o, E>(
    get: Get<'_, 'o, E>,
    mut o: &'o Object,
) -> Result<Option<&'o Object>, E> {
    for _ in 0..8 {
        match o.value() {
            Value::Ref(id) => match get(*id)? {
                Some(to) => o = to,
                None => return Ok(None),
            },
            _ => return Ok(Some(o)),
        }
    }
    Ok(None)
}

/// Which object the catalog's metadata stream is (ISO 32000-2, 14.3.2), of
/// the file whose trailer is `trailer`: the last the references from the
/// catalog's `/Metadata` lead to, followed as [`follow`] follows them, when
/// it's a stream. `get` gives the object a reference refers to.
pub(crate) fn metadata_stream<'o, E>(
    trailer: &'o Dict,
    get: Get<'_, 'o, E>,
) -> Result<Option<ObjectId>, E> {
    let Some(root) = trailer.get(b"Root") else {
        return Ok(None);
    };
    let Some(Value::Dict(catalog)) = follow(get, root)?.map(Object::value) else {
        return Ok(None);
    };
    let Some(mut o) = catalog.get(b"Metadata") else {
        return Ok(None);
    };
    let mut last = None;
    for _ in 0..8 {
        match o.value() {
            Value::Ref(id) => match get(*id)? {
                Some(to) => (o, last) = (to, Some(*id)),
                None => return Ok(None),
            },
            Value::Stream(_) => return Ok(last),
            _ => return Ok(None),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every method a crypt filter can have, and one that isn't one.
    fn methods() -> [Method; 6] {
        [
            Method::Identity,
            Method::None,
            Method::Rc4,
            Method::Aes128,
            Method::Aes256,
            Method::Unknown(b"Nonesuch".to_vec()),
        ]
    }

    /// An object stream's data in the clear, `/Identity` or `/CFM /None`,
    /// leaves its objects' strings in the clear where strings aren't; data
    /// encrypted, or strings in the clear too, doesn't.
    #[test]
    fn an_object_stream_bares_strings_where_its_data_is_in_the_clear_and_strings_arent() {
        let plain = |m: &Method| matches!(m, Method::Identity | Method::None);
        for data in methods() {
            for strings in methods() {
                assert_eq!(
                    bares_strings(&data, &strings),
                    plain(&data) && !plain(&strings),
                    "{data:?} {strings:?}"
                );
            }
        }
        assert!(bares_strings(&Method::Identity, &Method::Aes128));
        assert!(bares_strings(&Method::None, &Method::Rc4));
        assert!(bares_strings(
            &Method::Identity,
            &Method::Unknown(b"X".to_vec())
        ));
        assert!(!bares_strings(&Method::Aes256, &Method::Aes256));
        assert!(!bares_strings(&Method::Aes256, &Method::Identity));
        assert!(!bares_strings(&Method::Identity, &Method::None));
        assert!(!bares_strings(
            &Method::Unknown(b"X".to_vec()),
            &Method::Rc4
        ));
    }

    /// What a writer packs is as it was before it was said through
    /// `bares_strings`, over every pair of methods: the old expression
    /// beside.
    #[test]
    fn what_a_writer_packs_is_as_before_over_every_pair_of_methods() {
        let before = |streams: &Method, strings: &Method| {
            let plain = |m: &Method| matches!(m, Method::Identity | Method::None);
            let known = |m: &Method| !matches!(m, Method::Unknown(_));
            known(streams) && known(strings) && (!plain(streams) || plain(strings))
        };
        for streams in methods() {
            for strings in methods() {
                assert_eq!(
                    packs(&streams, &strings),
                    before(&streams, &strings),
                    "{streams:?} {strings:?}"
                );
            }
        }
    }

    /// Where the file didn't open, an object stream bares its objects'
    /// strings as the list says, where the file's strings are encrypted:
    /// by its own crypt filter, `/Identity` in the clear, else as the
    /// file's streams are. Where it opened, by the handler's methods, which
    /// the reader's test of what it records holds
    /// (`an_object_stream_left_in_the_clear_where_strings_arent_is_recorded`).
    #[test]
    fn an_object_stream_bares_strings_by_its_own_crypt_filter_else_the_files_streams() {
        let list = |strings: bool, streams: bool| List {
            encrypts: true,
            strings,
            streams,
            ..List::default()
        };
        // Not opened: by the list.
        let cases = [
            (list(true, false), None, true),
            (list(true, false), Some(&b"StdCF"[..]), false),
            (list(true, true), None, false),
            (list(true, true), Some(&b"Identity"[..]), true),
            (list(false, false), None, false),
            (list(false, true), Some(&b"Identity"[..]), false),
        ];
        for (list, own, bares) in cases {
            assert_eq!(list.bares(None, own), bares, "{own:?}");
        }
    }
}
