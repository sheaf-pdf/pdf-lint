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
pub mod bytes;
pub mod check;
mod filter;
#[cfg(any(test, fuzzing))]
pub mod fuzz;
#[cfg(test)]
mod json;
// qpdf, the tests' oracle: a second reader, and a decoder to set the
// reader's decoding beside.
#[cfg(test)]
pub(crate) mod qpdf;
pub mod source;
#[cfg(any(test, fuzzing))]
#[cfg_attr(fuzzing, allow(dead_code))]
pub(crate) mod write;

pub use allowance::{Allowance, Allowances};
pub use check::{
    Class, Exempt, Finding, Hold, Made, Options, Report, Rule, Unchecked, Unlisted, UnlistedIn,
    check, file_version,
};
pub use grammar::{ArlType, At, Grammar, Row, Table, Wrap, WrapFn, Wrapped, use_model};
pub use object::{Dict, Entry, Form, Object, ObjectId, Span, Str, Stream, Value};
// The model, its tables and predicates, and the commit it's at, are
// pdf-arlington's; `grammar` is the module and the function both.
pub use pdf_arlington::{COMMIT, grammar, predicate};
// The objects, `dict!` and versions are pdf-objects'.
pub use pdf_objects::{dict, object, version};
pub use predicate::{Cx, Gate, Reference, Since, Truth};
pub use source::{Document, Encryption, Source, Trailer, Update, Warning};
pub use version::Version;

/// Text strings, pdf-objects' [`text`](pdf_objects::text), and a typed
/// password's byte forms, pdf-crypt's [`passwords`](pdf_crypt::passwords).
pub mod text {
    pub use pdf_objects::text::*;

    // A typed password's byte forms, in PDFDocEncoding among others, are
    // pdf-crypt's, beside the security handler they open.
    pub use pdf_crypt::passwords;

    #[cfg(test)]
    mod tests {
        use super::*;

        /// `passwords` encodes by this table exactly, though pdf-crypt keeps a
        /// copy of its own. With `Ł` after it, 0x95 in PDFDocEncoding and in
        /// neither WinAnsiEncoding nor MacRomanEncoding, a character a byte
        /// decodes to has its UTF-8 and that byte's form, no more; the Latin-1
        /// characters at the three bytes the table leaves undefined have their
        /// UTF-8 alone.
        #[test]
        fn passwords_encode_by_this_table() {
            for b in 0..=255 {
                if let Some(c) = pdf_doc_char(b) {
                    let typed = format!("{c}Ł");
                    let want = [typed.as_bytes().to_vec(), vec![b, 0x95]];
                    assert_eq!(passwords(&typed), want, "{b:#x}");
                }
            }
            for c in ['\u{7F}', '\u{9F}', '\u{AD}'] {
                let typed = format!("{c}Ł");
                assert_eq!(passwords(&typed), [typed.as_bytes().to_vec()], "{c:?}");
            }
        }
    }
}

/// The extensions a file declares, read from a [`Source`]
/// ([`declared`](extension::declared)), and those the model names:
/// pdf-arlington's [`extension`](pdf_arlington::extension).
pub mod extension {
    use std::collections::BTreeSet;

    pub use pdf_arlington::extension::{ISO, named};

    use crate::source::Source;

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
    /// ARLINGTON-NOTICE.txt, which the package carries for the parts of the
    /// checker adapted from TestGrammar, is upstream's NOTICE.txt as
    /// pdf-arlington's model has it, byte for byte.
    #[test]
    fn the_arlington_notice_is_the_models() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let notice = std::fs::read(dir.join("ARLINGTON-NOTICE.txt")).unwrap();
        let model = std::fs::read(dir.join("../pdf-arlington/model/NOTICE.txt")).unwrap();
        assert!(
            notice == model,
            "crates/pdf-lint/ARLINGTON-NOTICE.txt isn't pdf-arlington's model/NOTICE.txt: copy it again"
        );
    }
}
