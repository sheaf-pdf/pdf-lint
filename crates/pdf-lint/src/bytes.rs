//! A reader of a file's bytes, on [`pdf_tokenizer`] and pdf-objects'
//! [`Parser`]: every value, key and stream's data with where it was
//! written ([`Span`]), dictionaries as written with any key written twice,
//! and every choice it had to make a [`Warning`]. It reads the header,
//! `startxref`, the cross-reference tables and streams along their `/Prev`
//! chain, and each object at the offset they give, a stream's data by its
//! `/Length` when that ends at `endstream`. Where the sections don't read,
//! it finds the objects by scanning the file, and says so.
//!
//! It decodes cross-reference streams and object streams from their own
//! data, through [`pdf_filters`]: a stream up to [`STREAM`] bytes, and a
//! reading's [`BUDGET`] in all. An encrypted file it opens with the
//! passwords it's given ([`read_opened`]) through [`pdf_crypt`]'s standard
//! security handler: its object streams decrypted before they're decoded,
//! its strings decrypted where they are, a signature's `/Contents` left as
//! it is. An encrypted file it doesn't open ([`Document::encryption`]) has
//! its strings [`Str::Encrypted`], and its object streams unread, saying
//! why. An object read from an object stream is placed in the stream's
//! decoded data ([`Span::stream`]), which the [`Document`] keeps.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use pdf_tokenizer::{Kind, Token, Tokenizer, is_delimiter, is_white_space};

use pdf_crypt::{Handler, Method};
use pdf_filters::{Fault, Filter, Layout, Predictor};
use pdf_objects::parse::{self, Body, Parser, Sink};

use crate::filter;
use crate::object::{Dict, Entry, Object, ObjectId, Span, Str, Stream, Value};
use crate::source::{Document, Encryption, Trailer, Update, Warning};
use crate::version::Version;

// How deeply arrays and dictionaries may nest in an object is the
// parser's, pdf-objects'.
pub use pdf_objects::parse::NESTING;

/// How many cross-reference sections the `/Prev` chain may have.
pub const SECTIONS: usize = 1024;

/// How many bytes a stream the reader decodes may decode to; more is cut
/// off there.
#[cfg(not(fuzzing))]
pub const STREAM: usize = 256 << 20;
#[cfg(fuzzing)]
pub const STREAM: usize = 1 << 20;

/// How many bytes the streams the reader decodes in a reading of a file
/// may decode to in all; more is cut off. (`--last-update` reads a file
/// twice, as of its newest section and as of the one before.)
#[cfg(not(fuzzing))]
pub const BUDGET: usize = 1 << 30;
#[cfg(fuzzing)]
pub const BUDGET: usize = 4 << 20;

/// What a cross-reference section says of an object number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefEntry {
    /// In use, written at `offset`.
    InUse {
        offset: usize,
        generation: u16,
    },
    /// The `index`th object of object stream `stream`.
    Compressed {
        stream: u32,
        index: u32,
    },
    Free,
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
    read_with_or_without(file, None, None)
}

/// The file `file` is, opened, when it's encrypted, by the first of
/// `passwords` that opens it, each tried as the owner's and then the
/// user's (none, the empty password).
pub fn read_opened(file: &[u8], passwords: &[&[u8]]) -> Document {
    read_as(file, None, None, true, (STREAM, BUDGET, None), passwords)
}

/// The file as of the cross-reference section at byte `section`, as
/// [`read_from`] reads it, opened as [`read_opened`] opens it: an
/// update's [`Update::previous`] by its password.
pub fn read_from_opened(file: &[u8], passwords: &[&[u8]], section: usize) -> Document {
    read_as(
        file,
        None,
        Some(section),
        true,
        (STREAM, BUDGET, None),
        passwords,
    )
}

/// The file `file` is, with `decoder` decoding what isn't in plain view.
#[cfg(test)]
pub(crate) fn read_with(file: &[u8], decoder: &dyn Decode) -> Document {
    read_with_or_without(file, Some(decoder), None)
}

/// The file as of the cross-reference section at byte `section`: read from
/// there along its `/Prev` chain, as if it were the newest, so that what
/// later updates wrote isn't in it, opened, when it's encrypted, by the
/// empty password. For an update's [`Update::previous`], that's the
/// revision it updates.
pub fn read_from(file: &[u8], section: usize) -> Document {
    read_with_or_without(file, None, Some(section))
}

/// The file `file` is, with `decoder` asked for every stream the reader
/// would decode itself, as it was before it did: for the tests that set
/// the two side by side.
#[cfg(test)]
pub(crate) fn read_through(file: &[u8], decoder: &dyn Decode) -> Document {
    read_as(
        file,
        Some(decoder),
        None,
        false,
        (STREAM, BUDGET, None),
        &[],
    )
}

/// The file `file` is, a stream decoding to at most `stream` bytes and
/// the file's to `budget` in all: for the tests of the limits.
#[cfg(test)]
pub(crate) fn read_limited(file: &[u8], stream: usize, budget: usize) -> Document {
    read_as(file, None, None, true, (stream, budget, None), &[])
}

/// The file `file` is, reading no more than `entries` cross-reference
/// entries: for the tests of that limit.
#[cfg(test)]
pub(crate) fn read_listing(file: &[u8], entries: usize) -> Document {
    read_as(file, None, None, true, (STREAM, BUDGET, Some(entries)), &[])
}

fn read_with_or_without(
    file: &[u8],
    decoder: Option<&dyn Decode>,
    section: Option<usize>,
) -> Document {
    read_as(file, decoder, section, true, (STREAM, BUDGET, None), &[])
}

fn read_as(
    file: &[u8],
    decoder: Option<&dyn Decode>,
    section: Option<usize>,
    native: bool,
    limits: (usize, usize, Option<usize>),
    passwords: &[&[u8]],
) -> Document {
    reading(file, decoder, section, native, limits, passwords).0
}

/// [`read_opened`]'s reading, and the security handler it opened the file
/// with, for the oracle to decrypt each stream as the reader would.
#[cfg(test)]
pub(crate) fn read_opened_by(file: &[u8], passwords: &[&[u8]]) -> (Document, Option<Handler>) {
    reading(file, None, None, true, (STREAM, BUDGET, None), passwords)
}

/// [`read_as`]'s reading, and the security handler it opened the file
/// with.
fn reading(
    file: &[u8],
    decoder: Option<&dyn Decode>,
    section: Option<usize>,
    native: bool,
    (stream, budget, entries): (usize, usize, Option<usize>),
    passwords: &[&[u8]],
) -> (Document, Option<Handler>) {
    let mut reader = Reader {
        file,
        native,
        passwords: passwords.iter().map(|p| p.to_vec()).collect(),
        handler: None,
        encryption: Encryption::None,
        stream,
        budget,
        listable: entries.unwrap_or_else(|| listable(file.len())),
        cut: HashSet::new(),
        decoder,
        warnings: Vec::new(),
        entries: BTreeMap::new(),
        encrypted: None,
        dictionaries: HashSet::new(),
        plain_strings: false,
        plain_metadata: false,
        encrypts: false,
        resolving: false,
        decoded: BTreeMap::new(),
        update: None,
    };
    let header = reader.header();
    let sections = match section {
        Some(section) => reader.sections_from(section),
        None => reader.sections(),
    };
    let trailer = match sections {
        Ok(trailer) => trailer,
        Err(why) => reader.scan(why),
    };
    if section.is_none() {
        reader.end();
    }
    reader.encryption(&trailer);
    let mut objects = reader.objects();
    match (&reader.handler, reader.native) {
        (Some(_), true) => reader.decrypt_strings(&mut objects),
        _ => reader.decrypt(&mut objects),
    }
    // Metadata that isn't encrypted is in plain view: in a file that
    // doesn't encrypt, or by `/EncryptMetadata false`, which counts from
    // version 4, whether the handler opens or not.
    let plain_metadata = !reader.encrypts || reader.plain_metadata;
    let metadata = metadata(file, &trailer, &objects, !plain_metadata);
    let document = Document {
        objects,
        trailer,
        header,
        warnings: reader.warnings,
        decoded: reader.decoded,
        update: reader.update,
        size: Some(file.len()),
        metadata,
        encryption: reader.encryption,
    };
    (document, reader.handler)
}

/// The data of the catalog's metadata stream (ISO 32000-2, 14.3.2), as
/// written in `file`, when it's in plain view: unfiltered, and not
/// `encrypted`.
fn metadata(
    file: &[u8],
    trailer: &Trailer,
    objects: &BTreeMap<ObjectId, Result<Object, String>>,
    encrypted: bool,
) -> Option<Vec<u8>> {
    /// `o` with its references followed, as many as an object's way to
    /// the catalog may take.
    fn get<'a>(
        objects: &'a BTreeMap<ObjectId, Result<Object, String>>,
        mut o: &'a Object,
    ) -> Option<&'a Object> {
        for _ in 0..8 {
            match o.value() {
                Value::Ref(id) => o = objects.get(id)?.as_ref().ok()?,
                _ => return Some(o),
            }
        }
        None
    }
    let dict = |o| match get(objects, o)?.value() {
        Value::Dict(d) => Some(d),
        _ => None,
    };
    let trailer = trailer.dict();
    if encrypted {
        return None;
    }
    let catalog = dict(trailer.get(b"Root")?)?;
    let Value::Stream(stream) = get(objects, catalog.get(b"Metadata")?)?.value() else {
        return None;
    };
    let at = (stream.data.at()).filter(|at| at.stream.is_none() && !stream.dict.has(b"Filter"))?;
    file.get(at.start..at.end).map(<[u8]>::to_vec)
}

struct Reader<'a> {
    file: &'a [u8],
    /// The passwords to open an encrypted file with.
    passwords: Vec<Vec<u8>>,
    /// The security handler of an encrypted file it opened.
    handler: Option<Handler>,
    /// Whether the file is encrypted, and if it is, whether it opened.
    encryption: Encryption,
    /// Whether it decodes streams itself, as it does but in the tests that
    /// make it ask the decoder.
    native: bool,
    /// How many bytes a stream it decodes may decode to.
    stream: usize,
    /// How many bytes the streams it decodes may still decode to.
    budget: usize,
    /// How many cross-reference entries it reads ([`listable`]).
    listable: usize,
    /// The object streams whose data a fault cut off.
    cut: HashSet<ObjectId>,
    decoder: Option<&'a dyn Decode>,
    warnings: Vec<Warning>,
    /// The newest entry for each object number, with its generation.
    entries: BTreeMap<u32, (u16, XrefEntry)>,
    /// In an encrypted file whose strings are encrypted, the objects whose
    /// strings aren't: the encryption dictionary's.
    encrypted: Option<HashSet<ObjectId>>,
    /// The objects the sections' trailers name as `/Encrypt`, and those its
    /// values refer to: an earlier update's encryption dictionary's strings
    /// aren't encrypted either.
    dictionaries: HashSet<ObjectId>,
    /// Whether an encrypted file's strings aren't, by its handler's
    /// `/StrF`, or its dictionary's when it doesn't open.
    plain_strings: bool,
    /// Whether an encrypted file's metadata stream isn't, by
    /// `/EncryptMetadata false` of a dictionary of `/V` 4 or later.
    plain_metadata: bool,
    /// Whether the file has an encryption dictionary, so that its streams
    /// may be encrypted, whatever its strings are.
    encrypts: bool,
    /// Whether a stream's indirect `/Length` is being read, which reads no
    /// other.
    resolving: bool,
    /// The object streams' decoded data, which their objects are read from.
    decoded: BTreeMap<ObjectId, Vec<u8>>,
    /// The newest section, when it updates one written before it.
    update: Option<Update>,
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

/// A cross-reference section, read but for its stream's rows.
struct Section {
    trailer: Trailer,
    prev: Option<usize>,
    /// A table's rows.
    table: BTreeMap<u32, (u16, XrefEntry)>,
    /// The cross-reference stream whose rows it has, at its offset: the
    /// section's own, or a hybrid table's `/XRefStm`.
    stream: Option<(usize, ObjectId, Object)>,
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

impl<'a> Reader<'a> {
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

    /// Stream `id`'s data, written at `at`, decoded by `filters`, as much
    /// as `limit`, [`STREAM`] and what's left of the file's [`BUDGET`]
    /// allow: what decoded, however far it went, and the faults on the
    /// way, each in its filter.
    fn decode_stream(
        &mut self,
        data: &[u8],
        filters: &[Filter],
        limit: usize,
    ) -> (Vec<u8>, Vec<(usize, Fault)>) {
        let limit = limit.min(self.stream).min(self.budget);
        let (decoded, faults) = match pdf_filters::decode(data, filters, limit) {
            Ok(decoded) => (decoded, Vec::new()),
            Err(error) => (error.data, error.faults),
        };
        self.budget -= decoded.len();
        (decoded, faults)
    }

    /// A warning on stream `id` for each of `faults` in its `filters`: at
    /// the fault's bytes, when it has a place in the data as written
    /// (`data`, in the file), else at the data.
    fn faulted(
        &mut self,
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
            if !self.warnings.contains(&warning) {
                self.warnings.push(warning);
            }
        }
    }

    /// Object stream `id`'s data, decoded from its own, its filters'
    /// values followed where they're references to `objects`.
    fn object_stream(
        &mut self,
        id: ObjectId,
        object: &Object,
        objects: &BTreeMap<ObjectId, Result<Object, String>>,
    ) -> Result<Vec<u8>, String> {
        let Value::Stream(stream) = object.value() else {
            return Err("it isn't a stream".to_string());
        };
        // Not decoded, and so not counted against the budget, unless it's
        // an object stream: its header says why it's not.
        if stream.dict.get(b"Type").and_then(Object::as_name) != Some(b"ObjStm") {
            return Ok(Vec::new());
        }
        let data = (stream.data.at())
            .filter(|at| at.stream.is_none())
            .and_then(|at| self.file.get(at.start..at.end))
            .unwrap_or_default();
        let resolve = |id| objects.get(&id)?.as_ref().ok();
        let filters = filter::filters(&stream.dict, Some(&resolve))?;
        // In an encrypted file, decrypted first: by its own crypt filter
        // when it names one, else as the file's streams are. In one no
        // password opened whose streams aren't encrypted, its own crypt
        // filter may yet encrypt it.
        let crypt = filter::crypt_filter(&stream.dict, Some(&resolve));
        if self.handler.is_none()
            && self.encrypts
            && let Some(name) = crypt.as_deref().filter(|&name| name != b"Identity")
        {
            let name = String::from_utf8_lossy(name);
            return Err(format!(
                "its own /Crypt filter, /{name}, encrypts it, and no password the reader was given opens the file"
            ));
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
                self.warn(Some(id), stream.data.at(), message);
                match error.data {
                    Some(plain) => (plain, false),
                    None => return Err(format!("its data doesn't decrypt: {fault}")),
                }
            }
        };
        if filters.is_empty() {
            return Ok(data);
        }
        let (decoded, faults) = self.decode_stream(&data, &filters, STREAM);
        if faults.iter().any(|(_, fault)| stops(fault)) {
            self.cut.insert(id);
        }
        // A fault's place is in the data as written only when it wasn't
        // decrypted first.
        let at = stream.data.at().filter(|_| exact);
        self.faulted(id, stream.data.at(), at, &filters, faults);
        Ok(decoded)
    }

    /// What the decoder makes of `ids`; nothing without one.
    fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
        match self.decoder {
            Some(decoder) if !ids.is_empty() => decoder.decode(ids),
            _ => BTreeMap::new(),
        }
    }

    /// Whether object `id`'s strings are ciphertext.
    fn ciphertext(&self, id: ObjectId) -> bool {
        (self.encrypted.as_ref()).is_some_and(|plain| !plain.contains(&id))
    }

    /// Whether the file ends as ISO 32000-2 7.5.5 has it, its last line
    /// `%%EOF` alone; and, failing that, whether a `%%EOF` is within its last
    /// 1024 bytes, where readers look for one.
    fn end(&mut self) {
        let file = self.file;
        let eols = file
            .iter()
            .rev()
            .take_while(|b| matches!(b, b'\r' | b'\n'))
            .count();
        let body = &file[..file.len() - eols];
        let alone = |at: usize| at == 0 || matches!(body[at - 1], b'\r' | b'\n');
        if body.ends_with(b"%%EOF") && alone(body.len() - 5) {
            return;
        }
        let tail = file.len().saturating_sub(1024);
        let (at, message) = match rfind(&file[tail..], b"%%EOF").map(|i| tail + i) {
            Some(at) if at + 5 == body.len() => (
                span(at, at + 5),
                "the file's last %%EOF isn't on a line of its own".to_string(),
            ),
            Some(at) => (
                span(at + 5, file.len()),
                format!(
                    "the file goes on for {} bytes after its last %%EOF",
                    file.len() - at - 5
                ),
            ),
            None => match rfind(file, b"%%EOF") {
                Some(at) => (
                    span(at, at + 5),
                    format!(
                        "the file's last %%EOF is {} bytes from its end, past the 1024 readers look in",
                        file.len() - at
                    ),
                ),
                None => (
                    span(file.len(), file.len()),
                    "the file has no %%EOF".to_string(),
                ),
            },
        };
        self.warn(None, Some(at), message);
    }

    /// The version `%PDF-` gives, within the first 1024 bytes.
    fn header(&mut self) -> Option<Version> {
        let head = &self.file[..self.file.len().min(1024)];
        let Some(at) = find(head, b"%PDF-") else {
            self.warn(None, None, "no %PDF- header".to_string());
            return None;
        };
        if at > 0 {
            let message = format!("%PDF- is at byte {at}, not the file's first");
            self.warn(None, Some(span(at, at + 5)), message);
        }
        let rest = &self.file[at + 5..];
        let end = (rest.iter())
            .position(|&b| is_white_space(b) || is_delimiter(b))
            .unwrap_or(rest.len());
        let version = Version::parse(&rest[..end]);
        if version.is_none() {
            let message = "the header's version doesn't read".to_string();
            self.warn(None, Some(span(at, at + 5 + end)), message);
        }
        version
    }

    /// The sections from the last `startxref` along their `/Prev` chain,
    /// their entries the newest for each number; the newest's trailer.
    /// Compressed streams' rows are decoded together, once the chain is
    /// known: each stream's dictionary, `/Prev` and all, is in plain view.
    fn sections(&mut self) -> Result<Trailer, String> {
        let at = rfind(self.file, b"startxref").ok_or("no startxref")?;
        let mut t = Tokenizer::at(self.file, at);
        if !is_keyword(t.next().as_ref(), b"startxref") {
            return Err(format!("startxref at byte {at} is inside another word"));
        }
        let offset = (significant(&mut t))
            .filter(|token| token.kind == Kind::Integer)
            .and_then(|token| usize::try_from(token.integer().ok()?).ok())
            .ok_or("startxref gives no offset")?;
        self.sections_from(offset)
    }

    /// The sections from the one at `offset` along their `/Prev` chain, as
    /// [`Reader::sections`] reads them from `startxref`'s.
    fn sections_from(&mut self, offset: usize) -> Result<Trailer, String> {
        let mut sections: Vec<Section> = Vec::new();
        let mut offsets = Vec::new();
        let mut seen = HashSet::new();
        let mut next = Some(offset);
        while let Some(offset) = next {
            if !seen.insert(offset) {
                let message = format!("the /Prev chain comes back to byte {offset}");
                self.warn(None, None, message);
                break;
            }
            if seen.len() > SECTIONS {
                let message = format!("more than {SECTIONS} cross-reference sections");
                self.warn(None, None, message);
                break;
            }
            let section = match self.section(offset) {
                Ok(section) => section,
                Err(why) if sections.is_empty() => return Err(why),
                Err(why) => {
                    let message = format!("the section at byte {offset} doesn't read: {why}");
                    self.warn(None, None, message);
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
        let decoded = self.decode(&compressed);
        let mut trailer = None;
        for (i, section) in sections.into_iter().enumerate() {
            let mut entries = section.table;
            if let Some((offset, id, stream)) = &section.stream {
                let hybrid = section.trailer.stream.is_none();
                match self.rows(*offset, *id, stream, decoded.get(id), !hybrid) {
                    // A hybrid file's stream lists what its table leaves free.
                    Ok(rows) => {
                        for (n, entry) in rows {
                            let free = matches!(entries.get(&n), None | Some((_, XrefEntry::Free)));
                            if free {
                                entries.insert(n, entry);
                            }
                        }
                    }
                    Err(why) if i == 0 && !hybrid => return Err(why),
                    Err(why) => {
                        let what = if hybrid {
                            "its /XRefStm"
                        } else {
                            "the section"
                        };
                        let message = format!("{what}, at byte {offset}, doesn't read: {why}");
                        self.warn(None, None, message);
                    }
                }
            }
            // An update is written after the section it follows: a
            // linearized file's first section, before its other, isn't one.
            if let [newest, previous, ..] = offsets[..]
                && i == 0
                && newest > previous
            {
                let objects = (entries.iter())
                    .filter(|&(&n, (_, entry))| n != 0 && *entry != XrefEntry::Free)
                    .map(|(&n, &(generation, _))| (n, generation))
                    .collect();
                self.update = Some(Update { objects, previous });
            }
            let most = self.listable;
            let mut past = 0usize;
            for (n, entry) in entries {
                match self.entries.len() < most || self.entries.contains_key(&n) {
                    true => {
                        self.entries.entry(n).or_insert(entry);
                    }
                    false => past += 1,
                }
            }
            if past > 0 {
                let message = format!(
                    "the cross-reference sections list {past} objects past the first {most}, as many as the reader reads of a file of {} bytes, which aren't read",
                    self.file.len()
                );
                self.warn(None, None, message);
            }
            trailer.get_or_insert(section.trailer);
        }
        trailer.ok_or_else(|| "no section".to_string())
    }

    /// The section at `offset`: a table, or a cross-reference stream.
    fn section(&mut self, offset: usize) -> Result<Section, String> {
        let mut t = Tokenizer::at(self.file, offset);
        match significant(&mut t) {
            Some(token) if is_keyword(Some(&token), b"xref") => self.table(&mut t),
            Some(token) if token.kind == Kind::Integer => {
                let (id, object) = self.xref_stream(offset)?;
                let prev = object
                    .as_dict()
                    .and_then(|d| d.get(b"Prev"))
                    .and_then(offset_of);
                Ok(Section {
                    trailer: Trailer {
                        stream: Some(id),
                        object: object.clone(),
                        unwritten: None,
                    },
                    prev,
                    table: BTreeMap::new(),
                    stream: Some((offset, id, object)),
                })
            }
            _ => Err(format!(
                "byte {offset}, which startxref or /Prev gives, is at neither `xref` nor an object"
            )),
        }
    }

    /// A table's subsections, after `xref`, and its trailer; and a hybrid
    /// file's `/XRefStm`.
    fn table(&mut self, t: &mut Tokenizer<'a>) -> Result<Section, String> {
        let integer = |t: &mut Tokenizer<'a>| {
            (significant(t))
                .filter(|token| token.kind == Kind::Integer)
                .and_then(|token| token.integer().ok())
        };
        let mut entries = BTreeMap::new();
        // The rows not as ISO 32000-2 7.5.4 writes them, and the first.
        let (mut rows, mut misshapen) = (0, 0);
        let mut first: Option<(u32, Span, &str)> = None;
        loop {
            let at = t.offset();
            match significant(&mut t.clone()) {
                Some(token) if is_keyword(Some(&token), b"trailer") => {
                    significant(t);
                    break;
                }
                Some(token) if token.kind == Kind::Integer => {
                    let start = integer(t).and_then(|n| u32::try_from(n).ok());
                    let count = integer(t).and_then(|n| u32::try_from(n).ok());
                    let (Some(start), Some(count)) = (start, count) else {
                        return Err(format!("a subsection's header at byte {at} doesn't read"));
                    };
                    for i in 0..count {
                        let row = significant(&mut t.clone()).map_or(t.offset(), |t| t.start);
                        let Some(n) = start.checked_add(i) else {
                            return Err(format!("the table's row at byte {row} numbers no object"));
                        };
                        rows += 1;
                        if let Err(why) = row_shape(self.file, row) {
                            misshapen += 1;
                            let end = (row + 20).min(self.file.len());
                            first.get_or_insert((n, span(row, end), why));
                        }
                        let offset = integer(t);
                        let generation = integer(t);
                        let kind = significant(t);
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
                                self.warn(None, Some(span(row, t.offset())), message);
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
            self.warn(None, Some(at), message);
        }
        let trailer = (Parser::at(self.file, t.offset()))
            .value(&mut self.warned(None))
            .map_err(|e| e.to_string())?;
        let Some(dict) = trailer.as_dict() else {
            return Err("the trailer isn't a dictionary".to_string());
        };
        let prev = dict.get(b"Prev").and_then(offset_of);
        let stream = match dict.get(b"XRefStm").and_then(offset_of) {
            Some(at) => match self.xref_stream(at) {
                Ok((id, object)) => Some((at, id, object)),
                Err(why) => {
                    let message = format!("its /XRefStm, at byte {at}, doesn't read: {why}");
                    self.warn(None, None, message);
                    None
                }
            },
            None => None,
        };
        Ok(Section {
            trailer: Trailer {
                stream: None,
                object: trailer,
                unwritten: None,
            },
            prev,
            table: entries,
            stream,
        })
    }

    /// The cross-reference stream at `offset`: a stream, `/Type /XRef`.
    fn xref_stream(&mut self, offset: usize) -> Result<(ObjectId, Object), String> {
        let (id, object) = self.indirect(offset, None)?;
        let Value::Stream(stream) = object.value() else {
            return Err(format!(
                "{} {} R, at byte {offset}, isn't a stream",
                id.0, id.1
            ));
        };
        if stream.dict.get(b"Type").and_then(Object::as_name) != Some(b"XRef") {
            return Err(format!(
                "{} {} R, at byte {offset}, isn't /Type /XRef",
                id.0, id.1
            ));
        }
        Ok((id, object))
    }

    /// The rows of cross-reference stream `id`, written at `offset`, read
    /// by its `/W` and `/Index` (ISO 32000-2, 7.5.8): from its data, decoded
    /// from its own (or, when the reader is made to ask the decoder, as
    /// `decoded` has it). A section's `own` stream lists itself; a hybrid
    /// table's `/XRefStm` is listed by the table.
    fn rows(
        &mut self,
        offset: usize,
        id: ObjectId,
        object: &Object,
        decoded: Option<&Result<Vec<u8>, String>>,
        own: bool,
    ) -> Result<BTreeMap<u32, (u16, XrefEntry)>, String> {
        let Value::Stream(stream) = object.value() else {
            return Err(format!("{} {} R isn't a stream", id.0, id.1));
        };
        let dict = &stream.dict;
        let written = (stream.data.at())
            .and_then(|at| self.file.get(at.start..at.end))
            .unwrap_or_default();
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
            // never encrypted (ISO 32000-2, 7.5.8.1), and its dictionary's
            // values are direct (7.5.8.2). What runs past its rows, which
            // aren't read, isn't decoded.
            true => {
                let filters = filter::filters(dict, None)
                    .map_err(|why| format!("its cross-reference stream doesn't decode: {why}"))?;
                match filters.is_empty() {
                    true => written,
                    false => {
                        // A predictor's rows are read whole: its rows, and
                        // one of the predictor's past them.
                        let limit = rows_size
                            .map_or(STREAM, |size| size.saturating_add(predictor_row(&filters)));
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
                        self.faulted(id, stream.data.at(), stream.data.at(), &filters, faults);
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
                        again = (self.decoder)
                            .zip(self.written(dict, written))
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
                self.file.len()
            );
            self.warn(Some(id), object.at, message);
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
            self.warn(Some(id), object.at, message);
        }
        Ok(entries)
    }

    /// Stream `object` as written, when it's a stream whose filters are
    /// written in its dictionary.
    fn written_stream(&self, object: &Object) -> Option<Written<'a>> {
        let Value::Stream(stream) = object.value() else {
            return None;
        };
        let data = (stream.data.at()).and_then(|at| self.file.get(at.start..at.end))?;
        self.written(&stream.dict, data)
    }

    /// A stream's filters, their parameters and `data`, as written, when
    /// they're written in its dictionary, not as references.
    fn written(&self, dict: &Dict, data: &'a [u8]) -> Option<Written<'a>> {
        let file = self.file;
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
    fn scan(&mut self, why: String) -> Trailer {
        self.entries.clear();
        let mut t = Tokenizer::new(self.file);
        let mut before: [Option<Token>; 2] = [None, None];
        let mut trailers = Vec::new();
        while let Some(token) = t.next() {
            match token.kind {
                Kind::Space | Kind::Comment => continue,
                Kind::Keyword if token.bytes == b"obj" => {
                    if let [Some(n), Some(g)] = before
                        && n.kind == Kind::Integer
                        && g.kind == Kind::Integer
                        && let (Some(number), Some(generation)) = (
                            n.integer().ok().and_then(|n| u32::try_from(n).ok()),
                            g.integer().ok().and_then(|g| u16::try_from(g).ok()),
                        )
                    {
                        let offset = n.start;
                        self.entries.insert(
                            number,
                            (generation, XrefEntry::InUse { offset, generation }),
                        );
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
        // The last trailer, with what it lacks of the document's entries
        // from those before it, newest first: a linearized file's last
        // may have only /Size, its first page's the rest.
        let mut found: Option<Object> = None;
        let mut taken = Vec::new();
        for at in trailers.into_iter().rev() {
            let Ok(object) = Parser::at(self.file, at).value(&mut self.warned(None)) else {
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
                self.warn(None, object.at, message);
            }
            return Trailer {
                stream: None,
                object,
                unwritten: Some(why),
            };
        }
        let mut offsets: Vec<(usize, ObjectId)> = (self.entries.iter())
            .filter_map(|(&n, &(g, entry))| match entry {
                XrefEntry::InUse { offset, .. } => Some((offset, (n, g))),
                _ => None,
            })
            .collect();
        offsets.sort();
        for (offset, id) in offsets.into_iter().rev() {
            if let Ok((_, object)) = self.indirect(offset, Some(id))
                && is_xref_stream(&object)
            {
                return Trailer {
                    stream: Some(id),
                    object,
                    unwritten: Some(why),
                };
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
    /// (ISO 32000-2, 7.6.5). The encryption dictionary's own strings never
    /// are, an earlier update's either, nor those in object streams, which
    /// are encrypted whole.
    fn encryption(&mut self, trailer: &Trailer) {
        let Some(encrypt) = trailer.dict().get(b"Encrypt") else {
            return;
        };
        let (dict, own, why) = match encrypt.value() {
            // A null is no entry, and so is a reference to no object (ISO
            // 32000-2, 7.3.9 and 7.3.10).
            Value::Null => return,
            Value::Ref(id) => match self.entries.get(&id.0) {
                Some(&(_, XrefEntry::InUse { offset, .. })) => {
                    match self.indirect(offset, Some(*id)) {
                        Ok((_, object)) => {
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
                    self.warn(None, encrypt.at, message);
                    return;
                }
            },
            Value::Dict(dict) => (Some(dict.clone()), None, ""),
            _ => (None, None, "isn't a dictionary"),
        };
        self.encrypts = true;
        self.encryption = match &dict {
            None => Encryption::Unsupported(format!("its /Encrypt {why}")),
            Some(dict) => self.open(dict, trailer, own),
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
            self.warn(None, encrypt.at, message);
        }
        if self.plain_strings {
            return;
        }
        let earlier = std::mem::take(&mut self.dictionaries);
        self.encrypted = Some(own.into_iter().chain(earlier).collect());
    }

    /// The security handler of encryption dictionary `dict` (object `own`,
    /// when it's indirect), opened by the reader's passwords, as far as it
    /// opens; its values followed where they're references, which are its
    /// own, so not encrypted. Whether its strings and metadata are plain
    /// is set by it.
    fn open(&mut self, dict: &Dict, trailer: &Trailer, own: Option<ObjectId>) -> Encryption {
        let mut get = |o: Option<&Object>| -> Option<Object> {
            let o = o?;
            match o.value() {
                Value::Ref(id) => match self.entries.get(&id.0) {
                    Some(&(_, XrefEntry::InUse { offset, .. })) => {
                        self.dictionaries.insert(*id);
                        self.indirect(offset, Some(*id)).ok().map(|(_, o)| o)
                    }
                    _ => None,
                },
                _ => Some(o.clone()),
            }
        };
        let bytes = |o: Option<Object>| match o.map(|o| o.into_value()) {
            Some(Value::String(s, _)) => Some(s.as_bytes().map(<[u8]>::to_vec).unwrap_or_default()),
            _ => None,
        };
        let name = |o: Option<Object>| o.and_then(|o| o.as_name().map(<[u8]>::to_vec));
        let integer = |o: Option<Object>| o.and_then(|o| o.as_integer());
        let mut crypt_filters = Vec::new();
        if let Some(Value::Dict(filters)) = get(dict.get(b"CF")).map(|o| o.into_value()) {
            for (name, filter) in filters.iter() {
                let method = match get(Some(filter)).map(|o| o.into_value()) {
                    Some(Value::Dict(filter)) => (get(filter.get(b"CFM")).as_ref())
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
            filter: name(get(dict.get(b"Filter"))).unwrap_or_default(),
            v: integer(get(dict.get(b"V"))).unwrap_or(0),
            r: integer(get(dict.get(b"R"))).unwrap_or(0),
            length: integer(get(dict.get(b"Length"))),
            o: bytes(get(dict.get(b"O"))),
            u: bytes(get(dict.get(b"U"))),
            oe: bytes(get(dict.get(b"OE"))),
            ue: bytes(get(dict.get(b"UE"))),
            perms: bytes(get(dict.get(b"Perms"))),
            p: integer(get(dict.get(b"P"))),
            encrypt_metadata: match get(dict.get(b"EncryptMetadata")).map(|o| o.into_value()) {
                Some(Value::Bool(b)) => Some(b),
                _ => None,
            },
            crypt_filters,
            stm_f: name(get(dict.get(b"StmF"))),
            str_f: name(get(dict.get(b"StrF"))),
        };
        // The file's first /ID element, as qpdf takes it: empty, said once
        // the standard handler takes the dictionary, when it isn't a string.
        let id = match get(trailer.dict().get(b"ID")).map(|o| o.into_value()) {
            Some(Value::Array(id)) => match id.first().map(|o| o.value()) {
                Some(Value::String(s, _)) => s.as_bytes().map(<[u8]>::to_vec),
                _ => None,
            },
            _ => None,
        };
        let no_id = id.is_none();
        let passwords: Vec<&[u8]> = self.passwords.iter().map(Vec::as_slice).collect();
        let opened = Handler::open(&values, id.as_deref().unwrap_or_default(), &passwords);
        if no_id && !matches!(opened, Err(pdf_crypt::Refused::Unsupported(_))) {
            let at = trailer.dict().get(b"ID").and_then(|o| o.at);
            let message = "its trailer's /ID has no string first, so its key is made without one";
            self.warn(None, at, message.to_string());
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
        self.plain_metadata = plain_streams
            || match &opened {
                Ok(handler) => !handler.encrypts_metadata(),
                Err(_) => values.v >= 4 && values.encrypt_metadata == Some(false),
            };
        match opened {
            Ok(handler) => {
                for fault in handler.faults() {
                    self.warn(
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
        }
    }

    /// Every object the entries list: each in use read at its offset, then
    /// each in an object stream from the stream's decoded data.
    fn objects(&mut self) -> BTreeMap<ObjectId, Result<Object, String>> {
        let entries: Vec<(u32, (u16, XrefEntry))> =
            self.entries.iter().map(|(&n, &e)| (n, e)).collect();
        let mut objects = BTreeMap::new();
        let mut compressed = Vec::new();
        for (n, (generation, entry)) in entries {
            let id = (n, generation);
            if n == 0 && entry != XrefEntry::Free {
                let message =
                    "the cross-references list object 0, which is always free (ISO 32000-2, 7.5.4)";
                self.warn(None, None, message.to_string());
                continue;
            }
            match entry {
                XrefEntry::Free => {}
                XrefEntry::InUse { offset, .. } => {
                    let object = self.indirect(offset, Some(id)).map(|(_, mut o)| {
                        // A cross-reference stream is never encrypted (ISO
                        // 32000-2, 7.5.8.1).
                        if self.encrypted.is_some() && is_xref_stream(&o) {
                            plain_strings(&mut o);
                        }
                        o
                    });
                    objects.insert(id, object);
                }
                XrefEntry::Compressed { stream, index } => compressed.push((n, stream, index)),
            }
        }
        let streams: BTreeSet<u32> = compressed.iter().map(|&(_, stream, _)| stream).collect();
        // In a file that doesn't encrypt, or whose handler opened, or whose
        // streams aren't encrypted, each decoded from its own data; else
        // asked of the decoder.
        let plain = self.encryption == Encryption::Plain;
        let native = self.native && (!self.encrypts || self.handler.is_some() || plain);
        let mut decoded = match native {
            true => (streams.iter())
                .filter_map(|&n| {
                    let object = objects.get(&(n, 0))?.as_ref().ok()?;
                    Some(((n, 0), self.object_stream((n, 0), object, &objects)))
                })
                .collect(),
            false => {
                // An object stream without filters, in a file that doesn't
                // encrypt, is in plain view: its data is as written.
                let plain: BTreeMap<u32, Vec<u8>> = (streams.iter())
                    .filter(|_| !self.encrypts)
                    .filter_map(|&n| {
                        let object = objects.get(&(n, 0))?.as_ref().ok()?;
                        let Value::Stream(stream) = object.value() else {
                            return None;
                        };
                        let at = stream.data.at().filter(|at| at.stream.is_none())?;
                        let data = self.file.get(at.start..at.end)?;
                        filters(object).is_none().then(|| (n, data.to_vec()))
                    })
                    .collect();
                let ids: Vec<ObjectId> = (streams.iter())
                    .filter(|n| !plain.contains_key(n))
                    .map(|&n| (n, 0))
                    .collect();
                let mut decoded = self.decode(&ids);
                decoded.extend(plain.into_iter().map(|(n, data)| ((n, 0), Ok(data))));
                decoded
            }
        };
        let mut headers: BTreeMap<u32, Result<Vec<(u32, usize)>, String>> = BTreeMap::new();
        for &stream in &streams {
            let id = (stream, 0);
            let header = match (objects.get(&id), decoded.remove(&id)) {
                (Some(Ok(object)), decoded) => {
                    let by_number = match decoded {
                        Some(Ok(data)) => object_stream_header(object, &data).map(|h| (h, data)),
                        Some(Err(why)) => Err(format!(
                            "its object stream {stream} 0 R doesn't decode: {why}"
                        )),
                        None => Err(format!(
                            "it's in object stream {stream} 0 R, and nothing decodes that"
                        )),
                    };
                    // Asked of the decoder in a file that isn't encrypted,
                    // a stream it can't decode by its number, or decodes as
                    // another object's, is decoded from its bytes.
                    let decoded = by_number.or_else(|why| {
                        (self.decoder)
                            .filter(|_| !self.encrypts && !native)
                            .zip(self.written_stream(object))
                            .and_then(|(decoder, written)| decoder.decode_written(written).ok())
                            .and_then(|data| {
                                Some((object_stream_header(object, &data).ok()?, data))
                            })
                            .ok_or(why)
                    });
                    decoded.map(|(header, data)| {
                        self.decoded.insert(id, data);
                        header
                    })
                }
                (Some(Err(why)), _) => Err(format!(
                    "its object stream {stream} 0 R doesn't read: {why}"
                )),
                (None, _) => Err(format!("its object stream {stream} 0 R isn't in the file")),
            };
            headers.insert(stream, header);
        }
        for (n, stream, index) in compressed {
            let id = (n, 0);
            let object = match &headers[&stream] {
                Err(why) => Err(why.clone()),
                Ok(header) => self.in_stream(id, (stream, 0), header, index),
            };
            objects.insert(id, object);
        }
        objects
    }

    /// In an encrypted file it opened, the strings in plain view decrypted,
    /// each where it is, but a signature's `/Contents`, which isn't
    /// encrypted; one that doesn't decrypt cleanly said, once an object and
    /// a fault.
    fn decrypt_strings(&mut self, objects: &mut BTreeMap<ObjectId, Result<Object, String>>) {
        let Some(handler) = self.handler.take() else {
            return;
        };
        for (&id, object) in objects.iter_mut() {
            let Ok(object) = object else {
                continue;
            };
            if ciphertexts(object.value()) == 0 {
                continue;
            }
            let mut faults: BTreeMap<String, (usize, Option<Span>)> = BTreeMap::new();
            plaintext(object, id, &handler, &mut faults);
            for (fault, (count, at)) in faults {
                let message = match count {
                    1 => format!("a string of it doesn't decrypt cleanly: {fault}"),
                    n => format!("{n} of its strings don't decrypt cleanly: {fault}"),
                };
                self.warn(Some(id), at, message);
            }
        }
        self.handler = Some(handler);
    }

    /// In an encrypted file, the strings in plain view decrypted: the
    /// objects holding ciphertext asked of the decoder, and each string
    /// replaced by the one where it is in the decoder's object. A string it
    /// can't place stays ciphertext, and so does one the decoder can't
    /// decrypt, and the reader says so.
    fn decrypt(&mut self, objects: &mut BTreeMap<ObjectId, Result<Object, String>>) {
        let Some(decoder) = self.decoder.filter(|_| self.encrypted.is_some()) else {
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
                        self.warn(Some(id), object.at, message);
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
            self.warn(
                None,
                None,
                format!("{objects} strings stay ciphertext: {why}"),
            );
        }
    }

    /// Object `id`, the `index`th of object stream `stream`, whose header
    /// says where each object starts.
    fn in_stream(
        &mut self,
        id: ObjectId,
        stream: ObjectId,
        header: &[(u32, usize)],
        index: u32,
    ) -> Result<Object, String> {
        let &(number, offset) = usize::try_from(index)
            .ok()
            .and_then(|i| header.get(i))
            .ok_or_else(|| format!("object stream {} 0 R has no object {index}", stream.0))?;
        if number != id.0 {
            return Err(format!(
                "object {index} of object stream {} 0 R is {number}, not {}",
                stream.0, id.0
            ));
        }
        // Taken out while it's read, as its warnings borrow the reader, and
        // put back.
        let Some(data) = self.decoded.get_mut(&stream).map(std::mem::take) else {
            return Err(format!("object stream {} 0 R has no data", stream.0));
        };
        let object = (Parser::at(&data, offset).in_stream(stream))
            .value(&mut self.warned(Some(id)))
            .map_err(|e| e.to_string());
        // One that runs to where a fault cut the data off may be cut short,
        // when it's a value nothing closes: a number, a name, a keyword.
        let open = |o: &Object| {
            !matches!(
                o.value(),
                Value::String(..) | Value::Array(_) | Value::Dict(_) | Value::Stream(_)
            )
        };
        let cut = self.cut.contains(&stream)
            && (object.as_ref())
                .is_ok_and(|o| open(o) && o.at.is_some_and(|at| at.end == data.len()));
        self.decoded.insert(stream, data);
        match cut {
            true => Err(format!(
                "it runs to where a fault cut object stream {} 0 R's data off, so it may be cut short",
                stream.0
            )),
            false => object,
        }
    }

    /// The indirect object at `offset`: `n g obj`, a value (a dictionary
    /// with its stream), `endobj`. When `expected`, it must be that one. A
    /// stream's data is as long as its `/Length` says when that ends at
    /// `endstream`, else up to `endstream`.
    fn indirect(
        &mut self,
        offset: usize,
        expected: Option<ObjectId>,
    ) -> Result<(ObjectId, Object), String> {
        let mut parser = Parser::at(self.file, offset);
        let id = parser.header().map_err(|e| e.to_string())?;
        if let Some(expected) = expected
            && expected != id
        {
            return Err(format!(
                "byte {offset} holds {} {} obj, where the cross-references put {} {} R",
                id.0, id.1, expected.0, expected.1
            ));
        }
        let mut parser = parser.ciphertext(self.ciphertext(id));
        let body = parser.body(id, &mut self.warned(Some(id)));
        let object = match body.map_err(|e| e.to_string())? {
            Body::Object(object) => object,
            Body::Stream(head) => {
                let length = self.length(head.dict(), id);
                let data = match length.and_then(|len| head.declared(len)) {
                    Some(data) => data,
                    None => {
                        let message = match length {
                            Some(len) => format!("its /Length, {len}, doesn't end at endstream"),
                            None => "it has no /Length that reads".to_string(),
                        };
                        let at = (head.dict().get(b"Length"))
                            .and_then(|l| l.at)
                            .unwrap_or(head.keyword());
                        self.warn(Some(id), Some(at), message);
                        head.scanned()
                    }
                };
                (parser.stream(head, data, &mut self.warned(Some(id))))
                    .map_err(|e| e.to_string())?
            }
        };
        Ok((id, object))
    }

    /// A stream's `/Length`: an integer, or a reference to one written in
    /// plain view.
    fn length(&mut self, dict: &Dict, id: ObjectId) -> Option<usize> {
        let length = dict.get(b"Length")?;
        let n = match *length.value() {
            Value::Integer(n) => n,
            Value::Ref(to) if to != id && !self.resolving => {
                let Some(&(_, XrefEntry::InUse { offset, .. })) = self.entries.get(&to.0) else {
                    return None;
                };
                self.resolving = true;
                let found = self.indirect(offset, Some(to));
                self.resolving = false;
                found.ok()?.1.as_integer()?
            }
            _ => return None,
        };
        usize::try_from(n).ok()
    }
}

/// An object stream's header, from its decoded `data`: each object's
/// number and where it starts, from the stream's `/First` (ISO 32000-2,
/// 7.5.7). It's read as far as its `/N` says, and no further than the
/// data holds pairs of integers.
fn object_stream_header(object: &Object, data: &[u8]) -> Result<Vec<(u32, usize)>, String> {
    let dict = object.as_dict().ok_or("it isn't a stream")?;
    if dict.get(b"Type").and_then(Object::as_name) != Some(b"ObjStm") {
        return Err("it isn't /Type /ObjStm".to_string());
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
        return Err("its /N or /First isn't a count".to_string());
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
            return Err(format!(
                "its header has {} of its {count} pairs",
                header.len()
            ));
        };
        let number = u32::try_from(number).map_err(|_| "its header numbers no object")?;
        let offset = usize::try_from(offset)
            .ok()
            .and_then(|offset| offset.checked_add(first))
            .ok_or("its header gives an offset that isn't one")?;
        header.push((number, offset));
    }
    Ok(header)
}

/// Whether `o` is a cross-reference stream.
fn is_xref_stream(o: &Object) -> bool {
    matches!(o.value(), Value::Stream(s) if s.dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef"))
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
        Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => {
            let contents = matches!(
                d.get(b"Contents").map(|c| c.value()),
                Some(Value::String(..))
            );
            let typed = (d.get(b"Type"))
                .is_none_or(|t| matches!(t.as_name(), Some(b"Sig" | b"DocTimeStamp")));
            let signature = d.has(b"ByteRange") && contents && typed;
            for entry in d.entries_mut() {
                match signature && entry.key == b"Contents" {
                    true => plain_strings(&mut entry.value),
                    false => plaintext(&mut entry.value, id, handler, faults),
                }
            }
        }
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
            Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => {
                stack.extend(d.entries_mut().map(|entry| &mut entry.value));
            }
            _ => {}
        }
    }
}

/// How many strings in `value` are ciphertext.
fn ciphertexts(value: &Value) -> usize {
    match value {
        Value::String(Str::Encrypted(_), _) => 1,
        Value::Array(a) => a.iter().map(|o| ciphertexts(o.value())).sum(),
        Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => d
            .entries()
            .iter()
            .map(|e| ciphertexts(e.value.value()))
            .sum(),
        _ => 0,
    }
}

/// `object`'s ciphertext strings, each replaced by the string where it is in
/// `plain`, the same object as the decoder read it; how many it couldn't
/// place, which stay ciphertext. Of a key written twice, only the last
/// entry has a place in `plain`, as a reader keeps only that.
fn decrypted(object: &mut Object, plain: &Object) -> usize {
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
        (Value::Dict(d), Value::Dict(p))
        | (Value::Stream(Stream { dict: d, .. }), Value::Stream(Stream { dict: p, .. })) => {
            let mut placed = HashSet::new();
            let mut entries: Vec<&mut Entry> = d.entries_mut().collect();
            entries.reverse();
            (entries.into_iter())
                .map(|entry| match p.get(&entry.key) {
                    Some(p) if placed.insert(entry.key.clone()) => decrypted(&mut entry.value, p),
                    _ => ciphertexts(entry.value.value()),
                })
                .sum()
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict;
    use crate::object::{Data, Form};
    use crate::source::Source;
    use crate::write::{self, Body, Xref};

    /// `o` with every place it was written, and every string's form, left
    /// out.
    fn plain(o: &Object) -> Object {
        Object::new(match o.value() {
            Value::String(s, _) => Value::String(s.clone(), None),
            Value::Array(a) => Value::Array(a.iter().map(plain).collect()),
            Value::Dict(d) => Value::Dict(plain_dict(d)),
            Value::Stream(s) => Value::Stream(Stream {
                dict: plain_dict(&s.dict),
                data: Data::Unplaced,
            }),
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
                        Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => {
                            stack.extend(d.entries_mut().map(|entry| &mut entry.value));
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
        match o.value() {
            Value::Array(a) => a.iter().for_each(|e| placed_as(file, e, strings)),
            Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => {
                for entry in d.entries() {
                    let key_at = entry
                        .key_at
                        .expect("a key from the file says where it was written");
                    assert_eq!(file[key_at.start], b'/');
                    placed_as(file, &entry.value, strings);
                }
            }
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
    fn clean() -> Vec<(u32, Body)> {
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

    fn root() -> Dict {
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
    fn small() -> Vec<u8> {
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
    fn updated_with_padding() -> (Vec<u8>, usize) {
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
        let why = |n: u32| doc.get((n, 0)).err().unwrap_or_default();
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
        let why = doc.get((2, 0)).unwrap_err();
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
                    Err(why) => *bad.entry(class(why)).or_insert(0usize) += 1,
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
    fn xref_row(kind: u8, field: usize, index: u8) -> [u8; 4] {
        let [high, low] = (field as u16).to_be_bytes();
        [kind, high, low, index]
    }

    /// Objects 2 and 3, the page tree and its page, as an object stream
    /// holds them, and where the first begins (`/First`).
    fn objects_2_and_3() -> (Vec<u8>, usize) {
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
    fn compressed(encrypted: bool) -> (Vec<u8>, BTreeMap<ObjectId, Vec<u8>>) {
        let (file, mut decoded) = streams(encrypted, true, b"", None);
        decoded.remove(&(5, 0));
        if !encrypted {
            decoded.clear();
        }
        (file, decoded)
    }

    /// [`compressed`]'s file with both streams' data junk, which only a
    /// decoder asked for every stream ([`read_through`]) decodes, as given.
    fn junk(encrypted: bool) -> (Vec<u8>, BTreeMap<ObjectId, Vec<u8>>) {
        streams(encrypted, false, b"", None)
    }

    /// The file [`compressed`] and [`junk`] write, its streams' data zlib
    /// data when `real` (but an encrypted file's object stream), else
    /// junk, the cross-reference stream's rows followed by `past`, and
    /// with `png`, PNG-predicted (by none) in rows of that many columns,
    /// the last filled out with zeros; and what each decodes to.
    fn streams(
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
    fn unfiltered(encrypted: bool) -> Vec<u8> {
        let (objects, _) = objects_2_and_3();
        filtered(encrypted, "", &objects)
    }

    /// [`unfiltered`]'s file, its object stream's dictionary given
    /// `filter`'s entries, and its data `data`.
    fn filtered(encrypted: bool, filter: &str, data: &[u8]) -> Vec<u8> {
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
        let why = read(&unfiltered(true)).get((2, 0)).unwrap_err();
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
        let why = doc.get((3, 0)).unwrap_err();
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
    fn to_encrypt(extra: &[&[u8]]) -> Vec<(u32, Body)> {
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

    fn settings(
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

    const KINDS: [pdf_crypt::writer::Kind; 5] = [
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
        let why = doc.get((2, 0)).unwrap_err();
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
    fn encryption_updated(file: &[u8], from: &str, to: &str, extra: &[(u32, &str)]) -> Vec<u8> {
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
        let doc = read_as(
            &file,
            Some(&Unaskable),
            None,
            true,
            (STREAM, BUDGET, None),
            &[b"user"],
        );
        assert_eq!(doc.encryption, Encryption::Opened { owner: false });
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.decoded[&(4, 0)], objects);
        assert_eq!(doc.get((2, 0)).unwrap().at.unwrap().stream, Some((4, 0)));
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
        let report = crate::check::check(
            &doc,
            &crate::check::Hold::All(crate::check::Made::Merged),
            &Default::default(),
        );
        let rules: Vec<_> = report
            .findings
            .iter()
            .map(|f| (f.rule, f.key.as_str()))
            .collect();
        assert_eq!(rules, [], "{report}");
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
        let why = doc.get((2, 0)).unwrap_err();
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
        let why = read(&file).get((2, 0)).unwrap_err();
        assert!(why.contains("nothing decodes that"), "{why}");
        // Asked of a decoder: the rows but not the object stream's data.
        let (file, mut decoded) = junk(false);
        decoded.remove(&(4, 0));
        let doc = read_through(&file, &decoded);
        let why = doc.get((2, 0)).unwrap_err();
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
        let why = doc.get((2, 0)).unwrap_err();
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
        let why = doc.get((3, 0)).unwrap_err();
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
