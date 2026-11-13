//! What reading a file takes and gives, by name, in one `use`:
//! `use pdfs::prelude::*;`. The facade's own types, and the family's items
//! its calls take and give, each through the crate it's from, at the
//! versions this one is built with.

#[cfg(any(unix, windows))]
pub use crate::open;
pub use crate::{Content, Error, Limits, NamedFont, Options, Page, Pages, Pdf, from_bytes};

pub use pdf_crypt::{Encoded, Password};
pub use pdf_fonts::{Font, Shown, ShownString, Unread};
pub use pdf_objects::pdf_tokenizer::Rules;
pub use pdf_objects::version::Version;
pub use pdf_objects::{Dict, Object, ObjectId, Why};
pub use pdf_semantics::pages::Rect;
pub use pdf_structure::Encryption;
