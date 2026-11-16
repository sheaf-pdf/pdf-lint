//! Marked-content sequences (`BMC` / `BDC` / `EMC`) and points (`MP` / `DP`).
//!
//! Marked content is tracked across Form XObject invocations in PDFium's order:
//! - A `BDC` whose property list operand is neither an inline dictionary nor a
//!   name present in `/Resources/Properties` opens nothing, so its matching
//!   `EMC` closes the next outer sequence in that content stream;
//! - An `EMC` with no sequence open in the current content stream is ignored;
//! - Sequences left unclosed at the end of a content stream close implicitly
//!   there;
//! - A Form XObject inherits its caller's open [`Marks`] without being able to
//!   close them via its own `EMC`s.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use pdf_objects::text;
use pdf_objects::{Dict, Object, ObjectId};

use crate::xobject::UseId;

/// Property list attached to a [`Mark`] (ISO 32000-2, §14.6.2).
///
/// Exhaustive on purpose: a property list is either written inline (`<< … >>`)
/// or named in `/Resources/Properties`. Its `Debug` implementation reports
/// entry counts and name byte lengths rather than printing raw dictionary text.
#[derive(Clone, PartialEq)]
pub enum MarkProps {
    /// Written inline in the content stream: `/Tag << … >> BDC` or `DP`.
    Inline(Dict),
    /// Named in `/Resources/Properties`: `/Tag /Name BDC` or `DP`, holding the
    /// decoded resource name and the resolved property dictionary (if present).
    Named(Vec<u8>, Option<Dict>),
}

impl MarkProps {
    /// Returns the property dictionary regardless of whether it was written
    /// inline or named in `/Resources/Properties`.
    #[must_use]
    pub const fn dict(&self) -> Option<&Dict> {
        match self {
            Self::Inline(dict) => Some(dict),
            Self::Named(_, dict) => dict.as_ref(),
        }
    }
}

impl fmt::Debug for MarkProps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline(dict) => f
                .debug_struct("Inline")
                .field("entries", &dict.entries().len())
                .finish(),
            Self::Named(name, dict) => f
                .debug_struct("Named")
                .field("name_len", &name.len())
                .field("entries", &dict.as_ref().map(|d| d.entries().len()))
                .finish(),
        }
    }
}

/// One marked-content sequence (`BMC`, `BDC`) or marked-content point (`MP`,
/// `DP`).
///
/// Its `Debug` implementation reports `tag_len` and `actual_text_len` rather
/// than printing raw tag names or document text.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct Mark {
    /// Decoded tag name bytes (e.g., `b"P"`, `b"Span"`, `b"Artifact"`, `b"OC"`).
    pub tag: Vec<u8>,
    /// Property list (`None` for `BMC` and `MP`).
    pub props: Option<MarkProps>,
    /// Structure-tree marked-content identifier (`/MCID`), when a non-negative
    /// `u32` integer in the property list.
    pub mcid: Option<u32>,
    /// Replacement text (`/ActualText`), decoded using PDFium's text string
    /// rules, when present as a string in the property list.
    pub actual_text: Option<String>,
    /// Optional content group or membership dictionary ID when `tag == b"OC"`
    /// and the named property list in `/Resources/Properties` is an indirect
    /// reference.
    pub oc: Option<ObjectId>,
    /// Enclosing Form XObject use in which this mark appeared, or `None` in the
    /// page's own content stream.
    pub stream: Option<UseId>,
    /// Byte span of the opening `BMC`, `BDC`, `MP`, or `DP` operation within
    /// `stream`.
    pub at: Range<usize>,
}

impl fmt::Debug for Mark {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mark")
            .field("tag_len", &self.tag.len())
            .field("has_props", &self.props.is_some())
            .field("mcid", &self.mcid)
            .field(
                "actual_text_len",
                &self.actual_text.as_ref().map(String::len),
            )
            .field("oc", &self.oc)
            .field("stream", &self.stream)
            .field("at", &self.at)
            .finish()
    }
}

impl Mark {
    pub(crate) fn new(
        tag: Vec<u8>,
        props: Option<MarkProps>,
        named_entry: Option<&Object>,
        stream: Option<&UseId>,
        at: Range<usize>,
    ) -> Self {
        let dict: Option<&Dict> = props.as_ref().and_then(MarkProps::dict);
        let mcid = dict
            .and_then(|d| d.get(b"MCID"))
            .and_then(Object::as_integer)
            .and_then(|n| u32::try_from(n).ok());
        let actual_text = dict
            .and_then(|d| d.get(b"ActualText"))
            .and_then(Object::as_bytes)
            .map(|bytes| text::pdfium::decode(bytes).text);
        let oc = named_entry
            .filter(|_| tag == b"OC")
            .and_then(Object::as_reference);
        Self {
            tag,
            props,
            mcid,
            actual_text,
            oc,
            stream: stream.cloned(),
            at,
        }
    }

    /// Returns `true` when `self.tag == b"Artifact"`.
    #[must_use]
    pub fn is_artifact(&self) -> bool {
        self.tag == b"Artifact"
    }
}

/// Open marked-content sequences enclosing an [`Event`](crate::Event), across
/// enclosing Form XObjects, ordered innermost first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Marks(Option<Arc<Node>>);

#[derive(Debug, PartialEq)]
struct Node {
    mark: Mark,
    outer: Marks,
}

/// Iterator over open [`Mark`]s in [`Marks`], innermost first.
#[derive(Debug, Clone)]
pub struct MarksIter<'a> {
    next: Option<&'a Node>,
}

impl<'a> Iterator for MarksIter<'a> {
    type Item = &'a Mark;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.next?;
        self.next = node.outer.0.as_deref();
        Some(&node.mark)
    }
}

impl std::iter::FusedIterator for MarksIter<'_> {}

impl Marks {
    /// Returns `true` if no marked-content sequence is currently open.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Iterates open [`Mark`]s from innermost to outermost.
    #[must_use]
    pub fn iter(&self) -> MarksIter<'_> {
        MarksIter {
            next: self.0.as_deref(),
        }
    }

    /// Returns `true` if any open [`Mark`] is tagged `/Artifact`.
    #[must_use]
    pub fn in_artifact(&self) -> bool {
        self.iter().any(Mark::is_artifact)
    }

    /// Innermost `/MCID` present on an open [`Mark`], if any.
    #[must_use]
    pub fn innermost_mcid(&self) -> Option<u32> {
        self.iter().find_map(|m| m.mcid)
    }

    pub(crate) fn push(&mut self, mark: Mark) {
        let outer = std::mem::take(self);
        *self = Marks(Some(Arc::new(Node { mark, outer })));
    }

    pub(crate) fn pop(&mut self) -> Option<Closed> {
        let node = self.0.take()?;
        self.0.clone_from(&node.outer.0);
        Some(Closed(node))
    }
}

impl<'a> IntoIterator for &'a Marks {
    type Item = &'a Mark;
    type IntoIter = MarksIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub(crate) struct Closed(Arc<Node>);

impl Closed {
    pub(crate) fn mark(&self) -> &Mark {
        &self.0.mark
    }
}

impl Drop for Marks {
    fn drop(&mut self) {
        let mut next = self.0.take();
        while let Some(node) = next {
            next = Arc::try_unwrap(node)
                .ok()
                .and_then(|mut node| node.outer.0.take());
        }
    }
}
