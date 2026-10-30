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
pub use pdf_objects::{Resolve, dict, object, version};
// The reader, and what it reads a file into, are pdf-structure's; the
// error a stream's data that doesn't decode cleanly gives is pdf-filters',
// and the passwords a reader is given and its security handler are
// pdf-crypt's.
pub use pdf_structure::{bytes, pdf_crypt, pdf_filters, source};
pub use predicate::{Cx, FileCx, Gate, Reference, Since, Truth};
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
    /// model has no row for them.
    pub const NAMED: [&str; 5] = [
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
        for name in NAMED {
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
    /// reads them.
    pub fn declared(source: &dyn Source) -> BTreeSet<String> {
        let resolve = |id| source.get(id).ok().cloned();
        let root = source.trailer().dict().get(b"Root");
        pdf_arlington::extension::declared(root, source.metadata(), &resolve)
    }

    #[cfg(test)]
    mod tests {
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
            doc.metadata = Some(br#"<rdf:Description pdfaid:part="3"/>"#.to_vec());
            let both = ["ISO_19005_3", "ISO_TS_32001"].map(String::from);
            assert_eq!(declared(&doc), BTreeSet::from(both));
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
