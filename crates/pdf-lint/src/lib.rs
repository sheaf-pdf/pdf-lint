//! Validate PDF files against the Arlington PDF Model and ISO 32000-2.
//!
//! `pdf-lint` combines two layers of validation:
//!
//! 1. **Arlington grammar walk ([`pdf_arlington`]):** Walks every object
//!    reachable from the trailer (plus object streams, linearization
//!    dictionaries, and earlier incremental-update trailers) and evaluates the
//!    embedded Arlington TSV tables—checking key types, required keys, version
//!    gates, allowed values, special-case predicates, broken references, and
//!    duplicate keys.
//! 2. **ISO 32000-2 subsystem checks:** Uses [`pdf_content_streams`],
//!    [`pdf_semantics`], [`pdf_color`], [`pdf_fonts`], and [`pdf_linearize`] to
//!    validate content streams (`q`/`Q` balance, marked content, operator
//!    states, operand types, resource bindings), page trees (including viewer
//!    vs. ISO divergences), name/number trees, XMP metadata, color spaces,
//!    fonts, object-stream packing rules, and Annex F linearization.
//!
//! # Entry Points
//!
//! - [`check()`] validates any [`Source`](pdf_structure::Source) (such as an
//!   in-memory [`Document`](pdf_structure::Document)).
//! - [`check_with_reading`] takes a
//!   [`WithReading`](pdf_structure::WithReading) so it can also decode
//!   content/font/metadata streams, inspect overwritten objects in
//!   historical revisions, and verify Annex F linearization.
//! - [`Report`] holds all [`Finding`]s (with exact byte
//!   [`Span`](pdf_objects::Span)s) and summary counts, and supports baseline
//!   diffing via [`Report::since`], [`Report::by_path`], and
//!   [`Report::beyond`].
//! - [`grammar()`] returns the active Arlington
//!   [`Grammar`](pdf_arlington::Grammar) (embedded by default, or overridden
//!   once at startup via [`use_model`]).
//!
//! The crates whose types these take and give are re-exported whole, at the
//! versions this crate takes: [`pdf_objects`], [`pdf_structure`] and
//! [`pdf_arlington`].
//!
//! # Example
//!
//! A document made in memory, whose page has a key the model doesn't list,
//! checked with every object held as built, so that the key is a finding:
//!
//! ```
//! use pdf_lint::pdf_objects::{Object, Version, dict};
//! use pdf_lint::pdf_structure::Document;
//! use pdf_lint::{Hold, Made, Options, Rule, check};
//!
//! let r = Object::reference;
//! let trailer = dict! { "Size" => 4, "Root" => r(1) };
//! let mut doc = Document::new(Version::new(1, 7), trailer);
//! doc.insert(1, dict! { "Type" => Object::name("Catalog"), "Pages" => r(2) });
//! doc.insert(
//!     2,
//!     dict! { "Type" => Object::name("Pages"), "Kids" => vec![r(3)], "Count" => 1 },
//! );
//! let media_box: Vec<Object> = [0, 0, 612, 792].map(Object::from).into();
//! doc.insert(
//!     3,
//!     dict! {
//!         "Type" => Object::name("Page"),
//!         "Parent" => r(2),
//!         "MediaBox" => media_box,
//!         "Resources" => dict! {},
//!         "MyKey" => true,
//!     },
//! );
//!
//! let report = check(&doc, &Hold::All(Made::Built), &Options::default());
//! let [finding] = report.findings.as_slice() else {
//!     panic!("{report}");
//! };
//! assert_eq!(finding.rule, Rule::Unlisted);
//! assert_eq!((finding.table.as_str(), finding.key.as_str()), ("PageObject", "MyKey"));
//! assert_eq!(finding.path, "Root/Pages/Kids[0]");
//! assert_eq!(finding.object.map(|id| id.number), Some(3));
//! ```

pub mod allowance;
pub mod check;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;

pub use allowance::{Allowance, AllowanceError, Allowances};
pub use check::{
    Class, Exempt, ExemptError, Finding, Hold, HoldError, Linearized, Made, Options, Report, Rule,
    Unchecked, Unlisted, UnlistedIn, check, check_with_reading, file_version,
};
// `grammar` is the module and the function both.
pub use grammar::{grammar, use_model};
// The crates whose types this crate's API names, each re-exported whole, at
// the version this crate takes: the model, its tables and predicates are
// pdf-arlington's (`grammar()`'s `Grammar`); the objects, their ids, spans
// and versions pdf-objects' (`Finding::object` and `at`,
// `Options::version`); and the reader, what it reads a file into, the
// passwords it's given and the faults its filters tell, pdf-structure's
// (`check`'s `Source`, `check_with_reading`'s `WithReading`), pdf-crypt and
// pdf-filters under it.
pub use pdf_arlington;
pub use pdf_objects;
pub use pdf_structure;

// The names this crate's modules reach the family's by: pdf-structure's
// `read` module is `bytes` here, the name it has had.
use pdf_arlington::predicate;
use pdf_objects::{object, text, version};
use pdf_structure::read as bytes;
use pdf_structure::source;

/// Which model every check reads: the embedded one, or one [`use_model`]
/// chose. The model's tables are pdf-arlington's
/// ([`pdf_arlington::grammar`]).
pub mod grammar {
    use std::sync::OnceLock;

    // The model's types, as this crate's modules name them.
    pub(crate) use pdf_arlington::grammar::*;

    /// The model [`use_model`] chose; none once anything has read the
    /// embedded one instead.
    static CHOSEN: OnceLock<Option<Grammar>> = OnceLock::new();

    /// The tables the checker reads by name, which a model must have: the
    /// trailer's two; the object streams', which nothing in the model links
    /// to but an object stream, so the checker visits each as it; the
    /// linearization dictionary's, which nothing links to, which the
    /// checker visits a linearized file's as; and those it checks a
    /// metadata stream and associated files as where the model has no row
    /// for them. A slice, not an array of a fixed length, so a table can
    /// come without a breaking release.
    pub const NAMED: &[&str] = &[
        "FileTrailer",
        "XRefStream",
        "ObjectStream",
        "LinearizationParameterDict",
        "Metadata",
        "ArrayOfFileSpecifications",
    ];

    /// The model every check reads: the one [`use_model`] chose, else the
    /// embedded one ([`Grammar::embedded`]), read once.
    ///
    /// # Panics
    ///
    /// When the embedded one doesn't read, naming the file and line, which
    /// its tests rule out.
    #[must_use]
    pub fn grammar() -> &'static Grammar {
        CHOSEN
            .get_or_init(|| None)
            .as_ref()
            .unwrap_or_else(Grammar::embedded)
    }

    /// Makes `model` the one every check reads, in place of the embedded
    /// one: a newer checkout of upstream's `tsv/latest`, read with
    /// [`Grammar::load`]. Before anything has read the model, and once.
    ///
    /// # Errors
    ///
    /// A [`Refused`] says when something has read the model already, or
    /// when `model` lacks a table the checker reads by name ([`NAMED`]).
    pub fn use_model(model: Grammar) -> Result<(), Refused> {
        for &name in NAMED {
            if model.table(name).is_none() {
                return Err(Refused::Lacks { table: name });
            }
        }
        CHOSEN.set(Some(model)).map_err(|_| Refused::Chosen)
    }

    /// Why [`use_model`] doesn't take a model.
    #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
    #[non_exhaustive]
    pub enum Refused {
        /// It lacks a table the checker reads by name ([`NAMED`]).
        #[error("it has no {table} table, which the checker reads by name")]
        Lacks {
            /// The table.
            table: &'static str,
        },
        /// Something has read the model already: the embedded one, or one
        /// chosen before.
        #[error("the model is chosen already: something has read it")]
        Chosen,
    }
}

/// The extensions a file declares, read from a
/// [`Source`](pdf_structure::Source)
/// ([`declared`](extension::declared)), and those the model names
/// ([`named`](extension::named)): pdf-arlington's
/// [`extension`](pdf_arlington::extension).
pub mod extension {
    use std::collections::BTreeSet;

    use pdf_arlington::extension::Claims;
    use pdf_semantics::{Limits, xmp};

    use crate::check::{Objects, Packet};
    use crate::source::Source;

    /// Every extension the model names, in its cells and around its
    /// types, values and links: what a file can be checked under. It's
    /// [`Grammar::extensions`](pdf_arlington::Grammar::extensions) of the
    /// model every check reads, [`grammar()`](crate::grammar()).
    #[must_use]
    pub fn named() -> BTreeSet<String> {
        crate::grammar().extensions()
    }

    /// The extensions `source` declares, as the model names them, from its
    /// trailer's `/Root` and its metadata ([`Source::metadata`]), as
    /// pdf-arlington's [`declared`](pdf_arlington::extension::declared)
    /// reads them. Its metadata is read as XML by
    /// [`pdf_semantics::xmp::read`], so a claim is one in the namespace it's
    /// bound to; what a packet claims before anything that stops the read
    /// counts.
    #[must_use]
    pub fn declared(source: &dyn Source) -> BTreeSet<String> {
        read(source).0
    }

    /// What `source`'s metadata says, its packet read once: the extensions
    /// it declares, as [`declared`] gives them; and the packet as read: the
    /// catalog's metadata stream it's the data of, where that's known
    /// ([`xmp::metadata_stream`]), what [`xmp::read`] told of it, kept,
    /// each place naming that stream, at bytes of its data, and what stopped
    /// the read, if anything did, after which nothing counts. An object on
    /// the way to the stream that doesn't read leaves it not known.
    pub(crate) fn read(source: &dyn Source) -> (BTreeSet<String>, Packet) {
        let resolve = |id| source.get(id).ok().cloned();
        let trailer = source.trailer().dict();
        let mut packet = Packet::default();
        let result = match source.metadata().as_deref() {
            Some(xml) => {
                let catalog = pdf_semantics::catalog(Objects(source), trailer)
                    .ok()
                    .flatten();
                packet.stream = catalog
                    .and_then(|catalog| xmp::metadata_stream(Objects(source), catalog).ok())
                    .flatten();
                packet.len = xml.len();
                Some(xmp::read(
                    xml,
                    packet.stream,
                    &Limits::DEFAULT,
                    &mut packet.kept,
                ))
            }
            None => None,
        };
        let read = match &result {
            Some(Ok(claims)) => Some(claims),
            Some(Err(error)) => Some(&error.read),
            None => None,
        };
        let mut claims = Claims::default();
        claims.pdfa_part = read.and_then(|read| read.pdfa_part.as_deref());
        claims.pdfvt_version = read.and_then(|read| read.pdfvt_version.as_deref());
        let declared = pdf_arlington::extension::declared(trailer.get(b"Root"), claims, &resolve);
        packet.stopped = result.and_then(Result::err);
        (declared, packet)
    }

    #[cfg(test)]
    mod tests {
        use pdf_objects::dict;
        use pdf_semantics::Fault;

        use super::*;
        use crate::object::{Object, ObjectId};
        use crate::source::Document;
        use crate::version::Version;

        /// A document's catalog, through its trailer's reference, and its
        /// metadata, both declare.
        #[test]
        fn a_document_declares_through_its_catalog_and_metadata() {
            let mut doc =
                Document::new(Version::new(2, 0), dict! { "Root" => Object::reference(1) });
            let iso = dict! { "BaseVersion" => Object::name("2.0"), "ExtensionLevel" => 32001 };
            doc.insert(
                1,
                dict! {
                    "Type" => Object::name("Catalog"),
                    "Extensions" => dict! { "ISO_" => iso },
                },
            );
            assert_eq!(declared(&doc), BTreeSet::from(["ISO_TS_32001".to_string()]));
            assert_eq!(declared(&doc), quietly(&doc));
            doc.set_metadata(Some(xmp(r#"pdfaid:part="3""#)));
            let both = ["ISO_19005_3", "ISO_TS_32001"].map(String::from);
            assert_eq!(declared(&doc), BTreeSet::from(both));
            assert_eq!(declared(&doc), quietly(&doc));
        }

        /// What `doc` declares, its packet read with no stream and a sink that
        /// wants no faults, so none is looked for: what [`declared`] gives,
        /// which reads it with both, as neither changes what's read.
        fn quietly(doc: &Document) -> BTreeSet<String> {
            let resolve = |id| doc.get(id).ok().cloned();
            let read = (doc.metadata())
                .map(|xml| {
                    xmp::read(xml, None, &Limits::DEFAULT, &mut ())
                        .unwrap_or_else(|error| error.read)
                })
                .unwrap_or_default();
            let mut claims = Claims::default();
            claims.pdfa_part = read.pdfa_part.as_deref();
            claims.pdfvt_version = read.pdfvt_version.as_deref();
            pdf_arlington::extension::declared(doc.trailer().dict().get(b"Root"), claims, &resolve)
        }

        /// A packet read once, for what it declares and what it tells: with
        /// faults, the same claims as [`declared`]'s, and as without them;
        /// each fault kept in the catalog's metadata stream, which the packet
        /// names, at its bytes there, or in no object where no catalog names
        /// the stream, which it leaves not known; and what stops the read,
        /// what's claimed before it counted.
        #[test]
        fn a_packet_read_once_declares_as_declared_and_keeps_its_faults() {
            let vt = "<pdfvtid:GTS_PDFVTVersion>PDF/VT-2</pdfvtid:GTS_PDFVTVersion>";
            let faulty = packet(r#"pdfaid:part="3" x=y"#, vt);
            let clean = packet(r#"pdfaid:part="3""#, vt);
            let mut doc =
                Document::new(Version::new(2, 0), dict! { "Root" => Object::reference(1) });
            let catalog = dict! {
                "Type" => Object::name("Catalog"),
                "Metadata" => Object::reference(5),
            };
            doc.insert(1, catalog);
            let stream = |data: &[u8]| {
                let dict = dict! {
                    "Type" => Object::name("Metadata"),
                    "Subtype" => Object::name("XML"),
                };
                crate::object::Stream::new(dict, data.to_vec())
            };
            let both = ["ISO_19005_3", "PDF_VT2"].map(String::from);
            for (data, faults) in [(&clean, 0), (&faulty, 1)] {
                doc.insert(5, stream(data));
                doc.set_metadata(Some(data.clone()));
                let (claimed, got) = read(&doc);
                assert_eq!(claimed, BTreeSet::from(both.clone()));
                assert_eq!(claimed, declared(&doc));
                assert_eq!(claimed, quietly(&doc));
                assert_eq!(got.stream, Some(ObjectId::new(5, 0)));
                assert_eq!(got.len, data.len());
                assert!(got.stopped.is_none());
                assert_eq!(got.kept.told().len(), faults);
                for (place, fault, count) in got.kept.told() {
                    assert_eq!(*fault, Fault::XmpAttribute);
                    assert_eq!(*count, 1);
                    assert_eq!(place.object, Some(ObjectId::new(5, 0)));
                    let at = place.at.unwrap();
                    assert_eq!(at.stream(), Some(ObjectId::new(5, 0)));
                    assert!(data[at.start()..at.end()].starts_with(b"<rdf:Description"));
                }
            }
            // With no catalog to name its stream, a fault's in no object.
            let mut alone = Document::new(Version::new(2, 0), dict! {});
            alone.set_metadata(Some(faulty.clone()));
            let (claimed, got) = read(&alone);
            assert_eq!(claimed, BTreeSet::from(both.clone()));
            assert_eq!(got.stream, None);
            assert_eq!(got.kept.told().len(), 1);
            let place = got.kept.told()[0].0;
            assert_eq!((place.object, place.at), (None, None));
            // What stops the read, with what's claimed before it.
            let stops = [packet(r#"pdfaid:part="3""#, ""), b"</e>".to_vec()].concat();
            alone.set_metadata(Some(stops));
            let (claimed, got) = read(&alone);
            assert_eq!(claimed, BTreeSet::from(["ISO_19005_3".to_string()]));
            assert_eq!(claimed, quietly(&alone));
            assert!(matches!(got.stopped.unwrap().why, xmp::XmpWhy::Xml(_)));
            // With no packet, nothing.
            alone.set_metadata(None);
            let (_, none) = read(&alone);
            assert_eq!((none.stream, none.len), (None, 0));
            assert!(none.kept.told().is_empty() && none.stopped.is_none());
        }

        /// XMP metadata with `attributes` on an element that binds pdfaid's
        /// and pdfvtid's prefixes, and then `elements`.
        fn packet(attributes: &str, elements: &str) -> Vec<u8> {
            format!(
                "<rdf:Description xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" \
                 xmlns:pdfvtid=\"http://www.npes.org/pdfvt/ns/id/\" {attributes}>\
                 {elements}</rdf:Description>"
            )
            .into_bytes()
        }

        /// XMP metadata with `attributes` on an element that binds pdfaid's
        /// prefix.
        fn xmp(attributes: &str) -> Vec<u8> {
            packet(attributes, "")
        }

        /// What a document with `packet` as its metadata declares, which is
        /// what it declared read quietly.
        fn from(packet: &[u8]) -> Vec<String> {
            let mut doc = Document::new(Version::new(2, 0), dict! {});
            doc.set_metadata(Some(packet.to_vec()));
            let declared = declared(&doc);
            let quiet = quietly(&doc);
            assert_eq!(declared, quiet, "{}", String::from_utf8_lossy(packet));
            declared.into_iter().collect()
        }

        /// As an attribute or an element, with spaces round its `=` and
        /// inside its value, in either quotes; and PDF/VT beside it.
        #[test]
        fn metadata_declares_by_an_attribute_or_an_element() {
            let a3 = ["ISO_19005_3"];
            assert_eq!(from(&xmp(r#"pdfaid:part="3" pdfaid:conformance="B""#)), a3);
            assert_eq!(from(&xmp(r#"pdfaid:part = '3'"#)), a3);
            assert_eq!(from(&xmp(r#"pdfaid:part=" 3 ""#)), a3);
            assert_eq!(from(&packet("", "<pdfaid:part> 3 </pdfaid:part>")), a3);
            let element = "<pdfaid:part> 3 </pdfaid:part>\
                 <pdfvtid:GTS_PDFVTVersion>PDF/VT-2</pdfvtid:GTS_PDFVTVersion>";
            assert_eq!(from(&packet("", element)), ["ISO_19005_3", "PDF_VT2"]);
            let attribute = r#"pdfvtid:GTS_PDFVTVersion="PDF/VT-2""#;
            assert_eq!(from(&xmp(attribute)), ["PDF_VT2"]);
            assert_eq!(from(&xmp(r#"pdfaid:part='2'"#)), Vec::<String>::new());
        }

        /// What the reader of bytes this replaced read otherwise, each on
        /// purpose: a claim is XML's, in its namespace.
        #[test]
        fn a_claim_is_read_as_xml() {
            let nothing = Vec::<String>::new();
            let a3 = ["ISO_19005_3"];
            // A prefix bound to nothing, or to another namespace.
            assert_eq!(from(br#"<d pdfaid:part="3"/>"#), nothing);
            let other = br#"<d xmlns:pdfaid="http://example.com/" pdfaid:part="3"/>"#;
            assert_eq!(from(other), nothing);
            // Not markup, in a comment or CDATA.
            assert_eq!(
                from(&packet("", "<!-- <pdfaid:part>3</pdfaid:part> -->")),
                nothing
            );
            assert_eq!(from(&packet("", r#"<!-- pdfaid:part="3" -->"#)), nothing);
            let cdata = "<![CDATA[<pdfaid:part>3</pdfaid:part>]]>";
            assert_eq!(from(&packet("", cdata)), nothing);
            // After what stops the read.
            let late = r#"</e><d xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="3"/>"#;
            assert_eq!(from(late.as_bytes()), nothing);
            // But before it, it counts.
            let early = [xmp(r#"pdfaid:part="3""#), b"</e>".to_vec()].concat();
            assert_eq!(from(&early), a3);
            // Under any prefix bound to its namespace, a character by its
            // number, and an element with attributes of its own.
            let another = br#"<d xmlns:p="http://www.aiim.org/pdfa/ns/id/" p:part="3"/>"#;
            assert_eq!(from(another), a3);
            assert_eq!(from(&xmp(r#"pdfaid:part="&#51;""#)), a3);
            let attributed = r#"<pdfaid:part xml:lang="en">3</pdfaid:part>"#;
            assert_eq!(from(&packet("", attributed)), a3);
        }
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pdf_objects::text::{pdf_doc_char, pdfium};
    use pdf_structure::pdf_crypt::{Encoding, Password};

    use crate::grammar::Grammar;
    use crate::{grammar, use_model};

    /// The README is shown on crates.io and docs.rs, where a path beside the
    /// crate reaches nothing: its licence link is the repository's URL, the
    /// manifest's `repository` and the file, and every link it has is a URL,
    /// so none is a path such as `../` (the crates it names are linked by
    /// their folders in the repository). The crate is Apache-2.0 alone, as
    /// the model it embeds and the parts adapted from TestGrammar are, so the
    /// link is to `LICENSE-APACHE` alone.
    #[test]
    fn the_readmes_licence_link_reaches_the_file() {
        let readme = include_str!("../README.md");
        let repository = env!("CARGO_PKG_REPOSITORY");
        let link = format!("[LICENSE-APACHE]({repository}/blob/main/LICENSE-APACHE)");
        assert!(
            readme.contains(&link),
            "LICENSE-APACHE's link isn't the repository's URL"
        );
        for (at, _) in readme.match_indices("](") {
            let target = &readme[at + 2..];
            let target = &target[..target.find(')').unwrap_or(target.len())];
            assert!(
                target.starts_with("https://"),
                "a link isn't a URL: {target}"
            );
        }
    }

    /// A model that lacks a table the checker reads by name is refused,
    /// and the refusal names the table, word for word.
    #[test]
    fn a_model_without_a_table_the_checker_reads_by_name_is_refused() {
        let dir = tempfile::Builder::new()
            .prefix("pdf-lint-model-")
            .tempdir()
            .unwrap();
        let header = grammar::HEADER.join("\t");
        let row = "Rotate\tinteger\t1.0\t\tFALSE\tFALSE\tTRUE\t0\t\t\t\t";
        std::fs::write(dir.path().join("T.tsv"), format!("{header}\n{row}\n")).unwrap();
        let e = use_model(Grammar::load(dir.path()).unwrap()).unwrap_err();
        assert_eq!(
            format!("{e:#}"),
            "it has no FileTrailer table, which the checker reads by name"
        );
    }

    /// A model with every table the checker reads by name but the object
    /// streams' is refused, naming it: the checker visits every object
    /// stream as it.
    #[test]
    fn a_model_without_the_object_streams_table_is_refused() {
        let dir = tempfile::Builder::new()
            .prefix("pdf-lint-model-")
            .tempdir()
            .unwrap();
        let header = grammar::HEADER.join("\t");
        let row = "Rotate\tinteger\t1.0\t\tFALSE\tFALSE\tTRUE\t0\t\t\t\t";
        for table in grammar::NAMED.iter().filter(|&&t| t != "ObjectStream") {
            let path = dir.path().join(format!("{table}.tsv"));
            std::fs::write(path, format!("{header}\n{row}\n")).unwrap();
        }
        let e = use_model(Grammar::load(dir.path()).unwrap()).unwrap_err();
        assert_eq!(
            format!("{e:#}"),
            "it has no ObjectStream table, which the checker reads by name"
        );
    }

    /// A model with every table the checker reads by name but the
    /// linearization dictionary's is refused, naming it: the checker visits
    /// a linearized file's as it.
    #[test]
    fn a_model_without_the_linearization_dictionarys_table_is_refused() {
        let dir = tempfile::Builder::new()
            .prefix("pdf-lint-model-")
            .tempdir()
            .unwrap();
        let header = grammar::HEADER.join("\t");
        let row = "Rotate\tinteger\t1.0\t\tFALSE\tFALSE\tTRUE\t0\t\t\t\t";
        let named = grammar::NAMED.iter();
        for table in named.filter(|&&t| t != "LinearizationParameterDict") {
            let path = dir.path().join(format!("{table}.tsv"));
            std::fs::write(path, format!("{header}\n{row}\n")).unwrap();
        }
        let e = use_model(Grammar::load(dir.path()).unwrap()).unwrap_err();
        assert_eq!(
            format!("{e:#}"),
            "it has no LinearizationParameterDict table, which the checker reads by name"
        );
    }

    /// Once anything has read the model, none is chosen, and that's said
    /// word for word.
    #[test]
    fn a_model_is_chosen_before_anything_reads_one_or_not_at_all() {
        let _ = grammar();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pdf-arlington/model/tsv/latest");
        let e = use_model(Grammar::load(&dir).unwrap()).unwrap_err();
        assert_eq!(
            format!("{e:#}"),
            "the model is chosen already: something has read it"
        );
    }

    /// A password's encodings (pdf-crypt's [`Password::encodings`])
    /// take [`pdfium::pdf_doc_char`] for PDFDocEncoding exactly, though
    /// pdf-crypt keeps a copy of its own. With `Ł` after it, 0x95 in
    /// PDFDocEncoding and in neither WinAnsiEncoding nor
    /// MacRomanEncoding, a character a byte decodes to has its UTF-8
    /// and that byte's PDFDocEncoding, no more; the Latin-1 characters
    /// at the three bytes the table leaves undefined have their UTF-8
    /// alone. Their SASLprep forms, where they have one (˘'s is a space
    /// and a breve), are no table's, and left aside.
    #[test]
    fn the_pdfdoc_encoding_matches_this_table() {
        let forms = |typed: &str| -> Vec<(Encoding, Vec<u8>)> {
            (Password(typed).encodings().iter())
                .filter(|e| e.encoding() != Encoding::Saslprep)
                .map(|e| (e.encoding(), e.as_bytes().to_vec()))
                .collect()
        };
        for b in 0..=255 {
            if let Some(c) = pdf_doc_char(b) {
                assert_eq!(pdfium::pdf_doc_char(b), Some(c), "{b:#x}");
            }
            if let Some(c) = pdfium::pdf_doc_char(b) {
                let typed = format!("{c}Ł");
                let want = [
                    (Encoding::Utf8, typed.as_bytes().to_vec()),
                    (Encoding::PdfDoc, vec![b, 0x95]),
                ];
                assert_eq!(forms(&typed), want, "{b:#x}");
            }
        }
        for c in ['\u{7F}', '\u{9F}', '\u{AD}'] {
            let typed = format!("{c}Ł");
            let want = [(Encoding::Utf8, typed.as_bytes().to_vec())];
            assert_eq!(forms(&typed), want, "{c:?}");
        }
    }
}
