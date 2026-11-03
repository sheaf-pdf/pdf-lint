//! The common structures, read through pdf-semantics: the page tree (ISO
//! 32000-2, 7.7.3), by ISO's rules and by PDFium's; each name and number
//! tree the walk reaches (7.9.6, 7.9.7); and the catalog's XMP packet
//! (14.3.2), which the extensions a file declares are read from
//! ([`extension::read`](crate::extension::read)).
//!
//! The page tree is walked before the walk from the trailer, from the
//! catalog the trailer as written leads to, and what it told is found after
//! the content. The trees are read after it: each root the walk reached as
//! a name or number tree in the newest revision, once, in the order it
//! reached them, a root with a number given by it, so that what's told of
//! the root is in its object. Each reads the file's objects as the
//! predicates resolve them
//! ([`Objects`]), which says why one doesn't read as a document's own
//! resolver does, so that an object that takes a password or is past a cap
//! ends a walk, where one that doesn't parse is the file's fault. What each
//! tells is kept ([`Kept`]), each walked to its end.
//!
//! The pages ISO's walk reads, where it reads the tree whole, are what the
//! predicates' `fn:NumberOfPages` and `fn:PageProperty` read. Of what it
//! tells, what the walk can't say is a [`Rule::PageTree`] finding: a page
//! or a node listed again, a node listed inside itself, a `/Parent` chain
//! that loops, and a reference that names another reference, always; a
//! root's `/Parent`, a `/Parent` that isn't the numbered node listing it, a
//! `/Type` that doesn't say what's there, a tree of no pages, and a node's
//! `/Count` that's an integer other than the pages under it, where the walk
//! found nothing by the model's rules in that object at that key. The rest
//! the walk says, or the object's own fault there. Each kind once in each
//! object it's in, at its first place there, its value the fault's words
//! with how many times; what's in no object, the root written in the
//! catalog, in the catalog's, held where that is; its path and table the
//! object's where the walk first visited it as one of the tree's tables
//! ([`Walk::note_structure`]), `-` where it didn't. What stops the tree
//! being read whole that isn't the file's fault (an object that takes a
//! password or is past a cap, a tree deeper or with more pages than is
//! read) is counted as not checked, once, where the catalog is held, and the
//! predicates are given no pages; so is a node's `/Count` or `/Parent` that
//! doesn't read so, which isn't checked.
//!
//! The pages the walk by PDFium's rules reads, at the build pdf-semantics'
//! tests hold it to, are set beside ISO's, where both read a table of them,
//! or PDFium's finds no page under the root, which PDFium counts all the
//! same ([`pages::counted`]), and neither met an object that doesn't read:
//! where they part is a [`Rule::Viewer`] finding, a viewer's reading, not
//! ISO 32000-2's. Each kind once in a file: the count, at the root's
//! `/Count`; the first page, of those both count, a viewer has another
//! object at or no page, with how many, at the tree's page there; and the
//! first page that's the same object to both, a viewer loading it, whose
//! media box, crop box or rotation a viewer reads otherwise, as PDFium
//! takes a box of no area, with how many, at that page; held where it is,
//! its path and table as a page tree finding's. A walk by PDFium's rules
//! that stops at a node listed inside itself or a `/Parent` chain that
//! loops is pdf-semantics' choice, not PDFium's reading, and compares
//! nothing; where an object of the tree doesn't read, what either makes of
//! it is the reader's, and they aren't set side by side, nor where a cap or
//! a password stops PDFium's walk alone, which is counted as not checked,
//! once, where the catalog is held.
//!
//! Of what a tree's walk tells, what [`Walk::tree`] doesn't say is a
//! [`Rule::Tree`] finding: a node reached again, a kid written in place,
//! `/Limits` that aren't the node's least and greatest keys, a key not past
//! the one before it or outside a node's `/Limits`, and a node under the
//! root with neither `/Kids` nor entries (the root's is the walk's own).
//! Each kind once in each object it's in, at its first place there, its
//! value the fault's words with how many times; in the object the fault is
//! in, or, for a root written in place, the object that holds it, held
//! where that is; its path, table and key those [`Walk::tree`] gives the
//! root's findings. A kind in a node two trees share is told by the first
//! alone, as the walk checks the node as the first's. What stops a tree
//! being read whole that isn't the file's fault (an object that takes a
//! password or is past a cap, a key whose bytes the reader didn't give, a
//! tree deeper or with more nodes than is read, and `/Limits` that don't
//! read for a password or a cap, which aren't checked) is counted as not
//! checked, once a tree for each, where its root is held; what was read
//! before it is found all the same.
//!
//! What the packet's read told is a [`Rule::Xmp`] finding, last: a mark of
//! UTF-16 it begins with, an attribute and a claim's value that don't read
//! as XML, each kind once, at its first place, with how many times, and
//! XML that stops the read, at the byte it stops at, in the XML reader's
//! words; in the catalog's metadata stream, at the bytes in its data,
//! held where it is, its path the stream's where the walk first visited it
//! as `Metadata` ([`Walk::note_structure`]), `-` where it didn't. A packet
//! longer, deeper or binding more namespaces at once than is read is
//! counted as not checked, once, where the stream is held. Where the stream
//! isn't known, as for a document given its packet with no `/Metadata`,
//! what the packet tells has no object to be in, and is counted so, once,
//! where the catalog is held.

use std::collections::HashSet;
use std::mem::{self, Discriminant};
use std::rc::Rc;

use pdf_semantics::pages::{self, Page, Pages, Rect, pages};
use pdf_semantics::tree::{names, numbers};
use pdf_semantics::{Fault, Limits, Place, Sink, xmp};

use super::content::times;
use super::kept::Kept;
use super::{Finding, Objects, REFERENCES, Rule, Trail, Visit, Walk, entry_at};
use crate::object::{Dict, Object, ObjectId, Span};
use crate::source::Source;
use crate::{Rules, Unreadable, Why};

/// What the structures are read under: pdf-semantics' caps, which ISO
/// 32000-2 doesn't set.
const LIMITS: Limits = Limits::DEFAULT;

/// Notes whether a walk told of an object that doesn't read
/// ([`Fault::Damaged`]), the one fault a walk by PDFium's rules tells, and
/// keeps nothing else.
#[derive(Default)]
struct Damage {
    /// Whether one was told.
    told: bool,
}

impl Sink for Damage {
    fn fault(&mut self, _: Place, fault: Fault) {
        self.told |= matches!(fault, Fault::Damaged { .. });
    }
}

/// A name or number tree's root the walk reached ([`Walk::tree`]), which
/// [`Walk::structures`] reads again.
pub(super) struct Root<'a> {
    /// The root as written.
    object: &'a Object,
    /// Its number, where it's an indirect object.
    id: Option<ObjectId>,
    /// Whether it's a name tree's, not a number tree's.
    names: bool,
    /// The numbered object it is or is in, which holds what's told of the
    /// tree in no object: none for a table's trailer.
    holder: Option<ObjectId>,
    /// The walk's way to it, the path of the tree's findings.
    trail: Rc<Trail<'a>>,
    /// The table and key of the row whose value it is.
    table: &'static str,
    key: &'static str,
}

/// Why a tree isn't read whole, other than for the file's faults: an
/// object of it takes a password the reader wasn't given.
const LOCKED: &str = "a tree with an object that takes a password";

/// Why a tree isn't read whole: an object of it is past a cap the reader
/// reads under, or a chain of references to it is longer than is followed.
const CAPPED: &str = "a tree with an object past a cap it's read under";

/// Why a tree isn't read whole: the source didn't give an object's bytes.
const UNGIVEN: &str = "a tree with an object whose bytes the source didn't give";

/// Why a tree isn't read whole: an object of it doesn't read, for another
/// reason a reader gives.
const UNREAD: &str = "a tree with an object that doesn't read";

/// Why a tree isn't read whole: it goes deeper than pdf-semantics reads a
/// tree ([`Limits::tree_depth`]).
const DEEP: &str = "a tree deeper than it's read";

/// Why a tree isn't read whole: it has more nodes than pdf-semantics reads
/// of a tree ([`Limits::tree_nodes`]).
const NODES: &str = "a tree with more nodes than are read";

/// Why a tree isn't read whole: a key's bytes are ciphertext the reader
/// couldn't decrypt, so where it goes can't be told.
const KEY: &str = "a tree with a key whose bytes aren't known";

/// Why a tree isn't read whole, given what stopped its walk.
fn unread(why: Why) -> &'static str {
    match why {
        Why::Locked => LOCKED,
        Why::Limit => CAPPED,
        Why::Source => UNGIVEN,
        _ => UNREAD,
    }
}

/// What stopped `walk`, a tree's, from reading it, run to its end: why an
/// object didn't read, where one didn't.
fn stopped<T>(walk: impl Iterator<Item = Result<T, Unreadable>>) -> Option<Why> {
    walk.filter_map(Result::err).map(|e| e.why).last()
}

/// `value`, a tree finding's, without the count it ends with, `, 3 times`,
/// so that how many times a kind was found doesn't tell two findings apart
/// ([`super::like`]). The walk's own values never end so.
pub(super) fn uncounted(value: &str) -> &str {
    let Some((what, count)) = value.rsplit_once(", ") else {
        return value;
    };
    match count.strip_suffix(" times") {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => what,
        _ => value,
    }
}

/// What the page tree's walk by ISO's rules told ([`page_trees`]), which
/// [`Walk::structures`] finds in, and why it didn't read the tree whole,
/// where that isn't the file's fault.
#[derive(Default)]
pub(super) struct PageTree<'a> {
    /// What it told of what's found, or stops a check.
    kept: Kept<Fault>,
    /// Why it stopped: an object on the way that takes a password or is
    /// past a cap, or a tree deeper or with more pages than is read.
    unread: Option<&'static str>,
    /// Where the walk by PDFium's rules parts from it.
    viewer: Viewer,
    /// The root, where it's written in the catalog, as the catalog holds
    /// it: what the walk tells in no object is in it, but at the catalog's
    /// `/Pages` itself.
    root: Option<&'a Object>,
}

/// The faults of the page tree's walk by ISO's rules that
/// [`Walk::structures`] reads, kept: those it can find ([`page_tree_key`]),
/// a reference that names another, an object that doesn't read and what
/// isn't checked; so that a kind's first and its count are of those, and
/// never of one it doesn't find by what the fault holds.
struct PageFaults<'k>(&'k mut Kept<Fault>);

impl Sink for PageFaults<'_> {
    fn fault(&mut self, at: Place, fault: Fault) {
        let read = matches!(
            fault,
            Fault::PageTreeChain | Fault::Damaged { .. } | Fault::Unchecked { .. }
        );
        if read || page_tree_key(&fault).is_some() {
            self.0.fault(at, fault);
        }
    }
}

/// Where a viewer's pages, as the page tree's walk by PDFium's rules reads
/// them, part from the walk's by ISO's ([`page_trees`]), which
/// [`Walk::structures`] finds as [`Rule::Viewer`]; and why they weren't set
/// side by side, where they weren't.
#[derive(Default)]
pub(super) struct Viewer {
    /// Each kind they part by, in turn: where, its key, and what it says.
    parted: Vec<(Place, &'static str, String)>,
    /// Why they weren't set side by side, where that's counted: an object
    /// of the tree that doesn't read, so that what either walk makes of it
    /// is the reader's; or a cap or a password that stopped the walk by
    /// PDFium's rules.
    unread: Option<&'static str>,
}

impl Viewer {
    /// Not set side by side, for `why`.
    fn unread(why: &'static str) -> Viewer {
        Viewer {
            parted: Vec::new(),
            unread: Some(why),
        }
    }

    /// What `stopped` the walk by PDFium's rules makes of the two: a cap,
    /// a password or the source counted; a node listed inside itself or a
    /// `/Parent` chain that loops, where pdf-semantics stops and PDFium goes
    /// on, nothing, as the page tree's findings say each.
    fn stopped(stopped: &pages::Error) -> Viewer {
        match stopped {
            pages::Error::Unreadable(unreadable) => Viewer::unread(match unreadable.why {
                Why::Locked => VIEWER_LOCKED,
                Why::Limit => VIEWER_CAPPED,
                _ => VIEWER_UNREAD,
            }),
            pages::Error::TooDeep | pages::Error::TooManyPages => Viewer::unread(VIEWER_CAPPED),
            _ => Viewer::default(),
        }
    }
}

/// Why a viewer's pages aren't set beside the tree's: an object of the
/// tree doesn't read, so that what either walk makes of it is the reader's.
const VIEWER_UNREAD: &str = "a viewer's pages beside a page tree with an object that doesn't read";

/// Why a viewer's pages aren't set beside the tree's: an object on the way
/// takes a password the reader wasn't given.
const VIEWER_LOCKED: &str =
    "a viewer's pages beside a page tree with an object that takes a password";

/// Why a viewer's pages aren't set beside the tree's: the walk by PDFium's
/// rules is past a cap it's read under, an object's, the tree's depth or
/// its pages.
const VIEWER_CAPPED: &str = "a viewer's pages beside a page tree past a cap it's read under";

/// Where the walk first visited an object of the page tree as one of its
/// tables ([`Walk::note_structure`]).
pub(super) struct Visited<'a> {
    /// The table, `PageTreeNodeRoot`, `PageTreeNode` or `PageObject`.
    table: &'static str,
    /// The walk's way to it, the path of its findings.
    trail: Rc<Trail<'a>>,
}

/// The page tree's tables, as the walk reaches what it's made of.
const PAGE_TREE: [&str; 3] = ["PageTreeNodeRoot", "PageTreeNode", "PageObject"];

/// Why the page tree isn't read whole, other than for the file's faults:
/// an object of it takes a password the reader wasn't given.
const PAGES_LOCKED: &str = "a page tree with an object that takes a password";

/// Why the page tree isn't read whole: an object of it is past a cap the
/// reader reads under, or a chain of references to it is longer than is
/// followed.
const PAGES_CAPPED: &str = "a page tree with an object past a cap it's read under";

/// Why the page tree isn't read whole: the source didn't give an object's
/// bytes.
const PAGES_UNGIVEN: &str = "a page tree with an object whose bytes the source didn't give";

/// Why the page tree isn't read whole: an object of it doesn't read, for
/// another reason a reader gives.
const PAGES_UNREAD: &str = "a page tree with an object that doesn't read";

/// Why the page tree isn't read whole: it goes deeper than pdf-semantics
/// reads one ([`Limits::page_depth`]).
const PAGES_DEEP: &str = "a page tree deeper than it's read";

/// Why the page tree isn't read whole: it has more pages, or nodes, than
/// pdf-semantics reads of one ([`Limits::pages`]).
const PAGES_MANY: &str = "a page tree with more pages than are read";

/// Why the page tree isn't read whole, given why an object of it didn't
/// read: none where that's the file's fault, an object that doesn't parse,
/// which the walk finds.
fn pages_unread(why: Why) -> Option<&'static str> {
    match why {
        Why::Damaged => None,
        Why::Locked => Some(PAGES_LOCKED),
        Why::Limit => Some(PAGES_CAPPED),
        Why::Source => Some(PAGES_UNGIVEN),
        _ => Some(PAGES_UNREAD),
    }
}

/// Walks the page tree that `trailer`'s catalog leads to, by ISO's rules,
/// what it tells kept, and by PDFium's, noting whether an object didn't
/// read; each to its end. Gives what ISO's walk told, and where PDFium's
/// parts from it, and the dictionaries of its pages, in order, where it
/// read the tree whole: which ISO's rules always give each page. A catalog
/// that isn't there, or a tree with no `/Pages` to walk, gives none, which
/// the walk finds.
pub(super) fn page_trees<'a>(
    source: &'a dyn Source,
    trailer: &'a Dict,
) -> (PageTree<'a>, Option<Vec<&'a Dict>>) {
    let mut tree = PageTree::default();
    let catalog = match pdf_semantics::catalog(Objects(source), trailer) {
        Ok(Some(catalog)) => catalog,
        Ok(None) => return (tree, None),
        Err(unreadable) => {
            tree.unread = pages_unread(unreadable.why);
            return (tree, None);
        }
    };
    tree.root = root(source, catalog).and_then(|(id, root)| id.is_none().then_some(root));
    let iso = pages(
        Objects(source),
        catalog,
        Rules::Iso,
        &LIMITS,
        &mut PageFaults(&mut tree.kept),
    );
    let mut damage = Damage::default();
    let pdfium = pages(
        Objects(source),
        catalog,
        Rules::Pdfium,
        &LIMITS,
        &mut damage,
    );
    let read = match iso {
        Ok(read) => read,
        Err(stopped) => {
            tree.unread = match stopped {
                pages::Error::Unreadable(unreadable) => pages_unread(unreadable.why),
                pages::Error::TooDeep => Some(PAGES_DEEP),
                pages::Error::TooManyPages => Some(PAGES_MANY),
                _ => None,
            };
            return (tree, None);
        }
    };
    // PDFium's pages are set beside ISO's where its walk reads a table, or
    // finds no page under the root, which PDFium counts all the same; where
    // it stops otherwise, at a node listed inside itself, a loop, a cap or a
    // password, pdf-semantics chose it should, which is none of PDFium's
    // reading. Not where an object doesn't read, which is the reader's.
    let damaged =
        (tree.kept.told().iter()).any(|(_, fault, _)| matches!(fault, Fault::Damaged { .. }));
    tree.viewer = match pdfium {
        Ok(_) | Err(pages::Error::NoPages) if damaged || damage.told => {
            Viewer::unread(VIEWER_UNREAD)
        }
        Ok(viewer) => Viewer {
            parted: parted(source, catalog, &read, Some(&viewer), viewer.counted()),
            unread: None,
        },
        Err(pages::Error::NoPages) => match pages::counted(Objects(source), catalog, &LIMITS) {
            Ok(counted) => Viewer {
                parted: parted(source, catalog, &read, None, counted),
                unread: None,
            },
            Err(stopped) => Viewer::stopped(&stopped),
        },
        Err(stopped) => Viewer::stopped(&stopped),
    };
    let dicts = read.iter().filter_map(|page| page.dict).collect();
    (tree, Some(dicts))
}

/// Where `viewer`'s pages, read by PDFium's rules (none for none under the
/// root), and the count PDFium gives, `counted`, part from `tree`'s, read
/// by ISO's from `catalog`: each kind once, where it is, its key and what it
/// says. The count, at the root's `/Count`, else the root; the first page,
/// of those both count, that a viewer has another object at or no page,
/// with how many, at the tree's page there, else, one written in place, the
/// root; and the first page that's the same object to both, a viewer
/// loading it, whose media box, crop box or rotation a viewer reads
/// otherwise, the first of those that is, with how many, at that page's
/// key, else the page, or the root.
fn parted<'a>(
    source: &'a dyn Source,
    catalog: &'a Dict,
    tree: &Pages<'a>,
    viewer: Option<&Pages<'a>>,
    counted: usize,
) -> Vec<(Place, &'static str, String)> {
    let (root, written) = match root(source, catalog) {
        Some((id, root)) => (id, Some(root)),
        None => (None, None),
    };
    let root_at = written.and_then(|root| root.at);
    // Where a finding of a page is: at its key, else the page, where it
    // has a number; else the root.
    let at_page = |page: &Page<'a>, key: Option<&[u8]>| match page.id {
        Some(id) => {
            let own = (key.zip(page.dict)).and_then(|(key, dict)| dict.entry(key));
            let at = own.and_then(entry_at);
            let at = at.or_else(|| source.get(id).ok().and_then(|page| page.at));
            Place::new(Some(id), at)
        }
        None => Place::new(root, root_at),
    };
    let mut parted = Vec::new();
    if counted != tree.len() {
        let count = (written.and_then(Object::as_dict))
            .and_then(|root| root.entry(b"Count"))
            .and_then(entry_at)
            .or(root_at);
        let said = format!(
            "a viewer counts {}, where the tree lists {}",
            pages_of(counted),
            tree.len()
        );
        parted.push((Place::new(root, count), "Count", said));
    }
    // Each page both count, and a viewer's there, if it has one.
    let both = (0..tree.len().min(counted))
        .filter_map(|i| Some((i, tree.get(i)?, viewer.and_then(|viewer| viewer.get(i)))));
    let mut other = both.clone().filter_map(|(i, page, theirs)| {
        let loaded = theirs.filter(|theirs| theirs.dict.is_some());
        match loaded {
            Some(theirs) if same(page, theirs) => None,
            _ => Some((i, page, loaded)),
        }
    });
    if let Some((i, page, theirs)) = other.next() {
        let has = match theirs.map(|theirs| theirs.id) {
            None => " has no page".to_string(),
            Some(Some((n, g))) => format!("'s is {n} {g} R"),
            Some(None) => "'s is another, written in place".to_string(),
        };
        let said = format!(
            "page {} is {}, where a viewer{has}",
            i + 1,
            written_as(page)
        );
        parted.push((at_page(page, None), "-", times(&said, 1 + other.count())));
    }
    let mut sized = both.filter_map(|(i, page, theirs)| {
        let theirs = theirs.filter(|theirs| theirs.dict.is_some() && same(page, theirs))?;
        Some((i, page, sizes(page, theirs)?))
    });
    if let Some((i, page, (key, what, ours, theirs))) = sized.next() {
        let said = format!(
            "page {}'s {what} is {ours}, where a viewer's is {theirs}",
            i + 1
        );
        let at = at_page(page, Some(key.as_bytes()));
        parted.push((at, key, times(&said, 1 + sized.count())));
    }
    parted
}

/// The page tree's root, as `catalog`'s `/Pages` leads to it through any
/// references: the number of the last, none where it's written in place,
/// and the root.
fn root<'a>(source: &'a dyn Source, catalog: &'a Dict) -> Option<(Option<ObjectId>, &'a Object)> {
    let (mut id, mut at) = (None, catalog.get(b"Pages")?);
    for _ in 0..REFERENCES {
        let Some(to) = at.as_reference() else {
            return Some((id, at));
        };
        at = source.get(to).ok()?;
        id = Some(to);
    }
    None
}

/// Whether `page` and `theirs`, each walk's at one place, are the same
/// object: of the same number, or written in place, the same dictionary.
fn same(page: &Page<'_>, theirs: &Page<'_>) -> bool {
    match (page.id, theirs.id, page.dict, theirs.dict) {
        (Some(id), Some(their), ..) => id == their,
        (None, None, Some(dict), Some(their)) => std::ptr::eq(dict, their),
        _ => false,
    }
}

/// Where a viewer reads `page`'s size otherwise than the tree does, of the
/// same page, the first that differs of its media box, crop box and
/// rotation: its key, what it is, and each one's. A viewer's boxes are as
/// PDFium takes them: a media box of no area US Letter, and a crop box of
/// no area the media box.
fn sizes(
    page: &Page<'_>,
    theirs: &Page<'_>,
) -> Option<(&'static str, &'static str, String, String)> {
    let shown = |rect: Rect, or: Rect| if no_area(rect) { or } else { rect };
    let their_media = shown(theirs.media_box().value, Rect::LETTER);
    let media = (page.media_box().value, their_media);
    let crop = (
        page.crop_box().value,
        shown(theirs.crop_box().value, their_media),
    );
    let rotate = (page.rotate().value, theirs.rotate().value);
    let rect = |rect: Rect| rect.0.map(|n| n.to_string()).join(" ");
    Some(if media.0 != media.1 {
        ("MediaBox", "media box", rect(media.0), rect(media.1))
    } else if crop.0 != crop.1 {
        ("CropBox", "crop box", rect(crop.0), rect(crop.1))
    } else if rotate.0 != rotate.1 {
        let (ours, theirs) = (rotate.0.to_string(), rotate.1.to_string());
        ("Rotate", "rotation", ours, theirs)
    } else {
        return None;
    })
}

/// Whether `rect` has no area, as PDFium holds a box, in `f32`s: its two
/// edges across or its two up and down at one place.
fn no_area(Rect([a, b, c, d]): Rect) -> bool {
    let at = |x: f64, y: f64| x as f32 == y as f32;
    at(a, c) || at(b, d)
}

/// `page`'s object, as a finding says it: its reference, or that it's
/// written in place.
fn written_as(page: &Page<'_>) -> String {
    match page.id {
        Some((n, g)) => format!("{n} {g} R"),
        None => "written in place".to_string(),
    }
}

/// `n` pages, as a finding says it.
fn pages_of(n: usize) -> String {
    match n {
        1 => "1 page".to_string(),
        n => format!("{n} pages"),
    }
}

/// Of what the page tree's walk by ISO's rules tells, each kind
/// [`Walk::structures`] finds, but a reference that names another, which
/// it finds at the key holding it: the key it's at, and whether it's found
/// only where the walk found nothing by the model's rules ([`MODEL`]) in
/// that object at that key. None for what the walk says, or the object's
/// own fault there: a root or a kid written in place, a kid that isn't a
/// dictionary, a node with no kids, a `/Parent` written in place, or under
/// a node written in place, which no `/Parent` can name, a page with no
/// `/MediaBox`, an object that doesn't read; and a `/Count` that isn't an
/// integer, or is the pages under it, an empty tree's 0, which
/// [`Fault::PageTreeEmpty`] says.
fn page_tree_key(fault: &Fault) -> Option<(&'static str, bool)> {
    Some(match *fault {
        Fault::PageTreeRepeated { .. } | Fault::PageTreeCycle { .. } => ("Kids", false),
        Fault::PageTreeParentLoop => ("Parent", false),
        Fault::PageTreeRootParent | Fault::PageTreeParent { lister: Some(_) } => ("Parent", true),
        Fault::PageTreeType => ("Type", true),
        Fault::PageTreeEmpty => ("Kids", true),
        Fault::PageTreeCount {
            count: Some(count),
            under,
        } if usize::try_from(count).ok() != Some(under) => ("Count", true),
        _ => return None,
    })
}

/// The keys a page tree fault told only where the walk found nothing is at
/// ([`page_tree_key`]).
const KEYS: [&str; 4] = ["Parent", "Type", "Kids", "Count"];

/// The model's rules that say what's wrong at a key, which a page tree
/// fault told only where the walk found nothing ([`page_tree_key`]) gives
/// way to.
const MODEL: [Rule; 8] = [
    Rule::Missing,
    Rule::Type,
    Rule::Value,
    Rule::Indirect,
    Rule::Direct,
    Rule::Special,
    Rule::Unresolved,
    Rule::Unlisted,
];

/// What the catalog's XMP packet told, read once for the extensions the
/// file declares ([`extension::read`](crate::extension::read)), which
/// [`Walk::structures`] finds in.
#[derive(Default)]
pub(crate) struct Packet {
    /// The stream it's the data of, as [`xmp::metadata_stream`] names it:
    /// none where that isn't known, as for a document given its packet with
    /// no `/Metadata` to name the stream, or where there's no packet.
    pub(crate) stream: Option<ObjectId>,
    /// Its length.
    pub(crate) len: usize,
    /// What it told, each place in that stream, at bytes of its data.
    pub(crate) kept: Kept<Fault>,
    /// What stopped it being read, if anything did.
    pub(crate) stopped: Option<xmp::Error>,
}

/// Why a packet isn't read whole, other than for the file's faults: it's
/// longer than pdf-semantics reads ([`Limits::xmp_bytes`]).
const XMP_LONG: &str = "a packet longer than it's read";

/// Why a packet isn't read whole: it nests deeper than pdf-semantics reads
/// ([`Limits::xmp_depth`]).
const XMP_DEEP: &str = "a packet nested deeper than it's read";

/// Why a packet isn't read whole: it binds more namespaces at once than
/// pdf-semantics reads ([`Limits::xmp_bindings`]).
const XMP_BINDINGS: &str = "a packet binding more namespaces at once than are read";

/// Why a packet isn't read whole, for another reason its reader gives.
const XMP_UNREAD: &str = "a packet read no further, for another reason";

/// Why what a packet tells isn't found: the stream it's the data of isn't
/// known, so it has no object to be in.
const XMP_UNPLACED: &str = "the packet's stream isn't known";

impl<'a> Walk<'a> {
    /// Keeps `root`, the visit of a name tree's root when `names`, else a
    /// number tree's, the value of `key` of its row, for
    /// [`Walk::structures`]. Not in an earlier revision's trailer, whose
    /// objects are the file's where no later revision wrote them, as the
    /// walk from the trailer reaches them.
    pub(super) fn note_root(&mut self, root: &Visit<'a>, names: bool, key: &'static str) {
        if self.revision.is_none() {
            self.roots.push(Root {
                object: root.object,
                id: root.id(),
                names,
                holder: root.holder,
                trail: root.trail.clone(),
                table: root.table.name.as_str(),
                key,
            });
        }
    }

    /// Keeps where the walk first visited `v`'s object as one of the page
    /// tree's tables, by its number, or, the root written in the catalog, by
    /// none, as pdf-semantics places what's in it; and, the packet's stream
    /// first visited as `Metadata`, where: the path and table of what
    /// [`Walk::structures`] finds there. The root written in place is the
    /// catalog's, by what it is: a node written in place elsewhere, as a
    /// page's `/Parent` may be, isn't it. Not in an earlier revision's
    /// trailer.
    pub(super) fn note_structure(&mut self, v: &Visit<'a>) {
        let table = v.table.name.as_str();
        if self.revision.is_some() {
            return;
        }
        let id = v.id();
        if table == "Metadata" {
            if id.is_some() && id == self.packet.stream && self.packet_at.is_none() {
                self.packet_at = Some(v.trail.clone());
            }
            return;
        }
        if !PAGE_TREE.contains(&table) {
            return;
        }
        let root = self.page_tree.root;
        if id.is_none() && !root.is_some_and(|root| std::ptr::eq(root, v.object)) {
            return;
        }
        (self.page_tree_at.entry(id)).or_insert_with(|| Visited {
            table,
            trail: v.trail.clone(),
        });
    }

    /// Finds what the page tree's walk by ISO's rules, and each name and
    /// number tree's, read through pdf-semantics, tell that the walk
    /// doesn't say, where the page tree's walk by PDFium's rules parts from
    /// ISO's, and what the XMP packet's read told: the page tree first,
    /// then PDFium's, then the trees from their roots in the order the walk
    /// reached them, then the packet: see the module's documentation.
    pub(super) fn structures(&mut self) {
        let PageTree {
            kept,
            unread,
            viewer,
            root,
        } = mem::take(&mut self.page_tree);
        let catalog = self.catalog();
        let root = root.and_then(Object::as_dict);
        self.page_tree_told(&kept, unread, catalog, root);
        self.viewer_told(viewer, catalog);
        let roots = mem::take(&mut self.roots);
        // Each numbered object's kinds told, so that a node two trees
        // share is told of once.
        let mut told = HashSet::new();
        for root in &roots {
            self.tree_told(root, &mut told);
        }
        let packet = mem::take(&mut self.packet);
        self.packet_told(packet, catalog);
    }

    /// The catalog's number, which what the page tree's walks tell in no
    /// object is in: the root written in it, or at its `/Pages`; a catalog
    /// written in the trailer is in what the trailer is.
    fn catalog(&self) -> Option<ObjectId> {
        match self.trailer.get(b"Root").map(|root| self.resolve(root)) {
            Some(Ok((id, _))) => id.or(self.trailer_id),
            Some(Err(id)) => Some(id),
            None => self.trailer_id,
        }
    }

    /// The path and table of what's found in `object`, of the page tree:
    /// the object's where the walk first visited it as one of the tree's
    /// tables, the root written in place's by none, `-` and `-` where it
    /// didn't.
    fn page_tree_path(&self, object: Option<ObjectId>) -> (String, String) {
        match self.page_tree_at.get(&object) {
            Some(at) => (at.trail.path(), at.table.to_string()),
            None => ("-".to_string(), "-".to_string()),
        }
    }

    /// Finds what the page tree's walk by ISO's rules told, `kept`, that
    /// the walk doesn't say, each kind once in each object, a
    /// [`Rule::PageTree`] finding, what's in no object in `catalog`, or in
    /// `root`, the root written in it; and counts what stopped it reading
    /// the tree whole, `unread`, and what it couldn't check, but the file's
    /// faults, as not checked, each once, where the catalog is held.
    fn page_tree_told(
        &mut self,
        kept: &Kept<Fault>,
        unread: Option<&'static str>,
        catalog: Option<ObjectId>,
        root: Option<&Dict>,
    ) {
        // What the walk found by the model's rules, where a kind is told
        // only where it found nothing.
        let found: HashSet<(Option<ObjectId>, &'static str)> = (self.report.findings.iter())
            .filter(|f| f.revision.is_none() && MODEL.contains(&f.rule))
            .filter_map(|f| Some((f.object, KEYS.into_iter().find(|&k| k == f.key)?)))
            .collect();
        let mut told = HashSet::new();
        let mut stops: Vec<_> = unread.into_iter().collect();
        for &(place, fault, n) in kept.told() {
            let object = place.object.or(catalog);
            let key = match fault {
                Fault::PageTreeChain => {
                    let root = root.filter(|_| place.object.is_none());
                    self.key_holding(object, root, place.at)
                }
                Fault::Unchecked { why, .. } => {
                    stops.extend(pages_unread(why));
                    continue;
                }
                _ => match page_tree_key(&fault) {
                    Some((key, true)) if found.contains(&(object, key)) => continue,
                    Some((key, _)) => key.to_string(),
                    None => continue,
                },
            };
            if !told.insert((object, mem::discriminant(&fault))) {
                continue;
            }
            let Some(made) = self.made(object) else {
                continue;
            };
            let (path, table) = self.page_tree_path(place.object);
            self.report.findings.push(Finding {
                object,
                made,
                path,
                table,
                key,
                rule: Rule::PageTree,
                value: times(&fault.to_string(), n),
                at: place.at,
                revision: None,
            });
        }
        if self.made(catalog).is_some() {
            stops.sort_unstable();
            stops.dedup();
            for why in stops {
                self.unchecked("-", why.to_string(), "PageTreeNodeRoot", "-");
            }
        }
    }

    /// Finds where the page tree's walk by PDFium's rules parts from ISO's,
    /// `viewer`, each kind once, a [`Rule::Viewer`] finding where it is,
    /// what's in no object in `catalog`, its path and table as a page tree
    /// finding's; and counts the two not set side by side, an object of the
    /// tree not reading, or a cap or a password stopping PDFium's, as not
    /// checked, where the catalog is held.
    fn viewer_told(&mut self, viewer: Viewer, catalog: Option<ObjectId>) {
        for (place, key, value) in viewer.parted {
            let object = place.object.or(catalog);
            let Some(made) = self.made(object) else {
                continue;
            };
            let (path, table) = self.page_tree_path(place.object);
            self.report.findings.push(Finding {
                object,
                made,
                path,
                table,
                key: key.to_string(),
                rule: Rule::Viewer,
                value,
                at: place.at,
                revision: None,
            });
        }
        if let Some(why) = viewer.unread
            && self.made(catalog).is_some()
        {
            self.unchecked("-", why.to_string(), "PageTreeNodeRoot", "-");
        }
    }

    /// The key whose value holds the bytes `at`, as written: of `within`,
    /// a dictionary written in `object`, where one of its keys does, else of
    /// `object`'s own dictionary; `-` where none does, or where they aren't
    /// known.
    fn key_holding(
        &self,
        object: Option<ObjectId>,
        within: Option<&Dict>,
        at: Option<Span>,
    ) -> String {
        let dict = match object {
            Some(id) => self.source.get(id).ok().and_then(Object::as_dict),
            None => Some(self.trailer),
        };
        let holds = |value: Span, at: Span| {
            value.stream == at.stream && value.start <= at.start && at.end <= value.end
        };
        let Some(at) = at else {
            return "-".to_string();
        };
        let key = |dict: &Dict| {
            (dict.entries().iter())
                .find(|entry| entry.value.at.is_some_and(|value| holds(value, at)))
                .map(|entry| String::from_utf8_lossy(&entry.key).into_owned())
        };
        (within.and_then(key))
            .or_else(|| dict.and_then(key))
            .unwrap_or_else(|| "-".to_string())
    }

    /// Finds what pdf-semantics' walk of `root`'s tree tells that
    /// [`Walk::tree`] doesn't, but each numbered object's kinds in `told`,
    /// which it adds to; and counts what stops it reading the tree whole,
    /// but the file's faults, as not checked.
    fn tree_told(&mut self, root: &Root<'a>, told: &mut HashSet<(ObjectId, Discriminant<Fault>)>) {
        // Given by its number, the walk starts in the root's object.
        let reference;
        let given = match root.id {
            Some(id) => {
                reference = Object::from(id);
                &reference
            }
            None => root.object,
        };
        let objects = Objects(self.source);
        let mut kept = Kept::default();
        let (entries, ended): (&[u8], _) = match root.names {
            true => (b"Names", stopped(names(objects, given, &LIMITS, &mut kept))),
            false => (
                b"Nums",
                stopped(numbers(objects, given, &LIMITS, &mut kept)),
            ),
        };
        // A root with neither /Kids nor entries has no node under it, so a
        // node with neither is the root, which the walk says.
        let has = |dict: &Dict, key: &[u8]| dict.get(key).is_some_and(|o| !o.is_null());
        let bare = (root.object.as_dict()).is_some_and(|d| !has(d, b"Kids") && !has(d, entries));
        let mut stops: Vec<_> = ended.map(unread).into_iter().collect();
        for &(place, fault, n) in kept.told() {
            match fault {
                Fault::TreeRepeated { .. }
                | Fault::TreeKidDirect
                | Fault::TreeBadLimits
                | Fault::TreeKeyOrder
                | Fault::TreeKeyOutsideLimits => {}
                Fault::TreeNoEntries if !bare => {}
                Fault::TreeTooDeep => {
                    stops.push(DEEP);
                    continue;
                }
                Fault::TreeTooManyNodes => {
                    stops.push(NODES);
                    continue;
                }
                Fault::TreeKeyUnread => {
                    stops.push(KEY);
                    continue;
                }
                // /Limits past a cap or a password, so not checked.
                Fault::Unchecked { why, .. } => {
                    stops.push(unread(why));
                    continue;
                }
                // The walk says the rest, and an object that doesn't read.
                _ => continue,
            }
            if let Some(id) = place.object
                && !told.insert((id, mem::discriminant(&fault)))
            {
                continue;
            }
            let object = place.object.or(root.holder);
            let Some(made) = self.made(object) else {
                continue;
            };
            self.report.findings.push(Finding {
                object,
                made,
                path: root.trail.path(),
                table: root.table.to_string(),
                key: root.key.to_string(),
                rule: Rule::Tree,
                value: times(&fault.to_string(), n),
                at: place.at,
                revision: None,
            });
        }
        if self.made(root.holder).is_some() {
            stops.sort_unstable();
            stops.dedup();
            for why in stops {
                self.unchecked("-", why.to_string(), root.table, root.key);
            }
        }
    }

    /// Finds what the XMP packet's read told, `packet`, each kind once, and
    /// XML that stopped it, at the byte it stopped at, a [`Rule::Xmp`]
    /// finding in the packet's stream, held where that is; and counts what
    /// else stopped it, a cap, as not checked, where the stream is held, and
    /// what it told where its stream isn't known, once, where `catalog` is.
    fn packet_told(&mut self, packet: Packet, catalog: Option<ObjectId>) {
        let Packet {
            stream,
            len,
            kept,
            stopped,
        } = packet;
        let mut told: Vec<(Option<Span>, String)> = (kept.told().iter())
            .filter(|(_, fault, _)| {
                matches!(
                    fault,
                    Fault::XmpNotUtf8 | Fault::XmpAttribute | Fault::XmpValue
                )
            })
            .map(|(place, fault, n)| (place.at, times(&fault.to_string(), *n)))
            .collect();
        let mut unread = None;
        if let Some(stopped) = stopped {
            match stopped.why {
                xmp::XmpWhy::Xml(words) => {
                    let (start, end) = (stopped.at.min(len), stopped.at.saturating_add(1).min(len));
                    let at = stream.map(|id| Span::new(Some(id), start, end));
                    told.push((at, format!("XMP metadata isn't XML: {words}")));
                }
                xmp::XmpWhy::TooLong => unread = Some(XMP_LONG),
                xmp::XmpWhy::TooDeep => unread = Some(XMP_DEEP),
                xmp::XmpWhy::TooManyBindings => unread = Some(XMP_BINDINGS),
                _ => unread = Some(XMP_UNREAD),
            }
        }
        // Where the stream isn't known, the catalog, whose /Metadata the
        // packet is, holds what's counted of it.
        let Some(made) = self.made(stream.or(catalog)) else {
            return;
        };
        match stream {
            Some(id) => {
                let (path, table) = match &self.packet_at {
                    Some(trail) => (trail.path(), "Metadata".to_string()),
                    None => ("-".to_string(), "-".to_string()),
                };
                for (at, value) in told {
                    self.report.findings.push(Finding {
                        object: Some(id),
                        made,
                        path: path.clone(),
                        table: table.clone(),
                        key: "-".to_string(),
                        rule: Rule::Xmp,
                        value,
                        at,
                        revision: None,
                    });
                }
            }
            None if !told.is_empty() => {
                self.unchecked("-", XMP_UNPLACED.to_string(), "Metadata", "-");
            }
            None => {}
        }
        if let Some(why) = unread {
            self.unchecked("-", why.to_string(), "Metadata", "-");
        }
    }
}

#[cfg(test)]
mod tests {
    //! What's noted of a walk's faults; and what's found of a tree's, the
    //! page tree's and the packet's, where, and as whose, and what's
    //! counted.

    use std::collections::BTreeSet;

    use pdf_structure::fixture;

    use super::super::{Hold, Made, Options, Report, check};
    use super::*;
    use crate::bytes;
    use crate::dict;
    use crate::source::Document;
    use crate::version::Version;

    /// A fault's place in `object`, at no bytes in particular.
    fn at(object: Option<ObjectId>) -> Place {
        Place::new(object, None)
    }

    /// A destination, the value of each name tree's key here.
    const D: &str = "[3 0 R /Fit]";

    /// A file of a catalog, object 1, with `more` in it, its page tree of a
    /// page, 2 and 3, and `objects` besides, each its number and body.
    fn file(more: &str, objects: &[(u32, String)]) -> Vec<u8> {
        let catalog = format!("<< /Type /Catalog /Pages 2 0 R {more} >>");
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>";
        let mut all: Vec<(u32, &[u8])> = vec![
            (1, catalog.as_bytes()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page.as_bytes()),
        ];
        all.extend(objects.iter().map(|(n, body)| (*n, body.as_bytes())));
        let size = all.iter().map(|&(n, _)| n).max().unwrap_or(0) + 1;
        fixture::assembled("1.7", &all, &format!("<< /Size {size} /Root 1 0 R >>"))
    }

    /// A file whose name tree of destinations has its root, object 10, as
    /// `root`, and `nodes` besides.
    fn dests(root: &str, nodes: &[(u32, &str)]) -> Vec<u8> {
        let mut objects = vec![(10, root.to_string())];
        objects.extend(nodes.iter().map(|&(n, body)| (n, body.to_string())));
        file("/Names << /Dests 10 0 R >>", &objects)
    }

    /// `file` read and checked, holding what `hold` does.
    fn checked(file: &[u8], hold: &Hold, options: &Options) -> Report {
        check(&bytes::read(file), hold, options)
    }

    /// A tree finding, as [`found`] gives it.
    type Found = (Option<u32>, String, String, String, String, Made);

    /// Each tree finding of `file`'s check, holding what `hold` does: its
    /// object's number, path, table and key, value, the bytes it's at in
    /// the file, and how its object was made.
    fn found(file: &[u8], hold: &Hold) -> Vec<Found> {
        found_by(Rule::Tree, file, hold)
    }

    /// Each finding of `rule` of `file`'s check, as [`found`] gives a tree
    /// finding.
    fn found_by(rule: Rule, file: &[u8], hold: &Hold) -> Vec<Found> {
        let report = checked(file, hold, &Options::default());
        (report.findings.iter())
            .filter(|f| f.rule == rule)
            .map(|f| {
                let at = f.at.map(|at| &file[at.start..at.end]).unwrap_or_default();
                (
                    f.object.map(|(n, _)| n),
                    f.path.clone(),
                    format!("{}/{}", f.table, f.key),
                    f.value.clone(),
                    String::from_utf8_lossy(at).into_owned(),
                    f.made,
                )
            })
            .collect()
    }

    /// A finding of [`found`]'s, in object `n` as merged, at `path`, of the
    /// row `table`, at `bytes`.
    fn tree(n: u32, path: &str, table: &str, value: &str, bytes: &str) -> Found {
        let (path, table) = (path.to_string(), table.to_string());
        let (value, bytes) = (value.to_string(), bytes.to_string());
        (Some(n), path, table, value, bytes, Made::Merged)
    }

    /// A finding of [`found`]'s in the tree of destinations.
    fn dest(n: u32, value: &str, bytes: &str) -> Found {
        tree(n, "Root/Names/Dests", "Name/Dests", value, bytes)
    }

    /// Every object held, as merged.
    const ALL: Hold = Hold::All(Made::Merged);

    const ORDER: &str = "a key isn't past the one before it";
    const OUTSIDE: &str = "a key is outside its node's /Limits";
    const LIMITS_BAD: &str = "a node's /Limits isn't its least and greatest keys";
    const AGAIN: &str = "the tree reaches 11 0 R again";
    const IN_PLACE: &str = "a kid is written in place, not as a reference";
    const EMPTY: &str = "a node has neither /Kids nor entries of its own";

    /// Each kind of fault pdf-semantics' walk tells that the walk doesn't
    /// say is found once, in the object it's in, at its bytes there, with
    /// the path, table and key the walk's own tree findings have: keys out
    /// of order in the root's own `/Names`, in its object, not the
    /// catalog's, and across its kids, in the kid; a key outside its
    /// node's `/Limits`; `/Limits` out of order, or one key alone; a node
    /// listed twice, in the node that lists it, or by a loop, back to the
    /// root too, which the walk is given by its number; a kid written
    /// in place, in the object it's in; a node under the root with neither
    /// `/Kids` nor `/Names`; and a number tree's keys out of order.
    #[test]
    fn what_a_trees_walk_tells_that_the_walk_doesnt_is_found_once_as_the_walks() {
        let leaf = |limits: &str, names: &str| format!("<< /Limits [{limits}] /Names [{names}] >>");
        let (a_b, c_d) = (format!("(a) {D} (b) {D}"), format!("(c) {D} (d) {D}"));
        let shapes = [
            (
                dests(&format!("<< /Names [(b) {D} (a) {D}] >>"), &[]),
                dest(10, ORDER, "(a)"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R 12 0 R] >>",
                    &[(11, &leaf("(c) (d)", &c_d)), (12, &leaf("(a) (b)", &a_b))],
                ),
                dest(12, ORDER, "(a)"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R] >>",
                    &[(11, &leaf("(a) (b)", &format!("(a) {D} (c) {D}")))],
                ),
                dest(11, OUTSIDE, "(c)"),
            ),
            (
                dests("<< /Kids [11 0 R] >>", &[(11, &leaf("(b) (a)", &a_b))]),
                dest(11, LIMITS_BAD, "[(b) (a)]"),
            ),
            (
                dests("<< /Kids [11 0 R] >>", &[(11, &leaf("(a)", &a_b))]),
                dest(11, LIMITS_BAD, "[(a)]"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R 11 0 R] >>",
                    &[(11, &leaf("(a) (a)", &format!("(a) {D}")))],
                ),
                dest(10, AGAIN, "11 0 R"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R] >>",
                    &[(11, "<< /Limits [(a) (a)] /Kids [11 0 R] >>")],
                ),
                dest(11, AGAIN, "11 0 R"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R] >>",
                    &[(11, "<< /Limits [(a) (a)] /Kids [10 0 R] >>")],
                ),
                dest(11, "the tree reaches 10 0 R again", "10 0 R"),
            ),
            (
                dests(
                    &format!("<< /Kids [{}] >>", leaf("(a) (a)", &format!("(a) {D}"))),
                    &[],
                ),
                dest(10, IN_PLACE, &leaf("(a) (a)", &format!("(a) {D}"))),
            ),
            (
                dests("<< /Kids [11 0 R] >>", &[(11, "<< /Limits [(a) (a)] >>")]),
                dest(11, EMPTY, "<< /Limits [(a) (a)] >>"),
            ),
        ];
        for (file, expected) in shapes {
            assert_eq!(found(&file, &ALL), [expected]);
        }
        let labels = "<< /Nums [1 << /S /D >> 0 << /S /r >>] >>".to_string();
        let file = file("/PageLabels 10 0 R", &[(10, labels)]);
        let order = tree(10, "Root/PageLabels", "Catalog/PageLabels", ORDER, "0");
        assert_eq!(found(&file, &ALL), [order]);
    }

    /// A tree of destinations' root, its nodes besides, and the node a
    /// finding is in and what it says.
    type Shape<'s> = (&'s str, &'s [(u32, &'s str)], u32, &'s str);

    /// What the walk says of a tree is found by it alone, once, though
    /// pdf-semantics' walk tells it too: a root with neither `/Kids` nor
    /// `/Names`, or with both, or with `/Limits`; a node under the root
    /// without `/Limits`; an odd last key, and a key of another type; a kid
    /// that's null; `/Kids` or `/Names` not an array; and a kid that doesn't
    /// read, which the walk finds unresolved.
    #[test]
    fn what_the_walk_says_of_a_tree_is_found_by_it_alone() {
        let leaf = format!("<< /Limits [(k) (k)] /Names [(k) {D}] >>");
        let shapes: [Shape; 9] = [
            ("<< >>", &[], 10, "a root with neither /Kids nor /Names"),
            (
                &format!("<< /Names [(a) {D}] /Kids [11 0 R] >>"),
                &[(11, &leaf)],
                10,
                "a node with both /Kids and /Names",
            ),
            (
                &format!("<< /Limits [(a) (a)] /Names [(a) {D}] >>"),
                &[],
                10,
                "a root with /Limits",
            ),
            (
                "<< /Kids [11 0 R] >>",
                &[(11, &format!("<< /Names [(a) {D}] >>"))],
                11,
                "a node under the root without /Limits",
            ),
            (
                &format!("<< /Names [(a) {D} (b)] >>"),
                &[],
                10,
                "its last key, [2], has no value",
            ),
            (
                &format!("<< /Names [1 {D} (a) {D}] >>"),
                &[],
                10,
                "its key [0] isn't a string",
            ),
            (
                "<< /Kids [null] >>",
                &[],
                10,
                "its /Kids[0] is null, not a dictionary",
            ),
            ("<< /Kids 5 >>", &[], 10, "its /Kids isn't an array"),
            ("<< /Names (a) >>", &[], 10, "its /Names isn't an array"),
        ];
        for (root, nodes, n, said) in shapes {
            let file = dests(root, nodes);
            let node = nodes
                .iter()
                .find(|&&(m, _)| m == n)
                .map_or(root, |&(_, body)| body);
            assert_eq!(found(&file, &ALL), [dest(n, said, node)], "{root}");
        }
        let damaged = dests("<< /Kids [11 0 R] >>", &[(11, "<< /Limits [")]);
        assert_eq!(found(&damaged, &ALL), []);
        let report = checked(&damaged, &ALL, &Options::default());
        let unresolved = (report.findings.iter()).filter(|f| {
            (f.rule, f.object, f.table.as_str()) == (Rule::Unresolved, Some((10, 0)), "Name")
        });
        assert_eq!(unresolved.count(), 1, "{report}");
    }

    /// A root written in place is in the object it's written in, which
    /// holds what's found in it; what's in a node by number, that node.
    #[test]
    fn a_root_written_in_place_is_held_by_the_object_it_s_in() {
        let in_place = file(
            &format!("/Names << /Dests << /Names [(b) {D} (a) {D}] >> >>"),
            &[],
        );
        assert_eq!(found(&in_place, &ALL), [dest(1, ORDER, "(a)")]);
        let only = |n| Hold::Only([((n, 0), Made::Built)].into());
        let mut built = dest(1, ORDER, "(a)");
        built.5 = Made::Built;
        assert_eq!(found(&in_place, &only(1)), [built]);
        assert_eq!(found(&in_place, &only(2)), []);

        let leaf = format!("<< /Limits [(a) (b)] /Names [(b) {D} (a) {D}] >>");
        let kids = file("/Names << /Dests << /Kids [11 0 R] >> >>", &[(11, leaf)]);
        assert_eq!(found(&kids, &ALL), [dest(11, ORDER, "(a)")]);
        assert_eq!(found(&kids, &only(1)), []);
    }

    /// Each kind is found once in each object it's in, at its first place
    /// there, with how many times it's there: 999 keys out of order in one
    /// node are one finding; two kinds in a node two, and a kind in two
    /// nodes two, in the order the walk first told each.
    #[test]
    fn a_kind_in_a_node_is_one_finding_saying_how_many_times() {
        let names: Vec<_> = (0..1000).rev().map(|i| format!("({i:03}) {D}")).collect();
        let file = dests(&format!("<< /Names [{}] >>", names.join(" ")), &[]);
        let many = format!("{ORDER}, 999 times");
        assert_eq!(found(&file, &ALL), [dest(10, &many, "(998)")]);

        let file = dests(
            "<< /Kids [11 0 R 12 0 R] >>",
            &[
                (
                    11,
                    &format!("<< /Limits [(b) (a)] /Names [(b) {D} (a) {D}] >>"),
                ),
                (
                    12,
                    &format!("<< /Limits [(c) (d)] /Names [(d) {D} (c) {D}] >>"),
                ),
            ],
        );
        let found = found(&file, &ALL);
        let expected = [
            dest(11, LIMITS_BAD, "[(b) (a)]"),
            dest(11, ORDER, "(a)"),
            dest(12, ORDER, "(c)"),
        ];
        assert_eq!(found, expected);
    }

    /// A tree is found in once, however many times the walk reaches it: from
    /// two rows, its root reached again; and a node two trees share, as the
    /// first's, the tree the walk checks it as.
    #[test]
    fn a_tree_reached_twice_is_found_in_once() {
        let root = format!("<< /Names [(b) {D} (a) {D}] >>");
        let twice = file("/Names << /Dests 10 0 R /AP 10 0 R >>", &[(10, root)]);
        assert_eq!(found(&twice, &ALL), [dest(10, ORDER, "(a)")]);

        let leaf = format!("<< /Limits [(a) (b)] /Names [(b) {D} (a) {D}] >>");
        let kid = "<< /Kids [11 0 R] >>".to_string();
        let shared = file(
            "/Names << /Dests 10 0 R /AP 20 0 R >>",
            &[(10, kid.clone()), (11, leaf), (20, kid)],
        );
        assert_eq!(found(&shared, &ALL), [dest(11, ORDER, "(a)")]);
    }

    /// Only what's in an object held is found, though every tree is read.
    #[test]
    fn what_s_in_a_node_not_held_isnt_found() {
        let leaf = |limits: &str, keys: [&str; 2]| {
            let [a, b] = keys;
            format!("<< /Limits [{limits}] /Names [{a} {D} {b} {D}] >>")
        };
        let file = dests(
            "<< /Kids [11 0 R 12 0 R] >>",
            &[
                (11, &leaf("(c) (d)", ["(c)", "(d)"])),
                (12, &leaf("(a) (b)", ["(a)", "(b)"])),
            ],
        );
        let held = |ns: &[u32]| Hold::Only(ns.iter().map(|&n| ((n, 0), Made::Merged)).collect());
        assert_eq!(found(&file, &held(&[12])), [dest(12, ORDER, "(a)")]);
        assert_eq!(found(&file, &held(&[1, 10, 11])), []);
    }

    /// A tree finding is carried by a like one, whose count differs: by
    /// number, by path and from a source; one of a kind the earlier report
    /// hasn't is the file's own.
    #[test]
    fn a_tree_finding_is_carried_whatever_its_count() {
        let report = |root: &str| {
            let options = Options {
                paths: true,
                ..Options::default()
            };
            checked(&dests(root, &[]), &ALL, &options)
        };
        let values = |found: Vec<&Finding>| -> Vec<String> {
            found.into_iter().map(|f| f.value.clone()).collect()
        };
        let before = report(&format!("<< /Names [(c) {D} (b) {D} (a) {D}] >>"));
        let after = report(&format!("<< /Names [(d) {D} (c) {D} (b) {D} (a) {D}] >>"));
        assert_eq!(
            values(before.findings.iter().collect()),
            [format!("{ORDER}, 2 times")]
        );
        assert_eq!(
            values(after.findings.iter().collect()),
            [format!("{ORDER}, 3 times")]
        );
        assert_eq!(values(after.since(&before)), Vec::<String>::new());
        assert_eq!(values(after.by_path(&before)), Vec::<String>::new());
        assert_eq!(values(after.beyond(&[&before])), Vec::<String>::new());
        let names = format!("(d) {D} (c) {D} (b) {D} (a) {D}");
        let kid = report(&format!(
            "<< /Kids [<< /Limits [(a) (d)] /Names [{names}] >>] >>"
        ));
        assert_eq!(kid.findings.len(), 2, "{kid}");
        assert_eq!(values(kid.since(&before)), [IN_PLACE]);
        assert_eq!(values(kid.by_path(&before)), [IN_PLACE]);
        assert_eq!(values(kid.beyond(&[&before])), [IN_PLACE]);
    }

    /// The rows a check counts as not checked for a tree of destinations:
    /// why, and how many.
    fn unread_trees(report: &Report) -> Vec<(&str, usize)> {
        (report.unchecked.iter())
            .filter(|(u, _)| (u.column, u.table.as_str(), u.key.as_str()) == ("-", "Name", "Dests"))
            .map(|(u, &n)| (u.why.as_str(), n))
            .collect()
    }

    /// A tree not read whole for what isn't the file's fault is counted as
    /// not checked, once, its table and key the root's row's, where its root
    /// is held, and what was read of it is found all the same: one deeper
    /// than pdf-semantics reads, 34 levels, and one whose keys' bytes the
    /// reader didn't give. One not read whole for its own faults isn't.
    #[test]
    fn a_tree_not_read_whole_for_what_isnt_the_file_s_fault_is_counted() {
        let mut nodes: Vec<(u32, String)> = (11..43)
            .map(|n| (n, format!("<< /Limits [(a) (a)] /Kids [{} 0 R] >>", n + 1)))
            .collect();
        nodes[0].1 = "<< /Limits [(b) (a)] /Kids [12 0 R] >>".to_string();
        nodes.push((43, format!("<< /Limits [(a) (a)] /Names [(a) {D}] >>")));
        let nodes: Vec<_> = nodes.iter().map(|(n, body)| (*n, body.as_str())).collect();
        let deep = dests("<< /Kids [11 0 R] >>", &nodes);
        let bad = || dest(11, LIMITS_BAD, "[(b) (a)]");
        assert_eq!(found(&deep, &ALL), [bad()]);
        let report = checked(&deep, &ALL, &Options::default());
        assert_eq!(unread_trees(&report), [(DEEP, 1)], "{report}");
        let only = |n| Hold::Only([((n, 0), Made::Merged)].into());
        assert_eq!(found(&deep, &only(11)), [bad()]);
        let report = checked(&deep, &only(11), &Options::default());
        assert_eq!(unread_trees(&report), [], "{report}");
        let report = checked(&deep, &only(10), &Options::default());
        assert_eq!(unread_trees(&report), [(DEEP, 1)], "{report}");
        assert_eq!(found(&deep, &only(10)), []);

        // Strings left ciphertext: an encryption dictionary the reader
        // can't open.
        let catalog = "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 10 0 R >> >>";
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>";
        let root = format!("<< /Names [(b) {D} (a) {D}] >>");
        let objects: [(u32, &[u8]); 4] = [
            (1, catalog.as_bytes()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page.as_bytes()),
            (10, root.as_bytes()),
        ];
        let trailer = "<< /Size 11 /Root 1 0 R /Encrypt << /Filter /Standard /V 2 >> >>";
        let locked = fixture::assembled("1.7", &objects, trailer);
        assert_eq!(found(&locked, &ALL), []);
        let report = checked(&locked, &ALL, &Options::default());
        assert_eq!(unread_trees(&report), [(KEY, 1)], "{report}");

        let null = dests(
            "<< /Kids [null 11 0 R] >>",
            &[(11, &format!("<< /Limits [(a) (a)] /Names [(a) {D}] >>"))],
        );
        let report = checked(&null, &ALL, &Options::default());
        assert_eq!(unread_trees(&report), [], "{report}");
    }

    /// A file of a catalog, object 1, of the page tree at 2, unless
    /// `objects` writes 1 too, and `objects`, each its number and body.
    fn paged(objects: &[(u32, &str)]) -> Vec<u8> {
        let mut all: Vec<(u32, &[u8])> = Vec::new();
        if !objects.iter().any(|&(n, _)| n == 1) {
            all.push((1, b"<< /Type /Catalog /Pages 2 0 R >>"));
        }
        all.extend(objects.iter().map(|&(n, body)| (n, body.as_bytes())));
        all.sort_by_key(|&(n, _)| n);
        let size = all.iter().map(|&(n, _)| n).max().unwrap_or(0) + 1;
        fixture::assembled("1.7", &all, &format!("<< /Size {size} /Root 1 0 R >>"))
    }

    /// A page whose `/Parent` is `parent`, with `boxes` for its
    /// `/MediaBox` and `/Resources`.
    fn page_with(parent: &str, boxes: &str) -> String {
        format!("<< /Type /Page /Parent {parent} {boxes} >>")
    }

    /// A page whose `/Parent` is `parent`, with a box and resources.
    fn page(parent: &str) -> String {
        page_with(parent, "/MediaBox [0 0 612 792] /Resources << >>")
    }

    /// A node whose `/Parent` is `parent`, listing `kids`, its `/Count`
    /// `count`.
    fn node(parent: &str, kids: &str, count: &str) -> String {
        format!("<< /Type /Pages /Parent {parent} /Kids [{kids}] /Count {count} >>")
    }

    /// Each page tree finding of `file`'s check, every object held.
    fn page_tree_found(file: &[u8]) -> Vec<Found> {
        found_by(Rule::PageTree, file, &ALL)
    }

    const ROOT: &str = "Root/Pages";
    const KID: &str = "Root/Pages/Kids[0]";
    const CHAIN: &str = "a reference in the page tree names another reference";
    const TYPE: &str = "a /Type in the page tree doesn't say what's there";

    /// Each kind of fault pdf-semantics' walk of the page tree by ISO's
    /// rules tells that the walk can't is found once, in the object it's
    /// in, at its bytes there, its path and table the object's as the walk
    /// first visited it in the tree: a `/Count` more and less than the
    /// pages under the root, and a node's; a node listed inside itself,
    /// its `/Count` then other than what's under it; a page and a node
    /// listed again, the node's page again in it; a `/Parent` that isn't
    /// the node listing the page, and a `/Parent` chain that loops; a
    /// root's `/Parent`; an empty tree, once, its `/Count` of 0 no finding;
    /// a node whose `/Type` is `/Page`, which the walk takes for a page; a
    /// reference to a reference, a kid, a `/Parent` and a box, at the key
    /// that holds it, told once though the walk reads a `/Parent` twice;
    /// and a root written in the catalog, its faults in the catalog.
    #[test]
    fn what_the_page_trees_walk_tells_that_the_walk_cant_is_found_once() {
        let two = node("null", "3 0 R", "1");
        let shapes: Vec<(Vec<u8>, Vec<Found>)> = vec![
            (
                paged(&[(2, &node("null", "3 0 R", "3")), (3, &page("2 0 R"))]),
                vec![tree(
                    2,
                    ROOT,
                    "PageTreeNodeRoot/Count",
                    "a node's /Count is 3, with 1 page under it",
                    &node("null", "3 0 R", "3"),
                )],
            ),
            (
                paged(&[
                    (2, &node("null", "3 0 R 4 0 R", "1")),
                    (3, &page("2 0 R")),
                    (4, &page("2 0 R")),
                ]),
                vec![tree(
                    2,
                    ROOT,
                    "PageTreeNodeRoot/Count",
                    "a node's /Count is 1, with 2 pages under it",
                    &node("null", "3 0 R 4 0 R", "1"),
                )],
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R", "1")),
                    (4, &node("2 0 R", "3 0 R", "2")),
                    (3, &page("4 0 R")),
                ]),
                vec![tree(
                    4,
                    KID,
                    "PageTreeNode/Count",
                    "a node's /Count is 2, with 1 page under it",
                    &node("2 0 R", "3 0 R", "2"),
                )],
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R 3 0 R", "1")),
                    (4, &node("2 0 R", "2 0 R", "1")),
                    (3, &page("2 0 R")),
                ]),
                vec![
                    tree(
                        4,
                        KID,
                        "PageTreeNode/Kids",
                        "the page tree lists 2 0 R inside itself",
                        "2 0 R",
                    ),
                    tree(
                        4,
                        KID,
                        "PageTreeNode/Count",
                        "a node's /Count is 1, with 0 pages under it",
                        &node("2 0 R", "2 0 R", "1"),
                    ),
                ],
            ),
            (
                paged(&[(2, &node("null", "3 0 R 3 0 R", "2")), (3, &page("2 0 R"))]),
                vec![tree(
                    2,
                    ROOT,
                    "PageTreeNodeRoot/Kids",
                    "the page tree lists 3 0 R again",
                    "3 0 R",
                )],
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R 4 0 R", "2")),
                    (4, &node("2 0 R", "3 0 R", "1")),
                    (3, &page("4 0 R")),
                ]),
                vec![
                    tree(
                        2,
                        ROOT,
                        "PageTreeNodeRoot/Kids",
                        "the page tree lists 4 0 R again",
                        "4 0 R",
                    ),
                    tree(
                        4,
                        KID,
                        "PageTreeNode/Kids",
                        "the page tree lists 3 0 R again",
                        "3 0 R",
                    ),
                ],
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R", "1")),
                    (4, &node("2 0 R", "3 0 R", "1")),
                    (3, &page("2 0 R")),
                ]),
                vec![tree(
                    3,
                    "Root/Pages/Kids[0]/Kids[0]",
                    "PageObject/Parent",
                    "a /Parent isn't 4 0 R, the node that lists it",
                    &page("2 0 R"),
                )],
            ),
            (
                paged(&[
                    (2, &two),
                    (3, &page("4 0 R")),
                    (4, &node("5 0 R", "3 0 R", "1")),
                    (5, &node("4 0 R", "4 0 R", "1")),
                ]),
                vec![
                    tree(
                        3,
                        KID,
                        "PageObject/Parent",
                        "a /Parent isn't 2 0 R, the node that lists it",
                        &page("4 0 R"),
                    ),
                    tree(
                        5,
                        "Root/Pages/Kids[0]/Parent/Parent",
                        "PageTreeNode/Parent",
                        "a /Parent chain loops",
                        "4 0 R",
                    ),
                ],
            ),
            (
                paged(&[
                    (2, &node("4 0 R", "3 0 R", "1")),
                    (3, &page("2 0 R")),
                    (4, &node("null", "2 0 R", "1")),
                ]),
                vec![tree(
                    2,
                    ROOT,
                    "PageTreeNodeRoot/Parent",
                    "the root of the page tree has a /Parent",
                    &node("4 0 R", "3 0 R", "1"),
                )],
            ),
            (
                paged(&[(2, "<< /Type /Pages /Kids [] /Count 0 >>")]),
                vec![tree(
                    2,
                    ROOT,
                    "PageTreeNodeRoot/Kids",
                    "the page tree has no pages",
                    "<< /Type /Pages /Kids [] /Count 0 >>",
                )],
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R", "1")),
                    (4, "<< /Type /Page /Parent 2 0 R /Kids [3 0 R] /Count 1 >>"),
                    (3, &page("4 0 R")),
                ]),
                vec![tree(
                    4,
                    KID,
                    "PageObject/Type",
                    TYPE,
                    "<< /Type /Page /Parent 2 0 R /Kids [3 0 R] /Count 1 >>",
                )],
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R", "1")),
                    (3, &page("2 0 R")),
                    (4, "3 0 R"),
                ]),
                vec![tree(2, ROOT, "PageTreeNodeRoot/Kids", CHAIN, "4 0 R")],
            ),
            (
                paged(&[(2, &two), (3, &page("4 0 R")), (4, "2 0 R")]),
                vec![tree(3, KID, "PageObject/Parent", CHAIN, "4 0 R")],
            ),
            (
                paged(&[
                    (2, &two),
                    (3, &page_with("2 0 R", "/MediaBox 4 0 R /Resources << >>")),
                    (4, "5 0 R"),
                    (5, "[0 0 595 842]"),
                ]),
                vec![tree(3, KID, "PageObject/MediaBox", CHAIN, "4 0 R")],
            ),
            (
                paged(&[
                    (
                        1,
                        "<< /Type /Catalog /Pages << /Type /Pages /Kids [3 0 R] /Count 2 >> >>",
                    ),
                    (3, &page("1 0 R")),
                ]),
                vec![tree(
                    1,
                    ROOT,
                    "PageTreeNodeRoot/Count",
                    "a node's /Count is 2, with 1 page under it",
                    "<< /Type /Pages /Kids [3 0 R] /Count 2 >>",
                )],
            ),
        ];
        for (file, expected) in shapes {
            assert_eq!(page_tree_found(&file), expected);
        }
    }

    /// What the walk says of a page tree, or of the object a fault is in,
    /// is found by it alone, though pdf-semantics' walk tells a fault too:
    /// each shape's own finding, and no page tree finding. A `/Count`
    /// absent or a real, a node's `/Kids` empty or absent, the root's
    /// absent; a `/Type` absent, of another name, or `/Page` at the root; a
    /// kid written in place, null, an integer, a reference to nothing or a
    /// stream; a `/Parent` absent or written in place, and a root written
    /// in place, whose pages' `/Parent`s can't name it; a page with no
    /// `/MediaBox`, or one of three numbers.
    #[test]
    fn what_the_walk_says_of_a_page_tree_is_found_by_it_alone() {
        let one = node("null", "3 0 R", "1");
        let pages = |root: &str, three: &str| paged(&[(2, root), (3, three)]);
        let kids = |kids: &str| paged(&[(2, &node("null", kids, "1")), (3, &page("2 0 R"))]);
        let empty = |kid: &str| {
            paged(&[
                (2, &node("null", "4 0 R 3 0 R", "1")),
                (3, &page("2 0 R")),
                (4, kid),
            ])
        };
        let shapes = [
            (
                pages("<< /Type /Pages /Kids [3 0 R] >>", &page("2 0 R")),
                (2, "PageTreeNodeRoot", "Count", Rule::Missing),
            ),
            (
                pages(&node("null", "3 0 R", "1.0"), &page("2 0 R")),
                (2, "PageTreeNodeRoot", "Count", Rule::Type),
            ),
            (
                empty("<< /Type /Pages /Parent 2 0 R /Kids [] /Count 0 >>"),
                (4, "PageTreeNode", "Kids", Rule::Special),
            ),
            (
                empty("<< /Type /Pages /Parent 2 0 R /Count 0 >>"),
                (4, "PageTreeNode", "Kids", Rule::Missing),
            ),
            (
                paged(&[(2, "<< /Type /Pages /Count 0 >>")]),
                (2, "PageTreeNodeRoot", "Kids", Rule::Missing),
            ),
            (
                pages(
                    &one,
                    "<< /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>",
                ),
                (3, "PageObject", "Type", Rule::Missing),
            ),
            (
                pages(
                    &one,
                    "<< /Type /Foo /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>",
                ),
                (3, "PageObject", "Type", Rule::Value),
            ),
            (
                paged(&[(
                    2,
                    "<< /Type /Page /MediaBox [0 0 612 792] /Resources << >> >>",
                )]),
                (2, "PageTreeNodeRoot", "Type", Rule::Value),
            ),
            (
                paged(&[(2, &node("null", &page("2 0 R"), "1"))]),
                (2, "ArrayOfPageTreeNodeKids", "0", Rule::Indirect),
            ),
            (
                kids("null 3 0 R"),
                (2, "ArrayOfPageTreeNodeKids", "0", Rule::Type),
            ),
            (
                kids("5 3 0 R"),
                (2, "ArrayOfPageTreeNodeKids", "0", Rule::Type),
            ),
            (
                kids("9 0 R 3 0 R"),
                (2, "ArrayOfPageTreeNodeKids", "0", Rule::Unresolved),
            ),
            (
                pages(
                    &one,
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Length 0 >>\nstream\n\nendstream",
                ),
                (2, "ArrayOfPageTreeNodeKids", "0", Rule::Type),
            ),
            (
                pages(
                    &one,
                    "<< /Type /Page /MediaBox [0 0 612 792] /Resources << >> >>",
                ),
                (3, "PageObject", "Parent", Rule::Missing),
            ),
            (
                pages(&one, &page(&one)),
                (3, "PageObject", "Parent", Rule::Indirect),
            ),
            (
                paged(&[
                    (1, &format!("<< /Type /Catalog /Pages {one} >>")),
                    (3, &page("1 0 R")),
                ]),
                (1, "Catalog", "Pages", Rule::Indirect),
            ),
            (
                pages(&one, &page_with("2 0 R", "/Resources << >>")),
                (3, "PageObject", "MediaBox", Rule::Missing),
            ),
            (
                pages(
                    &one,
                    &page_with("2 0 R", "/MediaBox [0 0 612] /Resources << >>"),
                ),
                (3, "PageObject", "MediaBox", Rule::Type),
            ),
        ];
        for (file, (n, table, key, rule)) in shapes {
            let report = checked(&file, &ALL, &Options::default());
            let own = (report.findings.iter()).any(|f| {
                (f.object, f.table.as_str(), f.key.as_str(), f.rule)
                    == (Some((n, 0)), table, key, rule)
            });
            assert!(own, "{table}/{key} {rule}: {report}");
            let told: Vec<_> = (report.findings.iter())
                .filter(|f| f.rule == Rule::PageTree)
                .collect();
            assert!(told.is_empty(), "{report}");
        }
    }

    /// A root's `/Parent`, a key the model doesn't list there, is a page
    /// tree finding in an object merged, where the walk counts it alone;
    /// and in one built, where the walk lets it be as the root is reached
    /// again, by its page's `/Parent`, as a node, whose table lists it. In a
    /// root built that nothing reaches again, the walk finds it unlisted,
    /// and it isn't told again; the tree's emptiness is, at its own key.
    #[test]
    fn a_roots_parent_is_found_once_unlisted_where_the_walk_finds_it() {
        let told = |file: &[u8], hold: &Hold| -> Vec<(String, Rule, String)> {
            let report = checked(file, hold, &Options::default());
            (report.findings.iter())
                .filter(|f| f.object == Some((2, 0)))
                .map(|f| (f.key.clone(), f.rule, f.value.clone()))
                .collect()
        };
        let parent = || "Parent".to_string();
        let said = "the root of the page tree has a /Parent".to_string();
        let file = paged(&[
            (2, &node("4 0 R", "3 0 R", "1")),
            (3, &page("2 0 R")),
            (4, &node("null", "2 0 R", "1")),
        ]);
        let found = [(parent(), Rule::PageTree, said.clone())];
        assert_eq!(told(&file, &ALL), found);
        assert_eq!(told(&file, &Hold::All(Made::Built)), found);

        let empty = node("4 0 R", "", "0");
        let file = paged(&[(2, &empty), (4, &node("null", "2 0 R", "1"))]);
        let none = (
            "Kids".to_string(),
            Rule::PageTree,
            "the page tree has no pages".to_string(),
        );
        let merged = told(&file, &ALL);
        assert_eq!(merged, [(parent(), Rule::PageTree, said), none.clone()]);
        let built = told(&file, &Hold::All(Made::Built));
        let four = "<< /Count 1 /Kids [2 0 R] /Parent null /Type /Pages >>";
        assert_eq!(built, [(parent(), Rule::Unlisted, four.to_string()), none]);
    }

    /// What's found in an object of the page tree the walk first visited
    /// as another table, here the catalog, which its `/Pages` names, has
    /// `-` for its path and table, as the walk reached it as none of the
    /// tree's.
    #[test]
    fn what_the_walk_reached_as_another_table_has_no_path() {
        let catalog = "<< /Type /Catalog /Pages 1 0 R >>";
        let file = paged(&[(1, catalog)]);
        assert_eq!(
            page_tree_found(&file),
            [tree(1, "-", "-/Type", TYPE, catalog)]
        );
    }

    /// What pdf-semantics tells in no object is the catalog's, its path
    /// and table the root's where that's written in the catalog, and none
    /// else's: a chain at the catalog's `/Pages`, at its key there, `-` and
    /// `-` though a page's `/Parent` is a node written in place, which the
    /// walk visits as a root too; and a chain in the root's own `/Kids`,
    /// written in the catalog, at the root's key, `Kids`.
    #[test]
    fn what_s_in_no_object_is_the_root_written_in_the_catalog_s_alone() {
        let chained = "<< /Type /Catalog /Pages 7 0 R >>";
        let two = node("null", "3 0 R", "1");
        let in_place = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
        let shapes = [
            (
                paged(&[(1, chained), (7, "2 0 R"), (2, &two), (3, &page("2 0 R"))]),
                tree(1, "-", "-/Pages", CHAIN, "7 0 R"),
            ),
            (
                paged(&[(1, chained), (7, "2 0 R"), (2, &two), (3, &page(in_place))]),
                tree(1, "-", "-/Pages", CHAIN, "7 0 R"),
            ),
            (
                paged(&[
                    (
                        1,
                        "<< /Type /Catalog /Pages << /Type /Pages /Kids [4 0 R] /Count 1 >> >>",
                    ),
                    (4, "3 0 R"),
                    (3, &page("1 0 R")),
                ]),
                tree(1, ROOT, "PageTreeNodeRoot/Kids", CHAIN, "4 0 R"),
            ),
        ];
        for (file, expected) in shapes {
            assert_eq!(page_tree_found(&file), [expected]);
        }
    }

    /// A kind is found by the first of its faults that's found, whatever
    /// came before it that isn't: a node listed in place, where no
    /// `/Parent` can name what lists it, and then by a numbered node its
    /// `/Parent` doesn't name, is found as the second, as it is the other
    /// way round.
    #[test]
    fn a_fault_not_found_doesnt_stand_for_its_kind() {
        let five = node("2 0 R", "3 0 R", "1");
        let in_place = node("2 0 R", "5 0 R", "1");
        let four = node("2 0 R", "5 0 R", "1");
        let parent = "a /Parent isn't 4 0 R, the node that lists it";
        for kids in [format!("{in_place} 4 0 R"), format!("4 0 R {in_place}")] {
            let file = paged(&[
                (2, &node("null", &kids, "2")),
                (4, &four),
                (5, &five),
                (3, &page("5 0 R")),
            ]);
            let found: Vec<_> = (page_tree_found(&file).into_iter())
                .filter(|f| f.2 == "PageTreeNode/Parent")
                .collect();
            // The first visit of 5 is under the first kid either way.
            let path = "Root/Pages/Kids[0]/Kids[0]";
            assert_eq!(found, [tree(5, path, "PageTreeNode/Parent", parent, &five)]);
        }
    }

    /// Only what's in an object held is found: a page's `/Parent`, in the
    /// page held, not in the node that lists it; and of a node listed
    /// twice, only what's in the node held.
    #[test]
    fn what_s_in_a_page_tree_object_not_held_isnt_found() {
        let held = |ns: &[u32]| Hold::Only(ns.iter().map(|&n| ((n, 0), Made::Merged)).collect());
        let wrong = paged(&[
            (2, &node("null", "4 0 R", "1")),
            (4, &node("2 0 R", "3 0 R", "1")),
            (3, &page("2 0 R")),
        ]);
        let parent = tree(
            3,
            "Root/Pages/Kids[0]/Kids[0]",
            "PageObject/Parent",
            "a /Parent isn't 4 0 R, the node that lists it",
            &page("2 0 R"),
        );
        assert_eq!(found_by(Rule::PageTree, &wrong, &held(&[3])), [parent]);
        assert_eq!(found_by(Rule::PageTree, &wrong, &held(&[1, 2, 4])), []);
        let twice = paged(&[
            (2, &node("null", "4 0 R 4 0 R", "2")),
            (4, &node("2 0 R", "3 0 R", "1")),
            (3, &page("4 0 R")),
        ]);
        let again = tree(
            4,
            KID,
            "PageTreeNode/Kids",
            "the page tree lists 3 0 R again",
            "3 0 R",
        );
        assert_eq!(found_by(Rule::PageTree, &twice, &held(&[4])), [again]);
    }

    /// A page tree finding is carried by a like one whatever its numbers
    /// and count: a `/Count` of 3 by one of 5, by number, by path and from
    /// a source, as is the viewer's count beside it; a page listed again
    /// twice by one listed again once. One of another kind is the file's
    /// own.
    #[test]
    fn a_page_tree_finding_is_carried_whatever_its_numbers() {
        let report = |two: &str| {
            let options = Options {
                paths: true,
                ..Options::default()
            };
            checked(&paged(&[(2, two), (3, &page("2 0 R"))]), &ALL, &options)
        };
        let values = |found: Vec<&Finding>| -> Vec<String> {
            found.into_iter().map(|f| f.value.clone()).collect()
        };
        let carried = |after: &Report, before: &Report| {
            assert_eq!(values(after.since(before)), Vec::<String>::new(), "{after}");
            assert_eq!(
                values(after.by_path(before)),
                Vec::<String>::new(),
                "{after}"
            );
            assert_eq!(
                values(after.beyond(&[before])),
                Vec::<String>::new(),
                "{after}"
            );
        };
        let three = report(&node("null", "3 0 R", "3"));
        let five = report(&node("null", "3 0 R", "5"));
        assert_eq!(
            values(five.findings.iter().collect()),
            [
                "a node's /Count is 5, with 1 page under it",
                "a viewer counts 5 pages, where the tree lists 1"
            ]
        );
        carried(&five, &three);
        let again = report(&node("null", "3 0 R 3 0 R", "2"));
        let twice = report(&node("null", "3 0 R 3 0 R 3 0 R", "3"));
        assert_eq!(
            values(twice.findings.iter().collect()),
            ["the page tree lists 3 0 R again, 2 times"]
        );
        carried(&twice, &again);
        let own = ["the page tree lists 3 0 R again"];
        assert_eq!(values(again.since(&three)), own);
        assert_eq!(values(again.by_path(&three)), own);
        assert_eq!(values(again.beyond(&[&three])), own);
    }

    /// The rows a check counts as not checked for the page tree: why, and
    /// how many.
    fn unread_pages(report: &Report) -> Vec<(&str, usize)> {
        (report.unchecked.iter())
            .filter(|(u, _)| {
                (u.column, u.table.as_str(), u.key.as_str()) == ("-", "PageTreeNodeRoot", "-")
            })
            .map(|(u, &n)| (u.why.as_str(), n))
            .collect()
    }

    /// A page tree deeper than pdf-semantics reads, 1,025 nodes from the
    /// root down, is counted as not checked, once, where the catalog is
    /// held, and what was told before is found: a page listed again at the
    /// root, before its node goes down. A tree read whole isn't counted.
    #[test]
    fn a_page_tree_deeper_than_its_read_is_counted_what_was_read_found() {
        let (first, last) = (10, 10 + 1023);
        let mut nodes: Vec<(u32, String)> = (first..last)
            .map(|n| {
                (
                    n,
                    node(&format!("{} 0 R", n - 1), &format!("{} 0 R", n + 1), "1"),
                )
            })
            .collect();
        nodes[0].1 = node("2 0 R", "11 0 R", "1");
        nodes.push((last, node(&format!("{} 0 R", last - 1), "", "0")));
        let mut objects: Vec<(u32, &str)> =
            nodes.iter().map(|(n, body)| (*n, body.as_str())).collect();
        let root = node("null", "3 0 R 3 0 R 10 0 R", "3");
        let leaf = page("2 0 R");
        objects.extend([(2, root.as_str()), (3, leaf.as_str())]);
        let deep = paged(&objects);
        let again = tree(
            2,
            ROOT,
            "PageTreeNodeRoot/Kids",
            "the page tree lists 3 0 R again",
            "3 0 R",
        );
        assert_eq!(page_tree_found(&deep), [again]);
        let report = checked(&deep, &ALL, &Options::default());
        assert_eq!(unread_pages(&report), [(PAGES_DEEP, 1)], "{report}");
        let only = |n| Hold::Only([((n, 0), Made::Merged)].into());
        let report = checked(&deep, &only(1), &Options::default());
        assert_eq!(unread_pages(&report), [(PAGES_DEEP, 1)], "{report}");
        let report = checked(&deep, &only(2), &Options::default());
        assert_eq!(unread_pages(&report), [], "{report}");

        let whole = paged(&[(2, &node("null", "3 0 R", "1")), (3, &page("2 0 R"))]);
        let report = checked(&whole, &ALL, &Options::default());
        assert_eq!(unread_pages(&report), [], "{report}");
        assert_eq!(report.findings, [], "{report}");
    }

    /// The model's target dictionary (ISO 32000-2, 12.6.4.4), at a GoToE
    /// action's `/T`, asks `fn:NumberOfPages()` of its `/P` and
    /// `fn:PageProperty` of its `/A`, over the pages the page tree's walk
    /// gives: `/P` past them a `value` finding, within them none; `/A` an
    /// index past the page's `/Annots` and within, and a string equal to an
    /// annotation's `/NM` and to none. A `/P` that's a string, a named
    /// destination, leaves `/A` untold, counted; a target under another's
    /// `/T`, of another document's pages, is counted, never found, its `/P`
    /// past this file's; and a file whose pages aren't read leaves both
    /// counted.
    #[test]
    fn a_targets_page_and_annotation_are_held_to_the_pages_read() {
        let file = |p: &str, a: &str, pages: bool| {
            let catalog = match pages {
                true => "<< /Type /Catalog /Pages 2 0 R /OpenAction 20 0 R >>",
                false => "<< /Type /Catalog /OpenAction 20 0 R >>",
            };
            let target = format!("<< /R /C /P {p} /A {a} /T 22 0 R >>");
            let leaf = page_with(
                "2 0 R",
                "/MediaBox [0 0 612 792] /Resources << >> /Annots [5 0 R]",
            );
            paged(&[
                (1, catalog),
                (2, &node("null", "3 0 R", "1")),
                (3, &leaf),
                (
                    5,
                    "<< /Type /Annot /Subtype /Text /Rect [0 0 1 1] /NM (one) /P 3 0 R >>",
                ),
                (20, "<< /S /GoToE /D (x) /T 21 0 R >>"),
                (21, &target),
                (22, "<< /R /C /N (x) /T 23 0 R >>"),
                (23, "<< /R /C /P 7 /A 0 >>"),
            ])
        };
        // Each finding in a target, and each check not made there.
        let target = |p: &str, a: &str, pages: bool| {
            let report = checked(&file(p, a, pages), &ALL, &Options::default());
            let found: Vec<_> = (report.findings.iter())
                .filter(|f| f.table == "Target")
                .map(|f| {
                    (
                        f.object.map(|(n, _)| n),
                        f.key.clone(),
                        f.rule,
                        f.value.clone(),
                    )
                })
                .collect();
            let counted: Vec<_> = (report.unchecked.iter())
                .filter(|(u, _)| u.table == "Target")
                .map(|(u, &n)| (u.column, u.why.clone(), u.key.clone(), n))
                .collect();
            (found, counted)
        };
        // The checks not made, `/P`'s for want of the pages, and `/A`'s,
        // in how many targets each.
        let untold = |p: usize, a: usize| {
            let row =
                |why: &str, key: &str, n| ("PossibleValues", why.to_string(), key.to_string(), n);
            let rows = [
                row("fn:NumberOfPages", "P", p),
                row("fn:PageProperty", "A", a),
            ];
            rows.into_iter().filter(|row| row.3 > 0).collect::<Vec<_>>()
        };
        let p_past = "1 (not one of fn:Eval((@P>=0) && (@P<fn:NumberOfPages())))";
        let a_past =
            "1 (not one of fn:Eval((@A>=0) && (@A<fn:ArrayLength(fn:PageProperty(@P,Annots)))))";
        let a_none = "(two) (not one of fn:Eval(@A==fn:PageProperty(@P,Annots::@NM)))";
        let value =
            |key: &str, said: &str| (Some(21), key.to_string(), Rule::Value, said.to_string());
        // The nested target's two, alone.
        assert_eq!(target("0", "0", true), (vec![], untold(1, 1)));
        assert_eq!(
            target("1", "0", true),
            (vec![value("P", p_past)], untold(1, 2))
        );
        assert_eq!(
            target("0", "1", true),
            (vec![value("A", a_past)], untold(1, 1))
        );
        assert_eq!(target("0", "(one)", true), (vec![], untold(1, 1)));
        assert_eq!(
            target("0", "(two)", true),
            (vec![value("A", a_none)], untold(1, 1))
        );
        let (found, counted) = target("(chapter)", "0", true);
        assert!(found.iter().all(|f| f.1 == "P"), "{found:?}");
        assert_eq!(counted, untold(1, 2));
        assert_eq!(target("0", "0", false), (vec![], untold(2, 2)));
    }

    /// Each viewer finding of `file`'s check, holding what `hold` does.
    fn viewer_found(file: &[u8], hold: &Hold) -> Vec<Found> {
        found_by(Rule::Viewer, file, hold)
    }

    /// The rows a check counts as not checked for a viewer's pages: how
    /// many.
    fn unread_viewer(report: &Report) -> usize {
        (report.unchecked.iter())
            .filter(|(u, _)| u.why == VIEWER_UNREAD)
            .map(|(_, &n)| n)
            .sum()
    }

    /// A page of `parent`'s whose `/MediaBox` is null, over what it
    /// inherits, which a viewer reads as none.
    fn page_box_null(parent: &str) -> String {
        page_with(parent, "/MediaBox null /Resources << >>")
    }

    /// A4, a page's box a viewer that reads none takes for Letter.
    const A4: &str = "/MediaBox [0 0 595 842]";

    /// What a viewer reads otherwise than the tree's box of A4.
    const SIZE: &str = "page 1's media box is 0 0 595 842, where a viewer's is 0 0 612 792";

    /// What a viewer has no page at, where the tree has 3 0 R.
    const NO_PAGE: &str = "page 1 is 3 0 R, where a viewer has no page";

    /// Where a viewer's pages part from the tree's, each kind is found
    /// once, in a viewer's words, where it is: the count, a `/Count` more
    /// or less than the pages the tree lists, a root with no `/Kids`,
    /// which a viewer takes for its one page, and a tree with no page under
    /// its root, which a viewer counts by the root's `/Count`, or a node's
    /// where the root has none, at the root's `/Count`, else the root; the
    /// pages, a kid that's null, an integer or a reference to nothing,
    /// which a viewer gives a place with no page, a node with no `/Kids`,
    /// which a viewer reads as a leaf it doesn't load, a kid that's a
    /// reference to a reference, which a viewer reads as no page, a page
    /// whose `/Type` a viewer doesn't load, two pages a viewer has none at,
    /// and a node whose `/Type` is `/Page`, which the tree takes for its
    /// page, at the tree's page, or the root, for one written in place; and
    /// the sizes, a `/MediaBox` of null over an inherited one, on one page
    /// and on two, one through a reference to a reference, which a viewer
    /// reads as none, a `/CropBox` of null over an inherited one, a
    /// `/MediaBox` of no area, which a viewer takes for Letter, a
    /// `/CropBox` of no area, which a viewer takes for the media box, and a
    /// `/Rotate` through three references, which a viewer reads two of, at
    /// the page's key.
    #[test]
    fn where_a_viewers_pages_part_from_the_trees_is_found_once_a_kind() {
        let two = node("null", "3 0 R", "1");
        let kids = |kids: &str| paged(&[(2, &node("null", kids, "1")), (3, &page("2 0 R"))]);
        let at_count = |count: &str, value: &str| {
            let value = format!("a viewer counts {value}");
            tree(2, ROOT, "PageTreeNodeRoot/Count", &value, count)
        };
        let page_three =
            |path: &str, value: &str| tree(3, path, "PageObject/-", value, &page("2 0 R"));
        let second = "Root/Pages/Kids[1]";
        let foo = format!("<< /Type /Foo /Parent 2 0 R {A4} >>");
        let in_place = node("null", &foo, "1");
        let page_kids = "<< /Type /Page /Parent 2 0 R /Kids [3 0 R] /Count 1 >>";
        let a4 = format!("<< /Type /Pages /Kids [3 0 R] /Count 1 {A4} >>");
        let a4_two = format!("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 {A4} >>");
        let crop = "<< /Type /Pages /Kids [3 0 R] /Count 1 /CropBox [0 0 300 300] >>";
        let crop_null = page_with("2 0 R", "/MediaBox [0 0 612 792] /CropBox null");
        let rotate = page_with("2 0 R", "/MediaBox [0 0 612 792] /Rotate 4 0 R");
        let no_count = "<< /Type /Pages /Kids [4 0 R] >>";
        let flat_media = page_with("2 0 R", "/MediaBox [0 0 0 0] /Resources << >>");
        let flat_crop = page_with("2 0 R", &format!("{A4} /CropBox [10 10 300 10]"));
        let shapes: Vec<(Vec<u8>, Found)> = vec![
            (
                paged(&[(2, &node("null", "3 0 R", "3")), (3, &page("2 0 R"))]),
                at_count("/Count 3", "3 pages, where the tree lists 1"),
            ),
            (
                paged(&[
                    (2, &node("null", "3 0 R 4 0 R", "1")),
                    (3, &page("2 0 R")),
                    (4, &page("2 0 R")),
                ]),
                at_count("/Count 1", "1 page, where the tree lists 2"),
            ),
            (
                paged(&[(2, "<< /Type /Pages /Count 0 >>")]),
                at_count("/Count 0", "1 page, where the tree lists 0"),
            ),
            (
                paged(&[(2, &node("null", "", "5"))]),
                at_count("/Count 5", "5 pages, where the tree lists 0"),
            ),
            (
                paged(&[(2, &node("null", "", "1"))]),
                at_count("/Count 1", "1 page, where the tree lists 0"),
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R", "2")),
                    (4, &node("2 0 R", "", "0")),
                ]),
                at_count("/Count 2", "2 pages, where the tree lists 0"),
            ),
            (
                paged(&[(2, no_count), (4, &node("2 0 R", "", "3"))]),
                tree(
                    2,
                    ROOT,
                    "PageTreeNodeRoot/Count",
                    "a viewer counts 3 pages, where the tree lists 0",
                    no_count,
                ),
            ),
            (kids("null 3 0 R"), page_three(second, NO_PAGE)),
            (kids("5 3 0 R"), page_three(second, NO_PAGE)),
            (kids("9 0 R 3 0 R"), page_three(second, NO_PAGE)),
            (
                paged(&[
                    (2, &node("null", "4 0 R 3 0 R", "1")),
                    (3, &page("2 0 R")),
                    (4, "<< /Type /Pages /Parent 2 0 R /Count 0 >>"),
                ]),
                page_three(second, NO_PAGE),
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R", "1")),
                    (3, &page("2 0 R")),
                    (4, "3 0 R"),
                ]),
                page_three(KID, NO_PAGE),
            ),
            (
                paged(&[(2, &two), (3, &foo)]),
                tree(3, KID, "PageObject/-", NO_PAGE, &foo),
            ),
            (
                paged(&[
                    (2, &node("null", "null null 3 0 R 4 0 R", "2")),
                    (3, &page("2 0 R")),
                    (4, &page("2 0 R")),
                ]),
                page_three("Root/Pages/Kids[2]", &format!("{NO_PAGE}, 2 times")),
            ),
            (
                paged(&[
                    (2, &node("null", "4 0 R", "1")),
                    (4, page_kids),
                    (3, &page("4 0 R")),
                ]),
                tree(
                    4,
                    KID,
                    "PageObject/-",
                    "page 1 is 4 0 R, where a viewer's is 3 0 R",
                    page_kids,
                ),
            ),
            (
                paged(&[(2, &in_place)]),
                tree(
                    2,
                    ROOT,
                    "PageTreeNodeRoot/-",
                    "page 1 is written in place, where a viewer has no page",
                    &in_place,
                ),
            ),
            (
                paged(&[(2, &a4), (3, &page_box_null("2 0 R"))]),
                tree(3, KID, "PageObject/MediaBox", SIZE, "/MediaBox null"),
            ),
            (
                paged(&[
                    (2, &a4_two),
                    (3, &page_box_null("2 0 R")),
                    (4, &page_box_null("2 0 R")),
                ]),
                tree(
                    3,
                    KID,
                    "PageObject/MediaBox",
                    &format!("{SIZE}, 2 times"),
                    "/MediaBox null",
                ),
            ),
            (
                paged(&[
                    (2, &two),
                    (3, &page_with("2 0 R", "/MediaBox 4 0 R /Resources << >>")),
                    (4, "5 0 R"),
                    (5, "[0 0 595 842]"),
                ]),
                tree(3, KID, "PageObject/MediaBox", SIZE, "/MediaBox 4 0 R"),
            ),
            (
                paged(&[(2, crop), (3, &crop_null)]),
                tree(
                    3,
                    KID,
                    "PageObject/CropBox",
                    "page 1's crop box is 0 0 300 300, where a viewer's is 0 0 612 792",
                    "/CropBox null",
                ),
            ),
            (
                paged(&[(2, &two), (3, &flat_media)]),
                tree(
                    3,
                    KID,
                    "PageObject/MediaBox",
                    "page 1's media box is 0 0 0 0, where a viewer's is 0 0 612 792",
                    "/MediaBox [0 0 0 0]",
                ),
            ),
            (
                paged(&[(2, &two), (3, &flat_crop)]),
                tree(
                    3,
                    KID,
                    "PageObject/CropBox",
                    "page 1's crop box is 10 10 300 10, where a viewer's is 0 0 595 842",
                    "/CropBox [10 10 300 10]",
                ),
            ),
            (
                paged(&[
                    (2, &two),
                    (3, &rotate),
                    (4, "5 0 R"),
                    (5, "6 0 R"),
                    (6, "90"),
                ]),
                tree(
                    3,
                    KID,
                    "PageObject/Rotate",
                    "page 1's rotation is 90, where a viewer's is 0",
                    "/Rotate 4 0 R",
                ),
            ),
        ];
        for (file, expected) in shapes {
            assert_eq!(viewer_found(&file, &ALL), [expected]);
            let report = checked(&file, &ALL, &Options::default());
            assert_eq!(unread_viewer(&report), 0, "{report}");
        }
    }

    /// Nothing is found, nor counted, where a viewer reads the pages the
    /// tree lists, or where its walk stops, which pdf-semantics chooses: a
    /// tree of a page; a page written in place, the same dictionary to
    /// both; an empty tree, where a viewer finds no pages, the same count;
    /// a node listed inside itself; and a `/Parent` chain that loops.
    #[test]
    fn nothing_is_found_where_a_viewer_reads_the_tree_alike_or_its_walk_stops() {
        let two = node("null", "3 0 R", "1");
        let shapes = [
            paged(&[(2, &two), (3, &page("2 0 R"))]),
            paged(&[(2, &node("null", &page("2 0 R"), "1"))]),
            paged(&[(2, "<< /Type /Pages /Kids [] /Count 0 >>")]),
            paged(&[
                (2, &node("null", "4 0 R 3 0 R", "1")),
                (4, &node("2 0 R", "2 0 R", "1")),
                (3, &page("2 0 R")),
            ]),
            paged(&[
                (2, &two),
                (3, &page("4 0 R")),
                (4, &node("5 0 R", "3 0 R", "1")),
                (5, &node("4 0 R", "4 0 R", "1")),
            ]),
        ];
        for file in shapes {
            let report = checked(&file, &ALL, &Options::default());
            let viewer: Vec<_> = (report.findings.iter())
                .filter(|f| f.rule == Rule::Viewer)
                .collect();
            assert_eq!(viewer, Vec::<&Finding>::new(), "{report}");
            assert_eq!(unread_viewer(&report), 0, "{report}");
        }
    }

    /// Where an object of the tree doesn't read, here a kid that doesn't
    /// parse, a viewer's pages aren't set beside the tree's, and that's
    /// counted as not checked, once, where the catalog is held: where both
    /// walks meet it, the two parting by their count and their first page;
    /// where the tree's alone does, past the count a viewer goes by; and
    /// where a viewer's alone does, under a node whose `/Type` is `/Page`,
    /// which the tree takes for its page, the two parting by it.
    #[test]
    fn a_tree_with_an_object_that_doesnt_read_is_counted_not_set_beside_a_viewers() {
        let broken = "<< /Type /Page /Parent 2 0 R /Kids [ >>";
        let page_kids = "<< /Type /Page /Parent 2 0 R /Kids [3 0 R] /Count 1 >>";
        let shapes = [
            paged(&[
                (2, &node("null", "4 0 R 3 0 R", "2")),
                (3, &page("2 0 R")),
                (4, broken),
            ]),
            paged(&[
                (2, &node("null", "3 0 R 4 0 R", "1")),
                (3, &page("2 0 R")),
                (4, broken),
            ]),
            paged(&[
                (2, &node("null", "4 0 R", "1")),
                (4, page_kids),
                (3, broken),
            ]),
        ];
        let only = |n| Hold::Only([((n, 0), Made::Merged)].into());
        for file in shapes {
            assert_eq!(viewer_found(&file, &ALL), []);
            let report = checked(&file, &ALL, &Options::default());
            assert_eq!(unread_viewer(&report), 1, "{report}");
            let report = checked(&file, &only(1), &Options::default());
            assert_eq!(unread_viewer(&report), 1, "{report}");
            let report = checked(&file, &only(2), &Options::default());
            assert_eq!(unread_viewer(&report), 0, "{report}");
        }
    }

    /// Only a viewer finding in an object held is found: the count, in the
    /// root, not where the page alone is held; the pages and the sizes, in
    /// the tree's page, not where the root alone is.
    #[test]
    fn a_viewer_finding_in_an_object_not_held_isnt_found() {
        let held = |ns: &[u32]| Hold::Only(ns.iter().map(|&n| ((n, 0), Made::Merged)).collect());
        let more = paged(&[(2, &node("null", "3 0 R", "3")), (3, &page("2 0 R"))]);
        let count = tree(
            2,
            ROOT,
            "PageTreeNodeRoot/Count",
            "a viewer counts 3 pages, where the tree lists 1",
            "/Count 3",
        );
        assert_eq!(viewer_found(&more, &held(&[2])), [count]);
        assert_eq!(viewer_found(&more, &held(&[1, 3])), []);
        let null = paged(&[(2, &node("null", "null 3 0 R", "1")), (3, &page("2 0 R"))]);
        let no_page = tree(
            3,
            "Root/Pages/Kids[1]",
            "PageObject/-",
            NO_PAGE,
            &page("2 0 R"),
        );
        assert_eq!(viewer_found(&null, &held(&[3])), [no_page]);
        assert_eq!(viewer_found(&null, &held(&[1, 2])), []);
        let a4 = format!("<< /Type /Pages /Kids [3 0 R] /Count 1 {A4} >>");
        let size = paged(&[(2, &a4), (3, &page_box_null("2 0 R"))]);
        let sized = tree(3, KID, "PageObject/MediaBox", SIZE, "/MediaBox null");
        assert_eq!(viewer_found(&size, &held(&[3])), [sized]);
        assert_eq!(viewer_found(&size, &held(&[1, 2])), []);
    }

    /// A viewer finding is carried by a like one whatever its numbers and
    /// count: a page a viewer has no page at, by one at two pages; a box
    /// read otherwise, by another; by number, by path and from a source.
    /// One of another kind is the file's own.
    #[test]
    fn a_viewer_finding_is_carried_whatever_its_numbers() {
        let report = |objects: &[(u32, &str)]| {
            let options = Options {
                paths: true,
                ..Options::default()
            };
            checked(&paged(objects), &ALL, &options)
        };
        let values = |found: Vec<&Finding>| -> Vec<String> {
            (found.into_iter())
                .filter(|f| f.rule == Rule::Viewer)
                .map(|f| f.value.clone())
                .collect()
        };
        let carried = |after: &Report, before: &Report| {
            let none = Vec::<String>::new();
            assert_eq!(values(after.since(before)), none, "{after}");
            assert_eq!(values(after.by_path(before)), none, "{after}");
            assert_eq!(values(after.beyond(&[before])), none, "{after}");
        };
        let leaf = page("2 0 R");
        let once = report(&[(2, &node("null", "null 3 0 R", "1")), (3, &leaf)]);
        let twice = report(&[
            (2, &node("null", "null 3 0 R 4 0 R", "2")),
            (3, &leaf),
            (4, &leaf),
        ]);
        assert_eq!(
            values(twice.findings.iter().collect()),
            [format!("{NO_PAGE}, 2 times")]
        );
        carried(&twice, &once);
        let boxed = |media: &str| {
            let root = format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox {media} >>");
            report(&[(2, &root), (3, &page_box_null("2 0 R"))])
        };
        let (a4, a5) = (boxed("[0 0 595 842]"), boxed("[0 0 420 595]"));
        assert_eq!(
            values(a5.findings.iter().collect()),
            ["page 1's media box is 0 0 420 595, where a viewer's is 0 0 612 792"]
        );
        carried(&a5, &a4);
        let own = [NO_PAGE];
        let file = [
            (2, node("null", "null 3 0 R", "1")),
            (3, page_box_null("2 0 R")),
        ];
        let file: Vec<_> = file.iter().map(|(n, body)| (*n, body.as_str())).collect();
        let mixed = report(&file);
        assert_eq!(values(mixed.since(&a4)), own);
        assert_eq!(values(mixed.by_path(&a4)), own);
        assert_eq!(values(mixed.beyond(&[&a4])), own);
    }

    /// XMP metadata whose namespaces are bound and whose `rdf:Description`
    /// claims PDF/A-2b by its attributes.
    const XMP: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                       xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description \
                       xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"2\" \
                       pdfaid:conformance=\"B\"/></rdf:RDF></x:xmpmeta>";

    /// A file of a page whose catalog's `/Metadata` is 4 0 R, a metadata
    /// stream of `data`, its dictionary's entries besides `more`.
    fn metadata(data: &[u8], more: &str) -> Vec<u8> {
        let stream = [
            format!(
                "<< /Type /Metadata /Subtype /XML {more} /Length {} >>\nstream\n",
                data.len()
            )
            .into_bytes(),
            data.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let page = page("2 0 R");
        let objects: [(u32, &[u8]); 4] = [
            (1, b"<< /Type /Catalog /Pages 2 0 R /Metadata 4 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page.as_bytes()),
            (4, &stream),
        ];
        fixture::assembled("1.7", &objects, "<< /Size 5 /Root 1 0 R >>")
    }

    /// Each XMP finding of `file`'s check, holding what `hold` does, as
    /// [`found`] gives a tree finding, but its bytes those of `data`, the
    /// packet, in its stream, 4 0 R.
    fn xmp_found(file: &[u8], data: &[u8], hold: &Hold) -> Vec<Found> {
        let report = checked(file, hold, &Options::default());
        (report.findings.iter())
            .filter(|f| f.rule == Rule::Xmp)
            .map(|f| {
                let at = f.at.expect("an XMP finding is at bytes");
                assert_eq!(at.stream, Some((4, 0)), "{report}");
                (
                    f.object.map(|(n, _)| n),
                    f.path.clone(),
                    format!("{}/{}", f.table, f.key),
                    f.value.clone(),
                    String::from_utf8_lossy(&data[at.start..at.end]).into_owned(),
                    f.made,
                )
            })
            .collect()
    }

    /// An XMP finding of [`xmp_found`]'s, in 4 0 R as merged.
    fn packet_finding(value: &str, bytes: &str) -> Found {
        tree(4, "Root/Metadata", "Metadata/-", value, bytes)
    }

    /// `XMP` with `from` replaced by `to`.
    fn xmp_with(from: &str, to: &str) -> Vec<u8> {
        assert!(XMP.contains(from));
        XMP.replace(from, to).into_bytes()
    }

    const ATTRIBUTE: &str = "an attribute in XMP metadata doesn't read";
    const VALUE: &str = "a value in XMP metadata doesn't read";

    /// What the packet's read tells is found once each, in the metadata
    /// stream, at its bytes in the stream's data, its path the stream's,
    /// its table `Metadata` and its key `-`: a mark of UTF-16 it begins
    /// with, at the mark; an attribute with no quotes, at its tag; a
    /// claim's value with an entity that isn't XML's, at its tag; and XML
    /// that stops the read, at the byte it stops at, in the XML reader's
    /// words. A packet that reads, and one in a stream that's filtered, so
    /// not in plain view, have nothing found. Two of a kind are one
    /// finding, with how many times, and the kinds come in the order first
    /// told, what stops the read last.
    #[test]
    fn what_a_packets_read_tells_is_found_once_a_kind_at_its_bytes() {
        let tag = |xmp: &[u8]| {
            let xmp = String::from_utf8_lossy(xmp).into_owned();
            let start = xmp.find("<rdf:Description").unwrap();
            let end = start + xmp[start..].find("/>").unwrap() + 2;
            xmp[start..end].to_string()
        };
        let utf16: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain(XMP.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        let unquoted = xmp_with(r#"pdfaid:conformance="B""#, "pdfaid:conformance=B");
        let entity = xmp_with(r#"pdfaid:part="2""#, r#"pdfaid:part="&two;""#);
        let misnested = xmp_with("</rdf:RDF>", "</rdf:RFD>");
        let words = "XMP metadata isn't XML: ill-formed document: expected `</rdf:RDF>`, \
                     but `</rdf:RFD>` was found";
        let shapes = [
            (
                utf16,
                packet_finding(
                    "XMP metadata begins with a mark of UTF-16, not as UTF-8",
                    "\u{FFFD}\u{FFFD}",
                ),
            ),
            (unquoted.clone(), packet_finding(ATTRIBUTE, &tag(&unquoted))),
            (entity.clone(), packet_finding(VALUE, &tag(&entity))),
            (misnested.clone(), packet_finding(words, "<")),
        ];
        for (data, expected) in shapes {
            let file = metadata(&data, "");
            assert_eq!(xmp_found(&file, &data, &ALL), [expected]);
            let report = checked(&file, &ALL, &Options::default());
            let rows = (report.unchecked.keys()).filter(|u| u.table == "Metadata");
            assert_eq!(rows.count(), 0, "{report}");
        }
        // The read stops at `</rdf:RFD>`, its first byte.
        let at = String::from_utf8_lossy(&misnested)
            .find("</rdf:RFD>")
            .unwrap();
        let report = checked(&metadata(&misnested, ""), &ALL, &Options::default());
        let stops = report
            .findings
            .iter()
            .find(|f| f.rule == Rule::Xmp)
            .unwrap();
        assert_eq!(stops.at, Some(Span::new(Some((4, 0)), at, at + 1)));
        let clean = metadata(XMP.as_bytes(), "");
        assert_eq!(xmp_found(&clean, XMP.as_bytes(), &ALL), []);
        let deflated = fixture::zlib(&misnested);
        let filtered = metadata(&deflated, "/Filter /FlateDecode");
        assert_eq!(xmp_found(&filtered, &deflated, &ALL), []);

        let both = [
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><a b=1/><c d=2/>",
            "<e xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"&two;\"/>",
            "</x:xmpmeta></f>",
        ]
        .concat();
        let e = both.find("<e ").unwrap();
        let e = &both[e..e + both[e..].find("/>").unwrap() + 2];
        let found = xmp_found(&metadata(both.as_bytes(), ""), both.as_bytes(), &ALL);
        let values: Vec<_> = found.iter().map(|f| (f.3.as_str(), f.4.as_str())).collect();
        let twice = format!("{ATTRIBUTE}, 2 times");
        let words = "XMP metadata isn't XML: ill-formed document: close tag `</f>` does not \
                     match any open tag";
        assert_eq!(
            values,
            [(twice.as_str(), "<a b=1/>"), (VALUE, e), (words, "<")]
        );
    }

    /// A packet longer, nested deeper or binding more namespaces at once
    /// than is read isn't read whole, which is counted as not checked, once,
    /// where its stream is held, and not where it isn't; what was told
    /// before is found all the same: here, 257 elements one inside another,
    /// past the 256 read, after an attribute with no quotes; and an element
    /// binding 129 namespaces, past the 128 read.
    #[test]
    fn a_packet_past_what_s_read_is_counted_what_was_read_found() {
        let deep = ["<a b=1/>", &"<e>".repeat(257), &"</e>".repeat(257)].concat();
        let file = metadata(deep.as_bytes(), "");
        let rows = |report: &Report| -> Vec<(String, usize)> {
            (report.unchecked.iter())
                .filter(|(u, _)| {
                    (u.column, u.table.as_str(), u.key.as_str()) == ("-", "Metadata", "-")
                })
                .map(|(u, &n)| (u.why.clone(), n))
                .collect()
        };
        let report = checked(&file, &ALL, &Options::default());
        assert_eq!(rows(&report), [(XMP_DEEP.to_string(), 1)]);
        assert_eq!(
            xmp_found(&file, deep.as_bytes(), &ALL),
            [packet_finding(ATTRIBUTE, "<a b=1/>")]
        );
        let only = |n| Hold::Only([((n, 0), Made::Merged)].into());
        assert_eq!(
            rows(&checked(&file, &only(4), &Options::default())).len(),
            1
        );
        assert_eq!(rows(&checked(&file, &only(1), &Options::default())), []);
        assert_eq!(xmp_found(&file, deep.as_bytes(), &only(1)), []);

        let bound = |n: usize| -> Vec<u8> {
            let bound: String = (0..n)
                .map(|i| format!(" xmlns:p{i}=\"http://example.com/{i}\""))
                .collect();
            format!("<e{bound}/>").into_bytes()
        };
        let wide = checked(&metadata(&bound(129), ""), &ALL, &Options::default());
        assert_eq!(rows(&wide), [(XMP_BINDINGS.to_string(), 1)]);
        let within = checked(&metadata(&bound(128), ""), &ALL, &Options::default());
        assert_eq!(rows(&within), []);
    }

    /// A document given its packet with no `/Metadata` to name the stream
    /// has what the packet tells counted as not checked, once, where the
    /// catalog is held, and nothing found, as it has no object to be in;
    /// a packet that tells nothing isn't counted. What the packet claims is
    /// the same with a fault as without: here, a fault beside PDF/A-3.
    #[test]
    fn a_packet_whose_stream_isnt_known_is_counted_not_found() {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        doc.insert(
            1,
            dict! { "Type" => "Catalog", "Pages" => Object::reference(2) },
        );
        let kids = vec![Object::reference(3)];
        doc.insert(2, dict! { "Type" => "Pages", "Kids" => kids, "Count" => 1 });
        let page = dict! {
            "Type" => "Page",
            "Parent" => Object::reference(2),
            "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
            "Resources" => dict! {},
        };
        doc.insert(3, page);
        let unplaced = |report: &Report| {
            (report.unchecked.iter())
                .filter(|(u, _)| u.why == XMP_UNPLACED)
                .map(|(u, &n)| (u.column, u.table.clone(), u.key.clone(), n))
                .collect::<Vec<_>>()
        };
        let only = |n| Hold::Only([((n, 0), Made::Merged)].into());
        let claims = |attributes: &str| {
            format!(
                "<rdf:Description xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" {attributes}/>"
            )
            .into_bytes()
        };
        let a3 = BTreeSet::from(["ISO_19005_3".to_string()]);
        for (packet, told) in [
            (claims(r#"pdfaid:part="3""#), false),
            (claims(r#"pdfaid:part="3" x=y"#), true),
            (
                [claims(r#"pdfaid:part="3""#), b"</e>".to_vec()].concat(),
                true,
            ),
        ] {
            doc.metadata = Some(packet);
            let report = check(&doc, &ALL, &Options::default());
            let xmp = (report.findings.iter()).filter(|f| f.rule == Rule::Xmp);
            assert_eq!(xmp.count(), 0, "{report}");
            let row = ("-", "Metadata".to_string(), "-".to_string(), 1);
            let expected = if told { vec![row] } else { vec![] };
            assert_eq!(unplaced(&report), expected, "{report}");
            assert_eq!(report.extensions, a3, "{report}");
            let held = check(&doc, &only(1), &Options::default());
            assert_eq!(unplaced(&held).len(), usize::from(told));
            assert_eq!(unplaced(&check(&doc, &only(2), &Options::default())), []);
        }
        // A file's packet, with a fault and without, declares the same.
        let a3 = |attributes: &str| {
            let data = xmp_with(r#"pdfaid:part="2""#, attributes);
            let data = String::from_utf8(data)
                .unwrap()
                .replace(r#"pdfaid:conformance="B""#, "");
            checked(&metadata(data.as_bytes(), ""), &ALL, &Options::default()).extensions
        };
        assert_eq!(a3(r#"pdfaid:part="3" x=y"#), a3(r#"pdfaid:part="3""#));
        assert!(a3(r#"pdfaid:part="3" x=y"#).contains("ISO_19005_3"));
    }

    /// An XMP finding is carried by a like one whatever its numbers and
    /// count, by number, by path and from a source: an attribute that
    /// doesn't read twice by one once. One of another kind is the file's own.
    #[test]
    fn an_xmp_finding_is_carried_whatever_its_count() {
        let report = |data: &str| {
            let options = Options {
                paths: true,
                ..Options::default()
            };
            checked(&metadata(data.as_bytes(), ""), &ALL, &options)
        };
        let values = |found: Vec<&Finding>| -> Vec<String> {
            (found.into_iter())
                .filter(|f| f.rule == Rule::Xmp)
                .map(|f| f.value.clone())
                .collect()
        };
        let (once, twice) = (report("<a b=1/>"), report("<a b=1/><c d=2/>"));
        assert_eq!(
            values(twice.findings.iter().collect()),
            [format!("{ATTRIBUTE}, 2 times")]
        );
        let none = Vec::<String>::new();
        assert_eq!(values(twice.since(&once)), none, "{twice}");
        assert_eq!(values(twice.by_path(&once)), none, "{twice}");
        assert_eq!(values(twice.beyond(&[&once])), none, "{twice}");
        let other = report(r#"<e xmlns:p="http://www.aiim.org/pdfa/ns/id/" p:part="&two;"/>"#);
        assert_eq!(values(other.since(&once)), [VALUE]);
        assert_eq!(values(other.by_path(&once)), [VALUE]);
        assert_eq!(values(other.beyond(&[&once])), [VALUE]);
    }

    /// Only an object that doesn't read is noted: the one fault PDFium's
    /// rules tell, and the walk's others, which they don't, aren't.
    #[test]
    fn damage_notes_an_object_that_doesn_t_read_alone() {
        let mut damage = Damage::default();
        damage.fault(at(Some((2, 0))), Fault::PageTreeEmpty);
        damage.fault(at(None), Fault::PageNoMediaBox);
        assert!(!damage.told);
        damage.fault(at(Some((2, 0))), Fault::Damaged { id: (7, 0) });
        damage.fault(at(Some((2, 0))), Fault::PageTreeEmpty);
        assert!(damage.told);
    }
}
