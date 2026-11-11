//! The page tree (ISO 32000-2, 7.7.3): every page a file's tree lists, in
//! order, with what each inherits and where each value came from. Walked
//! without reading any page's content, and iteratively, so a deep tree is
//! never deep on the stack.
//!
//! [`pages`] walks it by one of two sets of rules, and on a well-formed tree
//! they give the same pages, each object in the same place, though not always
//! the same values: a `/MediaBox null` is one that's absent to ISO's, which
//! inherit one, and one that's there to PDFium's, which take it for none:
//!
//! - **PDFium's** ([`Rules::Pdfium`]), as of chromium/8125, a development
//!   snapshot of PDFium that the crate's tests hold them to, so that a page's
//!   number is PDFium's number for it. Depth first through `/Kids`, a kid
//!   with `/Kids` a node whatever its `/Type`; any other dictionary or stream
//!   a page, written in place or not, and any other kid a place with no page
//!   at it, which takes its number all the same. The walk stops at the count
//!   PDFium gives ([`Pages::counted`]): the root's `/Count` where that's
//!   positive and under its cap, and otherwise a count of the tree, in which a
//!   node's own `/Count` is trusted in the same way. A leaf whose `/Type` isn't
//!   `/Page` loads no page, unless the count of the tree counted it as one.
//!   A reference names what PDFium finds through it: one reference, or for
//!   `/Pages`, `/Parent` and `/Rotate`, two. Where PDFium goes on quietly past
//!   a node listed inside itself, a `/Parent` chain that loops or a tree past
//!   the caps, the walk fails instead.
//! - **ISO's** ([`Rules::Iso`]): 7.7.3, reading on past whatever departs from
//!   it, each told to the [`Sink`]. `/Type` tells a node from a page, and where
//!   it says neither, `/Kids` does. References are followed through any chain,
//!   null is what's absent (7.3.7), and the walk never stops at a `/Count`,
//!   which it holds to what's under each node if the sink wants faults. Only
//!   the caps and a catalog with no `/Pages` are errors.
//!
//! The inheritable attributes come down the `/Parent` chain from the page
//! (7.7.3.4), which needn't be the path the walk took to it, so a page object
//! reads the same wherever it's listed. [`Page::attribute`] gives one as
//! written and followed, and [`Page::media_box`] and the like one as a value,
//! each with its [`Origin`]. A page object listed twice is two pages, and what
//! a page holds is borrowed from what the resolver keeps.
//!
//! An object that doesn't read ([`Why::Damaged`]) is one
//! that isn't there, told as [`Fault::Damaged`]. One past a cap the reader
//! reads under, one that takes a password, or bytes the source didn't give fail
//! the walk ([`Error::Unreadable`]), as does a chain of references too long to
//! follow.
//!
//! [`write()`] writes a tree's nodes over pages a caller has, balanced, each
//! numbered by the caller, as [`pages`] reads them back by either rules.
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_semantics::Limits;
//! use pdf_semantics::pages::{Origin, Rect, pages};
//! use pdf_semantics::pdf_objects::pdf_tokenizer::Rules;
//! use pdf_semantics::pdf_objects::{Object, ObjectId, dict};
//!
//! let objects = BTreeMap::from([
//!     (ObjectId::new(2, 0), Object::from(dict! {
//!         "Type" => "Pages",
//!         "Kids" => vec![Object::reference(3)],
//!         "Count" => 1,
//!         "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(200)],
//!     })),
//!     (ObjectId::new(3, 0), Object::from(dict! { "Type" => "Page", "Parent" => Object::reference(2) })),
//! ]);
//! let resolve = |id: ObjectId| objects.get(&id);
//! let catalog = dict! { "Pages" => Object::reference(2) };
//!
//! let table = pages(resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
//! assert_eq!(table.len(), 1);
//! let page = table.get(0).unwrap();
//! assert_eq!(page.id, Some(ObjectId::new(3, 0)));
//! let media = page.media_box();
//! assert_eq!(media.value, Rect([0.0, 0.0, 100.0, 200.0]));
//! assert_eq!(media.origin, Origin::Ancestor(Some(ObjectId::new(2, 0))));
//! assert_eq!(page.rotate().origin, Origin::Default);
//! ```

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::iter::FusedIterator;
use std::rc::Rc;
use std::sync::Arc;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Unreadable, Value, Why};

use crate::fault::Fault;
use crate::limits::Limits;
use crate::phases::phase;
use crate::{HOPS, too_long};

/// PDFium's cap on what it counts: past this, a file's page count is none.
/// The root's or a node's `/Count` is counted by if it's positive and under.
const PDFIUM_PAGE_MAX: i64 = 0xFFFFF;

/// How many kids a pass over the tree looks at, for each of [`Limits::pages`]:
/// a kid that gives neither a page nor a node costs as one that does, and in a
/// node listed again, adds up to the square of what's written. Those that do
/// give one are at most two for each, as pages and nodes have that cap each.
const KIDS_A_PAGE: usize = 4;

/// The most room a table starts with, however many pages a `/Count` claims:
/// a file of a hundred bytes can claim a million.
const ROOM: usize = 4096;

/// The pages of the tree at `catalog`'s `/Pages`, by `rules`, as the module
/// says. What departs from ISO 32000-2 is told to `sink` under ISO's rules;
/// under PDFium's, only what doesn't read is.
///
/// # Errors
///
/// An [`Error`], and no table: [`Error::NoPages`] for a catalog with no
/// `/Pages` that's a dictionary, and by PDFium's rules for no page under it;
/// [`Error::TooDeep`] and [`Error::TooManyPages`] for a tree past the caps;
/// by PDFium's rules [`Error::Cycle`] and [`Error::ParentLoop`]; and
/// [`Error::Unreadable`] for an object on the way past a cap the reader
/// reads under, that takes a password, or whose bytes the source didn't
/// give, or a chain of references too long to follow.
pub fn pages<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Pages<'o>, Error> {
    phase!(DEBUG, "pages", pdfium = matches!(rules, Rules::Pdfium));
    let walk = walked(objects, catalog, rules, limits, sink)?;
    if walk.pdfium && walk.pages.is_empty() {
        return Err(Error::NoPages);
    }
    let counted = walk.count.unwrap_or(walk.pages.len());
    Ok(Pages {
        pages: walk.pages,
        counted,
    })
}

/// The count of pages PDFium gives the tree at `catalog`'s `/Pages`, as
/// [`Pages::counted`] is under [`Rules::Pdfium`], also where [`pages`] by
/// those rules gives [`Error::NoPages`] for no page under the root: PDFium
/// gives its count all the same, the root's `/Count` where it goes by one,
/// and loads no page. 0 where the catalog has no `/Pages` that's a
/// dictionary; any other error as [`pages`] gives it.
///
/// # Errors
///
/// As [`pages`] fails by PDFium's rules, but never [`Error::NoPages`].
pub fn counted<'o, R: Resolve<'o>>(
    objects: R,
    catalog: &'o Dict,
    limits: &Limits,
) -> Result<usize, Error> {
    phase!(DEBUG, "counted");
    match walked(objects, catalog, Rules::Pdfium, limits, &mut ()) {
        Ok(walk) => Ok(walk.count.unwrap_or(walk.pages.len())),
        Err(Error::NoPages) => Ok(0),
        Err(error) => Err(error),
    }
}

/// The tree at `catalog`'s `/Pages` walked by `rules`, as [`pages`] walks
/// it, but by PDFium's rules a walk that found no page under the root.
fn walked<'o, 'r, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &'r mut S,
) -> Result<Walk<'o, 'r, R, S>, Error> {
    let mut walk = Walk {
        objects,
        sink,
        limits: *limits,
        pdfium: matches!(rules, Rules::Pdfium),
        count: None,
        nodes: HashMap::new(),
        listed: HashMap::new(),
        counted_as_pages: HashSet::new(),
        seen_nodes: HashSet::new(),
        pages: Vec::new(),
        visits: 0,
        kids_looked_at: 0,
        object: None,
    };
    walk.table(catalog)?;
    Ok(walk)
}

/// A file's pages, in order, as [`pages`] gives them.
#[derive(Debug, Clone, PartialEq)]
pub struct Pages<'o> {
    pages: Vec<Page<'o>>,
    counted: usize,
}

impl<'o> Pages<'o> {
    /// How many pages the tree lists, as far as the walk went.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.pages.len()
    }

    /// Whether it lists none, which only ISO's rules can give: PDFium's make
    /// that [`Error::NoPages`], and [`counted`](fn@counted) says what PDFium
    /// counts then.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Page `index`, from 0; none past the last the tree lists.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Page<'o>> {
        self.pages.get(index)
    }

    /// Each page, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, Page<'o>> {
        self.pages.iter()
    }

    /// The count of pages PDFium gives under its rules, which may be more than
    /// the tree lists, as a root's `/Count` past the tree is: past [`len`](Self::len),
    /// PDFium loads no page. Under ISO's rules, [`len`](Self::len).
    #[must_use]
    pub const fn counted(&self) -> usize {
        self.counted
    }
}

impl<'a, 'o> IntoIterator for &'a Pages<'o> {
    type Item = &'a Page<'o>;
    type IntoIter = std::slice::Iter<'a, Page<'o>>;

    fn into_iter(self) -> Self::IntoIter {
        self.pages.iter()
    }
}

/// A page, as the tree lists it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Page<'o> {
    /// Its object: none for a dictionary written in place, which has no
    /// number, or a kid that isn't a reference.
    pub id: Option<ObjectId>,
    /// Its dictionary, or a stream's: none for a place where PDFium loads no
    /// page, which has none of the attributes and only the defaults.
    pub dict: Option<&'o Dict>,
    /// How often the tree listed this object before: 0 the first time, 1 the
    /// second.
    pub repeat: u32,
    parent: Option<Arc<Parent>>,
    attributes: [Option<Attribute<'o>>; 4],
    media_box: Sourced<Rect>,
    crop_box: Sourced<Rect>,
    rotate: Sourced<i64>,
    user_unit: Sourced<f64>,
}

impl<'o> Page<'o> {
    /// A place with no page at it.
    fn none(id: Option<ObjectId>) -> Page<'o> {
        let default = |value| Sourced {
            value,
            origin: Origin::Default,
        };
        Page {
            id,
            dict: None,
            repeat: 0,
            parent: None,
            attributes: [None; 4],
            media_box: default(Rect::LETTER),
            crop_box: default(Rect::LETTER),
            rotate: Sourced {
                value: 0,
                origin: Origin::Default,
            },
            user_unit: Sourced {
                value: 1.0,
                origin: Origin::Default,
            },
        }
    }

    /// The numbered nodes on its `/Parent` chain, nearest first. A `/Parent`
    /// written in place has no number, and isn't given.
    pub fn parents(&self) -> Parents<'_> {
        Parents {
            next: self.parent.as_deref(),
        }
    }

    /// An inheritable attribute: its own, else the nearest on its `/Parent`
    /// chain; none where neither has it. By ISO's rules a null is absent; by
    /// PDFium's it stops the climb, so `value` can be null, and can be a
    /// reference, where that named another reference.
    #[must_use]
    pub const fn attribute(&self, attribute: Inheritable) -> Option<Attribute<'o>> {
        self.attributes[attribute as usize]
    }

    /// Its `/MediaBox`, as far as it's four numbers; what's in place of any
    /// that isn't a number is 0, as PDFium reads it. Otherwise Letter, of
    /// [`Origin::Default`]: ISO 32000-2 gives none, requiring one, and it's
    /// what PDFium makes of none. An empty box is given as written.
    #[must_use]
    pub const fn media_box(&self) -> Sourced<Rect> {
        self.media_box
    }

    /// Its `/CropBox`, as [`media_box`](Self::media_box) reads one; otherwise
    /// the media box, of [`Origin::Default`] (7.7.3.3).
    #[must_use]
    pub const fn crop_box(&self) -> Sourced<Rect> {
        self.crop_box
    }

    /// Its `/Rotate` in degrees, as written: a real's whole part. Otherwise 0,
    /// of [`Origin::Default`] (7.7.3.3).
    #[must_use]
    pub const fn rotate(&self) -> Sourced<i64> {
        self.rotate
    }

    /// Its `/UserUnit`, a positive number, which isn't inherited. Otherwise 1,
    /// of [`Origin::Default`] (7.7.3.3).
    #[must_use]
    pub const fn user_unit(&self) -> Sourced<f64> {
        self.user_unit
    }
}

/// The numbered nodes on a page's `/Parent` chain, nearest first: what
/// [`Page::parents`] gives. It's fused, and a `count`, `for_each` or `fold`
/// over it goes up the chain in one pass. Its `Debug` prints the numbers it
/// has left to give.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Clone)]
pub struct Parents<'a> {
    /// The next node on the chain, numbered or not.
    next: Option<&'a Parent>,
}

impl Iterator for Parents<'_> {
    type Item = ObjectId;

    #[inline]
    fn next(&mut self) -> Option<ObjectId> {
        while let Some(parent) = self.next {
            self.next = parent.up.as_deref();
            if let Some(id) = parent.id {
                return Some(id);
            }
        }
        None
    }

    /// How many are left isn't known until they're walked, as a node written
    /// in place gives none; once it's walked, none.
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self.next {
            Some(_) => (0, None),
            None => (0, Some(0)),
        }
    }

    /// One pass up the chain, as `count`, `for_each` and `last` fold.
    #[inline]
    fn fold<B, F: FnMut(B, ObjectId) -> B>(self, init: B, mut f: F) -> B {
        let (mut at, mut acc) = (self.next, init);
        while let Some(parent) = at {
            if let Some(id) = parent.id {
                acc = f(acc, id);
            }
            at = parent.up.as_deref();
        }
        acc
    }
}

impl FusedIterator for Parents<'_> {}

impl fmt::Debug for Parents<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let left: Vec<ObjectId> = self.clone().collect();
        f.debug_tuple("Parents").field(&left).finish()
    }
}

/// The attributes a page inherits (ISO 32000-2, 7.7.3.3 and 7.7.3.4).
/// Exhaustive on purpose, as the clause's list is closed and a consumer
/// matches every one; [`Inheritable::ALL`] lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Inheritable {
    /// `/Resources`.
    Resources,
    /// `/MediaBox`.
    MediaBox,
    /// `/CropBox`.
    CropBox,
    /// `/Rotate`.
    Rotate,
}

impl Inheritable {
    /// Every attribute, in the order they're declared, so a table indexed by
    /// them needn't write their count.
    pub const ALL: [Inheritable; 4] = [
        Inheritable::Resources,
        Inheritable::MediaBox,
        Inheritable::CropBox,
        Inheritable::Rotate,
    ];

    /// Its key in a page's or a node's dictionary, without the slash:
    /// `"MediaBox"`.
    #[must_use]
    pub const fn key(self) -> &'static str {
        KEYS[self as usize]
    }
}

/// Each attribute's key, in [`Inheritable`]'s order.
const KEYS: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];

/// An inheritable attribute of a page.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Attribute<'o> {
    /// As it's written in the dictionary that has it: a reference stays one.
    pub written: &'o Object,
    /// What that reads as, its references followed.
    pub value: &'o Object,
    /// Which dictionary has it.
    pub origin: Origin,
}

/// A value, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Sourced<T> {
    /// The value.
    pub value: T,
    /// Where it came from.
    pub origin: Origin,
}

/// Where a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Origin {
    /// The dictionary it's a value of: a page's own.
    Own,
    /// A dictionary it's inherited from, on a `/Parent` chain: that one's
    /// number, none for a `/Parent` written in place.
    Ancestor(Option<ObjectId>),
    /// None of them has it as a value that reads: what stands for it.
    Default,
}

/// A rectangle, its four numbers in the order they're written (ISO 32000-2,
/// 7.9.5): any two opposite corners, not put in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect(pub [f64; 4]);

impl Rect {
    /// What a page with no box reads as: US Letter, 612 by 792.
    pub const LETTER: Rect = Rect([0.0, 0.0, 612.0, 792.0]);
}

/// Why a file has no table of pages.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A node met again on its own path, by PDFium's rules.
    #[error("the page tree lists a node inside itself")]
    Cycle,
    /// A `/Parent` chain that comes back to itself, by PDFium's rules.
    #[error("a page's /Parent chain loops")]
    ParentLoop,
    /// More levels of nodes than [`Limits::page_depth`], under the root or on
    /// a `/Parent` chain.
    #[error("the page tree is too deep")]
    TooDeep,
    /// More pages or visits to nodes than [`Limits::pages`], more kids looked
    /// at than a few times that, or by PDFium's rules, a count of the tree past
    /// what PDFium would give.
    #[error("the page tree lists too many pages")]
    TooManyPages,
    /// A catalog with no `/Pages` that's a dictionary, or, by PDFium's rules,
    /// no pages under it, though PDFium may count some ([`counted`](fn@counted)).
    #[error("the file has no pages")]
    NoPages,
    /// An object on the way past a cap, that takes a password, or whose bytes
    /// the source didn't give.
    #[error("{0}")]
    Unreadable(Unreadable),
}

impl From<Unreadable> for Error {
    fn from(unreadable: Unreadable) -> Error {
        Error::Unreadable(unreadable)
    }
}

/// A node on a page's `/Parent` chain, and the chain above it: pages under
/// one node share it.
struct Parent {
    /// Its number: none for one written in place.
    id: Option<ObjectId>,
    up: Option<Arc<Parent>>,
}

/// A chain is as long as [`Limits::page_depth`] lets it be, which a caller can
/// raise, so what goes along one goes in a loop: these three aren't derived, as
/// what's derived goes down it on the stack.
impl Drop for Parent {
    fn drop(&mut self) {
        let mut up = self.up.take();
        while let Some(Ok(mut parent)) = up.map(Arc::try_unwrap) {
            up = parent.up.take();
        }
    }
}

impl PartialEq for Parent {
    fn eq(&self, other: &Parent) -> bool {
        let (mut a, mut b) = (self, other);
        loop {
            if std::ptr::eq(a, b) {
                return true;
            }
            if a.id != b.id {
                return false;
            }
            match (a.up.as_deref(), b.up.as_deref()) {
                (Some(above_a), Some(above_b)) => (a, b) = (above_a, above_b),
                (None, None) => return true,
                _ => return false,
            }
        }
    }
}

impl Eq for Parent {}

impl fmt::Debug for Parent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let chain = std::iter::successors(Some(self), |parent| parent.up.as_deref());
        f.debug_list()
            .entries(chain.map(|parent| parent.id))
            .finish()
    }
}

/// What a node gives the pages under it by `/Parent`.
struct Inherited<'o> {
    chain: Arc<Parent>,
    /// Nodes on the chain.
    depth: usize,
    /// The nearest value of each attribute, on it or above it.
    attributes: [Option<Attribute<'o>>; 4],
}

/// A node being walked: its kids, and the next to visit.
struct Frame<'o> {
    id: Option<ObjectId>,
    /// The numbered object it's in.
    object: Option<ObjectId>,
    dict: &'o Dict,
    at: Option<Span>,
    kids: &'o [Object],
    next: usize,
    /// How many pages the table had when it was reached.
    first: usize,
}

/// A node being counted.
struct Counting<'o> {
    id: Option<ObjectId>,
    kids: &'o [Object],
    next: usize,
    total: usize,
}

/// What following a value comes to.
enum Reached<'o> {
    /// An object, perhaps null, and the number of the last reference to it.
    Object(&'o Object, Option<ObjectId>),
    /// A reference to an object the file hasn't.
    Nothing,
    /// An object that doesn't read, said.
    Damaged,
}

/// What a dictionary among a node's kids is.
enum Class {
    Node,
    Page,
    /// A place with no page at it.
    Empty,
}

struct Walk<'o, 'r, R, S: ?Sized> {
    objects: R,
    sink: &'r mut S,
    limits: Limits,
    pdfium: bool,
    /// The count of pages PDFium gives, at which its walk stops.
    count: Option<usize>,
    /// What each node met on a `/Parent` chain gives.
    nodes: HashMap<ObjectId, Rc<Inherited<'o>>>,
    /// Each object listed: where the table first has it, and how often it's
    /// been listed.
    listed: HashMap<ObjectId, (usize, u32)>,
    /// The leaves the count of the tree counted as pages, by their address, as
    /// one written in place has no number, where their `/Type` wouldn't load.
    counted_as_pages: HashSet<*const Dict>,
    /// The nodes the walk has been in, when a sink wants to hear of a repeat.
    seen_nodes: HashSet<ObjectId>,
    pages: Vec<Page<'o>>,
    /// Nodes visited under the root.
    visits: usize,
    /// Kids the walk has looked at.
    kids_looked_at: usize,
    /// The numbered object what's being read is in.
    object: Option<ObjectId>,
}

impl<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized> Walk<'o, '_, R, S> {
    /// How many references to follow: `pdfium`'s number by PDFium's rules, and
    /// any by ISO's.
    fn hops(&self, pdfium: usize) -> usize {
        if self.pdfium { pdfium } else { HOPS }
    }

    /// `written`, through up to `hops` references. By PDFium's rules a reference
    /// past them is what's found, and by ISO's it's a chain too long, an error.
    fn reach(&mut self, written: &'o Object, hops: usize) -> Result<Reached<'o>, Error> {
        let (mut at, mut last, mut followed) = (written, None, 0);
        while let Value::Ref(id) = at.value() {
            if followed == hops {
                if self.pdfium {
                    break;
                }
                return Err(too_long(*id).into());
            }
            followed += 1;
            last = Some(*id);
            match self.objects.try_resolve(*id) {
                Ok(Some(object)) => at = object,
                Ok(None) => return Ok(Reached::Nothing),
                Err(unreadable) if unreadable.why == Why::Damaged => {
                    self.say(at.at, Fault::Damaged { id: *id });
                    return Ok(Reached::Damaged);
                }
                Err(unreadable) => return Err(unreadable.into()),
            }
        }
        if followed > 1 && !self.pdfium {
            self.say(written.at, Fault::PageTreeChain);
        }
        Ok(Reached::Object(at, last))
    }

    /// The entry `key` of `dict`, as written and as it reads: none where it's
    /// missing, names nothing or doesn't read. By ISO's rules null is absent
    /// too; by PDFium's it's found, and stops a climb up `/Parent`.
    fn found(
        &mut self,
        dict: &'o Dict,
        key: &[u8],
    ) -> Result<Option<(&'o Object, &'o Object)>, Error> {
        let Some(written) = dict.get(key) else {
            return Ok(None);
        };
        let hops = self.hops(1);
        Ok(match self.reach(written, hops)? {
            Reached::Object(value, _) if self.pdfium || !value.is_null() => Some((written, value)),
            _ => None,
        })
    }

    /// A node's kids: none when its `/Kids` isn't an array.
    fn kids(&mut self, node: &'o Dict) -> Result<&'o [Object], Error> {
        Ok(self
            .found(node, b"Kids")?
            .and_then(|(_, kids)| kids.as_array())
            .unwrap_or_default())
    }

    fn table(&mut self, catalog: &'o Dict) -> Result<(), Error> {
        let hops = self.hops(2);
        let root = match catalog.get(b"Pages") {
            Some(written) => match self.reach(written, hops)? {
                Reached::Object(root, id) => root.as_dict().map(|dict| (root, id, dict)),
                Reached::Nothing | Reached::Damaged => None,
            },
            None => None,
        };
        let Some((object, id, root)) = root else {
            return Err(Error::NoPages);
        };
        self.object = id;
        if self.pdfium {
            return self.walk_pdfium_root(id, root, object.at);
        }
        if id.is_none() {
            self.say(object.at, Fault::PageTreeRootDirect);
        }
        if has_value(root, b"Parent") {
            self.say(object.at, Fault::PageTreeRootParent);
        }
        match self.class(root, object.at)? {
            Class::Node => self.walk(id, root, object.at)?,
            Class::Page => {
                if root.has_type(b"Page") && !root.has(b"Kids") {
                    self.say(object.at, Fault::PageTreeType);
                }
                self.page(id, root, object.at)?;
            }
            Class::Empty => {}
        }
        if self.pages.is_empty() {
            self.say(object.at, Fault::PageTreeEmpty);
        }
        Ok(())
    }

    /// The tree under `root` by PDFium's rules: a root without `/Kids` is the
    /// one page, counted so whatever its `/Count`, unless it says it's a node.
    /// One with `/Kids` that isn't an array has none to walk, and is a page only
    /// if its `/Type` is written as the name `/Page` and the count isn't 0: page
    /// 0, the rest of the count none.
    fn walk_pdfium_root(
        &mut self,
        id: Option<ObjectId>,
        root: &'o Dict,
        at: Option<Span>,
    ) -> Result<(), Error> {
        if !root.has(b"Kids") {
            self.count = Some(1);
            if root.has_type(b"Pages") {
                return self.push(Page::none(id));
            }
            return self.page(id, root, at);
        }
        let count = match self.trusted(root)? {
            Some(count) => count,
            None => self.count_tree(id, root)?,
        };
        self.count = Some(count);
        if count > 0 && root.has_type(b"Page") {
            let kids = self.found(root, b"Kids")?;
            if kids.and_then(|(_, kids)| kids.as_array()).is_none() {
                return self.page(id, root, at);
            }
        }
        self.pages.reserve(count.min(ROOM));
        self.walk(id, root, at)
    }

    /// The node's `/Count`, as PDFium's count goes by it: if it's positive and
    /// under the cap, a real's whole part, a reference followed.
    fn trusted(&mut self, node: &'o Dict) -> Result<Option<usize>, Error> {
        let count = self
            .found(node, b"Count")?
            .and_then(|(_, count)| whole(count));
        Ok(count
            .filter(|count| (1..PDFIUM_PAGE_MAX).contains(count))
            .map(|count| count as usize))
    }

    /// The count of pages under `root`, as PDFium counts when the root's
    /// `/Count` isn't one it goes by. A dictionary among the kids counts as a
    /// page if its `/Type` is the name `/Page`, or if it has no `/Kids` and its
    /// `/Type` isn't the name `/Pages`; otherwise it's a node, by its `/Count`
    /// if that's one PDFium goes by, or what's under it. A kid on the path to
    /// it, or that names no dictionary, isn't counted. Its depth is no cap here,
    /// as in PDFium's: what's walked after is held to one, as far as it goes.
    fn count_tree(&mut self, id: Option<ObjectId>, root: &'o Dict) -> Result<usize, Error> {
        let mut stack = vec![Counting {
            id,
            kids: self.kids(root)?,
            next: 0,
            total: 0,
        }];
        let mut path: HashSet<ObjectId> = id.into_iter().collect();
        let (mut visits, mut kids_looked_at, mut counted) = (0, 0, 0);
        while let Some(frame) = stack.last_mut() {
            let Some(kid) = frame.kids.get(frame.next) else {
                let (id, total) = (frame.id, frame.total);
                stack.pop();
                if let Some(id) = id {
                    path.remove(&id);
                }
                match stack.last_mut() {
                    Some(up) => add(&mut up.total, total)?,
                    None => counted = total,
                }
                continue;
            };
            frame.next += 1;
            kids_looked_at += 1;
            if kids_looked_at > self.limits.pages.saturating_mul(KIDS_A_PAGE) {
                return Err(Error::TooManyPages);
            }
            let Reached::Object(object, id) = self.reach(kid, 1)? else {
                continue;
            };
            let Some(dict) = object.as_dict() else {
                continue;
            };
            if id.is_some_and(|id| path.contains(&id)) {
                continue;
            }
            if dict.has_type(b"Page") || (!dict.has(b"Kids") && !dict.has_type(b"Pages")) {
                if dict.has(b"Type") && !dict.has_type(b"Page") {
                    self.counted_as_pages.insert(std::ptr::from_ref(dict));
                }
                add(&mut frame.total, 1)?;
                continue;
            }
            visits += 1;
            if visits > self.limits.pages {
                return Err(Error::TooManyPages);
            }
            if let Some(count) = self.trusted(dict)? {
                add(&mut frame.total, count)?;
                continue;
            }
            if let Some(id) = id {
                path.insert(id);
            }
            let kids = self.kids(dict)?;
            stack.push(Counting {
                id,
                kids,
                next: 0,
                total: 0,
            });
        }
        Ok(counted)
    }

    /// The tree under the node `root`.
    fn walk(
        &mut self,
        id: Option<ObjectId>,
        root: &'o Dict,
        at: Option<Span>,
    ) -> Result<(), Error> {
        if let (Some(id), true) = (id, self.checking()) {
            self.seen_nodes.insert(id);
        }
        let mut stack = vec![Frame {
            id,
            object: self.object,
            dict: root,
            at,
            kids: self.node_kids(root, at)?,
            next: 0,
            first: 0,
        }];
        let mut path: HashSet<ObjectId> = id.into_iter().collect();
        while let Some(frame) = stack.last_mut() {
            if self.count.is_some_and(|count| self.pages.len() >= count) {
                break;
            }
            self.object = frame.object;
            let Some(kid) = frame.kids.get(frame.next) else {
                let (id, dict, at, first) = (frame.id, frame.dict, frame.at, frame.first);
                stack.pop();
                if let Some(id) = id {
                    path.remove(&id);
                }
                self.check_count(dict, at, first);
                continue;
            };
            frame.next += 1;
            self.kids_looked_at += 1;
            if self.kids_looked_at > self.limits.pages.saturating_mul(KIDS_A_PAGE) {
                return Err(Error::TooManyPages);
            }
            let holder = frame.id;
            if !self.pdfium && kid.as_reference().is_none() && !kid.is_null() {
                self.say(kid.at, Fault::PageTreeKidDirect);
            }
            let hops = self.hops(1);
            let (id, object) = match self.reach(kid, hops)? {
                Reached::Object(object, id) => (id, object),
                Reached::Nothing => {
                    self.no_dictionary(kid, kid.as_reference(), true)?;
                    continue;
                }
                Reached::Damaged => {
                    self.no_dictionary(kid, kid.as_reference(), false)?;
                    continue;
                }
            };
            if let Some(first) = id.and_then(|id| self.listed.get(&id)).map(|at| at.0) {
                if let (Some(id), true) = (id, self.checking()) {
                    self.say(kid.at, Fault::PageTreeRepeated { id });
                }
                let again = self.pages[first].clone();
                self.push(again)?;
                continue;
            }
            let Some(dict) = object.as_dict() else {
                self.no_dictionary(kid, id, true)?;
                continue;
            };
            // What's said of the kid's own dictionary is in its object, and what's
            // said of the reference to it below in the node that lists it.
            let listing = self.object;
            self.object = id.or(listing);
            if !self.pdfium && matches!(object.value(), Value::Stream(_)) {
                self.say(object.at, Fault::PageTreeKidNotDictionary);
            }
            let class = self.class(dict, object.at)?;
            self.object = listing;
            match class {
                Class::Page => {
                    self.object = id.or(self.object);
                    self.check_parent(dict, object.at, holder);
                    self.page(id, dict, object.at)?;
                }
                Class::Empty => self.push(Page::none(id))?,
                Class::Node => {
                    if let Some(id) = id.filter(|id| path.contains(id)) {
                        if self.pdfium {
                            return Err(Error::Cycle);
                        }
                        self.say(kid.at, Fault::PageTreeCycle { id });
                        continue;
                    }
                    if stack.len() >= self.limits.page_depth {
                        return Err(Error::TooDeep);
                    }
                    self.visits += 1;
                    if self.visits > self.limits.pages {
                        return Err(Error::TooManyPages);
                    }
                    if let (Some(id), true) = (id, self.checking())
                        && !self.seen_nodes.insert(id)
                    {
                        self.say(kid.at, Fault::PageTreeRepeated { id });
                    }
                    self.object = id.or(self.object);
                    self.check_parent(dict, object.at, holder);
                    let kids = self.node_kids(dict, object.at)?;
                    if let Some(id) = id {
                        path.insert(id);
                    }
                    stack.push(Frame {
                        id,
                        object: self.object,
                        dict,
                        at: object.at,
                        kids,
                        next: 0,
                        first: self.pages.len(),
                    });
                }
            }
        }
        Ok(())
    }

    /// A kid that names no dictionary: by PDFium's rules a place, as it takes
    /// a number; said by ISO's, `say` unless what's wrong has been said.
    fn no_dictionary(
        &mut self,
        kid: &'o Object,
        id: Option<ObjectId>,
        say: bool,
    ) -> Result<(), Error> {
        if self.pdfium {
            return self.push(Page::none(id));
        }
        if say {
            self.say(kid.at, Fault::PageTreeKidNotDictionary);
        }
        Ok(())
    }

    /// A node's kids, for walking: an array of none said by ISO's rules.
    fn node_kids(&mut self, node: &'o Dict, at: Option<Span>) -> Result<&'o [Object], Error> {
        let kids = self.kids(node)?;
        if kids.is_empty() && !self.pdfium {
            self.say(at, Fault::PageTreeNoKids);
        }
        Ok(kids)
    }

    /// What the dictionary `dict` is, among a node's kids or at the root.
    fn class(&mut self, dict: &'o Dict, at: Option<Span>) -> Result<Class, Error> {
        if self.pdfium {
            if dict.has(b"Kids") {
                return Ok(Class::Node);
            }
            let page =
                self.type_loads(dict)? || self.counted_as_pages.contains(&std::ptr::from_ref(dict));
            return Ok(if page { Class::Page } else { Class::Empty });
        }
        let kind = match dict.get(b"Type") {
            Some(written) => match self.reach(written, HOPS)? {
                Reached::Object(kind, _) => kind.as_name(),
                _ => None,
            },
            None => None,
        };
        Ok(match kind {
            Some(b"Pages") => Class::Node,
            Some(b"Page") => {
                if dict.has(b"Kids") {
                    self.say(at, Fault::PageTreeType);
                }
                Class::Page
            }
            _ => {
                self.say(at, Fault::PageTreeType);
                if has_value(dict, b"Kids") {
                    Class::Node
                } else {
                    Class::Page
                }
            }
        })
    }

    /// Whether PDFium loads a leaf by its `/Type`: if it has none, or if it
    /// reads, through one reference, as the name `/Page`.
    fn type_loads(&mut self, dict: &'o Dict) -> Result<bool, Error> {
        let Some(written) = dict.get(b"Type") else {
            return Ok(true);
        };
        Ok(match self.reach(written, 1)? {
            Reached::Object(kind, _) => matches!(kind.as_name(), Some(b"Page")),
            _ => false,
        })
    }

    /// The page `dict`, with what it inherits, added to the table.
    fn page(
        &mut self,
        id: Option<ObjectId>,
        dict: &'o Dict,
        at: Option<Span>,
    ) -> Result<(), Error> {
        self.object = id.or(self.object);
        let up = match dict.get(b"Parent") {
            Some(parent) => self.inherited(parent)?,
            None => None,
        };
        let up = up.as_deref();
        let mut attributes = [None; 4];
        for (i, key) in KEYS.iter().enumerate() {
            attributes[i] = match self.found(dict, key.as_bytes())? {
                Some((written, value)) => Some(Attribute {
                    written,
                    value,
                    origin: Origin::Own,
                }),
                None => up.and_then(|up| up.attributes[i]),
            };
        }
        let media_box = match self.rect(attributes[Inheritable::MediaBox as usize])? {
            Some(media_box) => media_box,
            None => {
                if !self.pdfium {
                    self.say(at, Fault::PageNoMediaBox);
                }
                Sourced {
                    value: Rect::LETTER,
                    origin: Origin::Default,
                }
            }
        };
        let crop_box = self
            .rect(attributes[Inheritable::CropBox as usize])?
            .unwrap_or(Sourced {
                value: media_box.value,
                origin: Origin::Default,
            });
        let rotate = self
            .rotation(attributes[Inheritable::Rotate as usize])?
            .unwrap_or(Sourced {
                value: 0,
                origin: Origin::Default,
            });
        let user_unit = match self.found(dict, b"UserUnit")? {
            Some((_, unit)) => unit
                .as_number()
                .filter(|unit| *unit > 0.0)
                .map(|value| Sourced {
                    value,
                    origin: Origin::Own,
                }),
            None => None,
        };
        self.push(Page {
            id,
            dict: Some(dict),
            repeat: 0,
            parent: up.map(|up| up.chain.clone()),
            attributes,
            media_box,
            crop_box,
            rotate,
            user_unit: user_unit.unwrap_or(Sourced {
                value: 1.0,
                origin: Origin::Default,
            }),
        })
    }

    /// `page` added to the table, numbered by how often its object was listed
    /// before.
    fn push(&mut self, mut page: Page<'o>) -> Result<(), Error> {
        if self.pages.len() >= self.limits.pages {
            return Err(Error::TooManyPages);
        }
        if let Some(id) = page.id {
            let at = self.pages.len();
            let (_, listed) = self.listed.entry(id).or_insert((at, 0));
            page.repeat = *listed;
            *listed += 1;
        }
        self.pages.push(page);
        Ok(())
    }

    /// What the node `written` names gives the pages under it, up its
    /// `/Parent` chain to a node already met and back down. None when it
    /// isn't a dictionary.
    fn inherited(&mut self, written: &'o Object) -> Result<Option<Rc<Inherited<'o>>>, Error> {
        let object = self.object;
        let hops = self.hops(2);
        let mut chain = Vec::new();
        let mut on_chain = HashSet::new();
        let mut at = Some(written);
        let mut above = None;
        while let Some(parent) = at {
            let (node, id) = match self.reach(parent, hops)? {
                Reached::Object(node, id) => (node, id),
                Reached::Nothing | Reached::Damaged => break,
            };
            if !self.pdfium && parent.as_reference().is_none() && !parent.is_null() {
                self.say(parent.at, Fault::PageTreeParentDirect);
            }
            if let Some(id) = id {
                if let Some(known) = self.nodes.get(&id) {
                    above = Some(known.clone());
                    break;
                }
                if !on_chain.insert(id) {
                    if self.pdfium {
                        return Err(Error::ParentLoop);
                    }
                    self.say(parent.at, Fault::PageTreeParentLoop);
                    break;
                }
            }
            if chain.len() >= self.limits.page_depth {
                return Err(Error::TooDeep);
            }
            let Some(dict) = node.as_dict() else {
                break;
            };
            chain.push((id, dict));
            self.object = id.or(self.object);
            at = dict.get(b"Parent");
        }
        for (id, dict) in chain.into_iter().rev() {
            self.object = id.or(self.object);
            let up = above.as_deref();
            let depth = up.map_or(0, |up| up.depth) + 1;
            if depth > self.limits.page_depth {
                return Err(Error::TooDeep);
            }
            let mut attributes = [None; 4];
            for (i, key) in KEYS.iter().enumerate() {
                attributes[i] = match self.found(dict, key.as_bytes())? {
                    Some((written, value)) => Some(Attribute {
                        written,
                        value,
                        origin: Origin::Ancestor(id),
                    }),
                    None => up.and_then(|up| up.attributes[i]),
                };
            }
            let node = Rc::new(Inherited {
                chain: Arc::new(Parent {
                    id,
                    up: up.map(|up| up.chain.clone()),
                }),
                depth,
                attributes,
            });
            if let Some(id) = id {
                self.nodes.insert(id, node.clone());
            }
            above = Some(node);
        }
        self.object = object;
        Ok(above)
    }

    /// A box as a value, where it's an array of four: a number's references
    /// followed, what's no number 0.
    fn rect(&mut self, attribute: Option<Attribute<'o>>) -> Result<Option<Sourced<Rect>>, Error> {
        let Some(attribute) = attribute else {
            return Ok(None);
        };
        let Some([a, b, c, d]) = attribute.value.as_array() else {
            return Ok(None);
        };
        let hops = self.hops(1);
        let mut corners = [0.0; 4];
        for (corner, written) in corners.iter_mut().zip([a, b, c, d]) {
            if let Reached::Object(number, _) = self.reach(written, hops)? {
                *corner = number.as_number().unwrap_or(0.0);
            }
        }
        Ok(Some(Sourced {
            value: Rect(corners),
            origin: attribute.origin,
        }))
    }

    /// `/Rotate` as a value, where it's a number. What PDFium found through
    /// one reference may be another, which it follows here.
    fn rotation(
        &mut self,
        attribute: Option<Attribute<'o>>,
    ) -> Result<Option<Sourced<i64>>, Error> {
        let Some(attribute) = attribute else {
            return Ok(None);
        };
        let hops = self.hops(1);
        let Reached::Object(value, _) = self.reach(attribute.value, hops)? else {
            return Ok(None);
        };
        Ok(whole(value).map(|value| Sourced {
            value,
            origin: attribute.origin,
        }))
    }

    /// Whether what only a checker wants is looked for: by ISO's rules, for a
    /// sink that wants faults.
    fn checking(&self) -> bool {
        !self.pdfium && self.sink.wants()
    }

    /// Holds what `dict` says of its `/Parent` to `holder`, the node that lists
    /// it, when a checker wants to know. A walk without one never reads a node's
    /// `/Parent`, so one that fails here fails nothing: one that doesn't read is
    /// that `/Parent` said, and one past a cap or a password isn't checked.
    fn check_parent(&mut self, dict: &'o Dict, at: Option<Span>, holder: Option<ObjectId>) {
        if !self.checking() {
            return;
        }
        let parent = match dict.get(b"Parent") {
            Some(written) => match self.reach(written, HOPS) {
                Ok(Reached::Object(_, id)) => id,
                Ok(_) => None,
                Err(stopped) => return self.unchecked(written.at, stopped),
            },
            None => None,
        };
        if parent.is_none() || parent != holder {
            self.say(at, Fault::PageTreeParent { lister: holder });
        }
    }

    /// Holds a node's `/Count` to the pages found under it, when a checker
    /// wants to know. A walk without one never reads it here, so one that fails
    /// here fails nothing: one that doesn't read is that `/Count` said, and one
    /// past a cap or a password isn't checked.
    fn check_count(&mut self, node: &'o Dict, at: Option<Span>, first: usize) {
        if !self.checking() {
            return;
        }
        let count = match self.found(node, b"Count") {
            Ok(found) => found.and_then(|(_, count)| count.as_integer()),
            Err(stopped) => {
                let written = node.get(b"Count").and_then(|count| count.at);
                return self.unchecked(written.or(at), stopped);
            }
        };
        let under = self.pages.len() - first;
        if count != Some(under as i64) || under == 0 {
            self.say(at, Fault::PageTreeCount { count, under });
        }
    }

    /// Says that what a check reads at `at` wasn't checked, as `stopped`, an
    /// object on the way past a cap or that takes a password, has it.
    fn unchecked(&mut self, at: Option<Span>, stopped: Error) {
        if let Error::Unreadable(unreadable) = stopped {
            let (id, why) = (unreadable.id, unreadable.why);
            self.say(at, Fault::Unchecked { id, why });
        }
    }

    fn say(&mut self, at: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            self.sink.fault(Place::new(self.object, at), fault);
        }
    }
}

/// Whether `dict` has `key`, as something other than null.
fn has_value(dict: &Dict, key: &[u8]) -> bool {
    dict.get(key).is_some_and(|value| !value.is_null())
}

/// A number's whole part.
fn whole(number: &Object) -> Option<i64> {
    match number.value() {
        Value::Integer(integer) => Some(*integer),
        Value::Real(real) => Some(*real as i64),
        _ => None,
    }
}

/// Adds `count` to what's counted so far, past PDFium's cap an error.
fn add(total: &mut usize, count: usize) -> Result<(), Error> {
    *total += count;
    if *total >= PDFIUM_PAGE_MAX as usize {
        return Err(Error::TooManyPages);
    }
    Ok(())
}

/// The most kids a node of a tree [`write()`] writes has, where a caller has
/// no reason to choose: 32, so 1,024 pages take two levels of nodes, and
/// 32,768 three.
pub const FANOUT: usize = 32;

/// A balanced page tree over `pages`, in order (ISO 32000-2, 7.7.3.2): every
/// page at one depth, under as few levels of nodes as hold them with no node
/// of more than `fanout` kids, and each node's kids as even as they can be,
/// so 33 pages under nodes of 32 are two leaves of 17 and 16. [`FANOUT`]
/// serves where nothing says otherwise, and `usize::MAX` writes one node
/// over every page.
///
/// Its nodes are numbered by `ids`, called once a node, the root first and
/// each node before those under it, so that a document's own way of handing
/// out numbers, `|| document.new_id()`, serves. Each node is `/Type /Pages`,
/// `/Parent` but the root and `/Kids`, by reference, and `/Count`, and has no
/// inheritable attribute (7.7.3.4): a page carries its own, or a caller sets
/// one on the root. The pages aren't written: a caller writes each, its
/// `/Parent` the node [`Written::parents`] names.
///
/// With each page `/Type /Page`, that `/Parent` and a `/MediaBox`, its own or
/// the root's, [`pages`] reads the tree back by either rules under the same
/// `limits` with no fault, every page once and in order. Ids are told apart
/// by number, as a file has one object at a number (7.5.4). More pages than
/// PDFium counts, [`Limits::DEFAULT`]'s cap, are written only under a cap
/// raised past it, and by PDFium's rules read as too many.
///
/// ```
/// use std::collections::BTreeMap;
///
/// use pdf_semantics::Limits;
/// use pdf_semantics::pages::{self, pages};
/// use pdf_semantics::pdf_objects::pdf_tokenizer::Rules;
/// use pdf_semantics::pdf_objects::{Object, ObjectId, dict};
///
/// // Three pages under nodes of two: a leaf of two and one of one.
/// let numbered = [ObjectId::new(1, 0), ObjectId::new(2, 0), ObjectId::new(3, 0)];
/// let mut next = 10;
/// let ids = || {
///     next += 1;
///     ObjectId::new(next - 1, 0)
/// };
/// let written = pages::write(&numbered, 2, &Limits::DEFAULT, ids).unwrap();
/// assert_eq!(written.root, ObjectId::new(10, 0));
/// let (eleven, twelve) = (ObjectId::new(11, 0), ObjectId::new(12, 0));
/// assert_eq!(written.parents, [eleven, eleven, twelve]);
///
/// let mut objects: BTreeMap<ObjectId, Object> = (written.nodes.into_iter())
///     .map(|(id, node)| (id, Object::from(node)))
///     .collect();
/// for (&page, &parent) in numbered.iter().zip(&written.parents) {
///     let media = vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)];
///     let page_dict = dict! { "Type" => "Page", "Parent" => parent, "MediaBox" => media };
///     objects.insert(page, Object::from(page_dict));
/// }
/// let resolve = |id: ObjectId| objects.get(&id);
/// let catalog = dict! { "Pages" => written.root };
/// let mut faults = Vec::new();
/// let table = pages(resolve, &catalog, Rules::Iso, &Limits::DEFAULT, &mut faults).unwrap();
/// assert!(faults.is_empty());
/// let listed: Vec<_> = table.iter().map(|page| page.id.unwrap()).collect();
/// assert_eq!(listed, numbered);
/// ```
///
/// # Errors
///
/// A [`WriteError`], with nothing written, checked in this order, `ids`
/// called for none but the last: no pages ([`WriteError::NoPages`]); a
/// fanout under 2 ([`WriteError::Fanout`]); more pages than `limits.pages`
/// ([`WriteError::TooManyPages`]); more levels of nodes than
/// `limits.page_depth`, which only a small fanout over many pages comes to,
/// and under the default caps none ([`WriteError::TooDeep`]); the first page
/// of number 0 ([`WriteError::Zero`]) or of a page's number before it
/// ([`WriteError::Repeated`]); more nodes under the root than `limits.pages`,
/// as a walk visits them, which only a fanout of 2 comes to
/// ([`WriteError::TooManyPages`]); and as `ids` gives each node's id, one of
/// number 0 ([`WriteError::Zero`]) or of a page's number or a node's before it
/// ([`WriteError::Taken`]), after which `ids` isn't called again.
pub fn write(
    pages: &[ObjectId],
    fanout: usize,
    limits: &Limits,
    mut ids: impl FnMut() -> ObjectId,
) -> Result<Written, WriteError> {
    phase!(DEBUG, "write", pages = pages.len());
    if pages.is_empty() {
        return Err(WriteError::NoPages);
    }
    if fanout < 2 {
        return Err(WriteError::Fanout { fanout });
    }
    let too_many = WriteError::TooManyPages { cap: limits.pages };
    if pages.len() > limits.pages {
        return Err(too_many);
    }
    let height = height(pages.len(), fanout);
    if height as usize > limits.page_depth {
        return Err(WriteError::TooDeep {
            cap: limits.page_depth,
        });
    }
    let mut numbers = HashSet::with_capacity(pages.len());
    for &page in pages {
        if page.number == 0 {
            return Err(WriteError::Zero);
        }
        if !numbers.insert(page.number) {
            return Err(WriteError::Repeated { id: page });
        }
    }
    let shape = balanced(pages.len(), fanout, height);
    if shape.nodes.len() - 1 > limits.pages {
        return Err(too_many);
    }
    let mut numbered = Vec::with_capacity(shape.nodes.len());
    for _ in &shape.nodes {
        let id = ids();
        if id.number == 0 {
            return Err(WriteError::Zero);
        }
        if !numbers.insert(id.number) {
            return Err(WriteError::Taken { id });
        }
        numbered.push(id);
    }
    let nodes = (shape.nodes.iter().zip(&numbered))
        .map(|(node, &id)| {
            let mut dict = Dict::new();
            dict.push("Type", "Pages");
            if let Some(parent) = node.parent {
                dict.push("Parent", numbered[parent]);
            }
            let kids: Vec<Object> = (node.kids.iter())
                .map(|kid| match *kid {
                    Kid::Node(n) => Object::from(numbered[n]),
                    Kid::Page(p) => Object::from(pages[p]),
                })
                .collect();
            dict.push("Kids", kids);
            // A slice is never longer than an i64 counts.
            dict.push("Count", i64::try_from(node.count).unwrap_or(i64::MAX));
            (id, dict)
        })
        .collect();
    let parents = shape.leaf.iter().map(|&leaf| numbered[leaf]).collect();
    Ok(Written {
        root: numbered[0],
        nodes,
        parents,
    })
}

/// A page tree [`write()`] wrote.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Written {
    /// The root node's id, for the catalog's `/Pages`.
    pub root: ObjectId,
    /// Each node's id and dictionary, in the order `ids` numbered them: the
    /// root first, each node before those under it.
    pub nodes: Vec<(ObjectId, Dict)>,
    /// The node each page is a kid of, by the page's place among those
    /// given: its `/Parent`.
    pub parents: Vec<ObjectId>,
}

/// Why [`write()`] wrote no page tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// No pages: a tree lists one at least (ISO 32000-2, 7.7.3.2), and a
    /// walk finds none in one without.
    #[error("a page tree lists a page at least, and none is given")]
    NoPages,
    /// A fanout under 2, which can't put more than one page under a node
    /// over others.
    #[error("a page tree's nodes are given at most {fanout} kids, where it takes 2")]
    Fanout {
        /// The fanout given.
        fanout: usize,
    },
    /// A page whose number a page before it has: the tree would list one
    /// page twice, or one the file can't hold beside the other.
    #[error("object {} is given as a page twice", id.number)]
    Repeated {
        /// The second page of that number.
        id: ObjectId,
    },
    /// A page of number 0, or an id of it for a node: the head of a file's
    /// list of free numbers (7.5.4), never an object.
    #[error("object 0 is given as a page or a node, and it's never an object")]
    Zero,
    /// An id for a node whose number is a page's, or a node's before it: the
    /// tree would list itself, or a page where a node is.
    #[error("object {} is given for a node, and it's a page or a node already", id.number)]
    Taken {
        /// The id `ids` gave.
        id: ObjectId,
    },
    /// More pages than [`Limits::pages`], or more nodes under the root, as a
    /// walk visits them.
    #[error("the page tree would list more pages, or have more nodes under its root, than {cap}")]
    TooManyPages {
        /// The cap.
        cap: usize,
    },
    /// More levels of nodes than [`Limits::page_depth`].
    #[error("the page tree would have more levels of nodes than {cap}")]
    TooDeep {
        /// The cap.
        cap: usize,
    },
}

/// A page tree's nodes, in the order the tree is walked: the root first, each
/// node before the nodes under it.
struct Shape {
    nodes: Vec<PagesNode>,
    /// The node each page is a kid of, by the page's position.
    leaf: Vec<usize>,
}

/// A node of a written page tree.
struct PagesNode {
    /// None for the root.
    parent: Option<usize>,
    kids: Vec<Kid>,
    /// The pages under it.
    count: usize,
}

/// A kid of a node: a node by its place among the nodes, a page by its
/// position.
enum Kid {
    Node(usize),
    Page(usize),
}

/// How many levels of nodes hold `pages` pages, no node with more than
/// `fanout` kids.
fn height(pages: usize, fanout: usize) -> u32 {
    let (mut height, mut reach) = (1, fanout);
    while reach < pages {
        height += 1;
        reach = reach.saturating_mul(fanout);
    }
    height
}

/// The balanced tree over `pages` pages, in order, `height` levels of nodes
/// deep, no node with more than `fanout` kids.
fn balanced(pages: usize, fanout: usize, height: u32) -> Shape {
    let mut shape = Shape {
        nodes: Vec::new(),
        leaf: vec![0; pages],
    };
    build(&mut shape, 0, pages, fanout, height, None);
    shape
}

/// A node over `len` pages, the first at position `first`, `height` levels of
/// nodes deep: its place among the nodes. Its kids are spread evenly: as many
/// as `len` takes under nodes of `fanout` to the power of `height - 1`, the
/// first ones a page more than the rest.
fn build(
    shape: &mut Shape,
    first: usize,
    len: usize,
    fanout: usize,
    height: u32,
    parent: Option<usize>,
) -> usize {
    let me = shape.nodes.len();
    shape.nodes.push(PagesNode {
        parent,
        kids: Vec::new(),
        count: len,
    });
    let kids = if height <= 1 {
        shape.leaf[first..first + len].fill(me);
        (first..first + len).map(Kid::Page).collect()
    } else {
        let under = fanout.saturating_pow(height - 1);
        let groups = len.div_ceil(under).max(1);
        let (size, more) = (len / groups, len % groups);
        let mut at = 0;
        let mut kids = Vec::with_capacity(groups);
        for g in 0..groups {
            let len = size + usize::from(g < more);
            let node = build(shape, first + at, len, fanout, height - 1, Some(me));
            kids.push(Kid::Node(node));
            at += len;
        }
        kids
    };
    shape.nodes[me].kids = kids;
    me
}

#[cfg(test)]
mod tests;
