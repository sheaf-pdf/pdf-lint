//! Checks a PDF file's objects against the Arlington PDF Model, the PDF
//! Association's machine-readable grammar of ISO 32000-2.
//!
//! The model is embedded (`model/`, whose `SOURCE.md` says from where): a
//! TSV table per object, a row per key or array element, saying its types,
//! the PDF version it came in and any it was deprecated in, whether it's
//! required, the values it may take and the tables its value links to. The
//! tables' layout is upstream's README.md, "TSV Data Overview"; the rules
//! for each column are INTERNAL_GRAMMAR.md's "TSV file rules" and "TSV Data
//! Fields", from "Column 1 - Key" to "Column 11 - Link". [`grammar()`]
//! reads them.

pub mod allowance;
pub mod bytes;
pub mod check;
pub mod extension;
mod filter;
#[cfg(any(test, fuzzing))]
pub mod fuzz;
pub mod grammar;
#[cfg(test)]
mod json;
pub mod predicate;
// qpdf, the tests' oracle: a second reader, and a decoder to set the
// reader's decoding beside.
#[cfg(test)]
pub(crate) mod qpdf;
pub mod source;
#[cfg(any(test, fuzzing))]
#[cfg_attr(fuzzing, allow(dead_code))]
pub(crate) mod write;
mod xxh64;

pub use allowance::{Allowance, Allowances};
pub use check::{
    Class, Exempt, Finding, Hold, Made, Options, Report, Rule, Unchecked, Unlisted, UnlistedIn,
    check, file_version,
};
pub use grammar::{ArlType, At, Grammar, Row, Table, Wrap, WrapFn, Wrapped, grammar, use_model};
pub use object::{Dict, Entry, Object, ObjectId, Span, Str, Stream, Value};
// The objects, `dict!` and versions are pdf-objects'.
pub use pdf_objects::{dict, object, version};
pub use predicate::{Cx, Gate, Reference, Since, Truth};
pub use source::{Document, Encryption, Source, Trailer, Update, Warning};
pub use version::Version;

/// The upstream commit the embedded model is (`model/SOURCE.md`).
pub const COMMIT: &str = "d148fee5a0cb6dcacadf58c09ed842e32847024d";

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
