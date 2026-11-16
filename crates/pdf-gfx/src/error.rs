//! Errors returned by content interpretation and re-encoding.

use pdf_objects::{ObjectId, Unreadable};

use crate::limits::Cap;

/// Why an operator in [`encode`](crate::encode) cannot be re-encoded without
/// risking a different parse on read-back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Unwritable {
    /// A line-moving text operator moved the text line outside finite
    /// fixed-point `f32` representation.
    #[error("text line position exceeded representable fixed-point range")]
    Line,
    /// An inline image declares `/L` or `/Length` unequal to its parsed data
    /// length, so placing new operators after `EI` could shift where an
    /// `/L`-aware reader ends the image.
    #[error("inline image /L disagrees with its parsed data length")]
    InlineLength,
    /// An inline image produced no stream in PDFium
    /// ([`ImageStream::Missing`](pdf_content_streams::ImageStream::Missing)),
    /// or its stream end shifts when written out (for example, when a filter
    /// peeks past `EI` or a Form XObject without `/Resources` is drawn under
    /// conflicting inherited color-space component counts).
    #[error("inline image has no valid stream end")]
    InlineStream,
    /// A `BI` operator begins no inline image because a non-`ID` keyword
    /// appeared where a dictionary key was expected, so rewriting subsequent
    /// operators could turn it into an inline image on read-back.
    #[error("lone BI operator begins no inline image")]
    LoneBi,
}

/// Why [`interpret`](crate::interpret) or [`encode`](crate::encode) stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A content stream failed to lex or hit a `pdf-content-streams` cap in
    /// the page (`form: None`) or in a Form XObject (`form: Some(id)`).
    #[error("content stream {} failed to lex: {error}", fmt_form(*.form))]
    Content {
        /// The Form XObject stream being read, or `None` for the page's own
        /// content.
        form: Option<ObjectId>,
        /// Underlying `pdf-content-streams` error.
        #[source]
        error: pdf_content_streams::Error,
    },
    /// A Form XObject's stream could not be decoded (e.g., damaged filter data
    /// or an unsupported stream filter).
    #[error("form XObject {form} stream failed to decode: {source}")]
    FormStream {
        /// Object ID of the Form XObject.
        form: ObjectId,
        /// Underlying stream decode failure.
        #[source]
        source: Unreadable,
    },
    /// Re-encoding refused a page whose
    /// [`Concat::undecoded`](pdf_content_streams::Concat::undecoded) list is
    /// non-empty, preventing silent loss of undecoded page streams.
    #[error("page content has an undecoded stream: {0}")]
    PageStream(#[source] Unreadable),
    /// Re-encoding refused content containing an inline image whose byte length
    /// was sized under a color-space cap
    /// ([`InlineImage::capped`](pdf_content_streams::InlineImage::capped)).
    #[error("content {} contains an inline image sized under a color-space cap at byte {}", fmt_form(*.form), .at)]
    CappedInlineImage {
        /// Form XObject containing the capped inline image, or `None` for the
        /// page's own content.
        form: Option<ObjectId>,
        /// Start byte offset of the inline image in that stream.
        at: usize,
    },
    /// Re-encoding refused an operator that cannot be written back safely.
    #[error("{why} at byte {at} in content {}", fmt_form(*.form))]
    Unwritable {
        /// Form XObject containing the operator, or `None` for the page's own
        /// content.
        form: Option<ObjectId>,
        /// Byte offset of the operator in that stream.
        at: usize,
        /// Reason the operator cannot be re-encoded.
        why: Unwritable,
    },
    /// One of [`Limits`](crate::Limits)'s per-page caps was exceeded.
    #[error("exceeded cap on {0}")]
    Limit(Cap),
    /// Placed coordinates or matrix products overflowed finite `f32` geometry.
    #[error("placed geometry exceeded finite f32 range")]
    NotFinite,
    /// An object or stream could not be read for a caller-level reason (locked
    /// encryption, reader cap, detached handle, or I/O failure).
    #[error("{0}")]
    Unreadable(#[from] Unreadable),
}

fn fmt_form(form: Option<ObjectId>) -> String {
    match form {
        Some(id) => format!("of form {id}"),
        None => "of page".to_string(),
    }
}
