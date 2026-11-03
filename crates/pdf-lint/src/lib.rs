//! Checks a PDF file's objects against the Arlington PDF Model, the PDF
//! Association's machine-readable grammar of ISO 32000-2.
//!
//! The model is pdf-arlington's, embedded there (its `model/`, whose
//! `SOURCE.md` says from where): a TSV table per object, a row per key or
//! array element, saying its types, the PDF version it came in and any it
//! was deprecated in, whether it's required, the values it may take and
//! the tables its value links to. The tables' layout is upstream's
//! README.md, "TSV Data Overview"; the rules for each column are
//! INTERNAL_GRAMMAR.md's "TSV file rules" and "TSV Data Fields", from
//! "Column 1 - Key" to "Column 11 - Link". [`grammar()`] reads them.
//!
//! What the walk reaches that holds content, each page, form XObject,
//! tiling pattern and Type 3 glyph, has its content read too, where the
//! source gives streams' data ([`bytes::WithReading`]), as PDFium reads it,
//! through pdf-content-streams, and held to ISO 32000-2 itself, which the
//! model doesn't describe: `q` and `Q` balanced, marked content ended,
//! each operator where the graphics objects allow it and with the operands
//! it takes, each resource name in the resources, what they hold that
//! nothing uses, and each place ISO reads the content otherwise than
//! PDFium ([`check()`] says how).
//!
//! The page tree, and each name and number tree the walk reaches, are read
//! through pdf-semantics too, and held to ISO 32000-2 where the model can't
//! say what's wrong; the pages the page tree's walk reads are what the
//! model's `fn:NumberOfPages` and `fn:PageProperty` read, and where PDFium
//! reads other pages than ISO's, or reads their sizes otherwise, is told as
//! a viewer's reading. The catalog's XMP metadata, read for the extensions
//! a file declares, is held to ISO 32000-2 too: what isn't UTF-8 or XML as
//! 14.3.2 has it is told at its bytes in the metadata stream ([`check()`]
//! says how).
//!
//! Each color space and font the walk reaches is read again, through
//! pdf-color and pdf-fonts, the fonts for their text, no program read, and
//! each held to ISO 32000-2 where the model can't say what's wrong
//! ([`check()`] says how). What the model asks that only they can say is
//! read through them too: which fonts have Latin characters, read before
//! the walk, that a descriptor's `/CapHeight` is required of, and what a
//! DeviceN space's or a halftone's colorants' names say of them.

pub mod allowance;
pub mod check;
#[cfg(any(test, fuzzing))]
pub mod fuzz;

pub use allowance::{Allowance, AllowanceError, Allowances};
pub use check::{
    Class, Exempt, ExemptError, Finding, Hold, HoldError, Made, Options, Report, Rule, Unchecked,
    Unlisted, UnlistedIn, check, file_version,
};
// `grammar` is the module and the function both.
pub use grammar::{ArlType, At, Grammar, Row, Table, Wrap, WrapFn, Wrapped, grammar, use_model};
pub use object::{Data, Dict, Entry, Form, Object, ObjectId, Span, Str, Stream, Value};
// The model, its tables and predicates, and the commit it's at, are
// pdf-arlington's; which model every check reads is this crate's.
pub use pdf_arlington::{COMMIT, predicate};
// The objects, `dict!`, versions and what resolves a reference are
// pdf-objects'.
pub use pdf_objects::{Resolve, Rules, Unreadable, Why, dict, object, version};
// The reader, and what it reads a file into, are pdf-structure's; the
// error a stream's data that doesn't decode cleanly gives is pdf-filters',
// and the passwords a reader is given and its security handler are
// pdf-crypt's.
pub use pdf_structure::{bytes, pdf_crypt, pdf_filters, source};
pub use predicate::{Cx, FileCx, Gate, Latin, Reference, Since, Truth};
pub use source::{Document, Encryption, Revision, Source, Trailer, Update, Warning};
pub use version::Version;

/// The model's tables, pdf-arlington's [`grammar`](pdf_arlington::grammar)
/// module, and which model every check reads: the embedded one, or one
/// [`use_model`] chose.
pub mod grammar {
    use std::sync::OnceLock;

    pub use pdf_arlington::grammar::*;

    /// The model [`use_model`] chose; none once anything has read the
    /// embedded one instead.
    static CHOSEN: OnceLock<Option<Grammar>> = OnceLock::new();

    /// The tables the checker reads by name, which a model must have: the
    /// trailer's two; the object streams', which nothing in the model links
    /// to but an object stream, so the checker visits each as it; and
    /// those it checks a metadata stream and associated files as where the
    /// model has no row for them. A slice, not an array of a fixed length,
    /// so a table can come without a breaking release.
    pub const NAMED: &[&str] = &[
        "FileTrailer",
        "XRefStream",
        "ObjectStream",
        "Metadata",
        "ArrayOfFileSpecifications",
    ];

    /// The model every check reads: the one [`use_model`] chose, else the
    /// embedded one ([`Grammar::embedded`]), read once. Panics, naming the
    /// file and line, when the embedded one doesn't read, which its tests
    /// rule out.
    pub fn grammar() -> &'static Grammar {
        CHOSEN
            .get_or_init(|| None)
            .as_ref()
            .unwrap_or_else(Grammar::embedded)
    }

    /// Makes `model` the one every check reads, in place of the embedded
    /// one: a newer checkout of upstream's `tsv/latest`, read with
    /// [`Grammar::load`]. Before anything has read the model, and once. A
    /// [`Refused`] says when something has, or when `model` lacks a table
    /// the checker reads by name ([`NAMED`]).
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

/// Text strings, pdf-objects' [`text`](pdf_objects::text).
pub mod text {
    pub use pdf_objects::text::*;

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::pdf_crypt::{Encoding, Password};

        /// A password's encodings (pdf-crypt's [`Password::encodings`])
        /// take this table for PDFDocEncoding exactly, though pdf-crypt
        /// keeps a copy of its own. With `Ł` after it, 0x95 in
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
}

/// The extensions a file declares, read from a [`Source`]
/// ([`declared`](extension::declared)), and those the model names
/// ([`named`](extension::named)): pdf-arlington's
/// [`extension`](pdf_arlington::extension).
pub mod extension {
    use std::collections::BTreeSet;

    pub use pdf_arlington::extension::ISO;

    use pdf_arlington::extension::Claims;
    use pdf_semantics::{Limits, xmp};

    use crate::check::{Objects, Packet};
    use crate::source::Source;

    /// Every extension the model names, in its cells and around its
    /// types, values and links: what a file can be checked under. It's
    /// [`Grammar::extensions`](crate::Grammar::extensions) of the model
    /// every check reads, [`grammar()`](crate::grammar()).
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
        let result = match source.metadata() {
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
        use pdf_semantics::Fault;

        use super::*;
        use crate::dict;
        use crate::object::Object;
        use crate::source::Document;
        use crate::version::Version;

        /// A document's catalog, through its trailer's reference, and its
        /// metadata, both declare.
        #[test]
        fn a_document_declares_through_its_catalog_and_metadata() {
            let mut doc =
                Document::new(Version::new(2, 0), dict! { "Root" => Object::reference(1) });
            let iso = dict! { "BaseVersion" => "2.0", "ExtensionLevel" => 32001 };
            doc.insert(
                1,
                dict! { "Type" => "Catalog", "Extensions" => dict! { "ISO_" => iso } },
            );
            assert_eq!(declared(&doc), BTreeSet::from(["ISO_TS_32001".to_string()]));
            assert_eq!(declared(&doc), quietly(&doc));
            doc.metadata = Some(xmp(r#"pdfaid:part="3""#));
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
            pdf_arlington::extension::declared(doc.trailer.dict().get(b"Root"), claims, &resolve)
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
            let catalog = dict! { "Type" => "Catalog", "Metadata" => Object::reference(5) };
            doc.insert(1, catalog);
            let stream = |data: &[u8]| {
                let dict = dict! { "Type" => "Metadata", "Subtype" => "XML" };
                crate::object::Stream::new(dict, data.to_vec())
            };
            let both = ["ISO_19005_3", "PDF_VT2"].map(String::from);
            for (data, faults) in [(&clean, 0), (&faulty, 1)] {
                doc.insert(5, stream(data));
                doc.metadata = Some(data.clone());
                let (claimed, got) = read(&doc);
                assert_eq!(claimed, BTreeSet::from(both.clone()));
                assert_eq!(claimed, declared(&doc));
                assert_eq!(claimed, quietly(&doc));
                assert_eq!(got.stream, Some((5, 0)));
                assert_eq!(got.len, data.len());
                assert!(got.stopped.is_none());
                assert_eq!(got.kept.told().len(), faults);
                for (place, fault, count) in got.kept.told() {
                    assert_eq!(*fault, Fault::XmpAttribute);
                    assert_eq!(*count, 1);
                    assert_eq!(place.object, Some((5, 0)));
                    let at = place.at.unwrap();
                    assert_eq!(at.stream, Some((5, 0)));
                    assert!(data[at.start..at.end].starts_with(b"<rdf:Description"));
                }
            }
            // With no catalog to name its stream, a fault's in no object.
            let mut alone = Document::new(Version::new(2, 0), dict! {});
            alone.metadata = Some(faulty.clone());
            let (claimed, got) = read(&alone);
            assert_eq!(claimed, BTreeSet::from(both.clone()));
            assert_eq!(got.stream, None);
            assert_eq!(got.kept.told().len(), 1);
            let place = got.kept.told()[0].0;
            assert_eq!((place.object, place.at), (None, None));
            // What stops the read, with what's claimed before it.
            let stops = [packet(r#"pdfaid:part="3""#, ""), b"</e>".to_vec()].concat();
            alone.metadata = Some(stops);
            let (claimed, got) = read(&alone);
            assert_eq!(claimed, BTreeSet::from(["ISO_19005_3".to_string()]));
            assert_eq!(claimed, quietly(&alone));
            assert!(matches!(got.stopped.unwrap().why, xmp::XmpWhy::Xml(_)));
            // With no packet, nothing.
            alone.metadata = None;
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
            doc.metadata = Some(packet.to_vec());
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{Grammar, grammar, use_model};

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

    /// Once anything has read the model, none is chosen, and that's said
    /// word for word.
    #[test]
    fn a_model_is_chosen_before_anything_reads_one_or_not_at_all() {
        grammar();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pdf-arlington/model/tsv/latest");
        let e = use_model(Grammar::load(&dir).unwrap()).unwrap_err();
        assert_eq!(
            format!("{e:#}"),
            "the model is chosen already: something has read it"
        );
    }
}
