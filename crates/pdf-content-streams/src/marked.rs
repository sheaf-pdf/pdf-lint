//! Marked content (ISO 32000-2, 14.6): the ranges `BMC` and `BDC` open
//! and `EMC` ends, found across a page's streams, which are one stream to
//! marked content (14.6.1), each with its tag and property list. A named
//! property list is looked up in the lists the caller gives
//! ([`Properties`]), its resources' `/Properties` (14.6.2).
//!
//! Marked content must balance: an `EMC` that ends nothing, or a range
//! still open at the end, is an [`Error`], since where a range ends can't
//! be told from the content around it.

use std::collections::HashMap;

use pdf_objects::{Dict, Value};

use crate::error::{Error, ErrorKind};
use crate::inline::ResCx;
use crate::limits::Limits;
use crate::operand::Operand;
use crate::operations::Operation;
use crate::page::{Concat, SpanAcross};

/// The property lists named marked content names: a page's or a form's
/// `/Resources /Properties` (14.6.2), each list under its name, decoded,
/// without its `/`. They're the caller's to read, references resolved as
/// it reads them: no file is read here.
#[derive(Debug, Clone, Default)]
pub struct Properties {
    lists: HashMap<Vec<u8>, Dict>,
}

impl Properties {
    /// The lists given, each under its name; of a name given twice, the
    /// last list.
    pub fn new(lists: impl IntoIterator<Item = (Vec<u8>, Dict)>) -> Properties {
        Properties {
            lists: lists.into_iter().collect(),
        }
    }

    /// The list named `name`.
    pub fn get(&self, name: &[u8]) -> Option<&Dict> {
        self.lists.get(name)
    }

    /// Every list, in no order.
    pub fn lists(&self) -> impl Iterator<Item = &Dict> {
        self.lists.values()
    }
}

/// A marked range's property list.
#[derive(Debug, Clone, PartialEq)]
pub enum Props {
    /// Written in the content: `/Tag << … >> BDC`.
    Inline(Dict),
    /// Named there, `/Tag /Name BDC`: the name, decoded, and the list the
    /// resources have under it, if they have one.
    Named(Vec<u8>, Option<Dict>),
}

impl Props {
    /// The list itself, wherever it's written.
    pub fn dict(&self) -> Option<&Dict> {
        match self {
            Props::Inline(dict) => Some(dict),
            Props::Named(_, dict) => dict.as_ref(),
        }
    }
}

/// A marked range: from its tag, before `BMC` or `BDC`, through its `EMC`.
#[derive(Debug, Clone, PartialEq)]
pub struct Marked {
    /// Its tag, decoded, without its `/`: empty when what's before `BMC`
    /// or `BDC` there isn't a name.
    pub tag: Vec<u8>,
    /// None for `BMC`, or a `BDC` whose list is neither a dictionary nor
    /// a name.
    pub props: Option<Props>,
    /// Where it is: from the first operand of its `BMC` or `BDC` to the
    /// end of its `EMC`.
    pub span: SpanAcross,
    /// The ranges it's inside: 0 at the top.
    pub depth: u16,
}

/// `content`'s marked ranges, in the order they open, its operations read
/// with `res` under `limits`. Named lists are looked up in `props`.
///
/// ```
/// use pdf_content_streams::{Concat, Limits, Properties, Props, ResCx, marked};
/// use pdf_objects::dict;
///
/// let props = Properties::new([(b"P0".to_vec(), dict! { "Subtype" => "Header" })]);
/// let page = Concat::new(&[b"/Artifact /P0 BDC q", b"/Span BMC EMC Q EMC"]);
/// let found = marked(&page, &props, &ResCx::none(), &Limits::DEFAULT).unwrap();
/// assert_eq!(found.len(), 2);
/// assert_eq!((&found[0].tag[..], found[0].depth), (&b"Artifact"[..], 0));
/// // The first runs from the first stream into the second.
/// assert_eq!(found[0].span.to_string(), "stream 0 byte 0 to stream 1 byte 19");
/// let list = found[0].props.as_ref().and_then(Props::dict).unwrap();
/// assert!(list.get(b"Subtype").is_some());
/// assert_eq!((&found[1].tag[..], found[1].depth), (&b"Span"[..], 1));
/// ```
pub fn marked(
    content: &Concat,
    props: &Properties,
    res: &ResCx,
    limits: &Limits,
) -> Result<Vec<Marked>, Error> {
    let mut nest = MarkedNest::new(props, limits);
    for op in content.operations(res, limits) {
        nest.step(content, &op?)?;
    }
    nest.done()
}

/// Marked content as content is stepped through, an operation at a time
/// ([`step`](MarkedNest::step)): the ranges open, and every range found,
/// in the order they open. [`marked`] steps a page's; a caller stepping
/// through content for more than its marked ranges finds them in the same
/// pass, and is told as each range opens and ends ([`MarkedEvent`]).
///
/// ```
/// use pdf_content_streams::{Concat, Limits, MarkedEvent, MarkedNest, Properties, ResCx};
///
/// let props = Properties::default();
/// let page = Concat::new(&[b"/Artifact BMC 0 0 9 9 re f EMC"]);
/// let mut nest = MarkedNest::new(&props, &Limits::DEFAULT);
/// let mut events = Vec::new();
/// for op in page.operations(&ResCx::none(), &Limits::DEFAULT) {
///     match nest.step(&page, &op.unwrap()).unwrap() {
///         Some(MarkedEvent::Opened(i)) => events.push(("opened", i)),
///         Some(MarkedEvent::Ended(i)) => events.push(("ended", i)),
///         None => {}
///     }
/// }
/// assert_eq!(events, [("opened", 0), ("ended", 0)]);
/// let found = nest.done().unwrap();
/// assert_eq!(found[0].span.to_string(), "stream 0 byte 0 to stream 0 byte 30");
/// ```
#[derive(Debug, Clone)]
pub struct MarkedNest<'p> {
    props: &'p Properties,
    depth: usize,
    /// Each range open: where it is in `found`, and where it starts.
    open: Vec<(usize, usize)>,
    found: Vec<Marked>,
}

/// What an operation did to marked content: the range it opened or
/// ended, by its index among those found ([`MarkedNest::get`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkedEvent {
    /// A `BMC` or `BDC` opened it.
    Opened(usize),
    /// An `EMC` ended it.
    Ended(usize),
}

impl<'p> MarkedNest<'p> {
    /// None open and none found yet, named lists to be looked up in
    /// `props`, ranges nested up to [`Limits::depth`] deep, and property
    /// lists read up to it.
    pub fn new(props: &'p Properties, limits: &Limits) -> MarkedNest<'p> {
        MarkedNest {
            props,
            depth: limits.depth,
            open: Vec::new(),
            found: Vec::new(),
        }
    }

    /// Marked content after `op`, content's next operation, read from
    /// `content`: a range opened by a `BMC` or `BDC`, its span from its
    /// first operand and, until its `EMC`, empty; or ended by an `EMC`,
    /// its span then through it. An error for an `EMC` with none open
    /// ([`ErrorKind::Emc`]), or a range opened past [`Limits::depth`]
    /// ([`ErrorKind::Depth`]), where its operator is.
    pub fn step(&mut self, content: &Concat, op: &Operation) -> Result<Option<MarkedEvent>, Error> {
        let at = op.at();
        let name = op.name();
        if name == b"EMC" {
            let (index, start) = self.open.pop().ok_or(Error {
                at: at.start,
                kind: ErrorKind::Emc,
            })?;
            self.found[index].span = content.span(start..at.end);
            return Ok(Some(MarkedEvent::Ended(index)));
        }
        if name != b"BMC" && name != b"BDC" {
            return Ok(None);
        }
        if self.open.len() >= self.depth {
            return Err(Error {
                at: at.start,
                kind: ErrorKind::Depth,
            });
        }
        let (tag, props) = match name {
            b"BDC" => (op.arg(1), op.arg(0).and_then(|p| self.props(&p))),
            _ => (op.arg(0), None),
        };
        let start = op.range().start;
        let index = self.found.len();
        self.found.push(Marked {
            tag: tag
                .and_then(|t| t.name())
                .map(|t| t.into_owned())
                .unwrap_or_default(),
            props,
            span: content.span(start..start),
            depth: u16::try_from(self.open.len()).unwrap_or(u16::MAX),
        });
        self.open.push((index, start));
        Ok(Some(MarkedEvent::Opened(index)))
    }

    /// A `BDC`'s list, `operand` as written.
    fn props(&self, operand: &Operand) -> Option<Props> {
        if let Some(name) = operand.name() {
            let list = self.props.get(&name).cloned();
            return Some(Props::Named(name.into_owned(), list));
        }
        match operand.object(self.depth)?.into_value() {
            Value::Dict(dict) => Some(Props::Inline(dict)),
            _ => None,
        }
    }

    /// The range at `index` among those found, in the order they open:
    /// one still open spans its start alone.
    pub fn get(&self, index: usize) -> Option<&Marked> {
        self.found.get(index)
    }

    /// Every range found, once content has ended; an error when one is
    /// still open ([`ErrorKind::Marked`]), where the first of those
    /// starts.
    pub fn done(self) -> Result<Vec<Marked>, Error> {
        match self.open.first() {
            Some(&(_, start)) => Err(Error {
                at: start,
                kind: ErrorKind::Marked,
            }),
            None => Ok(self.found),
        }
    }
}
