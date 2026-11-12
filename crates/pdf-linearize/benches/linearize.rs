//! pdf-linearize timed on documents made here, so that a change to its plan,
//! its walk of what each page uses, the bytes it assembles around a caller's
//! bodies, its hint tables, its check or the family adapter can be judged
//! before and after it, by `cargo bench -p pdf-linearize`:
//!
//! - `plan`: `plan::plan` of a graph of 2,000 pages, each with ten objects of
//!   its own (a content stream, an image read last and eight dictionaries), a
//!   font each ten pages share, an outline of 100 items and a form of 50
//!   fields, each field's widget on a page of its own: listed by tables
//!   (`table`), and by streams with what may be packed in object streams
//!   (`packed`).
//! - `uses`: `plan::page_uses` of that graph's objects by their ids
//!   (`page-uses`), and `plan::group` of each object it reaches by its use
//!   (`group`).
//! - `assemble`: `layout::assemble` around that plan's bodies, each 40 bytes,
//!   its own streams as they are, in each kind (`table`, `packed`).
//! - `hint`: `hint::PageOffsets` of 10,000 pages written (`page-offsets/write`)
//!   and read back (`page-offsets/read`), and `hint::tables` of every table
//!   each assembled file's hint stream holds (`tables/table`,
//!   `tables/packed`).
//! - `check`: `check::check` of the facts of each assembled file, built from
//!   its bytes and its plan as a reader of any model would give them, the
//!   graph in the numbers it's written at among them, with `()`, which wants
//!   no finding (`table/none`, `packed/none`), and with a `Vec` that keeps
//!   them (`table/vec`, `packed/vec`).
//! - `family`, with the `family` feature, on by default: `family::linearize`
//!   of a document of 500 pages, read from a file made here, in the clear,
//!   under tables (`table`), streams (`stream`), and streams with object
//!   streams (`packed`), on one thread.
//!
//! Throughput is in pages, or in objects for `uses`. Each input is held to
//! what it gives before it's timed: a plan to its pages, every object written
//! once, and its size; a walk to each page's objects and the fonts shared; the
//! groups to every object once, none past the most a stream holds; a layout's
//! length to its pieces'; a table to what it reads back; the facts to a
//! verdict that holds with nothing to say; a file written to one that reads
//! back, holds with nothing to say and has every page. Nothing is read from
//! disk, nothing is encrypted, no cap is met and no filter is refused, so the
//! inputs are the same on every run.

use std::convert::Infallible;
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdf_linearize::Limits;
use pdf_linearize::check::{
    Extent, Facts, Finding, FirstObject, HintData, HintFacts, PageFacts, Raw, RowEntry, RowFacts,
    SectionFacts, check,
};
use pdf_linearize::hint::{self, PageEntry, PageOffsets};
use pdf_linearize::layout::{self, Encoded, Layout, Piece, assemble};
use pdf_linearize::pdf_objects::ObjectId;
use pdf_linearize::plan::{self, Graph, Group, Kind, MEMBERS, Page, Part, Plan, Use, What};

/// How many pages the graph has.
const PAGES: u32 = 2_000;

/// How many objects each page has of its own, beside the page object.
const OWN: u32 = 10;

/// How many pages share a font.
const FONT_PAGES: u32 = 10;

/// How many items the outline has, and fields the form.
const ITEMS: u32 = 100;
const FIELDS: u32 = 50;

/// How long each body is, as the plan's writes are given to `assemble`.
const BODY: u64 = 40;

/// How many pages the page offset table has.
const ENTRIES: u32 = 10_000;

/// The catalog, the page tree's root, its first node and the outline's root.
const CATALOG: u32 = 1;
const ROOT: u32 = 2;
const NODES: u32 = 3;
const NODE_COUNT: u32 = 20;
const OUTLINE: u32 = NODES + NODE_COUNT;
/// The form's dictionary, at open, and its fields after it.
const ACRO_FORM: u32 = OUTLINE + ITEMS + 1;
const FONTS: u32 = ACRO_FORM + FIELDS + 1;
const FIRST_PAGE: u32 = FONTS + PAGES / FONT_PAGES;

/// One object of the document: its number, what it refers to that a page's
/// walk goes into, in order, whether it may be packed, and whether a viewer
/// can do without it at first.
struct Object {
    number: u32,
    refs: Vec<u32>,
    packable: bool,
    late: bool,
}

/// A document's objects as the engine's groups take them, by number.
struct Doc {
    objects: Vec<Object>,
    /// Each page, in order, and its content stream.
    pages: Vec<(u32, u32)>,
}

/// Page `page`'s object.
fn page_number(page: u32) -> u32 {
    FIRST_PAGE + page * (OWN + 1)
}

/// The document: a catalog; a root node over 20 nodes of 100 pages each; an
/// outline's root and its items; a form's dictionary and its fields, field
/// `i` the parent of a widget on page `i`; 200 fonts, each of ten pages in a
/// row; and each page, its widget, seven more dictionaries, a font, an image
/// read last and its content stream, in the order F.3.7 has a page's objects
/// read. A page tree node's `/Kids`, a page's `/Parent` and an item's `/Dest`
/// aren't gone into, so they're no reference here.
fn doc() -> Doc {
    let mut objects = Vec::new();
    let mut object = |number: u32, refs: Vec<u32>, packable: bool, late: bool| {
        objects.push(Object {
            number,
            refs,
            packable,
            late,
        });
    };
    object(CATALOG, vec![], true, false);
    object(ROOT, vec![], true, false);
    for node in NODES..NODES + NODE_COUNT {
        object(node, vec![], true, false);
    }
    object(
        OUTLINE,
        (OUTLINE + 1..=OUTLINE + ITEMS).collect(),
        true,
        false,
    );
    for item in OUTLINE + 1..=OUTLINE + ITEMS {
        object(item, vec![], true, false);
    }
    let fields = ACRO_FORM + 1..=ACRO_FORM + FIELDS;
    object(ACRO_FORM, fields.clone().collect(), true, false);
    for (page, field) in (0..).zip(fields) {
        object(field, vec![page_number(page) + 3], true, false);
    }
    for font in FONTS..FIRST_PAGE {
        object(font, vec![], true, false);
    }
    let mut pages = Vec::new();
    for page in 0..PAGES {
        let at = page_number(page);
        let (content, image) = (at + 1, at + 2);
        let font = FONTS + page / FONT_PAGES;
        let mut refs: Vec<u32> = (at + 3..=at + OWN).collect();
        refs.extend([font, image, content]);
        object(at, refs, true, false);
        object(content, vec![], false, false);
        object(image, vec![], false, true);
        for own in at + 3..=at + OWN {
            object(own, vec![], true, false);
        }
        pages.push((at, content));
    }
    Doc { objects, pages }
}

impl Doc {
    /// The graph, listed as `kind` lists it, what may be packed packable where
    /// `packed`, each object at the id `to` gives its number.
    fn graph(&self, kind: Kind, packed: bool, to: impl Fn(u32) -> ObjectId) -> Graph {
        let mut graph = Graph::new(to(CATALOG), kind);
        for object in &self.objects {
            let refs = object.refs.iter().map(|&r| to(r));
            let node = graph.object(to(object.number), refs);
            node.packable(packed && object.packable);
            node.late(object.late);
        }
        for &(page, content) in &self.pages {
            let mut said = Page::new(to(page));
            said.contents = vec![to(content)];
            graph.page(said);
        }
        graph.tree_node(to(ROOT));
        for node in NODES..NODES + NODE_COUNT {
            graph.tree_node(to(node));
        }
        graph.open(to(ACRO_FORM));
        graph.group(Group::Outlines, [to(OUTLINE)]);
        graph.group(Group::Form, (ACRO_FORM + 1..=ACRO_FORM + FIELDS).map(&to));
        graph
    }

    /// What object `number` refers to.
    fn refs(&self) -> std::collections::HashMap<ObjectId, Vec<ObjectId>> {
        (self.objects.iter())
            .map(|object| {
                let refs = object.refs.iter().map(|&r| id(r)).collect();
                (id(object.number), refs)
            })
            .collect()
    }
}

/// Object `number` at generation 0.
fn id(number: u32) -> ObjectId {
    ObjectId::new(number, 0)
}

/// The two kinds each engine group times: tables, and streams with what may
/// be packed packed.
const KINDS: [(&str, Kind, bool); 2] = [
    ("table", Kind::Table, false),
    ("packed", Kind::Stream, true),
];

/// The plan of `doc` in `kind`, held to its pages, every object written once,
/// and its size, `name` saying which.
fn planned(name: &str, doc: &Doc, kind: Kind, packed: bool) -> Plan {
    let planned = plan::plan(&doc.graph(kind, packed, id), &Limits::DEFAULT)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(planned.page_count(), PAGES, "{name}: its pages");
    let written = (planned.writes().iter())
        .filter(|write| matches!(write.what, What::Object(_)))
        .count();
    let members: usize = (planned.containers().iter())
        .map(|container| container.members.len())
        .sum();
    assert_eq!(
        written + members,
        doc.objects.len(),
        "{name}: every object written once"
    );
    assert_eq!(packed, !planned.containers().is_empty(), "{name}: packed");
    let own = if kind == Kind::Stream { 4 } else { 2 };
    assert_eq!(
        planned.size() as usize,
        planned.writes().len() + members + own + 1,
        "{name}: its size"
    );
    planned
}

fn plan_group(c: &mut Criterion) {
    let doc = doc();
    let mut group = c.benchmark_group("plan");
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    for (name, kind, packed) in KINDS {
        let graph = doc.graph(kind, packed, id);
        planned(&format!("plan/{name}"), &doc, kind, packed);
        group.bench_function(name, |b| {
            b.iter(|| plan::plan(black_box(&graph), &Limits::DEFAULT))
        });
    }
    group.finish();
}

fn uses_group(c: &mut Criterion) {
    let doc = doc();
    let refs = doc.refs();
    let pages: Vec<ObjectId> = doc.pages.iter().map(|&(page, _)| id(page)).collect();
    let mut stop: std::collections::HashSet<ObjectId> = pages.iter().copied().collect();
    stop.extend([id(CATALOG), id(ROOT), id(ACRO_FORM)]);
    stop.extend((NODES..NODES + NODE_COUNT).map(id));
    let walk = |pages: &[ObjectId]| {
        plan::page_uses(pages, &stop, |from, to| {
            refs[&from].iter().for_each(|&r| to(r));
        })
    };
    let uses = walk(&pages);
    let reached = PAGES * (OWN + 1) + PAGES / FONT_PAGES;
    assert_eq!(
        uses.len(),
        reached as usize,
        "uses/page-uses: what's reached"
    );
    for page in [0, 1, PAGES - 1] {
        let at = page_number(page);
        for number in at..=at + OWN {
            assert_eq!(
                uses[&id(number)],
                Use::Page(page),
                "uses/page-uses: page {page}'s own"
            );
        }
    }
    let shared = uses.values().filter(|&&used| used == Use::Shared).count();
    assert_eq!(
        shared,
        (PAGES / FONT_PAGES) as usize,
        "uses/page-uses: the fonts shared"
    );
    let mut objects: Vec<(u32, Use)> = uses.iter().map(|(id, &used)| (id.number, used)).collect();
    objects.sort_unstable_by_key(|&(number, _)| number);
    let grouped = plan::group(objects.iter().copied());
    let first = (OWN + 1) as usize;
    let expected = first.div_ceil(MEMBERS) + (reached as usize - first).div_ceil(MEMBERS);
    assert_eq!(grouped.len(), expected, "uses/group: its streams");
    let mut members: Vec<u32> = grouped.iter().flatten().copied().collect();
    assert!(
        grouped.iter().all(|members| members.len() <= MEMBERS),
        "uses/group: none past the most a stream holds"
    );
    members.sort_unstable();
    let numbers: Vec<u32> = objects.iter().map(|&(number, _)| number).collect();
    assert_eq!(members, numbers, "uses/group: every object once");

    let mut group = c.benchmark_group("uses");
    group.throughput(Throughput::Elements(u64::from(reached)));
    group.bench_function("page-uses", |b| b.iter(|| walk(black_box(&pages))));
    group.bench_function("group", |b| {
        b.iter(|| plan::group(black_box(&objects).iter().copied()))
    });
    group.finish();
}

/// A stream of the layout's own, as it is.
fn plain(stream: layout::Stream<'_>) -> Result<Encoded, Infallible> {
    Ok(Encoded::plain(stream.data.to_vec()))
}

/// `plan`'s layout, every body [`BODY`] bytes, held to its length, its pieces'
/// and every body once in order, `name` saying which.
fn assembled(name: &str, plan: &Plan) -> Layout {
    let lengths = vec![BODY; plan.writes().len()];
    let options = layout::Options::new((1, 7));
    let layout =
        assemble(plan, &lengths, &options, plain).unwrap_or_else(|e| panic!("{name}: {e}"));
    let (mut len, mut bodies) = (0, Vec::new());
    for piece in layout.pieces() {
        match piece {
            Piece::Bytes(bytes) => len += bytes.len() as u64,
            Piece::Body(at) => {
                len += lengths[at];
                bodies.push(at);
            }
        }
    }
    assert_eq!(layout.len(), len, "{name}: its length, its pieces'");
    let every: Vec<usize> = (0..plan.writes().len()).collect();
    assert_eq!(bodies, every, "{name}: every body once, in order");
    layout
}

fn assemble_group(c: &mut Criterion) {
    let doc = doc();
    let mut group = c.benchmark_group("assemble");
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    for (name, kind, packed) in KINDS {
        let plan = planned(&format!("assemble/{name}"), &doc, kind, packed);
        assembled(&format!("assemble/{name}"), &plan);
        let lengths = vec![BODY; plan.writes().len()];
        let options = layout::Options::new((1, 7));
        group.bench_function(name, |b| {
            b.iter(|| assemble(black_box(&plan), &lengths, &options, plain))
        });
    }
    group.finish();
}

/// The page offset table of [`ENTRIES`] pages, each of its own number of
/// objects, length and content, and up to three shared objects of 200.
fn page_offsets() -> PageOffsets {
    let pages = (0..u64::from(ENTRIES))
        .map(|i| PageEntry {
            objects: 3 + i % 9,
            length: 2_000 + (i * 7_919) % 30_000,
            shared: (0..i % 4)
                .map(|k| (((i + k * 13) % 200) as u32, (k % 4) as u32))
                .collect(),
            content_at: 60 + i % 40,
            content_length: 900 + (i * 31) % 1_000,
        })
        .collect();
    PageOffsets::of(1_000, pages, hint::id_width(199), 4)
}

fn hint_group(c: &mut Criterion) {
    let table = page_offsets();
    let mut data = Vec::new();
    table
        .write(&mut data)
        .unwrap_or_else(|e| panic!("hint/page-offsets/write: {e}"));
    let read = PageOffsets::read(&data, ENTRIES, &Limits::DEFAULT)
        .unwrap_or_else(|e| panic!("hint/page-offsets/read: {e}"));
    assert_eq!(read, table, "hint/page-offsets/read: the table written");

    let mut group = c.benchmark_group("hint");
    group.throughput(Throughput::Elements(u64::from(ENTRIES)));
    group.bench_function("page-offsets/write", |b| {
        b.iter(|| {
            let mut out = Vec::with_capacity(data.len());
            black_box(&table).write(&mut out).map(|()| out)
        })
    });
    group.bench_function("page-offsets/read", |b| {
        b.iter(|| PageOffsets::read(black_box(&data), ENTRIES, &Limits::DEFAULT))
    });

    let doc = doc();
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    for (name, kind, packed) in KINDS {
        let name = format!("tables/{name}");
        let plan = planned(&format!("hint/{name}"), &doc, kind, packed);
        let layout = assembled(&format!("hint/{name}"), &plan);
        let (data, keys) = (layout.hint_data(), layout.hint_keys());
        let tables = hint::tables(data, keys, PAGES, &Limits::DEFAULT)
            .unwrap_or_else(|e| panic!("hint/{name}: {e}"));
        assert_eq!(
            tables.pages.pages.len(),
            PAGES as usize,
            "hint/{name}: its pages"
        );
        assert_eq!(
            tables.shared.groups.len(),
            plan.entries().len(),
            "hint/{name}: its shared entries"
        );
        assert!(
            tables.outlines.is_some() && tables.form.is_some(),
            "hint/{name}: the outline's and the form's"
        );
        group.bench_function(name, |b| {
            b.iter(|| hint::tables(black_box(data), keys, PAGES, &Limits::DEFAULT))
        });
    }
    group.finish();
}

/// The bytes of `layout`, each body [`BODY`] spaces.
fn bytes_of(layout: &Layout) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(layout.len() as usize);
    for piece in layout.pieces() {
        match piece {
            Piece::Bytes(part) => bytes.extend_from_slice(part),
            Piece::Body(_) => bytes.resize(bytes.len() + BODY as usize, b' '),
        }
    }
    bytes
}

/// The facts of the file `plan` and `layout` make, its positions from its
/// bytes where a reader would have them from there, and `graph`, its objects
/// in the numbers they're written at, as a plan of a table would take them.
fn facts_of(plan: &Plan, layout: &Layout, graph: Graph) -> Facts {
    let bytes = bytes_of(layout);
    let mut facts = Facts::new(bytes.len() as u64);

    let dictionary = plan.dictionary_number();
    let at = layout.position(dictionary).expect("the dictionary's place");
    let text = String::from_utf8_lossy(&bytes[at as usize..at as usize + 200]);
    let end = at + (text.find("endobj").expect("its end") + 6) as u64;
    let mut first = FirstObject::new(at, end, dictionary, 0);
    let key = |name: &str| {
        let rest = text.split(&format!("/{name} ")).nth(1)?;
        rest.split_ascii_whitespace().next()?.parse::<i64>().ok()
    };
    let raw = |name: &str| key(name).map_or(Raw::Absent, Raw::Integer);
    first.linearized = raw("Linearized");
    first.l = raw("L");
    first.o = raw("O");
    first.e = raw("E");
    first.n = raw("N");
    first.t = raw("T");
    first.p = raw("P");
    first.h = Raw::Array;
    let items = text.split("/H [").nth(1).and_then(|h| h.split(']').next());
    first.h_items = (items.expect("/H").split_ascii_whitespace())
        .map(|item| Raw::Integer(item.parse().expect("an integer")))
        .collect();
    facts.first_object = Some(first);

    let row_of = |number: u32| {
        let in_stream = (plan.containers().iter()).find_map(|container| {
            let index = container.members.iter().position(|m| m.1 == number)?;
            Some(RowEntry::Compressed {
                stream: container.number,
                index: index as u32,
            })
        });
        let entry = match (number, in_stream, layout.position(number)) {
            (0, ..) => RowEntry::Free,
            (_, Some(entry), _) => entry,
            (_, None, Some(at)) => RowEntry::InUse(at),
            _ => panic!("nothing's at {number}"),
        };
        RowFacts::new(number, u16::from(number == 0) * u16::MAX, entry)
    };
    let mut main = SectionFacts::new(layout.main_section(), plan.kind());
    main.subsections = vec![(0, dictionary)];
    main.rows = (0..dictionary).map(row_of).collect();
    main.size = Raw::Integer(i64::from(dictionary));
    main.startxref = Some(layout.first_section());
    main.stream = plan.main_xref_number();
    let t = layout.main_first_entry();
    main.first_row = Some(match plan.kind() {
        Kind::Table => (t, t + 1),
        _ => (t, t),
    });
    let mut section = SectionFacts::new(layout.first_section(), plan.kind());
    section.subsections = vec![(dictionary, plan.size() - dictionary)];
    section.rows = (dictionary..plan.size()).map(row_of).collect();
    section.size = Raw::Integer(i64::from(plan.size()));
    section.prev = Raw::Integer(layout.main_section() as i64);
    section.root = Raw::Reference(id(plan.catalog_number()));
    section.startxref = Some(0);
    section.stream = plan.first_xref_number();
    facts.first = Some(section);
    facts.main = Some(main);

    let (hint_at, hint_len) = layout.hint_extent();
    let data = HintData::Decoded(layout.hint_data().to_vec());
    let mut hint = HintFacts::new(hint_at, hint_at + hint_len, plan.hint_number(), data);
    let keys = layout.hint_keys();
    let position = |at: u64| Raw::Integer(at as i64);
    hint.keys.push((b"S".to_vec(), position(keys.shared)));
    let optional = [
        (&b"T"[..], keys.thumbnails),
        (b"O", keys.outlines),
        (b"A", keys.threads),
        (b"E", keys.dests),
        (b"V", keys.form),
        (b"I", keys.info),
        (b"C", keys.structure),
        (b"L", keys.labels),
        (b"R", keys.renditions),
        (b"B", keys.files),
    ];
    for (key, at) in optional {
        hint.keys.extend(at.map(|at| (key.to_vec(), position(at))));
    }
    facts.hint = Some(hint);

    facts.pages = (0..plan.page_count())
        .map(|page| {
            let number = match page == plan.first_page() {
                true => plan.first_page_number(),
                false => plan.writes()[plan.page_writes(page).start].number,
            };
            let mut said = PageFacts::new(Some(id(number)));
            said.resources = true;
            said.media_box = true;
            said
        })
        .collect();
    // Where what's last in the first page's section ends, as read.
    let part6 = plan.part_writes(Part::FirstPage);
    if !part6.is_empty() {
        let last = plan.writes()[part6.end - 1].number;
        let start = layout.position(last).expect("written in place");
        facts.objects.push(Extent::new(last, start, start + BODY));
    }
    facts.graph = Some(graph);
    facts
}

fn check_group(c: &mut Criterion) {
    let doc = doc();
    let mut group = c.benchmark_group("check");
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    for (name, kind, packed) in KINDS {
        let plan = planned(&format!("check/{name}"), &doc, kind, packed);
        let layout = assembled(&format!("check/{name}"), &plan);
        let numbered = |number: u32| plan.number(id(number)).expect("planned");
        // As a reader gives it: what each object refers to, nothing packed.
        let graph = doc.graph(Kind::Table, false, numbered);
        let facts = facts_of(&plan, &layout, graph);
        let mut findings: Vec<Finding> = Vec::new();
        let verdict = check(&facts, &Limits::DEFAULT, &mut findings);
        assert_eq!(findings, [], "check/{name}: nothing to say");
        assert!(
            verdict.linearized && verdict.holds && verdict.hints_read,
            "check/{name}: it holds"
        );
        assert_eq!(
            check(&facts, &Limits::DEFAULT, &mut ()),
            verdict,
            "check/{name}/none: the verdict"
        );
        group.bench_function(format!("{name}/none"), |b| {
            b.iter(|| check(black_box(&facts), &Limits::DEFAULT, &mut ()))
        });
        group.bench_function(format!("{name}/vec"), |b| {
            b.iter(|| {
                let mut findings: Vec<Finding> = Vec::new();
                let verdict = check(black_box(&facts), &Limits::DEFAULT, &mut findings);
                (verdict, findings)
            })
        });
    }
    group.finish();
}

/// The `family` group, over a document read from a file made here: a catalog,
/// a root node over ten nodes of 50 pages each with the box every page
/// inherits, 50 fonts, each of ten pages in a row, an information dictionary,
/// and each page with its resources and its own content stream, under a table.
#[cfg(feature = "family")]
mod family {
    use super::{Criterion, FONT_PAGES, Finding, Limits, Throughput, black_box, check};
    use pdf_linearize::family::{self, Options};
    use pdf_linearize::pdf_structure::XrefKind;
    use pdf_linearize::pdf_structure::read::{self, Reading};

    /// How many pages the file has, and how many each node lists.
    pub const PAGES: u32 = 500;
    const NODE_PAGES: u32 = 50;
    const FONTS: u32 = PAGES / FONT_PAGES;

    /// The file, numbered from 1: the catalog, the root, the nodes, the fonts,
    /// the information dictionary, then each page and its content.
    pub fn file() -> Vec<u8> {
        let nodes = PAGES / NODE_PAGES;
        let (first_node, first_font) = (3, 3 + nodes);
        let info = first_font + FONTS;
        let page_at = |page: u32| info + 1 + 2 * page;
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {PAGES} /MediaBox [0 0 612 792] >>",
                (first_node..first_node + nodes)
                    .map(|node| format!("{node} 0 R"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ];
        for node in 0..nodes {
            let kids: Vec<String> = (node * NODE_PAGES..(node + 1) * NODE_PAGES)
                .map(|page| format!("{} 0 R", page_at(page)))
                .collect();
            objects.push(format!(
                "<< /Type /Pages /Parent 2 0 R /Kids [{}] /Count {NODE_PAGES} >>",
                kids.join(" ")
            ));
        }
        for font in 0..FONTS {
            objects.push(format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /F{font} >>"
            ));
        }
        objects.push("<< /Title (500 pages) /Producer (a benchmark) >>".to_string());
        for page in 0..PAGES {
            let node = first_node + page / NODE_PAGES;
            let font = first_font + page / FONT_PAGES;
            let page_of = format!("/Type /Page /Parent {node} 0 R");
            let resources = format!("/Resources << /Font << /F1 {font} 0 R >> >>");
            let contents = format!("/Contents {} 0 R", page_at(page) + 1);
            objects.push(format!("<< {page_of} {resources} {contents} >>"));
            let mut content = format!("BT /F1 12 Tf 72 720 Td (page {page}) Tj ET\n");
            for line in 0..20 {
                content.push_str(&format!(
                    "{} {} m {} {} l S\n",
                    line * 3,
                    page % 700,
                    line * 29,
                    792 - line
                ));
            }
            objects.push(format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ));
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut rows = Vec::new();
        for (number, object) in (1..).zip(&objects) {
            rows.push(out.len());
            out.extend_from_slice(format!("{number} 0 obj\n{object}\nendobj\n").as_bytes());
        }
        let at = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for row in rows {
            out.extend_from_slice(format!("{row:010} 00000 n \n").as_bytes());
        }
        let size = objects.len() + 1;
        let trailer = format!("<< /Size {size} /Root 1 0 R /Info {info} 0 R >>");
        out.extend_from_slice(format!("trailer\n{trailer}\nstartxref\n{at}\n%%EOF\n").as_bytes());
        out
    }

    /// The three kinds, on one thread.
    pub fn kinds() -> [(&'static str, Options<'static>); 3] {
        let mut table = Options::DEFAULT;
        table.threads = 1;
        let mut stream = table;
        stream.kind = XrefKind::Stream;
        let mut packed = stream;
        packed.pack = true;
        [("table", table), ("stream", stream), ("packed", packed)]
    }

    pub fn group(c: &mut Criterion) {
        let bytes = file();
        let reading = Reading::open(&bytes[..], &read::Options::DEFAULT)
            .unwrap_or_else(|e| panic!("family: the file: {e}"));
        let doc = (reading.to_document()).unwrap_or_else(|e| panic!("family: the document: {e}"));
        let mut group = c.benchmark_group("family");
        group.throughput(Throughput::Elements(u64::from(PAGES)));
        for (name, options) in kinds() {
            let mut out = Vec::new();
            let linearized = family::linearize(&mut out, &doc, None, Some(&reading), &options)
                .unwrap_or_else(|e| panic!("family/{name}: {e}"));
            assert_eq!(
                linearized.layout.len(),
                out.len() as u64,
                "family/{name}: its length"
            );
            let back = Reading::open(&out[..], &read::Options::DEFAULT)
                .unwrap_or_else(|e| panic!("family/{name}: read back: {e}"));
            let facts = family::facts(&back, &Limits::DEFAULT)
                .unwrap_or_else(|e| panic!("family/{name}: its facts: {e}"));
            assert_eq!(
                facts.pages.len(),
                PAGES as usize,
                "family/{name}: its pages"
            );
            let mut findings: Vec<Finding> = Vec::new();
            let verdict = check(&facts, &Limits::DEFAULT, &mut findings);
            assert_eq!(findings, [], "family/{name}: nothing to say");
            assert!(
                verdict.linearized && verdict.holds && verdict.hints_read,
                "family/{name}: it holds"
            );
            group.bench_function(name, |b| {
                b.iter(|| {
                    let mut out = Vec::new();
                    family::linearize(&mut out, black_box(&doc), None, Some(&reading), &options)
                        .map(|_| out)
                })
            });
        }
        group.finish();
    }
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    plan_group(&mut criterion);
    uses_group(&mut criterion);
    assemble_group(&mut criterion);
    hint_group(&mut criterion);
    check_group(&mut criterion);
    #[cfg(feature = "family")]
    family::group(&mut criterion);
    criterion.final_summary();
}
