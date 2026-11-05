//! What a plan is told of a document: its bodies, inheritable attributes pushed
//! down, and for each what it refers to, what it is, and where it belongs.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use pdf_objects::object::{Dict, Object, ObjectId, Value};
use pdf_semantics::pages::{Attribute, Inheritable, Origin, Pages, pages};
use pdf_semantics::{Limits as TreeLimits, Rules, tree};
use pdf_structure::write::{self, Body, Encryptor};
use pdf_structure::{Document, Source as _};

use super::{Error, FirstPage, Options, PageTree};
use crate::plan::{Graph, Group, Kind, Page};

/// The attributes a page inherits, each with its key.
const INHERITED: [(Inheritable, &str); 4] = [
    (Inheritable::Resources, "Resources"),
    (Inheritable::MediaBox, "MediaBox"),
    (Inheritable::CropBox, "CropBox"),
    (Inheritable::Rotate, "Rotate"),
];

/// How far up a page's `/Parent`s are followed for what it inherits, as
/// pdf-semantics has it by default.
const DEPTH: usize = 1024;

/// The keys that name a font file (9.9).
const FONT_FILES: [&[u8]; 3] = [b"FontFile", b"FontFile2", b"FontFile3"];

/// A document ready to plan: the objects to write and what a [`Graph`] says of
/// them, which is made again without what a plan takes out of its object
/// streams.
pub(crate) struct Prepared<'d> {
    catalog: ObjectId,
    /// Each object to write, by id.
    bodies: Vec<(ObjectId, Body<'d>)>,
    /// The pages by id, each its place, with what they say of themselves.
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
    /// What's never in an object stream, as only the file tells: pages that are
    /// no page of the tree, a name dictionary's templates (7.5.7), and what an
    /// encryption dictionary refers to, left in the clear.
    kept_out: HashSet<ObjectId>,
    /// The encryption dictionary, where it's an object, by what the graph knows
    /// it as, and what its values refer to. It has no body: it's written from
    /// the encryptor.
    dictionary: Option<(ObjectId, Vec<ObjectId>)>,
}

impl<'d> Prepared<'d> {
    /// `doc` read and its page tree's attributes pushed down, as far as nothing
    /// refuses it, to be written under `encryptor`.
    pub(crate) fn of(
        doc: &'d Document,
        encryptor: Option<&Encryptor>,
        options: &Options<'_>,
    ) -> Result<Prepared<'d>, Error> {
        let trailer = doc.trailer.dict();
        // The object that's the catalog, through any references to it: a
        // section's /Root names it, and it's never packed (F.3.1).
        let root = trailer
            .get(b"Root")
            .filter(|root| root.as_reference().is_some());
        let found = root.and_then(|root| doc.dereference(root));
        let catalog_id = found.and_then(|(id, _)| id).ok_or(Error::NoCatalog)?;
        let catalog = doc.get_dict(catalog_id).ok_or(Error::NoCatalog)?;
        let mut limits = TreeLimits::DEFAULT;
        limits.pages = limits.pages.max(options.limits.pages);
        let table = pages_agreed(doc, catalog, &limits)?;

        let left = write::encryption_objects(doc, encryptor);
        let values: Vec<ObjectId> = encryptor.map_or_else(Vec::new, |encryptor| {
            let values = encryptor.dictionary().entries().iter();
            values.flat_map(|entry| entry.value.references()).collect()
        });
        let reached = reached(doc, &values);
        let mut bodies: Vec<(ObjectId, Body<'d>)> = write::bodies(doc)
            .filter(|(id, body)| {
                reached.contains(id) && !left.contains(id) && !no_body(doc, body.object(), &left)
            })
            .collect();
        let dictionary = match encryptor {
            Some(encryptor) if !encryptor.direct() => {
                let id = dictionary_id(encryptor, doc, &bodies)?;
                Some((id, values.clone()))
            }
            _ => None,
        };
        // What the dictionary refers to stays in the clear, and an object
        // stream is encrypted (7.6.2).
        let in_the_clear = encryptor.map_or_else(Vec::new, |encryptor| {
            let ids = bodies.iter().map(|(id, _)| *id);
            ids.filter(|&id| encryptor.dictionary_object(id)).collect()
        });

        let page_ids: HashSet<ObjectId> = table.iter().filter_map(|page| page.id).collect();
        pushed_down(&mut bodies, &table)?;
        let mut tree_nodes = nodes_taken_off(doc, catalog, &table, &page_ids, &mut bodies)?;
        kids_agree(doc, catalog, &table)?;
        // A page or a node the tree doesn't list is still no object of the page
        // that names it (F.3.7), and a page isn't packed (F.3.1).
        let strays = strays(&bodies, &page_ids);
        strays_pushed_down(doc, &strays, &tree_nodes, &mut bodies);
        let listed: HashSet<ObjectId> = tree_nodes.iter().copied().collect();
        tree_nodes.extend(strays.iter().copied().filter(|id| !listed.contains(id)));

        let pages_said: Vec<Page> = table
            .iter()
            .map(|page| {
                let id = page.id.expect("a page without a number is refused");
                let held = body_of(&bodies, id).map(Body::object);
                page_said(doc, id, held.and_then(dict_of))
            })
            .collect();

        let mut prepared = Prepared {
            catalog: catalog_id,
            bodies,
            pages: pages_said,
            page_ids,
            first_page: first_page(doc, catalog, &table, options.first_page),
            tree_nodes,
            open: Vec::new(),
            groups: Vec::new(),
            outlines_first: false,
            outline: HashSet::new(),
            names: None,
            font_files: HashSet::new(),
            kept_out: strays.into_iter().chain(in_the_clear).collect(),
            dictionary,
        };
        prepared.document_level(doc, catalog, &limits);
        // The trailer's /Encrypt and all its values (F.3.5).
        let named = prepared.dictionary.iter().map(|(id, _)| *id);
        let written = values.iter().copied();
        let written = written.filter(|&id| body_of(&prepared.bodies, id).is_some());
        let encryption: Vec<ObjectId> = named.chain(written).collect();
        prepared.open.extend(encryption);
        prepared.font_files = font_files(&prepared.bodies);
        Ok(prepared)
    }

    /// Part 4 and part 9's groups, and the document parts' nodes (F.3.5,
    /// F.3.10).
    fn document_level(&mut self, doc: &'d Document, catalog: &'d Dict, limits: &TreeLimits) {
        for key in [
            &b"ViewerPreferences"[..],
            b"PageMode",
            b"OpenAction",
            b"AcroForm",
        ] {
            self.open
                .extend(catalog.get(key).and_then(Object::as_reference));
        }
        // The threads and each thread's dictionary; beads are their pages', and
        // what a thread's /I names is a group's.
        let mut thread_info = Vec::new();
        if let Some(threads) = catalog.get(b"Threads") {
            self.open.extend(threads.as_reference());
            for thread in array_of(doc, threads) {
                self.open.extend(thread.as_reference());
                let dict = doc.dereference(thread).and_then(|(_, o)| dict_of(o));
                if let Some(info) = dict.and_then(|d| d.get(b"I")) {
                    thread_info.extend(roots(info));
                }
            }
            self.groups.push((Group::ThreadInfo, thread_info));
        }
        document_parts(doc, catalog, &mut self.tree_nodes);

        if let Some(outlines) = catalog.get(b"Outlines") {
            let order = outline_order(doc, outlines);
            self.outline = order.iter().copied().collect();
            self.groups.push((Group::Outlines, order));
        }
        self.outlines_first = (catalog.get(b"PageMode"))
            .and_then(|mode| doc.dereference(mode))
            .and_then(|(_, mode)| mode.as_name())
            .is_some_and(|mode| mode == b"UseOutlines");

        let names = catalog.get(b"Names");
        let name_dict = names.and_then(|names| doc.dereference(names));
        let name_dict = name_dict.and_then(|(_, names)| dict_of(names));
        let of_names = |key: &[u8]| name_dict.and_then(|names| names.get(key));
        let name_tree = of_names(b"Dests");
        if catalog.has(b"Dests") || name_tree.is_some() {
            let mut dests: Vec<ObjectId> =
                catalog.get(b"Dests").into_iter().flat_map(roots).collect();
            if name_tree.is_some() {
                self.names = names.and_then(Object::as_reference);
                dests.extend(self.names);
            }
            dests.extend(name_tree.into_iter().flat_map(roots));
            self.groups.push((Group::Dests, dests));
        }
        if let Some(info) = doc.trailer.dict().get(b"Info") {
            self.groups.push((Group::Info, roots(info)));
        }
        let form = catalog
            .get(b"AcroForm")
            .and_then(|form| doc.dereference(form));
        if let Some(form) = form.and_then(|(_, form)| dict_of(form)) {
            let fields = form.get(b"Fields").into_iter().flat_map(roots).collect();
            self.groups.push((Group::Form, fields));
        }
        if let Some(structure) = catalog.get(b"StructTreeRoot") {
            self.groups.push((Group::Structure, roots(structure)));
        }
        if let Some(renditions) = of_names(b"Renditions") {
            self.groups.push((Group::Renditions, roots(renditions)));
        }
        if let Some(files) = of_names(b"EmbeddedFiles") {
            let walk = tree::names(doc, files, limits, ());
            let files = walk
                .filter_map(Result::ok)
                .flat_map(|(_, file)| roots(file));
            self.groups.push((Group::EmbeddedFile, files.collect()));
        }
        // A page by another name is still a page object (7.5.7).
        for key in [&b"Pages"[..], b"Templates"] {
            if let Some(templates) = of_names(key) {
                let walk = tree::names(doc, templates, limits, ());
                let named = walk
                    .filter_map(Result::ok)
                    .flat_map(|(_, page)| roots(page));
                self.kept_out.extend(named);
            }
        }
    }

    /// Each object to write, by id.
    #[cfg(any(test, fuzzing))]
    pub(crate) fn bodies(&self) -> &[(ObjectId, Body<'d>)] {
        &self.bodies
    }

    /// The body of object `id`.
    pub(super) fn body(&self, id: ObjectId) -> Option<&Body<'d>> {
        body_of(&self.bodies, id)
    }

    /// What the graph knows the encryption dictionary as, where it's an object.
    pub(super) fn dictionary(&self) -> Option<ObjectId> {
        self.dictionary.as_ref().map(|(id, _)| *id)
    }

    /// The graph of it, listed by `kind`, what may be packed in object streams
    /// where `pack` says, but `alone`.
    pub(crate) fn graph(&self, kind: Kind, pack: bool, alone: &HashSet<ObjectId>) -> Graph {
        let mut graph = Graph::new(self.catalog, kind);
        let mut refs = Vec::new();
        for (id, body) in &self.bodies {
            let object = body.object();
            refs.clear();
            self.references(*id, object, &mut refs);
            let packable = pack
                && !alone.contains(id)
                && !self.kept_out.contains(id)
                && write::packable(ObjectId::new(id.number, 0), object);
            let late = self.font_files.contains(id) || image(object);
            (graph.object(*id, refs.iter().copied()))
                .packable(packable)
                .late(late);
        }
        if let Some((id, values)) = &self.dictionary {
            graph.object(*id, values.iter().copied());
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
                let others = dict.entries().iter().filter(|e| !links.contains(&&*e.key));
                refs.extend(others.flat_map(|entry| entry.value.references()));
                return;
            }
        }
        if self.names == Some(id) {
            if let Some(dests) = dict_of(object).and_then(|names| names.get(b"Dests")) {
                refs.extend(dests.references());
                return;
            }
        }
        if let Value::Stream(stream) = object.value() {
            let held = stream.dict.entries().iter();
            let entries = held.filter(|entry| entry.key != b"Length");
            refs.extend(entries.flat_map(|entry| entry.value.references()));
            return;
        }
        refs.extend(object.references());
    }
}

/// What the graph knows `encryptor`'s dictionary as: the number it has in the
/// file it was read from, and no object's here; or, for a new file's, one past
/// every number an object has or a reference names, so that none comes to name
/// it.
fn dictionary_id(
    encryptor: &Encryptor,
    doc: &Document,
    bodies: &[(ObjectId, Body<'_>)],
) -> Result<ObjectId, Error> {
    if let Some(at) = encryptor.at() {
        return match body_of(bodies, at) {
            Some(_) => Err(Error::Write(write::Error::EncryptAt(at.number))),
            None => Ok(at),
        };
    }
    let trailer = (doc.trailer.dict().entries().iter()).flat_map(|entry| entry.value.references());
    let named = bodies.iter().flat_map(|(id, body)| {
        let references = body.object().references();
        std::iter::once(*id).chain(references)
    });
    let last = named.chain(trailer).map(|id| id.number).max().unwrap_or(0);
    match last.checked_add(1) {
        Some(past) => Ok(ObjectId {
            number: past,
            generation: 0,
        }),
        None => Err(Error::Write(write::Error::Number(last))),
    }
}

/// What the trailer's entries and `values`, an encryption dictionary's, reach,
/// as [`Document::reachable`] finds it: but for a stream's `/Length`, which the
/// writer writes direct, so an object only that names is one nothing in the
/// file names.
fn reached(doc: &Document, values: &[ObjectId]) -> HashSet<ObjectId> {
    let trailer = write::trailer_entries(doc.trailer.dict(), &());
    let mut stack: Vec<ObjectId> = (trailer.entries().iter())
        .flat_map(|entry| entry.value.references())
        .chain(values.iter().copied())
        .collect();
    let mut seen: HashSet<ObjectId> = stack.iter().copied().collect();
    let mut see = |id: ObjectId, stack: &mut Vec<ObjectId>| {
        if seen.insert(id) {
            stack.push(id);
        }
    };
    while let Some(id) = stack.pop() {
        let Ok(object) = doc.get(id) else {
            continue;
        };
        match object.value() {
            Value::Stream(stream) => {
                let held = stream.dict.entries().iter();
                let entries = held.filter(|entry| entry.key != b"Length");
                for id in entries.flat_map(|entry| entry.value.references()) {
                    see(id, &mut stack);
                }
            }
            _ => object.references().for_each(|id| see(id, &mut stack)),
        }
    }
    seen
}

/// Whether `object` has no body to write, a reference to it being written
/// `null`: a null, what a file leaves out, what's among `left`, and a reference
/// whose chain ends at one of those, at no object, or back where it's been,
/// each of which a reference to reads as a reference to none (7.3.10).
fn no_body(doc: &Document, object: &Object, left: &HashSet<ObjectId>) -> bool {
    if object.is_null() || write::left_out(object) {
        return true;
    }
    let mut seen = HashSet::new();
    let mut at = object;
    while let Some(id) = at.as_reference() {
        if !seen.insert(id) || left.contains(&id) {
            return true;
        }
        match doc.get(id) {
            Ok(next) if !next.is_null() && !write::left_out(next) => at = next,
            _ => return true,
        }
    }
    false
}

/// The body of object `id`, of `bodies` by id.
fn body_of<'b, 'd>(bodies: &'b [(ObjectId, Body<'d>)], id: ObjectId) -> Option<&'b Body<'d>> {
    let at = bodies.binary_search_by_key(&id, |(id, _)| *id).ok()?;
    Some(&bodies[at].1)
}

/// `object`'s dictionary, or its stream's.
fn dict_of(object: &Object) -> Option<&Dict> {
    match object.value() {
        Value::Dict(dict) => Some(dict),
        Value::Stream(stream) => Some(&stream.dict),
        _ => None,
    }
}

/// `object`'s dictionary, or its stream's, to change.
fn dict_of_mut(object: &mut Object) -> Option<&mut Dict> {
    match object.value_mut() {
        Value::Dict(dict) => Some(dict),
        Value::Stream(stream) => Some(&mut stream.dict),
        _ => None,
    }
}

/// What `value` names as what a walk starts from: itself where it's a
/// reference, and what it refers to where it's written in place.
fn roots(value: &Object) -> Vec<ObjectId> {
    match value.as_reference() {
        Some(id) => vec![id],
        None => value.references().collect(),
    }
}

/// The elements of `value`, an array or a reference to one.
fn array_of<'d>(doc: &'d Document, value: &'d Object) -> &'d [Object] {
    (doc.dereference(value))
        .and_then(|(_, array)| array.as_array())
        .unwrap_or_default()
}

/// Whether `object` is an image XObject, as its dictionary says.
fn image(object: &Object) -> bool {
    match object.value() {
        Value::Stream(stream) => (stream.dict.get(b"Subtype"))
            .and_then(Object::as_name)
            .is_some_and(|subtype| subtype == b"Image"),
        _ => false,
    }
}

/// The objects that say they're a page or a node of the tree and aren't among
/// the pages.
fn strays(bodies: &[(ObjectId, Body<'_>)], pages: &HashSet<ObjectId>) -> Vec<ObjectId> {
    let tree = |dict: &Dict| dict.has_type(b"Page") || dict.has_type(b"Pages");
    (bodies.iter())
        .filter(|(id, body)| !pages.contains(id) && dict_of(body.object()).is_some_and(tree))
        .map(|(id, _)| *id)
        .collect()
}

/// Each of `strays` that says it's a page, given in a body written anew what it
/// inherits from one of `nodes`, which lose it: an annotation's `/P`, say, can
/// still name it, and nothing else would be left to give it what it had. What
/// it inherits from anything else it still does.
fn strays_pushed_down(
    doc: &Document,
    strays: &[ObjectId],
    nodes: &[ObjectId],
    bodies: &mut [(ObjectId, Body<'_>)],
) {
    let nodes: HashSet<ObjectId> = nodes.iter().copied().collect();
    let reads = |value: &Object| {
        let read = doc.dereference(value);
        read.is_some_and(|(_, value)| !value.is_null())
    };
    // One a page's /Parent makes a node is taken off with the rest.
    for id in strays.iter().filter(|id| !nodes.contains(id)) {
        let Ok(at) = bodies.binary_search_by_key(id, |(id, _)| *id) else {
            continue;
        };
        let object = bodies[at].1.object();
        let Some(dict) = dict_of(object).filter(|dict| dict.has_type(b"Page")) else {
            continue;
        };
        let mut pushed = Vec::new();
        for (_, key) in INHERITED {
            if dict.get(key.as_bytes()).is_some_and(reads) {
                continue;
            }
            let mut parent = dict.get(b"Parent");
            for _ in 0..DEPTH {
                let read = parent.and_then(|parent| doc.dereference(parent));
                let Some((holder, up)) = read else { break };
                let Some(up) = dict_of(up) else { break };
                if let Some(written) = up.get(key.as_bytes()).filter(|written| reads(written)) {
                    if holder.is_some_and(|holder| nodes.contains(&holder)) {
                        pushed.push((key, written.clone()));
                    }
                    break;
                }
                parent = up.get(b"Parent");
            }
        }
        if !pushed.is_empty() {
            let mut object = object.clone();
            if let Some(dict) = dict_of_mut(&mut object) {
                for (key, written) in pushed {
                    dict.set(key, written);
                }
            }
            bodies[at].1 = Body::New(Cow::Owned(object));
        }
    }
}

/// What's named as a font file: by a font descriptor, which is an object of its
/// own, or written in place in its font.
fn font_files(bodies: &[(ObjectId, Body<'_>)]) -> HashSet<ObjectId> {
    let mut files = HashSet::new();
    for (_, body) in bodies {
        let Value::Dict(dict) = body.object().value() else {
            continue;
        };
        let inner = dict.get(b"FontDescriptor").and_then(Object::as_dict);
        for holder in std::iter::once(dict).chain(inner) {
            for key in FONT_FILES {
                files.extend(holder.get(key).and_then(Object::as_reference));
            }
        }
    }
    files
}

/// The tree's pages, by ISO's rules, where PDFium's read the same: each a page
/// of its own, numbered, whose inheritable attributes both read alike.
fn pages_agreed<'d>(
    doc: &'d Document,
    catalog: &'d Dict,
    limits: &TreeLimits,
) -> Result<Pages<'d>, Error> {
    let iso = pages(doc, catalog, Rules::Iso, limits, &mut ()).map_err(Error::Pages)?;
    let pdfium = pages(doc, catalog, Rules::Pdfium, limits, &mut ()).map_err(Error::Pages)?;
    let refused = |why| Err(Error::PageTree(why));
    let mut seen = HashSet::with_capacity(iso.len());
    for (page, (a, b)) in iso.iter().zip(&pdfium).enumerate() {
        if a.id != b.id {
            return refused(PageTree::Rules { page });
        }
        if a.dict.is_none() || b.dict.is_none() {
            return refused(PageTree::NoPage { page });
        }
        let Some(id) = a.id else {
            return refused(PageTree::Direct { page });
        };
        if !seen.insert(id) {
            return refused(PageTree::Repeated { page });
        }
        for (attribute, key) in INHERITED {
            if !alike(a.attribute(attribute), b.attribute(attribute)) {
                return refused(PageTree::Attribute {
                    page,
                    attribute: key,
                });
            }
        }
    }
    if iso.len() != pdfium.len() {
        let page = iso.len().min(pdfium.len());
        return refused(PageTree::Rules { page });
    }
    Ok(iso)
}

/// Each page's inheritable attributes read going down `/Kids` from the root, as
/// some readers take them, held to what its `/Parent`s give it, as `table` has
/// them: where a page's `/Parent` isn't the node that lists it, the two can
/// differ, and pushing one value down would change the other reader's page.
fn kids_agree(doc: &Document, catalog: &Dict, table: &Pages<'_>) -> Result<(), Error> {
    let places: HashMap<ObjectId, usize> = (table.iter().enumerate())
        .filter_map(|(place, page)| Some((page.id?, place)))
        .collect();
    let mut seen = HashSet::new();
    let none: [Option<&Object>; 4] = [None; 4];
    let mut stack: Vec<(&Object, [Option<&Object>; 4])> = catalog
        .get(b"Pages")
        .map(|root| (root, none))
        .into_iter()
        .collect();
    while let Some((kid, above)) = stack.pop() {
        let Some((id, object)) = doc.dereference(kid) else {
            continue;
        };
        let Some(dict) = dict_of(object) else {
            continue;
        };
        if id.is_some_and(|id| !seen.insert(id)) {
            continue;
        }
        let mut held = above;
        for (slot, (_, key)) in held.iter_mut().zip(INHERITED) {
            let value = dict
                .get(key.as_bytes())
                .and_then(|value| doc.dereference(value));
            if let Some((_, value)) = value.filter(|(_, value)| !value.is_null()) {
                *slot = Some(value);
            }
        }
        match id.and_then(|id| places.get(&id)) {
            Some(&page) => {
                for (&down, (attribute, key)) in held.iter().zip(INHERITED) {
                    let up = table.get(page).and_then(|page| page.attribute(attribute));
                    let up = up.map(|up| up.value).filter(|up| !up.is_null());
                    let alike = match (down, up) {
                        (None, None) => true,
                        (Some(a), Some(b)) => same_value(a, b),
                        _ => false,
                    };
                    if !alike {
                        let attribute = key;
                        return Err(Error::PageTree(PageTree::Kids { page, attribute }));
                    }
                }
            }
            None => {
                let kids = dict.get(b"Kids").map(|kids| array_of(doc, kids));
                stack.extend(kids.unwrap_or_default().iter().rev().map(|kid| (kid, held)));
            }
        }
    }
    Ok(())
}

/// Whether two values are the same wherever each was written.
fn same_value(a: &Object, b: &Object) -> bool {
    if std::ptr::eq(a, b) {
        return true;
    }
    let (mut a, mut b) = (a.clone(), b.clone());
    a.unplace();
    b.unplace();
    a == b
}

/// Whether the two rules read an attribute alike. A null is none to a page by
/// either, PDFium's stopping its climb at it where ISO's goes on.
fn alike(a: Option<Attribute<'_>>, b: Option<Attribute<'_>>) -> bool {
    let there = |attribute: &Attribute<'_>| !attribute.value.is_null();
    match (a.filter(there), b.filter(there)) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::eq(a.value, b.value) || a.value == b.value,
        _ => false,
    }
}

/// Each page lacking an attribute it inherits given it as written, in a body
/// written anew: the rest stay as read, to be copied.
fn pushed_down(bodies: &mut [(ObjectId, Body<'_>)], table: &Pages<'_>) -> Result<(), Error> {
    for listed in table {
        let Some(id) = listed.id else { continue };
        let Ok(at) = bodies.binary_search_by_key(&id, |(id, _)| *id) else {
            continue;
        };
        let mut pushed: Option<Object> = None;
        for (attribute, key) in INHERITED {
            let Some(inherited) = listed.attribute(attribute) else {
                continue;
            };
            match inherited.origin {
                Origin::Ancestor(Some(_)) => {}
                Origin::Ancestor(None) => {
                    return Err(Error::PageTree(PageTree::DirectNode { attribute: key }));
                }
                _ => continue,
            }
            let object = pushed.get_or_insert_with(|| bodies[at].1.object().clone());
            if let Some(dict) = dict_of_mut(object) {
                dict.set(key, inherited.written.clone());
            }
        }
        if let Some(object) = pushed {
            bodies[at].1 = Body::New(Cow::Owned(object));
        }
    }
    Ok(())
}

/// Every node of the page tree, from its root by `/Kids` and from each page by
/// `/Parent`, each that holds an inheritable attribute given a body without it.
fn nodes_taken_off(
    doc: &Document,
    catalog: &Dict,
    table: &Pages<'_>,
    page_ids: &HashSet<ObjectId>,
    bodies: &mut [(ObjectId, Body<'_>)],
) -> Result<Vec<ObjectId>, Error> {
    let mut nodes = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<&Object> = catalog.get(b"Pages").into_iter().collect();
    while let Some(node) = stack.pop() {
        let Some((id, object)) = doc.dereference(node) else {
            continue;
        };
        let Some(dict) = dict_of(object) else {
            continue;
        };
        if !dict.has(b"Kids") {
            continue;
        }
        match id {
            Some(id) => {
                if !seen.insert(id) {
                    continue;
                }
                if !page_ids.contains(&id) {
                    nodes.push(id);
                }
            }
            None => {
                if let Some((_, key)) = INHERITED.iter().find(|(_, key)| dict.has(key.as_bytes())) {
                    return Err(Error::PageTree(PageTree::DirectNode { attribute: key }));
                }
            }
        }
        let kids = dict.get(b"Kids").map(|kids| array_of(doc, kids));
        stack.extend(kids.unwrap_or_default().iter().rev());
    }
    // What a page says is its parent is a node only if it holds kids, or says
    // so: a broken file's /Parent can name its /Resources.
    let node = |id: ObjectId| {
        doc.get_dict(id)
            .is_some_and(|dict| dict.has(b"Kids") || dict.has_type(b"Pages"))
    };
    // A node met by /Kids can still have one above it that only its /Parent
    // names.
    let mut climbed = HashSet::new();
    for page in table {
        for id in page.parents() {
            if !climbed.insert(id) {
                break;
            }
            if seen.insert(id) && !page_ids.contains(&id) && node(id) {
                nodes.push(id);
            }
        }
    }
    for id in &nodes {
        let Ok(at) = bodies.binary_search_by_key(id, |(id, _)| *id) else {
            continue;
        };
        let object = bodies[at].1.object();
        let holds = dict_of(object)
            .is_some_and(|dict| INHERITED.iter().any(|(_, key)| dict.has(key.as_bytes())));
        if holds {
            let mut object = object.clone();
            if let Some(dict) = dict_of_mut(&mut object) {
                for (_, key) in INHERITED {
                    dict.remove(key.as_bytes());
                }
            }
            bodies[at].1 = Body::New(Cow::Owned(object));
        }
    }
    Ok(nodes)
}

/// What a page says of itself: its content streams and its thumbnail. It's told
/// from the page's dictionary, what a page's `/Contents` names read through.
fn page_said(doc: &Document, id: ObjectId, dict: Option<&Dict>) -> Page {
    let mut page = Page::new(id);
    let Some(dict) = dict else {
        return page;
    };
    page.thumb = dict.get(b"Thumb").and_then(Object::as_reference);
    if let Some(contents) = dict.get(b"Contents") {
        let target = doc.dereference(contents);
        match target {
            Some((_, object)) if object.as_array().is_some() => {
                let streams = object.as_array().unwrap_or_default();
                page.contents = streams.iter().filter_map(Object::as_reference).collect();
            }
            Some((Some(stream), _)) => page.contents.push(stream),
            _ => {}
        }
    }
    page
}

/// What a page refers to, in the order F.3.7 recommends: annotations, beads,
/// resources, what else it names, its content; not its parent, nor its
/// thumbnail, which F.3.10 has in part 9.
fn page_references(dict: &Dict, refs: &mut Vec<ObjectId>) {
    let mut of = |key: &[u8]| {
        if let Some(value) = dict.get(key) {
            refs.extend(value.references());
        }
    };
    of(b"Annots");
    of(b"B");
    of(b"Resources");
    let placed: [&[u8]; 5] = [b"Annots", b"B", b"Resources", b"Contents", b"Parent"];
    for entry in dict.entries() {
        if !placed.contains(&&*entry.key) && entry.key != b"Thumb" {
            refs.extend(entry.value.references());
        }
    }
    if let Some(contents) = dict.get(b"Contents") {
        refs.extend(contents.references());
    }
}

/// The place of the page that opens first: `first`'s, or where `/OpenAction`
/// opens, if that's a page of the tree, or 0.
fn first_page(doc: &Document, catalog: &Dict, table: &Pages<'_>, first: FirstPage) -> u32 {
    let action = match first {
        FirstPage::Index(index) => return index,
        FirstPage::OpenAction => catalog.get(b"OpenAction"),
    };
    let Some(page) = action.and_then(|action| opens_at(doc, catalog, action)) else {
        return 0;
    };
    let place = table.iter().position(|listed| listed.id == Some(page));
    place.map_or(0, |place| place as u32)
}

/// The page object an `/OpenAction` opens at: a destination, or a `/GoTo`
/// action's.
fn opens_at(doc: &Document, catalog: &Dict, action: &Object) -> Option<ObjectId> {
    let (_, action) = doc.dereference(action)?;
    let destination = match dict_of(action) {
        Some(dict) => {
            let goes = dict.get(b"S").and_then(Object::as_name) == Some(&b"GoTo"[..]);
            goes.then(|| dict.get(b"D")).flatten()?
        }
        None => action,
    };
    destination_page(doc, catalog, destination)
}

/// The page object a destination names: an array's first element, by way of a
/// name (the catalog's `/Dests`) or a string (the name dictionary's `/Dests`)
/// where it's named (12.3.2).
fn destination_page(doc: &Document, catalog: &Dict, destination: &Object) -> Option<ObjectId> {
    let (_, destination) = doc.dereference(destination)?;
    let target = match destination.value() {
        Value::Array(array) => return array.first()?.as_reference(),
        Value::Name(name) => {
            let dests = catalog.get(b"Dests")?;
            let dests = dict_of(doc.dereference(dests)?.1)?;
            dests.get(name)?
        }
        Value::String(text, _) => {
            let wanted = text.as_bytes()?;
            let names = catalog.get(b"Names")?;
            let names = dict_of(doc.dereference(names)?.1)?;
            let walk = tree::names(doc, names.get(b"Dests")?, &TreeLimits::DEFAULT, ());
            let found = walk
                .filter_map(Result::ok)
                .find(|(key, _)| *key == wanted)?;
            found.1
        }
        _ => return None,
    };
    let (_, target) = doc.dereference(target)?;
    let array = match dict_of(target) {
        // A named destination may be a dictionary holding it (12.3.2.4).
        Some(dict) => doc.dereference(dict.get(b"D")?)?.1,
        None => target,
    };
    array.as_array()?.first()?.as_reference()
}

/// The document parts' nodes (14.12): the `/DPartRoot` dictionary and each node
/// under it, so a page's `/DPart` stops there (F.3.7).
fn document_parts(doc: &Document, catalog: &Dict, nodes: &mut Vec<ObjectId>) {
    let Some(root) = catalog.get(b"DPartRoot") else {
        return;
    };
    let mut seen = HashSet::new();
    nodes.extend(root.as_reference());
    let root = doc.dereference(root).and_then(|(_, root)| dict_of(root));
    let mut stack: Vec<&Object> = root
        .and_then(|root| root.get(b"DPartRootNode"))
        .into_iter()
        .collect();
    while let Some(node) = stack.pop() {
        if let Some(id) = node.as_reference() {
            if !seen.insert(id) {
                continue;
            }
            nodes.push(id);
        }
        let dict = doc.dereference(node).and_then(|(_, node)| dict_of(node));
        if let Some(parts) = dict.and_then(|dict| dict.get(b"DParts")) {
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
    let root = outlines.as_reference();
    let link = |dict: &Dict, key: &[u8]| dict.get(key).and_then(Object::as_reference);
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
        let Some(dict) = doc.get_dict(id) else {
            continue;
        };
        match under_closed {
            true => hidden.push(id),
            false => shown.push(id),
        }
        let count = dict.get(b"Count").and_then(|count| doc.dereference(count));
        let count = count.and_then(|(_, count)| count.as_integer());
        let closed = Some(id) != root && count.is_some_and(|count| count < 0);
        for key in [&b"Prev"[..], b"Last", b"Next"] {
            stack.extend(link(dict, key).map(|id| (id, under_closed)));
        }
        stack.extend(link(dict, b"First").map(|id| (id, under_closed || closed)));
    }
    shown.append(&mut hidden);
    shown
}
