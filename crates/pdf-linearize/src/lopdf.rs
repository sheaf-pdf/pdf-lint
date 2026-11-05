//! A [`Document`] of lopdf's written as a linearized file, by plain functions
//! over its public objects: no lopdf type goes into the engine, and nothing of
//! the family's crates is used. [`graph`] says what [`plan()`] is told of a
//! document, and [`linearize`] writes the file.
//!
//! The file is the plainest there is: cross-reference tables, nothing in object
//! streams, the hint stream unfiltered, so nothing but lopdf is pulled in.
//! lopdf has no public writer of one object, so a small one is here, in the
//! plan's numbers: a reference to no object is `null` (7.3.10), a string is
//! hexadecimal, a stream's `/Length` is direct and its data as lopdf holds it,
//! a real as its `f32` prints, with a point. The `/ID` is as the document holds
//! it, as lopdf's own save leaves it: a caller who changed the document says so
//! there.
//!
//! # Where everything goes
//!
//! As the family's adapter has it, from the same keys: part 4 has the catalog
//! and the values of its `/ViewerPreferences`, `/PageMode`, `/OpenAction`,
//! `/AcroForm` (that object alone) and `/Threads` with its thread dictionaries
//! (F.3.5); each page's objects are found by walking from it, in the order
//! F.3.7 recommends, images and font files last, never through `/Parent` or
//! `/Thumb`, stopping at the page tree's and the document parts' nodes; part 9
//! has the groups a hint table names (F.3.10). The first page is where
//! `/OpenAction` opens (F.3.7), else page 0, unless [`Options::first_page`]
//! says. So a document read from one file has the numbers the family adapter
//! gives that file, where the two read the same objects.
//!
//! # What changes in the document
//!
//! Nothing in the [`Document`] given, which is only read. In what's written:
//!
//! - **Inheritable attributes are pushed down** (F.3.7, F.3.10): each page gets
//!   what it inherited, as it was written, a reference staying one, and each
//!   page tree node loses it, so a page the tree doesn't list gets what it
//!   inherited from such a node too.
//! - **What the trailer doesn't reach isn't written**, nor object streams,
//!   cross-reference streams and a linearization dictionary; nor an object only
//!   a stream's `/Length` names, nor a null object, nor an object that's only a
//!   reference that ends at one of those or at nothing, a reference to any of
//!   which reads the same as one to none.
//!
//! Every object moves, as Annex F has it, so anything that names one from
//! outside the file, a signature's byte range above all, doesn't hold
//! afterwards.
//!
//! # What's refused
//!
//! - **A document that's encrypted or was** ([`Error::Encrypted`]), rather than
//!   write it in the clear or half encrypted: [`Document::was_encrypted`], as
//!   lopdf's reader decrypts a file the empty password opens and takes
//!   `/Encrypt` out of its trailer, so that it looks plain, or any `/Encrypt`
//!   in the trailer. lopdf's encryption works on a whole document under each
//!   object's own number, where every number here is new.
//! - **A page tree that gives a page no place of its own**
//!   ([`Error::PageTree`]), as Annex F gives each page one, and the numbers and
//!   hint tables depend on which pages a viewer finds: a place with no page at
//!   it, a page or node written in place or listed twice, a node its own
//!   ancestor, a tree whose pages by ISO 32000-2's rules aren't what
//!   [`Document::get_pages`] lists, a kid those rules and PDFium's read as
//!   different things, a page they read an inheritable attribute of
//!   differently, or that reads otherwise going down `/Kids` from the root than
//!   up its `/Parent`s, as pushing one value down would change the other's
//!   page; a tree past 1,024 levels or too many pages, and a value it's read by
//!   that's a chain of references past 32 or going round, as a reader stops
//!   there.
//! - **What can't be written**: a real that isn't finite, a stream inside
//!   another object, a stream lopdf didn't read the data of.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::io::Write as _;

use ::lopdf::{Dictionary, Document, Object, Stream};

use crate::layout::{self, Encoded, Layout, Piece, assemble};
pub use crate::plan::FirstPage;
use crate::plan::{self, Graph, Group, Kind, Page, Plan, What, plan};
use pdf_objects::ObjectId;

use crate::Limits;

/// The attributes a page inherits (7.7.3.4).
const INHERITED: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];

/// How many levels of nodes a tree has, from its root down, and how far up a
/// page's `/Parent`s are followed: a tree deeper than this is refused.
const DEPTH: usize = 1024;

/// How many references a value of the tree is followed through: one past them,
/// or going round, is refused.
const HOPS: usize = 32;

/// How many kids a walk looks at for each page it may list, as one that's no
/// page costs as one that is.
const KIDS_A_PAGE: usize = 4;

/// How deep what's written may be nested: past it, nothing's written. lopdf's
/// reader nests far less than this.
const NESTED: usize = 256;

/// The trailer keys a section writes anew or doesn't have: its links to other
/// sections, and a cross-reference stream's dictionary's own, which is the
/// trailer lopdf holds where a stream listed the file.
const SECTION_KEYS: [&[u8]; 13] = [
    b"Prev",
    b"XRefStm",
    b"Type",
    b"Size",
    b"W",
    b"Index",
    b"Length",
    b"Filter",
    b"DecodeParms",
    b"F",
    b"FFilter",
    b"FDecodeParms",
    b"DL",
];

/// Why a page tree can't be laid out, each page by its place in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PageTree {
    /// A place in the tree that names no dictionary.
    #[error("page {page} is a place with no page at it")]
    NoPage {
        /// The place.
        page: usize,
    },
    /// A kid written in place, which can't be numbered.
    #[error("page {page} is written in place, so it has no number")]
    Direct {
        /// The place.
        page: usize,
    },
    /// A page or a node listed before, its own ancestor among them.
    #[error("page {page} is a page or a node listed before")]
    Repeated {
        /// The place.
        page: usize,
    },
    /// The pages by ISO 32000-2's rules aren't those [`Document::get_pages`]
    /// lists: `page` is the first that differs, or the first past the shorter's
    /// end.
    #[error("page {page} is another page, or none, by lopdf's list than by ISO 32000-2's rules")]
    Listed {
        /// The place.
        page: usize,
    },
    /// A kid ISO 32000-2's rules and PDFium's read as different things: a page
    /// or a node by its `/Type` where its `/Kids` says the other, or an object
    /// that's itself a reference, which PDFium's rules don't go through.
    #[error("page {page} is another page, or none, by PDFium's rules than by ISO 32000-2's")]
    Rules {
        /// The place.
        page: usize,
    },
    /// A chain of `/Parent`s that's a loop, or too long to follow.
    #[error("the /Parent chain of page {page} goes round, or past {DEPTH}")]
    Parents {
        /// The place.
        page: usize,
    },
    /// A value the tree is read by that's a chain of references past 32, or in
    /// a loop of them, so nothing can say what it reads as.
    #[error("{id} R is past {HOPS} references, or in a loop of them")]
    Unread {
        /// What the last it followed names.
        id: ObjectId,
    },
    /// More levels of nodes than 1,024, under the root.
    #[error("the page tree is too deep")]
    TooDeep,
    /// More pages, nodes or kids than the most pages that's allowed gives room
    /// for.
    #[error("the page tree lists too many pages")]
    TooMany,
    /// A tree node written in place that gives a page an attribute, which can't
    /// be taken off it.
    #[error("page {page} inherits from a page tree node written in place")]
    DirectNode {
        /// The place.
        page: usize,
    },
    /// An inheritable attribute of a page that reads otherwise going down
    /// `/Kids` from the root than up its `/Parent`s, where a page's `/Parent`
    /// isn't the node that lists it, as pushing it down would change one
    /// reader's page.
    #[error("/{attribute} of page {page} reads otherwise going down /Kids than up /Parent")]
    Kids {
        /// The place.
        page: usize,
        /// The attribute's key.
        attribute: &'static str,
    },
    /// An inheritable attribute with a null nearer the page than a value: ISO
    /// 32000-2's rules go past a null to the value, and PDFium's stop at it, so
    /// pushing the value down would change one reader's page.
    #[error("/{attribute} of page {page} reads otherwise by PDFium's rules than by ISO 32000-2's")]
    Attribute {
        /// The place.
        page: usize,
        /// The attribute's key.
        attribute: &'static str,
    },
}

/// Why nothing was written.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A document that's encrypted, or was when lopdf read it.
    #[error("it's encrypted, or was when it was read")]
    Encrypted,
    /// A trailer whose `/Root` names no dictionary.
    #[error("the trailer's /Root names no catalog")]
    NoCatalog,
    /// A page tree that gives a page no place of its own.
    #[error("{0}")]
    PageTree(PageTree),
    /// The graph can't be planned.
    #[error("{0}")]
    Plan(plan::Error),
    /// The bytes around the bodies can't be made.
    #[error("{0}")]
    Layout(layout::Error<Infallible>),
    /// A real that's infinite or not a number, which a file can't hold.
    #[error("object {} {} holds a real that isn't finite", .id.number, .id.generation)]
    NonFinite {
        /// The object.
        id: ObjectId,
    },
    /// A stream inside another object, which only an object of its own can be
    /// (7.3.8).
    #[error("object {} {} holds a stream inside it", .id.number, .id.generation)]
    NestedStream {
        /// The object.
        id: ObjectId,
    },
    /// A stream whose data lopdf didn't read, as its `/Length` didn't hold:
    /// what's held is empty, where the file's isn't.
    #[error("object {} {} is a stream whose data wasn't read", .id.number, .id.generation)]
    UnreadStream {
        /// The object.
        id: ObjectId,
    },
    /// An object nested past what's written.
    #[error("object {} {} is nested past {NESTED}", .id.number, .id.generation)]
    TooDeep {
        /// The object.
        id: ObjectId,
    },
}

/// How a file is laid out. A caller sets the fields on a copy of
/// [`Options::DEFAULT`] (`options.first_page = FirstPage::Index(3)`, say).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Options {
    /// Which page goes first.
    pub first_page: FirstPage,
    /// The plan's caps, the most pages among them.
    pub limits: Limits,
}

impl Options {
    /// Opening where `/OpenAction` says, under [`Limits::DEFAULT`].
    pub const DEFAULT: Options = Options {
        first_page: FirstPage::OpenAction,
        limits: Limits::DEFAULT,
    };
}

impl Default for Options {
    fn default() -> Self {
        Options::DEFAULT
    }
}

/// What [`plan()`] is told of `doc`: its objects as they'd be written, each
/// with what it refers to, as a table lists them, nothing packable. Refused as
/// [`linearize`] is.
pub fn graph(doc: &Document, options: &Options) -> Result<Graph, Error> {
    Ok(Prepared::of(doc, options)?.graph())
}

/// `doc`, as a linearized file, at the end of `out`: every object it reaches
/// written at the number a plan gives it, what's written counting its positions
/// from where it starts in `out`. It gives the [`Layout`], which says where
/// everything is.
///
/// Refused, with `out` as it was given, by an [`Error`]: what the module says;
/// a trailer with no catalog; a document past [`Options::limits`], or a file
/// past the 4 GiB a hint table can describe.
pub fn linearize(out: &mut Vec<u8>, doc: &Document, options: &Options) -> Result<Layout, Error> {
    let prepared = Prepared::of(doc, options)?;
    let plan = plan(&prepared.graph(), &options.limits).map_err(Error::Plan)?;

    // Bodies first: a table's positions depend only on their lengths.
    let mut bodies = Vec::new();
    let mut starts = Vec::with_capacity(plan.writes().len());
    for write in plan.writes() {
        starts.push(bodies.len());
        if let What::Object(id) = write.what {
            if let Some(object) = prepared.body(id) {
                indirect(&mut bodies, id, write.number, object, &plan)?;
            }
        }
    }
    let ends = starts.iter().skip(1).copied().chain([bodies.len()]);
    let lengths: Vec<u64> = (starts.iter().zip(ends))
        .map(|(start, end)| (end - start) as u64)
        .collect();

    let trailer = trailer_entries(doc, &plan)?;
    let mut around = layout::Options::new(version(doc));
    around.trailer = &trailer;
    let layout = assemble(&plan, &lengths, &around, |stream| {
        Ok::<_, Infallible>(Encoded::plain(stream.data.to_vec()))
    })
    .map_err(Error::Layout)?;

    out.reserve(layout.len() as usize);
    for piece in layout.pieces() {
        match piece {
            Piece::Bytes(bytes) => out.extend_from_slice(bytes),
            Piece::Body(write) => {
                let start = starts[write];
                out.extend_from_slice(&bodies[start..start + lengths[write] as usize]);
            }
        }
    }
    Ok(layout)
}

/// The header's version, as lopdf holds it, or 1.7 where it isn't one.
fn version(doc: &Document) -> (u8, u8) {
    let parsed = doc.version.split_once('.').and_then(|(major, minor)| {
        let major = major.parse().ok()?;
        Some((major, minor.parse().ok()?))
    });
    parsed.unwrap_or((1, 7))
}

/// The trailer's own entries, in the numbers written, as a section takes them.
fn trailer_entries(doc: &Document, plan: &Plan) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    for (key, value) in &doc.trailer {
        if key == b"Root" || SECTION_KEYS.contains(&&key[..]) {
            continue;
        }
        if !out.is_empty() {
            out.push(b' ');
        }
        name(&mut out, key);
        out.push(b' ');
        // The trailer's no object: a fault in it is said of object 0.
        written(&mut out, value, ObjectId::new(0, 0), plan, 0)?;
    }
    Ok(out)
}

/// `object` as an indirect object numbered `at`, its references in `plan`'s
/// numbers.
fn indirect(
    out: &mut Vec<u8>,
    id: ObjectId,
    at: u32,
    object: &Object,
    plan: &Plan,
) -> Result<(), Error> {
    let _ = writeln!(out, "{at} 0 obj");
    match object {
        Object::Stream(stream) => {
            if unread(stream) {
                return Err(Error::UnreadStream { id });
            }
            dictionary(out, &stream.dict, Some(stream.content.len()), id, plan, 0)?;
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&stream.content);
            out.extend_from_slice(b"\nendstream");
        }
        object => written(out, object, id, plan, 0)?,
    }
    out.extend_from_slice(b"\nendobj\n");
    Ok(())
}

/// Whether `stream` is one lopdf's reader meant to come back to and couldn't:
/// it holds nothing, and the `/Length` it would have made direct isn't.
fn unread(stream: &Stream) -> bool {
    let empty = stream.start_position.is_some() && stream.content.is_empty();
    empty && !matches!(stream.dict.get(b"Length"), Ok(Object::Integer(0)))
}

/// `object`, nested `depth` deep in what `id` writes.
fn written(
    out: &mut Vec<u8>,
    object: &Object,
    id: ObjectId,
    plan: &Plan,
    depth: usize,
) -> Result<(), Error> {
    if depth > NESTED {
        return Err(Error::TooDeep { id });
    }
    match object {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(true) => out.extend_from_slice(b"true"),
        Object::Boolean(false) => out.extend_from_slice(b"false"),
        Object::Integer(value) => {
            let _ = write!(out, "{value}");
        }
        Object::Real(value) => real(out, *value, id)?,
        Object::Name(text) => name(out, text),
        Object::String(text, _) => {
            out.push(b'<');
            for byte in text {
                let _ = write!(out, "{byte:02X}");
            }
            out.push(b'>');
        }
        Object::Array(items) => {
            out.push(b'[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(b' ');
                }
                written(out, item, id, plan, depth + 1)?;
            }
            out.push(b']');
        }
        Object::Dictionary(dict) => dictionary(out, dict, None, id, plan, depth)?,
        Object::Stream(_) => return Err(Error::NestedStream { id }),
        Object::Reference(to) => match plan.number((*to).into()) {
            Some(id) => {
                let _ = write!(out, "{id} R");
            }
            None => out.extend_from_slice(b"null"),
        },
    }
    Ok(())
}

/// `dict`, every entry in order, and where it's a stream's, `length` as its
/// `/Length`, at the place it was or last.
fn dictionary(
    out: &mut Vec<u8>,
    dict: &Dictionary,
    length: Option<usize>,
    id: ObjectId,
    plan: &Plan,
    depth: usize,
) -> Result<(), Error> {
    out.extend_from_slice(b"<<");
    let mut said = false;
    for (key, value) in dict {
        out.push(b' ');
        name(out, key);
        out.push(b' ');
        match length {
            Some(length) if key == b"Length" => {
                let _ = write!(out, "{length}");
                said = true;
            }
            _ => written(out, value, id, plan, depth + 1)?,
        }
    }
    if let (Some(length), false) = (length, said) {
        let _ = write!(out, " /Length {length}");
    }
    out.extend_from_slice(b" >>");
    Ok(())
}

/// `value` as a real: with a point, as without one it reads as an integer,
/// however large (7.3.3); what's too small for a reader to hold is 0.
fn real(out: &mut Vec<u8>, value: f32, id: ObjectId) -> Result<(), Error> {
    if !value.is_finite() {
        return Err(Error::NonFinite { id });
    }
    let value = if value.abs() < f32::MIN_POSITIVE {
        0.0
    } else {
        value
    };
    let start = out.len();
    let _ = write!(out, "{value}");
    if !out[start..].contains(&b'.') {
        out.extend_from_slice(b".0");
    }
    Ok(())
}

/// `text` as a name, each byte that isn't a regular character by its code
/// (7.3.5).
fn name(out: &mut Vec<u8>, text: &[u8]) {
    out.push(b'/');
    for &byte in text {
        let regular = (b'!'..=b'~').contains(&byte) && !b"()<>[]{}/%#".contains(&byte);
        if regular {
            out.push(byte);
        } else {
            let _ = write!(out, "#{byte:02X}");
        }
    }
}

/// A document ready to plan: the objects to write and what a [`Graph`] says of
/// them.
struct Prepared<'d> {
    catalog: ObjectId,
    /// Each object to write, the document's own or, where something's pushed
    /// down or taken off, a copy.
    bodies: Bodies<'d>,
    pages: Vec<Page>,
    page_ids: HashSet<ObjectId>,
    first_page: u32,
    tree_nodes: Vec<ObjectId>,
    open: Vec<ObjectId>,
    groups: Vec<(Group, Vec<ObjectId>)>,
    outlines_first: bool,
    /// The outline's objects, whose links to each other aren't walked, as
    /// they're given to the plan in the order they go.
    outline: HashSet<ObjectId>,
    /// The name dictionary, of which only `/Dests` is walked, as it's given to
    /// the plan with the named destinations and the rest have groups of their
    /// own.
    names: Option<ObjectId>,
    /// What's named as a font file.
    font_files: HashSet<ObjectId>,
}

/// Each object to write, by id.
type Bodies<'d> = BTreeMap<ObjectId, Cow<'d, Object>>;

impl<'d> Prepared<'d> {
    fn of(doc: &'d Document, options: &Options) -> Result<Prepared<'d>, Error> {
        // What `is_encrypted` looks for, and more: it says no of an /Encrypt in
        // place, or one that names no dictionary, which a section would then
        // carry over the numbers it was written under.
        if doc.was_encrypted() || doc.trailer.has(b"Encrypt") {
            return Err(Error::Encrypted);
        }
        let root = doc.trailer.get(b"Root").ok();
        let root = root.filter(|root| root.as_reference().is_ok());
        let found = root.and_then(|root| doc.dereference(root).ok());
        let (catalog_id, catalog) = found
            .and_then(|(id, object)| Some((id?, object.as_dict().ok()?)))
            .ok_or(Error::NoCatalog)?;

        let tree = page_tree(doc, catalog, &options.limits).map_err(Error::PageTree)?;
        let table = &tree.pages;
        let reached = reached(doc);
        let mut bodies: Bodies<'d> = (doc.objects.iter())
            .filter(|(id, object)| reached.contains(&ObjectId::from(**id)) && !no_body(doc, object))
            .map(|(id, object)| (ObjectId::from(*id), Cow::Borrowed(object)))
            .collect();

        let page_ids: HashSet<ObjectId> = table.iter().map(|page| page.id).collect();
        pushed_down(&mut bodies, table)?;
        if let Some(why) = tree.apart {
            return Err(Error::PageTree(why));
        }
        let mut tree_nodes = nodes_taken_off(doc, catalog, &tree, &page_ids, &mut bodies);
        // A page or a node the tree doesn't list is still no object of the page
        // that names it (F.3.7).
        let listed: HashSet<ObjectId> = tree_nodes.iter().copied().collect();
        let strays: Vec<ObjectId> = (bodies.iter())
            .filter(|(id, body)| {
                let tree = |dict: &Dictionary| dict.has_type(b"Page") || dict.has_type(b"Pages");
                !page_ids.contains(id) && dict_of(body).is_some_and(tree)
            })
            .map(|(id, _)| *id)
            .collect();
        strays_pushed_down(doc, &strays, &listed, &mut bodies);
        tree_nodes.extend(strays.into_iter().filter(|id| !listed.contains(id)));

        let pages = (table.iter())
            .map(|listed| {
                let held = bodies.get(&listed.id);
                page_said(doc, listed.id, held.and_then(|body| dict_of(body)))
            })
            .collect();

        let mut prepared = Prepared {
            catalog: catalog_id.into(),
            pages,
            page_ids,
            first_page: first_page(doc, catalog, table, options.first_page),
            tree_nodes,
            open: Vec::new(),
            groups: Vec::new(),
            outlines_first: false,
            outline: HashSet::new(),
            names: None,
            font_files: font_files(&bodies),
            bodies,
        };
        prepared.document_level(doc, catalog);
        Ok(prepared)
    }

    /// Part 4 and part 9's groups, and the document parts' nodes (F.3.5,
    /// F.3.10).
    fn document_level(&mut self, doc: &'d Document, catalog: &'d Dictionary) {
        for key in [
            &b"ViewerPreferences"[..],
            b"PageMode",
            b"OpenAction",
            b"AcroForm",
        ] {
            self.open.extend(reference(catalog.get(key).ok()));
        }
        // The threads and each thread's dictionary; beads are their pages', and
        // what a thread's /I names is a group's.
        if let Ok(threads) = catalog.get(b"Threads") {
            let mut thread_info = Vec::new();
            self.open.extend(reference(Some(threads)));
            for thread in array_of(doc, threads) {
                self.open.extend(reference(Some(thread)));
                let dict = doc.dereference(thread).ok().and_then(|(_, o)| dict_of(o));
                if let Some(info) = dict.and_then(|dict| dict.get(b"I").ok()) {
                    thread_info.extend(roots(info));
                }
            }
            self.groups.push((Group::ThreadInfo, thread_info));
        }
        document_parts(doc, catalog, &mut self.tree_nodes);

        if let Ok(outlines) = catalog.get(b"Outlines") {
            let order = outline_order(doc, outlines);
            self.outline = order.iter().copied().collect();
            self.groups.push((Group::Outlines, order));
        }
        let mode = catalog.get(b"PageMode").ok();
        let mode = mode.and_then(|mode| doc.dereference(mode).ok());
        self.outlines_first = mode
            .is_some_and(|(_, mode)| matches!(mode, Object::Name(mode) if mode == b"UseOutlines"));

        let names = catalog.get(b"Names").ok();
        let name_dict = names.and_then(|names| doc.dereference(names).ok());
        let name_dict = name_dict.and_then(|(_, names)| dict_of(names));
        let of_names = |key: &[u8]| name_dict.and_then(|names| names.get(key).ok());
        let name_tree = of_names(b"Dests");
        if catalog.has(b"Dests") || name_tree.is_some() {
            let mut dests: Vec<ObjectId> =
                catalog.get(b"Dests").into_iter().flat_map(roots).collect();
            if name_tree.is_some() {
                self.names = reference(names);
                dests.extend(self.names);
            }
            dests.extend(name_tree.into_iter().flat_map(roots));
            self.groups.push((Group::Dests, dests));
        }
        if let Ok(info) = doc.trailer.get(b"Info") {
            self.groups.push((Group::Info, roots(info)));
        }
        let form = catalog.get(b"AcroForm").ok();
        let form = form.and_then(|form| doc.dereference(form).ok());
        if let Some(form) = form.and_then(|(_, form)| dict_of(form)) {
            let fields = form.get(b"Fields").into_iter().flat_map(roots).collect();
            self.groups.push((Group::Form, fields));
        }
        if let Ok(structure) = catalog.get(b"StructTreeRoot") {
            self.groups.push((Group::Structure, roots(structure)));
        }
        if let Some(renditions) = of_names(b"Renditions") {
            self.groups.push((Group::Renditions, roots(renditions)));
        }
        if let Some(files) = of_names(b"EmbeddedFiles") {
            let entries = name_tree_entries(doc, files);
            let files = entries.into_iter().flat_map(|(_, file)| roots(file));
            self.groups.push((Group::EmbeddedFile, files.collect()));
        }
    }

    fn body(&self, id: ObjectId) -> Option<&Object> {
        self.bodies.get(&id).map(|body| &**body)
    }

    /// The graph of it: every object, as a table lists them.
    fn graph(&self) -> Graph {
        let mut graph = Graph::new(self.catalog, Kind::Table);
        let mut refs = Vec::new();
        for (id, object) in &self.bodies {
            refs.clear();
            self.references(*id, object, &mut refs);
            let late = self.font_files.contains(id) || image(object);
            graph.object(*id, refs.iter().copied()).late(late);
        }
        self.pages.iter().for_each(|page| graph.page(page.clone()));
        graph.first_page(self.first_page);
        self.tree_nodes.iter().for_each(|id| graph.tree_node(*id));
        self.open.iter().for_each(|id| graph.open(*id));
        for (group, roots) in &self.groups {
            graph.group(*group, roots.iter().copied());
        }
        graph.outlines_first(self.outlines_first);
        graph
    }

    /// What a walk goes into from object `id`, in the order it does.
    fn references(&self, id: ObjectId, object: &Object, refs: &mut Vec<ObjectId>) {
        if self.page_ids.contains(&id) {
            if let Some(dict) = dict_of(object) {
                page_references(dict, refs);
                return;
            }
        }
        if self.outline.contains(&id) {
            if let Some(dict) = dict_of(object) {
                let links: [&[u8]; 5] = [b"First", b"Last", b"Next", b"Prev", b"Parent"];
                for (key, value) in dict {
                    if !links.contains(&&key[..]) {
                        references(value, refs);
                    }
                }
                return;
            }
        }
        if self.names == Some(id) {
            if let Some(dests) = dict_of(object).and_then(|names| names.get(b"Dests").ok()) {
                references(dests, refs);
                return;
            }
        }
        match object {
            Object::Stream(stream) => stream_references(stream, refs),
            object => references(object, refs),
        }
    }
}

/// What `object` refers to, wherever in it, in the order written.
fn references(object: &Object, refs: &mut Vec<ObjectId>) {
    let mut stack = vec![object];
    while let Some(object) = stack.pop() {
        match object {
            Object::Reference(id) => refs.push((*id).into()),
            Object::Array(items) => stack.extend(items.iter().rev()),
            Object::Dictionary(dict) => stack.extend(dict.iter().rev().map(|(_, value)| value)),
            Object::Stream(stream) => {
                stack.extend(stream.dict.iter().rev().map(|(_, value)| value));
            }
            _ => {}
        }
    }
}

/// What a stream's dictionary refers to, but its `/Length`, which is written
/// direct, so an object only that names is one nothing in the file names.
fn stream_references(stream: &Stream, refs: &mut Vec<ObjectId>) {
    for (key, value) in &stream.dict {
        if key != b"Length" {
            references(value, refs);
        }
    }
}

/// What the trailer's entries reach.
fn reached(doc: &Document) -> HashSet<ObjectId> {
    let mut stack = Vec::new();
    for (key, value) in &doc.trailer {
        let root = key == b"Root";
        if root || !SECTION_KEYS.contains(&&key[..]) {
            references(value, &mut stack);
        }
    }
    let mut seen: HashSet<ObjectId> = stack.iter().copied().collect();
    let mut refs = Vec::new();
    while let Some(id) = stack.pop() {
        let Some(object) = doc.objects.get(&<(u32, u16)>::from(id)) else {
            continue;
        };
        refs.clear();
        match object {
            Object::Stream(stream) => stream_references(stream, &mut refs),
            object => references(object, &mut refs),
        }
        for &id in &refs {
            if seen.insert(id) {
                stack.push(id);
            }
        }
    }
    seen
}

/// Whether it's an object stream, a cross-reference stream or a linearization
/// dictionary: what's in the first is written anew, and the others are the
/// file's that was.
fn left_out(object: &Object) -> bool {
    match object {
        Object::Stream(stream) => stream.dict.has_type(b"ObjStm") || stream.dict.has_type(b"XRef"),
        Object::Dictionary(dict) => dict.has(b"Linearized"),
        _ => false,
    }
}

/// Whether `object` has no body to write, a reference to it being written
/// `null`: a null, what a file leaves out, and a reference whose chain ends at
/// one of those, at no object, or back where it's been, each of which a
/// reference to reads as a reference to none (7.3.10).
fn no_body(doc: &Document, object: &Object) -> bool {
    let none = |object: &Object| matches!(object, Object::Null) || left_out(object);
    if none(object) {
        return true;
    }
    let mut seen = HashSet::new();
    let mut at = object;
    while let Object::Reference(id) = at {
        if !seen.insert(*id) {
            return true;
        }
        match doc.objects.get(id) {
            Some(next) if !none(next) => at = next,
            _ => return true,
        }
    }
    false
}

/// `object`'s dictionary, or its stream's.
fn dict_of(object: &Object) -> Option<&Dictionary> {
    match object {
        Object::Dictionary(dict) => Some(dict),
        Object::Stream(stream) => Some(&stream.dict),
        _ => None,
    }
}

/// Object `id`'s dictionary, or its stream's, through the references it is.
fn dict_at(doc: &Document, id: ObjectId) -> Option<&Dictionary> {
    let object = doc.objects.get(&<(u32, u16)>::from(id))?;
    dict_of(doc.dereference(object).ok()?.1)
}

/// `object`, where it's a reference.
fn reference(object: Option<&Object>) -> Option<ObjectId> {
    object
        .and_then(|object| object.as_reference().ok())
        .map(Into::into)
}

/// What `value` names as what a walk starts from: itself where it's a
/// reference, and what it refers to where it's written in place.
fn roots(value: &Object) -> Vec<ObjectId> {
    let mut refs = Vec::new();
    references(value, &mut refs);
    refs
}

/// The elements of `value`, an array or a reference to one.
fn array_of<'d>(doc: &'d Document, value: &'d Object) -> &'d [Object] {
    match doc.dereference(value) {
        Ok((_, Object::Array(items))) => items,
        _ => &[],
    }
}

/// Whether `object` is an image XObject, as its dictionary says.
fn image(object: &Object) -> bool {
    matches!(object, Object::Stream(stream)
        if matches!(stream.dict.get(b"Subtype"), Ok(Object::Name(subtype)) if subtype == b"Image"))
}

/// What's named as a font file: by a font descriptor, which is an object of its
/// own, or written in place in its font.
fn font_files(bodies: &Bodies<'_>) -> HashSet<ObjectId> {
    let mut files = HashSet::new();
    for body in bodies.values() {
        let Object::Dictionary(dict) = &**body else {
            continue;
        };
        let inner = dict.get(b"FontDescriptor").ok();
        let inner = inner.and_then(|inner| inner.as_dict().ok());
        for holder in std::iter::once(dict).chain(inner) {
            for key in [&b"FontFile"[..], b"FontFile2", b"FontFile3"] {
                files.extend(reference(holder.get(key).ok()));
            }
        }
    }
    files
}

/// An attribute a page inherited: as written, the object that holds it, none
/// where that's written in place, and whether a null nearer the page goes
/// before it.
#[derive(Clone, Copy)]
struct Held<'d> {
    written: &'d Object,
    holder: Option<ObjectId>,
    nulled: bool,
}

type Attributes<'d> = [Option<Held<'d>>; 4];

/// A page of the tree.
struct Listed<'d> {
    id: ObjectId,
    /// What it holds itself, each of [`INHERITED`].
    own: [bool; 4],
    /// What's above it gives it, each of [`INHERITED`].
    up: Attributes<'d>,
    /// The first numbered node up its `/Parent`s.
    parent: Option<ObjectId>,
}

/// A node met going up from a page.
struct Above<'d> {
    /// What it gives the pages under it, each of [`INHERITED`].
    attributes: Attributes<'d>,
    /// The next numbered node up its `/Parent`s.
    parent: Option<ObjectId>,
    /// How many nodes its `/Parent` chain has, it among them.
    depth: usize,
}

/// A page tree as read.
struct Tree<'d> {
    pages: Vec<Listed<'d>>,
    above: HashMap<ObjectId, Above<'d>>,
    /// The first page that reads otherwise going down `/Kids` than up
    /// `/Parent`.
    apart: Option<PageTree>,
}

/// `written`, through the references it is, with the last it followed: none
/// where one names no object. Past [`HOPS`], where one going round comes to as
/// well, it doesn't read: a tree that takes that for nothing lists what a
/// reader doesn't.
fn read<'d>(
    doc: &'d Document,
    written: &'d Object,
) -> Result<Option<(Option<ObjectId>, &'d Object)>, PageTree> {
    let (mut last, mut at, mut followed) = (None, written, 0);
    while let Object::Reference(id) = at {
        if followed == HOPS {
            return Err(PageTree::Unread { id: (*id).into() });
        }
        followed += 1;
        last = Some(ObjectId::from(*id));
        match doc.objects.get(id) {
            Some(object) => at = object,
            None => return Ok(None),
        }
    }
    Ok(Some((last, at)))
}

/// What `dict`'s `key` holds, where it's there and reads as something: a null,
/// or a reference to nothing, is absent (7.3.10).
fn found<'d>(
    doc: &'d Document,
    dict: &'d Dictionary,
    key: &[u8],
) -> Result<Option<&'d Object>, PageTree> {
    let Ok(written) = dict.get(key) else {
        return Ok(None);
    };
    Ok(match read(doc, written)? {
        None | Some((_, Object::Null)) => None,
        Some(_) => Some(written),
    })
}

/// `attributes`, past the node or page `dict`, numbered `holder`: what it holds
/// takes the place of what's above, and a null goes before it.
fn through<'d>(
    doc: &'d Document,
    dict: &'d Dictionary,
    holder: Option<ObjectId>,
    attributes: &mut Attributes<'d>,
) -> Result<(), PageTree> {
    for (slot, key) in attributes.iter_mut().zip(INHERITED) {
        let key = key.as_bytes();
        if let Some(written) = found(doc, dict, key)? {
            *slot = Some(Held {
                written,
                holder,
                nulled: false,
            });
        } else if let (Some(held), true) = (slot, is_null(doc, dict, key)?) {
            held.nulled = true;
        }
    }
    Ok(())
}

/// Whether `dict`'s `key` reads as the null object, not as nothing.
fn is_null(doc: &Document, dict: &Dictionary, key: &[u8]) -> Result<bool, PageTree> {
    let Ok(written) = dict.get(key) else {
        return Ok(false);
    };
    Ok(matches!(read(doc, written)?, Some((_, Object::Null))))
}

/// The tree's pages by ISO 32000-2's rules, each a page of its own, numbered,
/// and what each inherits; which lopdf's own list of pages has to be, each kid
/// what PDFium's rules read it as, and what each inherits what going down
/// `/Kids` from the root reads it as. Nothing the walk reads by may fail to
/// read, as it's a tree of other pages to a reader that stops there.
fn page_tree<'d>(
    doc: &'d Document,
    catalog: &'d Dictionary,
    limits: &Limits,
) -> Result<Tree<'d>, PageTree> {
    let most = limits.pages.max(Limits::DEFAULT.pages);
    let mut table = Vec::new();
    let mut seen = HashSet::new();
    let mut above = HashMap::new();
    let written = catalog.get(b"Pages").ok();
    let root = written.map(|root| read(doc, root)).transpose()?.flatten();
    let root = root.and_then(|(_, root)| root.as_dict().ok());
    // A root that says it's a page is the one page by ISO's rules, and lopdf's
    // list goes under it for its kids, or has none.
    if let Some(root) = root {
        if !is_node(doc, root)? {
            return Err(PageTree::Listed { page: 0 });
        }
    }
    let root_id = written
        .and_then(|root| root.as_reference().ok())
        .map(ObjectId::from);
    seen.extend(root_id);
    // What going down gives what's under each.
    let mut held = [None; 4];
    if let Some(root) = root {
        through(doc, root, root_id, &mut held)?;
    }
    let mut stack = vec![(kids_of(doc, root)?.iter(), held)];
    let (mut nodes, mut looked_at, mut apart) = (0, 0, None);
    while let Some((kids, held)) = stack.last_mut() {
        let held = *held;
        let Some(kid) = kids.next() else {
            stack.pop();
            continue;
        };
        looked_at += 1;
        if looked_at > most.saturating_mul(KIDS_A_PAGE) {
            return Err(PageTree::TooMany);
        }
        let page = table.len();
        let id = match kid {
            Object::Reference(id) => *id,
            Object::Null => return Err(PageTree::NoPage { page }),
            _ => return Err(PageTree::Direct { page }),
        };
        let dict = match read(doc, kid)? {
            Some((_, Object::Dictionary(dict))) => dict,
            _ => return Err(PageTree::NoPage { page }),
        };
        // PDFium's rules go by /Kids alone and through one reference, so past
        // either it finds other pages than the walk here does.
        let chain = matches!(doc.objects.get(&id), Some(Object::Reference(_)));
        let id = ObjectId::from(id);
        let node = is_node(doc, dict)?;
        if chain || node != dict.has(b"Kids") {
            return Err(PageTree::Rules { page });
        }
        if !seen.insert(id) {
            return Err(PageTree::Repeated { page });
        }
        if node {
            nodes += 1;
            if nodes > most {
                return Err(PageTree::TooMany);
            }
            if stack.len() >= DEPTH {
                return Err(PageTree::TooDeep);
            }
            let mut under = held;
            through(doc, dict, Some(id), &mut under)?;
            stack.push((kids_of(doc, Some(dict))?.iter(), under));
            continue;
        }
        if table.len() >= most {
            return Err(PageTree::TooMany);
        }
        let (up, parent) = match dict.get(b"Parent") {
            Ok(parent) => inherited(doc, parent, &mut above, page)?,
            Err(_) => ([None; 4], None),
        };
        // What's its own takes it out of what it inherits; a null of its own
        // goes before what does.
        let mut inherits = up;
        let mut own = [false; 4];
        for (slot, key) in own.iter_mut().zip(INHERITED) {
            *slot = found(doc, dict, key.as_bytes())?.is_some();
        }
        through(doc, dict, None, &mut inherits)?;
        values_read(doc, dict, &inherits)?;
        let mut going_down = held;
        through(doc, dict, None, &mut going_down)?;
        for (at, attribute) in INHERITED.into_iter().enumerate() {
            if apart.is_none() && !read_alike(doc, inherits[at], going_down[at])? {
                apart = Some(PageTree::Kids { page, attribute });
            }
        }
        table.push(Listed {
            id,
            own,
            up: inherits,
            parent,
        });
    }
    // What lopdf's user calls the pages, or else the file's aren't what they
    // think.
    let mut lopdf = doc.page_iter();
    for (page, listed) in table.iter().enumerate() {
        if lopdf.next() != Some(listed.id.into()) {
            return Err(PageTree::Listed { page });
        }
    }
    if lopdf.next().is_some() {
        return Err(PageTree::Listed { page: table.len() });
    }
    Ok(Tree {
        pages: table,
        above,
        apart,
    })
}

/// Whether what a page holds or inherits, each as what a reader goes on to read
/// of it, reads: a box's corners, and its `/UserUnit`.
fn values_read(
    doc: &Document,
    page: &Dictionary,
    inherits: &Attributes<'_>,
) -> Result<(), PageTree> {
    found(doc, page, b"UserUnit")?;
    for held in [inherits[1], inherits[2]].into_iter().flatten() {
        if let Some((_, Object::Array(corners))) = read(doc, held.written)? {
            if corners.len() == 4 {
                for corner in corners {
                    read(doc, corner)?;
                }
            }
        }
    }
    Ok(())
}

/// Whether two ways up or down a tree read an attribute as the same value.
fn read_alike(doc: &Document, a: Option<Held<'_>>, b: Option<Held<'_>>) -> Result<bool, PageTree> {
    let value = |held: Option<Held<'_>>| -> Result<Option<Object>, PageTree> {
        Ok(match held {
            Some(held) => read(doc, held.written)?.map(|(_, value)| value.clone()),
            None => None,
        })
    };
    match (a, b) {
        (Some(a), Some(b)) if std::ptr::eq(a.written, b.written) => Ok(true),
        (None, None) => Ok(true),
        _ => Ok(value(a)? == value(b)?),
    }
}

/// Whether a kid is a node of the tree, or a page: by its `/Type`, and
/// otherwise by whether it holds `/Kids`.
fn is_node(doc: &Document, dict: &Dictionary) -> Result<bool, PageTree> {
    let kind = dict.get(b"Type").ok();
    let kind = kind.map(|kind| read(doc, kind)).transpose()?.flatten();
    Ok(match kind {
        Some((_, Object::Name(kind))) if kind == b"Pages" => true,
        Some((_, Object::Name(kind))) if kind == b"Page" => false,
        _ => found(doc, dict, b"Kids")?.is_some(),
    })
}

/// A node's kids: none where its `/Kids` isn't an array.
fn kids_of<'d>(doc: &'d Document, node: Option<&'d Dictionary>) -> Result<&'d [Object], PageTree> {
    let Some(kids) = node.and_then(|node| node.get(b"Kids").ok()) else {
        return Ok(&[]);
    };
    Ok(match read(doc, kids)? {
        Some((_, Object::Array(items))) => items,
        _ => &[],
    })
}

/// What the node `written` names gives the pages under it, and the first
/// numbered node from it: up its `/Parent` chain to a node already met and back
/// down. Refused past [`DEPTH`], where one that goes round comes to as well, of
/// `page`.
fn inherited<'d>(
    doc: &'d Document,
    written: &'d Object,
    above: &mut HashMap<ObjectId, Above<'d>>,
    page: usize,
) -> Result<(Attributes<'d>, Option<ObjectId>), PageTree> {
    let mut chain = Vec::new();
    let mut at = Some(written);
    let mut attributes = [None; 4];
    let mut parent = None;
    let mut depth = 0;
    while let Some(written) = at {
        let Some((id, object)) = read(doc, written)? else {
            break;
        };
        if let Some(id) = id {
            if let Some(met) = above.get(&id) {
                attributes = met.attributes;
                parent = Some(id);
                depth = met.depth;
                break;
            }
        }
        if chain.len() >= DEPTH {
            return Err(PageTree::Parents { page });
        }
        let Some(dict) = dict_of(object) else {
            break;
        };
        chain.push((id, dict));
        at = dict.get(b"Parent").ok();
    }
    for (id, dict) in chain.into_iter().rev() {
        depth += 1;
        if depth > DEPTH {
            return Err(PageTree::Parents { page });
        }
        through(doc, dict, id, &mut attributes)?;
        if let Some(id) = id {
            above.insert(
                id,
                Above {
                    attributes,
                    parent,
                    depth,
                },
            );
            parent = Some(id);
        }
    }
    Ok((attributes, parent))
}

/// Each page lacking an attribute it inherits given it as written, in a copy:
/// the rest stay as they are.
fn pushed_down(bodies: &mut Bodies<'_>, table: &[Listed<'_>]) -> Result<(), Error> {
    for (page, listed) in table.iter().enumerate() {
        let Some(body) = bodies.get_mut(&listed.id) else {
            continue;
        };
        for (at, key) in INHERITED.into_iter().enumerate() {
            let Some(held) = listed.up[at].filter(|_| !listed.own[at]) else {
                continue;
            };
            if held.nulled {
                let attribute = key;
                return Err(Error::PageTree(PageTree::Attribute { page, attribute }));
            }
            if held.holder.is_none() {
                return Err(Error::PageTree(PageTree::DirectNode { page }));
            }
            if let Object::Dictionary(dict) = body.to_mut() {
                dict.set(key, held.written.clone());
            }
        }
    }
    Ok(())
}

/// Every node of the page tree, from its root by `/Kids` and from each page by
/// `/Parent`, each that holds an inheritable attribute given a copy without it.
fn nodes_taken_off(
    doc: &Document,
    catalog: &Dictionary,
    tree: &Tree<'_>,
    page_ids: &HashSet<ObjectId>,
    bodies: &mut Bodies<'_>,
) -> Vec<ObjectId> {
    let mut nodes = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<&Object> = catalog.get(b"Pages").into_iter().collect();
    while let Some(node) = stack.pop() {
        let Ok((Some(id), object)) = doc.dereference(node) else {
            continue;
        };
        let id = ObjectId::from(id);
        let Some(dict) = dict_of(object).filter(|dict| dict.has(b"Kids")) else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        if !page_ids.contains(&id) {
            nodes.push(id);
        }
        let kids = dict
            .get(b"Kids")
            .map_or(&[][..], |kids| array_of(doc, kids));
        stack.extend(kids.iter().rev());
    }
    // What a page says is its parent is a node only if it holds kids, or says
    // so: a broken file's /Parent can name its /Resources.
    let node = |id: ObjectId| {
        dict_at(doc, id).is_some_and(|dict| dict.has(b"Kids") || dict.has_type(b"Pages"))
    };
    // A node met by /Kids can still have one above it that only its /Parent
    // names.
    let mut climbed = HashSet::new();
    for page in &tree.pages {
        let mut at = page.parent;
        while let Some(id) = at {
            if !climbed.insert(id) {
                break;
            }
            if seen.insert(id) && !page_ids.contains(&id) && node(id) {
                nodes.push(id);
            }
            at = tree.above.get(&id).and_then(|above| above.parent);
        }
    }
    for id in &nodes {
        let Some(body) = bodies.get_mut(id) else {
            continue;
        };
        let holds =
            dict_of(body).is_some_and(|dict| INHERITED.iter().any(|key| dict.has(key.as_bytes())));
        if holds {
            let dict = match body.to_mut() {
                Object::Dictionary(dict) => dict,
                Object::Stream(stream) => &mut stream.dict,
                _ => continue,
            };
            // Not lopdf's `remove`, which moves the last key into its place.
            let keys = dict.as_hashmap_mut();
            for key in INHERITED {
                keys.shift_remove(key.as_bytes());
            }
        }
    }
    nodes
}

/// Each of `strays` that says it's a page, given in a copy what it inherits
/// from one of `nodes`, which lose it: an annotation's `/P`, say, can still
/// name it, and nothing else would be left to give it what it had. What it
/// inherits from anything else it still does.
fn strays_pushed_down(
    doc: &Document,
    strays: &[ObjectId],
    nodes: &HashSet<ObjectId>,
    bodies: &mut Bodies<'_>,
) {
    let reads = |dict: &'_ Dictionary, key: &[u8]| {
        let read = dict.get(key).map(|written| doc.dereference(written));
        matches!(read, Ok(Ok((_, object))) if !matches!(object, Object::Null))
    };
    // One a page's /Parent makes a node is taken off with the rest.
    for id in strays.iter().filter(|id| !nodes.contains(id)) {
        let Some(body) = bodies.get_mut(id) else {
            continue;
        };
        let Some(dict) = dict_of(body).filter(|dict| dict.has_type(b"Page")) else {
            continue;
        };
        let mut pushed = Vec::new();
        for key in INHERITED {
            if reads(dict, key.as_bytes()) {
                continue;
            }
            let mut parent = dict.get(b"Parent").ok();
            for _ in 0..DEPTH {
                let read = parent.and_then(|parent| doc.dereference(parent).ok());
                let Some((holder, up)) = read else { break };
                let Some(up) = dict_of(up) else { break };
                if reads(up, key.as_bytes()) {
                    if let (Some(true), Ok(written)) = (
                        holder.map(|holder| nodes.contains(&ObjectId::from(holder))),
                        up.get(key.as_bytes()),
                    ) {
                        pushed.push((key, written.clone()));
                    }
                    break;
                }
                parent = up.get(b"Parent").ok();
            }
        }
        if !pushed.is_empty() {
            let dict = match body.to_mut() {
                Object::Dictionary(dict) => dict,
                Object::Stream(stream) => &mut stream.dict,
                _ => continue,
            };
            for (key, written) in pushed {
                dict.set(key, written);
            }
        }
    }
}

/// What a page says of itself: its content streams and its thumbnail, what its
/// `/Contents` names read through.
fn page_said(doc: &Document, id: ObjectId, dict: Option<&Dictionary>) -> Page {
    let mut page = Page::new(id);
    let Some(dict) = dict else {
        return page;
    };
    page.thumb = reference(dict.get(b"Thumb").ok());
    if let Ok(contents) = dict.get(b"Contents") {
        match doc.dereference(contents) {
            Ok((_, Object::Array(streams))) => {
                page.contents = streams.iter().filter_map(|s| reference(Some(s))).collect();
            }
            Ok((Some(stream), _)) => page.contents.push(stream.into()),
            _ => {}
        }
    }
    page
}

/// What a page refers to, in the order F.3.7 recommends: annotations, beads,
/// resources, what else it names, its content; not its parent, nor its
/// thumbnail, which F.3.10 has in part 9.
fn page_references(dict: &Dictionary, refs: &mut Vec<ObjectId>) {
    let mut of = |key: &[u8]| {
        if let Ok(value) = dict.get(key) {
            references(value, refs);
        }
    };
    of(b"Annots");
    of(b"B");
    of(b"Resources");
    let placed: [&[u8]; 6] = [
        b"Annots",
        b"B",
        b"Resources",
        b"Contents",
        b"Parent",
        b"Thumb",
    ];
    for (key, value) in dict {
        if !placed.contains(&&key[..]) {
            references(value, refs);
        }
    }
    if let Ok(contents) = dict.get(b"Contents") {
        references(contents, refs);
    }
}

/// The place of the page that opens first: `first`'s, or where `/OpenAction`
/// opens, if that's a page of the tree, or 0.
fn first_page(doc: &Document, catalog: &Dictionary, table: &[Listed<'_>], first: FirstPage) -> u32 {
    let action = match first {
        FirstPage::Index(index) => return index,
        FirstPage::OpenAction => catalog.get(b"OpenAction").ok(),
    };
    let Some(page) = action.and_then(|action| opens_at(doc, catalog, action)) else {
        return 0;
    };
    let place = table.iter().position(|listed| listed.id == page);
    place.map_or(0, |place| place as u32)
}

/// The page object an `/OpenAction` opens at: a destination, or a `/GoTo`
/// action's.
fn opens_at(doc: &Document, catalog: &Dictionary, action: &Object) -> Option<ObjectId> {
    let (_, action) = doc.dereference(action).ok()?;
    let destination = match dict_of(action) {
        Some(dict) => {
            let goes = matches!(dict.get(b"S"), Ok(Object::Name(kind)) if kind == b"GoTo");
            goes.then(|| dict.get(b"D").ok()).flatten()?
        }
        None => action,
    };
    destination_page(doc, catalog, destination)
}

/// The page object a destination names: an array's first element, by way of a
/// name (the catalog's `/Dests`) or a string (the name dictionary's `/Dests`)
/// where it's named (12.3.2).
fn destination_page(
    doc: &Document,
    catalog: &Dictionary,
    destination: &Object,
) -> Option<ObjectId> {
    let (_, destination) = doc.dereference(destination).ok()?;
    let target = match destination {
        Object::Array(array) => return reference(array.first()),
        Object::Name(name) => {
            let dests = catalog.get(b"Dests").ok()?;
            let dests = dict_of(doc.dereference(dests).ok()?.1)?;
            dests.get(name).ok()?
        }
        Object::String(text, _) => {
            let names = catalog.get(b"Names").ok()?;
            let names = dict_of(doc.dereference(names).ok()?.1)?;
            let entries = name_tree_entries(doc, names.get(b"Dests").ok()?);
            entries.into_iter().find(|(key, _)| *key == text)?.1
        }
        _ => return None,
    };
    let (_, target) = doc.dereference(target).ok()?;
    let array = match dict_of(target) {
        // A named destination may be a dictionary holding it (12.3.2.4).
        Some(dict) => doc.dereference(dict.get(b"D").ok()?).ok()?.1,
        None => target,
    };
    reference(array.as_array().ok()?.first())
}

/// The entries of the name tree at `root`, in the order the tree is written, as
/// far as it reads: each node's own entries, then its kids, each node once
/// (7.9.6).
fn name_tree_entries<'d>(doc: &'d Document, root: &'d Object) -> Vec<(&'d [u8], &'d Object)> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let Ok((id, object)) = doc.dereference(node) else {
            continue;
        };
        if id.is_some_and(|id| !seen.insert(id)) {
            continue;
        }
        let Some(dict) = dict_of(object) else {
            continue;
        };
        if let Ok(names) = dict.get(b"Names") {
            for pair in array_of(doc, names).chunks_exact(2) {
                let key = doc.dereference(&pair[0]).ok();
                if let Some((_, Object::String(key, _))) = key {
                    entries.push((&key[..], &pair[1]));
                }
            }
        }
        let kids = dict
            .get(b"Kids")
            .map_or(&[][..], |kids| array_of(doc, kids));
        stack.extend(kids.iter().rev());
    }
    entries
}

/// The document parts' nodes (14.12): the `/DPartRoot` dictionary and each node
/// under it, so a page's `/DPart` stops there (F.3.7).
fn document_parts(doc: &Document, catalog: &Dictionary, nodes: &mut Vec<ObjectId>) {
    let Ok(root) = catalog.get(b"DPartRoot") else {
        return;
    };
    let mut seen = HashSet::new();
    nodes.extend(reference(Some(root)));
    let root = doc
        .dereference(root)
        .ok()
        .and_then(|(_, root)| dict_of(root));
    let first = root.and_then(|root| root.get(b"DPartRootNode").ok());
    let mut stack: Vec<&Object> = first.into_iter().collect();
    while let Some(node) = stack.pop() {
        if let Some(id) = reference(Some(node)) {
            if !seen.insert(id) {
                continue;
            }
            nodes.push(id);
        }
        let dict = doc
            .dereference(node)
            .ok()
            .and_then(|(_, node)| dict_of(node));
        if let Some(parts) = dict.and_then(|dict| dict.get(b"DParts").ok()) {
            for run in array_of(doc, parts) {
                stack.extend(array_of(doc, run).iter().rev());
            }
        }
    }
}

/// The outline's objects in the order F.3.10 has them: as a viewer shows it,
/// then what's under a closed item, each in the order it would show if all were
/// open. What only a broken file's `/Last` or `/Prev` reaches comes after
/// what's beside it.
fn outline_order(doc: &Document, outlines: &Object) -> Vec<ObjectId> {
    let (mut shown, mut hidden) = (Vec::new(), Vec::new());
    let mut seen = HashSet::new();
    let root = reference(Some(outlines));
    let link = |dict: &Dictionary, key: &[u8]| reference(dict.get(key).ok());
    let mut stack: Vec<(ObjectId, bool)> = Vec::new();
    stack.extend(root.map(|id| (id, false)));
    if root.is_none() {
        // Written in place, its items are the outline's objects.
        let items = dict_of(outlines).and_then(|dict| link(dict, b"First"));
        stack.extend(items.map(|id| (id, false)));
    }
    while let Some((id, under_closed)) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(dict) = dict_at(doc, id) else {
            continue;
        };
        match under_closed {
            true => hidden.push(id),
            false => shown.push(id),
        }
        let count = dict.get(b"Count").ok();
        let count = count.and_then(|count| doc.dereference(count).ok());
        let count = count.and_then(|(_, count)| count.as_i64().ok());
        let closed = Some(id) != root && count.is_some_and(|count| count < 0);
        for key in [&b"Prev"[..], b"Last", b"Next"] {
            stack.extend(link(dict, key).map(|id| (id, under_closed)));
        }
        stack.extend(link(dict, b"First").map(|id| (id, under_closed || closed)));
    }
    shown.append(&mut hidden);
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::lopdf::{StringFormat, dictionary};

    /// A plan of a catalog, 1 0, and a page, 2 0, that gives them numbers.
    fn small_plan() -> Plan {
        let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
        graph.object(ObjectId::new(1, 0), [ObjectId::new(2, 0)]);
        graph.object(ObjectId::new(2, 0), []);
        graph.page(Page::new(ObjectId::new(2, 0)));
        plan(&graph, &Limits::DEFAULT).unwrap()
    }

    fn write(object: &Object) -> Result<String, Error> {
        let mut out = Vec::new();
        written(&mut out, object, ObjectId::new(9, 0), &small_plan(), 0)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn each_kind_of_object_is_written() {
        assert_eq!(write(&Object::Null).unwrap(), "null");
        assert_eq!(write(&Object::Boolean(true)).unwrap(), "true");
        assert_eq!(write(&Object::Boolean(false)).unwrap(), "false");
        assert_eq!(write(&Object::Integer(-7)).unwrap(), "-7");
        let array = Object::Array(vec![Object::Integer(1), Object::Name(b"N".to_vec())]);
        assert_eq!(write(&array).unwrap(), "[1 /N]");
        let dict = Object::Dictionary(dictionary! { "A" => 1, "B" => "C" });
        assert_eq!(write(&dict).unwrap(), "<< /A 1 /B /C >>");
    }

    #[test]
    fn a_reference_is_in_the_numbers_written_or_null() {
        let plan = small_plan();
        let id = plan.number(ObjectId::new(1, 0)).unwrap();
        let wanted = format!("{id} R");
        assert_eq!(write(&Object::Reference((1, 0))).unwrap(), wanted);
        assert_eq!(write(&Object::Reference((1, 3))).unwrap(), "null");
        assert_eq!(write(&Object::Reference((5, 0))).unwrap(), "null");
    }

    #[test]
    fn a_string_is_hexadecimal_whatever_its_form() {
        for form in [StringFormat::Literal, StringFormat::Hexadecimal] {
            let string = Object::String(vec![0, b'(', 0xFF], form);
            assert_eq!(write(&string).unwrap(), "<0028FF>");
        }
    }

    #[test]
    fn a_name_has_each_byte_that_isn_t_regular_by_its_code() {
        let mut out = Vec::new();
        name(&mut out, b"A b#c/(d)\xE9\x7F!~");
        assert_eq!(out, b"/A#20b#23c#2F#28d#29#E9#7F!~");
    }

    #[test]
    fn a_real_has_a_point_and_is_finite() {
        let real = |value: f32| write(&Object::Real(value));
        assert_eq!(real(0.5).unwrap(), "0.5");
        assert_eq!(real(-3.25).unwrap(), "-3.25");
        // No exponent, and without a point it would read as an integer past
        // any.
        assert_eq!(real(2.0).unwrap(), "2.0");
        assert_eq!(
            real(1e20).unwrap(),
            "100000000000000000000.0",
            "as it prints"
        );
        assert_eq!(real(1e-40).unwrap(), "0.0");
        assert_eq!(real(-0.0).unwrap(), "0.0");
        for bad in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            assert!(matches!(
                real(bad),
                Err(Error::NonFinite {
                    id: ObjectId {
                        number: 9,
                        generation: 0
                    }
                })
            ));
        }
    }

    #[test]
    fn a_stream_inside_an_object_is_refused() {
        let stream = Stream::new(dictionary! {}, vec![1]);
        let array = Object::Array(vec![Object::Stream(stream)]);
        assert!(matches!(
            write(&array),
            Err(Error::NestedStream {
                id: ObjectId {
                    number: 9,
                    generation: 0
                }
            })
        ));
    }

    #[test]
    fn what_s_nested_too_far_is_refused_where_lopdf_s_reader_stops_well_before() {
        let nested = |depth: usize| {
            let mut object = Object::Integer(1);
            for _ in 0..depth {
                object = Object::Array(vec![object]);
            }
            object
        };
        assert!(write(&nested(NESTED)).is_ok());
        assert!(matches!(
            write(&nested(NESTED + 1)),
            Err(Error::TooDeep {
                id: ObjectId {
                    number: 9,
                    generation: 0
                }
            })
        ));
    }

    /// An object as an indirect object numbered 7.
    fn at_seven(object: &Object) -> Result<String, Error> {
        let mut out = Vec::new();
        indirect(&mut out, ObjectId::new(9, 0), 7, object, &small_plan())?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    #[test]
    fn a_stream_s_length_is_direct_at_its_place_or_last() {
        let mut stream = Stream::new(dictionary! { "A" => 1 }, b"abc".to_vec());
        stream.dict.set("Length", Object::Reference((1, 0)));
        stream.dict.set("B", 2);
        assert_eq!(
            at_seven(&Object::Stream(stream.clone())).unwrap(),
            "7 0 obj\n<< /A 1 /Length 3 /B 2 >>\nstream\nabc\nendstream\nendobj\n"
        );
        stream.dict.remove(b"Length");
        assert_eq!(
            at_seven(&Object::Stream(stream)).unwrap(),
            "7 0 obj\n<< /A 1 /B 2 /Length 3 >>\nstream\nabc\nendstream\nendobj\n"
        );
    }

    #[test]
    fn a_stream_lopdf_didn_t_read_is_refused() {
        // As its reader leaves one whose /Length names nothing.
        let dict = dictionary! { "Length" => Object::Reference((5, 0)) };
        let unread = Stream::with_position(dict, 10);
        assert!(matches!(
            at_seven(&Object::Stream(unread)),
            Err(Error::UnreadStream {
                id: ObjectId {
                    number: 9,
                    generation: 0
                }
            })
        ));
        // As it leaves one it did read, empty.
        let mut read = Stream::with_position(dictionary! {}, 10);
        read.set_content(Vec::new());
        assert!(at_seven(&Object::Stream(read)).is_ok());
        // As one's made by hand.
        let made = Stream::new(dictionary! {}, Vec::new());
        assert!(at_seven(&Object::Stream(made)).is_ok());
    }
}
