//! File specifications, embedded file streams, and associated files
//! (ISO 32000-2, §7.11 and §14.13, Tables 43–46).
//!
//! [`read_spec`] decodes standalone string or `/Filespec` dictionary file
//! specifications; [`embedded_files`] walks a catalog's `/Names /EmbeddedFiles`
//! name tree; [`associated_files`] reads `/AF` arrays on catalog, page, stream,
//! or annotation dictionaries; and [`write_embedded`] / [`write_embedded_tree`]
//! build `/EmbeddedFile` streams, `/Filespec` dictionaries, and
//! `/EmbeddedFiles` name trees.

use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{
    Dict, Object, ObjectId, Resolve, Span, Str, Streams, Unreadable, Value, Why, dict, text,
};

use crate::date::Date;
use crate::info::{Dated, Len, len};
use crate::phases::phase;
use crate::tree::{self, names, write_names};
use crate::{ByRef, Fault, HOPS, Limits, too_long};

/// Associated-file relationship (`/AFRelationship`, ISO 32000-2 §14.13
/// Table 43).
#[derive(Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AfRelationship {
    /// `/Source`: original content from which the PDF or object was derived.
    Source,
    /// `/Data`: data used to derive visual presentation (e.g. Factur-X / ZUGFeRD XML, table CSV).
    Data,
    /// `/Alternative`: alternative representation of content.
    Alternative,
    /// `/Supplement`: supplemental information when none of `Source`/`Data`/`Alternative` applies.
    Supplement,
    /// `/EncryptedPayload`: encrypted payload document.
    EncryptedPayload,
    /// `/FormData`: interactive form data associated with `/AcroForm`.
    FormData,
    /// `/Schema`: schema definition object.
    Schema,
    /// `/Unspecified`: relationship is unknown or unspecified.
    Unspecified,
    /// Non-standard relationship name.
    Other(Box<[u8]>),
}

impl fmt::Debug for AfRelationship {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source => f.write_str("Source"),
            Self::Data => f.write_str("Data"),
            Self::Alternative => f.write_str("Alternative"),
            Self::Supplement => f.write_str("Supplement"),
            Self::EncryptedPayload => f.write_str("EncryptedPayload"),
            Self::FormData => f.write_str("FormData"),
            Self::Schema => f.write_str("Schema"),
            Self::Unspecified => f.write_str("Unspecified"),
            Self::Other(b) => f.debug_tuple("Other").field(&Len(b.len())).finish(),
        }
    }
}

impl AfRelationship {
    /// Parses a `/AFRelationship` name token.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"Source" => Self::Source,
            b"Data" => Self::Data,
            b"Alternative" => Self::Alternative,
            b"Supplement" => Self::Supplement,
            b"EncryptedPayload" => Self::EncryptedPayload,
            b"FormData" => Self::FormData,
            b"Schema" => Self::Schema,
            b"Unspecified" => Self::Unspecified,
            other => Self::Other(Box::from(other)),
        }
    }

    /// Raw PDF name bytes for this relationship.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Source => b"Source",
            Self::Data => b"Data",
            Self::Alternative => b"Alternative",
            Self::Supplement => b"Supplement",
            Self::EncryptedPayload => b"EncryptedPayload",
            Self::FormData => b"FormData",
            Self::Schema => b"Schema",
            Self::Unspecified => b"Unspecified",
            Self::Other(b) => b,
        }
    }
}

/// Embedded file stream metadata from a `/Filespec`'s `/EF` dictionary
/// (ISO 32000-2 §7.11.4, Tables 44–45).
///
/// Its `Debug` implementation reports `subtype`'s byte length and whether a
/// checksum is present rather than raw MIME or hash bytes.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EmbeddedFile {
    /// Indirect object ID of the `/Type /EmbeddedFile` stream.
    pub stream_id: ObjectId,
    /// Decoded MIME media type from `/Subtype` (e.g. `"text/plain"`, `"application/xml"`).
    pub subtype: Option<String>,
    /// Uncompressed size in bytes from `/Params /Size`, if given and `>= 0`.
    pub size: Option<u64>,
    /// Creation timestamp from `/Params /CreationDate`.
    pub created: Option<Dated>,
    /// Modification timestamp from `/Params /ModDate`.
    pub modified: Option<Dated>,
    /// 16-byte MD5 checksum from `/Params /CheckSum`, if 16 bytes long.
    pub checksum: Option<[u8; 16]>,
}

impl fmt::Debug for EmbeddedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedFile")
            .field("stream_id", &self.stream_id)
            .field("subtype", &len(&self.subtype))
            .field("size", &self.size)
            .field("created", &self.created)
            .field("modified", &self.modified)
            .field("has_checksum", &self.checksum.is_some())
            .finish()
    }
}

impl EmbeddedFile {
    /// Reads and filter-decodes (and decrypts, if applicable) the embedded file
    /// stream payload up to `limit` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] if stream decoding fails or exceeds `limit`
    /// ([`Why::Limit`]).
    pub fn bytes(&self, streams: &impl Streams, limit: usize) -> Result<Vec<u8>, Unreadable> {
        let data = streams.decoded(self.stream_id, limit)?;
        if data.len() > limit {
            return Err(Unreadable::new(
                self.stream_id,
                Why::Limit,
                format!(
                    "{} R exceeds embedded file byte limit ({limit})",
                    self.stream_id
                ),
            ));
        }
        Ok(data.into_owned())
    }
}

/// Decoded PDF file specification (ISO 32000-2 §7.11, Table 43).
///
/// Its `Debug` implementation prints only byte lengths, IDs, and flags—never
/// filenames or descriptions.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileSpec {
    /// Indirect object ID of the `/Filespec` dictionary, if indirect.
    pub id: Option<ObjectId>,
    /// Name-tree key when read from `/Names /EmbeddedFiles`.
    pub tree_key: Option<String>,
    /// Preferred filename (`/UF`, falling back to `/F`, `/DOS`, `/Unix`, `/Mac`).
    pub name: String,
    /// Explicit `/UF` Unicode filename, if present.
    pub unicode_file: Option<String>,
    /// Explicit `/F` filename, if present.
    pub file: Option<String>,
    /// Optional `/Desc` description string.
    pub description: Option<String>,
    /// `/V` volatile file flag (`false` by default).
    pub volatile: bool,
    /// `/AFRelationship` entry, if present.
    pub relationship: Option<AfRelationship>,
    /// Embedded file stream resolved from `/EF` (`/UF` preferred over `/F`,
    /// `/DOS`, `/Unix`, `/Mac`).
    pub embedded: Option<EmbeddedFile>,
}

impl fmt::Debug for FileSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileSpec")
            .field("id", &self.id)
            .field("tree_key", &len(&self.tree_key))
            .field("name", &Len(self.name.len()))
            .field("unicode_file", &len(&self.unicode_file))
            .field("file", &len(&self.file))
            .field("description", &len(&self.description))
            .field("volatile", &self.volatile)
            .field("relationship", &self.relationship)
            .field("embedded", &self.embedded)
            .finish()
    }
}

/// Reads a single file specification from `raw` (a string or `/Filespec`
/// dictionary, ISO 32000-2 §7.11).
///
/// # Errors
///
/// Returns [`Unreadable`] on reader caps, locked objects/strings, or reference
/// loops.
pub fn read_spec<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    raw: &'o Object,
    within: Option<ObjectId>,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<FileSpec>, Unreadable> {
    read_spec_with_key(&objects, raw, within, None, rules, sink)
}

fn read_spec_with_key<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    raw: &'o Object,
    within: Option<ObjectId>,
    tree_key: Option<String>,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<FileSpec>, Unreadable> {
    let Some((obj, id)) = reached(objects, raw, within, sink)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::String(Str::Bytes(bytes), _) => {
            let decoded = text::decode_str(bytes, rules);
            Ok(Some(FileSpec {
                id,
                tree_key,
                name: decoded.clone(),
                unicode_file: None,
                file: Some(decoded),
                description: None,
                volatile: false,
                relationship: None,
                embedded: None,
            }))
        }
        Value::String(Str::Encrypted(_), _) => Err(locked_str(id, "F")),
        Value::Dict(dict) => read_spec_dict(objects, dict, id, tree_key, rules, sink).map(Some),
        _ => {
            say(sink, id, obj.at(), Fault::FileSpecInvalid);
            Ok(None)
        }
    }
}

fn read_spec_dict<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    id: Option<ObjectId>,
    tree_key: Option<String>,
    rules: Rules,
    sink: &mut S,
) -> Result<FileSpec, Unreadable> {
    let unicode_file = text_entry(objects, dict, id, "UF", rules, sink)?;
    let file = text_entry(objects, dict, id, "F", rules, sink)?;
    let fallback = if unicode_file.is_none() && file.is_none() {
        let mut found = None;
        for key in ["DOS", "Unix", "Mac"] {
            if let Some(s) = text_entry(objects, dict, id, key, rules, sink)? {
                found = Some(s);
                break;
            }
        }
        found
    } else {
        None
    };
    let name = unicode_file
        .clone()
        .or_else(|| file.clone())
        .or(fallback)
        .unwrap_or_default();

    let description = text_entry(objects, dict, id, "Desc", rules, sink)?;
    let volatile = match dict.get(b"V") {
        Some(raw) => match reached(objects, raw, id, sink)? {
            Some((o, vid)) => match o.as_bool() {
                Some(b) => b,
                None => {
                    say(sink, vid, o.at(), Fault::FileSpecWrongType { key: "V" });
                    false
                }
            },
            None => false,
        },
        None => false,
    };
    let relationship = match dict.get(b"AFRelationship") {
        Some(raw) => match reached(objects, raw, id, sink)? {
            Some((o, rid)) => match o.as_name() {
                Some(n) => Some(AfRelationship::from_name(n)),
                None => {
                    say(
                        sink,
                        rid,
                        o.at(),
                        Fault::FileSpecWrongType {
                            key: "AFRelationship",
                        },
                    );
                    None
                }
            },
            None => None,
        },
        None => None,
    };

    let embedded = match dict.get(b"EF") {
        Some(raw_ef) => match reached(objects, raw_ef, id, sink)? {
            Some((ef_obj, ef_id)) => match ef_obj.as_dict() {
                Some(ef_dict) => read_ef_dict(objects, dict, ef_dict, ef_id, rules, sink)?,
                None => {
                    say(
                        sink,
                        ef_id,
                        ef_obj.at(),
                        Fault::FileSpecWrongType { key: "EF" },
                    );
                    None
                }
            },
            None => None,
        },
        None => None,
    };

    Ok(FileSpec {
        id,
        tree_key,
        name,
        unicode_file,
        file,
        description,
        volatile,
        relationship,
        embedded,
    })
}

fn read_ef_dict<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    spec_dict: &'o Dict,
    ef_dict: &'o Dict,
    within: Option<ObjectId>,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<EmbeddedFile>, Unreadable> {
    // Per ISO 32000-2 §7.11.4 Table 44 and PDFium's `CPDF_FileSpec::GetFileStream`,
    // each `/EF` key must have a corresponding filename key present on the
    // `/Filespec` dictionary itself.
    let mut target = None;
    for key in ["UF", "F", "DOS", "Unix", "Mac"] {
        let Some(raw) = ef_dict.get(key.as_bytes()) else {
            continue;
        };
        if spec_dict.get(key.as_bytes()).is_none_or(Object::is_null) {
            say(sink, within, raw.at(), Fault::FileSpecInvalid);
            continue;
        }
        let Some((stream_obj, Some(stream_id))) = reached(objects, raw, within, sink)? else {
            if !raw.is_null() {
                say(sink, within, raw.at(), Fault::FileSpecWrongType { key });
            }
            continue;
        };
        if let Some(stream) = stream_obj.as_stream() {
            target = Some((stream_id, &stream.dict));
            break;
        }
        say(
            sink,
            Some(stream_id),
            stream_obj.at(),
            Fault::FileSpecWrongType { key },
        );
    }

    let Some((stream_id, sdict)) = target else {
        return Ok(None);
    };

    let subtype = match sdict.get(b"Subtype") {
        Some(raw) => match reached(objects, raw, Some(stream_id), sink)? {
            Some((o, sid)) => match o.as_name() {
                Some(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
                None => {
                    say(
                        sink,
                        sid,
                        o.at(),
                        Fault::FileSpecWrongType { key: "Subtype" },
                    );
                    None
                }
            },
            None => None,
        },
        None => None,
    };

    let (mut size, mut created, mut modified, mut checksum) = (None, None, None, None);
    if let Some(raw_params) = sdict.get(b"Params")
        && let Some((p_obj, p_id)) = reached(objects, raw_params, Some(stream_id), sink)?
    {
        if let Some(p_dict) = p_obj.as_dict() {
            if let Some(raw_sz) = p_dict.get(b"Size")
                && let Some((sz_obj, sz_id)) = reached(objects, raw_sz, p_id, sink)?
            {
                match sz_obj.as_integer() {
                    Some(n) if n >= 0 => size = u64::try_from(n).ok(),
                    _ => say(
                        sink,
                        sz_id,
                        sz_obj.at(),
                        Fault::FileSpecWrongType { key: "Size" },
                    ),
                }
            }
            created = date_entry(objects, p_dict, p_id, "CreationDate", rules, sink)?;
            modified = date_entry(objects, p_dict, p_id, "ModDate", rules, sink)?;
            if let Some(raw_cs) = p_dict.get(b"CheckSum")
                && let Some((cs_obj, cs_id)) = reached(objects, raw_cs, p_id, sink)?
            {
                match cs_obj.value() {
                    Value::String(Str::Bytes(bytes), _) if bytes.len() == 16 => {
                        let mut arr = [0u8; 16];
                        arr.copy_from_slice(&bytes[..]);
                        checksum = Some(arr);
                    }
                    Value::String(Str::Encrypted(_), _) => {
                        return Err(locked_str(cs_id, "CheckSum"));
                    }
                    _ => say(
                        sink,
                        cs_id,
                        cs_obj.at(),
                        Fault::FileSpecWrongType { key: "CheckSum" },
                    ),
                }
            }
        } else {
            say(
                sink,
                p_id,
                p_obj.at(),
                Fault::FileSpecWrongType { key: "Params" },
            );
        }
    }

    Ok(Some(EmbeddedFile {
        stream_id,
        subtype,
        size,
        created,
        modified,
        checksum,
    }))
}

/// Walks `catalog`'s `/Names /EmbeddedFiles` name tree (ISO 32000-2 §7.7.4).
///
/// Returns an empty `Vec` when `/Names` or `/EmbeddedFiles` is absent.
///
/// # Errors
///
/// Returns [`Unreadable`] on reader caps, locked objects/strings, or reference
/// loops.
pub fn embedded_files<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Vec<FileSpec>, Unreadable> {
    let Some(raw_names) = catalog.get(b"Names") else {
        return Ok(Vec::new());
    };
    let Some((names_obj, names_id)) = reached(&objects, raw_names, None, sink)? else {
        return Ok(Vec::new());
    };
    let Some(names_dict) = names_obj.as_dict() else {
        say(
            sink,
            names_id,
            names_obj.at(),
            Fault::FileSpecWrongType { key: "Names" },
        );
        return Ok(Vec::new());
    };
    let Some(ef_root) = names_dict.get(b"EmbeddedFiles") else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    let mut tree_faults = Vec::new();
    let wants = sink.wants();
    {
        let walk = names(
            ByRef(&objects),
            ef_root,
            limits,
            TreeCollector {
                wants,
                out: &mut tree_faults,
            },
        );
        for item in walk {
            let (key_bytes, val) = item?;
            let key_str = text::decode_str(key_bytes, rules);
            if let Some(spec) =
                read_spec_with_key(&objects, val, names_id, Some(key_str), rules, sink)?
            {
                out.push(spec);
            }
        }
    }
    if wants {
        for (place, fault) in tree_faults {
            sink.fault(place, fault);
        }
    }
    phase!(DEBUG, "files::embedded_files", count = out.len());
    Ok(out)
}

/// Reads the `/AF` Associated Files array on `dict` (a catalog, page, stream,
/// or annotation dictionary, ISO 32000-2 §14.13).
///
/// # Errors
///
/// Returns [`Unreadable`] on reader caps, locked objects/strings, or reference
/// loops.
pub fn associated_files<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    rules: Rules,
    sink: &mut S,
) -> Result<Vec<FileSpec>, Unreadable> {
    let Some(raw_af) = dict.get(b"AF") else {
        return Ok(Vec::new());
    };
    let Some((af_obj, af_id)) = reached(&objects, raw_af, within, sink)? else {
        return Ok(Vec::new());
    };
    // ISO 32000-2 §14.13 requires an array of file specification dictionaries;
    // under `Rules::Pdfium` tolerate a single direct/indirect dictionary too.
    match af_obj.value() {
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if let Some(spec) = read_spec_with_key(&objects, item, af_id, None, rules, sink)? {
                    out.push(spec);
                }
            }
            Ok(out)
        }
        Value::Dict(single) if matches!(rules, Rules::Pdfium) => {
            say(
                sink,
                af_id,
                af_obj.at(),
                Fault::FileSpecWrongType { key: "AF" },
            );
            let spec = read_spec_dict(&objects, single, af_id, None, rules, sink)?;
            Ok(vec![spec])
        }
        _ => {
            say(
                sink,
                af_id,
                af_obj.at(),
                Fault::FileSpecWrongType { key: "AF" },
            );
            Ok(Vec::new())
        }
    }
}

struct TreeCollector<'a> {
    wants: bool,
    out: &'a mut Vec<(Place, Fault)>,
}

impl Sink<Fault> for TreeCollector<'_> {
    fn wants(&self) -> bool {
        self.wants
    }

    fn fault(&mut self, place: Place, fault: Fault) {
        self.out.push((place, fault));
    }
}

fn text_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<String>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = reached(objects, raw, within, sink)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::String(Str::Bytes(bytes), _) => Ok(Some(text::decode_str(bytes, rules))),
        Value::String(Str::Encrypted(_), _) => Err(locked_str(id, key)),
        _ => {
            say(sink, id, obj.at(), Fault::FileSpecWrongType { key });
            Ok(None)
        }
    }
}

fn date_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<Dated>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = reached(objects, raw, within, sink)? else {
        return Ok(None);
    };
    let s = match obj.value() {
        Value::String(Str::Bytes(bytes), _) => text::decode_str(bytes, rules),
        Value::String(Str::Encrypted(_), _) => return Err(locked_str(id, key)),
        _ => {
            say(sink, id, obj.at(), Fault::FileSpecWrongType { key });
            return Ok(None);
        }
    };
    Ok(Some(match Date::read(&s) {
        Ok(d) => Dated::Date(d),
        Err(error) => {
            say(sink, id, obj.at(), Fault::FileSpecDate { key, error });
            Dated::Text(s)
        }
    }))
}

fn locked_str(id: Option<ObjectId>, key: &'static str) -> Unreadable {
    let oid = id.unwrap_or(ObjectId::new(0, 0));
    Unreadable::new(
        oid,
        Why::Locked,
        format!("{oid} R holds /{key} as ciphertext: it takes a password"),
    )
}

fn say<S: Sink<Fault> + ?Sized>(
    sink: &mut S,
    id: Option<ObjectId>,
    at: Option<Span>,
    fault: Fault,
) {
    if sink.wants() {
        sink.fault(Place::new(id, at), fault);
    }
}

fn reached<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    value: &'o Object,
    within: Option<ObjectId>,
    sink: &mut S,
) -> Result<Option<(&'o Object, Option<ObjectId>)>, Unreadable> {
    let (mut at, mut last) = (value, None);
    for _ in 0..HOPS {
        match at.value() {
            Value::Ref(id) => {
                last = Some(*id);
                match objects.try_resolve(*id) {
                    Ok(Some(obj)) => at = obj,
                    Ok(None) => return Ok(None),
                    Err(u) if u.why == Why::Damaged => {
                        say(sink, within, value.at(), Fault::Damaged { id: *id });
                        return Ok(None);
                    }
                    Err(u) => return Err(u),
                }
            }
            Value::Null => return Ok(None),
            _ => return Ok(Some((at, last.or(within)))),
        }
    }
    Err(too_long(last.unwrap_or(ObjectId::new(0, 0))))
}

/// Builder describing an embedded file to construct with [`write_embedded`].
///
/// Its `Debug` implementation reports lengths rather than filenames or descriptions.
#[derive(Clone, PartialEq, Eq)]
pub struct NewEmbeddedFile {
    /// Filename (`/UF` and ASCII-fallback `/F`).
    pub name: String,
    /// Optional MIME media type (`/Subtype`, e.g. `"application/xml"`).
    pub subtype: Option<String>,
    /// Optional `/Desc` description.
    pub description: Option<String>,
    /// Optional `/AFRelationship` (`Unspecified` recommended for PDF/A-3 and PDF 2.0 when unknown).
    pub relationship: Option<AfRelationship>,
    /// Optional creation date (`/Params /CreationDate`).
    pub created: Option<Date>,
    /// Optional modification date (`/Params /ModDate`).
    pub modified: Option<Date>,
    /// Uncompressed payload length for `/Params /Size`.
    pub size: usize,
}

impl fmt::Debug for NewEmbeddedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewEmbeddedFile")
            .field("name", &Len(self.name.len()))
            .field("subtype", &len(&self.subtype))
            .field("description", &len(&self.description))
            .field("relationship", &self.relationship)
            .field("created", &self.created)
            .field("modified", &self.modified)
            .field("size", &self.size)
            .finish()
    }
}

impl NewEmbeddedFile {
    /// Creates an embedded file descriptor for `name` and payload `size`.
    #[must_use]
    pub fn new(name: impl Into<String>, size: usize) -> Self {
        Self {
            name: name.into(),
            subtype: None,
            description: None,
            relationship: None,
            created: None,
            modified: None,
            size,
        }
    }

    /// Sets the MIME media type (`/Subtype`).
    #[must_use]
    pub fn with_subtype(mut self, subtype: impl Into<String>) -> Self {
        self.subtype = Some(subtype.into());
        self
    }

    /// Sets `/Desc`.
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Sets `/AFRelationship`.
    #[must_use]
    pub fn with_relationship(mut self, rel: AfRelationship) -> Self {
        self.relationship = Some(rel);
        self
    }

    /// Sets `/Params /ModDate`.
    #[must_use]
    pub fn with_modified(mut self, date: Date) -> Self {
        self.modified = Some(date);
        self
    }
}

/// Constructed `/EmbeddedFile` stream dictionary and `/Filespec` dictionary.
#[derive(Debug, Clone, PartialEq)]
pub struct WrittenEmbeddedFile {
    /// `/Type /EmbeddedFile` stream dictionary (pair with caller's payload bytes).
    pub stream_dict: Dict,
    /// `/Type /Filespec` dictionary referencing `stream_id`.
    pub filespec_dict: Dict,
}

/// Builds the `/Type /EmbeddedFile` stream dictionary and `/Type /Filespec`
/// dictionary for `spec`, binding the embedded stream at `stream_id`.
#[must_use]
pub fn write_embedded(spec: &NewEmbeddedFile, stream_id: ObjectId) -> WrittenEmbeddedFile {
    let mut params = dict! {
        "Size" => i64::try_from(spec.size).unwrap_or(i64::MAX),
    };
    if let Some(c) = &spec.created {
        params.set("CreationDate", Object::text(&c.write()));
    }
    if let Some(m) = &spec.modified {
        params.set("ModDate", Object::text(&m.write()));
    }

    let mut stream_dict = dict! {
        "Type" => Object::name("EmbeddedFile"),
        "Params" => params,
    };
    if let Some(sub) = &spec.subtype {
        stream_dict.set("Subtype", Object::name(sub.as_bytes()));
    }

    let ascii_f: String = spec
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut filespec_dict = dict! {
        "Type" => Object::name("Filespec"),
        "F" => Object::text(&ascii_f),
        "UF" => Object::text(&spec.name),
        "EF" => dict! {
            "F" => stream_id,
            "UF" => stream_id,
        },
    };
    if let Some(desc) = &spec.description {
        filespec_dict.set("Desc", Object::text(desc));
    }
    if let Some(rel) = &spec.relationship {
        filespec_dict.set("AFRelationship", Object::name(rel.as_bytes()));
    }

    WrittenEmbeddedFile {
        stream_dict,
        filespec_dict,
    }
}

/// Builds a flat `/EmbeddedFiles` name-tree root dictionary from `(name, filespec_id)`
/// entries, encoding each key via ISO 32000-2 §7.9.2.2 text string bytes.
///
/// # Errors
///
/// Returns [`tree::WriteError`] if two entries encode to the same key bytes.
pub fn write_embedded_tree<I, S>(entries: I) -> Result<Dict, tree::WriteError>
where
    I: IntoIterator<Item = (S, ObjectId)>,
    S: AsRef<str>,
{
    write_names(
        entries
            .into_iter()
            .map(|(k, id)| (text::encode(k.as_ref()), Object::from(id))),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams, dict};

    use super::{
        AfRelationship, NewEmbeddedFile, associated_files, embedded_files, write_embedded,
        write_embedded_tree,
    };
    use crate::Limits;
    use crate::date::Date;

    #[test]
    fn round_trip_embedded_and_associated_files_with_debug_hygiene() {
        let stream_id = ObjectId::new(10, 0);
        let spec_id = ObjectId::new(11, 0);
        let payload = b"<invoice>secret</invoice>";
        let desc = NewEmbeddedFile::new("facture-2026.xml", payload.len())
            .with_subtype("text/xml")
            .with_description("Private customer invoice")
            .with_relationship(AfRelationship::Data)
            .with_modified(Date::read("D:20261115120000Z").unwrap());
        let written = write_embedded(&desc, stream_id);
        let ef_tree = write_embedded_tree([("facture-2026.xml", spec_id)]).unwrap();

        let objects = BTreeMap::from([
            (stream_id, Object::stream(written.stream_dict)),
            (spec_id, Object::from(written.filespec_dict)),
        ]);
        let catalog = dict! {
            "Names" => dict! { "EmbeddedFiles" => ef_tree },
            "AF" => vec![Object::from(spec_id)],
        };

        let mut faults = Vec::new();
        let list = embedded_files(
            &objects,
            &catalog,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap();
        assert!(faults.is_empty(), "{faults:?}");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "facture-2026.xml");
        assert_eq!(
            list[0].description.as_deref(),
            Some("Private customer invoice")
        );
        assert_eq!(list[0].relationship, Some(AfRelationship::Data));
        let emb = list[0].embedded.as_ref().unwrap();
        assert_eq!(emb.subtype.as_deref(), Some("text/xml"));
        assert_eq!(emb.size, Some(payload.len() as u64));

        let streams = WithStreams::new(&objects, |id: ObjectId, _: usize| {
            if id == stream_id {
                Ok(payload.to_vec())
            } else {
                Err(Unreadable::new(id, Why::Damaged, "missing"))
            }
        });
        assert_eq!(emb.bytes(&streams, 1024).unwrap(), payload);

        let af = associated_files(&objects, &catalog, None, Rules::Iso, &mut faults).unwrap();
        assert_eq!(af.len(), 1);
        assert_eq!(af[0].id, Some(spec_id));

        let dbg = format!("{list:?} {desc:?}");
        assert!(!dbg.contains("facture"), "{dbg}");
        assert!(!dbg.contains("Private"), "{dbg}");
    }
}
