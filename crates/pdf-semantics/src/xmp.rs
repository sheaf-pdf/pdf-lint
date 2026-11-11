//! What a file's XMP metadata claims it conforms to (ISO 32000-2, 14.3.2):
//! PDF/A (ISO 19005), PDF/UA (ISO 14289) and PDF/VT (ISO 16612-2), each claim
//! an attribute or an element, under any prefix bound to its namespace.
//!
//! [`read`] reads a packet as XML, on quick-xml, for the first value of each
//! claim, as written, so one bad value doesn't lose another;
//! [`Claims::conformance`] reads PDF/A's and PDF/UA's as what they say.
//! [`metadata_stream`] says which stream holds a file's packet, for a caller
//! to decode: a reading's `decoded` gives its data, decrypted if a password
//! opened the file.
//!
//! A claim is the file's own word, and nothing here holds a file to it.
//!
//! The reader goes on past what it can: an attribute that doesn't read, a
//! value that doesn't, or one that holds an element beside its text, is a
//! [`Fault`] and the walk goes on, so a claim elsewhere still counts. Past
//! that, XML that stops making sense or a packet past its caps
//! ([`Limits::xmp_bytes`], [`Limits::xmp_depth`], [`Limits::xmp_bindings`])
//! is an [`Error`], holding what was read before it. No DOCTYPE is read: an
//! entity it declares isn't expanded, and only the five XML predefines, and
//! character references, stand for anything.

use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, ObjectId, Resolve, Span, Unreadable, Value};
use quick_xml::NsReader;
use quick_xml::encoding::EncodingError;
use quick_xml::errors::Error as XmlError;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::name::{LocalName, Namespace, NamespaceError, ResolveResult};

use crate::{Fault, HOPS, Limits, too_long};

#[cfg(test)]
mod tests;

const PDFA: &[u8] = b"http://www.aiim.org/pdfa/ns/id/";
const PDFUA: &[u8] = b"http://www.aiim.org/pdfua/ns/id/";
const PDFVT: &[u8] = b"http://www.npes.org/pdfvt/ns/id/";

/// What a packet claims: the first value of each, as written but for the
/// white space round it, and an attribute's value normalized as XML has it.
/// A packet's values needn't say anything, as `conformance=""` or `part="9"`,
/// so a value is held as it is, and [`Claims::conformance`] reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Claims {
    /// `part` in PDF/A's namespace: 3 for PDF/A-3.
    pub pdfa_part: Option<String>,
    /// `conformance` in PDF/A's namespace: `B` for PDF/A-3b.
    pub pdfa_conformance: Option<String>,
    /// `rev` in PDF/A's namespace: the year of the part's edition.
    pub pdfa_rev: Option<String>,
    /// `part` in PDF/UA's namespace: 1 for PDF/UA-1.
    pub pdfua_part: Option<String>,
    /// `rev` in PDF/UA's namespace: the year of the part's edition.
    pub pdfua_rev: Option<String>,
    /// `GTS_PDFVTVersion` in PDF/VT's namespace: `PDF/VT-2` for PDF/VT-2.
    pub pdfvt_version: Option<String>,
}

/// One of the values [`Claims`] holds.
#[derive(Clone, Copy)]
enum Claim {
    APart,
    AConformance,
    ARev,
    UaPart,
    UaRev,
    VtVersion,
}

impl Claim {
    /// The claim a name is, by its namespace; none for any other, or for a
    /// prefix bound to nothing, which isn't in a namespace.
    fn of(namespace: ResolveResult, name: LocalName) -> Option<Claim> {
        let ResolveResult::Bound(Namespace(namespace)) = namespace else {
            return None;
        };
        match (namespace, name.as_ref()) {
            (PDFA, b"part") => Some(Claim::APart),
            (PDFA, b"conformance") => Some(Claim::AConformance),
            (PDFA, b"rev") => Some(Claim::ARev),
            (PDFUA, b"part") => Some(Claim::UaPart),
            (PDFUA, b"rev") => Some(Claim::UaRev),
            (PDFVT, b"GTS_PDFVTVersion") => Some(Claim::VtVersion),
            _ => None,
        }
    }
}

impl Claims {
    /// Where `claim`'s value goes.
    fn slot(&mut self, claim: Claim) -> &mut Option<String> {
        match claim {
            Claim::APart => &mut self.pdfa_part,
            Claim::AConformance => &mut self.pdfa_conformance,
            Claim::ARev => &mut self.pdfa_rev,
            Claim::UaPart => &mut self.pdfua_part,
            Claim::UaRev => &mut self.pdfua_rev,
            Claim::VtVersion => &mut self.pdfvt_version,
        }
    }

    /// Holds `value`, trimmed, unless `claim` has its first value. Only
    /// ASCII's white space is taken off: a no-break space isn't XML's, and
    /// what's written with one isn't the value without it.
    fn set(&mut self, claim: Claim, value: &str) {
        let slot = self.slot(claim);
        if slot.is_none() {
            *slot = Some(value.trim_ascii().to_string());
        }
    }

    /// PDF/A's and PDF/UA's claims as what they say: a number for a part, a
    /// letter for a level. A level without a part is no PDF/A claim, though
    /// it's read. Any that says neither is an error here, where `Claims`
    /// still has the others.
    pub fn conformance(&self) -> Result<Conformance, ClaimError> {
        let number = |claim, value: &Option<String>| {
            (value.as_deref())
                .map(|value| number(claim, value))
                .transpose()
        };
        let part = number("pdfaid:part", &self.pdfa_part)?;
        let level = self.pdfa_conformance.as_deref().map(letter).transpose()?;
        Ok(Conformance {
            pdfa: part.map(|part| PdfA { part, level }),
            pdfua: number("pdfuaid:part", &self.pdfua_part)?.map(PdfUa::new),
        })
    }
}

/// What a value of a part, of the claim named `claim`, says.
fn number(claim: &'static str, value: &str) -> Result<u32, ClaimError> {
    (value.parse()).map_err(|_| ClaimError::Part {
        claim,
        value: value.to_string(),
    })
}

/// A conformance level: one letter, upper case.
fn letter(value: &str) -> Result<char, ClaimError> {
    let mut chars = value.chars();
    (chars.next())
        .filter(|c| c.is_ascii_alphabetic() && chars.next().is_none())
        .map(|c| c.to_ascii_uppercase())
        .ok_or_else(|| ClaimError::Level {
            value: value.to_string(),
        })
}

/// A file's conformance claims, read ([`Claims::conformance`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Conformance {
    /// `pdfaid:part`, with its level.
    pub pdfa: Option<PdfA>,
    /// `pdfuaid:part`.
    pub pdfua: Option<PdfUa>,
}

/// A PDF/A claim: `pdfaid:part` and `pdfaid:conformance`. PDF/A-2b is part 2,
/// level `B`; PDF/A-4 may give no level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PdfA {
    /// The part.
    pub part: u32,
    /// The level, upper case.
    pub level: Option<char>,
}

impl PdfA {
    /// The claim of `part` and `level`, which is taken to upper case.
    pub fn new(part: u32, level: Option<char>) -> PdfA {
        PdfA {
            part,
            level: level.map(|level| level.to_ascii_uppercase()),
        }
    }
}

/// As the claim is named: "PDF/A-2b", "PDF/A-4".
impl fmt::Display for PdfA {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PDF/A-{}", self.part)?;
        match self.level {
            Some(level) => write!(f, "{}", level.to_ascii_lowercase()),
            None => Ok(()),
        }
    }
}

/// A PDF/UA claim: `pdfuaid:part`. PDF/UA-1 is part 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PdfUa {
    /// The part.
    pub part: u32,
}

impl PdfUa {
    /// The claim of `part`.
    pub const fn new(part: u32) -> PdfUa {
        PdfUa { part }
    }
}

/// As the claim is named: "PDF/UA-1".
impl fmt::Display for PdfUa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PDF/UA-{}", self.part)
    }
}

/// A claim whose value doesn't say what it's for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ClaimError {
    /// A part that isn't a number.
    #[error("{claim} {value:?} isn't a number")]
    Part {
        /// Which claim it is, as it's usually prefixed: `pdfaid:part` or
        /// `pdfuaid:part`.
        claim: &'static str,
        /// The value, as written.
        value: String,
    },
    /// A level that isn't one letter.
    #[error("pdfaid:conformance {value:?} isn't a letter")]
    Level {
        /// The value, as written.
        value: String,
    },
}

/// Why a packet was read no further. What it read up to there is the error's
/// [`read`](Error::read), for a caller to take or leave.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("XMP metadata reads no further than byte {at}: {why}")]
#[non_exhaustive]
pub struct Error {
    /// What it claimed before there.
    pub read: Claims,
    /// The offset in the packet where it stopped.
    pub at: usize,
    /// Why it stopped.
    pub why: XmpWhy,
}

/// What stopped a packet being read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum XmpWhy {
    /// It's longer than [`Limits::xmp_bytes`]: the rest isn't read.
    #[error("it's longer than is read")]
    TooLong,
    /// An element below [`Limits::xmp_depth`] levels.
    #[error("it nests deeper than is read")]
    TooDeep,
    /// More namespaces bound at once than [`Limits::xmp_bindings`].
    #[error("it binds more namespaces at once than are read")]
    TooManyBindings,
    /// What isn't XML, in the XML reader's words.
    #[error("{0}")]
    Xml(String),
}

/// A claim's element being read, whose value is its text.
struct Open {
    claim: Claim,
    /// How many elements are open in it, its own among them, so its own
    /// text is what stands at that level.
    level: usize,
    /// Its own text, so far.
    text: String,
    /// Whether an element is in it.
    element: bool,
    /// Whether its value didn't read, which was said: nothing more is.
    lost: bool,
    /// Where its start tag begins.
    start: usize,
}

/// What `xml`, an XMP packet, claims: each claim's first value, as an
/// attribute or an element, under any prefix bound to its namespace.
///
/// An element's value is its text: what's written at its own level, its
/// references and CDATA among it, and comments and processing instructions
/// no part of it. Written as an empty element, `<pdfaid:part/>`, it's as
/// it is written open and closed, an empty value. An element in it makes it
/// RDF's other kind of value, not text: with text of its own beside it,
/// more than white space, its value is mixed, told, and with white space
/// alone, it's structured, not told; neither is taken, so a value after it
/// counts. What's in the element in it is no part of its value, but a claim
/// there is read as its own. A claim's element whose claim has its value,
/// when the element starts, isn't read.
///
/// `stream` is the stream `xml` is the decoded data of, as [`metadata_stream`]
/// gives it. `sink` hears what's read past, each [`Place`] naming that stream,
/// as its object and as its `at`'s stream, the offsets being of its decoded
/// data. Given none, a place has neither, as nothing says what they'd be of:
///
/// - [`Fault::XmpNotUtf8`]: a packet whose first bytes say it isn't UTF-8,
///   as XML tells an encoding from them: a mark of UTF-16 or UTF-32, at the
///   mark, or a zero byte among the first four, at those. Nothing's read;
/// - [`Fault::XmpAttribute`]: an attribute that doesn't read, with no quotes
///   or no value or given twice. The tag's others are read, as XML's reader
///   can go on, but a namespace bound after one that doesn't read isn't bound;
/// - [`Fault::XmpValue`]: a claim's value that doesn't read, with an entity
///   that isn't one of XML's five, an `&` that starts no reference, or that
///   isn't UTF-8. It's as though it weren't written, so a value after it
///   counts, and nothing more is told of it. An `&` elsewhere in the
///   packet's text, as in a title or in an element in a claim's, is passed;
/// - [`Fault::XmpMixed`]: a claim's element holding an element beside text
///   of its own, from its start tag to its end tag.
///
/// An error ([`Error`]) holds what was read before it. A packet may end
/// with elements still open, and an element's value counts once it closes:
/// one left open, at the end or where an error stops the read, is neither
/// taken nor told.
// A packet's read once a file, and its error holds what's read.
#[allow(clippy::result_large_err)]
pub fn read<S: Sink<Fault> + ?Sized>(
    xml: &[u8],
    stream: Option<ObjectId>,
    limits: &Limits,
    sink: &mut S,
) -> Result<Claims, Error> {
    read_as(xml, stream, limits, sink, false)
}

/// [`read`], and with `expand`, each empty element given by the XML reader
/// as a start tag and an end tag, which reads the same: the fuzz body's
/// check that the two forms are read alike.
#[allow(clippy::result_large_err)]
pub(crate) fn read_as<S: Sink<Fault> + ?Sized>(
    xml: &[u8],
    stream: Option<ObjectId>,
    limits: &Limits,
    sink: &mut S,
    expand: bool,
) -> Result<Claims, Error> {
    let mut claims = Claims::default();
    if let Some(end) = not_utf8(xml) {
        tell(sink, stream, 0, end, Fault::XmpNotUtf8);
        return Ok(claims);
    }
    let cut = xml.len() > limits.xmp_bytes;
    let data = &xml[..xml.len().min(limits.xmp_bytes)];
    let mut reader = NsReader::from_reader(data);
    reader.config_mut().allow_dangling_amp = true;
    reader.config_mut().expand_empty_elements = expand;
    (reader.resolver_mut()).set_max_declarations_per_element(limits.xmp_bindings);
    let depth = limits.xmp_depth.min(usize::from(u16::MAX) - 1);
    // How many namespaces each element open has bound, and them all.
    let (mut levels, mut bindings): (Vec<usize>, usize) = (Vec::new(), 0);
    // The claims' elements open, innermost last: at most one a level.
    let mut open: Vec<Open> = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let stop = |read: Claims, at, why| Err(Error { read, at, why });
        let event = match reader.read_event() {
            Ok(event) => event,
            // A tag, or the like, is what a cut packet may end on.
            Err(XmlError::Syntax(_)) if cut => {
                return stop(claims, data.len(), XmpWhy::TooLong);
            }
            Err(XmlError::Namespace(NamespaceError::TooManyDeclarations(_))) => {
                return stop(claims, start, XmpWhy::TooManyBindings);
            }
            Err(error) => {
                let at = reader.error_position() as usize;
                return stop(claims, at, XmpWhy::Xml(error.to_string()));
            }
        };
        let end = reader.buffer_position() as usize;
        match &event {
            Event::Start(tag) | Event::Empty(tag) => {
                if levels.len() >= depth {
                    return stop(claims, start, XmpWhy::TooDeep);
                }
                let bound = bound(tag);
                if bindings + bound > limits.xmp_bindings {
                    return stop(claims, start, XmpWhy::TooManyBindings);
                }
                for attribute in tag.attributes() {
                    let Ok(attribute) = attribute else {
                        tell(sink, stream, start, end, Fault::XmpAttribute);
                        continue;
                    };
                    let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                    let Some(claim) = Claim::of(namespace, name) else {
                        continue;
                    };
                    if claims.slot(claim).is_some() {
                        continue;
                    }
                    match attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
                        Ok(value) => claims.set(claim, &value),
                        Err(_) => tell(sink, stream, start, end, Fault::XmpValue),
                    }
                }
                // The innermost claim's element holds it; each outside that
                // holds the innermost's, marked when it started.
                if let Some(innermost) = open.last_mut() {
                    innermost.element = true;
                }
                let (namespace, name) = reader.resolver().resolve_element(tag.name());
                let claim =
                    Claim::of(namespace, name).filter(|claim| claims.slot(*claim).is_none());
                if matches!(event, Event::Start(_)) {
                    levels.push(bound);
                    bindings += bound;
                    open.extend(claim.map(|claim| Open {
                        claim,
                        level: levels.len(),
                        text: String::new(),
                        element: false,
                        lost: false,
                        start,
                    }));
                } else if let Some(claim) = claim {
                    // As it's written open and closed: an empty value.
                    claims.set(claim, "");
                }
            }
            // Text only starts with `&` where that starts no reference.
            Event::Text(text) => {
                if let Some(value) = own(&mut open, levels.len()) {
                    match text.starts_with(b"&") {
                        true => lose(value, sink, stream, start..end),
                        false => push(value, text.xml10_content(), sink, stream, start..end),
                    }
                }
            }
            Event::CData(text) => {
                if let Some(value) = own(&mut open, levels.len()) {
                    push(value, text.xml10_content(), sink, stream, start..end);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(value) = own(&mut open, levels.len()) {
                    match character(reference) {
                        Some(character) => value.text.push_str(&character),
                        None => lose(value, sink, stream, start..end),
                    }
                }
            }
            Event::End(_) => {
                let level = levels.len();
                bindings -= levels.pop().unwrap_or(0);
                if let Some(value) = open.pop_if(|value| value.level == level) {
                    let Open {
                        claim,
                        text,
                        element,
                        lost,
                        start,
                        ..
                    } = value;
                    match (lost, element) {
                        // Said where it was lost.
                        (true, _) => {}
                        // Structured: elements and white space alone.
                        (false, true) if text.trim_ascii().is_empty() => {}
                        (false, true) => tell(sink, stream, start, end, Fault::XmpMixed),
                        (false, false) => claims.set(claim, &text),
                    }
                }
            }
            Event::Eof if cut => return stop(claims, data.len(), XmpWhy::TooLong),
            Event::Eof => return Ok(claims),
            _ => {}
        }
    }
}

/// Where `xml`'s first bytes say it isn't UTF-8, the end of those bytes, as
/// XML 1.0's Appendix F tells an encoding from them: a mark of UTF-32 or of
/// UTF-16, UTF-32LE's tried before UTF-16LE's, which begins it; or a zero
/// byte among the first four, as a `<` has in UTF-16 or UTF-32 without a
/// mark, and no UTF-8 XML has, XML having no U+0000. UTF-8's own mark is
/// read past.
fn not_utf8(xml: &[u8]) -> Option<usize> {
    const MARKS: [&[u8]; 4] = [
        &[0x00, 0x00, 0xFE, 0xFF],
        &[0xFF, 0xFE, 0x00, 0x00],
        &[0xFE, 0xFF],
        &[0xFF, 0xFE],
    ];
    if let Some(mark) = MARKS.iter().find(|mark| xml.starts_with(mark)) {
        return Some(mark.len());
    }
    let first = &xml[..xml.len().min(4)];
    first.contains(&0).then_some(first.len())
}

/// How many namespaces `tag` binds, as the XML reader counts them: up to its
/// first attribute that doesn't read.
fn bound(tag: &BytesStart) -> usize {
    let mut attributes = tag.attributes();
    attributes.with_checks(false);
    (attributes.map_while(Result::ok))
        .filter(|attribute| attribute.key.as_namespace_binding().is_some())
        .count()
}

/// The innermost claim's element, where what's read at `level` is its own
/// text and its value isn't lost: none elsewhere.
fn own(open: &mut [Open], level: usize) -> Option<&mut Open> {
    (open.last_mut()).filter(|value| value.level == level && !value.lost)
}

/// Adds `text` to the value being read; if it doesn't read, the value's lost.
fn push<S: Sink<Fault> + ?Sized>(
    value: &mut Open,
    text: Result<std::borrow::Cow<str>, EncodingError>,
    sink: &mut S,
    stream: Option<ObjectId>,
    at: std::ops::Range<usize>,
) {
    match text {
        Ok(text) => value.text.push_str(&text),
        Err(_) => lose(value, sink, stream, at),
    }
}

/// Loses the value being read, at bytes `at` that don't read, and says so.
fn lose<S: Sink<Fault> + ?Sized>(
    value: &mut Open,
    sink: &mut S,
    stream: Option<ObjectId>,
    at: std::ops::Range<usize>,
) {
    value.lost = true;
    value.text = String::new();
    tell(sink, stream, at.start, at.end, Fault::XmpValue);
}

/// What a reference in an element's text stands for: a character's, or one
/// of the five XML predefines. No other is known, as no DOCTYPE is read.
fn character(reference: &BytesRef) -> Option<String> {
    match reference.resolve_char_ref() {
        Ok(Some(character)) => Some(character.to_string()),
        Ok(None) => {
            let name = reference.decode().ok()?;
            resolve_predefined_entity(&name).map(str::to_string)
        }
        Err(_) => None,
    }
}

/// Tells `sink`, if it wants faults, of `fault` at bytes `start..end` of the
/// packet, which is the decoded data of `stream`.
fn tell<S: Sink<Fault> + ?Sized>(
    sink: &mut S,
    stream: Option<ObjectId>,
    start: usize,
    end: usize,
    fault: Fault,
) {
    if sink.wants() {
        let at = stream.map(|id| Span::new(Some(id), start, end));
        sink.fault(Place::new(stream, at), fault);
    }
}

/// The stream `catalog`'s `/Metadata` leads to (ISO 32000-2, 14.3.2), whose
/// decoded data is the file's packet: none where there's no `/Metadata`, or it
/// leads to no stream. An object on the way that doesn't read says why, so a
/// failed read doesn't look like a file with no metadata.
pub fn metadata_stream<'o, R: Resolve<'o>>(
    objects: R,
    catalog: &'o Dict,
) -> Result<Option<ObjectId>, Unreadable> {
    let Some(mut at) = catalog.get(b"Metadata") else {
        return Ok(None);
    };
    let mut last = None;
    for _ in 0..HOPS {
        match at.value() {
            Value::Ref(id) => {
                last = Some(*id);
                match objects.try_resolve(*id)? {
                    Some(object) => at = object,
                    None => return Ok(None),
                }
            }
            Value::Stream(_) => return Ok(last),
            _ => return Ok(None),
        }
    }
    Err(too_long(last.unwrap_or(ObjectId::new(0, 0))))
}
