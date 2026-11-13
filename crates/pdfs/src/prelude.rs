//! What reading and writing a file take and give, by name, in one `use`:
//! `use pdfs::prelude::*;`. The facade's own types and calls, and the
//! family's items they take and give, each through the crate it's from, at
//! the versions this one is built with: what a page draws with and in, what
//! a file says of itself, how it's encrypted, and what's in it.

#[cfg(any(unix, windows))]
pub use crate::open;
pub use crate::{
    Content, Document, Error, Limits, NamedFont, Options, Page, Pages, Pdf, from_bytes,
    standard_font,
};

pub use pdf_content_streams::{Color, ContentWriter, Paint, Res};
pub use pdf_crypt::{Encoded, Password, Permissions, Protection};
pub use pdf_fonts::{Font, Shown, ShownString, Std14, Unread};
pub use pdf_linearize::check::Verdict;
pub use pdf_objects::pdf_tokenizer::Rules;
pub use pdf_objects::version::Version;
pub use pdf_objects::{Dict, Object, ObjectId, Why};
pub use pdf_semantics::info::Info;
pub use pdf_semantics::pages::Rect;
pub use pdf_semantics::xmp::Claims;
pub use pdf_structure::Encryption;
