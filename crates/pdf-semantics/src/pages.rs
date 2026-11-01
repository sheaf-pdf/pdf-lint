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
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_semantics::pages::{Origin, Rect, pages};
//! use pdf_semantics::{Limits, Object, ObjectId, Rules};
//! # use pdf_objects::dict;
//!
//! let objects = BTreeMap::from([
//!     ((2, 0), Object::from(dict! {
//!         "Type" => "Pages",
//!         "Kids" => vec![Object::reference(3)],
//!         "Count" => 1,
//!         "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(200)],
//!     })),
//!     ((3, 0), Object::from(dict! { "Type" => "Page", "Parent" => Object::reference(2) })),
//! ]);
//! let resolve = |id: ObjectId| objects.get(&id);
//! let catalog = dict! { "Pages" => Object::reference(2) };
//!
//! let table = pages(&resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
//! assert_eq!(table.len(), 1);
//! let page = table.get(0).unwrap();
//! assert_eq!(page.id, Some((3, 0)));
//! let media = page.media_box();
//! assert_eq!(media.value, Rect([0.0, 0.0, 100.0, 200.0]));
//! assert_eq!(media.origin, Origin::Ancestor(Some((2, 0))));
//! assert_eq!(page.rotate().origin, Origin::Default);
//! ```

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use pdf_objects::{Dict, Object, ObjectId, Resolve, Rules, Span, Unreadable, Value, Why};

use crate::fault::{Fault, Place, Sink};
use crate::limits::Limits;
use crate::{HOPS, too_long};

/// PDFium's cap on what it counts: past this, a file's page count is none.
/// The root's or a node's `/Count` is counted by if it's positive and under.
const PDFIUM_PAGE_MAX: i64 = 0xFFFFF;

/// The most room a table starts with, however many pages a `/Count` claims:
/// a file of a hundred bytes can claim a million.
const ROOM: usize = 4096;

/// The pages of the tree at `catalog`'s `/Pages`, by `rules`, as the module
/// says. What departs from ISO 32000-2 is told to `sink` under ISO's rules;
/// under PDFium's, only what doesn't read is.
pub fn pages<'o, R: Resolve<'o> + ?Sized, S: Sink + ?Sized>(
    objects: &R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Pages<'o>, Error> {
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
        object: None,
    };
    walk.table(catalog)?;
    let counted = walk.count.unwrap_or(walk.pages.len());
    Ok(Pages {
        pages: walk.pages,
        counted,
    })
}

/// A file's pages, in order, as [`pages`] gives them.
#[derive(Debug, Clone, PartialEq)]
pub struct Pages<'o> {
    pages: Vec<Page<'o>>,
    counted: usize,
}

impl<'o> Pages<'o> {
    /// How many pages the tree lists, as far as the walk went.
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// Whether it lists none, which only ISO's rules can give: PDFium's make
    /// that [`Error::NoPages`].
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Page `index`, from 0; none past the last the tree lists.
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
    pub fn counted(&self) -> usize {
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
    pub fn parents(&self) -> impl Iterator<Item = ObjectId> + '_ {
        std::iter::successors(self.parent.as_deref(), |parent| parent.up.as_deref())
            .filter_map(|parent| parent.id)
    }

    /// An inheritable attribute: its own, else the nearest on its `/Parent`
    /// chain; none where neither has it. By ISO's rules a null is absent; by
    /// PDFium's it stops the climb, so `value` can be null, and can be a
    /// reference, where that named another reference.
    pub fn attribute(&self, attribute: Inheritable) -> Option<Attribute<'o>> {
        self.attributes[attribute as usize]
    }

    /// Its `/MediaBox`, as far as it's four numbers; what's in place of any
    /// that isn't a number is 0, as PDFium reads it. Otherwise Letter, of
    /// [`Origin::Default`]: ISO 32000-2 gives none, requiring one, and it's
    /// what PDFium makes of none. An empty box is given as written.
    pub fn media_box(&self) -> Sourced<Rect> {
        self.media_box
    }

    /// Its `/CropBox`, as [`media_box`](Self::media_box) reads one; otherwise
    /// the media box, of [`Origin::Default`] (7.7.3.3).
    pub fn crop_box(&self) -> Sourced<Rect> {
        self.crop_box
    }

    /// Its `/Rotate` in degrees, as written: a real's whole part. Otherwise 0,
    /// of [`Origin::Default`] (7.7.3.3).
    pub fn rotate(&self) -> Sourced<i64> {
        self.rotate
    }

    /// Its `/UserUnit`, a positive number, which isn't inherited. Otherwise 1,
    /// of [`Origin::Default`] (7.7.3.3).
    pub fn user_unit(&self) -> Sourced<f64> {
        self.user_unit
    }
}

/// The attributes a page inherits (ISO 32000-2, 7.7.3.3 and 7.7.3.4).
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

/// Each attribute's key, in [`Inheritable`]'s order.
const KEYS: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// An inheritable attribute of a page.
#[derive(Debug, Clone, Copy, PartialEq)]
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
pub struct Sourced<T> {
    /// The value.
    pub value: T,
    /// Where it came from.
    pub origin: Origin,
}

/// Where a page's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The page's own dictionary.
    Own,
    /// A node on its `/Parent` chain: that node's number, none for a `/Parent`
    /// written in place.
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
    /// More pages or visits to nodes than [`Limits::pages`], or by PDFium's
    /// rules, a count of the tree past what PDFium would give.
    #[error("the page tree lists too many pages")]
    TooManyPages,
    /// A catalog with no `/Pages` that's a dictionary, or, by PDFium's rules,
    /// no pages under it.
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
#[derive(Debug, PartialEq, Eq)]
struct Parent {
    /// Its number: none for one written in place.
    id: Option<ObjectId>,
    up: Option<Arc<Parent>>,
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

struct Walk<'o, 'r, R: ?Sized, S: ?Sized> {
    objects: &'r R,
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
    /// The numbered object what's being read is in.
    object: Option<ObjectId>,
}

impl<'o, R: Resolve<'o> + ?Sized, S: Sink + ?Sized> Walk<'o, '_, R, S> {
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
            self.walk_pdfium_root(id, root, object.at)?;
            if self.pages.is_empty() {
                return Err(Error::NoPages);
            }
            return Ok(());
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
        let (mut visits, mut counted) = (0, 0);
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
                self.check_count(dict, at, first)?;
                continue;
            };
            frame.next += 1;
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
            if !self.pdfium && matches!(object.value(), Value::Stream(_)) {
                self.say(object.at, Fault::PageTreeKidNotDictionary);
            }
            match self.class(dict, object.at)? {
                Class::Page => {
                    self.object = id.or(self.object);
                    self.check_parent(dict, object.at, holder)?;
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
                    if let (Some(id), true) = (id, self.checking()) {
                        if !self.seen_nodes.insert(id) {
                            self.say(kid.at, Fault::PageTreeRepeated { id });
                        }
                    }
                    self.object = id.or(self.object);
                    self.check_parent(dict, object.at, holder)?;
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
            attributes[i] = match self.found(dict, key)? {
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
                attributes[i] = match self.found(dict, key)? {
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
    /// it, when a checker wants to know.
    fn check_parent(
        &mut self,
        dict: &'o Dict,
        at: Option<Span>,
        holder: Option<ObjectId>,
    ) -> Result<(), Error> {
        if !self.checking() {
            return Ok(());
        }
        let parent = match dict.get(b"Parent") {
            Some(written) => match self.reach(written, HOPS)? {
                Reached::Object(_, id) => id,
                _ => None,
            },
            None => None,
        };
        if parent.is_none() || parent != holder {
            self.say(at, Fault::PageTreeParent);
        }
        Ok(())
    }

    /// Holds a node's `/Count` to the pages found under it, when a checker
    /// wants to know.
    fn check_count(&mut self, node: &'o Dict, at: Option<Span>, first: usize) -> Result<(), Error> {
        if !self.checking() {
            return Ok(());
        }
        let count = self
            .found(node, b"Count")?
            .and_then(|(_, count)| count.as_integer());
        let under = (self.pages.len() - first) as i64;
        if count != Some(under) || under == 0 {
            self.say(at, Fault::PageTreeCount);
        }
        Ok(())
    }

    fn say(&mut self, at: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            let place = Place {
                object: self.object,
                at,
            };
            self.sink.fault(place, fault);
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

#[cfg(test)]
mod tests;
