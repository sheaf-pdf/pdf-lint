//! A reader of a file's bytes, on [`pdf_tokenizer`] and pdf-objects'
//! [`Parser`]: every value, key and stream's data with where it was
//! written ([`Span`]), dictionaries as written with any key written twice,
//! and every choice it had to make a [`Warning`]. It reads the header,
//! `startxref`, the cross-reference tables and streams along their `/Prev`
//! chain, and each object at the offset they give, a stream's data by its
//! `/Length` when that ends at `endstream`. Where the sections don't read,
//! it finds the objects by scanning the file, and says so. An object that
//! doesn't read says why ([`Unread`]): it doesn't parse, its offset holds
//! another, or the object stream it's in doesn't decode ([`Undecoded`],
//! its filters [`Unfiltered`]) or doesn't say where it is ([`Unindexed`]).
//!
//! A file is read one way, by a [`Reading`]: opened, its header,
//! cross-reference sections and trailer read and its security handler
//! opened, then each object read when it's first asked for, and each
//! object stream decoded when an object in it is. Read whole, every object
//! is read so, in order, into a [`Document`] ([`read`],
//! [`Reading::into_document`]); read on demand, on any thread, only what's
//! asked for is. Each object read records where it was found
//! ([`Record`]). Its bytes are in memory, or a file is read by position
//! ([`FileBytes`], or any [`ByteSource`] that doesn't hold its bytes), a
//! window at a time from where each read starts, larger while what's read
//! runs to the window's end, so that each read reads what it would of the
//! whole file.
//!
//! It decodes cross-reference streams and object streams from their own
//! data, through [`pdf_filters`]: a stream up to [`Limits::stream`] bytes,
//! and a reading's [`Limits::budget`] in all. An encrypted file it opens
//! with the passwords it's given ([`read_opened`]) through [`pdf_crypt`]'s
//! standard security handler: its object streams decrypted before they're
//! decoded, its strings decrypted where they are, a signature's
//! `/Contents` left as it is. An encrypted file it doesn't open
//! ([`Document::encryption`]) has its strings [`Str::Encrypted`], and its
//! object streams unread, saying why. An object read from an object stream
//! is placed in the stream's decoded data ([`Span::stream`]), which the
//! [`Document`] keeps. A reading hands out any stream's data when it's
//! asked for, decrypted as an object stream's is ([`Reading::data`]), and
//! decoded through its filters ([`Reading::decoded`]).
//!
//! **What's encrypted.** In an encrypted file it opens, what the reader
//! decrypts and what it leaves as written, each with the clause of
//! ISO 32000-2 that says so and the tests that hold it:
//!
//! - **Every string of an object read at its offset** is decrypted with
//!   the object's key, made from its number and generation, and stays
//!   where it was written; a stream's dictionary's strings too (7.6.2,
//!   7.6.3). A string that doesn't decrypt cleanly is a warning, and what
//!   decrypted is kept. Held by
//!   `each_kind_of_encryption_decrypts_its_strings_where_they_are`,
//!   `a_streams_dictionary_strings_decrypt_but_its_data_is_left_as_written`
//!   and `strings_that_dont_decrypt_cleanly_are_said_once_an_object_and_a_fault`.
//!   But for these, which are left as written:
//!   - **The encryption dictionary's:** the trailer's `/Encrypt`, every
//!     earlier cross-reference section's along `/Prev` (an earlier
//!     update's dictionary), and the objects its entries refer to, where
//!     the reader reads them to open the file (7.6.2). Held by
//!     `an_encrypted_files_strings_are_ciphertext_but_its_encryption_dictionarys`,
//!     `an_earlier_updates_encryption_dictionary_isnt_decrypted_either`
//!     and `what_an_encryption_dictionarys_values_refer_to_is_read_as_written`.
//!   - **A signature's `/Contents`,** in a dictionary with `/ByteRange` and
//!     a string `/Contents` whose `/Type` is `/Sig`, `/DocTimeStamp` or
//!     none (7.6.2). Held by
//!     `a_signatures_contents_isnt_decrypted_whatever_its_type` and
//!     `each_kind_of_encryption_decrypts_its_strings_where_they_are`.
//!   - **A cross-reference stream's,** read as the trailer or as an object
//!     (7.5.8.2). Held by
//!     `a_cross_reference_streams_strings_are_read_as_written_in_a_file_that_opens`.
//! - **An object in an object stream** isn't decrypted itself: it's read
//!   from the stream's decrypted data, its strings as they are there
//!   (7.5.7, 7.6.2). Held by
//!   `an_object_streams_own_crypt_filter_says_how_its_data_decrypts` and
//!   `an_encrypted_files_object_streams_decrypt_by_the_reader_as_through_qpdf`.
//! - **An object stream's data** is decrypted before it's decoded: by the
//!   stream's own `/Crypt` filter when it has one, the one its `/Name`
//!   names (`/Identity` when it names none), else as the encryption
//!   dictionary's `/StmF` says (7.6.6). Held by
//!   `an_object_streams_own_crypt_filter_says_how_its_data_decrypts`,
//!   `an_encrypted_object_stream_is_decrypted_and_decoded_without_the_decoder`
//!   and `a_file_whose_strings_and_streams_arent_encrypted_reads_whole_without_its_password`.
//! - **A cross-reference stream's data** is read as written, never
//!   decrypted (7.5.8.2), and **no other stream's data is read** as the
//!   file is read: it stays where it was written
//!   ([`Data::At`](pdf_objects::object::Data::At)), encrypted. Held by
//!   `a_cross_reference_streams_strings_are_read_as_written_in_a_file_that_opens`
//!   and `a_streams_dictionary_strings_decrypt_but_its_data_is_left_as_written`.
//!   A reading hands out no bytes of an encrypted file as written
//!   ([`Reading::bytes`]), so they can't pass for plaintext. Held by
//!   `an_encrypted_readings_bytes_are_refused_but_its_object_streams`.
//! - **A stream's data handed out** ([`Reading::data`], and decoded,
//!   [`Reading::decoded`]) is decrypted as an object stream's is: by the
//!   stream's own `/Crypt` filter when it has one, else as `/StmF` says
//!   (7.6.6). Data that doesn't decrypt cleanly is an error holding what
//!   did ([`Error::Decrypt`]). Held by
//!   `each_kind_of_encryption_hands_out_each_streams_data_decrypted`,
//!   `a_streams_own_crypt_filter_says_how_its_data_is_handed_out` and
//!   `a_streams_data_that_doesnt_decrypt_cleanly_is_an_error_holding_what_did`.
//!   But for these:
//!   - **A cross-reference stream's** is handed out as written (7.5.8.2).
//!     Held by `a_cross_reference_streams_data_is_handed_out_as_written`.
//!   - **The catalog's metadata stream's,** under `/EncryptMetadata
//!     false`, is handed out as written, whether the file opens or not, as
//!     the [`Document`]'s metadata is (7.6.4.2 and 14.3.2). Held by
//!     `metadata_not_encrypted_is_handed_out_as_written_whether_the_file_opens_or_not`.
//!     But where its own `/Crypt` filter names one that encrypts it, it's
//!     refused, decrypted by that or not ([`Error::Encrypted`]). Held by
//!     `metadata_not_encrypted_whose_own_crypt_filter_encrypts_it_is_refused`.
//!   - **In a file no password opened,** each is refused, as it may be
//!     ciphertext ([`Error::Locked`]); but in one whose streams aren't
//!     encrypted ([`Encryption::Plain`]), handed out as written, unless the
//!     stream's own `/Crypt` filter encrypts it. Held by
//!     `a_locked_files_streams_are_refused` and
//!     `a_plain_files_streams_are_handed_out_as_written_but_one_its_own_crypt_filter_encrypts`.
//!   - **In a file whose security handler the reader can't open,** each is
//!     refused ([`Error::Unsupported`]). Held by
//!     `an_unsupported_handlers_streams_are_refused`.
//!   - **An embedded file's** (`/Type /EmbeddedFile`), in a file whose
//!     encryption dictionary has an `/EFF`, the crypt filter embedded files
//!     are encrypted by, is refused, as the reader doesn't apply it
//!     ([`Error::EmbeddedFile`]; 7.6.6 and Table 20). Held by
//!     `an_embedded_files_data_under_eff_is_refused`.
//!   - **A stream the encryption dictionary refers to,** among the objects
//!     whose strings are left as written, is refused
//!     ([`Error::Encrypted`]). Held by
//!     `a_stream_the_encryption_dictionary_refers_to_is_refused`.
//! - **`/Identity`:** strings are left as written when `/StrF` is
//!   `/Identity`, as it is by default from `/V` 4, and object streams'
//!   data when `/StmF` is (7.6.6 and Table 20). A file whose strings and
//!   streams both are reads whole though no password it's given opens it
//!   ([`Encryption::Plain`]), but for an object stream whose own `/Crypt`
//!   filter encrypts it. Held by
//!   `an_encrypted_files_strings_are_ciphertext_but_its_encryption_dictionarys`,
//!   `an_encrypted_object_stream_is_decrypted_and_decoded_without_the_decoder`
//!   and `a_file_whose_strings_and_streams_arent_encrypted_reads_whole_without_its_password`.
//! - **Metadata under `/EncryptMetadata false`,** which counts from `/V`
//!   4: the catalog's metadata stream's data is in plain view
//!   ([`Document::metadata`]), whether the file opens or not, and its
//!   dictionary's strings are decrypted as any object's are (7.6.4.2 and
//!   14.3.2). Held by
//!   `metadata_not_encrypted_is_in_plain_view_whether_the_file_opens_or_not`
//!   and `metadata_not_encrypted_has_its_dictionarys_strings_decrypted_all_the_same`.
//! - **The trailer** isn't an object: its strings, its `/ID` among them,
//!   are read as written (7.5.5, 7.6.2). An object its `/ID` refers to,
//!   though the `/ID` should be direct, is left as written too, as what
//!   the encryption dictionary's entries refer to is. Held by
//!   `the_trailers_id_is_read_as_written_in_a_file_that_opens`.
//!
//! In a file it doesn't open, the encryption dictionary's strings, a
//! cross-reference stream's and the trailer's are read as written all the
//! same. Held by
//! `an_encrypted_files_strings_are_ciphertext_but_its_encryption_dictionarys`.
//! A reading read on demand decrypts what the whole read does, each object
//! as it's first read. Held by
//! `a_reading_on_demand_is_the_whole_read_in_any_order_on_any_thread`.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use pdf_tokenizer::{Kind, Token, Tokenizer, is_delimiter, is_white_space};

use pdf_crypt::{Handler, Method};
use pdf_filters::{Fault, Filter, Layout, Predictor};
use pdf_objects::object::{Dict, Entry, Object, ObjectId, Span, Str, Value};
use pdf_objects::parse::{self, Body, Parser, Sink};
use pdf_objects::version::Version;

use crate::filter;
pub use crate::filter::Unfiltered;
use crate::source::{
    DataExtent, Document, Encryption, Facts, Origin, Record, Trailer, Unread, Update, Warning,
    XrefKind,
};

mod file;
mod limits;
mod reading;
mod window;

#[cfg(any(unix, windows))]
pub use file::FileBytes;
pub use file::Identity;
pub use limits::{Limits, Options};
pub use reading::{ByteSource, Error, Reading, SourceId};
use window::{Bytes, InMemory, Window};

// How deeply arrays and dictionaries may nest in an object is the
// parser's, pdf-objects'.
pub use pdf_objects::parse::NESTING;

/// How many cross-reference sections the `/Prev` chain may have, unless a
/// reading is given other [`Limits`].
pub const SECTIONS: usize = 1024;

/// How many bytes a stream the reader decodes may decode to, unless a
/// reading is given other [`Limits`]; more is cut off there.
pub const STREAM: usize = 256 << 20;

/// How many bytes the streams the reader decodes in a reading of a file
/// may decode to in all, unless it's given other [`Limits`]; more is cut
/// off. (The file read again as of an earlier section, by [`read_from`],
/// is a reading of its own.)
pub const BUDGET: usize = 1 << 30;

/// What a cross-reference section says of an object number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum XrefEntry {
    /// In use, written at `offset`.
    InUse {
        /// Where its `n g obj` starts.
        offset: usize,
        /// Its generation number.
        generation: u16,
    },
    /// The `index`th object of object stream `stream`.
    Compressed {
        /// The object stream's number.
        stream: u32,
        /// Which of the stream's objects it is, from 0.
        index: u32,
    },
    /// Free: the number isn't in use.
    #[non_exhaustive]
    Free,
}

/// Why an object stream's data doesn't decode, for each object in it
/// ([`Unread::Container`]).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Undecoded {
    /// The object its number names isn't a stream.
    #[error("it isn't a stream")]
    NotAStream,
    /// Its `/Filter` or `/DecodeParms` are ones the reader doesn't take.
    #[error("{0}")]
    Filters(Unfiltered),
    /// Its own `/Crypt` filter, named `filter`, encrypts it, in an
    /// encrypted file that no password the reader was given opens: its
    /// password is needed.
    #[error(
        "its own /Crypt filter, /{}, encrypts it, and no password the reader was given opens the file",
        String::from_utf8_lossy(.filter)
    )]
    Encrypted {
        /// The crypt filter's name.
        filter: Vec<u8>,
    },
    /// Its data doesn't decrypt, and nothing of it did.
    #[error("its data doesn't decrypt: {0}")]
    Decrypt(pdf_crypt::Fault),
    /// Another decoder's words, where one decodes for the reader: only the
    /// tests' qpdf, behind the hidden `fixtures` feature, does. A reading
    /// never gives it.
    #[error("{0}")]
    Other(String),
}

/// Why an object stream's header doesn't say where its objects are (ISO
/// 32000-2, 7.5.7), for each object in it ([`Unread::ContainerHeader`]).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unindexed {
    /// The object its number names isn't a stream.
    #[error("it isn't a stream")]
    NotAStream,
    /// It isn't `/Type /ObjStm`.
    #[error("it isn't /Type /ObjStm")]
    NotAnObjectStream,
    /// Its `/N` or `/First` isn't a count.
    #[error("its /N or /First isn't a count")]
    Counts,
    /// Its header has fewer pairs of integers than its `/N` says.
    #[error("its header has {pairs} of its {count} pairs")]
    Pairs {
        /// How many it has.
        pairs: usize,
        /// How many its `/N` says.
        count: usize,
    },
    /// Its header gives a number that numbers no object.
    #[error("its header numbers no object")]
    Number,
    /// Its header gives an offset that isn't one.
    #[error("its header gives an offset that isn't one")]
    Offset,
}

/// What decodes what the reader can't: the object streams and strings of
/// an encrypted file it doesn't open. The reader asks twice at most: for
/// the object streams, then for the plaintext. Only the tests give one,
/// qpdf, to set the reader's decoding beside.
pub(crate) trait Decode {
    /// The data of each stream in `ids`, decoded (decompressed, and in an
    /// encrypted file decrypted), or why it can't be.
    fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>>;

    /// In an encrypted file, each object in `ids`, which hold strings in
    /// plain view, as the decoder reads it, its strings decrypted, or why it
    /// can't be: each string's plaintext is taken from where the string is
    /// in it. By default it gives none, and the strings stay ciphertext.
    fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
        let _ = ids;
        BTreeMap::new()
    }

    /// `stream`, which isn't encrypted, decoded from its bytes alone, when
    /// [`Decode::decode`] couldn't decode it by its number: a later update
    /// can give the number to another object, and a decoder that finds
    /// streams by number then finds that one; or the decoder can't read
    /// the file. The reader asks it only when it's made to ask the decoder
    /// for every stream, as the tests do to set the two side by side. By
    /// default, it can't be.
    fn decode_written(&self, stream: Written) -> Result<Vec<u8>, String> {
        let _ = stream;
        Err("nothing decodes a stream from its bytes".to_string())
    }
}

/// A stream as written: its `/Filter`'s and `/DecodeParms`'s values and its
/// data, each its bytes in the file.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Written<'a> {
    pub(crate) filter: &'a [u8],
    pub(crate) parms: Option<&'a [u8]>,
    pub(crate) data: &'a [u8],
}

/// Decoded data given beforehand, by stream.
impl Decode for BTreeMap<ObjectId, Vec<u8>> {
    fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
        (ids.iter())
            .map(|&id| {
                let data = self.get(&id).cloned();
                (
                    id,
                    data.ok_or_else(|| format!("no data for {} {} R", id.0, id.1)),
                )
            })
            .collect()
    }
}

/// The file `file` is, opened, when it's encrypted, by the empty password,
/// as [`read_opened`] opens it given none.
pub fn read(file: &[u8]) -> Document {
    read_as(file, None, true, &Options::DEFAULT, &[])
}

/// The file `file` is, opened, when it's encrypted, by the first of
/// `passwords` that opens it, each tried as the owner's and then the
/// user's (none, the empty password). Each is tried as the bytes given;
/// pdf-crypt's `passwords` gives the bytes a password typed as text may
/// have been set as.
pub fn read_opened(file: &[u8], passwords: &[&[u8]]) -> Document {
    read_as(file, None, true, &Options::DEFAULT, passwords)
}

/// The file as of the cross-reference section at byte `section`, as
/// [`read_from`] reads it, opened as [`read_opened`] opens it: an
/// update's [`Update::previous`] by its password.
pub fn read_from_opened(file: &[u8], passwords: &[&[u8]], section: usize) -> Document {
    read_as(file, None, true, &as_of(section), passwords)
}

/// The file `file` is, with `decoder` decoding what isn't in plain view.
#[cfg(any(test, feature = "fixtures"))]
pub(crate) fn read_with(file: &[u8], decoder: &dyn Decode) -> Document {
    read_as(file, Some(decoder), true, &Options::DEFAULT, &[])
}

/// The file as of the cross-reference section at byte `section`: read from
/// there along its `/Prev` chain, as if it were the newest, so that what
/// later updates wrote isn't in it, opened, when it's encrypted, by the
/// empty password. For an update's [`Update::previous`], that's the
/// revision it updates.
pub fn read_from(file: &[u8], section: usize) -> Document {
    read_as(file, None, true, &as_of(section), &[])
}

/// The file `file` is, with `decoder` asked for every stream the reader
/// would decode itself, as it was before it did: for the tests that set
/// the two side by side.
#[cfg(test)]
pub(crate) fn read_through(file: &[u8], decoder: &dyn Decode) -> Document {
    read_as(file, Some(decoder), false, &Options::DEFAULT, &[])
}

/// The file `file` is, a stream decoding to at most `stream` bytes and
/// the file's to `budget` in all: for the tests of the limits.
#[cfg(test)]
pub(crate) fn read_limited(file: &[u8], stream: usize, budget: usize) -> Document {
    let mut options = Options::DEFAULT;
    options.limits.stream = stream;
    options.limits.budget = budget;
    read_as(file, None, true, &options, &[])
}

/// The file `file` is, reading no more than `entries` cross-reference
/// entries: for the tests of that limit.
#[cfg(test)]
pub(crate) fn read_listing(file: &[u8], entries: usize) -> Document {
    let mut options = Options::DEFAULT;
    options.limits.entries = Some(entries);
    read_as(file, None, true, &options, &[])
}

/// The options that read a file as of the cross-reference section at byte
/// `section`.
fn as_of(section: usize) -> Options {
    let mut options = Options::DEFAULT;
    options.section = Some(section);
    options
}

/// The file read whole, through a [`Reading`] of its bytes.
fn read_as(
    file: &[u8],
    decoder: Option<&dyn Decode>,
    native: bool,
    options: &Options,
    passwords: &[&[u8]],
) -> Document {
    Reading::in_memory(file, passwords, options, native, decoder)
        .whole(decoder)
        .document
}

/// [`read_opened`]'s reading, and the security handler it opened the file
/// with, for the oracle to decrypt each stream as the reader would.
#[cfg(test)]
pub(crate) fn read_opened_by(file: &[u8], passwords: &[&[u8]]) -> (Document, Option<Handler>) {
    let whole = Reading::in_memory(file, passwords, &Options::DEFAULT, true, None).whole(None);
    (whole.document, whole.handler)
}

/// A reading read whole.
pub(crate) struct Whole {
    /// What it read.
    pub(crate) document: Document,
    /// The security handler it opened the file with.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) handler: Option<Handler>,
    /// Whether a stream it decoded began with less of its budget left than
    /// the stream's own limits allow ([`Reader::reached`]): the tests' and
    /// the fuzz target's.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) reached: bool,
}

/// What a read of the objects on the way to the metadata gives: the object
/// a reference refers to, none for no object that reads, or why the file
/// doesn't read.
type Get<'g, 'o, E> = &'g dyn Fn(ObjectId) -> Result<Option<&'o Object>, E>;

/// `o` with its references followed, as many as an object's way to the
/// catalog may take.
fn follow<'o, E>(get: Get<'_, 'o, E>, mut o: &'o Object) -> Result<Option<&'o Object>, E> {
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

/// Which object the catalog's metadata stream is (ISO 32000-2, 14.3.2):
/// the last the references from the catalog's `/Metadata` lead to, followed
/// as [`metadata`] follows them, when it's a stream. `get` gives the object
/// a reference refers to.
fn metadata_stream<'o, E>(
    trailer: &'o Trailer,
    get: Get<'_, 'o, E>,
) -> Result<Option<ObjectId>, E> {
    let Some(root) = trailer.dict().get(b"Root") else {
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

/// The data of the catalog's metadata stream (ISO 32000-2, 14.3.2), as
/// written in `file`, when it's in plain view: unfiltered, and not
/// `encrypted`. `get` gives the object a reference refers to.
fn metadata<'f, 'o, F: Bytes<'f>>(
    file: F,
    trailer: &'o Trailer,
    get: Get<'_, 'o, F::Error>,
    encrypted: bool,
) -> Result<Option<Cow<'f, [u8]>>, F::Error> {
    let dict = |o| -> Result<_, F::Error> {
        Ok(match follow(get, o)?.map(Object::value) {
            Some(Value::Dict(d)) => Some(d),
            _ => None,
        })
    };
    let trailer = trailer.dict();
    if encrypted {
        return Ok(None);
    }
    let Some(catalog) = trailer.get(b"Root").map(dict).transpose()?.flatten() else {
        return Ok(None);
    };
    let Some(metadata) = catalog.get(b"Metadata") else {
        return Ok(None);
    };
    let Some(Value::Stream(stream)) = follow(get, metadata)?.map(Object::value) else {
        return Ok(None);
    };
    let at = (stream.data.at()).filter(|at| at.stream.is_none() && !stream.dict.has(b"Filter"));
    match at {
        Some(at) => file.get(at.start..at.end),
        None => Ok(None),
    }
}

/// What a reading knows once it's open, which each read of it shares. Its
/// functions take the file's bytes, and all but the open's take `&self`,
/// so that a reading can be read on any thread: each object it reads is
/// kept in its slot, with its warnings, and each object stream it decodes
/// under its own lock.
struct Reader {
    /// Which reading it is, which each record it makes holds.
    id: SourceId,
    /// What it reads under.
    limits: Limits,
    /// Whether it decodes streams itself, as it does but in the tests that
    /// make it ask the decoder.
    native: bool,
    /// How many cross-reference entries it reads ([`listable`]).
    listable: usize,
    /// The version in the header.
    version: Option<Version>,
    /// The trailer.
    trailer: Trailer,
    /// What it warned of as it opened the file, in order.
    opened: Vec<Warning>,
    /// The newest section, when it updates one written before it.
    update: Option<Update>,
    /// What the header and sections say of the file.
    facts: Facts,
    /// Whether its entries were found by scanning the file.
    scanned: bool,
    /// The newest entry for each object number, with its generation, by
    /// number: as the sections give them, once they're read.
    entries: Box<[(u32, (u16, XrefEntry))]>,
    /// Where each object in use and each cross-reference section the
    /// sections list starts, in order, once they're read: for a file read
    /// by position, where each object can end ([`Reader::bound`]).
    offsets: Box<[usize]>,
    /// The security handler of an encrypted file it opened.
    handler: Option<Handler>,
    /// Whether the file is encrypted, and if it is, whether it opened.
    encryption: Encryption,
    /// In an encrypted file whose strings are encrypted, the objects whose
    /// strings aren't: the encryption dictionary's.
    encrypted: Option<HashSet<ObjectId>>,
    /// The objects the sections' trailers name as `/Encrypt`, and those its
    /// values refer to: an earlier update's encryption dictionary's strings
    /// aren't encrypted either. Kept, as a stream among them isn't handed
    /// out ([`Reading::data`]).
    dictionaries: HashSet<ObjectId>,
    /// Whether an encrypted file's strings aren't, by its handler's
    /// `/StrF`, or its dictionary's when it doesn't open.
    plain_strings: bool,
    /// Whether an encrypted file's metadata stream isn't, by
    /// `/EncryptMetadata false` of a dictionary of `/V` 4 or later.
    plain_metadata: bool,
    /// Whether an encrypted file's metadata stream is left as written by
    /// `/EncryptMetadata false` itself, whatever its streams are.
    unencrypted_metadata: bool,
    /// Whether an encrypted file's dictionary has an `/EFF`, the crypt
    /// filter its embedded files are encrypted by, which the reader doesn't
    /// apply.
    embedded_files: bool,
    /// Whether the file has an encryption dictionary, so that its streams
    /// may be encrypted, whatever its strings are.
    encrypts: bool,
    /// How many bytes the streams it decodes may still decode to. Each
    /// decode is limited by what's left when it starts, and what it decoded
    /// is taken after: two decodes that start on one remainder can pass it
    /// together by what they decode at once, but it never wraps.
    budget: AtomicUsize,
    /// Whether a stream it decoded began with less of the budget left than
    /// the stream's own limits allow: then which streams the budget cuts
    /// depends on the order they're decoded in.
    reached: AtomicBool,
    /// What reading each entry's object gave, once it's read: a slot for
    /// each of `entries`, in their order.
    slots: Box<[OnceLock<Box<Read>>]>,
    /// The object streams the entries put objects in, by number, each once
    /// it's decoded.
    containers: BTreeMap<u32, OnceLock<Box<Container>>>,
}

/// What reading an object gave, kept in its slot.
struct Read {
    /// The object and where it was read from, or why it doesn't read.
    object: Result<(Object, Record), Unread>,
    /// What the reader warned of as it read it, the parser's faults among
    /// them.
    warnings: Vec<Warning>,
    /// What it warned of as it decrypted the object's strings, which a
    /// reading says after every object's read.
    decrypt: Vec<Warning>,
}

/// An object stream, which objects the cross-references list are in,
/// decoded once.
struct Container {
    /// Where each of its objects starts, by its header; or why its objects
    /// don't read.
    header: Result<Vec<(u32, usize)>, Unread>,
    /// Its decoded data, which its objects are read from and placed in:
    /// none when its header doesn't read.
    data: Vec<u8>,
    /// Whether a fault cut its data off.
    cut: bool,
    /// What the reader warned of as it decoded it.
    warnings: Vec<Warning>,
}

/// One read's: its warnings, in order, and whether it's reading a stream's
/// indirect `/Length`, which reads no other.
#[derive(Default)]
struct Cx {
    warnings: Vec<Warning>,
    resolving: bool,
}

impl Cx {
    fn warn(&mut self, object: Option<ObjectId>, at: Option<Span>, message: String) {
        self.warnings.push(Warning {
            object,
            at,
            message,
        });
    }

    /// Where the parser's faults go, when it reads object `object` (none, a
    /// trailer).
    fn warned(&mut self, object: Option<ObjectId>) -> Warned<'_> {
        Warned {
            warnings: &mut self.warnings,
            object,
        }
    }
}

/// The reader's warnings, on object `object` (none, a trailer): where the
/// parser's faults go.
struct Warned<'r> {
    warnings: &'r mut Vec<Warning>,
    object: Option<ObjectId>,
}

impl Sink<'_> for Warned<'_> {
    fn fault(&mut self, at: Span, fault: parse::Fault<'_>) {
        self.warnings.push(Warning {
            object: self.object,
            at: Some(at),
            message: fault.to_string(),
        });
    }
}

/// A cross-reference section's entries, by number: each with its
/// generation.
type Rows = BTreeMap<u32, (u16, XrefEntry)>;

/// An indirect object read at an offset: its number, it, and where it was
/// read from; or why it doesn't read.
type Indirect = Result<(ObjectId, Object, Record), Unread>;

/// What reading an object in use gave, when an entry in use lists it.
type InUse<'r> = Option<Result<&'r Object, &'r Unread>>;

/// How a stream's data is handed out ([`Reading::data`]).
enum Crypt<'h> {
    /// As written.
    Written,
    /// Decrypted by the file's handler: by its own crypt filter's method
    /// when it has one, else as the file's streams are.
    Decrypted(&'h Handler, Option<Method>),
}

/// What's at the offset `startxref` or a `/Prev` gives.
enum At {
    /// A table, read but for its `/XRefStm`, and where that is; or why it
    /// doesn't read.
    Table(Result<(Box<Section>, Option<usize>), String>),
    /// An object, which may be a cross-reference stream.
    Stream,
    /// Neither.
    Neither,
}

/// A cross-reference section, read but for its stream's rows.
struct Section {
    trailer: Trailer,
    prev: Option<usize>,
    /// A table's rows.
    table: Rows,
    /// The cross-reference stream whose rows it has, at its offset: the
    /// section's own, or a hybrid table's `/XRefStm`.
    stream: Option<(usize, ObjectId, Object)>,
    /// Whether it's a table whose trailer gives an `/XRefStm`.
    hybrid: bool,
}

fn span(start: usize, end: usize) -> Span {
    Span::new(None, start, end)
}

/// The next token that isn't white-space or a comment.
fn significant<'t>(t: &mut Tokenizer<'t>) -> Option<Token<'t>> {
    t.find(|token| !matches!(token.kind, Kind::Space | Kind::Comment))
}

fn is_keyword(token: Option<&Token>, word: &[u8]) -> bool {
    token.is_some_and(|t| t.kind == Kind::Keyword && t.bytes == word)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// How many cross-reference entries the reader reads of a file `len`
/// bytes long: as many as its bytes, or 2²⁰, whichever is more. Objects
/// can be numbered sparsely, so not their numbers but their count.
fn listable(len: usize) -> usize {
    len.max(1 << 20)
}

/// Whether `fault` stopped its filter, cutting the data off.
fn stops(fault: &Fault) -> bool {
    matches!(
        fault,
        Fault::Byte { .. }
            | Fault::BrokenEnd { .. }
            | Fault::InGroup { .. }
            | Fault::Code { .. }
            | Fault::TableFull { .. }
            | Fault::Header
            | Fault::Corrupt { .. }
            | Fault::Truncated
            | Fault::Limit
    )
}

/// The bytes of a row of the predictor among `filters`, 1 without one.
fn predictor_row(filters: &[Filter]) -> usize {
    let layout = |filter: &Filter| match filter {
        Filter::Lzw { predictor, .. } | Filter::Flate { predictor } => match predictor {
            Predictor::Tiff(layout) | Predictor::Png(layout) => Some(*layout),
            Predictor::None => None,
        },
        _ => None,
    };
    let bits = |l: Layout| {
        (u64::from(l.colors).saturating_mul(u64::from(l.bits_per_component)))
            .saturating_mul(u64::from(l.columns))
    };
    (filters.iter().filter_map(layout))
        .map(|l| usize::try_from(bits(l).div_ceil(8)).unwrap_or(usize::MAX))
        .max()
        .unwrap_or(1)
        .max(1)
}

/// Where `fault` is in its filter's input, when it says.
fn place(fault: &Fault) -> Option<usize> {
    match *fault {
        Fault::Byte { at, .. }
        | Fault::BrokenEnd { at }
        | Fault::InGroup { at }
        | Fault::Overflow { at }
        | Fault::LoneByte { at }
        | Fault::Code { at }
        | Fault::TableFull { at }
        | Fault::Corrupt { at } => Some(at),
        _ => None,
    }
}

/// `filter`'s name, unabbreviated.
fn filter_name(filter: &Filter) -> &'static str {
    match filter {
        Filter::AsciiHex => "ASCIIHexDecode",
        Filter::Ascii85 => "ASCII85Decode",
        Filter::Lzw { .. } => "LZWDecode",
        Filter::Flate { .. } => "FlateDecode",
        Filter::RunLength => "RunLengthDecode",
    }
}

/// A stream's filters, as written, when it has any.
fn filters(o: &Object) -> Option<String> {
    let filter = o.as_dict()?.get(b"Filter")?;
    Some(match filter.value() {
        Value::Name(name) => format!("/{}", String::from_utf8_lossy(name)),
        _ => "filters".to_string(),
    })
}

/// What `decoder` makes of `ids`; nothing without one.
fn decode(
    decoder: Option<&dyn Decode>,
    ids: &[ObjectId],
) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
    match decoder {
        Some(decoder) if !ids.is_empty() => decoder.decode(ids),
        _ => BTreeMap::new(),
    }
}

impl Reader {
    /// The file `file` is, opened: its header, its cross-reference sections
    /// along `/Prev` from `startxref`'s (or from `options.section`, as of
    /// which it's read, without looking for `%%EOF`), or the objects found
    /// by scanning when they don't read, its `%%EOF`, and its encryption
    /// dictionary, its security handler opened with `passwords`, which
    /// aren't kept. `decoder` decodes the cross-reference streams the
    /// reader is made not to.
    fn new<'f, F: Bytes<'f>>(
        file: F,
        passwords: &[&[u8]],
        options: &Options,
        native: bool,
        decoder: Option<&dyn Decode>,
    ) -> Result<Reader, F::Error> {
        let limits = options.limits;
        let mut reader = Reader {
            id: SourceId::next(),
            limits,
            native,
            listable: limits.entries.unwrap_or_else(|| listable(file.len())),
            version: None,
            trailer: Trailer::new(Object::new(Value::Dict(Dict::new()))),
            opened: Vec::new(),
            update: None,
            facts: Facts::default(),
            scanned: false,
            entries: Box::default(),
            offsets: Box::default(),
            handler: None,
            encryption: Encryption::None,
            encrypted: None,
            dictionaries: HashSet::new(),
            plain_strings: false,
            plain_metadata: false,
            unencrypted_metadata: false,
            embedded_files: false,
            encrypts: false,
            budget: AtomicUsize::new(limits.budget),
            reached: AtomicBool::new(false),
            slots: Box::default(),
            containers: BTreeMap::new(),
        };
        let mut cx = Cx::default();
        reader.version = reader.header(file, &mut cx)?;
        let sections = match options.section {
            Some(section) => reader.sections_from(file, &mut cx, section, decoder)?,
            None => reader.sections(file, &mut cx, decoder)?,
        };
        let trailer = match sections {
            Ok(trailer) => trailer,
            Err(why) => reader.scan(file, &mut cx, why)?,
        };
        if options.section.is_none() {
            reader.end(file, &mut cx)?;
        }
        // Read by position, an object's first window reaches the next offset
        // listed, an object's or a section's.
        if file.bounded() {
            let in_use = (reader.entries.iter()).filter_map(|&(_, (_, entry))| match entry {
                XrefEntry::InUse { offset, .. } => Some(offset),
                _ => None,
            });
            let mut offsets = std::mem::take(&mut reader.offsets).into_vec();
            offsets.extend(in_use);
            offsets.sort_unstable();
            offsets.dedup();
            reader.offsets = offsets.into();
        }
        reader.encryption(file, &mut cx, &trailer, passwords)?;
        if let Some((0, (_, entry))) = reader.entries.first() {
            if *entry != XrefEntry::Free {
                let message =
                    "the cross-references list object 0, which is always free (ISO 32000-2, 7.5.4)";
                cx.warn(None, None, message.to_string());
            }
        }
        reader.facts.linearized = match reader.facts.header_offset {
            Some(at) => holds_linearization(file, at, limits.parser)?,
            None => false,
        };
        reader.slots = reader.entries.iter().map(|_| OnceLock::new()).collect();
        reader.containers = (reader.entries.iter())
            .filter_map(|&(_, (_, entry))| match entry {
                XrefEntry::Compressed { stream, .. } => Some((stream, OnceLock::new())),
                _ => None,
            })
            .collect();
        reader.trailer = trailer;
        reader.opened = cx.warnings;
        Ok(reader)
    }

    /// Where object number `n`'s entry is among the entries.
    fn index(&self, n: u32) -> Option<usize> {
        self.entries.binary_search_by_key(&n, |&(n, _)| n).ok()
    }

    /// The newest entry for object number `n`, with its generation.
    fn entry(&self, n: u32) -> Option<&(u16, XrefEntry)> {
        self.index(n).map(|i| &self.entries[i].1)
    }

    /// Where the entry that lists object `id` is among the entries: one in
    /// use of its generation, or one in an object stream, of generation 0.
    /// Object 0 is never one.
    fn listed(&self, id: ObjectId) -> Option<usize> {
        let i = self.index(id.0).filter(|_| id.0 != 0)?;
        match self.entries[i].1 {
            (generation, XrefEntry::InUse { .. }) if generation == id.1 => Some(i),
            (_, XrefEntry::Compressed { .. }) if id.1 == 0 => Some(i),
            _ => None,
        }
    }

    /// Where an object read by position at `offset` can end, at the most:
    /// at the next offset the sections list, an object's or a section's, or
    /// at the file's end, `len`. None when no offsets are listed, as in a
    /// file in memory.
    fn bound(&self, offset: usize, len: usize) -> Option<usize> {
        if self.offsets.is_empty() {
            return None;
        }
        let next = self.offsets.partition_point(|&next| next <= offset);
        Some(self.offsets.get(next).map_or(len, |&next| next.min(len)))
    }

    /// What reading entry `i`'s object gave: read, and kept, the first
    /// time it's asked for. Two threads may both read it; the first read
    /// kept is the one each gets. A read the file stops isn't kept.
    fn slot<'f, F: Bytes<'f>>(&self, file: F, i: usize) -> Result<&Read, F::Error> {
        let slot = &self.slots[i];
        if let Some(read) = slot.get() {
            return Ok(read);
        }
        let read = self.read(file, i)?;
        Ok(slot.get_or_init(|| Box::new(read)))
    }

    /// Entry `i`'s object, read, and not kept: at its offset, or from its
    /// object stream's decoded data, which is.
    fn read<'f, F: Bytes<'f>>(&self, file: F, i: usize) -> Result<Read, F::Error> {
        let (n, (generation, entry)) = self.entries[i];
        Ok(match entry {
            XrefEntry::InUse { offset, .. } => self.read_at(file, (n, generation), offset)?,
            XrefEntry::Compressed { stream, index } => self.read_in(file, (n, 0), stream, index)?,
            XrefEntry::Free => Read {
                object: Err(Unread::Missing {
                    id: (n, generation),
                }),
                warnings: Vec::new(),
                decrypt: Vec::new(),
            },
        })
    }

    /// Object `id`, read at `offset`, and in an encrypted file it opened, its
    /// strings decrypted.
    fn read_at<'f, F: Bytes<'f>>(
        &self,
        file: F,
        id: ObjectId,
        offset: usize,
    ) -> Result<Read, F::Error> {
        let mut cx = Cx::default();
        let mut object =
            self.indirect(file, &mut cx, offset, Some(id))?
                .map(|(_, mut o, mut record)| {
                    // A cross-reference stream is never encrypted (ISO 32000-2,
                    // 7.5.8.2).
                    if self.encrypted.is_some() && is_xref_stream(&o) {
                        plain_strings(&mut o);
                    }
                    if self.scanned {
                        record.origin = Origin::Recovered;
                    }
                    (o, record)
                });
        let decrypt = match (&self.handler, &mut object) {
            (Some(handler), Ok((o, _))) if self.native => decrypt_strings(handler, id, o),
            _ => Vec::new(),
        };
        Ok(Read {
            object,
            warnings: cx.warnings,
            decrypt,
        })
    }

    /// Object `id`, the `index`th of object stream `stream`, read from the
    /// stream's decoded data, which is decoded the first time an object in
    /// it is read.
    fn read_in<'f, F: Bytes<'f>>(
        &self,
        file: F,
        id: ObjectId,
        stream: u32,
        index: u32,
    ) -> Result<Read, F::Error> {
        let mut cx = Cx::default();
        let object = match self.container(file, stream)? {
            None => Err(Unread::ContainerMissing { stream }),
            Some(container) => match &container.header {
                Err(why) => Err(why.clone()),
                Ok(header) => self.in_stream(&mut cx, id, stream, header, index, container),
            },
        };
        Ok(Read {
            object,
            warnings: cx.warnings,
            decrypt: Vec::new(),
        })
    }

    /// Object `id`, when an entry in use lists it: what reading it gave.
    fn in_use<'f, F: Bytes<'f>>(&self, file: F, id: ObjectId) -> Result<InUse<'_>, F::Error> {
        let Some(i) = self.listed(id) else {
            return Ok(None);
        };
        Ok(match self.entries[i].1 {
            (_, XrefEntry::InUse { .. }) => {
                Some((self.slot(file, i)?.object.as_ref()).map(|(object, _)| object))
            }
            _ => None,
        })
    }

    /// Object `id`, when an entry in use lists it and it's read and kept:
    /// what reading it gave. It reads nothing, so it can't fail.
    fn kept(&self, id: ObjectId) -> InUse<'_> {
        let i = self.listed(id)?;
        match self.entries[i].1 {
            (_, XrefEntry::InUse { .. }) => {
                Some((self.slots[i].get()?.object.as_ref()).map(|(object, _)| object))
            }
            _ => None,
        }
    }

    /// Object `id`, read the first time it's asked for; or why it doesn't
    /// read.
    fn get<'f, F: Bytes<'f>>(
        &self,
        file: F,
        id: ObjectId,
    ) -> Result<Result<&Object, Unread>, F::Error> {
        let Some(i) = self.listed(id) else {
            return Ok(Err(Unread::Missing { id }));
        };
        Ok(match &self.slot(file, i)?.object {
            Ok((object, _)) => Ok(object),
            Err(why) => Err(why.clone()),
        })
    }

    /// Whether it decodes object streams itself: in a file that doesn't
    /// encrypt, or whose handler opened, or whose streams aren't encrypted;
    /// else it asks the decoder, when there's one.
    fn native_containers(&self) -> bool {
        self.native
            && (!self.encrypts || self.handler.is_some() || self.encryption == Encryption::Plain)
    }

    /// Object stream `stream`, which the entries put objects in, decoded
    /// the first time it's asked for. A read the file stops decodes
    /// nothing, and keeps nothing of it but the objects it read whole on
    /// the way, the stream's own and those its filters refer to, kept as
    /// any read of them keeps them.
    fn container<'f, F: Bytes<'f>>(
        &self,
        file: F,
        stream: u32,
    ) -> Result<Option<&Container>, F::Error> {
        let Some(lock) = self.containers.get(&stream) else {
            return Ok(None);
        };
        if let Some(container) = lock.get() {
            return Ok(Some(container));
        }
        // What a read can fail at is read before the lock, which a second
        // thread waits on rather than decode the stream twice: the object
        // stream itself, what its filters' values refer to, and its data.
        let object = self.in_use(file, (stream, 0))?;
        let mut data = None;
        if let Some(Ok(object)) = object.filter(|_| self.native_containers()) {
            if let Value::Stream(s) = object.value() {
                let failed = Cell::new(None);
                let resolve = |id| match self.in_use(file, id) {
                    Ok(object) => object?.ok(),
                    Err(error) => {
                        failed.set(Some(error));
                        None
                    }
                };
                let _ = filter::filters(&s.dict, Some(&resolve));
                let _ = filter::crypt_filter(&s.dict, Some(&resolve));
                if let Some(error) = failed.into_inner() {
                    return Err(error);
                }
                // Its data, which only an object stream's decode reads.
                let at = (s.data.at()).filter(|at| at.stream.is_none());
                if s.dict.get(b"Type").and_then(Object::as_name) == Some(b"ObjStm") {
                    data = at
                        .map(|at| file.get(at.start..at.end))
                        .transpose()?
                        .flatten();
                }
            }
        }
        let data = data.unwrap_or_default();
        let memory = file.memory();
        let container =
            || Box::new(self.decode_container(memory, stream, object, &data, None, None));
        Ok(Some(lock.get_or_init(container)))
    }

    /// Object stream `stream`, its object `object` (none when no entry in
    /// use lists it) and its data, `data`: decoded from its own data when
    /// it decodes them itself, else as `given`, what the decoder made of
    /// it, and failing that, in a file in `memory` that doesn't encrypt, by
    /// `decoder` from its bytes.
    fn decode_container(
        &self,
        memory: Option<&[u8]>,
        stream: u32,
        object: InUse<'_>,
        data: &[u8],
        given: Option<Result<Vec<u8>, Undecoded>>,
        decoder: Option<&dyn Decode>,
    ) -> Container {
        let id = (stream, 0);
        let mut cx = Cx::default();
        let mut cut = false;
        let native = self.native_containers();
        let header = match object {
            Some(Ok(object)) => {
                let decoded = match native {
                    true => Some(self.object_stream(&mut cx, &mut cut, id, object, data)),
                    false => given,
                };
                let by_number = match decoded {
                    Some(Ok(data)) => (object_stream_header(object, &data))
                        .map(|h| (h, data))
                        .map_err(|why| Unread::ContainerHeader { stream, why }),
                    Some(Err(why)) => Err(Unread::Container { stream, why }),
                    None => Err(Unread::Undecodable { stream }),
                };
                // Asked of the decoder in a file that isn't encrypted,
                // a stream it can't decode by its number, or decodes as
                // another object's, is decoded from its bytes.
                by_number.or_else(|why| {
                    decoder
                        .filter(|_| !self.encrypts && !native)
                        .zip(memory.and_then(|file| self.written_stream(file, object)))
                        .and_then(|(decoder, written)| decoder.decode_written(written).ok())
                        .and_then(|data| Some((object_stream_header(object, &data).ok()?, data)))
                        .ok_or(why)
                })
            }
            Some(Err(why)) => Err(Unread::ContainerUnread {
                stream,
                why: Box::new(why.clone()),
            }),
            None => Err(Unread::ContainerMissing { stream }),
        };
        let (header, data) = match header {
            Ok((header, data)) => (Ok(header), data),
            Err(why) => (Err(why), Vec::new()),
        };
        Container {
            header,
            data,
            cut,
            warnings: cx.warnings,
        }
    }

    /// Every object stream the entries put objects in decoded at once, as
    /// it is when the reader asks the decoder: in plain view when it's in
    /// a file that doesn't encrypt and has no filters, else as `decoder`
    /// makes of it.
    fn decode_containers<'f, F: Bytes<'f>>(
        &self,
        file: F,
        decoder: Option<&dyn Decode>,
    ) -> Result<(), F::Error> {
        // An object stream without filters, in a file that doesn't
        // encrypt, is in plain view: its data is as written.
        let mut plain: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
        for &n in self.containers.keys().filter(|_| !self.encrypts) {
            let Some(Ok(object)) = self.in_use(file, (n, 0))? else {
                continue;
            };
            let Value::Stream(stream) = object.value() else {
                continue;
            };
            let Some(at) = stream.data.at().filter(|at| at.stream.is_none()) else {
                continue;
            };
            let Some(data) = file.get(at.start..at.end)? else {
                continue;
            };
            if filters(object).is_none() {
                plain.insert(n, data.into_owned());
            }
        }
        let ids: Vec<ObjectId> = (self.containers.keys())
            .filter(|n| !plain.contains_key(n))
            .map(|&n| (n, 0))
            .collect();
        let mut decoded: BTreeMap<_, _> = (decode(decoder, &ids).into_iter())
            .map(|(id, data)| (id, data.map_err(Undecoded::Other)))
            .collect();
        decoded.extend(plain.into_iter().map(|(n, data)| ((n, 0), Ok(data))));
        let memory = file.memory();
        for (&stream, lock) in &self.containers {
            let object = self.in_use(file, (stream, 0))?;
            let given = decoded.remove(&(stream, 0));
            lock.get_or_init(|| {
                Box::new(self.decode_container(memory, stream, object, &[], given, decoder))
            });
        }
        Ok(())
    }

    /// Every object the entries list read, as the whole read reads them:
    /// each in use at its offset, by number; then each object stream the
    /// entries put objects in, by number, at once when the reader asks
    /// `decoder` for them; then each object in an object stream, by number.
    fn read_all<'f, F: Bytes<'f>>(
        &self,
        file: F,
        decoder: Option<&dyn Decode>,
    ) -> Result<(), F::Error> {
        let numbered = |i: usize| self.entries[i].0 != 0;
        let in_use =
            |i: &usize| numbered(*i) && matches!(self.entries[*i].1.1, XrefEntry::InUse { .. });
        let member = |i: &usize| {
            numbered(*i) && matches!(self.entries[*i].1.1, XrefEntry::Compressed { .. })
        };
        for i in (0..self.entries.len()).filter(in_use) {
            self.slot(file, i)?;
        }
        match self.native_containers() {
            true => {
                for &stream in self.containers.keys() {
                    self.container(file, stream)?;
                }
            }
            false => self.decode_containers(file, decoder)?,
        }
        for i in (0..self.entries.len()).filter(member) {
            self.slot(file, i)?;
        }
        Ok(())
    }

    /// What it warned of in the objects it has read, as a whole read says
    /// it: what it warned of as it opened the file; then as it read each
    /// object in use, by number; as it decoded each object stream, by
    /// number; as it read each object in an object stream, by number; and
    /// as it decrypted each object's strings, by number.
    fn warnings(&self) -> Vec<Warning> {
        let reads = |member: bool| {
            (self.entries.iter().zip(&self.slots))
                .filter(move |&(&(n, (_, entry)), _)| {
                    n != 0 && matches!(entry, XrefEntry::Compressed { .. }) == member
                })
                .filter_map(|(_, slot)| slot.get())
        };
        let mut warnings = self.opened.clone();
        for read in reads(false) {
            warnings.extend(read.warnings.iter().cloned());
        }
        for container in self.containers.values().filter_map(OnceLock::get) {
            warnings.extend(container.warnings.iter().cloned());
        }
        for read in reads(true) {
            warnings.extend(read.warnings.iter().cloned());
        }
        for read in reads(false) {
            warnings.extend(read.decrypt.iter().cloned());
        }
        warnings
    }

    /// The file read whole, every object the entries list read as
    /// [`Reader::read_all`] reads them, with `decoder` asked for what the
    /// reader doesn't decode or decrypt itself, into a [`Document`]: each
    /// object moved out of its slot, and the warnings as
    /// [`Reader::warnings`] says them.
    fn whole<'f, F: Bytes<'f>>(
        self,
        file: F,
        decoder: Option<&dyn Decode>,
    ) -> Result<Whole, F::Error> {
        self.read_all(file, decoder)?;
        let reached = self.reached.load(Ordering::Relaxed);
        // Metadata that isn't encrypted is in plain view: in a file that
        // doesn't encrypt, or by `/EncryptMetadata false`, which counts from
        // version 4, whether the handler opens or not.
        let plain_metadata = !self.encrypts || self.plain_metadata;
        let natively = self.native && self.handler.is_some();
        let Reader {
            entries,
            slots,
            containers,
            opened,
            trailer,
            version,
            update,
            encryption,
            encrypted,
            handler,
            facts,
            ..
        } = self;
        let mut objects = BTreeMap::new();
        let mut records = BTreeMap::new();
        let (mut read, mut members, mut decrypted) = (Vec::new(), Vec::new(), Vec::new());
        for (&(n, (generation, entry)), slot) in entries.iter().zip(slots.into_vec()) {
            let Some(got) = slot.into_inner() else {
                continue;
            };
            let Read {
                object,
                warnings,
                decrypt,
            } = *got;
            let id = match entry {
                XrefEntry::InUse { .. } => {
                    read.extend(warnings);
                    decrypted.extend(decrypt);
                    (n, generation)
                }
                _ => {
                    members.extend(warnings);
                    (n, 0)
                }
            };
            let object = object.map(|(object, record)| {
                records.insert(id, record);
                object
            });
            objects.insert(id, object);
        }
        let mut warnings = opened;
        warnings.extend(read);
        let mut decoded = BTreeMap::new();
        for (stream, lock) in containers {
            if let Some(container) = lock.into_inner() {
                let Container {
                    header,
                    data,
                    warnings: said,
                    ..
                } = *container;
                warnings.extend(said);
                if header.is_ok() {
                    decoded.insert((stream, 0), data);
                }
            }
        }
        warnings.extend(members);
        warnings.extend(decrypted);
        if !natively {
            decrypt(decoder, encrypted.is_some(), &mut objects, &mut warnings);
        }
        let get = |id| Ok(objects.get(&id).and_then(|o| o.as_ref().ok()));
        let metadata = metadata(file, &trailer, &get, !plain_metadata)?.map(Cow::into_owned);
        let document = Document {
            objects,
            trailer,
            header: version,
            warnings,
            decoded,
            update,
            size: Some(file.len()),
            metadata,
            encryption,
            records,
            facts,
        };
        Ok(Whole {
            document,
            handler,
            reached,
        })
    }

    /// Stream `id`'s data, written at `at`, decoded by `filters`, as much
    /// as `limit`, the stream's limit and what's left of the reading's
    /// budget allow: what decoded, however far it went, and the faults on
    /// the way, each in its filter.
    fn decode_stream(
        &self,
        data: &[u8],
        filters: &[Filter],
        limit: usize,
    ) -> (Vec<u8>, Vec<(usize, Fault)>) {
        let own = limit.min(self.limits.stream);
        let left = self.budget.load(Ordering::Relaxed);
        if left < own {
            self.reached.store(true, Ordering::Relaxed);
        }
        let (decoded, faults) = match pdf_filters::decode(data, filters, own.min(left)) {
            Ok(decoded) => (decoded, Vec::new()),
            Err(error) => (error.data, error.faults),
        };
        // Taken from what's left, not from what was: another decode may
        // have taken some since. Never below none, which wrapping would
        // make nearly all.
        let mut left = left;
        loop {
            let taken = left.saturating_sub(decoded.len());
            let swapped = (self.budget).compare_exchange_weak(
                left,
                taken,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
            match swapped {
                Ok(_) => break,
                Err(now) => left = now,
            }
        }
        (decoded, faults)
    }

    /// A warning on stream `id` for each of `faults` in its `filters`: at
    /// the fault's bytes, when it has a place in the data as written
    /// (`data`, in the file), else at the data.
    fn faulted(
        &self,
        cx: &mut Cx,
        id: ObjectId,
        data: Option<Span>,
        written: Option<Span>,
        filters: &[Filter],
        faults: Vec<(usize, Fault)>,
    ) {
        for (i, fault) in faults {
            let at = match (i, place(&fault), written) {
                (0, Some(at), Some(written)) => {
                    Some(span(written.start + at, written.start + at + 1))
                }
                _ => data,
            };
            let name = filter_name(&filters[i]);
            let message = format!("its data doesn't decode cleanly through /{name}: {fault}");
            let warning = Warning {
                object: Some(id),
                at,
                message,
            };
            // A stream two sections name, as a hybrid file's `/XRefStm`
            // can be, is decoded for each, and said once.
            if !cx.warnings.contains(&warning) {
                cx.warnings.push(warning);
            }
        }
    }

    /// Object stream `id`'s data, decoded from its own, `data`, its
    /// filters' values followed where they're references to objects in
    /// use; `cut` set when a fault cut it off.
    fn object_stream(
        &self,
        cx: &mut Cx,
        cut: &mut bool,
        id: ObjectId,
        object: &Object,
        data: &[u8],
    ) -> Result<Vec<u8>, Undecoded> {
        let Value::Stream(stream) = object.value() else {
            return Err(Undecoded::NotAStream);
        };
        // Not decoded, and so not counted against the budget, unless it's
        // an object stream: its header says why it's not.
        if stream.dict.get(b"Type").and_then(Object::as_name) != Some(b"ObjStm") {
            return Ok(Vec::new());
        }
        // What its filters' values refer to is read and kept before its
        // decode, which reads nothing.
        let resolve = |id| self.kept(id)?.ok();
        let filters = filter::filters(&stream.dict, Some(&resolve)).map_err(Undecoded::Filters)?;
        // In an encrypted file, decrypted first: by its own crypt filter
        // when it names one, else as the file's streams are. In one no
        // password opened whose streams aren't encrypted, its own crypt
        // filter may yet encrypt it.
        let crypt = filter::crypt_filter(&stream.dict, Some(&resolve));
        if self.handler.is_none() && self.encrypts {
            if let Some(name) = crypt.as_deref().filter(|&name| name != b"Identity") {
                return Err(Undecoded::Encrypted {
                    filter: name.to_vec(),
                });
            }
        }
        let decrypted = (self.handler.as_ref()).map(|handler| {
            let method = crypt.map(|name| handler.named(&name));
            handler.decrypt_stream(id, data, method.as_ref())
        });
        let (data, exact) = match decrypted {
            None => (data.to_vec(), true),
            Some(Ok(plain)) => (plain, false),
            Some(Err(error)) => {
                let fault = error.fault;
                let message = format!("its data doesn't decrypt cleanly: {fault}");
                cx.warn(Some(id), stream.data.at(), message);
                match error.data {
                    Some(plain) => (plain, false),
                    None => return Err(Undecoded::Decrypt(fault)),
                }
            }
        };
        if filters.is_empty() {
            return Ok(data);
        }
        let (decoded, faults) = self.decode_stream(&data, &filters, self.limits.stream);
        if faults.iter().any(|(_, fault)| stops(fault)) {
            *cut = true;
        }
        // A fault's place is in the data as written only when it wasn't
        // decrypted first.
        let at = stream.data.at().filter(|_| exact);
        self.faulted(cx, id, stream.data.at(), at, &filters, faults);
        Ok(decoded)
    }

    /// How the data of stream `id`, whose dictionary is `dict`, is handed
    /// out ([`Reading::data`]), by the rule its object streams are decrypted
    /// by: by the stream's own `/Crypt` filter when it has one, else as the
    /// file's streams are (ISO 32000-2, 7.6.6), its filters' values
    /// followed through what's read and kept. Left as written: in a file
    /// that doesn't encrypt; a cross-reference stream's (7.5.8.2); the
    /// catalog's metadata stream's under `/EncryptMetadata false` (7.6.4.2
    /// and 14.3.2), whether the file opens or not, but where its own crypt
    /// filter encrypts it; and in a file no password opened whose streams
    /// aren't encrypted ([`Encryption::Plain`]), every stream's but one
    /// whose own crypt filter encrypts it. Refused where it may be
    /// ciphertext the reader doesn't decrypt: in a file no password opened,
    /// every other stream's ([`Error::Locked`]); in a file whose handler it
    /// can't open, every stream's ([`Error::Unsupported`]); an embedded
    /// file's, in a file whose encryption dictionary has an `/EFF`
    /// ([`Error::EmbeddedFile`]); one the encryption dictionary refers to,
    /// whose strings are left as written, and the catalog's metadata
    /// stream's under `/EncryptMetadata false` whose own crypt filter
    /// encrypts it, which the reader neither decrypts nor leaves
    /// ([`Error::Encrypted`]).
    fn crypt<'f, F: Bytes<'f>>(
        &'f self,
        file: F,
        id: ObjectId,
        dict: &Dict,
    ) -> Result<Result<Crypt<'f>, Error>, F::Error> {
        if !self.encrypts || is_xref(dict) {
            return Ok(Ok(Crypt::Written));
        }
        if self.dictionaries.contains(&id) {
            return Ok(Err(Error::Encrypted));
        }
        let failed = Cell::new(None);
        let resolve = |id| match self.get(file, id) {
            Ok(object) => object.ok(),
            Err(error) => {
                failed.set(Some(error));
                None
            }
        };
        let crypt = filter::crypt_filter(dict, Some(&resolve));
        if let Some(error) = failed.into_inner() {
            return Err(error);
        }
        if self.unencrypted_metadata {
            let get = |id| Ok(self.get(file, id)?.ok());
            if metadata_stream(&self.trailer, &get)? == Some(id) {
                // Its own crypt filter may encrypt it all the same.
                return Ok(match crypt.as_deref() {
                    Some(name) if name != b"Identity" => Err(Error::Encrypted),
                    _ => Ok(Crypt::Written),
                });
            }
        }
        if self.embedded_files
            && dict.get(b"Type").and_then(Object::as_name) == Some(b"EmbeddedFile")
        {
            return Ok(Err(Error::EmbeddedFile));
        }
        Ok(match (&self.handler, &self.encryption) {
            (Some(handler), _) => {
                let method = crypt.map(|name| handler.named(&name));
                Ok(Crypt::Decrypted(handler, method))
            }
            (None, Encryption::Plain) => match crypt.as_deref() {
                Some(name) if name != b"Identity" => Err(Error::Locked),
                _ => Ok(Crypt::Written),
            },
            (None, Encryption::Unsupported(_)) => Err(Error::Unsupported),
            (None, _) => Err(Error::Locked),
        })
    }

    /// Whether object `id`'s strings are ciphertext.
    fn ciphertext(&self, id: ObjectId) -> bool {
        (self.encrypted.as_ref()).is_some_and(|plain| !plain.contains(&id))
    }

    /// Whether the file ends as ISO 32000-2 7.5.5 has it, its last line
    /// `%%EOF` alone; and, failing that, whether a `%%EOF` is within its last
    /// 1024 bytes, where readers look for one.
    ///
    /// Read by position, its last bytes are read back from the end, as
    /// far as its last `%%EOF` and the byte before it, and its last 1024.
    fn end<'f, F: Bytes<'f>>(&mut self, file: F, cx: &mut Cx) -> Result<(), F::Error> {
        let len = file.len();
        let said = file.read_back(|w| {
            // The window's bytes are the file's from `from` to its end.
            let (bytes, from) = (w.bytes, w.base);
            let eols = bytes
                .iter()
                .rev()
                .take_while(|b| matches!(b, b'\r' | b'\n'))
                .count();
            let tail = len.saturating_sub(1024);
            // The body's last bytes, `%%EOF` and the byte before it, and the
            // file's last 1024, at the least.
            if from > 0 && (eols + 6 > bytes.len() || from > tail) {
                w.touch();
                return None;
            }
            let body = &bytes[..bytes.len() - eols];
            let alone = |at: usize| at == 0 || matches!(body[at - 1 - from], b'\r' | b'\n');
            if body.ends_with(b"%%EOF") && alone(from + body.len() - 5) {
                return None;
            }
            let body = from + body.len();
            let said = match rfind(&bytes[tail - from..], b"%%EOF").map(|i| tail + i) {
                Some(at) if at + 5 == body => (
                    span(at, at + 5),
                    "the file's last %%EOF isn't on a line of its own".to_string(),
                ),
                Some(at) => (
                    span(at + 5, len),
                    format!(
                        "the file goes on for {} bytes after its last %%EOF",
                        len - at - 5
                    ),
                ),
                None => match rfind(bytes, b"%%EOF").map(|i| from + i) {
                    Some(at) => (
                        span(at, at + 5),
                        format!(
                            "the file's last %%EOF is {} bytes from its end, past the 1024 readers look in",
                            len - at
                        ),
                    ),
                    // Before the window, a %%EOF may yet be.
                    None if from > 0 => {
                        w.touch();
                        return None;
                    }
                    None => (span(len, len), "the file has no %%EOF".to_string()),
                },
            };
            Some(said)
        })?;
        if let Some((at, message)) = said {
            cx.warn(None, Some(at), message);
        }
        Ok(())
    }

    /// The version `%PDF-` gives, within the first 1024 bytes; where it
    /// starts, in the facts.
    fn header<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
        cx: &mut Cx,
    ) -> Result<Option<Version>, F::Error> {
        let header = file.read_from(cx, 0, 0, |w, cx| {
            // The window's bytes are the file's from its start.
            let file = w.bytes;
            let head = &file[..file.len().min(1024)];
            let Some(at) = find(head, b"%PDF-") else {
                if head.len() < 1024 {
                    w.touch();
                }
                cx.warn(None, None, "no %PDF- header".to_string());
                return Ok(None);
            };
            if at > 0 {
                let message = format!("%PDF- is at byte {at}, not the file's first");
                cx.warn(None, Some(span(at, at + 5)), message);
            }
            let rest = &file[at + 5..];
            let end = (rest.iter()).position(|&b| is_white_space(b) || is_delimiter(b));
            let end = end.unwrap_or_else(|| {
                w.touch();
                rest.len()
            });
            let version = Version::parse(&rest[..end]);
            if version.is_none() {
                let message = "the header's version doesn't read".to_string();
                cx.warn(None, Some(span(at, at + 5 + end)), message);
            }
            Ok(Some((at, version)))
        })?;
        let (at, version) = header.unzip();
        self.facts.header_offset = at;
        Ok(version.flatten())
    }

    /// The sections from the last `startxref` along their `/Prev` chain,
    /// their entries the newest for each number; the newest's trailer.
    /// Compressed streams' rows are decoded together, once the chain is
    /// known: each stream's dictionary, `/Prev` and all, is in plain view.
    ///
    /// Read by position, `startxref` is looked for in the file's last
    /// bytes, read back from its end until it's found.
    fn sections<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
        cx: &mut Cx,
        decoder: Option<&dyn Decode>,
    ) -> Result<Result<Trailer, String>, F::Error> {
        let offset = file.read_back(|w| {
            // The window's bytes are the file's from `w.base` to its end.
            let Some(i) = rfind(w.bytes, b"startxref") else {
                w.touch();
                return Err("no startxref".to_string());
            };
            let at = w.place(i);
            let mut t = Tokenizer::at(w.bytes, i);
            if !is_keyword(t.next().as_ref(), b"startxref") {
                return Err(format!("startxref at byte {at} is inside another word"));
            }
            (significant(&mut t))
                .filter(|token| token.kind == Kind::Integer)
                .and_then(|token| usize::try_from(token.integer().ok()?).ok())
                .ok_or_else(|| "startxref gives no offset".to_string())
        })?;
        match offset {
            Ok(offset) => self.sections_from(file, cx, offset, decoder),
            Err(why) => Ok(Err(why)),
        }
    }

    /// The sections from the one at `offset` along their `/Prev` chain, as
    /// [`Reader::sections`] reads them from `startxref`'s.
    fn sections_from<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
        cx: &mut Cx,
        offset: usize,
        decoder: Option<&dyn Decode>,
    ) -> Result<Result<Trailer, String>, F::Error> {
        let mut sections: Vec<Section> = Vec::new();
        let mut offsets = Vec::new();
        let mut seen = HashSet::new();
        let mut next = Some(offset);
        while let Some(offset) = next {
            if !seen.insert(offset) {
                let message = format!("the /Prev chain comes back to byte {offset}");
                cx.warn(None, None, message);
                break;
            }
            if seen.len() > self.limits.sections {
                let message = format!(
                    "more than {} cross-reference sections",
                    self.limits.sections
                );
                cx.warn(None, None, message);
                break;
            }
            let section = match self.section(file, cx, offset)? {
                Ok(section) => section,
                Err(why) if sections.is_empty() => return Ok(Err(why)),
                Err(why) => {
                    let message = format!("the section at byte {offset} doesn't read: {why}");
                    cx.warn(None, None, message);
                    break;
                }
            };
            next = section.prev;
            let encrypt = section.trailer.dict().get(b"Encrypt");
            if let Some(Value::Ref(id)) = encrypt.map(Object::value) {
                self.dictionaries.insert(*id);
            }
            sections.push(section);
            offsets.push(offset);
        }
        // Asked of the decoder only when the reader is made to.
        let compressed: Vec<ObjectId> = (sections.iter())
            .filter(|_| !self.native)
            .filter_map(|s| s.stream.as_ref())
            .filter(|(_, _, stream)| filters(stream).is_some())
            .map(|(_, id, _)| *id)
            .collect();
        let decoded = decode(decoder, &compressed);
        // What the sections say of the file, as written.
        let mut facts = Facts {
            sections: sections.len(),
            ..Facts::default()
        };
        if let (Some(section), Some(&offset)) = (sections.first(), offsets.first()) {
            let kind = match section.trailer.stream {
                Some(_) => XrefKind::Stream,
                None => XrefKind::Table,
            };
            facts.newest = Some((offset, kind));
        }
        facts.hybrid = sections.iter().any(|s| s.hybrid);
        facts.max_stream = (sections.iter())
            .filter_map(|s| s.stream.as_ref().map(|(_, (n, _), _)| *n))
            .max();
        let mut entries = BTreeMap::new();
        let mut trailer = None;
        // Where each section, and each stream a section's rows are in,
        // starts.
        let streams = sections.iter().filter_map(|s| s.stream.as_ref());
        let starts: Vec<usize> = (offsets.iter().copied())
            .chain(streams.map(|&(at, _, _)| at))
            .collect();
        for (i, section) in sections.into_iter().enumerate() {
            let mut listed = section.table;
            if let Some((offset, id, stream)) = &section.stream {
                let hybrid = section.trailer.stream.is_none();
                match self.rows(
                    file,
                    cx,
                    *offset,
                    *id,
                    stream,
                    decoded.get(id),
                    !hybrid,
                    decoder,
                )? {
                    // A section's own stream's rows are its entries.
                    Ok(rows) if listed.is_empty() => listed = rows,
                    // A hybrid file's stream lists what its table leaves free.
                    Ok(rows) => {
                        for (n, entry) in rows {
                            let free = matches!(listed.get(&n), None | Some((_, XrefEntry::Free)));
                            if free {
                                listed.insert(n, entry);
                            }
                        }
                    }
                    Err(why) if i == 0 && !hybrid => return Ok(Err(why)),
                    Err(why) => {
                        let what = if hybrid {
                            "its /XRefStm"
                        } else {
                            "the section"
                        };
                        let message = format!("{what}, at byte {offset}, doesn't read: {why}");
                        cx.warn(None, None, message);
                    }
                }
            }
            // An update is written after the section it follows: a
            // linearized file's first section, before its other, isn't one.
            if let [newest, previous, ..] = offsets[..] {
                if i == 0 && newest > previous {
                    let objects = (listed.iter())
                        .filter(|&(&n, (_, entry))| n != 0 && *entry != XrefEntry::Free)
                        .map(|(&n, &(generation, _))| (n, generation))
                        .collect();
                    self.update = Some(Update { objects, previous });
                }
            }
            let most = self.listable;
            let mut past = 0usize;
            // The newest section's entries, as many as are read, are the
            // entries so far.
            if entries.is_empty() && listed.len() <= most {
                entries = std::mem::take(&mut listed);
            }
            for (n, entry) in listed {
                match entries.len() < most || entries.contains_key(&n) {
                    true => {
                        entries.entry(n).or_insert(entry);
                    }
                    false => past += 1,
                }
            }
            if past > 0 {
                let message = format!(
                    "the cross-reference sections list {past} objects past the first {most}, as many as the reader reads of a file of {} bytes, which aren't read",
                    file.len()
                );
                cx.warn(None, None, message);
            }
            trailer.get_or_insert(section.trailer);
        }
        let Some(trailer) = trailer else {
            return Ok(Err("no section".to_string()));
        };
        self.entries = entries.into_iter().collect();
        self.offsets = starts.into();
        self.facts = Facts {
            header_offset: self.facts.header_offset,
            ..facts
        };
        Ok(Ok(trailer))
    }

    /// The section at `offset`: a table, or a cross-reference stream.
    fn section<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
        cx: &mut Cx,
        offset: usize,
    ) -> Result<Result<Section, String>, F::Error> {
        // What's at the offset, read in its window: a table, its trailer
        // and where its /XRefStm is, read whole.
        let at = file.read_from(cx, offset, 0, |w, cx| {
            let mut t = w.tokenizer(offset);
            Ok(match w.significant(&mut t) {
                Some(token) if is_keyword(Some(&token), b"xref") => {
                    At::Table(self.table(w, cx, &mut t).map(|(s, at)| (Box::new(s), at)))
                }
                Some(token) if token.kind == Kind::Integer => At::Stream,
                _ => At::Neither,
            })
        })?;
        match at {
            At::Table(Ok((mut section, xref_stm))) => {
                if let Some(at) = xref_stm {
                    section.stream = match self.xref_stream(file, cx, at)? {
                        Ok((id, object)) => Some((at, id, object)),
                        Err(why) => {
                            let message =
                                format!("its /XRefStm, at byte {at}, doesn't read: {why}");
                            cx.warn(None, None, message);
                            None
                        }
                    };
                }
                Ok(Ok(*section))
            }
            At::Table(Err(why)) => Ok(Err(why)),
            At::Stream => {
                let (id, object) = match self.xref_stream(file, cx, offset)? {
                    Ok(stream) => stream,
                    Err(why) => return Ok(Err(why)),
                };
                let prev = object
                    .as_dict()
                    .and_then(|d| d.get(b"Prev"))
                    .and_then(offset_of);
                Ok(Ok(Section {
                    trailer: Trailer {
                        stream: Some(id),
                        object: object.clone(),
                        unwritten: None,
                    },
                    prev,
                    table: BTreeMap::new(),
                    stream: Some((offset, id, object)),
                    hybrid: false,
                }))
            }
            At::Neither => Ok(Err(format!(
                "byte {offset}, which startxref or /Prev gives, is at neither `xref` nor an object"
            ))),
        }
    }

    /// A table's subsections, after `xref`, and its trailer, in window
    /// `w`; and where a hybrid file's `/XRefStm` is, which isn't read yet.
    fn table<'w>(
        &self,
        w: &Window<'w>,
        cx: &mut Cx,
        t: &mut Tokenizer<'w>,
    ) -> Result<(Section, Option<usize>), String> {
        let integer = |t: &mut Tokenizer<'w>| {
            (w.significant(t))
                .filter(|token| token.kind == Kind::Integer)
                .and_then(|token| token.integer().ok())
        };
        let mut entries = BTreeMap::new();
        // The rows not as ISO 32000-2 7.5.4 writes them, and the first.
        let (mut rows, mut misshapen) = (0, 0);
        let mut first: Option<(u32, Span, &str)> = None;
        loop {
            let at = w.place(t.offset());
            match w.significant(&mut t.clone()) {
                Some(token) if is_keyword(Some(&token), b"trailer") => {
                    w.significant(t);
                    break;
                }
                Some(token) if token.kind == Kind::Integer => {
                    let start = integer(t).and_then(|n| u32::try_from(n).ok());
                    let count = integer(t).and_then(|n| u32::try_from(n).ok());
                    let (Some(start), Some(count)) = (start, count) else {
                        return Err(format!("a subsection's header at byte {at} doesn't read"));
                    };
                    for i in 0..count {
                        let row = w
                            .significant(&mut t.clone())
                            .map_or(t.offset(), |t| t.start);
                        let row = w.place(row);
                        let Some(n) = start.checked_add(i) else {
                            return Err(format!("the table's row at byte {row} numbers no object"));
                        };
                        rows += 1;
                        if let Err(why) = w.row_shape(row) {
                            misshapen += 1;
                            let end = (row + 20).min(w.place(w.bytes.len()));
                            first.get_or_insert((n, span(row, end), why));
                        }
                        let offset = integer(t);
                        let generation = integer(t);
                        let kind = w.significant(t);
                        let (Some(offset), Some(generation), Some(kind @ (b"n" | b"f"))) =
                            (offset, generation, kind.as_ref().map(|k| k.bytes))
                        else {
                            return Err(format!("the table's row at byte {row} doesn't read"));
                        };
                        let entry = match (usize::try_from(offset), u16::try_from(generation)) {
                            (Ok(offset), Ok(generation)) if kind == b"n" => {
                                (generation, XrefEntry::InUse { offset, generation })
                            }
                            (_, Ok(generation)) if kind == b"f" => (generation, XrefEntry::Free),
                            // A row in its place, but a number out of range:
                            // the rest of the table still reads.
                            _ => {
                                let message = format!(
                                    "the table's row for object {n} gives a number out of range, and is read as free"
                                );
                                cx.warn(None, Some(span(row, w.place(t.offset()))), message);
                                (u16::MAX, XrefEntry::Free)
                            }
                        };
                        entries.entry(n).or_insert(entry);
                    }
                }
                _ => return Err(format!("the table at byte {at} has no trailer")),
            }
        }
        if let Some((n, at, why)) = first {
            let message = format!(
                "{misshapen} of the table's {rows} rows aren't 20 bytes as ISO 32000-2 7.5.4 writes them, the first object {n}'s: {why}"
            );
            cx.warn(None, Some(at), message);
        }
        let trailer =
            (w.parser(w.place(t.offset())).limits(self.limits.parser)).value(&mut cx.warned(None));
        let trailer = w.parsed(trailer).map_err(|e| e.to_string())?;
        let Some(dict) = trailer.as_dict() else {
            return Err("the trailer isn't a dictionary".to_string());
        };
        let prev = dict.get(b"Prev").and_then(offset_of);
        let xref_stm = dict.get(b"XRefStm").and_then(offset_of);
        let section = Section {
            trailer: Trailer {
                stream: None,
                object: trailer,
                unwritten: None,
            },
            prev,
            table: entries,
            stream: None,
            hybrid: xref_stm.is_some(),
        };
        Ok((section, xref_stm))
    }

    /// The cross-reference stream at `offset`: a stream, `/Type /XRef`.
    fn xref_stream<'f, F: Bytes<'f>>(
        &self,
        file: F,
        cx: &mut Cx,
        offset: usize,
    ) -> Result<Result<(ObjectId, Object), String>, F::Error> {
        let (id, object, _) = match self.indirect(file, cx, offset, None)? {
            Ok(read) => read,
            Err(why) => return Ok(Err(why.to_string())),
        };
        let Value::Stream(stream) = object.value() else {
            return Ok(Err(format!(
                "{} {} R, at byte {offset}, isn't a stream",
                id.0, id.1
            )));
        };
        if stream.dict.get(b"Type").and_then(Object::as_name) != Some(b"XRef") {
            return Ok(Err(format!(
                "{} {} R, at byte {offset}, isn't /Type /XRef",
                id.0, id.1
            )));
        }
        Ok(Ok((id, object)))
    }

    /// The rows of cross-reference stream `id`, written at `offset`, read
    /// by its `/W` and `/Index` (ISO 32000-2, 7.5.8): from its data, decoded
    /// from its own (or, when the reader is made to ask the decoder, as
    /// `decoded` has it). A section's `own` stream lists itself; a hybrid
    /// table's `/XRefStm` is listed by the table.
    #[allow(clippy::too_many_arguments)]
    fn rows<'f, F: Bytes<'f>>(
        &self,
        file: F,
        cx: &mut Cx,
        offset: usize,
        id: ObjectId,
        object: &Object,
        decoded: Option<&Result<Vec<u8>, String>>,
        own: bool,
        decoder: Option<&dyn Decode>,
    ) -> Result<Result<Rows, String>, F::Error> {
        let at = match object.value() {
            Value::Stream(stream) => stream.data.at(),
            _ => None,
        };
        let written = at.map(|at| file.get(at.start..at.end)).transpose()?;
        let written = written.flatten().unwrap_or_default();
        let rows = self.rows_of(
            file, cx, offset, id, object, &written, decoded, own, decoder,
        );
        Ok(rows)
    }

    /// [`Reader::rows`], given the stream's data as `written`.
    #[allow(clippy::too_many_arguments)]
    fn rows_of<'f, F: Bytes<'f>>(
        &self,
        file: F,
        cx: &mut Cx,
        offset: usize,
        id: ObjectId,
        object: &Object,
        written: &[u8],
        decoded: Option<&Result<Vec<u8>, String>>,
        own: bool,
        decoder: Option<&dyn Decode>,
    ) -> Result<Rows, String> {
        let Value::Stream(stream) = object.value() else {
            return Err(format!("{} {} R isn't a stream", id.0, id.1));
        };
        let dict = &stream.dict;
        let widths: Option<Vec<usize>> = match dict.get(b"W").map(|w| w.value()) {
            Some(Value::Array(w)) if w.len() == 3 => (w.iter())
                .map(|n| {
                    n.as_integer()
                        .and_then(|n| usize::try_from(n).ok())
                        .filter(|&n| n <= 8)
                })
                .collect(),
            _ => None,
        };
        let widths = widths.ok_or("its /W isn't three widths of 8 bytes or fewer")?;
        let size = dict.get(b"Size").and_then(Object::as_integer);
        let index: Vec<(u32, u32)> = match dict.get(b"Index").map(|i| i.value()) {
            Some(Value::Array(index)) => (index.chunks(2))
                .map(|pair| match pair {
                    [start, count] => Some((
                        u32::try_from(start.as_integer()?).ok()?,
                        u32::try_from(count.as_integer()?).ok()?,
                    )),
                    _ => None,
                })
                .collect::<Option<_>>()
                .ok_or("its /Index isn't pairs of numbers")?,
            _ => vec![(
                0,
                size.and_then(|s| u32::try_from(s).ok())
                    .ok_or("it has no /Size")?,
            )],
        };
        let row = widths.iter().sum::<usize>();
        let rows_size = (index.iter())
            .try_fold(0usize, |size, &(_, count)| {
                size.checked_add(usize::try_from(count).ok()?)
            })
            .and_then(|count| count.checked_mul(row));
        let again;
        let data: &[u8] = match self.native {
            // From its own data, in any file: a cross-reference stream is
            // never encrypted, and its dictionary's values are direct (ISO
            // 32000-2, 7.5.8.2). What runs past its rows, which aren't
            // read, isn't decoded.
            true => {
                let filters = filter::filters(dict, None)
                    .map_err(|why| format!("its cross-reference stream doesn't decode: {why}"))?;
                match filters.is_empty() {
                    true => written,
                    false => {
                        // A predictor's rows are read whole: its rows, and
                        // one of the predictor's past them.
                        let limit = rows_size.map_or(self.limits.stream, |size| {
                            size.saturating_add(predictor_row(&filters))
                        });
                        let (decoded, mut faults) = self.decode_stream(written, &filters, limit);
                        // Data that runs past its rows is cut off there,
                        // as data that's not filtered is read only so far:
                        // with its rows whole, the limit cut only what's
                        // past them, and a predictor row past them isn't
                        // read.
                        if let Some(size) = rows_size.filter(|&size| decoded.len() >= size) {
                            let row = predictor_row(&filters);
                            faults.retain(|(_, fault)| match *fault {
                                Fault::Limit => false,
                                Fault::RowTag { row: i, .. } => i.saturating_mul(row) < size,
                                Fault::PartialRow { .. } => {
                                    decoded.len().saturating_sub(row) < size
                                }
                                _ => true,
                            });
                        }
                        self.faulted(cx, id, stream.data.at(), stream.data.at(), &filters, faults);
                        again = decoded;
                        &again
                    }
                }
            }
            false => {
                let decoded = match (filters(object), decoded) {
                    (None, _) => Ok(written),
                    (Some(_), Some(Ok(data))) => Ok(&data[..]),
                    (Some(filter), Some(Err(why))) => Err(format!(
                        "its cross-reference stream ({filter}) doesn't decode: {why}"
                    )),
                    (Some(filter), None) => Err(format!(
                        "its cross-reference stream is compressed ({filter}), and nothing decodes it"
                    )),
                };
                // A decoder finds a stream by its number, which a later
                // update can give another object; then the data it gives
                // isn't the size this stream's rows are, and it's decoded
                // from its bytes instead.
                let (n, g) = id;
                let decoded = decoded.and_then(|data| match filters(object) {
                    Some(_) if rows_size != Some(data.len()) => Err(format!(
                        "decoded, it's {} bytes, not the {} its /W and /Index make: the decoder may have decoded another object {n} {g} R",
                        data.len(),
                        rows_size.map_or("many".to_string(), |size| size.to_string()),
                    )),
                    _ => Ok(data),
                });
                match decoded {
                    Ok(data) => data,
                    Err(why) => {
                        let file = file.memory();
                        again = decoder
                            .zip(file.and_then(|file| self.written(file, dict, written)))
                            .and_then(|(decoder, stream)| decoder.decode_written(stream).ok())
                            // Its own data, which may run on past its
                            // rows, as data that's not compressed may.
                            .filter(|data| rows_size.is_some_and(|size| data.len() >= size))
                            .ok_or(why)?;
                        &again
                    }
                }
            }
        };
        let field = |bytes: &[u8]| bytes.iter().fold(0u64, |n, &b| (n << 8) | u64::from(b));
        let mut rows = data.chunks_exact(row.max(1));
        let mut entries = BTreeMap::new();
        // No more entries than [`listable`] allows, so that a small stream
        // can't list hundreds of millions.
        let most = self.listable;
        let mut left = u32::try_from(most).unwrap_or(u32::MAX);
        let mut past = 0usize;
        for (start, count) in index {
            let fit = count.min(left);
            left -= fit;
            let skip = usize::try_from(count - fit).unwrap_or(usize::MAX);
            past = past.saturating_add(skip);
            for i in 0..count {
                if i == fit {
                    if rows.len() < skip {
                        return Err("its data ends before its /Index does".to_string());
                    }
                    if skip > 0 {
                        rows.nth(skip - 1);
                    }
                    break;
                }
                let bytes = rows.next().ok_or("its data ends before its /Index does")?;
                let (one, rest) = bytes.split_at(widths[0]);
                let (two, three) = rest.split_at(widths[1]);
                let kind = if widths[0] == 0 { 1 } else { field(one) };
                let (two, three) = (field(two), field(three));
                let entry = match kind {
                    0 => (u16::try_from(three).unwrap_or(u16::MAX), XrefEntry::Free),
                    1 => {
                        let generation = u16::try_from(three).unwrap_or(u16::MAX);
                        let offset = usize::try_from(two).unwrap_or(usize::MAX);
                        (generation, XrefEntry::InUse { offset, generation })
                    }
                    2 => {
                        let stream = u32::try_from(two).unwrap_or(u32::MAX);
                        let index = u32::try_from(three).unwrap_or(u32::MAX);
                        (0, XrefEntry::Compressed { stream, index })
                    }
                    // Readers treat an unknown type as null (ISO 32000-2,
                    // 7.5.8.3).
                    _ => continue,
                };
                let Some(n) = start.checked_add(i) else {
                    return Err("its /Index numbers no object".to_string());
                };
                entries.entry(n).or_insert(entry);
            }
        }
        if past > 0 {
            let message = format!(
                "its rows list {past} entries past the first {most}, as many as the reader reads of a file of {} bytes, which aren't read",
                file.len()
            );
            cx.warn(Some(id), object.at, message);
        }
        // It lists itself, at the offset it's at (ISO 32000-2, 7.5.8.2).
        let listed = matches!(
            entries.get(&id.0),
            Some(&(g, XrefEntry::InUse { offset: at, .. })) if g == id.1 && at == offset
        );
        if own && !listed {
            let message = format!(
                "cross-reference stream {} {} R doesn't list itself",
                id.0, id.1
            );
            cx.warn(Some(id), object.at, message);
        }
        Ok(entries)
    }

    /// Stream `object` as written, when it's a stream whose filters are
    /// written in its dictionary.
    fn written_stream<'a>(&self, file: &'a [u8], object: &Object) -> Option<Written<'a>> {
        let Value::Stream(stream) = object.value() else {
            return None;
        };
        let data = (stream.data.at()).and_then(|at| file.get(at.start..at.end))?;
        self.written(file, &stream.dict, data)
    }

    /// A stream's filters, their parameters and `data`, as written, when
    /// they're written in its dictionary, not as references.
    fn written<'a>(&self, file: &'a [u8], dict: &Dict, data: &'a [u8]) -> Option<Written<'a>> {
        let as_written = |key: &[u8]| {
            let value = dict.get(key)?;
            let at = (value.at).filter(|_| !matches!(value.value(), Value::Ref(_)))?;
            file.get(at.start..at.end)
        };
        let filter = as_written(b"Filter")?;
        let parms = match dict.has(b"DecodeParms") {
            true => Some(as_written(b"DecodeParms")?),
            false => None,
        };
        Some(Written {
            filter,
            parms,
            data,
        })
    }

    /// The objects found by scanning the file for `n g obj`, a later one
    /// of a number over an earlier, when the sections don't read; and the
    /// last `trailer` that reads, or else the last cross-reference stream.
    ///
    /// Read by position, the file is read whole for it.
    fn scan<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
        cx: &mut Cx,
        why: String,
    ) -> Result<Trailer, F::Error> {
        let all = file.all()?;
        Ok(self.scan_in(&all, cx, why))
    }

    /// [`Reader::scan`] of `file`, every byte.
    fn scan_in(&mut self, file: &[u8], cx: &mut Cx, why: String) -> Trailer {
        let mut entries = BTreeMap::new();
        let mut t = Tokenizer::new(file);
        let mut before: [Option<Token>; 2] = [None, None];
        let mut trailers = Vec::new();
        while let Some(token) = t.next() {
            match token.kind {
                Kind::Space | Kind::Comment => continue,
                Kind::Keyword if token.bytes == b"obj" => {
                    if let [Some(n), Some(g)] = before {
                        if n.kind == Kind::Integer && g.kind == Kind::Integer {
                            if let (Some(number), Some(generation)) = (
                                n.integer().ok().and_then(|n| u32::try_from(n).ok()),
                                g.integer().ok().and_then(|g| u16::try_from(g).ok()),
                            ) {
                                let offset = n.start;
                                entries.insert(
                                    number,
                                    (generation, XrefEntry::InUse { offset, generation }),
                                );
                            }
                        }
                    }
                }
                Kind::Keyword if token.bytes == b"stream" => {
                    t.eol();
                    t.data_until(b"endstream");
                }
                Kind::Keyword if token.bytes == b"trailer" => trailers.push(token.end()),
                _ => {}
            }
            before = [before[1], Some(token)];
        }
        self.entries = entries.into_iter().collect();
        self.scanned = true;
        // The last trailer, with what it lacks of the document's entries
        // from those before it, newest first: a linearized file's last
        // may have only /Size, its first page's the rest.
        let mut found: Option<Object> = None;
        let mut taken = Vec::new();
        for at in trailers.into_iter().rev() {
            let read = Parser::at(file, at)
                .limits(self.limits.parser)
                .value(&mut cx.warned(None));
            let Ok(object) = read else {
                continue;
            };
            let Value::Dict(earlier) = object.value().clone() else {
                continue;
            };
            let Some(trailer) = &mut found else {
                found = Some(object);
                continue;
            };
            let at = trailer.at;
            if let Value::Dict(last) = trailer.value_mut() {
                for key in [&b"Root"[..], b"Encrypt", b"ID", b"Info"] {
                    if let (false, Some(entry)) = (last.has(key), earlier.entry(key)) {
                        last.push_entry(entry.clone());
                        taken.push(format!("/{}", String::from_utf8_lossy(key)));
                    }
                }
            }
            // An edit, which drops its span; but the trailer this makes
            // stands where the last was written, which its warning, and
            // what's reported of it, point at.
            trailer.at = at;
        }
        if let Some(object) = found {
            if !taken.is_empty() {
                let message = format!(
                    "the last trailer lacks {}, which it's given from an earlier one",
                    taken.join(", ")
                );
                cx.warn(None, object.at, message);
            }
            return Trailer {
                stream: None,
                object,
                unwritten: Some(why),
            };
        }
        let mut offsets: Vec<(usize, ObjectId)> = (self.entries.iter())
            .filter_map(|&(n, (g, entry))| match entry {
                XrefEntry::InUse { offset, .. } => Some((offset, (n, g))),
                _ => None,
            })
            .collect();
        offsets.sort();
        for (offset, id) in offsets.into_iter().rev() {
            let Ok(read) = self.indirect(InMemory(file), cx, offset, Some(id));
            if let Ok((_, object, _)) = read {
                if is_xref_stream(&object) {
                    return Trailer {
                        stream: Some(id),
                        object,
                        unwritten: Some(why),
                    };
                }
            }
        }
        Trailer {
            stream: None,
            object: Object::new(Value::Dict(Dict::new())),
            unwritten: Some(why),
        }
    }

    /// Whether the file's strings are encrypted, from the trailer's
    /// `/Encrypt`: they are unless its crypt filter for strings is
    /// `/Identity`, which a version 4 or later dictionary has by default
    /// (ISO 32000-2, 7.6.6 and Table 20). The encryption dictionary's own
    /// strings never are, an earlier update's either, nor those in object
    /// streams, which are encrypted whole. Its security handler is opened
    /// with `passwords`.
    fn encryption<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
        cx: &mut Cx,
        trailer: &Trailer,
        passwords: &[&[u8]],
    ) -> Result<(), F::Error> {
        let Some(encrypt) = trailer.dict().get(b"Encrypt") else {
            return Ok(());
        };
        let (dict, own, why) = match encrypt.value() {
            // A null is no entry, and so is a reference to no object (ISO
            // 32000-2, 7.3.9 and 7.3.10).
            Value::Null => return Ok(()),
            Value::Ref(id) => match self.entry(id.0) {
                Some(&(_, XrefEntry::InUse { offset, .. })) => {
                    match self.indirect(file, cx, offset, Some(*id))? {
                        Ok((_, object, _)) => {
                            (object.as_dict().cloned(), Some(*id), "isn't a dictionary")
                        }
                        Err(_) => (None, Some(*id), "doesn't read"),
                    }
                }
                Some((_, XrefEntry::Compressed { .. })) => (
                    None,
                    Some(*id),
                    "is in an object stream, which it can't be (ISO 32000-2, 7.5.7)",
                ),
                None | Some((_, XrefEntry::Free)) => {
                    let (n, g) = id;
                    let message = format!(
                        "the trailer's /Encrypt is {n} {g} R, which is no object, so the file is read as not encrypted"
                    );
                    cx.warn(None, encrypt.at, message);
                    return Ok(());
                }
            },
            Value::Dict(dict) => (Some(dict.clone()), None, ""),
            _ => (None, None, "isn't a dictionary"),
        };
        self.encrypts = true;
        self.encryption = match &dict {
            None => Encryption::Unsupported(format!("its /Encrypt {why}")),
            Some(dict) => self.open(file, cx, dict, trailer, own, passwords)?,
        };
        // On the trailer, whose /Encrypt it's about.
        if let Encryption::Unsupported(why) = &self.encryption {
            let strings = match self.plain_strings {
                true => "",
                false => "its strings stay ciphertext, and ",
            };
            let message = format!(
                "its security handler doesn't open: {why}; {strings}its object streams aren't read"
            );
            cx.warn(None, encrypt.at, message);
        }
        self.dictionaries.extend(own);
        if self.plain_strings {
            return Ok(());
        }
        self.encrypted = Some(self.dictionaries.clone());
        Ok(())
    }

    /// The security handler of encryption dictionary `dict` (object `own`,
    /// when it's indirect), opened by `passwords`, as far as it opens; its
    /// values followed where they're references, which are its own, so not
    /// encrypted. Whether its strings and metadata are plain is set by it.
    fn open<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
        cx: &mut Cx,
        dict: &Dict,
        trailer: &Trailer,
        own: Option<ObjectId>,
        passwords: &[&[u8]],
    ) -> Result<Encryption, F::Error> {
        let mut get = |o: Option<&Object>| -> Result<Option<Object>, F::Error> {
            let Some(o) = o else {
                return Ok(None);
            };
            Ok(match o.value() {
                Value::Ref(id) => match self.entry(id.0) {
                    Some(&(_, XrefEntry::InUse { offset, .. })) => {
                        self.dictionaries.insert(*id);
                        (self.indirect(file, cx, offset, Some(*id))?.ok()).map(|(_, o, _)| o)
                    }
                    _ => None,
                },
                _ => Some(o.clone()),
            })
        };
        let bytes = |o: Option<Object>| match o.map(|o| o.into_value()) {
            Some(Value::String(s, _)) => Some(s.as_bytes().map(<[u8]>::to_vec).unwrap_or_default()),
            _ => None,
        };
        let name = |o: Option<Object>| o.and_then(|o| o.as_name().map(<[u8]>::to_vec));
        let integer = |o: Option<Object>| o.and_then(|o| o.as_integer());
        let mut crypt_filters = Vec::new();
        if let Some(Value::Dict(filters)) = get(dict.get(b"CF"))?.map(|o| o.into_value()) {
            for (name, filter) in filters.iter() {
                let method = match get(Some(filter))?.map(|o| o.into_value()) {
                    Some(Value::Dict(filter)) => (get(filter.get(b"CFM"))?.as_ref())
                        .and_then(Object::as_name)
                        .map(<[u8]>::to_vec),
                    _ => None,
                };
                crypt_filters.push(pdf_crypt::CryptFilter {
                    name: name.to_vec(),
                    method,
                });
            }
        }
        let values = pdf_crypt::Dictionary {
            filter: name(get(dict.get(b"Filter"))?).unwrap_or_default(),
            v: integer(get(dict.get(b"V"))?).unwrap_or(0),
            r: integer(get(dict.get(b"R"))?).unwrap_or(0),
            length: integer(get(dict.get(b"Length"))?),
            o: bytes(get(dict.get(b"O"))?),
            u: bytes(get(dict.get(b"U"))?),
            oe: bytes(get(dict.get(b"OE"))?),
            ue: bytes(get(dict.get(b"UE"))?),
            perms: bytes(get(dict.get(b"Perms"))?),
            p: integer(get(dict.get(b"P"))?),
            encrypt_metadata: match get(dict.get(b"EncryptMetadata"))?.map(|o| o.into_value()) {
                Some(Value::Bool(b)) => Some(b),
                _ => None,
            },
            crypt_filters,
            stm_f: name(get(dict.get(b"StmF"))?),
            str_f: name(get(dict.get(b"StrF"))?),
        };
        // The file's first /ID element, as qpdf takes it: empty, said once
        // the standard handler takes the dictionary, when it isn't a string.
        let id = match get(trailer.dict().get(b"ID"))?.map(|o| o.into_value()) {
            Some(Value::Array(id)) => match id.first().map(|o| o.value()) {
                Some(Value::String(s, _)) => s.as_bytes().map(<[u8]>::to_vec),
                _ => None,
            },
            _ => None,
        };
        let no_id = id.is_none();
        let opened = Handler::open(&values, id.as_deref().unwrap_or_default(), passwords);
        if no_id && !matches!(opened, Err(pdf_crypt::Refused::Unsupported(_))) {
            let at = trailer.dict().get(b"ID").and_then(|o| o.at);
            let message = "its trailer's /ID has no string first, so its key is made without one";
            cx.warn(None, at, message.to_string());
        }
        self.plain_strings = match &opened {
            Ok(handler) => matches!(handler.strings(), Method::Identity | Method::None),
            Err(_) => values.v >= 4 && (values.str_f.as_deref()).is_none_or(|f| f == b"Identity"),
        };
        // Neither, when no crypt filter encrypts them (from /V 4), whatever
        // the handler.
        let identity = |f: &Option<Vec<u8>>| f.as_deref().is_none_or(|f| f == b"Identity");
        let plain_streams = match &opened {
            Ok(handler) => matches!(handler.streams(), Method::Identity | Method::None),
            Err(_) => values.v >= 4 && identity(&values.stm_f),
        };
        self.unencrypted_metadata = match &opened {
            Ok(handler) => !handler.encrypts_metadata(),
            Err(_) => values.v >= 4 && values.encrypt_metadata == Some(false),
        };
        self.plain_metadata = plain_streams || self.unencrypted_metadata;
        // Said as written: a null is no entry (ISO 32000-2, 7.3.9).
        self.embedded_files = (dict.get(b"EFF")).is_some_and(|o| !matches!(o.value(), Value::Null));
        Ok(match opened {
            Ok(handler) => {
                for fault in handler.faults() {
                    cx.warn(
                        own,
                        None,
                        format!("its encryption dictionary isn't right: {fault}"),
                    );
                }
                let owner = handler.owner();
                self.handler = Some(handler);
                Encryption::Opened { owner }
            }
            // Its strings and streams aren't encrypted, as when only its
            // embedded files are: what handler it is, or the password, no
            // matter (but a stream's own crypt filter).
            Err(_) if self.plain_strings && plain_streams => Encryption::Plain,
            Err(pdf_crypt::Refused::Password) => Encryption::Locked,
            Err(pdf_crypt::Refused::Unsupported(why)) => Encryption::Unsupported(why),
            Err(refused) => Encryption::Unsupported(refused.to_string()),
        })
    }

    /// Object `id`, the `index`th of object stream `stream`, whose header
    /// says where each object starts, read from the stream's decoded data.
    fn in_stream(
        &self,
        cx: &mut Cx,
        id: ObjectId,
        stream: u32,
        header: &[(u32, usize)],
        index: u32,
        container: &Container,
    ) -> Result<(Object, Record), Unread> {
        let &(number, offset) = usize::try_from(index)
            .ok()
            .and_then(|i| header.get(i))
            .ok_or(Unread::NoIndex { stream, index })?;
        if number != id.0 {
            return Err(Unread::Numbered {
                stream,
                index,
                number,
                expected: id.0,
            });
        }
        let data = &container.data;
        let object = (Parser::at(data, offset).in_stream((stream, 0)))
            .limits(self.limits.parser)
            .value(&mut cx.warned(Some(id)))
            .map_err(Unread::Parse)?;
        // One that runs to where a fault cut the data off may be cut short,
        // when it's a value nothing closes: a number, a name, a keyword.
        let open = |o: &Object| {
            !matches!(
                o.value(),
                Value::String(..) | Value::Array(_) | Value::Dict(_) | Value::Stream(_)
            )
        };
        let cut =
            container.cut && open(&object) && object.at.is_some_and(|at| at.end == data.len());
        if cut {
            return Err(Unread::Cut { stream });
        }
        // Every value the parser reads is placed.
        let at = (object.at).unwrap_or(Span::new(Some((stream, 0)), offset, offset));
        let record = Record {
            source: self.id,
            origin: Origin::Stream { stream, index },
            at,
            endobj: false,
            data: None,
        };
        Ok((object, record))
    }

    /// The indirect object at `offset`: `n g obj`, a value (a dictionary
    /// with its stream), `endobj`. When `expected`, it must be that one. A
    /// stream's data is as long as its `/Length` says when that ends at
    /// `endstream`, else up to `endstream`. With it, where it was read
    /// from: from its `n` to past its `endobj`, the reader looking past the
    /// parser as the parser's own check for `endobj` looks.
    ///
    /// Read by position, it's read in a window from `offset`, at first to
    /// a byte past where it can end ([`Reader::bound`]), so that an object
    /// as it should be is one read, and larger while what it reads runs to
    /// the window's end.
    fn indirect<'f, F: Bytes<'f>>(
        &self,
        file: F,
        cx: &mut Cx,
        offset: usize,
        expected: Option<ObjectId>,
    ) -> Result<Indirect, F::Error> {
        let bound = self.bound(offset, file.len()).filter(|_| file.bounded());
        let min = bound.map_or(0, |bound| bound.saturating_sub(offset).saturating_add(1));
        file.read_from(cx, offset, min, |w, cx| {
            self.indirect_in(file, w, cx, offset, expected)
        })
    }

    /// [`Reader::indirect`], in window `w`.
    fn indirect_in<'f, F: Bytes<'f>>(
        &self,
        file: F,
        w: &Window<'_>,
        cx: &mut Cx,
        offset: usize,
        expected: Option<ObjectId>,
    ) -> Result<Indirect, F::Error> {
        let mut parser = w.parser(offset).limits(self.limits.parser);
        let id = match w.parsed(parser.header()) {
            Ok(id) => id,
            Err(error) => return Ok(Err(Unread::Parse(error))),
        };
        if let Some(expected) = expected {
            if expected != id {
                return Ok(Err(Unread::Elsewhere {
                    offset,
                    found: id,
                    expected,
                }));
            }
        }
        let mut parser = parser.ciphertext(self.ciphertext(id));
        let body = w.parsed(parser.body(id, &mut cx.warned(Some(id))));
        let body = match body {
            Ok(body) => body,
            Err(error) => return Ok(Err(Unread::Parse(error))),
        };
        let (object, data) = match body {
            Body::Object(object) => (object, None),
            Body::Stream(head) => {
                let length = self.length(file, cx, head.dict(), id)?;
                let declared = length.and_then(|len| head.declared(len));
                let data = match declared {
                    Some(data) => data,
                    None => {
                        let message = match length {
                            Some(len) => format!("its /Length, {len}, doesn't end at endstream"),
                            None => "it has no /Length that reads".to_string(),
                        };
                        let at = (head.dict().get(b"Length"))
                            .and_then(|l| l.at)
                            .unwrap_or(head.keyword());
                        cx.warn(Some(id), Some(at), message);
                        head.scanned()
                    }
                };
                let extent = DataExtent {
                    at: data,
                    declared: declared.is_some(),
                    indirect_length: (head.dict().get(b"Length"))
                        .is_some_and(|l| matches!(l.value(), Value::Ref(_))),
                };
                let object = w.parsed(parser.stream(head, data, &mut cx.warned(Some(id))));
                match object {
                    Ok(object) => (object, Some(extent)),
                    Err(error) => return Ok(Err(Unread::Parse(error))),
                }
            }
        };
        let end = parser.offset();
        let endobj = (w.significant(&mut w.tokenizer(end)))
            .filter(|token| is_keyword(Some(token), b"endobj"));
        let start = (w.significant(&mut w.tokenizer(offset))).map_or(offset, |n| w.place(n.start));
        let end = endobj.as_ref().map_or(end, |token| w.place(token.end()));
        let record = Record {
            source: self.id,
            origin: Origin::File,
            at: span(start, end),
            endobj: endobj.is_some(),
            data,
        };
        Ok(Ok((id, object, record)))
    }

    /// A stream's `/Length`: an integer, or a reference to one written in
    /// plain view, read again for it, its faults the stream's.
    fn length<'f, F: Bytes<'f>>(
        &self,
        file: F,
        cx: &mut Cx,
        dict: &Dict,
        id: ObjectId,
    ) -> Result<Option<usize>, F::Error> {
        let Some(length) = dict.get(b"Length") else {
            return Ok(None);
        };
        let n = match *length.value() {
            Value::Integer(n) => n,
            Value::Ref(to) if to != id && !cx.resolving => {
                let Some(&(_, XrefEntry::InUse { offset, .. })) = self.entry(to.0) else {
                    return Ok(None);
                };
                cx.resolving = true;
                let found = self.indirect(file, cx, offset, Some(to));
                cx.resolving = false;
                match found?.ok().and_then(|(_, o, _)| o.as_integer()) {
                    Some(n) => n,
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
        Ok(usize::try_from(n).ok())
    }
}

/// An object stream's header, from its decoded `data`: each object's
/// number and where it starts, from the stream's `/First` (ISO 32000-2,
/// 7.5.7). It's read as far as its `/N` says, and no further than the
/// data holds pairs of integers.
fn object_stream_header(object: &Object, data: &[u8]) -> Result<Vec<(u32, usize)>, Unindexed> {
    let dict = object.as_dict().ok_or(Unindexed::NotAStream)?;
    if dict.get(b"Type").and_then(Object::as_name) != Some(b"ObjStm") {
        return Err(Unindexed::NotAnObjectStream);
    }
    let count = dict
        .get(b"N")
        .and_then(Object::as_integer)
        .and_then(|n| usize::try_from(n).ok());
    let first = dict
        .get(b"First")
        .and_then(Object::as_integer)
        .and_then(|n| usize::try_from(n).ok());
    let (Some(count), Some(first)) = (count, first) else {
        return Err(Unindexed::Counts);
    };
    let mut t = Tokenizer::new(data.get(..first.min(data.len())).unwrap_or_default());
    let mut header = Vec::new();
    while header.len() < count {
        let integer = |t: &mut Tokenizer| {
            (significant(t))
                .filter(|token| token.kind == Kind::Integer)
                .and_then(|token| token.integer().ok())
        };
        let (Some(number), Some(offset)) = (integer(&mut t), integer(&mut t)) else {
            return Err(Unindexed::Pairs {
                pairs: header.len(),
                count,
            });
        };
        let number = u32::try_from(number).map_err(|_| Unindexed::Number)?;
        let offset = usize::try_from(offset)
            .ok()
            .and_then(|offset| offset.checked_add(first))
            .ok_or(Unindexed::Offset)?;
        header.push((number, offset));
    }
    Ok(header)
}

/// Whether `o` is a cross-reference stream.
fn is_xref_stream(o: &Object) -> bool {
    matches!(o.value(), Value::Stream(s) if is_xref(&s.dict))
}

/// Whether `dict` is a cross-reference stream's.
fn is_xref(dict: &Dict) -> bool {
    dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef")
}

/// In an encrypted file it opened, object `id`'s strings in plain view
/// decrypted by `handler`, each where it is, but a signature's `/Contents`,
/// which isn't encrypted; one that doesn't decrypt cleanly said, once an
/// object and a fault.
fn decrypt_strings(handler: &Handler, id: ObjectId, object: &mut Object) -> Vec<Warning> {
    if ciphertexts(object.value()) == 0 {
        return Vec::new();
    }
    let mut faults: BTreeMap<String, (usize, Option<Span>)> = BTreeMap::new();
    plaintext(object, id, handler, &mut faults);
    (faults.into_iter())
        .map(|(fault, (count, at))| {
            let message = match count {
                1 => format!("a string of it doesn't decrypt cleanly: {fault}"),
                n => format!("{n} of its strings don't decrypt cleanly: {fault}"),
            };
            Warning {
                object: Some(id),
                at,
                message,
            }
        })
        .collect()
}

/// In an `encrypted` file, the strings in plain view decrypted by
/// `decoder`: the objects holding ciphertext asked of it, and each string
/// replaced by the one where it is in the decoder's object. A string it
/// can't place stays ciphertext, and so does one the decoder can't
/// decrypt, and the reader says so, in `warnings`.
fn decrypt(
    decoder: Option<&dyn Decode>,
    encrypted: bool,
    objects: &mut BTreeMap<ObjectId, Result<Object, Unread>>,
    warnings: &mut Vec<Warning>,
) {
    let Some(decoder) = decoder.filter(|_| encrypted) else {
        return;
    };
    let ids: Vec<ObjectId> = (objects.iter())
        .filter(|(_, o)| o.as_ref().is_ok_and(|o| ciphertexts(o.value()) > 0))
        .map(|(&id, _)| id)
        .collect();
    if ids.is_empty() {
        return;
    }
    let mut why_not: BTreeMap<String, usize> = BTreeMap::new();
    for (id, plain) in decoder.plaintext(&ids) {
        let Some(Ok(object)) = objects.get_mut(&id) else {
            continue;
        };
        match plain {
            Ok(plain) => {
                let left = decrypted(object, &plain);
                if left > 0 {
                    let (n, g) = id;
                    let message = format!(
                        "{left} of {n} {g} R's strings stay ciphertext: the decoder reads the object otherwise"
                    );
                    warnings.push(Warning::new(Some(id), object.at, message));
                }
            }
            Err(why) => *why_not.entry(why).or_default() += 1,
        }
    }
    for (why, objects) in why_not {
        let objects = match objects {
            1 => "one object's".to_string(),
            n => format!("{n} objects'"),
        };
        let message = format!("{objects} strings stay ciphertext: {why}");
        warnings.push(Warning::new(None, None, message));
    }
}

/// `o`'s ciphertext strings, of object `id`, decrypted by `handler`, each
/// where it is; a signature's `/Contents` (a dictionary with `/ByteRange`
/// and a string `/Contents`, of `/Type /Sig`, `/DocTimeStamp` or none)
/// taken as plain, as it isn't encrypted (ISO 32000-2, 7.6.2). Each fault
/// counted in `faults`, with where it was first.
fn plaintext(
    o: &mut Object,
    id: ObjectId,
    handler: &Handler,
    faults: &mut BTreeMap<String, (usize, Option<Span>)>,
) {
    let at = o.at;
    // A dictionary's values, or a stream's dictionary's.
    let mut entries = |d: &mut Dict| {
        let contents = matches!(
            d.get(b"Contents").map(|c| c.value()),
            Some(Value::String(..))
        );
        let typed =
            (d.get(b"Type")).is_none_or(|t| matches!(t.as_name(), Some(b"Sig" | b"DocTimeStamp")));
        let signature = d.has(b"ByteRange") && contents && typed;
        for entry in d.entries_mut() {
            match signature && entry.key == b"Contents" {
                true => plain_strings(&mut entry.value),
                false => plaintext(&mut entry.value, id, handler, faults),
            }
        }
    };
    match o.as_written_mut() {
        Value::String(s, _) => {
            if let Str::Encrypted(bytes) = s {
                match handler.decrypt_string(id, bytes) {
                    Ok(plain) => *s = Str::Bytes(plain),
                    Err(error) => {
                        faults.entry(error.fault.to_string()).or_insert((0, at)).0 += 1;
                        if let Some(plain) = error.data {
                            *s = Str::Bytes(plain);
                        }
                    }
                }
            }
        }
        Value::Array(a) => {
            for o in a {
                plaintext(o, id, handler, faults);
            }
        }
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&mut s.dict),
        _ => {}
    }
}

/// `o` with each string in it taken as plain, not ciphertext.
fn plain_strings(o: &mut Object) {
    let mut stack = vec![o];
    while let Some(o) = stack.pop() {
        match o.as_written_mut() {
            Value::String(s, _) => {
                if let Str::Encrypted(bytes) = s {
                    *s = Str::Bytes(std::mem::take(bytes));
                }
            }
            Value::Array(a) => stack.extend(a.iter_mut()),
            Value::Dict(d) => stack.extend(d.entries_mut().map(|entry| &mut entry.value)),
            Value::Stream(s) => stack.extend(s.dict.entries_mut().map(|entry| &mut entry.value)),
            _ => {}
        }
    }
}

/// How many strings in `value` are ciphertext.
fn ciphertexts(value: &Value) -> usize {
    let entries = |d: &Dict| -> usize {
        d.entries()
            .iter()
            .map(|e| ciphertexts(e.value.value()))
            .sum()
    };
    match value {
        Value::String(Str::Encrypted(_), _) => 1,
        Value::Array(a) => a.iter().map(|o| ciphertexts(o.value())).sum(),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&s.dict),
        _ => 0,
    }
}

/// `object`'s ciphertext strings, each replaced by the string where it is in
/// `plain`, the same object as the decoder read it; how many it couldn't
/// place, which stay ciphertext. Of a key written twice, only the last
/// entry has a place in `plain`, as a reader keeps only that.
fn decrypted(object: &mut Object, plain: &Object) -> usize {
    // Two dictionaries' entries, or two streams' dictionaries'.
    let entries = |d: &mut Dict, p: &Dict| -> usize {
        let mut placed = HashSet::new();
        let mut entries: Vec<&mut Entry> = d.entries_mut().collect();
        entries.reverse();
        (entries.into_iter())
            .map(|entry| match p.get(&entry.key) {
                Some(p) if placed.insert(entry.key.clone()) => decrypted(&mut entry.value, p),
                _ => ciphertexts(entry.value.value()),
            })
            .sum()
    };
    match (object.as_written_mut(), plain.value()) {
        (
            Value::String(s @ Str::Encrypted(_), _),
            Value::String(p @ (Str::Bytes(_) | Str::Text(_)), _),
        ) => {
            *s = p.clone();
            0
        }
        (Value::Array(a), Value::Array(p)) if a.len() == p.len() => {
            a.iter_mut().zip(p).map(|(a, p)| decrypted(a, p)).sum()
        }
        (Value::Dict(d), Value::Dict(p)) => entries(d, p),
        (Value::Stream(d), Value::Stream(p)) => entries(&mut d.dict, &p.dict),
        (value, _) => ciphertexts(value),
    }
}

/// Whether the cross-reference table's row at `row` is written as ISO
/// 32000-2 7.5.4 writes one, 20 bytes: a 10-digit offset, a space, a
/// 5-digit generation, a space, `n` or `f`, and a two-byte end of line
/// (a space and CR or LF, or CR LF); else what's wrong.
fn row_shape(file: &[u8], row: usize) -> Result<(), &'static str> {
    let b = file.get(row..).unwrap_or_default();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    let fields = digits(0..10)
        && b.get(10) == Some(&b' ')
        && digits(11..16)
        && b.get(16) == Some(&b' ')
        && matches!(b.get(17), Some(b'n' | b'f'));
    if !fields {
        return Err("it isn't ten digits, a space, five digits, a space and n or f");
    }
    match b.get(18..20) {
        Some(b" \r" | b" \n" | b"\r\n") => Ok(()),
        _ => Err("its end of line isn't a space and CR or LF, or CR LF"),
    }
}

/// An offset an integer gives.
fn offset_of(o: &Object) -> Option<usize> {
    usize::try_from(o.as_integer()?).ok()
}

/// Whether a linearization holds in `file`, whose header, `%PDF-`, starts
/// at `header` (ISO 32000-2, Annex F): its first object, whole within the
/// 1024 bytes from the header, is a dictionary whose `/Linearized` is 1,
/// the format's version (or a real whose floor is 1, as qpdf reads it),
/// and whose `/L`, a direct integer, is the file's length counted from the
/// header. A first object that doesn't end within those bytes (qpdf asks
/// only that it start there: Annex F has it all within them), a
/// `/Linearized` of another value or type, or an `/L` that's missing,
/// indirect, a real or another length, says it doesn't; and so does a
/// stream's dictionary.
///
/// Its dictionary is read from those bytes and one more, which settles a
/// token that ends at the last of them, so that a large first object costs
/// no more; what follows it, to tell a stream, is read from where it ends.
fn holds_linearization<'f, F: Bytes<'f>>(
    file: F,
    header: usize,
    limits: parse::Limits,
) -> Result<bool, F::Error> {
    let length = file.len() - header;
    let end = header.saturating_add(1025).min(file.len());
    let Some(head) = file.get(header..end)? else {
        return Ok(false);
    };
    let mut parser = (Parser::at(&head, 0).window(header, end == file.len())).limits(limits);
    if parser.header().is_err() {
        return Ok(false);
    }
    // A dictionary that doesn't end within the bytes read runs past the
    // 1024, and so does one the window cuts short.
    let Ok(first) = parser.value(&mut ()) else {
        return Ok(false);
    };
    let within = first
        .at
        .is_some_and(|at| at.end <= header.saturating_add(1024));
    let Some(dict) = first.as_dict().filter(|_| within) else {
        return Ok(false);
    };
    let version = match dict.get(b"Linearized").map(Object::value) {
        Some(&Value::Integer(v)) => v == 1,
        Some(&Value::Real(v)) => v.floor() == 1.0,
        _ => false,
    };
    let l = matches!(dict.get(b"L").map(Object::value), Some(&Value::Integer(l)) if usize::try_from(l) == Ok(length));
    if !(version && l) {
        return Ok(false);
    }
    // A stream's dictionary when `stream` follows it.
    let after = parser.offset();
    file.read_from(&mut Cx::default(), after, 0, |w, _| {
        let next = w.significant(&mut w.tokenizer(after));
        Ok(!is_keyword(next.as_ref(), b"stream"))
    })
}

#[cfg(test)]
mod lazy;
#[cfg(test)]
mod streams;
#[cfg(test)]
mod words;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use pdf_objects::dict;
    use pdf_objects::object::Form;

    use super::*;
    use crate::source::Source;
    use crate::write::{self, Body, Xref};

    /// `o` with every place it was written, and every string's form, left
    /// out.
    fn plain(o: &Object) -> Object {
        Object::new(match o.value() {
            Value::String(s, _) => Value::String(s.clone(), None),
            Value::Array(a) => Value::Array(a.iter().map(plain).collect()),
            Value::Dict(d) => Value::Dict(plain_dict(d)),
            Value::Stream(s) => Object::stream(plain_dict(&s.dict)).into_value(),
            other => other.clone(),
        })
    }

    /// [`plain`], with ciphertext as bytes: a value read again outside
    /// its object doesn't know its file is encrypted.
    fn plain_bytes(o: &Object) -> Object {
        let mut o = plain(o);
        plain_strings(&mut o);
        o
    }

    fn plain_dict(d: &Dict) -> Dict {
        let mut plain_d = Dict::new();
        for entry in d.entries() {
            plain_d.push(&entry.key, plain(&entry.value));
        }
        plain_d
    }

    /// Checks that every value in `o`, at every depth, was written where
    /// it says: read again from there, it's itself, there. A stream's
    /// place runs from its dictionary through `endstream`, its data's
    /// inside it; a key's starts with its `/`.
    fn placed(file: &[u8], o: &Object) {
        placed_as(file, o, true);
    }

    /// [`placed`], comparing strings' values when `strings`: an encrypted
    /// file's are plaintext, which their bytes, read again, aren't.
    fn placed_as(file: &[u8], o: &Object, strings: bool) {
        let same = |o: &Object| {
            let mut o = plain_bytes(o);
            if !strings {
                let mut stack = vec![&mut o];
                while let Some(o) = stack.pop() {
                    match o.value_mut() {
                        Value::String(s, _) => *s = Str::Bytes(Vec::new()),
                        Value::Array(a) => stack.extend(a.iter_mut()),
                        Value::Dict(d) => {
                            stack.extend(d.entries_mut().map(|entry| &mut entry.value));
                        }
                        Value::Stream(s) => {
                            stack.extend(s.dict.entries_mut().map(|entry| &mut entry.value));
                        }
                        _ => {}
                    }
                }
            }
            o
        };
        let at =
            o.at.expect("a value from the file says where it was written");
        let written = &file[at.start..at.end];
        assert!(!written.is_empty(), "a value is a byte or more");
        let (first, last) = (written[0], written[written.len() - 1]);
        assert!(
            !is_white_space(first) && !is_white_space(last),
            "{:?}",
            String::from_utf8_lossy(written)
        );
        let ends =
            |open: &[u8], close: &[u8]| written.starts_with(open) && written.ends_with(close);
        match o.value() {
            Value::Dict(_) | Value::Stream(_) => assert!(written.starts_with(b"<<")),
            Value::Array(_) => assert!(ends(b"[", b"]")),
            Value::Name(_) => assert_eq!(first, b'/'),
            Value::String(_, Some(Form::Literal)) => assert!(ends(b"(", b")")),
            Value::String(_, Some(Form::Hex)) => assert!(ends(b"<", b">")),
            Value::String(_, None) => panic!("a string read from its bytes has its form"),
            Value::Ref(_) => assert_eq!(last, b'R'),
            _ => {}
        }
        // Read again from the file cut at the value's end: a place that
        // ends short leaves it unfinished.
        let cut = &file[..at.end];
        let again = Parser::at(cut, at.start).value(&mut ()).unwrap();
        match o.value() {
            Value::Stream(s) => {
                let dict = same(&Object::new(Value::Dict(s.dict.clone())));
                assert_eq!(same(&again), dict);
                assert!(file[at.start..at.end].ends_with(b"endstream"));
                let data = s.data.at().expect("a stream says where its data is");
                assert!(at.start < data.start && data.end < at.end);
            }
            _ => {
                assert_eq!(same(&again), same(o));
                // Read again, it's placed in what it's read from.
                let range = |o: &Object| o.at.map(|at| at.start..at.end);
                assert_eq!(
                    range(&again),
                    range(o),
                    "{:?}",
                    String::from_utf8_lossy(&file[at.start..at.end])
                );
            }
        }
        let entries = |d: &Dict| {
            for entry in d.entries() {
                let key_at = entry
                    .key_at
                    .expect("a key from the file says where it was written");
                assert_eq!(file[key_at.start], b'/');
                placed_as(file, &entry.value, strings);
            }
        };
        match o.value() {
            Value::Array(a) => a.iter().for_each(|e| placed_as(file, e, strings)),
            Value::Dict(d) => entries(d),
            Value::Stream(s) => entries(&s.dict),
            _ => {}
        }
    }

    fn numbers(n: &[f64]) -> Object {
        n.iter()
            .map(|&n| Object::from(n))
            .collect::<Vec<_>>()
            .into()
    }

    /// A catalog, its page tree, a page with an annotation, and the
    /// annotation's appearance stream; every kind of value somewhere.
    pub(super) fn clean() -> Vec<(u32, Body)> {
        vec![
            (
                1,
                dict! { "Type" => "Catalog", "Pages" => Object::reference(2) }.into(),
            ),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![Object::reference(3)], "Count" => 1 }
                    .into(),
            ),
            (
                3,
                dict! {
                    "Type" => "Page",
                    "Parent" => Object::reference(2),
                    "MediaBox" => numbers(&[0.0, 0.0, 612.5, 792.0]),
                    "Annots" => vec![Object::reference(4)],
                }
                .into(),
            ),
            (
                4,
                dict! {
                    "Type" => "Annot",
                    "Subtype" => "Text",
                    "Rect" => numbers(&[100.0, 100.0, 120.0, 120.0]),
                    "Contents" => Object::string("A (note) é\\"),
                    "NM" => Object::bytes(vec![0, 255, b'\r']),
                    "A#B" => true,
                    "Gone" => Object::null(),
                    "F" => -4,
                    "AP" => dict! { "N" => Object::reference(5) },
                }
                .into(),
            ),
            (
                5,
                Body::stream(dict! { "Subtype" => "Form" }, b"0 0 m 20 20 l S"),
            ),
        ]
    }

    pub(super) fn root() -> Dict {
        dict! { "Root" => Object::reference(1) }
    }

    #[test]
    fn a_file_reads_as_written_with_every_value_where_it_was_written() {
        for kind in [Xref::Table, Xref::Stream] {
            let file = write::file("1.7", &clean(), root(), kind);
            let doc = read(&file);
            assert!(doc.warnings.is_empty(), "{kind:?}: {:?}", doc.warnings);
            assert_eq!(doc.header, Some(Version::new(1, 7)));
            assert_eq!(doc.trailer.unwritten, None);
            assert_eq!(doc.trailer.stream.is_some(), kind == Xref::Stream);
            for (n, body) in clean() {
                let read = doc.get((n, 0)).unwrap();
                match body {
                    Body::Object(o) => assert_eq!(plain(read), plain(&o)),
                    Body::Stream(mut d, data) => {
                        let Value::Stream(s) = read.value() else {
                            panic!("{n} 0 R isn't a stream");
                        };
                        d.set("Length", data.len() as i64);
                        assert_eq!(plain_dict(&s.dict), plain_dict(&d));
                        let at = s.data.at().unwrap();
                        assert_eq!(&file[at.start..at.end], &data[..]);
                    }
                }
                placed(&file, read);
            }
            placed(&file, &doc.trailer.object);
            assert!((1..=5).all(|n| doc.ids().contains(&(n, 0))));
        }
    }

    #[test]
    fn an_indirect_length_is_followed() {
        let file = write::assembled(
            "1.7",
            &[
                (1, b"<< /Type /Catalog >>"),
                (2, b"<< /Length 3 0 R >>\nstream\r\nabcdef\nendstream"),
                (3, b"6"),
            ],
            "<< /Size 4 /Root 1 0 R >>",
        );
        let doc = read(&file);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let Value::Stream(s) = doc.get((2, 0)).unwrap().value() else {
            panic!("not a stream");
        };
        let at = s.data.at().unwrap();
        assert_eq!(&file[at.start..at.end], b"abcdef");
        assert_eq!(
            *s.dict.get(b"Length").unwrap().value(),
            Value::Ref((3, 0)),
            "as written"
        );
    }

    #[test]
    fn a_length_that_misses_endstream_is_said_and_the_data_found_by_it() {
        let file = write::assembled(
            "1.7",
            &[
                (1, b"<< /Type /Catalog >>"),
                (2, b"<< /Length 99 >>\nstream\nabcdef\nendstream"),
                (3, b"<< >>\rstream\rxy\nendstream"),
                (4, b"<< /Length 2 >>stream\nxy\nendstream"),
            ],
            "<< /Size 5 /Root 1 0 R >>",
        );
        let doc = read(&file);
        let messages: Vec<&str> = doc.warnings.iter().map(|w| w.message.as_str()).collect();
        assert!(
            messages.contains(&"its /Length, 99, doesn't end at endstream"),
            "{messages:?}"
        );
        assert!(
            messages.contains(&"it has no /Length that reads"),
            "{messages:?}"
        );
        assert!(
            messages.contains(&"a lone CR after `stream`, where ISO 32000-2 wants CR LF or LF"),
            "{messages:?}"
        );
        let data = |n: u32| match doc.get((n, 0)).unwrap().value() {
            Value::Stream(s) => {
                let at = s.data.at().unwrap();
                file[at.start..at.end].to_vec()
            }
            _ => panic!("{n} 0 R isn't a stream"),
        };
        assert_eq!(data(2), b"abcdef\n");
        assert_eq!(data(3), b"xy\n");
        assert_eq!(data(4), b"xy");
        assert!(
            doc.warnings
                .iter()
                .all(|w| w.object.is_some() && w.at.is_some())
        );
    }

    /// Where the last `startxref` points, in `file`.
    fn startxref(file: &[u8]) -> usize {
        let at = rfind(file, b"startxref").unwrap() + 9;
        let digits: String = (file[at..].iter())
            .skip_while(|b| b.is_ascii_whitespace())
            .take_while(|b| b.is_ascii_digit())
            .map(|&b| b as char)
            .collect();
        digits.parse().unwrap()
    }

    #[test]
    fn an_update_reads_newest_first() {
        let base: Vec<(u32, Body)> = vec![
            (1, dict! { "Type" => "Catalog" }.into()),
            (2, dict! { "A" => 1 }.into()),
            (3, dict! { "B" => 2 }.into()),
        ];
        let mut file = write::file("1.7", &base, root(), Xref::Table);
        let prev = startxref(&file);
        let changed = file.len();
        file.extend_from_slice(b"2 0 obj\n<< /A 9 >>\nendobj\n");
        let xref = file.len();
        file.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \n2 2\n{changed:010} 00000 n \n0000000000 00001 f \n\
                 trailer\n<< /Size 4 /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let doc = read(&file);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let a = doc
            .get((2, 0))
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"A")
            .unwrap();
        assert_eq!(a.as_integer(), Some(9));
        assert_eq!(doc.ids(), [(1, 0), (2, 0)], "3 is freed");
        assert!(doc.get((3, 0)).is_err());
        assert!(doc.trailer.dict().has(b"Prev"));

        // The update wrote 2, and freed 3; the revision before it is the
        // file read from the section it follows.
        let update = doc.update.expect("an update");
        assert_eq!(update.objects, BTreeSet::from([(2, 0)]));
        assert_eq!(update.previous, prev);
        let before = read_from(&file, update.previous);
        assert!(before.warnings.is_empty(), "{:?}", before.warnings);
        assert_eq!(before.ids(), [(1, 0), (2, 0), (3, 0)]);
        let a = before.get((2, 0)).unwrap().as_dict().unwrap().get(b"A");
        assert_eq!(a.and_then(Object::as_integer), Some(1));
        assert_eq!(before.update, None, "one section");
        assert_eq!(
            read(&write::file("1.7", &base, root(), Xref::Table)).update,
            None
        );

        // A linearized file's first section, read first, is written before
        // the one it follows: it isn't an update.
        let mut file = b"%PDF-1.7\n".to_vec();
        let one = file.len();
        file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let first = file.len();
        let last = first + 120;
        let section = format!(
            "xref\n0 2\n0000000000 65535 f \n{one:010} 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R /Prev {last} >>\n"
        );
        file.extend_from_slice(section.as_bytes());
        file.resize(last, b' ');
        file.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 2 >>\n");
        file.extend_from_slice(format!("startxref\n{first}\n%%EOF\n").as_bytes());
        let doc = read(&file);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert!(doc.get((1, 0)).is_ok());
        assert_eq!(doc.update, None);
    }

    #[test]
    fn object_0_is_never_an_object() {
        let mut file = write::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..])],
            "<< /Size 2 /Root 1 0 R >>",
        );
        let xref = startxref(&file);
        let row = xref + b"xref\n0 2\n".len();
        assert_eq!(&file[row..row + 18], b"0000000000 65535 f");
        file[row..row + 18].copy_from_slice(b"0000000009 00000 n");
        let doc = read(&file);
        assert_eq!(doc.ids(), [(1, 0)]);
        assert_eq!(doc.warnings.len(), 1, "{:?}", doc.warnings);
        assert!(doc.warnings[0].message.contains("object 0"));
    }

    #[test]
    fn a_table_row_out_of_range_is_read_as_free_and_the_rest_read() {
        let mut file = write::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog /Pages 3 0 R >>"[..]),
                (2, b"<< /A 1 >>"),
                (3, b"<< /Type /Pages /Kids [] /Count 0 >>"),
            ],
            "<< /Size 4 /Root 1 0 R >>",
        );
        let xref = startxref(&file);
        // Object 2's row, its generation one past the largest.
        let row = xref + b"xref\n0 4\n".len() + 2 * 20;
        assert_eq!(&file[row + 11..row + 16], b"00000");
        file[row + 11..row + 16].copy_from_slice(b"65536");
        let doc = read(&file);
        assert_eq!(doc.trailer.unwritten, None);
        assert!(doc.get((1, 0)).is_ok() && doc.get((3, 0)).is_ok());
        assert!(doc.get((2, 0)).is_err(), "read as free");
        assert_eq!(doc.warnings.len(), 1, "{:?}", doc.warnings);
        let warning = &doc.warnings[0];
        assert!(warning.message.contains("object 2"), "{warning:?}");
        let at = warning.at.unwrap();
        let written = &file[at.start..at.end];
        assert_eq!(at.start, row, "the row");
        assert!(
            written.ends_with(b" 65536 n") && written.len() == 18,
            "{:?}",
            String::from_utf8_lossy(written)
        );
    }

    /// The warnings reading `file` gives, and where each is.
    fn warned(file: &[u8]) -> Vec<(String, Option<ObjectId>, Option<std::ops::Range<usize>>)> {
        (read(file).warnings.into_iter())
            .map(|w| (w.message, w.object, w.at.map(|at| at.start..at.end)))
            .collect()
    }

    /// A catalog and an empty page tree, with a table.
    pub(super) fn small() -> Vec<u8> {
        write::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog /Pages 2 0 R >>"[..]),
                (2, b"<< /Type /Pages /Kids [] /Count 0 >>"),
            ],
            "<< /Size 3 /Root 1 0 R >>",
        )
    }

    #[test]
    fn a_tables_rows_not_twenty_bytes_are_said_once_with_the_first() {
        let file = small();
        assert_eq!(warned(&file), []);
        let rows = startxref(&file) + b"xref\n0 3\n".len();
        let with_rows = |row: &dyn Fn(&[u8]) -> Vec<u8>| {
            let table: Vec<u8> = file[rows..rows + 60].chunks(20).flat_map(row).collect();
            [&file[..rows], &table, &file[rows + 60..]].concat()
        };
        // A bare LF, each row 19 bytes, as many writers end them; or CR LF,
        // as ISO 32000-2 allows.
        let lf = with_rows(&|r| [&r[..18], b"\n"].concat());
        let warnings = warned(&lf);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let (message, object, at) = &warnings[0];
        assert_eq!(
            message,
            "3 of the table's 3 rows aren't 20 bytes as ISO 32000-2 7.5.4 writes them, the first object 0's: its end of line isn't a space and CR or LF, or CR LF"
        );
        assert_eq!(
            (*object, at.as_ref().map(|at| at.start)),
            (None, Some(rows))
        );
        assert!(read(&lf).get((2, 0)).is_ok(), "it reads all the same");
        let crlf = with_rows(&|r| [&r[..18], b"\r\n"].concat());
        assert_eq!(warned(&crlf), []);
        // A row of numbers not as wide as the table's.
        let narrow = with_rows(&|r| match r.ends_with(b" f \n") {
            true => r.to_vec(),
            false => format!(
                "{} 0 n \n",
                String::from_utf8_lossy(&r[..10]).trim_start_matches('0')
            )
            .into_bytes(),
        });
        let warnings = warned(&narrow);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].0.starts_with("2 of the table's 3 rows")
                && warnings[0].0.ends_with(
                    "object 1's: it isn't ten digits, a space, five digits, a space and n or f"
                ),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_file_whose_last_line_isnt_eof_alone_is_said() {
        let file = small();
        assert!(file.ends_with(b"\n%%EOF\n"));
        let end = file.len();
        let without = |cut: usize| file[..end - cut].to_vec();
        // Its end of line, any or none.
        for ok in [
            file.clone(),
            without(1),
            [&file[..end - 1], b"\r\n"].concat(),
        ] {
            assert_eq!(warned(&ok), []);
        }
        // Bytes after it.
        let padded = [&file[..], b"\0\0\0"].concat();
        let said = |file: &[u8]| {
            let warnings = warned(file);
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            let (message, object, at) = warnings.into_iter().next().unwrap();
            assert_eq!(object, None);
            (message, at.unwrap())
        };
        assert_eq!(
            said(&padded),
            (
                "the file goes on for 4 bytes after its last %%EOF".to_string(),
                end - 1..end + 3
            )
        );
        // On a line with something else.
        let joined = [&file[..end - 7], b" %%EOF\n"].concat();
        assert_eq!(
            said(&joined),
            (
                "the file's last %%EOF isn't on a line of its own".to_string(),
                end - 6..end - 1
            )
        );
        // Further from the end than readers look.
        let far = [&file[..], &[b' '; 1100][..]].concat();
        assert_eq!(
            said(&far).0,
            "the file's last %%EOF is 1106 bytes from its end, past the 1024 readers look in"
        );
        // None at all.
        let none = without(6);
        assert_eq!(
            said(&none),
            ("the file has no %%EOF".to_string(), end - 6..end - 6)
        );
        // A previous revision's end isn't the file's: an update's isn't asked.
        let (file, _) = updated_with_padding();
        assert!(
            read_from(&file, startxref_of_first(&file))
                .warnings
                .iter()
                .all(|w| !w.message.contains("%%EOF"))
        );
    }

    /// A file updated incrementally, its first revision's `%%EOF` followed
    /// by more than its end of line; and the first revision's `startxref`.
    pub(super) fn updated_with_padding() -> (Vec<u8>, usize) {
        let mut file = small();
        let first = startxref(&file);
        file.extend_from_slice(b"\0\0");
        let update = file.len();
        file.extend_from_slice(b"3 0 obj\n<< /A 1 >>\nendobj\n");
        let xref = file.len();
        file.extend_from_slice(format!("xref\n3 1\n{update:010} 00000 n \n").as_bytes());
        file.extend_from_slice(
            format!("trailer\n<< /Size 4 /Root 1 0 R /Prev {first} >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        (file, first)
    }

    fn startxref_of_first(file: &[u8]) -> usize {
        let at = find(file, b"startxref").unwrap() + b"startxref\n".len();
        let digits: String = file[at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .map(|&b| b as char)
            .collect();
        digits.parse().unwrap()
    }

    #[test]
    fn a_names_stray_hash_or_nul_is_said_where_it_is() {
        let file = write::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog /Pages 2 0 R /Ca#zt 1 >>"[..]),
                (
                    2,
                    b"<< /Type /Pages /Kids [] /Count 0 /S /N#00 /T /#4E#41 >>",
                ),
            ],
            "<< /Size 3 /Root 1 0 R >>",
        );
        let doc = read(&file);
        let at = |needle: &[u8]| {
            let start = find(&file, needle).unwrap();
            Some(start..start + needle.len())
        };
        assert_eq!(
            warned(&file),
            [
                (
                    "the name /Ca#zt has a # without two hex digits after it, which readers read differently".to_string(),
                    Some((1, 0)),
                    at(b"/Ca#zt")
                ),
                (
                    "the name /N#00 has #00, NUL, which a name can't hold, which readers read differently".to_string(),
                    Some((2, 0)),
                    at(b"/N#00")
                ),
            ]
        );
        // They read as before: the stray # as itself, #00 as NUL; and a
        // name's well-formed escapes are no warning.
        assert!(doc.get((1, 0)).unwrap().as_dict().unwrap().has(b"Ca#zt"));
        let pages = doc.get((2, 0)).unwrap().as_dict().unwrap();
        assert_eq!(pages.get(b"S").and_then(Object::as_name), Some(&b"N\0"[..]));
        assert_eq!(pages.get(b"T").and_then(Object::as_name), Some(&b"NA"[..]));
    }

    #[test]
    fn a_file_whose_sections_dont_read_is_scanned() {
        let mut file = write::file("1.7", &clean(), root(), Xref::Table);
        let at = rfind(&file, b"startxref").unwrap();
        file.truncate(at);
        file.extend_from_slice(b"startxref\n999999\n%%EOF\n");
        let doc = read(&file);
        assert!(
            doc.trailer
                .unwritten
                .as_deref()
                .is_some_and(|why| why.contains("999999"))
        );
        assert!(doc.trailer.dict().has(b"Root"));
        for n in 1..=5 {
            placed(&file, doc.get((n, 0)).unwrap());
        }

        // A cross-reference stream's file, scanned, has it for its trailer.
        let mut file = write::file("1.7", &clean(), root(), Xref::Stream);
        let at = rfind(&file, b"startxref").unwrap();
        file.truncate(at);
        let doc = read(&file);
        assert_eq!(doc.trailer.unwritten.as_deref(), Some("no startxref"));
        assert_eq!(doc.trailer.stream, Some((6, 0)));
    }

    #[test]
    fn a_scanned_files_last_trailer_takes_what_it_lacks_from_an_earlier_one() {
        // As a linearized file's: the first page's trailer has /Root and
        // /ID, the last only /Size; and startxref misses.
        let mut file = write::file("1.7", &clean(), root(), Xref::Table);
        let at = rfind(&file, b"startxref").unwrap();
        file.truncate(at);
        file.extend_from_slice(b"trailer\n<< /Size 6 /Root 9 0 R >>\ntrailer\n<< /Size 6 >>\nstartxref\n999999\n%%EOF\n");
        let doc = read(&file);
        let root = doc
            .trailer
            .dict()
            .get(b"Root")
            .and_then(Object::as_reference);
        // The newest earlier trailer's /Root, said.
        assert_eq!(root, Some((9, 0)));
        let message = "the last trailer lacks /Root, which it's given from an earlier one";
        assert!(
            doc.warnings.iter().any(|w| w.message == message),
            "{:?}",
            doc.warnings
        );
        // Given what it lacks, it's still placed where the last trailer
        // was written, and so is the warning.
        let at = doc.trailer.object.at.expect("placed");
        assert_eq!(&file[at.start..at.end], b"<< /Size 6 >>");
        let warning = doc.warnings.iter().find(|w| w.message == message);
        assert_eq!(warning.map(|w| w.at), Some(Some(at)));
    }

    #[test]
    fn a_key_written_twice_is_kept() {
        let file = write::assembled(
            "1.7",
            &[(1, b"<< /Type /Catalog /Type /Pages >>")],
            "<< /Size 2 /Root 1 0 R >>",
        );
        let doc = read(&file);
        let catalog = doc.get((1, 0)).unwrap().as_dict().unwrap();
        assert_eq!(catalog.entries().len(), 2);
        assert_eq!(
            catalog.get(b"Type").and_then(Object::as_name),
            Some(&b"Pages"[..])
        );
        let first = catalog.duplicates().next().unwrap();
        assert_eq!(first.value.as_name(), Some(&b"Catalog"[..]));
        let key_at = first.key_at.unwrap();
        assert_eq!(&file[key_at.start..key_at.end], b"/Type");
    }

    #[test]
    fn an_encrypted_files_strings_are_ciphertext_but_its_encryption_dictionarys() {
        let encrypted = |v: &str| {
            let encrypt = format!("<< /Filter /Standard /V {v} /O (owner) >>");
            let file = write::assembled(
                "1.7",
                &[
                    (1, b"<< /Type /Catalog /Lang (en) >>"),
                    (2, encrypt.as_bytes()),
                ],
                "<< /Size 3 /Root 1 0 R /Encrypt 2 0 R /ID [<01> <02>] >>",
            );
            read(&file)
        };
        let string = |doc: &Document, n: u32, key: &[u8]| match doc
            .get((n, 0))
            .unwrap()
            .as_dict()
            .unwrap()
            .get(key)
            .unwrap()
            .value()
        {
            Value::String(s, _) => s.clone(),
            _ => panic!("not a string"),
        };
        let doc = encrypted("2");
        assert_eq!(string(&doc, 1, b"Lang"), Str::Encrypted(b"en".to_vec()));
        assert_eq!(string(&doc, 2, b"O"), Str::Bytes(b"owner".to_vec()));
        let id = doc.trailer.dict().get(b"ID").unwrap();
        assert!(
            matches!(id.value(), Value::Array(a) if matches!(a[0].value(), Value::String(Str::Bytes(_), _)))
        );
        // A version 4 dictionary without /StrF leaves strings as they are.
        let doc = encrypted("4");
        assert_eq!(string(&doc, 1, b"Lang"), Str::Bytes(b"en".to_vec()));

        // A null /Encrypt, or one that refers to no object, is none.
        for (encrypt, warned) in [("null", false), ("9 0 R", true)] {
            let file = write::assembled(
                "1.7",
                &[(1, &b"<< /Type /Catalog /Lang (en) >>"[..])],
                &format!("<< /Size 2 /Root 1 0 R /Encrypt {encrypt} >>"),
            );
            let doc = read(&file);
            assert_eq!(
                string(&doc, 1, b"Lang"),
                Str::Bytes(b"en".to_vec()),
                "{encrypt}"
            );
            assert_eq!(
                doc.warnings.len(),
                usize::from(warned),
                "{:?}",
                doc.warnings
            );
        }

        // A cross-reference stream's strings are plain, read as the
        // trailer or as an object.
        let objects: Vec<(u32, Body)> = vec![
            (
                1,
                dict! { "Type" => "Catalog", "Lang" => Object::string("en") }.into(),
            ),
            (2, dict! { "Filter" => "Standard", "V" => 2 }.into()),
        ];
        let trailer = dict! {
            "Root" => Object::reference(1),
            "Encrypt" => Object::reference(2),
            "ID" => vec![Object::bytes(b"01".to_vec())],
        };
        let file = write::file("1.7", &objects, trailer, Xref::Stream);
        let doc = read(&file);
        assert_eq!(string(&doc, 1, b"Lang"), Str::Encrypted(b"en".to_vec()));
        let id = |o: &Object| o.as_dict().unwrap().get(b"ID").unwrap().clone();
        let written = Object::from(vec![Object::bytes(b"01".to_vec())]);
        assert_eq!(plain(&id(&doc.trailer.object)), written);
        assert_eq!(plain(&id(doc.get((3, 0)).unwrap())), written);
    }

    #[test]
    fn a_reference_may_be_broken_by_comments_and_lines() {
        let file = write::assembled(
            "1.7",
            &[
                (1, b"<< /Type /Catalog /Pages 2 % a comment\n 0\r\nR >>"),
                (2, b"<< >>"),
            ],
            "<< /Size 3 /Root 1 0 R >>",
        );
        let doc = read(&file);
        let pages = doc
            .get((1, 0))
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Pages")
            .unwrap();
        assert_eq!(*pages.value(), Value::Ref((2, 0)));
        let at = pages.at.unwrap();
        assert_eq!(&file[at.start..at.end], b"2 % a comment\n 0\r\nR");
    }

    #[test]
    fn a_malformed_object_says_why() {
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        let file = write::assembled(
            "1.7",
            &[
                (1, b"<< /Type /Catalog >>"),
                (2, b"<< /Type /Catalog"),
                (3, b"<< 3 /X >>"),
                (4, b"[1 2"),
                (5, b"<< >> stray"),
                (6, deep.as_bytes()),
                (7, b"(open"),
                (8, b"<< /Pg 18446744073284681740 0 R >>"),
            ],
            "<< /Size 9 /Root 1 0 R >>",
        );
        let doc = read(&file);
        let why = |n: u32| {
            doc.get((n, 0))
                .err()
                .map(|why| why.to_string())
                .unwrap_or_default()
        };
        assert!(why(2).contains("isn't a name"), "{}", why(2));
        assert!(why(3).contains("isn't a name"), "{}", why(3));
        assert!(why(4).contains("`endobj`"), "{}", why(4));
        assert!(doc.get((5, 0)).is_ok());
        assert!(
            doc.warnings
                .iter()
                .any(|w| w.message == "5 0 obj has no endobj")
        );
        assert!(why(6).contains("nested deeper"), "{}", why(6));
        assert!(why(7).contains("doesn't end"), "{}", why(7));
        assert!(
            why(8).contains("18446744073284681740 0 R, numbers no object"),
            "{}",
            why(8)
        );

        // An offset at another object.
        let mut file = write::file("1.7", &clean(), root(), Xref::Table);
        let one = find(&file, b"1 0 obj").unwrap();
        let two = find(&file, b"2 0 obj").unwrap();
        let row = format!("{two:010} 00000 n");
        let at = find(&file, row.as_bytes()).unwrap();
        file[at..at + 10].copy_from_slice(format!("{one:010}").as_bytes());
        let doc = read(&file);
        let why = doc.get((2, 0)).unwrap_err().to_string();
        assert!(
            why.contains("holds 1 0 obj, where the cross-references put 2 0 R"),
            "{why}"
        );
    }

    /// Every PDF under `PDF_LINT_CORPUS` read, with how much of each
    /// it read and why not the rest, and a sample of values placed.
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files"]
    fn a_corpus_reads() {
        use std::collections::BTreeMap as Counts;
        use std::path::PathBuf;
        use std::time::Instant;

        let dir = std::env::var_os("PDF_LINT_CORPUS").expect("PDF_LINT_CORPUS names a folder");
        let mut folders = vec![PathBuf::from(dir)];
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
                {
                    files.push(path);
                }
            }
        }
        files.sort();
        let class = |why: &str| {
            for (needle, class) in [
                ("in object stream", "in an object stream"),
                ("is compressed", "a compressed cross-reference stream"),
                ("no `n g obj`", "no object at its offset"),
                (
                    "where the cross-references put",
                    "another object at its offset",
                ),
                ("no startxref", "no startxref"),
                ("isn't a name", "a key that isn't a name"),
                ("where a value should be", "a word where a value should be"),
                ("doesn't end", "something that doesn't end"),
                ("no endstream", "no endstream"),
            ] {
                if why.contains(needle) {
                    return class.to_string();
                }
            }
            let cut: String = why.chars().take(48).collect();
            format!("other: {cut}")
        };
        let (mut ok, mut bad, mut written) = (0usize, Counts::new(), 0usize);
        let mut unwritten = Counts::new();
        let mut warnings: Counts<String, usize> = Counts::new();
        let mut seconds = 0.0;
        for (i, path) in files.iter().enumerate() {
            let data = std::fs::read(path).unwrap();
            let start = Instant::now();
            let doc = match std::panic::catch_unwind(|| read_opened(&data, &[])) {
                Ok(doc) => doc,
                Err(_) => panic!("{} panicked", path.display()),
            };
            seconds += start.elapsed().as_secs_f64();
            match &doc.trailer.unwritten {
                None => written += 1,
                Some(why) => *unwritten.entry(class(why)).or_insert(0) += 1,
            }
            for object in doc.objects.values() {
                match object {
                    Ok(object) => {
                        ok += 1;
                        if i % 50 == 0 {
                            // In the file, or in an object stream's data.
                            let within = match object.at.and_then(|at| at.stream) {
                                Some(stream) => &doc.decoded[&stream][..],
                                None => &data[..],
                            };
                            let encrypted = doc.trailer.dict().has(b"Encrypt");
                            placed_as(within, object, !encrypted);
                        }
                    }
                    Err(why) => *bad.entry(class(&why.to_string())).or_insert(0usize) += 1,
                }
            }
            for w in &doc.warnings {
                let cut: String = w
                    .message
                    .chars()
                    .filter(|c| !c.is_ascii_digit())
                    .take(60)
                    .collect();
                *warnings.entry(cut).or_insert(0) += 1;
            }
        }
        println!(
            "{} files in {seconds:.1} s: {written} with their sections read",
            files.len()
        );
        for (why, n) in &unwritten {
            println!("  scanned, {why}: {n}");
        }
        println!("objects read: {ok}");
        for (why, n) in &bad {
            println!("  not read, {why}: {n}");
        }
        let mut warnings: Vec<_> = warnings.into_iter().collect();
        warnings.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for (message, n) in warnings.iter().take(12) {
            println!("  warning: {message}: {n}");
        }
    }

    /// A cross-reference stream's row, its fields 1, 2 and 1 bytes wide.
    pub(super) fn xref_row(kind: u8, field: usize, index: u8) -> [u8; 4] {
        let [high, low] = (field as u16).to_be_bytes();
        [kind, high, low, index]
    }

    /// Objects 2 and 3, the page tree and its page, as an object stream
    /// holds them, and where the first begins (`/First`).
    pub(super) fn objects_2_and_3() -> (Vec<u8>, usize) {
        let pages: &[u8] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Mine (plain) >>";
        let page: &[u8] = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>";
        let header = format!("2 0 3 {} ", pages.len() + 1);
        let first = header.len();
        let mut objects = header.into_bytes();
        objects.extend_from_slice(pages);
        objects.push(b' ');
        objects.extend_from_slice(page);
        (objects, first)
    }

    /// A file whose cross-reference stream and object stream are both
    /// compressed: objects 2 and 3 are in object stream 4, and the
    /// cross-reference stream is 5. With `encrypted`, the trailer says the
    /// file is, and the object stream's data is junk a decoder replaces,
    /// as given.
    pub(super) fn compressed(encrypted: bool) -> (Vec<u8>, BTreeMap<ObjectId, Vec<u8>>) {
        let (file, mut decoded) = streams(encrypted, true, b"", None);
        decoded.remove(&(5, 0));
        if !encrypted {
            decoded.clear();
        }
        (file, decoded)
    }

    /// [`compressed`]'s file with both streams' data junk, which only a
    /// decoder asked for every stream ([`read_through`]) decodes, as given.
    pub(super) fn junk(encrypted: bool) -> (Vec<u8>, BTreeMap<ObjectId, Vec<u8>>) {
        streams(encrypted, false, b"", None)
    }

    /// The file [`compressed`] and [`junk`] write, its streams' data zlib
    /// data when `real` (but an encrypted file's object stream), else
    /// junk, the cross-reference stream's rows followed by `past`, and
    /// with `png`, PNG-predicted (by none) in rows of that many columns,
    /// the last filled out with zeros; and what each decodes to.
    pub(super) fn streams(
        encrypted: bool,
        real: bool,
        past: &[u8],
        png: Option<usize>,
    ) -> (Vec<u8>, BTreeMap<ObjectId, Vec<u8>>) {
        let (objects, first) = objects_2_and_3();
        let mut file = b"%PDF-1.7\n".to_vec();
        let one = file.len();
        file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Lang (en) >>\nendobj\n");
        let four = file.len();
        let data = match real && !encrypted {
            true => write::zlib(&objects),
            false => b"OBJS".to_vec(),
        };
        file.extend_from_slice(
            format!("4 0 obj\n<< /Type /ObjStm /N 2 /First {first} /Filter /FlateDecode /Length {} >>\nstream\n", data.len())
                .as_bytes(),
        );
        file.extend_from_slice(&data);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let five = file.len();
        let encrypt = if encrypted {
            " /Encrypt << /Filter /Standard /V 2 >>"
        } else {
            ""
        };
        let rows = [
            xref_row(0, 0, 255),
            xref_row(1, one, 0),
            xref_row(2, 4, 0),
            xref_row(2, 4, 1),
            xref_row(1, four, 0),
            xref_row(1, five, 0),
        ]
        .concat();
        let mut written = [&rows[..], past].concat();
        let parms = match png {
            Some(columns) => {
                written.resize(written.len().div_ceil(columns) * columns, 0);
                written = (written.chunks(columns))
                    .flat_map(|row| [&[0][..], row].concat())
                    .collect();
                format!(" /DecodeParms << /Predictor 12 /Columns {columns} >>")
            }
            None => String::new(),
        };
        let data = match real {
            true => write::zlib(&written),
            false => b"JUNK".to_vec(),
        };
        file.extend_from_slice(
            format!("5 0 obj\n<< /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R{encrypt} /Filter /FlateDecode{parms} /Length {} >>\nstream\n", data.len())
                .as_bytes(),
        );
        file.extend_from_slice(&data);
        file.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes(),
        );
        let decoded = BTreeMap::from([((5, 0), rows), ((4, 0), objects)]);
        (file, decoded)
    }

    /// A file all in plain view but for `encrypted`, which says in the
    /// trailer that the file is: [`compressed`]'s objects, with neither
    /// stream filtered.
    pub(super) fn unfiltered(encrypted: bool) -> Vec<u8> {
        let (objects, _) = objects_2_and_3();
        filtered(encrypted, "", &objects)
    }

    /// [`unfiltered`]'s file, its object stream's dictionary given
    /// `filter`'s entries, and its data `data`.
    pub(super) fn filtered(encrypted: bool, filter: &str, data: &[u8]) -> Vec<u8> {
        let (_, first) = objects_2_and_3();
        let mut file = b"%PDF-1.7\n".to_vec();
        let one = file.len();
        file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Lang (en) >>\nendobj\n");
        let four = file.len();
        let length = data.len();
        file.extend_from_slice(
            format!("4 0 obj\n<< /Type /ObjStm /N 2 /First {first}{filter} /Length {length} >>\nstream\n")
                .as_bytes(),
        );
        file.extend_from_slice(data);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let five = file.len();
        let rows = [
            xref_row(0, 0, 255),
            xref_row(1, one, 0),
            xref_row(2, 4, 0),
            xref_row(2, 4, 1),
            xref_row(1, four, 0),
            xref_row(1, five, 0),
        ]
        .concat();
        let encrypt = if encrypted {
            " /Encrypt << /Filter /Standard /V 2 >>"
        } else {
            ""
        };
        file.extend_from_slice(
            format!("5 0 obj\n<< /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R{encrypt} /Length {} >>\nstream\n", rows.len())
                .as_bytes(),
        );
        file.extend_from_slice(&rows);
        file.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes(),
        );
        file
    }

    #[test]
    fn an_unfiltered_object_stream_is_read_in_plain_view_unless_the_file_encrypts() {
        // No decoder: what it holds is read as written, and placed in it.
        let file = unfiltered(false);
        let doc = read(&file);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.trailer.unwritten, None);
        let pages = doc.get((2, 0)).unwrap();
        let at = pages.at.unwrap();
        assert_eq!(at.stream, Some((4, 0)));
        assert_eq!(
            &doc.decoded[&(4, 0)][at.start..at.end],
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Mine (plain) >>"
        );
        assert!(doc.get((3, 0)).is_ok());
        // A decoder isn't asked for it.
        struct Unasked;
        impl Decode for Unasked {
            fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
                panic!("asked to decode {ids:?}")
            }
        }
        assert!(read_with(&file, &Unasked).get((2, 0)).is_ok());
        // In a file that encrypts, its data is ciphertext, which only a
        // decoder decodes.
        let why = read(&unfiltered(true)).get((2, 0)).unwrap_err().to_string();
        assert!(why.contains("nothing decodes that"), "{why}");
    }

    #[test]
    fn an_object_stream_decodes_through_each_filter_without_a_decoder() {
        let (objects, _) = objects_2_and_3();
        let hex = |data: &[u8]| {
            let digits: String = data.iter().map(|b| format!("{b:02x} ")).collect();
            format!("{digits}>").into_bytes()
        };
        let cases = [
            (" /Filter /FlateDecode", write::zlib(&objects)),
            (" /Filter /Fl", write::zlib(&objects)),
            (" /Filter /LZWDecode", write::lzw(&objects)),
            (
                " /Filter /LZW /DecodeParms << /EarlyChange 1 >>",
                write::lzw(&objects),
            ),
            (" /Filter /ASCIIHexDecode", hex(&objects)),
            (" /Filter /AHx", hex(&objects)),
            (" /Filter /ASCII85Decode", write::ascii85(&objects)),
            (" /Filter /A85", write::ascii85(&objects)),
            (" /Filter /RunLengthDecode", write::run_length(&objects)),
            (" /Filter /RL", write::run_length(&objects)),
            (
                " /Filter [/AHx /Fl] /DecodeParms [null null]",
                hex(&write::zlib(&objects)),
            ),
            (
                " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Identity >> null]",
                write::zlib(&objects),
            ),
            (" /Filter []", objects.clone()),
            (" /Filter null", objects.clone()),
        ];
        struct Unasked;
        impl Decode for Unasked {
            fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
                panic!("asked to decode {ids:?}")
            }
        }
        for (filter, data) in cases {
            let doc = read_with(&filtered(false, filter, &data), &Unasked);
            assert!(doc.warnings.is_empty(), "{filter}: {:?}", doc.warnings);
            assert_eq!(doc.decoded[&(4, 0)], objects, "{filter}");
            assert!(
                doc.get((2, 0)).is_ok() && doc.get((3, 0)).is_ok(),
                "{filter}"
            );
        }
    }

    #[test]
    fn a_streams_faults_are_warnings_on_it_at_their_bytes() {
        let (objects, _) = objects_2_and_3();
        let hex: Vec<u8> = (objects.iter())
            .flat_map(|b| format!("{b:02x}").into_bytes())
            .collect();
        let warned = |filter: &str, data: &[u8]| {
            let file = filtered(false, filter, data);
            let doc = read(&file);
            assert!(
                doc.get((2, 0)).is_ok() && doc.get((3, 0)).is_ok(),
                "{filter}: what decoded reads"
            );
            let [warning] = &doc.warnings[..] else {
                panic!("{filter}: {:?}", doc.warnings);
            };
            assert_eq!(warning.object, Some((4, 0)), "{filter}");
            let at = warning.at.unwrap();
            (warning.message.clone(), file[at.start..at.end].to_vec())
        };
        // A fault with a place in the data as written: its byte.
        let (message, at) = warned(" /Filter /AHx", &[&hex[..], b"zz>"].concat());
        let n = hex.len();
        let expected = format!(
            "its data doesn't decode cleanly through /ASCIIHexDecode: byte 0x7a at {n} isn't one it takes"
        );
        assert_eq!((message, at), (expected, b"z".to_vec()));
        // One without: the data.
        let (message, at) = warned(" /Filter /AHx", &hex);
        let expected =
            "its data doesn't decode cleanly through /ASCIIHexDecode: no end-of-data marker";
        assert_eq!((message.as_str(), at), (expected, hex.clone()));
        // One in a filter after the first, whose input isn't in the file:
        // the data. The checksum's last byte gone, everything decoded.
        let zlib = write::zlib(&objects);
        let cut: Vec<u8> = (zlib[..zlib.len() - 1].iter())
            .flat_map(|b| format!("{b:02x}").into_bytes())
            .collect();
        let data = [&cut[..], b">"].concat();
        let (message, at) = warned(" /Filter [/AHx /Fl]", &data);
        let expected = "its data doesn't decode cleanly through /FlateDecode: the data ends early";
        assert_eq!((message.as_str(), at), (expected, data.clone()));
        // A checksum that doesn't match.
        let mut zlib = zlib;
        *zlib.last_mut().unwrap() ^= 1;
        let (message, _) = warned(" /Filter /FlateDecode", &zlib);
        assert_eq!(
            message,
            "its data doesn't decode cleanly through /FlateDecode: the Adler-32 checksum doesn't match"
        );
        // A cross-reference stream whose data runs past its rows, as data
        // that's not filtered may: no fault, its rows PNG-predicted or not.
        for png in [None, Some(4)] {
            let (file, _) = streams(false, true, &[0; 40], png);
            let doc = read(&file);
            assert!(doc.warnings.is_empty(), "{png:?}: {:?}", doc.warnings);
            assert!(doc.get((3, 0)).is_ok());
        }
    }

    #[test]
    fn a_cross_reference_streams_predictor_rows_needn_t_divide_its_rows() {
        // Six rows of four bytes in predictor rows of seven, the last
        // filled out: read whole, with or without data past them.
        for past in [&[][..], &[0; 40]] {
            let (file, _) = streams(false, true, past, Some(7));
            let doc = read(&file);
            assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
            assert_eq!(doc.trailer.unwritten, None);
            assert!(doc.get((3, 0)).is_ok());
        }
    }

    #[test]
    fn objects_past_where_a_fault_cut_an_object_stream_off_dont_read() {
        // Its stored block cut short, inside object 3: object 2 reads.
        let (objects, _) = objects_2_and_3();
        let zlib = write::zlib(&objects);
        let cut = &zlib[..zlib.len() - 20];
        let doc = read(&filtered(false, " /Filter /FlateDecode", cut));
        assert!(doc.get((2, 0)).is_ok());
        assert!(doc.get((3, 0)).is_err());
        let truncated = (doc.warnings.iter()).any(|w| w.message.ends_with("the data ends early"));
        assert!(truncated, "{:?}", doc.warnings);
    }

    #[test]
    fn a_number_where_a_fault_cut_an_object_stream_off_doesnt_read() {
        // Object 3 is 12345, cut off at 123.
        let objects = b"2 0 3 6 << >>\n12345";
        let zlib = write::zlib(objects);
        let cut = &zlib[..zlib.len() - 6];
        let stream = format!(
            "<< /Type /ObjStm /N 2 /First 8 /Filter /FlateDecode /Length {} >>",
            cut.len()
        );
        let mut file = b"%PDF-1.7\n".to_vec();
        let one = file.len();
        file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let four = file.len();
        file.extend_from_slice(format!("4 0 obj\n{stream}\nstream\n").as_bytes());
        file.extend_from_slice(cut);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let five = file.len();
        let rows = [
            xref_row(0, 0, 255),
            xref_row(1, one, 0),
            xref_row(2, 4, 0),
            xref_row(2, 4, 1),
            xref_row(1, four, 0),
            xref_row(1, five, 0),
        ]
        .concat();
        file.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
                rows.len()
            )
            .as_bytes(),
        );
        file.extend_from_slice(&rows);
        file.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes(),
        );
        let doc = read(&file);
        assert_eq!(doc.decoded[&(4, 0)], b"2 0 3 6 << >>\n123");
        let why = doc.get((3, 0)).unwrap_err().to_string();
        assert!(why.contains("so it may be cut short"), "{why}");
        assert!(doc.get((2, 0)).is_ok());
    }

    #[test]
    fn a_stream_decodes_to_its_limit_and_a_file_to_its_budget() {
        let (file, _) = compressed(false);
        let (objects, _) = objects_2_and_3();
        // The budget is the file's: the cross-reference stream's 24 bytes
        // of rows first, then 40 for the object stream.
        for (stream, budget) in [(40, BUDGET), (STREAM, 24 + 40)] {
            let doc = read_limited(&file, stream, budget);
            assert_eq!(doc.decoded[&(4, 0)], objects[..40], "{stream} {budget}");
            let over =
                (doc.warnings.iter()).any(|w| w.message.ends_with("its output is over the limit"));
            assert!(over, "{:?}", doc.warnings);
            assert!(doc.get((3, 0)).is_err());
        }
    }

    #[test]
    fn objects_may_be_numbered_sparsely_but_not_listed_without_end() {
        // Two more objects, numbered 900000 and 900001, in a file of a few
        // hundred bytes (in use at byte 9, where neither is): listed.
        let (file, _) = streams(false, true, &[1, 0, 9, 0, 1, 0, 9, 0], None);
        let from = b"/Size 6 /W";
        let to = b"/Size 6 /Index [0 6 900000 2] /W";
        let at = find(&file, from).unwrap();
        let file = [&file[..at], &to[..], &file[at + from.len()..]].concat();
        let doc = read(&file);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert!(doc.get((3, 0)).is_ok());
        assert!(doc.ids().contains(&(900_000, 0)) && doc.ids().contains(&(900_001, 0)));
        // Reading no more than six entries, the two aren't, and it says so.
        let doc = read_listing(&file, 6);
        let message = format!(
            "its rows list 2 entries past the first 6, as many as the reader reads of a file of {} bytes, which aren't read",
            file.len()
        );
        assert!(
            doc.warnings.iter().any(|w| w.message == message),
            "{:?}",
            doc.warnings
        );
        assert!(doc.get((3, 0)).is_ok());
        assert!(!doc.ids().contains(&(900_000, 0)));
    }

    #[test]
    fn the_revision_before_an_update_decodes_its_streams_itself() {
        let (mut file, _) = compressed(false);
        let five = find(&file, b"5 0 obj").unwrap();
        let one = file.len();
        file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Lang (fr) >>\nendobj\n");
        let xref = file.len();
        file.extend_from_slice(
            format!("xref\n1 1\n{one:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {five} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        struct Unasked;
        impl Decode for Unasked {
            fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
                panic!("asked to decode {ids:?}")
            }
        }
        let doc = read_with(&file, &Unasked);
        let update = doc.update.clone().expect("an update");
        assert_eq!(update.previous, five);
        let before = read_from(&file, update.previous);
        assert!(before.warnings.is_empty(), "{:?}", before.warnings);
        assert!(before.get((2, 0)).is_ok() && before.get((3, 0)).is_ok());
    }

    /// A catalog with a language, a page with a note, a signature, and
    /// `extra` strings of the note's object as written, to be encrypted.
    pub(super) fn to_encrypt(extra: &[&[u8]]) -> Vec<(u32, Body)> {
        let mut note = dict! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![Object::from(0), Object::from(0), Object::from(9), Object::from(9)],
            "Contents" => Object::string("A note"),
        };
        if !extra.is_empty() {
            let raw: Vec<Object> = (extra.iter())
                .map(|b| Object::new(Value::String(Str::Encrypted(b.to_vec()), None)))
                .collect();
            note.set("Raw", raw);
        }
        let signature = dict! {
            "Type" => "Sig",
            "Filter" => "Adobe.PPKLite",
            "ByteRange" => vec![Object::from(0), Object::from(1), Object::from(2), Object::from(3)],
            "Contents" => Object::new(Value::String(Str::Bytes(vec![0x30, 0x82, 0x00, 0x01]), None)),
        };
        vec![
            (1, dict! { "Type" => "Catalog", "Pages" => Object::reference(2), "Lang" => Object::string("en") }.into()),
            (2, dict! { "Type" => "Pages", "Kids" => vec![Object::reference(3)], "Count" => 1 }.into()),
            (3, dict! { "Type" => "Page", "Parent" => Object::reference(2), "Annots" => vec![Object::reference(4)] }.into()),
            (4, note.into()),
            (5, signature.into()),
        ]
    }

    pub(super) fn settings(
        kind: pdf_crypt::writer::Kind,
        user: &'static [u8],
    ) -> pdf_crypt::writer::Settings<'static> {
        pdf_crypt::writer::Settings {
            kind,
            user,
            owner: b"owner",
            p: -4,
            id: b"0123456789abcdef",
            encrypt_metadata: true,
        }
    }

    pub(super) const KINDS: [pdf_crypt::writer::Kind; 5] = [
        pdf_crypt::writer::Kind::Rc4By40,
        pdf_crypt::writer::Kind::Rc4By128,
        pdf_crypt::writer::Kind::Aes128,
        pdf_crypt::writer::Kind::Aes256R5,
        pdf_crypt::writer::Kind::Aes256,
    ];

    #[test]
    fn each_kind_of_encryption_decrypts_its_strings_where_they_are() {
        let root = || dict! { "Root" => Object::reference(1) };
        for kind in KINDS {
            let file = write::encrypted(&settings(kind, b"user"), &to_encrypt(&[]), root());
            for (password, owner) in [(&b"user"[..], false), (b"owner", true)] {
                let doc = read_opened(&file, &[password]);
                assert_eq!(doc.encryption, Encryption::Opened { owner }, "{kind:?}");
                assert!(doc.warnings.is_empty(), "{kind:?}: {:?}", doc.warnings);
                let get = |n: u32, key: &[u8]| {
                    doc.get((n, 0))
                        .unwrap()
                        .as_dict()
                        .unwrap()
                        .get(key)
                        .unwrap()
                        .clone()
                };
                let lang = get(1, b"Lang");
                assert_eq!(
                    *lang.value(),
                    Value::String(Str::Bytes(b"en".to_vec()), Some(Form::Literal)),
                    "{kind:?}"
                );
                // Where the string is written, as ciphertext.
                let at = lang.at.unwrap();
                assert!(
                    matches!(file[at.start], b'(' | b'<')
                        && !file[at.start..at.end].starts_with(b"(en)"),
                    "{kind:?}"
                );
                assert_eq!(
                    *get(4, b"Contents").value(),
                    Value::String(Str::Bytes(b"A note".to_vec()), Some(Form::Literal)),
                    "{kind:?}"
                );
                // A signature's /Contents isn't encrypted, so not decrypted.
                assert_eq!(
                    *get(5, b"Contents").value(),
                    Value::String(
                        Str::Bytes(vec![0x30, 0x82, 0x00, 0x01]),
                        Some(Form::Literal)
                    ),
                    "{kind:?}"
                );
            }
        }
    }

    /// A decoder that mustn't be asked anything.
    struct Unaskable;

    impl Decode for Unaskable {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            panic!("asked to decode {ids:?}")
        }

        fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
            panic!("asked for the plaintext of {ids:?}")
        }
    }

    #[test]
    fn a_decoder_isnt_asked_of_a_file_the_reader_opens_but_when_its_made_to() {
        let root = dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes256;
        let file = write::encrypted(&settings(kind, b""), &to_encrypt(&[]), root);
        let doc = read_with(&file, &Unaskable);
        assert_eq!(doc.encryption, Encryption::Opened { owner: false });
        let decoder = Plain(BTreeMap::new(), std::cell::Cell::new(0));
        read_through(&file, &decoder);
        assert_eq!(decoder.1.get(), 1, "asked for the plaintext");
    }

    #[test]
    fn a_password_that_doesnt_open_it_leaves_it_locked_and_its_strings_ciphertext() {
        let root = dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes128;
        let file = write::encrypted(&settings(kind, b"user"), &to_encrypt(&[]), root);
        for passwords in [&[][..], &[&b"wrong"[..]]] {
            let doc = read_opened(&file, passwords);
            assert_eq!(doc.encryption, Encryption::Locked);
            let lang = doc
                .get((1, 0))
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"Lang")
                .unwrap()
                .clone();
            assert!(matches!(lang.value(), Value::String(Str::Encrypted(_), _)));
        }
    }

    #[test]
    fn a_handler_the_reader_cant_open_leaves_its_strings_ciphertext_and_says_why() {
        let root = dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes128;
        let file = write::encrypted(&settings(kind, b""), &to_encrypt(&[]), root);
        // Another handler's name, of the same length, so nothing moves.
        let at = find(&file, b"/Standard").unwrap();
        let file = [&file[..at], b"/Standarx", &file[at + 9..]].concat();
        let doc = read_opened(&file, &[]);
        let why = "its security handler is /Standarx, not /Standard";
        assert_eq!(doc.encryption, Encryption::Unsupported(why.to_string()));
        let message = format!(
            "its security handler doesn't open: {why}; its strings stay ciphertext, and its object streams aren't read"
        );
        assert!(
            doc.warnings.iter().any(|w| w.message == message),
            "{:?}",
            doc.warnings
        );
        let lang = doc
            .get((1, 0))
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Lang")
            .unwrap()
            .clone();
        assert!(matches!(lang.value(), Value::String(Str::Encrypted(_), _)));
    }

    #[test]
    fn a_file_whose_strings_and_streams_arent_encrypted_reads_whole_without_its_password() {
        // Only embedded files are encrypted (/StmF and /StrF /Identity):
        // its object stream and strings are plain, its password no matter.
        let kind = pdf_crypt::writer::Kind::Aes128;
        let values = pdf_crypt::writer::dictionary(&settings(kind, b"user"));
        let hex = |b: &Option<Vec<u8>>| -> String {
            b.as_ref()
                .unwrap()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let encrypt = format!(
            "/Root 1 0 R /Encrypt << /Filter /Standard /V 4 /R 4 /Length 128 /P -4 /O <{}> /U <{}> \
             /CF << /StdCF << /CFM /AESV2 /AuthEvent /EFOpen >> >> /StmF /Identity /StrF /Identity /EFF /StdCF >> \
             /ID [(0123456789abcdef) (0123456789abcdef)]",
            hex(&values.o),
            hex(&values.u)
        );
        let (objects, _) = objects_2_and_3();
        let file = filtered(false, "", &objects);
        let at = find(&file, b"/Root 1 0 R").unwrap();
        let file = [&file[..at], encrypt.as_bytes(), &file[at + 11..]].concat();
        for passwords in [&[][..], &[&b"wrong"[..]]] {
            let doc = read_opened(&file, passwords);
            assert_eq!(doc.encryption, Encryption::Plain);
            assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
            assert_eq!(doc.get((2, 0)).unwrap().at.unwrap().stream, Some((4, 0)));
            let lang = doc.get((1, 0)).unwrap().as_dict().unwrap().get(b"Lang");
            assert_eq!(
                *lang.unwrap().value(),
                Value::String(Str::Bytes(b"en".to_vec()), Some(Form::Literal))
            );
        }
        // Its password opens it all the same.
        let doc = read_opened(&file, &[b"user"]);
        assert_eq!(doc.encryption, Encryption::Opened { owner: false });
        // But an object stream whose own crypt filter encrypts it doesn't
        // read without it.
        let file = filtered(
            false,
            " /Filter /Crypt /DecodeParms << /Name /StdCF >>",
            &objects,
        );
        let at = find(&file, b"/Root 1 0 R").unwrap();
        let file = [&file[..at], encrypt.as_bytes(), &file[at + 11..]].concat();
        let doc = read_opened(&file, &[]);
        assert_eq!(doc.encryption, Encryption::Plain);
        let why = doc.get((2, 0)).unwrap_err().to_string();
        assert!(
            why.contains("its own /Crypt filter, /StdCF, encrypts it"),
            "{why}"
        );
        // A handler the reader can't open, whose crypt filters encrypt
        // neither strings nor streams: the same.
        let other = encrypt.replace("/Filter /Standard", "/Filter /Adobe.PubSec");
        let file = filtered(false, "", &objects);
        let at = find(&file, b"/Root 1 0 R").unwrap();
        let file = [&file[..at], other.as_bytes(), &file[at + 11..]].concat();
        let doc = read_opened(&file, &[]);
        assert_eq!(doc.encryption, Encryption::Plain);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert!(doc.get((2, 0)).is_ok());
    }

    /// `file`, written by [`write::encrypted`] with its encryption
    /// dictionary 6 0 R, and an update that writes the dictionary again
    /// with `from` in it made `to`, and `extra` objects.
    pub(super) fn encryption_updated(
        file: &[u8],
        from: &str,
        to: &str,
        extra: &[(u32, &str)],
    ) -> Vec<u8> {
        let start = find(file, b"6 0 obj").unwrap();
        let end = start + find(&file[start..], b"endobj").unwrap();
        let own = String::from_utf8_lossy(&file[start + 7..end]).into_owned();
        assert!(own.contains(from), "{own}");
        let own = own.replacen(from, to, 1);
        let prev = startxref(file);
        let mut file = file.to_vec();
        let mut rows = String::from("xref\n");
        let objects = [&[(6, own.as_str())][..], extra].concat();
        for (n, body) in objects {
            rows.push_str(&format!("{n} 1\n{:010} 00000 n \n", file.len()));
            file.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let xref = file.len();
        file.extend_from_slice(rows.as_bytes());
        file.extend_from_slice(
            format!(
                "trailer\n<< /Size 10 /Root 1 0 R /Encrypt 6 0 R /ID [(0123456789abcdef) (0123456789abcdef)] /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        file
    }

    #[test]
    fn an_encryption_dictionarys_values_by_reference_decrypt_as_written_in_it() {
        let root = || dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes128;
        let file = write::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
        let filter = "<< /CFM /AESV2 /AuthEvent /DocOpen >>";
        let cases = [
            (
                "/StdCF << /CFM /AESV2 /AuthEvent /DocOpen >>",
                "/StdCF 8 0 R",
                vec![(8, filter)],
            ),
            ("/StrF /StdCF", "/StrF 9 0 R", vec![(9, "/StdCF")]),
        ];
        for (from, to, extra) in cases {
            let file = encryption_updated(&file, from, to, &extra);
            let doc = read_opened(&file, &[]);
            assert_eq!(doc.encryption, Encryption::Opened { owner: false }, "{to}");
            assert!(doc.warnings.is_empty(), "{to}: {:?}", doc.warnings);
            let lang = doc.get((1, 0)).unwrap().as_dict().unwrap().get(b"Lang");
            assert_eq!(
                *lang.unwrap().value(),
                Value::String(Str::Bytes(b"en".to_vec()), Some(Form::Literal)),
                "{to}"
            );
            // What its values refer to is its own, not encrypted.
            let (n, _) = extra[0];
            assert!(doc.get((n, 0)).is_ok(), "{to}");
        }
    }

    #[test]
    fn what_an_encryption_dictionarys_values_refer_to_is_read_as_written() {
        let root = || dict! { "Root" => Object::reference(1) };
        for kind in KINDS {
            let file = write::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
            // Its /O, a string, written again by reference to 8 0 R, which
            // holds it as written in the dictionary.
            let o = pdf_crypt::writer::dictionary(&settings(kind, b""))
                .o
                .unwrap();
            let mut written = Vec::new();
            write::value(&mut written, &Object::bytes(o.clone()));
            let written = String::from_utf8(written).unwrap();
            let from = format!("/O {written}");
            let file = encryption_updated(&file, &from, "/O 8 0 R", &[(8, &written)]);
            let doc = read_opened(&file, &[]);
            assert_eq!(
                doc.encryption,
                Encryption::Opened { owner: false },
                "{kind:?}"
            );
            assert!(doc.warnings.is_empty(), "{kind:?}: {:?}", doc.warnings);
            assert_eq!(
                plain(doc.get((8, 0)).unwrap()),
                Object::bytes(o),
                "{kind:?}"
            );
            let lang = doc.get((1, 0)).unwrap().as_dict().unwrap().get(b"Lang");
            assert_eq!(
                *lang.unwrap().value(),
                Value::String(Str::Bytes(b"en".to_vec()), Some(Form::Literal)),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn metadata_not_encrypted_is_in_plain_view_whether_the_file_opens_or_not() {
        let root = dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes128;
        let settings = pdf_crypt::writer::Settings {
            encrypt_metadata: false,
            ..settings(kind, b"user")
        };
        let xmp = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>";
        let metadata = dict! { "Type" => "Metadata", "Subtype" => "XML" };
        let objects = [
            (1, dict! { "Type" => "Catalog", "Pages" => Object::reference(2), "Metadata" => Object::reference(3) }.into()),
            (2, dict! { "Type" => "Pages", "Kids" => vec![], "Count" => 0 }.into()),
            (3, Body::stream(metadata, xmp)),
        ];
        let file = write::encrypted(&settings, &objects, root);
        for passwords in [&[][..], &[&b"user"[..]]] {
            let doc = read_opened(&file, passwords);
            assert_eq!(
                doc.metadata.as_deref(),
                Some(&xmp[..]),
                "{:?}",
                doc.encryption
            );
        }
    }

    #[test]
    fn metadata_not_encrypted_has_its_dictionarys_strings_decrypted_all_the_same() {
        let root = || dict! { "Root" => Object::reference(1) };
        let xmp = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>";
        let metadata =
            dict! { "Type" => "Metadata", "Subtype" => "XML", "Mine" => Object::string("about") };
        let objects = [
            (1, dict! { "Type" => "Catalog", "Pages" => Object::reference(2), "Metadata" => Object::reference(3) }.into()),
            (2, dict! { "Type" => "Pages", "Kids" => vec![], "Count" => 0 }.into()),
            (3, Body::stream(metadata, xmp)),
        ];
        // The kinds of /V 4 and 5, from which /EncryptMetadata counts.
        let kinds = [
            pdf_crypt::writer::Kind::Aes128,
            pdf_crypt::writer::Kind::Aes256R5,
            pdf_crypt::writer::Kind::Aes256,
        ];
        for kind in kinds {
            let settings = pdf_crypt::writer::Settings {
                encrypt_metadata: false,
                ..settings(kind, b"")
            };
            let file = write::encrypted(&settings, &objects, root());
            let doc = read_opened(&file, &[]);
            assert_eq!(
                doc.encryption,
                Encryption::Opened { owner: false },
                "{kind:?}"
            );
            assert!(doc.warnings.is_empty(), "{kind:?}: {:?}", doc.warnings);
            assert_eq!(doc.metadata.as_deref(), Some(&xmp[..]), "{kind:?}");
            let mine = doc.get((3, 0)).unwrap().as_dict().unwrap().get(b"Mine");
            assert_eq!(
                *mine.unwrap().value(),
                Value::String(Str::Bytes(b"about".to_vec()), Some(Form::Literal)),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn an_encrypted_object_stream_is_decrypted_and_decoded_without_the_decoder() {
        let kind = pdf_crypt::writer::Kind::Aes256;
        let values = pdf_crypt::writer::dictionary(&settings(kind, b"user"));
        let handler = Handler::open(&values, b"0123456789abcdef", &[b"user"]).unwrap();
        let (objects, _) = objects_2_and_3();
        let data = write::zlib(&objects);
        let data =
            pdf_crypt::writer::encrypt_stream(&handler, (4, 0), &data, None, [4; 16]).unwrap();
        let hex = |b: &Option<Vec<u8>>| -> String {
            b.as_ref()
                .unwrap()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let encrypt = format!(
            "/Root 1 0 R /Encrypt << /Filter /Standard /V 5 /R 6 /Length 256 /P -4 /O <{}> /U <{}> /OE <{}> /UE <{}> /Perms <{}> \
             /CF << /StdCF << /CFM /AESV3 >> >> /StmF /StdCF /StrF /Identity >> \
             /ID [(0123456789abcdef) (0123456789abcdef)]",
            hex(&values.o),
            hex(&values.u),
            hex(&values.oe),
            hex(&values.ue),
            hex(&values.perms)
        );
        let file = filtered(false, " /Filter /FlateDecode", &data);
        let at = find(&file, b"/Root 1 0 R").unwrap();
        let file = [&file[..at], encrypt.as_bytes(), &file[at + 11..]].concat();
        let doc = read_as(&file, Some(&Unaskable), true, &Options::DEFAULT, &[b"user"]);
        assert_eq!(doc.encryption, Encryption::Opened { owner: false });
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.decoded[&(4, 0)], objects);
        assert_eq!(doc.get((2, 0)).unwrap().at.unwrap().stream, Some((4, 0)));
    }

    #[test]
    fn an_object_streams_own_crypt_filter_says_how_its_data_decrypts() {
        let kind = pdf_crypt::writer::Kind::Aes256;
        let values = pdf_crypt::writer::dictionary(&settings(kind, b"user"));
        let handler = Handler::open(&values, b"0123456789abcdef", &[b"user"]).unwrap();
        let (objects, _) = objects_2_and_3();
        let plain = write::zlib(&objects);
        let encrypted =
            pdf_crypt::writer::encrypt_stream(&handler, (4, 0), &plain, None, [4; 16]).unwrap();
        let hex = |b: &Option<Vec<u8>>| -> String {
            b.as_ref()
                .unwrap()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        // Its strings and streams both encrypted, by /StdCF.
        let encrypt = format!(
            "/Root 1 0 R /Encrypt << /Filter /Standard /V 5 /R 6 /Length 256 /P -4 /O <{}> /U <{}> /OE <{}> /UE <{}> /Perms <{}> \
             /CF << /StdCF << /CFM /AESV3 >> >> /StmF /StdCF /StrF /StdCF >> \
             /ID [(0123456789abcdef) (0123456789abcdef)]",
            hex(&values.o),
            hex(&values.u),
            hex(&values.oe),
            hex(&values.ue),
            hex(&values.perms)
        );
        let cases = [
            // No crypt filter of its own: as /StmF says.
            (" /Filter /FlateDecode", &encrypted),
            // Its own, by its name.
            (
                " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /StdCF >> null]",
                &encrypted,
            ),
            // Its own, /Identity: not encrypted, though the file's streams
            // are.
            (
                " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Identity >> null]",
                &plain,
            ),
            // Its own, naming none: /Identity.
            (" /Filter [/Crypt /FlateDecode]", &plain),
        ];
        for (filter, data) in cases {
            let mut file = filtered(false, filter, data);
            // The catalog's string, written in plain view, blanked out, so
            // that nothing moves.
            let at = find(&file, b"/Lang (en)").unwrap();
            file[at..at + 10].fill(b' ');
            let at = find(&file, b"/Root 1 0 R").unwrap();
            let file = [&file[..at], encrypt.as_bytes(), &file[at + 11..]].concat();
            let doc = read_opened(&file, &[b"user"]);
            assert_eq!(
                doc.encryption,
                Encryption::Opened { owner: false },
                "{filter}"
            );
            assert!(doc.warnings.is_empty(), "{filter}: {:?}", doc.warnings);
            assert_eq!(doc.decoded[&(4, 0)], objects, "{filter}");
            // Its objects are read from its decrypted data, their strings
            // as they are there, not decrypted again.
            let pages = doc.get((2, 0)).unwrap();
            assert_eq!(pages.at.unwrap().stream, Some((4, 0)), "{filter}");
            let mine = pages.as_dict().unwrap().get(b"Mine");
            assert_eq!(
                *mine.unwrap().value(),
                Value::String(Str::Bytes(b"plain".to_vec()), Some(Form::Literal)),
                "{filter}"
            );
        }
    }

    #[test]
    fn a_trailer_without_an_id_makes_the_key_without_one_and_says_so() {
        let root = dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes128;
        let settings = pdf_crypt::writer::Settings {
            id: b"",
            ..settings(kind, b"")
        };
        let file = write::encrypted(&settings, &to_encrypt(&[]), root);
        // Its /ID, of empty strings, blanked out, so that nothing moves.
        let at = find(&file, b"/ID [() ()]").unwrap();
        let mut file = file;
        file[at..at + 11].fill(b' ');
        let doc = read_opened(&file, &[]);
        assert_eq!(doc.encryption, Encryption::Opened { owner: false });
        let message = "its trailer's /ID has no string first, so its key is made without one";
        let warnings: Vec<_> = doc.warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(warnings, [message]);
        let lang = doc.get((1, 0)).unwrap().as_dict().unwrap().get(b"Lang");
        assert_eq!(
            *lang.unwrap().value(),
            Value::String(Str::Bytes(b"en".to_vec()), Some(Form::Literal))
        );
    }

    #[test]
    fn the_trailers_id_is_read_as_written_in_a_file_that_opens() {
        let id = &b"0123456789abcdef"[..];
        let written = Object::from(vec![Object::bytes(id), Object::bytes(id)]);
        let en = Value::String(Str::Bytes(b"en".to_vec()), Some(Form::Literal));
        let lang = |doc: &Document| {
            let catalog = doc.get((1, 0)).unwrap().as_dict().unwrap().clone();
            catalog.get(b"Lang").unwrap().value().clone()
        };
        for kind in KINDS {
            let root = dict! { "Root" => Object::reference(1) };
            let file = write::encrypted(&settings(kind, b""), &to_encrypt(&[]), root);
            let doc = read_opened(&file, &[]);
            assert_eq!(
                doc.encryption,
                Encryption::Opened { owner: false },
                "{kind:?}"
            );
            assert!(doc.warnings.is_empty(), "{kind:?}: {:?}", doc.warnings);
            assert_eq!(lang(&doc), en, "{kind:?}");
            let trailer_id = doc.trailer.dict().get(b"ID").unwrap();
            assert_eq!(plain(trailer_id), written, "{kind:?}");
            let at = trailer_id.at.unwrap();
            assert_eq!(
                &file[at.start..at.end],
                b"[(0123456789abcdef) (0123456789abcdef)]",
                "{kind:?}"
            );

            // By reference, which it shouldn't be (ISO 32000-2, Table 15):
            // an update whose trailer names 7 0 R, which holds it as
            // written. The key is made from it, and it's read as written,
            // as what the encryption dictionary's values refer to is.
            let prev = startxref(&file);
            let mut file = file;
            let seven = file.len();
            file.extend_from_slice(b"7 0 obj\n[(0123456789abcdef) (0123456789abcdef)]\nendobj\n");
            let xref = file.len();
            file.extend_from_slice(
                format!(
                    "xref\n7 1\n{seven:010} 00000 n \ntrailer\n<< /Size 8 /Root 1 0 R /Encrypt 6 0 R \
                     /ID 7 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n"
                )
                .as_bytes(),
            );
            let doc = read_opened(&file, &[]);
            assert_eq!(
                doc.encryption,
                Encryption::Opened { owner: false },
                "{kind:?}"
            );
            assert!(doc.warnings.is_empty(), "{kind:?}: {:?}", doc.warnings);
            assert_eq!(lang(&doc), en, "{kind:?}");
            assert_eq!(plain(doc.get((7, 0)).unwrap()), written, "{kind:?}");
        }
    }

    #[test]
    fn a_signatures_contents_isnt_decrypted_whatever_its_type() {
        let root = dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes256;
        let der = vec![0x30, 0x82, 0x00, 0x01];
        let signature = |typed: Option<&str>| {
            let mut d = dict! {
                "Filter" => "Adobe.PPKLite",
                "ByteRange" => vec![Object::from(0), Object::from(1), Object::from(2), Object::from(3)],
                "Contents" => Object::new(Value::String(Str::Bytes(der.clone()), None)),
            };
            if let Some(typed) = typed {
                d.set("Type", typed);
            }
            d
        };
        let objects = [
            (
                1,
                dict! { "Type" => "Catalog", "Pages" => Object::reference(2) }.into(),
            ),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![], "Count" => 0 }.into(),
            ),
            (3, signature(None).into()),
            (4, signature(Some("DocTimeStamp")).into()),
            // Another type's /Contents is encrypted, and decrypted.
            (5, signature(Some("Annot")).into()),
        ];
        let file = write::encrypted(&settings(kind, b""), &objects, root);
        let doc = read_opened(&file, &[]);
        for n in [3, 4, 5] {
            let contents = doc.get((n, 0)).unwrap().as_dict().unwrap().get(b"Contents");
            assert_eq!(
                *contents.unwrap().value(),
                Value::String(Str::Bytes(der.clone()), Some(Form::Literal)),
                "{n}"
            );
        }
    }

    #[test]
    fn an_earlier_updates_encryption_dictionary_isnt_decrypted_either() {
        let root = || dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes128;
        let mut file = write::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
        // An update that writes the encryption dictionary, 6 0 R, again as
        // 7 0 R, its trailer naming that: the first trailer still names 6.
        let start = find(&file, b"6 0 obj").unwrap();
        let end = start + find(&file[start..], b"endobj").unwrap();
        let own = file[start + 7..end].to_vec();
        let prev = startxref(&file);
        let seven = file.len();
        file.extend_from_slice(&[&b"7 0 obj"[..], &own, b"endobj\n"].concat());
        let xref = file.len();
        file.extend_from_slice(
            format!(
                "xref\n7 1\n{seven:010} 00000 n \ntrailer\n<< /Size 8 /Root 1 0 R /Encrypt 7 0 R \
                 /ID [(0123456789abcdef) (0123456789abcdef)] /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let doc = read_opened(&file, &[]);
        assert_eq!(doc.encryption, Encryption::Opened { owner: false });
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let written = pdf_crypt::writer::dictionary(&settings(kind, b"")).o;
        for n in [6, 7] {
            let dict = doc.get((n, 0)).unwrap().as_dict().unwrap().clone();
            let o = dict.get(b"O").unwrap().value();
            assert_eq!(
                o,
                &Value::String(Str::Bytes(written.clone().unwrap()), Some(Form::Literal)),
                "{n}"
            );
        }
    }

    #[test]
    fn a_cross_reference_streams_strings_are_read_as_written_in_a_file_that_opens() {
        let id = &b"0123456789abcdef"[..];
        let written = Object::from(vec![Object::bytes(id), Object::bytes(id)]);
        let root = || dict! { "Root" => Object::reference(1) };
        for kind in KINDS {
            let file = write::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
            // Its table and trailer written again as a cross-reference
            // stream, 7 0 R: the table's rows, then its own, each a type,
            // an offset and a generation, of 1, 4 and 2 bytes; and the
            // trailer's entries, its /ID among them, in plain view.
            let xref = startxref(&file);
            let table = String::from_utf8(file[xref..].to_vec()).unwrap();
            let mut rows = Vec::new();
            let mut row = |kind: u8, offset: u32, generation: u16| {
                rows.push(kind);
                rows.extend_from_slice(&offset.to_be_bytes());
                rows.extend_from_slice(&generation.to_be_bytes());
            };
            for line in table.lines().skip(2).take_while(|line| *line != "trailer") {
                let in_use = u8::from(line.ends_with("n "));
                row(
                    in_use,
                    line[..10].parse().unwrap(),
                    line[11..16].parse().unwrap(),
                );
            }
            row(1, u32::try_from(xref).unwrap(), 0);
            let size = rows.len() / 7;
            assert_eq!(size, 8, "{kind:?}");
            let mut file = file[..xref].to_vec();
            file.extend_from_slice(
                format!(
                    "7 0 obj\n<< /Type /XRef /Size {size} /W [1 4 2] /Root 1 0 R /Encrypt 6 0 R \
                     /ID [(0123456789abcdef) (0123456789abcdef)] /Length {} >>\nstream\n",
                    rows.len()
                )
                .as_bytes(),
            );
            file.extend_from_slice(&rows);
            file.extend_from_slice(
                format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
            );
            let doc = read_opened(&file, &[]);
            assert_eq!(
                doc.encryption,
                Encryption::Opened { owner: false },
                "{kind:?}"
            );
            assert!(doc.warnings.is_empty(), "{kind:?}: {:?}", doc.warnings);
            assert_eq!(doc.trailer.stream, Some((7, 0)), "{kind:?}");
            // The file's strings are decrypted, but the cross-reference
            // stream's are read as written, as the trailer and as an
            // object.
            let lang = doc.get((1, 0)).unwrap().as_dict().unwrap().get(b"Lang");
            assert_eq!(
                *lang.unwrap().value(),
                Value::String(Str::Bytes(b"en".to_vec()), Some(Form::Literal)),
                "{kind:?}"
            );
            let id = |o: &Object| o.as_dict().unwrap().get(b"ID").unwrap().clone();
            assert_eq!(plain(&id(&doc.trailer.object)), written, "{kind:?}");
            let object = id(doc.get((7, 0)).unwrap());
            assert_eq!(plain(&object), written, "{kind:?}");
            let at = object.at.unwrap();
            assert_eq!(
                &file[at.start..at.end],
                b"[(0123456789abcdef) (0123456789abcdef)]",
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_streams_dictionary_strings_decrypt_but_its_data_is_left_as_written() {
        let root = || dict! { "Root" => Object::reference(1) };
        let drawn: &[u8] = b"0 0 m 9 9 l S";
        let form = dict! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(9), Object::from(9)],
            "Mine" => Object::string("on the form"),
        };
        let resources = dict! { "XObject" => dict! { "Fm" => Object::reference(4) } };
        let objects = [
            (1, dict! { "Type" => "Catalog", "Pages" => Object::reference(2) }.into()),
            (2, dict! { "Type" => "Pages", "Kids" => vec![Object::reference(3)], "Count" => 1 }.into()),
            (3, dict! { "Type" => "Page", "Parent" => Object::reference(2), "Resources" => resources }.into()),
            (4, Body::stream(form, drawn)),
        ];
        for kind in KINDS {
            let file = write::encrypted(&settings(kind, b""), &objects, root());
            let doc = read_opened(&file, &[]);
            assert_eq!(
                doc.encryption,
                Encryption::Opened { owner: false },
                "{kind:?}"
            );
            assert!(doc.warnings.is_empty(), "{kind:?}: {:?}", doc.warnings);
            let Value::Stream(stream) = doc.get((4, 0)).unwrap().value() else {
                panic!("{kind:?}: not a stream");
            };
            assert_eq!(
                *stream.dict.get(b"Mine").unwrap().value(),
                Value::String(Str::Bytes(b"on the form".to_vec()), Some(Form::Literal)),
                "{kind:?}"
            );
            // Its data isn't read: it's where it was written, encrypted.
            assert!(
                matches!(stream.data, pdf_objects::object::Data::At(_)),
                "{kind:?}"
            );
            let at = stream.data.at().unwrap();
            assert_eq!(at.stream, None, "{kind:?}");
            let data = &file[at.start..at.end];
            assert!(!data.windows(drawn.len()).any(|w| w == drawn), "{kind:?}");
            assert!(doc.decoded.is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn strings_that_dont_decrypt_cleanly_are_said_once_an_object_and_a_fault() {
        let root = dict! { "Root" => Object::reference(1) };
        let kind = pdf_crypt::writer::Kind::Aes128;
        // Two strings too short for AES, as written, of five bytes and
        // seven: the same fault, said once.
        let file = write::encrypted(
            &settings(kind, b""),
            &to_encrypt(&[b"short", b"longer!"]),
            root,
        );
        let doc = read_opened(&file, &[]);
        let messages: Vec<_> = doc
            .warnings
            .iter()
            .map(|w| (w.object, w.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [(
                Some((4, 0)),
                "2 of its strings don't decrypt cleanly: its AES ciphertext has no whole block past its IV"
            )]
        );
        let note = doc.get((4, 0)).unwrap().as_dict().unwrap().clone();
        assert_eq!(
            *note.get(b"Contents").unwrap().value(),
            Value::String(Str::Bytes(b"A note".to_vec()), Some(Form::Literal))
        );
        let raw = note.get(b"Raw").unwrap();
        assert!(
            matches!(raw.value(), Value::Array(a) if matches!(a[0].value(), Value::String(Str::Encrypted(_), _)))
        );
    }

    #[test]
    fn objects_in_an_object_stream_read_from_its_decoded_data() {
        let (file, _) = compressed(false);
        let doc = read(&file);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.trailer.unwritten, None);
        assert_eq!(doc.trailer.stream, Some((5, 0)));
        let pages = doc.get((2, 0)).unwrap();
        let at = pages.at.unwrap();
        assert_eq!(at.stream, Some((4, 0)));
        let written = &doc.decoded[&(4, 0)][at.start..at.end];
        assert_eq!(
            written,
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Mine (plain) >>"
        );
        let kids = pages.as_dict().unwrap().entry(b"Kids").unwrap();
        assert_eq!(kids.key_at.unwrap().stream, Some((4, 0)), "its keys too");
        let page = doc.get((3, 0)).unwrap();
        assert_eq!(
            *page.as_dict().unwrap().get(b"Parent").unwrap().value(),
            Value::Ref((2, 0))
        );
    }

    #[test]
    fn a_stream_that_doesnt_decode_says_why() {
        // A filter the reader doesn't decode, on the object stream, and on
        // the cross-reference stream, whose rows don't read, so the file
        // is scanned, and the compressed objects aren't found.
        let (file, _) = compressed(false);
        let image = |file: &[u8], from: usize| {
            let at = from + find(&file[from..], b"/FlateDecode").unwrap();
            [&file[..at], b"/JBIG2Decode", &file[at + 12..]].concat()
        };
        let doc = read(&image(&file, find(&file, b"4 0 obj").unwrap()));
        let why = doc.get((2, 0)).unwrap_err().to_string();
        assert!(
            why.contains("its object stream 4 0 R doesn't decode: its filter /JBIG2Decode isn't one the reader decodes"),
            "{why}"
        );
        assert!(doc.get((1, 0)).is_ok());
        let doc = read(&image(&file, find(&file, b"5 0 obj").unwrap()));
        let why = doc.trailer.unwritten.as_deref().unwrap();
        assert!(
            why.contains("its cross-reference stream doesn't decode: its filter /JBIG2Decode"),
            "{why}"
        );
        assert!(doc.get((2, 0)).is_err());
        // An encrypted file's object streams, without a decoder.
        let (file, _) = compressed(true);
        let why = read(&file).get((2, 0)).unwrap_err().to_string();
        assert!(why.contains("nothing decodes that"), "{why}");
        // Asked of a decoder: the rows but not the object stream's data.
        let (file, mut decoded) = junk(false);
        decoded.remove(&(4, 0));
        let doc = read_through(&file, &decoded);
        let why = doc.get((2, 0)).unwrap_err().to_string();
        assert!(
            why.contains("its object stream 4 0 R doesn't decode: no data for 4 0 R"),
            "{why}"
        );
        assert!(doc.get((1, 0)).is_ok());
        // A header that names another object.
        let (file, mut decoded) = junk(false);
        let objects = decoded.get_mut(&(4, 0)).unwrap();
        objects[0] = b'7';
        let doc = read_through(&file, &decoded);
        let why = doc.get((2, 0)).unwrap_err().to_string();
        assert!(
            why.contains("object 0 of object stream 4 0 R is 7, not 2"),
            "{why}"
        );
    }

    /// [`compressed`]'s file as a hybrid: a table lists objects 1 and 4,
    /// leaves 2 and 3 free, and gives an uncompressed `/XRefStm` that lists
    /// them in object stream 4, and lists 1 wrongly, which the table's row
    /// overrides.
    #[test]
    fn a_hybrid_files_stream_lists_what_its_table_leaves_free() {
        let (file, _) = compressed(false);
        let mut file = file[..find(&file, b"5 0 obj").unwrap()].to_vec();
        let one = find(&file, b"1 0 obj").unwrap();
        let four = find(&file, b"4 0 obj").unwrap();
        let row = |kind: u8, field: usize, index: u8| {
            let [high, low] = (field as u16).to_be_bytes();
            [kind, high, low, index]
        };
        let rows = [row(0, 0, 255), row(1, 9999, 0), row(2, 4, 0), row(2, 4, 1)].concat();
        let five = file.len();
        file.extend_from_slice(
            b"5 0 obj\n<< /Type /XRef /Size 4 /W [1 2 1] /Length 16 >>\nstream\n",
        );
        file.extend_from_slice(&rows);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = file.len();
        file.extend_from_slice(
            format!(
                "xref\n0 6\n0000000000 65535 f \n{one:010} 00000 n \n0000000000 00000 f \n\
                 0000000000 00000 f \n{four:010} 00000 n \n{five:010} 00000 n \n\
                 trailer\n<< /Size 6 /Root 1 0 R /XRefStm {five} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let doc = read(&file);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.trailer.stream, None, "the table's trailer");
        assert!(doc.get((1, 0)).is_ok(), "the table's row, not the stream's");
        assert_eq!(doc.get((2, 0)).unwrap().at.unwrap().stream, Some((4, 0)));
        assert!(doc.get((3, 0)).is_ok());
    }

    /// A decoder that counts the calls made of it.
    struct Counted(BTreeMap<ObjectId, Vec<u8>>, std::cell::Cell<usize>);

    impl Decode for Counted {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            self.1.set(self.1.get() + 1);
            self.0.decode(ids)
        }
    }

    #[test]
    fn a_decoder_is_asked_only_for_an_encrypted_files_object_streams() {
        // Not in a file that doesn't encrypt.
        let (file, decoded) = compressed(false);
        let counted = Counted(decoded, std::cell::Cell::new(0));
        let doc = read_with(&file, &counted);
        assert!(doc.get((3, 0)).is_ok());
        assert_eq!(counted.1.get(), 0);
        // In one that does, once, for the object streams.
        let (file, decoded) = compressed(true);
        let counted = Counted(decoded, std::cell::Cell::new(0));
        let doc = read_with(&file, &counted);
        assert!(doc.get((3, 0)).is_ok());
        assert_eq!(counted.1.get(), 1);
        // Asked for every stream, twice: the cross-reference streams, then
        // the object streams.
        let (file, decoded) = junk(false);
        let counted = Counted(decoded, std::cell::Cell::new(0));
        let doc = read_through(&file, &counted);
        assert!(doc.get((3, 0)).is_ok());
        assert_eq!(counted.1.get(), 2);
        // A file with nothing compressed asks nothing.
        let plain = write::file("1.7", &clean(), root(), Xref::Table);
        let counted = Counted(BTreeMap::new(), std::cell::Cell::new(0));
        read_with(&plain, &counted);
        assert_eq!(counted.1.get(), 0);
    }

    #[test]
    fn an_encrypted_files_object_streams_hold_plain_strings() {
        let (file, decoded) = compressed(true);
        let doc = read_with(&file, &decoded);
        let string = |n: u32, key: &[u8]| match doc
            .get((n, 0))
            .unwrap()
            .as_dict()
            .unwrap()
            .get(key)
            .unwrap()
            .value()
        {
            Value::String(s, _) => s.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            string(1, b"Lang"),
            Str::Encrypted(b"en".to_vec()),
            "in plain view"
        );
        assert_eq!(
            string(2, b"Mine"),
            Str::Bytes(b"plain".to_vec()),
            "in an object stream"
        );
    }

    /// A decoder that finds another object by the cross-reference stream's
    /// number, 5 0 R, and decodes it from its bytes, as `compressed` writes
    /// them, to its second field.
    struct Renumbered(BTreeMap<ObjectId, Vec<u8>>, Vec<u8>);

    impl Decode for Renumbered {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            let mut decoded = self.0.decode(ids);
            // Longer than the stream's rows, which read as rows would be
            // wrong.
            if let Some(data) = decoded.get_mut(&(5, 0)) {
                *data = Ok(vec![0x01; 32]);
            }
            decoded
        }

        fn decode_written(&self, stream: Written) -> Result<Vec<u8>, String> {
            assert_eq!(stream.filter, b"/FlateDecode");
            assert_eq!(stream.parms, None);
            assert_eq!(stream.data, b"JUNK");
            Ok(self.1.clone())
        }
    }

    #[test]
    fn a_cross_reference_stream_whose_number_finds_another_is_decoded_from_its_bytes() {
        // As the reader asks a decoder for every stream when it's made to.
        let (file, decoded) = junk(false);
        let rows = decoded[&(5, 0)].clone();
        let doc = read_through(&file, &Renumbered(decoded.clone(), rows.clone()));
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.trailer.unwritten, None);
        assert!(doc.get((3, 0)).is_ok());
        // What it decodes from the bytes may run on past the rows, but not
        // end before them; then the reader says why it decoded it so.
        let longer = [&rows[..], b"more"].concat();
        let doc = read_through(&file, &Renumbered(decoded.clone(), longer));
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert!(doc.get((3, 0)).is_ok());
        let short = read_through(&file, &Renumbered(decoded.clone(), rows[..20].to_vec()));
        let why = short.trailer.unwritten.unwrap();
        assert!(why.contains("32 bytes, not the 24"), "{why}");

        // A decoder that can't decode from bytes leaves it unread, and
        // says why.
        struct ByNumber(Renumbered);
        impl Decode for ByNumber {
            fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
                self.0.decode(ids)
            }
        }
        let doc = read_through(&file, &ByNumber(Renumbered(decoded, rows)));
        let why = doc.trailer.unwritten.unwrap();
        assert!(
            why.contains("32 bytes, not the 24") && why.contains("another object 5 0 R"),
            "{why}"
        );
    }

    /// A decoder that can't read the file, as qpdf can't one whose page
    /// tree has no `/Kids`, but decodes streams from their bytes, as
    /// `compressed` writes them.
    struct Refused(BTreeMap<ObjectId, Vec<u8>>);

    impl Decode for Refused {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            let refused = || Err("qpdf refuses the file".to_string());
            ids.iter().map(|&id| (id, refused())).collect()
        }

        fn decode_written(&self, stream: Written) -> Result<Vec<u8>, String> {
            match stream.data {
                b"JUNK" => Ok(self.0[&(5, 0)].clone()),
                b"OBJS" => Ok(self.0[&(4, 0)].clone()),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn streams_a_decoder_cant_decode_by_number_are_decoded_from_their_bytes_unless_encrypted() {
        // As the reader asks a decoder for every stream when it's made to.
        let (file, decoded) = junk(false);
        let doc = read_through(&file, &Refused(decoded));
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.trailer.unwritten, None);
        assert!(doc.get((2, 0)).is_ok() && doc.get((3, 0)).is_ok());

        // An encrypted file's object streams are encrypted: its
        // cross-reference stream, never.
        let (file, decoded) = junk(true);
        let doc = read_through(&file, &Refused(decoded));
        assert_eq!(doc.trailer.unwritten, None, "the rows, from their bytes");
        let why = doc.get((3, 0)).unwrap_err().to_string();
        assert!(
            why.contains("doesn't decode: qpdf refuses the file"),
            "{why}"
        );
    }

    /// A decoder that gives these objects as plaintext, and counts the
    /// calls made of it.
    struct Plain(
        BTreeMap<ObjectId, Result<Object, String>>,
        std::cell::Cell<usize>,
    );

    impl Decode for Plain {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            BTreeMap::new().decode(ids)
        }

        fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
            self.1.set(self.1.get() + 1);
            (ids.iter())
                .filter_map(|id| Some((*id, self.0.get(id)?.clone())))
                .collect()
        }
    }

    #[test]
    fn an_encrypted_files_strings_in_plain_view_take_the_decoders_plaintext() {
        let catalog =
            b"<< /Type /Catalog /Lang (xx) /Dup (aa) /Dup (bb) /Kids [(k1) (k2)] /Short [(s1)] >>";
        let file = write::assembled(
            "1.7",
            &[
                (1, catalog),
                (2, b"<< /Filter /Standard /V 2 /O (owner) >>"),
                (3, b"<< /T (tt) >>"),
                (4, b"<< /N 4 >>"),
            ],
            "<< /Size 5 /Root 1 0 R /Encrypt 2 0 R /ID [<01> <02>] >>",
        );
        let s = |text: &str| Object::bytes(text.as_bytes().to_vec());
        let plaintext = dict! {
            "Type" => "Catalog",
            "Lang" => s("en"),
            "Dup" => s("BB"),
            "Kids" => vec![s("K1"), Object::string("K2")],
            "Short" => vec![s("S1"), s("S2")],
        };
        let decoder = Plain(
            BTreeMap::from([
                ((1, 0), Ok(plaintext.into())),
                ((3, 0), Err("a wrong password".to_string())),
            ]),
            std::cell::Cell::new(0),
        );
        let doc = read_with(&file, &decoder);
        assert_eq!(
            decoder.1.get(),
            1,
            "asked once, for the objects with ciphertext"
        );
        let catalog = doc.get((1, 0)).unwrap().as_dict().unwrap();
        let lang = catalog.get(b"Lang").unwrap();
        assert_eq!(
            *lang.value(),
            Value::String(Str::Bytes(b"en".to_vec()), Some(Form::Literal))
        );
        let at = lang.at.unwrap();
        assert_eq!(
            &file[at.start..at.end],
            b"(xx)",
            "placed where it's written"
        );
        let dup: Vec<_> = (catalog.entries().iter())
            .filter(|e| e.key == b"Dup")
            .map(|e| e.value.value().clone())
            .collect();
        assert_eq!(
            dup,
            [
                Value::String(Str::Encrypted(b"aa".to_vec()), Some(Form::Literal)),
                Value::String(Str::Bytes(b"BB".to_vec()), Some(Form::Literal))
            ],
            "only the last entry has a place in the plaintext"
        );
        let kids = plain(catalog.get(b"Kids").unwrap());
        assert_eq!(kids, Object::from(vec![s("K1"), Object::string("K2")]));
        let short = plain(catalog.get(b"Short").unwrap());
        assert_eq!(
            short,
            Object::from(vec![Object::new(Value::String(
                Str::Encrypted(b"s1".to_vec()),
                None
            ))]),
            "an array of another length has no places"
        );
        let t = doc.get((3, 0)).unwrap().as_dict().unwrap().get(b"T");
        assert_eq!(
            *t.unwrap().value(),
            Value::String(Str::Encrypted(b"tt".to_vec()), Some(Form::Literal))
        );
        let warnings: Vec<_> = doc.warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(
            warnings,
            [
                // Its dictionary, which has no /R, isn't the standard
                // handler's, so the decoder is asked, as before.
                "its security handler doesn't open: it's /V 2 /R 0, which the standard security handler isn't; its strings stay ciphertext, and its object streams aren't read",
                "2 of 1 0 R's strings stay ciphertext: the decoder reads the object otherwise",
                "one object's strings stay ciphertext: a wrong password",
            ]
        );

        // A decoder that doesn't decrypt leaves them, without a word of its
        // own: only the handler's.
        let doc = read_with(&file, &BTreeMap::new());
        let catalog = doc.get((1, 0)).unwrap().as_dict().unwrap();
        assert_eq!(
            *catalog.get(b"Lang").unwrap().value(),
            Value::String(Str::Encrypted(b"xx".to_vec()), Some(Form::Literal))
        );
        let warnings: Vec<_> = doc.warnings.iter().map(|w| w.message.as_str()).collect();
        assert!(
            matches!(warnings[..], [w] if w.starts_with("its security handler doesn't open")),
            "{warnings:?}"
        );
        // Nor is a file that isn't encrypted asked about.
        let decoder = Plain(BTreeMap::new(), std::cell::Cell::new(0));
        read_with(
            &write::assembled(
                "1.7",
                &[(1, &b"<< /Type /Catalog /Lang (en) >>"[..])],
                "<< /Size 2 /Root 1 0 R >>",
            ),
            &decoder,
        );
        assert_eq!(decoder.1.get(), 0);
    }

    /// Never a panic, whatever the bytes: every prefix of a file, every
    /// byte of one changed, and random soups of PDF's words. Whatever reads
    /// says where it was written, inside the file.
    #[test]
    fn any_bytes_read_without_a_panic() {
        let inside = |file: &[u8], doc: &Document| {
            for object in doc.objects.values().flatten() {
                let at = object.at.unwrap();
                assert!(at.start <= at.end && at.end <= file.len());
            }
        };
        let file = write::file("1.7", &clean(), root(), Xref::Table);
        for end in 0..=file.len() {
            inside(&file[..end], &read(&file[..end]));
        }
        for at in 0..file.len() {
            for b in [b'(', b'<', b'[', b'/', b'0', b' ', b'\\', 0xFF] {
                let mut changed = file.clone();
                changed[at] = b;
                inside(&changed, &read(&changed));
            }
        }
        const WORDS: &[&[u8]] = &[
            b"1",
            b"0",
            b"7",
            b" ",
            b"\n",
            b"\r",
            b"obj",
            b"endobj",
            b"stream\n",
            b"endstream",
            b"xref\n0 2\n",
            b"0000000000 65535 f \n",
            b"0000000009 00000 n \n",
            b"trailer",
            b"startxref\n",
            b"9",
            b"%%EOF",
            b"<<",
            b">>",
            b"[",
            b"]",
            b"(",
            b")",
            b"<",
            b">",
            b"/Length",
            b"/Root",
            b"/Prev",
            b"/Type",
            b"/XRef",
            b"/W",
            b"R",
            b"%PDF-1.7\n",
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut random = move |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
        for _ in 0..5_000 {
            let mut soup = Vec::new();
            for _ in 0..random(60) {
                soup.extend_from_slice(WORDS[random(WORDS.len())]);
            }
            inside(&soup, &read(&soup));
        }
    }
}
