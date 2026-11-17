//! Standard Dublin Core, XMP Basic, Adobe PDF, and PDF/X + PDF/E properties in
//! an XMP packet, plus `/Info` synchronization helpers (ISO 32000-2 §14.3.2–14.3.3,
//! ISO 16684-1).

use std::fmt;
use std::fmt::Write as _;

use pdf_objects::fault::Sink;
use pdf_objects::{Dict, ObjectId, Resolve, Streams, Why};
use quick_xml::NsReader;
use quick_xml::errors::Error as XmlError;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, NamespaceError, ResolveResult};

use super::{
    Claim, Claims, Error, PDFA, PDFUA, PDFVT, WriteError, XmpWhy, bound, character, escaped,
    in_xml, not_utf8, stream_of, tell,
};
use crate::date::{Date, Offset};
use crate::info::{Dated, Info, Trapped, len};
use crate::phases::phase;
use crate::{Fault, Limits};

const RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML_NS: &[u8] = b"http://www.w3.org/XML/1998/namespace";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const XMP_BASIC: &str = "http://ns.adobe.com/xap/1.0/";
const ADOBE_PDF: &str = "http://ns.adobe.com/pdf/1.3/";
const PDFX_13: &str = "http://ns.adobe.com/pdfx/1.3/";
const PDFX_ID: &str = "http://www.npes.org/pdfx/ns/id/";
const PDFE_ID: &str = "http://www.aiim.org/pdfe/ns/id/";

/// Standard document metadata and conformance claims decoded from an XMP packet.
///
/// Its `Debug` implementation prints only byte lengths and item counts—never
/// metadata strings, which may come from an encrypted document.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Metadata {
    /// PDF/A, PDF/UA, and PDF/VT claims (matching [`super::read`]).
    pub claims: Claims,
    /// `pdfx:GTS_PDFXVersion` or `pdfxid:GTS_PDFXVersion`, if present.
    pub pdfx_version: Option<String>,
    /// `pdfeid:GTS_PDFEVersion`, if present.
    pub pdfe_version: Option<String>,
    /// `dc:title` (`x-default` preferred when written as `rdf:Alt`).
    pub title: Option<String>,
    /// `dc:creator` ordered sequence (`rdf:Seq` items or scalar fallback).
    pub creators: Vec<String>,
    /// `dc:description` (`x-default` preferred when written as `rdf:Alt`).
    pub description: Option<String>,
    /// `dc:subject` unordered bag (`rdf:Bag` items).
    pub subjects: Vec<String>,
    /// `dc:format` (typically `"application/pdf"`).
    pub format: Option<String>,
    /// `xmp:CreatorTool` (mirrors `/Info /Creator`).
    pub creator_tool: Option<String>,
    /// `xmp:CreateDate` ISO 8601 string (mirrors `/Info /CreationDate`).
    pub create_date: Option<String>,
    /// `xmp:ModifyDate` ISO 8601 string (mirrors `/Info /ModifyDate` / `/ModDate`).
    pub modify_date: Option<String>,
    /// `xmp:MetadataDate` ISO 8601 string.
    pub metadata_date: Option<String>,
    /// `pdf:Keywords` (mirrors `/Info /Keywords`).
    pub keywords: Option<String>,
    /// `pdf:Producer` (mirrors `/Info /Producer`).
    pub producer: Option<String>,
    /// `pdf:Trapped` (`"True"`, `"False"`, or `"Unknown"`).
    pub trapped: Option<String>,
}

impl fmt::Debug for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metadata")
            .field("claims", &self.claims)
            .field("pdfx_version", &len(&self.pdfx_version))
            .field("pdfe_version", &len(&self.pdfe_version))
            .field("title", &len(&self.title))
            .field("creators", &self.creators.len())
            .field("description", &len(&self.description))
            .field("subjects", &self.subjects.len())
            .field("format", &len(&self.format))
            .field("creator_tool", &len(&self.creator_tool))
            .field("create_date", &len(&self.create_date))
            .field("modify_date", &len(&self.modify_date))
            .field("metadata_date", &len(&self.metadata_date))
            .field("keywords", &len(&self.keywords))
            .field("producer", &len(&self.producer))
            .field("trapped", &len(&self.trapped))
            .finish()
    }
}

impl Metadata {
    /// Builds an XMP [`Metadata`] record mirroring `info` (converting PDF `D:...`
    /// dates to ISO 8601 strings where valid).
    #[must_use]
    pub fn from_info(info: &Info) -> Self {
        let mut meta = Self {
            title: info.title.clone(),
            creators: info.author.iter().cloned().collect(),
            description: info.subject.clone(),
            creator_tool: info.creator.clone(),
            keywords: info.keywords.clone(),
            producer: info.producer.clone(),
            ..Self::default()
        };
        if let Some(Dated::Date(d)) = &info.created {
            meta.create_date = Some(date_to_xmp(d));
        }
        if let Some(Dated::Date(d)) = &info.modified {
            meta.modify_date = Some(date_to_xmp(d));
        }
        if let Some(trapped) = info.trapped {
            meta.trapped = Some(
                match trapped {
                    Trapped::True => "True",
                    Trapped::False => "False",
                    Trapped::Unknown => "Unknown",
                }
                .to_string(),
            );
        }
        meta
    }
}

/// Formats a PDF [`Date`] as a W3C / ISO 8601 XMP date string
/// (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`, or `YYYY-MM-DDThh:mm:ss[Z|±hh:mm]`).
#[must_use]
pub fn date_to_xmp(date: &Date) -> String {
    let mut out = format!("{:04}", date.year());
    let Some(month) = date.month() else {
        return out;
    };
    let _ = write!(out, "-{month:02}");
    let Some(day) = date.day() else {
        return out;
    };
    let _ = write!(out, "-{day:02}");
    let Some(hour) = date.hour() else {
        return out;
    };
    let min = date.minute_or_default();
    let _ = write!(out, "T{hour:02}:{min:02}");
    if let Some(sec) = date.second() {
        let _ = write!(out, ":{sec:02}");
    } else if date.offset().is_some() {
        out.push_str(":00");
    }
    match date.offset() {
        None => {}
        Some(Offset::Utc) => out.push('Z'),
        Some(Offset::Ahead { hours, minutes }) => {
            let _ = write!(out, "+{hours:02}:{minutes:02}");
        }
        Some(Offset::Behind { hours, minutes }) => {
            let _ = write!(out, "-{hours:02}:{minutes:02}");
        }
    }
    out
}

/// Parses a W3C / ISO 8601 XMP date string into a [`Date`] (discarding
/// fractional seconds if present).
#[must_use]
pub fn xmp_to_date(s: &str) -> Option<Date> {
    let bytes = s.trim_ascii().as_bytes();
    if bytes.len() < 4 || !bytes[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let year: u16 = std::str::from_utf8(&bytes[..4]).ok()?.parse().ok()?;
    let mut d = Date::new(year).ok()?;
    let mut rest = &bytes[4..];
    if rest.is_empty() {
        return Some(d);
    }

    let take2 = |r: &mut &[u8]| -> Option<u8> {
        let (two, tail) = r.split_at_checked(2)?;
        if !two.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *r = tail;
        std::str::from_utf8(two).ok()?.parse().ok()
    };

    rest = rest.strip_prefix(b"-")?;
    d = d.with_month(take2(&mut rest)?).ok()?;
    if rest.is_empty() {
        return Some(d);
    }

    rest = rest.strip_prefix(b"-")?;
    d = d.with_day(take2(&mut rest)?).ok()?;
    if rest.is_empty() {
        return Some(d);
    }

    rest = rest
        .strip_prefix(b"T")
        .or_else(|| rest.strip_prefix(b"t"))?;
    d = d.with_hour(take2(&mut rest)?).ok()?;
    rest = rest.strip_prefix(b":")?;
    d = d.with_minute(take2(&mut rest)?).ok()?;

    if let Some(after_colon) = rest.strip_prefix(b":") {
        rest = after_colon;
        d = d.with_second(take2(&mut rest)?).ok()?;
        // Skip optional `.sss` fractional seconds.
        if let Some(frac) = rest.strip_prefix(b".") {
            let digits = frac.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits == 0 {
                return None;
            }
            rest = &frac[digits..];
        }
    } else if !rest.is_empty() {
        d = d.with_second(0).ok()?;
    }

    if rest.is_empty() {
        return Some(d);
    }
    if rest == b"Z" || rest == b"z" {
        return d.with_offset(Offset::Utc).ok();
    }
    let (ahead, mut tz) = if let Some(t) = rest.strip_prefix(b"+") {
        (true, t)
    } else {
        let t = rest.strip_prefix(b"-")?;
        (false, t)
    };
    let hours = take2(&mut tz)?;
    let minutes = if tz.is_empty() {
        0
    } else {
        tz = tz.strip_prefix(b":").unwrap_or(tz);
        take2(&mut tz)?
    };
    if !tz.is_empty() {
        return None;
    }
    let offset = if ahead {
        Offset::Ahead { hours, minutes }
    } else {
        Offset::Behind { hours, minutes }
    };
    d.with_offset(offset).ok()
}

/// Which `/Info` <-> XMP field pair disagreed in [`check_info_sync`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InfoField {
    /// `/Title` vs `dc:title`.
    Title,
    /// `/Author` vs `dc:creator`.
    Author,
    /// `/Subject` vs `dc:description`.
    Subject,
    /// `/Keywords` vs `pdf:Keywords`.
    Keywords,
    /// `/Creator` vs `xmp:CreatorTool`.
    Creator,
    /// `/Producer` vs `pdf:Producer`.
    Producer,
    /// `/CreationDate` vs `xmp:CreateDate`.
    CreationDate,
    /// `/ModDate` vs `xmp:ModifyDate`.
    ModDate,
    /// `/Trapped` vs `pdf:Trapped`.
    Trapped,
}

/// Checks whether every present `/Info` entry matches its corresponding XMP
/// property in `meta` (as required by PDF/A-1..3 when `/Info` is present).
///
/// Returns the list of fields that are present in `/Info` and either missing or
/// different in `meta`.
#[must_use]
pub fn check_info_sync(meta: &Metadata, info: &Info) -> Vec<InfoField> {
    let mut out = Vec::new();
    if let Some(t) = &info.title
        && meta.title.as_ref() != Some(t)
    {
        out.push(InfoField::Title);
    }
    if let Some(a) = &info.author {
        let joined = meta.creators.join("; ");
        let matches_single = meta.creators.len() == 1 && &meta.creators[0] == a;
        let matches_joined = !meta.creators.is_empty() && &joined == a;
        if !matches_single && !matches_joined {
            out.push(InfoField::Author);
        }
    }
    if let Some(s) = &info.subject
        && meta.description.as_ref() != Some(s)
    {
        out.push(InfoField::Subject);
    }
    if let Some(k) = &info.keywords
        && meta.keywords.as_ref() != Some(k)
    {
        out.push(InfoField::Keywords);
    }
    if let Some(c) = &info.creator
        && meta.creator_tool.as_ref() != Some(c)
    {
        out.push(InfoField::Creator);
    }
    if let Some(p) = &info.producer
        && meta.producer.as_ref() != Some(p)
    {
        out.push(InfoField::Producer);
    }
    if let Some(created) = &info.created
        && !dates_match(created, meta.create_date.as_deref())
    {
        out.push(InfoField::CreationDate);
    }
    if let Some(modified) = &info.modified
        && !dates_match(modified, meta.modify_date.as_deref())
    {
        out.push(InfoField::ModDate);
    }
    if let Some(trapped) = info.trapped {
        let want = match trapped {
            Trapped::True => "True",
            Trapped::False => "False",
            Trapped::Unknown => "Unknown",
        };
        if meta.trapped.as_deref() != Some(want) {
            out.push(InfoField::Trapped);
        }
    }
    out
}

fn dates_match(info_date: &Dated, xmp_str: Option<&str>) -> bool {
    let Some(xmp_str) = xmp_str else {
        return false;
    };
    let Dated::Date(id) = info_date else {
        return false;
    };
    let Some(xd) = xmp_to_date(xmp_str) else {
        return false;
    };
    id.year() == xd.year()
        && id.month_or_default() == xd.month_or_default()
        && id.day_or_default() == xd.day_or_default()
        && id.hour_or_default() == xd.hour_or_default()
        && id.minute_or_default() == xd.minute_or_default()
        && id.second_or_default() == xd.second_or_default()
        && offsets_equal(id.offset(), xd.offset())
}

fn offsets_equal(a: Option<Offset>, b: Option<Offset>) -> bool {
    let norm = |o: Option<Offset>| match o {
        Some(Offset::Ahead {
            hours: 0,
            minutes: 0,
        })
        | Some(Offset::Behind {
            hours: 0,
            minutes: 0,
        }) => Some(Offset::Utc),
        other => other,
    };
    norm(a) == norm(b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropKind {
    Claim(Claim),
    PdfxVersion,
    PdfeVersion,
    DcTitle,
    DcCreator,
    DcDescription,
    DcSubject,
    DcFormat,
    XmpCreatorTool,
    XmpCreateDate,
    XmpModifyDate,
    XmpMetadataDate,
    PdfKeywords,
    PdfProducer,
    PdfTrapped,
}

impl PropKind {
    fn of(ns: ResolveResult<'_>, local_name: quick_xml::name::LocalName<'_>) -> Option<Self> {
        let ResolveResult::Bound(Namespace(ns_bytes)) = ns else {
            return None;
        };
        if let Some(c) = Claim::of(ResolveResult::Bound(Namespace(ns_bytes)), local_name) {
            return Some(Self::Claim(c));
        }
        let local = local_name.as_ref();
        match (ns_bytes, local) {
            (ns, b"title") if ns == DC.as_bytes() => Some(Self::DcTitle),
            (ns, b"creator") if ns == DC.as_bytes() => Some(Self::DcCreator),
            (ns, b"description") if ns == DC.as_bytes() => Some(Self::DcDescription),
            (ns, b"subject") if ns == DC.as_bytes() => Some(Self::DcSubject),
            (ns, b"format") if ns == DC.as_bytes() => Some(Self::DcFormat),
            (ns, b"CreatorTool") if ns == XMP_BASIC.as_bytes() => Some(Self::XmpCreatorTool),
            (ns, b"CreateDate") if ns == XMP_BASIC.as_bytes() => Some(Self::XmpCreateDate),
            (ns, b"ModifyDate") if ns == XMP_BASIC.as_bytes() => Some(Self::XmpModifyDate),
            (ns, b"MetadataDate") if ns == XMP_BASIC.as_bytes() => Some(Self::XmpMetadataDate),
            (ns, b"Keywords") if ns == ADOBE_PDF.as_bytes() => Some(Self::PdfKeywords),
            (ns, b"Producer") if ns == ADOBE_PDF.as_bytes() => Some(Self::PdfProducer),
            (ns, b"Trapped") if ns == ADOBE_PDF.as_bytes() => Some(Self::PdfTrapped),
            (ns, b"GTS_PDFXVersion") if ns == PDFX_13.as_bytes() || ns == PDFX_ID.as_bytes() => {
                Some(Self::PdfxVersion)
            }
            (ns, b"GTS_PDFEVersion") if ns == PDFE_ID.as_bytes() => Some(Self::PdfeVersion),
            _ => None,
        }
    }
}

struct OpenProp {
    kind: PropKind,
    level: usize,
    text: String,
    has_child_elem: bool,
    lost: bool,
    start: usize,
    items: Vec<(Option<String>, String)>,
}

struct OpenLi {
    level: usize,
    lang: Option<String>,
    text: String,
    has_child_elem: bool,
    lost: bool,
}

/// Reads standard Dublin Core, XMP Basic, Adobe PDF, and ISO conformance
/// properties from `xml` under `limits`.
///
/// # Errors
///
/// Returns [`Error`] on packets exceeding `limits` or malformed XML, preserving
/// claims read up to the failure offset in [`Error::read`].
#[allow(clippy::result_large_err)]
pub fn read_metadata<S: Sink<Fault> + ?Sized>(
    xml: &[u8],
    stream: Option<ObjectId>,
    limits: &Limits,
    sink: &mut S,
) -> Result<Metadata, Error> {
    phase!(DEBUG, "read_metadata", bytes = xml.len());
    let mut meta = Metadata::default();
    if let Some(end) = not_utf8(xml) {
        tell(sink, stream, 0, end, Fault::XmpNotUtf8);
        return Ok(meta);
    }
    let cut = xml.len() > limits.xmp_bytes;
    let data = &xml[..xml.len().min(limits.xmp_bytes)];
    let mut reader = NsReader::from_reader(data);
    reader.config_mut().allow_dangling_amp = true;
    (reader.resolver_mut()).set_max_declarations_per_element(limits.xmp_bindings);
    let depth = limits.xmp_depth.min(usize::from(u16::MAX) - 1);

    let (mut levels, mut bindings): (Vec<usize>, usize) = (Vec::new(), 0);
    let mut open_props: Vec<OpenProp> = Vec::new();
    let mut open_li: Option<OpenLi> = None;

    loop {
        let start = reader.buffer_position() as usize;
        let stop = |read: Claims, at, why| Err(Error { read, at, why });
        let event = match reader.read_event() {
            Ok(ev) => ev,
            Err(XmlError::Syntax(_)) if cut => {
                return stop(meta.claims, data.len(), XmpWhy::TooLong);
            }
            Err(XmlError::Namespace(NamespaceError::TooManyDeclarations(_))) => {
                return stop(meta.claims, start, XmpWhy::TooManyBindings);
            }
            Err(err) => {
                let at = reader.error_position() as usize;
                return stop(meta.claims, at, XmpWhy::Xml(err.to_string()));
            }
        };
        let end = reader.buffer_position() as usize;

        match &event {
            Event::Start(tag) | Event::Empty(tag) => {
                if levels.len() >= depth {
                    return stop(meta.claims, start, XmpWhy::TooDeep);
                }
                let b = bound(tag);
                if bindings + b > limits.xmp_bindings {
                    return stop(meta.claims, start, XmpWhy::TooManyBindings);
                }

                let mut xml_lang = None;
                for attr in tag.attributes() {
                    let Ok(attr) = attr else {
                        tell(sink, stream, start, end, Fault::XmpAttribute);
                        continue;
                    };
                    let (ns, name) = reader.resolver().resolve_attribute(attr.key);
                    if matches!(ns, ResolveResult::Bound(Namespace(XML_NS)))
                        && name.as_ref() == b"lang"
                        && let Ok(v) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    {
                        xml_lang = Some(v.into_owned());
                    } else if attr.key.as_ref() == b"xml:lang"
                        && let Ok(v) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    {
                        // `xml:` prefix is implicitly bound in XML 1.0.
                        xml_lang = Some(v.into_owned());
                    }
                    if let Some(kind) = PropKind::of(ns, name)
                        && !prop_already_set(&meta, kind)
                    {
                        match attr.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
                            Ok(val) => apply_scalar(&mut meta, kind, &val),
                            Err(_) => tell(sink, stream, start, end, Fault::XmpValue),
                        }
                    }
                }

                if let Some(li) = open_li.as_mut() {
                    li.has_child_elem = true;
                } else if let Some(inner) = open_props.last_mut() {
                    inner.has_child_elem = true;
                }

                let (ns, elem_name) = reader.resolver().resolve_element(tag.name());
                let is_rdf_li = matches!(ns, ResolveResult::Bound(Namespace(RDF)))
                    && elem_name.as_ref() == b"li";

                if matches!(event, Event::Start(_)) {
                    levels.push(b);
                    bindings += b;
                    if is_rdf_li && !open_props.is_empty() && open_li.is_none() {
                        open_li = Some(OpenLi {
                            level: levels.len(),
                            lang: xml_lang,
                            text: String::new(),
                            has_child_elem: false,
                            lost: false,
                        });
                    } else if let Some(kind) =
                        PropKind::of(ns, elem_name).filter(|&k| !prop_already_set(&meta, k))
                    {
                        open_props.push(OpenProp {
                            kind,
                            level: levels.len(),
                            text: String::new(),
                            has_child_elem: false,
                            lost: false,
                            start,
                            items: Vec::new(),
                        });
                    }
                } else if is_rdf_li {
                    if let Some(prop) = open_props.last_mut() {
                        prop.items.push((xml_lang, String::new()));
                    }
                } else if let Some(kind) =
                    PropKind::of(ns, elem_name).filter(|&k| !prop_already_set(&meta, k))
                {
                    apply_scalar(&mut meta, kind, "");
                }
            }
            Event::Text(t) => {
                let cur_level = levels.len();
                if let Some(li) = open_li
                    .as_mut()
                    .filter(|li| li.level == cur_level && !li.lost)
                {
                    if t.starts_with(b"&") {
                        li.lost = true;
                    } else if let Ok(s) = t.xml10_content() {
                        li.text.push_str(&s);
                    } else {
                        li.lost = true;
                    }
                } else if let Some(p) = open_props
                    .last_mut()
                    .filter(|p| p.level == cur_level && !p.lost)
                {
                    let is_claim = matches!(p.kind, PropKind::Claim(_));
                    if t.starts_with(b"&") {
                        p.lost = true;
                        if is_claim {
                            tell(sink, stream, start, end, Fault::XmpValue);
                        }
                    } else if let Ok(s) = t.xml10_content() {
                        p.text.push_str(&s);
                    } else {
                        p.lost = true;
                        if is_claim {
                            tell(sink, stream, start, end, Fault::XmpValue);
                        }
                    }
                }
            }
            Event::CData(t) => {
                let cur_level = levels.len();
                if let Some(li) = open_li
                    .as_mut()
                    .filter(|li| li.level == cur_level && !li.lost)
                {
                    if let Ok(s) = t.xml10_content() {
                        li.text.push_str(&s);
                    } else {
                        li.lost = true;
                    }
                } else if let Some(p) = open_props
                    .last_mut()
                    .filter(|p| p.level == cur_level && !p.lost)
                {
                    if let Ok(s) = t.xml10_content() {
                        p.text.push_str(&s);
                    } else {
                        p.lost = true;
                        if matches!(p.kind, PropKind::Claim(_)) {
                            tell(sink, stream, start, end, Fault::XmpValue);
                        }
                    }
                }
            }
            Event::GeneralRef(r) => {
                let cur_level = levels.len();
                if let Some(li) = open_li
                    .as_mut()
                    .filter(|li| li.level == cur_level && !li.lost)
                {
                    match character(r) {
                        Some(ch) => li.text.push_str(&ch),
                        None => li.lost = true,
                    }
                } else if let Some(p) = open_props
                    .last_mut()
                    .filter(|p| p.level == cur_level && !p.lost)
                {
                    match character(r) {
                        Some(ch) => p.text.push_str(&ch),
                        None => {
                            p.lost = true;
                            if matches!(p.kind, PropKind::Claim(_)) {
                                tell(sink, stream, start, end, Fault::XmpValue);
                            }
                        }
                    }
                }
            }
            Event::End(_) => {
                let level = levels.len();
                bindings -= levels.pop().unwrap_or(0);
                if open_li.as_ref().is_some_and(|li| li.level == level) {
                    let li = open_li.take().unwrap();
                    if !li.lost
                        && !li.has_child_elem
                        && let Some(prop) = open_props.last_mut()
                    {
                        prop.items.push((li.lang, li.text.trim_ascii().to_string()));
                    }
                } else if let Some(prop) = open_props.pop_if(|p| p.level == level) {
                    finish_prop(&mut meta, prop, sink, stream, end);
                }
            }
            Event::Eof if cut => return stop(meta.claims, data.len(), XmpWhy::TooLong),
            Event::Eof => return Ok(meta),
            _ => {}
        }
    }
}

fn prop_already_set(meta: &Metadata, kind: PropKind) -> bool {
    match kind {
        PropKind::Claim(c) => meta.claims.value(c).is_some(),
        PropKind::PdfxVersion => meta.pdfx_version.is_some(),
        PropKind::PdfeVersion => meta.pdfe_version.is_some(),
        PropKind::DcTitle => meta.title.is_some(),
        PropKind::DcCreator => !meta.creators.is_empty(),
        PropKind::DcDescription => meta.description.is_some(),
        PropKind::DcSubject => !meta.subjects.is_empty(),
        PropKind::DcFormat => meta.format.is_some(),
        PropKind::XmpCreatorTool => meta.creator_tool.is_some(),
        PropKind::XmpCreateDate => meta.create_date.is_some(),
        PropKind::XmpModifyDate => meta.modify_date.is_some(),
        PropKind::XmpMetadataDate => meta.metadata_date.is_some(),
        PropKind::PdfKeywords => meta.keywords.is_some(),
        PropKind::PdfProducer => meta.producer.is_some(),
        PropKind::PdfTrapped => meta.trapped.is_some(),
    }
}

fn apply_scalar(meta: &mut Metadata, kind: PropKind, raw: &str) {
    let val = raw.trim_ascii().to_string();
    match kind {
        PropKind::Claim(c) => meta.claims.set(c, &val),
        PropKind::PdfxVersion => meta.pdfx_version = Some(val),
        PropKind::PdfeVersion => meta.pdfe_version = Some(val),
        PropKind::DcTitle => meta.title = Some(val),
        PropKind::DcCreator => meta.creators = vec![val],
        PropKind::DcDescription => meta.description = Some(val),
        PropKind::DcSubject => meta.subjects = vec![val],
        PropKind::DcFormat => meta.format = Some(val),
        PropKind::XmpCreatorTool => meta.creator_tool = Some(val),
        PropKind::XmpCreateDate => meta.create_date = Some(val),
        PropKind::XmpModifyDate => meta.modify_date = Some(val),
        PropKind::XmpMetadataDate => meta.metadata_date = Some(val),
        PropKind::PdfKeywords => meta.keywords = Some(val),
        PropKind::PdfProducer => meta.producer = Some(val),
        PropKind::PdfTrapped => meta.trapped = Some(val),
    }
}

fn finish_prop<S: Sink<Fault> + ?Sized>(
    meta: &mut Metadata,
    prop: OpenProp,
    sink: &mut S,
    stream: Option<ObjectId>,
    end: usize,
) {
    if let PropKind::Claim(c) = prop.kind {
        match (prop.lost, prop.has_child_elem) {
            (true, _) => {}
            (false, true) if prop.text.trim_ascii().is_empty() => {}
            (false, true) => tell(sink, stream, prop.start, end, Fault::XmpMixed),
            (false, false) => meta.claims.set(c, &prop.text),
        }
        return;
    }

    if !prop.items.is_empty() {
        match prop.kind {
            PropKind::DcTitle => meta.title = pick_alt(&prop.items),
            PropKind::DcDescription => meta.description = pick_alt(&prop.items),
            PropKind::DcCreator => {
                meta.creators = prop.items.into_iter().map(|(_, v)| v).collect();
            }
            PropKind::DcSubject => {
                meta.subjects = prop.items.into_iter().map(|(_, v)| v).collect();
            }
            other => {
                if let Some((_, first)) = prop.items.into_iter().next() {
                    apply_scalar(meta, other, &first);
                }
            }
        }
        return;
    }

    if !prop.lost && !prop.has_child_elem {
        apply_scalar(meta, prop.kind, &prop.text);
    }
}

fn pick_alt(items: &[(Option<String>, String)]) -> Option<String> {
    items
        .iter()
        .find(|(lang, _)| lang.as_deref() == Some("x-default"))
        .or_else(|| items.first())
        .map(|(_, val)| val.clone())
}

/// Reads [`Metadata`] from `catalog`'s `/Metadata` stream via `objects`.
///
/// # Errors
///
/// Same error conditions as [`super::read_from`].
#[allow(clippy::result_large_err)]
pub fn read_metadata_from<'o, O: Resolve<'o> + Streams, S: Sink<Fault> + ?Sized>(
    objects: O,
    catalog: &'o Dict,
    limits: &Limits,
    sink: &mut S,
) -> Result<Option<Metadata>, Error> {
    let unread = |why| Error {
        read: Claims::default(),
        at: 0,
        why,
    };
    let found = stream_of(&objects, catalog);
    let Some(id) = found.map_err(|u| unread(XmpWhy::Unreadable(u)))? else {
        return Ok(None);
    };
    let data = match objects.decoded(id, limits.xmp_bytes) {
        Ok(data) if data.len() > limits.xmp_bytes => return Err(unread(XmpWhy::TooLong)),
        Ok(data) => data,
        Err(u) if u.why == Why::Limit => return Err(unread(XmpWhy::TooLong)),
        Err(u) => return Err(unread(XmpWhy::Unreadable(u))),
    };
    read_metadata(&data, Some(id), limits, sink).map(Some)
}

/// Serializes `meta` (including [`Metadata::claims`] and Dublin Core / XMP
/// Basic / Adobe PDF properties) as a UTF-8 XMP packet suitable for `/Metadata`.
///
/// # Errors
///
/// Returns [`WriteError`] if any string has leading/trailing ASCII whitespace
/// (which reading trims) or characters outside XML 1.0 `Char`.
pub fn write_metadata(meta: &Metadata) -> Result<String, WriteError> {
    for claim in Claim::ALL {
        if let Some(v) = meta.claims.value(claim) {
            check_xml_value(claim.written().1, v)?;
        }
    }
    let scalars: [(&'static str, Option<&str>); 10] = [
        ("pdfxid:GTS_PDFXVersion", meta.pdfx_version.as_deref()),
        ("pdfeid:GTS_PDFEVersion", meta.pdfe_version.as_deref()),
        ("dc:title", meta.title.as_deref()),
        ("dc:description", meta.description.as_deref()),
        ("dc:format", meta.format.as_deref()),
        ("xmp:CreatorTool", meta.creator_tool.as_deref()),
        ("xmp:CreateDate", meta.create_date.as_deref()),
        ("xmp:ModifyDate", meta.modify_date.as_deref()),
        ("xmp:MetadataDate", meta.metadata_date.as_deref()),
        ("pdf:Producer", meta.producer.as_deref()),
    ];
    for (name, val) in scalars {
        if let Some(v) = val {
            check_xml_value(name, v)?;
        }
    }
    if let Some(v) = &meta.keywords {
        check_xml_value("pdf:Keywords", v)?;
    }
    if let Some(v) = &meta.trapped {
        check_xml_value("pdf:Trapped", v)?;
    }
    for c in &meta.creators {
        check_xml_value("dc:creator", c)?;
    }
    for s in &meta.subjects {
        check_xml_value("dc:subject", s)?;
    }

    let mut packet = String::from(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
         \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20 <rdf:Description rdf:about=\"\"",
    );

    let has_dc = meta.title.is_some()
        || !meta.creators.is_empty()
        || meta.description.is_some()
        || !meta.subjects.is_empty()
        || meta.format.is_some();
    let has_xmp = meta.creator_tool.is_some()
        || meta.create_date.is_some()
        || meta.modify_date.is_some()
        || meta.metadata_date.is_some();
    let has_pdf = meta.keywords.is_some() || meta.producer.is_some() || meta.trapped.is_some();
    let has_pdfa = meta.claims.pdfa_part.is_some()
        || meta.claims.pdfa_conformance.is_some()
        || meta.claims.pdfa_rev.is_some();
    let has_pdfua = meta.claims.pdfua_part.is_some() || meta.claims.pdfua_rev.is_some();
    let has_pdfvt = meta.claims.pdfvt_version.is_some();

    for (present, prefix, ns) in [
        (has_dc, "dc", DC),
        (has_xmp, "xmp", XMP_BASIC),
        (has_pdf, "pdf", ADOBE_PDF),
        (has_pdfa, "pdfaid", PDFA),
        (has_pdfua, "pdfuaid", PDFUA),
        (has_pdfvt, "pdfvtid", PDFVT),
        (meta.pdfx_version.is_some(), "pdfxid", PDFX_ID),
        (meta.pdfe_version.is_some(), "pdfeid", PDFE_ID),
    ] {
        if present {
            let _ = write!(packet, "\n    xmlns:{prefix}=\"{ns}\"");
        }
    }
    packet.push_str(">\n");

    if let Some(title) = &meta.title {
        write_alt(&mut packet, "dc:title", title);
    }
    if !meta.creators.is_empty() {
        write_list(&mut packet, "dc:creator", "rdf:Seq", &meta.creators);
    }
    if let Some(desc) = &meta.description {
        write_alt(&mut packet, "dc:description", desc);
    }
    if !meta.subjects.is_empty() {
        write_list(&mut packet, "dc:subject", "rdf:Bag", &meta.subjects);
    }
    for (tag, val) in [
        ("dc:format", meta.format.as_deref()),
        ("xmp:CreatorTool", meta.creator_tool.as_deref()),
        ("xmp:CreateDate", meta.create_date.as_deref()),
        ("xmp:ModifyDate", meta.modify_date.as_deref()),
        ("xmp:MetadataDate", meta.metadata_date.as_deref()),
        ("pdf:Keywords", meta.keywords.as_deref()),
        ("pdf:Producer", meta.producer.as_deref()),
        ("pdf:Trapped", meta.trapped.as_deref()),
        ("pdfxid:GTS_PDFXVersion", meta.pdfx_version.as_deref()),
        ("pdfeid:GTS_PDFEVersion", meta.pdfe_version.as_deref()),
    ] {
        if let Some(v) = val {
            write_simple(&mut packet, tag, v);
        }
    }
    for claim in Claim::ALL {
        if let Some(v) = meta.claims.value(claim) {
            write_simple(&mut packet, claim.written().1, v);
        }
    }

    packet.push_str(
        "  </rdf:Description>\n\
         \x20</rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"w\"?>",
    );
    Ok(packet)
}

fn write_simple(out: &mut String, tag: &str, value: &str) {
    let _ = write!(out, "   <{tag}>");
    escaped(out, value);
    let _ = writeln!(out, "</{tag}>");
}

fn write_alt(out: &mut String, tag: &str, value: &str) {
    let _ = write!(out, "   <{tag}><rdf:Alt><rdf:li xml:lang=\"x-default\">");
    escaped(out, value);
    let _ = writeln!(out, "</rdf:li></rdf:Alt></{tag}>");
}

fn write_list(out: &mut String, tag: &str, container: &str, items: &[String]) {
    let _ = writeln!(out, "   <{tag}><{container}>");
    for item in items {
        out.push_str("    <rdf:li>");
        escaped(out, item);
        out.push_str("</rdf:li>\n");
    }
    let _ = writeln!(out, "   </{container}></{tag}>");
}

fn check_xml_value(claim: &'static str, value: &str) -> Result<(), WriteError> {
    if value.trim_ascii() != value {
        return Err(WriteError::Spaced { claim });
    }
    if let Some((at, character)) = value.char_indices().find(|&(_, c)| !in_xml(c)) {
        return Err(WriteError::Character {
            claim,
            at,
            character,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId};

    use super::{
        InfoField, Metadata, check_info_sync, date_to_xmp, read_metadata, write_metadata,
        xmp_to_date,
    };
    use crate::Limits;
    use crate::date::Date;
    use crate::info::{self, Dated, Info, Trapped};

    #[test]
    fn round_trip_metadata_info_write_and_sync_check() {
        let info = Info {
            title: Some("Confidential Audit Report".into()),
            author: Some("Jane Doe".into()),
            subject: Some("FY2026 Review".into()),
            keywords: Some("audit, tax".into()),
            creator: Some("BinderTool".into()),
            producer: Some("pdf-semantics".into()),
            created: Some(Dated::Date(Date::read("D:20261115093000-08'00").unwrap())),
            modified: Some(Dated::Date(Date::read("D:20261115173000Z").unwrap())),
            trapped: Some(Trapped::False),
        };

        let info_dict = info::write(&info);
        let objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
        let read_info = info::read(&objects, &Object::from(info_dict), Rules::Iso, &mut ())
            .unwrap()
            .unwrap();
        assert_eq!(read_info, info);

        let mut meta = Metadata::from_info(&info);
        meta.claims.pdfa_part = Some("2".into());
        meta.claims.pdfa_conformance = Some("B".into());
        meta.subjects = vec!["audit".into(), "tax".into()];
        meta.pdfx_version = Some("PDF/X-4".into());

        let xml = write_metadata(&meta).unwrap();
        let mut faults = Vec::new();
        let decoded = read_metadata(xml.as_bytes(), None, &Limits::DEFAULT, &mut faults).unwrap();
        assert!(faults.is_empty(), "{faults:?}");
        assert_eq!(decoded, meta);
        assert!(check_info_sync(&decoded, &info).is_empty());

        let mut drifted = info.clone();
        drifted.title = Some("Different Title".into());
        drifted.modified = Some(Dated::Date(Date::read("D:20261115180000Z").unwrap()));
        assert_eq!(
            check_info_sync(&decoded, &drifted),
            vec![InfoField::Title, InfoField::ModDate]
        );

        let d = Date::read("D:20261115093000-08'00").unwrap();
        let iso = date_to_xmp(&d);
        assert_eq!(iso, "2026-11-15T09:30:00-08:00");
        assert_eq!(xmp_to_date(&iso).unwrap().write(), d.write());
        assert_eq!(
            xmp_to_date("2026-11-15T09:30:00.123Z").unwrap().write(),
            "D:20261115093000Z"
        );

        let dbg = format!("{decoded:?}");
        assert!(!dbg.contains("Confidential"), "{dbg}");
        assert!(!dbg.contains("Jane Doe"), "{dbg}");
    }
}
