//! Each finding, from the facts of a file laid out by a plan with one thing
//! changed: it fires for that and nothing else that isn't its own consequence.

use super::*;
use crate::fuzz::facts_of;
use crate::hint::{EmbeddedFiles, FileEntry, Generic, ThumbEntry, Thumbnails};
use crate::layout::{Encoded, Options, assemble};
use crate::plan::{Graph, Group, Kind, Page, plan};
use pdf_objects::ObjectId;

use crate::Limits;

fn id(number: u32) -> ObjectId {
    ObjectId::new(number, 0)
}

/// Four pages, each its own object and a content stream, what all three share,
/// what the later two share, a form's fields and a catalog, each object by what
/// `to` makes of its number.
fn graph_by(kind: Kind, to: impl Fn(u32) -> ObjectId) -> Graph {
    let mut graph = Graph::new(to(1), kind);
    graph.object(to(1), [to(2), to(30)]);
    graph.object(to(2), [to(3), to(4), to(5), to(6)]);
    graph.object(to(30), []);
    for page in [3, 4, 5, 6] {
        let later = (page != 3).then_some(to(21));
        let refs = [to(page + 10), to(20)].into_iter().chain(later);
        graph.object(to(page), refs);
        graph.object(to(page + 10), []);
        let mut said = Page::new(to(page));
        said.contents = vec![to(page + 10)];
        graph.page(said);
    }
    graph.object(to(20), []);
    graph.object(to(21), []);
    graph.tree_node(to(2));
    graph.group(Group::Form, [to(30)]);
    graph
}

fn graph(kind: Kind) -> Graph {
    graph_by(kind, id)
}

/// The facts of [`graph`] laid out as `kind` lists it.
fn laid_out(kind: Kind) -> Facts {
    let graph = graph(kind);
    let planned = plan(&graph, &Limits::DEFAULT).unwrap();
    let lengths: Vec<u64> = (0..planned.writes().len() as u64)
        .map(|at| 40 + at)
        .collect();
    let layout = assemble(&planned, &lengths, &Options::new((1, 7)), |stream| {
        Ok::<_, std::convert::Infallible>(Encoded::plain(stream.data.to_vec()))
    })
    .unwrap();
    facts_of(&planned, &layout)
}

fn table() -> Facts {
    laid_out(Kind::Table)
}

fn findings(facts: &Facts) -> Vec<Finding> {
    let mut findings = Vec::new();
    check(facts, &Limits::DEFAULT, &mut findings);
    findings
}

/// That `change` makes `facts` tell what `told` says, and the verdict what it
/// should.
#[track_caller]
fn fires(mut facts: Facts, change: impl FnOnce(&mut Facts), told: impl Fn(&Finding) -> bool) {
    change(&mut facts);
    let found = findings(&facts);
    let finding = found.iter().find(|finding| told(finding));
    let Some(finding) = finding else {
        panic!("not among {found:?}");
    };
    let verdict = check(&facts, &Limits::DEFAULT, &mut ());
    let error = found.iter().any(|f| f.grade() == Grade::Error);
    assert_eq!(verdict.holds, verdict.linearized && !error);
    assert!(!finding.to_string().is_empty());
    assert!(
        finding.to_string().contains(finding.clause()),
        "{finding}: {}",
        finding.clause()
    );
}

fn dict(facts: &mut Facts) -> &mut FirstObject {
    facts.first_object.as_mut().unwrap()
}

fn first_section(facts: &mut Facts) -> &mut SectionFacts {
    facts.first.as_mut().unwrap()
}

fn main_section(facts: &mut Facts) -> &mut SectionFacts {
    facts.main.as_mut().unwrap()
}

fn hint_of(facts: &mut Facts) -> &mut HintFacts {
    facts.hint.as_mut().unwrap()
}

/// `facts`' hint data's tables, changed by `change`, and written back. A
/// table's widths stay what they were, or say so themselves.
fn with_tables(facts: &mut Facts, change: impl FnOnce(&mut crate::hint::Tables)) {
    let keys = {
        let hint = hint_of(facts);
        let mut keys = crate::hint::HintKeys::default();
        for (key, raw) in &hint.keys {
            let Raw::Integer(at) = raw else { continue };
            match key.as_slice() {
                b"S" => keys.shared = *at as u64,
                b"V" => keys.form = Some(*at as u64),
                _ => {}
            }
        }
        keys
    };
    let pages = match dict(facts).n {
        Raw::Integer(n) => n as u32,
        _ => 0,
    };
    let HintData::Decoded(data) = &hint_of(facts).data else {
        panic!("decoded");
    };
    let mut tables = crate::hint::tables(data, &keys, pages, &Limits::DEFAULT).unwrap();
    change(&mut tables);
    let mut written = Vec::new();
    tables.pages.write(&mut written).unwrap();
    let shared = written.len() as i64;
    tables.shared.write(&mut written).unwrap();
    let form = written.len() as i64;
    tables.form.as_ref().unwrap().write(&mut written).unwrap();
    let mut more = Vec::new();
    if let Some(thumbnails) = &tables.thumbnails {
        more.push((b"T".to_vec(), Raw::Integer(written.len() as i64)));
        thumbnails.write(&mut written).unwrap();
    }
    if let Some(outlines) = &tables.outlines {
        more.push((b"O".to_vec(), Raw::Integer(written.len() as i64)));
        outlines.write(&mut written).unwrap();
    }
    if let Some(files) = &tables.files {
        more.push((b"B".to_vec(), Raw::Integer(written.len() as i64)));
        let id_bits = tables.pages.header.id_bits;
        files.write(&mut written, id_bits).unwrap();
    }
    let hint = hint_of(facts);
    hint.data = HintData::Decoded(written);
    for (key, raw) in &mut hint.keys {
        match key.as_slice() {
            b"S" => *raw = Raw::Integer(shared),
            b"V" => *raw = Raw::Integer(form),
            _ => {}
        }
    }
    hint.keys.extend(more);
}

#[test]
fn what_a_plan_lays_out_holds_in_each_kind() {
    for kind in [Kind::Table, Kind::Stream] {
        let facts = laid_out(kind);
        assert_eq!(findings(&facts), []);
        let verdict = check(&facts, &Limits::DEFAULT, &mut ());
        assert!(verdict.linearized && verdict.holds && verdict.hints_read);
    }
}

/// `facts` of the file with `by` bytes before its header: each position
/// that many further on, every value the file writes as it was, as those
/// count from the header (ISO 32000-2, 7.5.2).
fn past(facts: &Facts, by: u64) -> Facts {
    let mut facts = facts.clone();
    facts.len += by;
    facts.header += by;
    if let Some(first) = &mut facts.first_object {
        (first.at, first.end) = (first.at + by, first.end + by);
    }
    let rows = |rows: &mut Vec<RowFacts>| {
        for row in rows {
            if let RowEntry::InUse(at) = &mut row.entry {
                *at += by;
            }
        }
    };
    for section in facts.first.iter_mut().chain(&mut facts.main) {
        section.at += by;
        rows(&mut section.rows);
        rows(&mut section.hybrid);
        section.first_row = (section.first_row).map(|(space, row)| (space + by, row + by));
        section.startxref = section.startxref.map(|at| at + by);
    }
    if let Some(hint) = &mut facts.hint {
        (hint.at, hint.end) = (hint.at + by, hint.end + by);
        hint.overflow = (hint.overflow).map(|(at, end)| (at + by, end + by));
    }
    for extent in &mut facts.objects {
        (extent.start, extent.end) = (extent.start + by, extent.end + by);
    }
    facts.update = facts.update.map(|at| at + by);
    facts
}

/// Bytes before the header move nothing the check holds: what a plan lays
/// out holds in each kind, its hints read, whatever's before its header;
/// and an `/E` short of the first page is told alike, the places it's held
/// to further on, its value as written.
#[test]
fn bytes_before_the_header_move_nothing_the_check_holds() {
    for kind in [Kind::Table, Kind::Stream] {
        let facts = laid_out(kind);
        for by in [1, 7, 300] {
            let moved = past(&facts, by);
            assert_eq!(findings(&moved), [], "{kind:?} {by}");
            let verdict = check(&moved, &Limits::DEFAULT, &mut ());
            assert!(verdict.linearized && verdict.holds && verdict.hints_read);
            assert_eq!(moved.hint_extent().0, facts.hint_extent().0 + by);
        }
    }
    let mut facts = table();
    let Raw::Integer(e) = dict(&mut facts).e else {
        panic!("/E");
    };
    dict(&mut facts).e = Raw::Integer(e + 1);
    let found = findings(&facts);
    let [
        Finding::FirstPageEnd {
            e: said,
            least,
            most,
        },
    ] = found[..]
    else {
        panic!("{found:?}");
    };
    for by in [1, 7, 300] {
        let moved = findings(&past(&facts, by));
        let (least, most) = (least + by, most + by);
        assert_eq!(
            moved,
            [Finding::FirstPageEnd {
                e: said,
                least,
                most
            }],
            "{by}"
        );
    }
}

#[test]
fn a_file_that_doesnt_say_it_is_has_no_findings() {
    let mut none = Facts::new(100);
    assert_eq!(findings(&none), []);
    assert!(!check(&none, &Limits::DEFAULT, &mut ()).linearized);
    none.first_object = Some(FirstObject::new(9, 20, 1, 0));
    assert_eq!(findings(&none), []);
    assert!(!check(&none, &Limits::DEFAULT, &mut ()).linearized);
}

#[test]
fn a_stream_or_a_dictionary_past_the_first_bytes_is_no_linearization() {
    let mut facts = table();
    dict(&mut facts).stream = true;
    assert_eq!(findings(&facts), []);
    assert!(!check(&facts, &Limits::DEFAULT, &mut ()).linearized);
    let mut facts = table();
    dict(&mut facts).at = 1024;
    assert_eq!(findings(&facts), []);
    assert!(!check(&facts, &Limits::DEFAULT, &mut ()).linearized);
    let mut facts = table();
    facts.header = 1;
    dict(&mut facts).at = 1024;
    dict(&mut facts).l = Raw::Integer(facts.len as i64 - 1);
    assert!(check(&facts, &Limits::DEFAULT, &mut ()).linearized);
}

#[test]
fn a_version_of_another_integer_part_is_told() {
    for raw in [Raw::Integer(2), Raw::Real(2.0), Raw::Other] {
        let mut facts = table();
        dict(&mut facts).linearized = raw;
        assert_eq!(findings(&facts), [Finding::Version]);
        assert!(!check(&facts, &Limits::DEFAULT, &mut ()).linearized);
    }
    let mut facts = table();
    dict(&mut facts).linearized = Raw::Real(1.9);
    assert_eq!(findings(&facts), []);
}

#[test]
fn a_length_that_isnt_the_files_says_it_isnt_and_an_update_is_said() {
    let mut facts = table();
    let len = facts.len;
    facts.len += 10;
    facts.update = Some(len + 5);
    let at = len + 5;
    assert_eq!(
        findings(&facts),
        [
            Finding::LengthNotFile {
                l: len as i64,
                len: len + 10
            },
            Finding::Updated { at }
        ]
    );
    let verdict = check(&facts, &Limits::DEFAULT, &mut ());
    assert!(!verdict.linearized && !verdict.holds && !verdict.hints_read);
}

#[test]
fn a_length_of_another_type_is_told_and_one_by_reference_too() {
    let mut facts = table();
    dict(&mut facts).l = Raw::Real(10.0);
    assert_eq!(findings(&facts), [Finding::BadValue { key: DictKey::L }]);
    let mut facts = table();
    dict(&mut facts).l = Raw::Reference(ObjectId::new(1, 0));
    assert_eq!(findings(&facts), [Finding::Indirect { key: DictKey::L }]);
}

#[test]
fn a_length_from_the_header() {
    let mut facts = table();
    facts.header = 3;
    assert!(!check(&facts, &Limits::DEFAULT, &mut ()).linearized);
    let l = facts.len as i64 - 3;
    dict(&mut facts).l = Raw::Integer(l);
    assert!(check(&facts, &Limits::DEFAULT, &mut ()).linearized);
}

#[test]
fn a_dictionary_past_1024_is_said_and_only_said() {
    let mut facts = table();
    dict(&mut facts).end = 1025;
    let found = findings(&facts);
    assert!(found.contains(&Finding::DictionaryPast1024 { end: 1025 }));
    assert_eq!(
        Finding::DictionaryPast1024 { end: 1025 }.grade(),
        Grade::Departure
    );
    let mut facts = table();
    dict(&mut facts).end = 1024;
    assert_eq!(findings(&facts), []);
    // From the header.
    let mut facts = table();
    facts.header = 2;
    dict(&mut facts).end = 1026;
    dict(&mut facts).l = Raw::Integer(facts.len as i64 - 2);
    assert!(!(findings(&facts).iter()).any(|f| matches!(f, Finding::DictionaryPast1024 { .. })));
}

#[test]
fn each_key_missing_or_by_reference_is_told() {
    type Set = fn(&mut FirstObject, Raw);
    let keys: [(DictKey, Set); 5] = [
        (DictKey::H, |first, raw| first.h = raw),
        (DictKey::O, |first, raw| first.o = raw),
        (DictKey::E, |first, raw| first.e = raw),
        (DictKey::N, |first, raw| first.n = raw),
        (DictKey::T, |first, raw| first.t = raw),
    ];
    for (key, set) in keys {
        fires(
            table(),
            |facts| set(dict(facts), Raw::Other),
            |f| *f == Finding::BadValue { key },
        );
        fires(
            table(),
            |facts| set(dict(facts), Raw::Reference(ObjectId::new(1, 0))),
            |f| *f == Finding::Indirect { key },
        );
    }
    fires(
        table(),
        |facts| dict(facts).p = Raw::Other,
        |f| *f == Finding::BadValue { key: DictKey::P },
    );
    fires(
        table(),
        |facts| dict(facts).p = Raw::Reference(ObjectId::new(1, 0)),
        |f| *f == Finding::Indirect { key: DictKey::P },
    );
}

#[test]
fn h_items_are_told() {
    fires(
        table(),
        |facts| dict(facts).h_items.push(Raw::Integer(1)),
        |f| *f == Finding::HintItems { items: 3 },
    );
    fires(
        table(),
        |facts| dict(facts).h_items[1] = Raw::Other,
        |f| *f == Finding::BadValue { key: DictKey::H },
    );
    fires(
        table(),
        |facts| dict(facts).h_items[1] = Raw::Reference(ObjectId::new(1, 0)),
        |f| *f == Finding::Indirect { key: DictKey::H },
    );
}

#[test]
fn o_that_isnt_the_first_pages_is_told() {
    fires(
        table(),
        |facts| dict(facts).o = Raw::Integer(1),
        |f| matches!(f, Finding::FirstPageObject { o: 1, page: 0, .. }),
    );
}

#[test]
fn o_is_of_the_page_p_says() {
    let mut facts = table();
    dict(&mut facts).p = Raw::Integer(1);
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::FirstPageObject { page: 1, .. })),
        "{found:?}"
    );
}

#[test]
fn a_page_count_off_is_told() {
    let mut facts = table();
    let pages = facts.pages.len() as u64;
    dict(&mut facts).n = Raw::Integer(pages as i64 + 1);
    let found = findings(&facts);
    assert!(found.contains(&Finding::PageCount {
        n: pages as i64 + 1,
        pages
    }));
    let verdict = check(&facts, &Limits::DEFAULT, &mut ());
    assert!(!verdict.holds);
}

#[test]
fn e_outside_the_first_pages_end_is_told_and_inside_isnt() {
    let e = |facts: &mut Facts| match dict(facts).e {
        Raw::Integer(e) => e,
        _ => panic!("/E"),
    };
    let mut facts = table();
    let at = e(&mut facts);
    fires(
        facts.clone(),
        |facts| dict(facts).e = Raw::Integer(at + 1),
        |f| matches!(f, Finding::FirstPageEnd { .. }),
    );
    // Short by the last object, or by a read end: within it's not its end.
    let number = {
        let first = first_section(&mut facts);
        first.rows[first.rows.len() - 2].number
    };
    let row = (first_section(&mut facts).rows.iter())
        .find(|row| row.number == number)
        .copied()
        .unwrap();
    let RowEntry::InUse(start) = row.entry else {
        panic!("in use");
    };
    fires(
        facts.clone(),
        |facts| dict(facts).e = Raw::Integer(start as i64),
        |f| matches!(f, Finding::FirstPageEnd { .. }),
    );
    let mut read = facts.clone();
    read.objects = vec![Extent::new(number, start, at as u64 - 2)];
    fires(
        read.clone(),
        |facts| dict(facts).e = Raw::Integer(at - 3),
        |f| matches!(f, Finding::FirstPageEnd { .. }),
    );
    let mut within = read;
    dict(&mut within).e = Raw::Integer(at - 2);
    assert_eq!(findings(&within), []);
}

#[test]
fn e_isnt_held_to_a_hint_stream_after_the_first_page() {
    let mut facts = table();
    let hint = hint_of(&mut facts).number;
    let Raw::Integer(length) = dict(&mut facts).h_items[1] else {
        panic!("/H");
    };
    // Written at the file's end, though not numbered so: F.3.6 has it either
    // way, and whatever else moved by that, the first group's last object
    // doesn't.
    let at = facts.len;
    facts.len += length as u64;
    dict(&mut facts).l = Raw::Integer(facts.len as i64);
    dict(&mut facts).h_items[0] = Raw::Integer(at as i64);
    let first = first_section(&mut facts);
    let row = first
        .rows
        .iter_mut()
        .find(|row| row.number == hint)
        .unwrap();
    row.entry = RowEntry::InUse(at);
    hint_of(&mut facts).at = at;
    hint_of(&mut facts).end = at + length as u64;
    let found = findings(&facts);
    assert!(
        !found
            .iter()
            .any(|f| matches!(f, Finding::FirstPageEnd { .. })),
        "{found:?}"
    );
}

#[test]
fn t_not_at_the_main_sections_first_entry_is_told() {
    // A table's: the white-space before the row, or the row.
    let t = |facts: &mut Facts| match dict(facts).t {
        Raw::Integer(t) => t,
        _ => panic!("/T"),
    };
    let mut facts = table();
    let at = t(&mut facts);
    dict(&mut facts).t = Raw::Integer(at + 1);
    assert_eq!(findings(&facts), []);
    fires(
        table(),
        |facts| dict(facts).t = Raw::Integer(at + 2),
        |f| matches!(f, Finding::MainFirstEntry { .. }),
    );
    fires(
        table(),
        |facts| dict(facts).t = Raw::Integer(at - 1),
        |f| matches!(f, Finding::MainFirstEntry { .. }),
    );
    // A stream's: the object, or white-space before it.
    let mut stream = laid_out(Kind::Stream);
    let at = t(&mut stream);
    fires(
        stream.clone(),
        |facts| dict(facts).t = Raw::Integer(at - 1),
        |f| matches!(f, Finding::MainFirstEntry { .. }),
    );
    main_section(&mut stream).first_row = Some((at as u64 - 1, at as u64));
    dict(&mut stream).t = Raw::Integer(at - 1);
    assert_eq!(findings(&stream), []);
}

#[test]
fn h_at_no_stream_or_of_another_length_is_told() {
    fires(
        table(),
        |facts| facts.hint = None,
        |f| matches!(f, Finding::HintNotListed { .. }),
    );
    fires(
        table(),
        |facts| hint_of(facts).at += 1,
        |f| matches!(f, Finding::HintNotListed { .. }),
    );
    let mut facts = table();
    let Raw::Integer(length) = dict(&mut facts).h_items[1] else {
        panic!("/H");
    };
    fires(
        facts,
        |facts| dict(facts).h_items[1] = Raw::Integer(length - 1),
        |f| matches!(f, Finding::HintExtent { .. }),
    );
    // Told once, not of everything after it.
    let mut facts = table();
    dict(&mut facts).h_items[1] = Raw::Integer(length + 5000);
    let found = findings(&facts);
    assert!(
        matches!(found[..], [Finding::HintExtent { .. }]),
        "{found:?}"
    );
}

#[test]
fn an_overflow_streams_length_is_held_too() {
    let mut facts = table();
    let last = (main_section(&mut facts).rows.iter())
        .filter_map(|row| match row.entry {
            RowEntry::InUse(at) => Some(at),
            _ => None,
        })
        .max()
        .unwrap();
    hint_of(&mut facts).overflow = Some((last, last + 10));
    dict(&mut facts)
        .h_items
        .extend([Raw::Integer(last as i64), Raw::Integer(9)]);
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::HintExtent { at, .. } if *at == last)),
        "{found:?}"
    );
}

/// The claim a caller reads (`Facts::claim`) is the one the check takes
/// before it tells anything: a dictionary with `/Linearized`, not a stream,
/// starting 1,023 bytes past the header but not 1,024, whatever comes before
/// the header.
#[test]
fn a_claim_is_the_one_the_check_takes() {
    let told = |facts: &Facts| {
        let mut found = Vec::new();
        check(facts, &Limits::DEFAULT, &mut found);
        !found.is_empty()
    };
    let claimed = |facts: &Facts| facts.claim().is_some();
    for header in [0, 300] {
        for (at, taken) in [(1022, true), (1023, true), (1024, false), (1025, false)] {
            let mut facts = table();
            facts.header = header;
            dict(&mut facts).at = header + at;
            // `/L` the file's from the header, so a claim taken is linearized,
            // and the dictionary run past 1024 bytes, said whenever it's taken.
            dict(&mut facts).l = Raw::Integer((facts.len - header) as i64);
            dict(&mut facts).end = header + 1100;
            assert_eq!(claimed(&facts), taken, "{header} {at}");
            assert_eq!(told(&facts), taken, "{header} {at}");
            assert_eq!(
                check(&facts, &Limits::DEFAULT, &mut ()).linearized,
                taken,
                "{header} {at}"
            );
        }
    }
    let mut facts = table();
    assert!(claimed(&facts));
    dict(&mut facts).stream = true;
    assert!(!claimed(&facts) && !told(&facts));
    let mut facts = table();
    dict(&mut facts).linearized = Raw::Absent;
    assert!(!claimed(&facts) && !told(&facts));
    // A version the check doesn't take is a claim all the same, and told.
    dict(&mut facts).linearized = Raw::Integer(2);
    assert!(claimed(&facts) && told(&facts));
    facts.first_object = None;
    assert!(!claimed(&facts));
}

/// The version a caller reads (`FirstObject::version_one`) is the one the
/// check takes: a number whose integer part is 1, and nothing else, where
/// anything else is `Version`.
#[test]
fn the_version_one_is_the_one_the_check_takes() {
    for (raw, one) in [
        (Raw::Integer(1), true),
        (Raw::Real(1.0), true),
        (Raw::Real(1.9), true),
        (Raw::Real(0.9), false),
        (Raw::Integer(2), false),
        (Raw::Real(2.0), false),
        (Raw::Integer(0), false),
        (Raw::Other, false),
        (Raw::Reference(ObjectId::new(1, 0)), false),
    ] {
        let mut facts = table();
        dict(&mut facts).linearized = raw;
        assert_eq!(dict(&mut facts).version_one(), one, "{raw:?}");
        let told = findings(&facts).contains(&Finding::Version);
        assert_eq!(told, !one, "{raw:?}");
    }
}

/// The hint stream's extent a caller places a position by
/// (`Facts::hint_extent`) is the one the check places it by: `/H`'s offset
/// and length where the stream listed there takes that length, else the
/// most it takes, to where the next thing listed starts, which the check's
/// `HintExtent` says too: of a number listed twice its first row, a hybrid's
/// stream's before its table's, a table section's `xref` and the file's end
/// among what's listed.
#[test]
fn the_hint_extent_is_the_one_the_check_places_positions_by() {
    let row = |number, at| RowFacts::new(number, 0, RowEntry::InUse(at));
    let int = Raw::Integer;
    // A file of 1,000 bytes, its hint stream 2 from 100 to 140, object 5 next,
    // at 300.
    let mut facts = Facts::new(1000);
    let mut first = FirstObject::new(9, 90, 1, 0);
    first.linearized = int(1);
    first.l = int(1000);
    first.h = Raw::Array;
    facts.first_object = Some(first);
    let mut section = SectionFacts::new(950, Kind::Table);
    section.rows = vec![row(1, 9), row(2, 100), row(5, 300)];
    facts.first = Some(section);
    let mut main = SectionFacts::new(900, Kind::Stream);
    main.rows = vec![row(3, 500)];
    facts.main = Some(main);
    let data = HintData::Decoded(Vec::new());
    facts.hint = Some(HintFacts::new(100, 140, 2, data));
    let h = |facts: &mut Facts, items: &[Raw]| {
        dict(facts).h_items = items.to_vec();
    };
    for (items, placed) in [
        (vec![int(100), int(40)], (100, 40)),
        (vec![int(100), int(200)], (100, 200)),
        (vec![int(100), int(39)], (100, 200)),
        (vec![int(100), int(201)], (100, 200)),
        (vec![int(100), int(-40)], (100, 200)),
        (vec![int(100), Raw::Real(40.0)], (100, 200)),
        (vec![int(100)], (100, 200)),
        // No stream listed where `/H` says, or its offset no integer: as it
        // says, 0 for what isn't one.
        (vec![int(99), int(7)], (99, 7)),
        (vec![Raw::Real(100.0), int(7)], (0, 7)),
    ] {
        h(&mut facts, &items);
        assert_eq!(facts.hint_extent(), placed, "{items:?}");
    }
    let most = |facts: &mut Facts| {
        h(facts, &[int(100), int(999)]);
        let (_, most) = facts.hint_extent();
        let told = findings(facts).iter().find_map(|finding| match *finding {
            Finding::HintExtent { most, .. } => Some(most),
            _ => None,
        });
        assert_eq!(Some(most), told);
        most
    };
    assert_eq!(most(&mut facts), 200);
    // 5 listed again, nearer, by the main section: its first row stands.
    main_section(&mut facts).rows.push(row(5, 150));
    assert_eq!(most(&mut facts), 200);
    // A hybrid's stream listing 4 where its table has it free: the stream's
    // row stands.
    let section = first_section(&mut facts);
    section.hybrid = vec![row(4, 250)];
    section.rows.push(RowFacts::new(4, 0, RowEntry::Free));
    assert_eq!(most(&mut facts), 150);
    // The main section a table at 200, before them.
    let main = main_section(&mut facts);
    (main.at, main.kind) = (200, Kind::Table);
    assert_eq!(most(&mut facts), 100);
    // 6 right at the stream's end: it takes its own bytes, no more.
    first_section(&mut facts).rows.push(row(6, 140));
    assert_eq!(most(&mut facts), 40);
    // Nothing listed after the stream, no table after it: to the end.
    let section = first_section(&mut facts);
    (section.kind, section.rows, section.hybrid) = (Kind::Stream, vec![row(2, 100)], vec![]);
    facts.main = None;
    assert_eq!(most(&mut facts), 900);
    // No first object: nothing's said of /H, and 0 for both.
    facts.first_object = None;
    assert_eq!(facts.hint_extent(), (0, 0));
}

/// The shared object table's first object, placed wrong, says where it is,
/// or that nothing lists it in use.
#[test]
fn the_shared_tables_first_object_says_where_it_is() {
    let at = |computed| {
        let finding = Finding::SharedFirstAt {
            number: 7,
            hint: 100,
            computed,
        };
        finding.to_string()
    };
    assert_eq!(
        at(Some(90)),
        "the shared object table has its first object past the first page, 7, at 100, where it's at 90 as if the hint stream weren't there (ISO 32000-2, F.4.3)"
    );
    assert_eq!(
        at(None),
        "the shared object table has its first object past the first page, 7, at 100, where nothing lists it in use (ISO 32000-2, F.4.3)"
    );
}

#[test]
fn sections_missing_are_told() {
    fires(
        table(),
        |facts| facts.first = None,
        |f| *f == Finding::NoSections,
    );
    fires(
        table(),
        |facts| facts.main = None,
        |f| *f == Finding::NoMainSection,
    );
}

#[test]
fn the_first_sections_shape_is_told() {
    fires(
        table(),
        |facts| first_section(facts).subsections.push((90, 1)),
        |f| *f == Finding::FirstSubsections { count: 2 },
    );
    fires(
        table(),
        |facts| first_section(facts).rows[1].entry = RowEntry::Free,
        |f| matches!(f, Finding::FirstFree { .. }),
    );
    fires(
        table(),
        |facts| first_section(facts).size = Raw::Integer(1),
        |f| matches!(f, Finding::FirstSize { size: 1, .. }),
    );
    fires(
        table(),
        |facts| first_section(facts).size = Raw::Absent,
        |f| matches!(f, Finding::FirstSize { size: -1, .. }),
    );
    fires(
        table(),
        |facts| first_section(facts).root = Raw::Other,
        |f| *f == Finding::FirstNoRoot,
    );
    fires(
        table(),
        |facts| first_section(facts).rows.rotate_left(1),
        |f| matches!(f, Finding::FirstNotDictionary { .. }),
    );
    fires(
        table(),
        |facts| first_section(facts).at = main_section(facts).at + 1,
        |f| matches!(f, Finding::SectionOrder { .. }),
    );
}

#[test]
fn a_hybrids_stream_says_whats_free_isnt() {
    let mut facts = table();
    let row = first_section(&mut facts).rows[1];
    first_section(&mut facts).rows[1].entry = RowEntry::Free;
    first_section(&mut facts).hybrid = vec![row];
    assert!(!(findings(&facts).iter()).any(|f| matches!(f, Finding::FirstFree { .. })));
    let mut facts = table();
    let row = main_section(&mut facts).rows[1];
    main_section(&mut facts).rows[1].entry = RowEntry::Free;
    main_section(&mut facts).hybrid = vec![row];
    assert!(!(findings(&facts).iter()).any(|f| matches!(f, Finding::MainFree { .. })));
}

#[test]
fn the_main_sections_shape_is_told() {
    fires(
        table(),
        |facts| main_section(facts).subsections.push((90, 1)),
        |f| *f == Finding::MainSubsections { count: 2 },
    );
    fires(
        table(),
        |facts| main_section(facts).subsections[0].0 = 1,
        |f| *f == Finding::MainFrom { from: 1 },
    );
    fires(
        table(),
        |facts| main_section(facts).rows[0].entry = RowEntry::InUse(9),
        |f| *f == Finding::MainZeroInUse,
    );
    fires(
        table(),
        |facts| main_section(facts).rows[1].entry = RowEntry::Free,
        |f| matches!(f, Finding::MainFree { .. }),
    );
    fires(
        table(),
        |facts| main_section(facts).prev = Raw::Integer(9),
        |f| *f == Finding::MainPrev,
    );
    fires(
        table(),
        |facts| main_section(facts).startxref = Some(9),
        |f| matches!(f, Finding::LastStartxref { value: 9, .. }),
    );
    fires(
        table(),
        |facts| main_section(facts).startxref = None,
        |f| matches!(f, Finding::LastStartxref { value: 0, .. }),
    );
}

#[test]
fn numbers_are_told() {
    fires(
        table(),
        |facts| main_section(facts).rows[1].generation = 1,
        |f| matches!(f, Finding::Generation { generation: 1, .. }),
    );
    fires(
        table(),
        |facts| {
            let at = first_section(facts).rows.len() as u32 + 100;
            for row in &mut first_section(facts).rows {
                row.number += at;
            }
            hint_of(facts).number += at;
        },
        |f| matches!(f, Finding::GroupsApart { .. }),
    );
    fires(
        table(),
        |facts| {
            let rows = &mut main_section(facts).rows;
            let (a, b) = (rows[1].entry, rows[2].entry);
            rows[1].entry = b;
            rows[2].entry = a;
        },
        |f| matches!(f, Finding::NumberOutOfOrder { .. }),
    );
    fires(
        table(),
        |facts| {
            let rows = &mut main_section(facts).rows;
            let stream = rows[1].number;
            rows[2].entry = RowEntry::Compressed { stream, index: 0 };
        },
        |f| matches!(f, Finding::UncompressedAfterCompressed { .. }),
    );
}

#[test]
fn a_sections_stream_is_where_it_is_whatever_its_number() {
    let mut facts = laid_out(Kind::Stream);
    let stream = first_section(&mut facts).stream.unwrap();
    let rows = &mut first_section(&mut facts).rows;
    let row = rows.iter_mut().find(|row| row.number == stream).unwrap();
    row.entry = RowEntry::InUse(u64::from(u32::MAX));
    let order = |f: &Finding| matches!(f, Finding::NumberOutOfOrder { .. });
    assert!(!findings(&facts).iter().any(order));
    first_section(&mut facts).stream = None;
    assert!(findings(&facts).iter().any(order));
}

#[test]
fn the_hint_stream_and_its_length_are_last_wherever_theyre_written() {
    let mut facts = table();
    let hint = hint_of(&mut facts).number;
    let first = first_section(&mut facts);
    let at = first.rows.len() - 1;
    let RowEntry::InUse(place) = first.rows[at].entry else {
        panic!("in use");
    };
    // An object numbered past it, as an indirect /Length written after it is,
    // within what /H says.
    first
        .rows
        .push(RowFacts::new(hint + 1, 0, RowEntry::InUse(place + 1)));
    first.size = Raw::Integer(match first.size {
        Raw::Integer(size) => size + 1,
        _ => 0,
    });
    assert_eq!(findings(&facts), []);
    let first = first_section(&mut facts);
    let last = first.rows.len() - 1;
    first.rows[last].entry = RowEntry::InUse(9);
    fires(facts, |_| {}, |f| matches!(f, Finding::HintNotLast { .. }));
}

#[test]
fn whats_packed_that_cant_be_is_told() {
    let facts = table();
    let Some(Raw::Reference(ObjectId { number, .. })) =
        facts.first.as_ref().map(|first| first.root)
    else {
        panic!("/Root");
    };
    fires(
        facts,
        |facts| {
            let first = first_section(facts);
            let row = first
                .rows
                .iter_mut()
                .find(|row| row.number == number)
                .unwrap();
            row.entry = RowEntry::Compressed {
                stream: 1,
                index: 0,
            };
        },
        |f| *f == Finding::CatalogCompressed { number },
    );
    fires(
        table(),
        |facts| first_section(facts).root = Raw::Reference(ObjectId::new(1, 0)),
        |f| *f == Finding::CatalogNotFirst { number: 1 },
    );
    let mut facts = table();
    let ObjectId { number, .. } = facts.pages[1].id.unwrap();
    let main = main_section(&mut facts);
    let row = main
        .rows
        .iter_mut()
        .find(|row| row.number == number)
        .unwrap();
    row.entry = RowEntry::Compressed {
        stream: 1,
        index: 0,
    };
    fires(
        facts,
        |_| {},
        |f| *f == Finding::PageCompressed { page: 1, number },
    );
}

#[test]
fn the_hint_stream_is_told_of() {
    fires(
        table(),
        |facts| hint_of(facts).data = HintData::NotAStream,
        |f| *f == Finding::HintNotAStream,
    );
    fires(
        table(),
        |facts| hint_of(facts).data = HintData::Undecodable,
        |f| *f == Finding::HintUndecodable,
    );
    fires(
        table(),
        |facts| hint_of(facts).keys.retain(|(key, _)| key != b"S"),
        |f| *f == Finding::HintNoShared,
    );
    fires(
        table(),
        |facts| hint_of(facts).keys[0].1 = Raw::Integer(-1),
        |f| matches!(f, Finding::HintKeyValue { key } if key.as_bytes() == b"S"),
    );
    fires(
        table(),
        |facts| hint_of(facts).keys[0].1 = Raw::Reference(ObjectId::new(1, 0)),
        |f| matches!(f, Finding::HintKeyIndirect { key } if key.as_bytes() == b"S"),
    );
    fires(
        table(),
        |facts| hint_of(facts).keys.push((b"P".to_vec(), Raw::Integer(1))),
        |f| matches!(f, Finding::HintKeyOther { key } if key.as_bytes() == b"P"),
    );
}

#[test]
fn an_indirect_length_is_no_finding() {
    let mut facts = table();
    let keys = &mut hint_of(&mut facts).keys;
    let length = keys.iter_mut().find(|(key, _)| key == b"Length").unwrap();
    length.1 = Raw::Reference(ObjectId::new(1, 0));
    assert_eq!(findings(&facts), []);
}

const WHYS: [Unread; 3] = [Unread::Locked, Unread::Capped, Unread::Unfiltered];

/// A hint stream the reader didn't read, for each of its reasons, is said once
/// and not judged: nothing of its data or tables, and the file holds by the
/// rest, its hints not read.
#[test]
fn a_hint_stream_the_reader_didnt_read_is_said_and_nothing_of_its_data() {
    for why in WHYS {
        let mut facts = table();
        hint_of(&mut facts).data = HintData::Unread(why);
        assert_eq!(findings(&facts), [Finding::HintUnread { why }]);
        let verdict = check(&facts, &Limits::DEFAULT, &mut ());
        assert!(verdict.linearized && verdict.holds && !verdict.hints_read);
        assert_eq!(Finding::HintUnread { why }.grade(), Grade::Unjudged);
        // Of its dictionary, what's there is judged.
        hint_of(&mut facts).keys.retain(|(key, _)| key != b"S");
        assert_eq!(
            findings(&facts),
            [Finding::HintUnread { why }, Finding::HintNoShared]
        );
    }
    // Its own object unread, its end at its start: nothing's said of its
    // dictionary, nor of its length beside /H's, which stands.
    let mut facts = table();
    let hint = hint_of(&mut facts);
    hint.data = HintData::Unread(Unread::Capped);
    hint.end = hint.at;
    hint.keys.clear();
    let why = Unread::Capped;
    assert_eq!(findings(&facts), [Finding::HintUnread { why }]);
    assert_eq!(facts.hint_extent(), table().hint_extent());
    // The page tree's own say is the facts', not the hint stream's.
    let mut facts = table();
    let why = Unread::Locked;
    hint_of(&mut facts).data = HintData::Unread(why);
    facts.pages.clear();
    assert_eq!(
        findings(&facts),
        [Finding::HintUnread { why }, Finding::NoPages]
    );
    facts.unread.pages = Some(why);
    assert_eq!(
        findings(&facts),
        [Finding::HintUnread { why }, Finding::PagesUnread { why }]
    );
}

/// Sections the reader stopped short of the main one, for each of its reasons,
/// are said once, and nothing they list or place is judged, nor a page tree
/// that gives no pages, whose objects may be among what they'd list: only
/// what's of the first section alone, and of the hint stream and its tables.
#[test]
fn sections_the_reader_didnt_read_are_said_and_nothing_they_place() {
    for why in WHYS {
        let mut facts = table();
        facts.main = None;
        facts.unread.sections = Some(why);
        assert_eq!(findings(&facts), [Finding::SectionsUnread { why }]);
        let verdict = check(&facts, &Limits::DEFAULT, &mut ());
        assert!(verdict.linearized && verdict.holds && verdict.hints_read);
        facts.pages.clear();
        assert_eq!(findings(&facts), [Finding::SectionsUnread { why }]);
        facts.first = None;
        assert_eq!(findings(&facts), [Finding::SectionsUnread { why }]);
    }
    // Of what's placed: a first page's length and each position, wrong.
    let mut facts = table();
    with_tables(&mut facts, |tables| tables.pages.pages[0].length += 1);
    first_section(&mut facts).subsections.push((90, 1));
    dict(&mut facts).e = Raw::Integer(1);
    let placed = findings(&facts);
    assert!(placed.len() > 2, "{placed:?}");
    facts.main = None;
    facts.unread.sections = Some(Unread::Capped);
    let why = Unread::Capped;
    assert_eq!(
        findings(&facts),
        [
            Finding::SectionsUnread { why },
            Finding::FirstSubsections { count: 2 }
        ]
    );
}

/// A page tree the reader didn't read, for each of its reasons, is said once,
/// and nothing of its pages or nodes is judged.
#[test]
fn a_page_tree_the_reader_didnt_read_is_said_and_nothing_of_its_pages() {
    for why in WHYS {
        let mut facts = table();
        facts.pages.clear();
        facts.nodes.push((2, Attribute::Rotate));
        facts.unread.pages = Some(why);
        assert_eq!(findings(&facts), [Finding::PagesUnread { why }]);
        let verdict = check(&facts, &Limits::DEFAULT, &mut ());
        assert!(verdict.linearized && verdict.holds && verdict.hints_read);
    }
}

#[test]
fn hint_tables_that_dont_read_are_told_never_a_panic() {
    fires(
        table(),
        |facts| hint_of(facts).data = HintData::Decoded(vec![0; 8]),
        |f| matches!(f, Finding::HintTables { .. }),
    );
    let mut facts = table();
    hint_of(&mut facts).data = HintData::Decoded(vec![0; 8]);
    assert!(!check(&facts, &Limits::DEFAULT, &mut ()).hints_read);
    // A width past 32.
    let mut facts = table();
    let HintData::Decoded(data) = &mut hint_of(&mut facts).data else {
        panic!("decoded");
    };
    data[9] = 200;
    let found = findings(&facts);
    assert!(
        found.iter().any(|f| matches!(
            f,
            Finding::HintTables {
                fault: crate::hint::Error::Width { .. }
            }
        )),
        "{found:?}"
    );
    // /N past what the data can hold, past the cap.
    let mut limits = Limits::DEFAULT;
    limits.pages = 1;
    let mut told = Vec::new();
    let facts = table();
    let verdict = check(&facts, &limits, &mut told);
    assert!(!verdict.hints_read && !verdict.holds);
    assert!(matches!(
        told[0],
        Finding::HintTables {
            fault: crate::hint::Error::TooManyPages { .. }
        }
    ));
}

#[test]
fn a_page_without_its_attributes_or_a_node_with_them_is_said() {
    fires(
        table(),
        |facts| facts.pages[0].resources = false,
        |f| {
            *f == Finding::PageAttribute {
                page: 0,
                attribute: Attribute::Resources,
            }
        },
    );
    fires(
        table(),
        |facts| facts.pages[1].media_box = false,
        |f| {
            *f == Finding::PageAttribute {
                page: 1,
                attribute: Attribute::MediaBox,
            }
        },
    );
    fires(
        table(),
        |facts| facts.nodes.push((2, Attribute::Rotate)),
        |f| {
            *f == Finding::NodeAttribute {
                number: 2,
                attribute: Attribute::Rotate,
            }
        },
    );
    fires(
        table(),
        |facts| facts.pages.clear(),
        |f| *f == Finding::NoPages,
    );
}

/// Each finding's place and clause, one of each: its message cites the clause,
/// and says the number its place gives.
#[test]
fn a_finding_says_where_and_by_what_clause_apart_from_its_message() {
    use crate::check::Place::*;
    use crate::hint::Error as Fault;
    let table = "O";
    let key = KeyName::new(b"X");
    let attribute = Attribute::Rotate;
    let all: Vec<(Finding, Option<Place>)> = vec![
        (Finding::Version, Some(Key(DictKey::Linearized))),
        (
            Finding::LengthNotFile { l: 1, len: 2 },
            Some(Key(DictKey::L)),
        ),
        (Finding::Updated { at: 77 }, Some(Byte(77))),
        (Finding::BadValue { key: DictKey::P }, Some(Key(DictKey::P))),
        (Finding::Indirect { key: DictKey::O }, Some(Key(DictKey::O))),
        (Finding::DictionaryPast1024 { end: 1025 }, Some(Byte(1025))),
        (Finding::HintItems { items: 3 }, Some(Key(DictKey::H))),
        (
            Finding::FirstPageObject {
                o: 1,
                page: 77,
                computed: 2,
            },
            Some(Key(DictKey::O)),
        ),
        (Finding::PageCount { n: 1, pages: 2 }, Some(Key(DictKey::N))),
        (
            Finding::FirstPageEnd {
                e: 1,
                least: 2,
                most: 3,
            },
            Some(Key(DictKey::E)),
        ),
        (
            Finding::MainFirstEntry {
                t: 1,
                least: 2,
                most: 3,
            },
            Some(Key(DictKey::T)),
        ),
        (Finding::HintNotListed { at: 1 }, Some(Key(DictKey::H))),
        (
            Finding::HintExtent {
                at: 77,
                length: 1,
                least: 2,
                most: 3,
            },
            Some(Key(DictKey::H)),
        ),
        (
            Finding::SectionsUnread {
                why: Unread::Capped,
            },
            None,
        ),
        (Finding::NoSections, None),
        (Finding::NoMainSection, None),
        (Finding::SectionOrder { first: 77, main: 2 }, Some(Byte(77))),
        (Finding::FirstSubsections { count: 2 }, None),
        (Finding::FirstFree { number: 77 }, Some(Object(77))),
        (Finding::FirstSize { size: 1, total: 2 }, None),
        (Finding::FirstNoRoot, None),
        (
            Finding::FirstNotDictionary {
                number: 77,
                dictionary: 2,
            },
            Some(Object(77)),
        ),
        (Finding::MainSubsections { count: 2 }, None),
        (Finding::MainFrom { from: 1 }, None),
        (Finding::MainZeroInUse, Some(Object(0))),
        (Finding::MainFree { number: 77 }, Some(Object(77))),
        (Finding::MainPrev, None),
        (
            Finding::LastStartxref {
                value: 77,
                first: 2,
            },
            Some(Byte(77)),
        ),
        (Finding::CatalogCompressed { number: 77 }, Some(Object(77))),
        (Finding::CatalogNotFirst { number: 77 }, Some(Object(77))),
        (
            Finding::PageCompressed {
                page: 77,
                number: 2,
            },
            Some(Object(2)),
        ),
        (
            Finding::UncompressedAfterCompressed { number: 77 },
            Some(Object(77)),
        ),
        (
            Finding::Generation {
                number: 77,
                generation: 1,
            },
            Some(Object(77)),
        ),
        (
            Finding::GroupsApart { first: 77, next: 2 },
            Some(Object(77)),
        ),
        (Finding::NumberOutOfOrder { number: 77 }, Some(Object(77))),
        (Finding::HintNotLast { hint: 77, last: 2 }, Some(Object(77))),
        (
            Finding::HintUnread {
                why: Unread::Locked,
            },
            None,
        ),
        (Finding::HintNotAStream, None),
        (Finding::HintUndecodable, None),
        (Finding::HintNoShared, None),
        (Finding::HintKeyValue { key }, Some(HintKey(key))),
        (Finding::HintKeyIndirect { key }, Some(HintKey(key))),
        (Finding::HintKeyOther { key }, Some(HintKey(key))),
        (
            Finding::HintTables {
                fault: Fault::PastEnd {
                    what: "a",
                    bit: 77,
                    bits: 2,
                },
            },
            Some(HintBit(77)),
        ),
        (
            Finding::HintTables {
                fault: Fault::TableAt {
                    table,
                    at: 1,
                    len: 2,
                },
            },
            Some(Table(table)),
        ),
        (
            Finding::HintTables {
                fault: Fault::Width {
                    what: "a",
                    width: 33,
                },
            },
            None,
        ),
        (
            Finding::TableOverruns {
                table,
                end: 77,
                next: 2,
            },
            Some(HintByte(77)),
        ),
        (
            Finding::PageAttribute {
                page: 77,
                attribute,
            },
            Some(Page(77)),
        ),
        (
            Finding::NodeAttribute {
                number: 77,
                attribute,
            },
            Some(Object(77)),
        ),
        (
            Finding::PagesUnread {
                why: Unread::Unfiltered,
            },
            None,
        ),
        (Finding::NoPages, None),
        (Finding::FirstPageNotFirst { number: 77 }, Some(Object(77))),
        (
            Finding::FirstPageAt {
                hint: 1,
                computed: 77,
            },
            Some(Position(77)),
        ),
        (
            Finding::FirstPageNotFirstObject {
                hint: 1,
                computed: 77,
            },
            Some(Position(77)),
        ),
        (
            Finding::PageAt {
                page: 77,
                hint: 1,
                computed: 2,
            },
            Some(Page(77)),
        ),
        (
            Finding::PageObjects {
                page: 77,
                hint: 1,
                computed: 2,
            },
            Some(Page(77)),
        ),
        (
            Finding::PageLength {
                page: 77,
                hint: 1,
                computed: 2,
            },
            Some(Page(77)),
        ),
        (
            Finding::ContentPast {
                page: 77,
                end: 1,
                length: 2,
            },
            Some(Page(77)),
        ),
        (Finding::FirstPageShared { entries: 1 }, None),
        (
            Finding::SharedIdPast {
                page: 77,
                id: 1,
                entries: 2,
            },
            Some(Page(77)),
        ),
        (Finding::SharedCounts { first: 1, total: 2 }, None),
        (
            Finding::SharedFirstAt {
                number: 77,
                hint: 1,
                computed: None,
            },
            Some(Object(77)),
        ),
        (
            Finding::SharedObjects {
                entry: 77,
                hint: 1,
                computed: 2,
            },
            Some(SharedEntry(77)),
        ),
        (
            Finding::SharedLength {
                entry: 77,
                hint: 1,
                computed: 2,
            },
            Some(SharedEntry(77)),
        ),
        (Finding::Signature { entry: 77 }, Some(SharedEntry(77))),
        (Finding::TableObject { table, number: 77 }, Some(Object(77))),
        (
            Finding::TableAt {
                table,
                hint: 1,
                computed: 77,
            },
            Some(Position(77)),
        ),
        (
            Finding::TableObjects {
                table,
                hint: 1,
                computed: 2,
            },
            Some(Table(table)),
        ),
        (
            Finding::TableLength {
                table,
                hint: 1,
                computed: 2,
            },
            Some(Table(table)),
        ),
        (
            Finding::TableIdPast {
                table,
                id: 1,
                entries: 2,
            },
            Some(Table(table)),
        ),
        (
            Finding::ThumbnailPages { said: 1, pages: 2 },
            Some(Table("T")),
        ),
        (
            Finding::GraphRefused {
                why: crate::plan::Error::NoPages,
            },
            None,
        ),
        (Finding::FirstPageReaches { number: 77 }, Some(Object(77))),
        (
            Finding::PageReaches {
                page: 1,
                number: 77,
            },
            Some(Object(77)),
        ),
        (
            Finding::SharedNotListed { page: 77, entry: 2 },
            Some(Page(77)),
        ),
        (
            Finding::SharedNotReached { page: 77, entry: 2 },
            Some(Page(77)),
        ),
    ];
    for (finding, place) in all {
        assert_eq!(finding.place(), place, "{finding}");
        let said = finding.to_string();
        assert!(
            said.contains(finding.clause()),
            "{said}: {}",
            finding.clause()
        );
        match place {
            Some(Page(n) | SharedEntry(n) | Byte(n) | Position(n) | HintByte(n) | HintBit(n)) => {
                assert!(said.contains(&n.to_string()), "{said}");
            }
            Some(Object(n)) if n != 0 => assert!(said.contains(&n.to_string()), "{said}"),
            _ => {}
        }
    }
}

#[test]
fn a_first_page_past_the_pages_is_told() {
    let bad = |f: &Finding| *f == Finding::BadValue { key: DictKey::P };
    for p in [2, 9] {
        let mut facts = table();
        let first = facts.first_object.as_mut().unwrap();
        first.n = Raw::Integer(2);
        first.p = Raw::Integer(p);
        let found = findings(&facts);
        assert!(found.iter().any(bad), "{found:?}");
        let verdict = check(&facts, &Limits::DEFAULT, &mut ());
        assert!(verdict.linearized && !verdict.holds);
    }
    let mut facts = table();
    let first = facts.first_object.as_mut().unwrap();
    first.n = Raw::Integer(2);
    first.p = Raw::Integer(1);
    assert!(!findings(&facts).iter().any(bad));
}

#[test]
fn the_first_pages_object_not_listed_first_is_told() {
    let mut facts = table();
    let ObjectId { number, .. } = facts.pages[0].id.unwrap();
    let first = first_section(&mut facts);
    let at = first
        .rows
        .iter()
        .position(|row| row.number == number)
        .unwrap();
    let row = first.rows.remove(at);
    first.size = Raw::Integer(match first.size {
        Raw::Integer(size) => size - 1,
        _ => 0,
    });
    main_section(&mut facts).rows.push(row);
    let found = findings(&facts);
    assert!(
        found.contains(&Finding::FirstPageNotFirst { number }),
        "{found:?}"
    );
    assert!(!check(&facts, &Limits::DEFAULT, &mut ()).holds);
}

#[test]
fn the_page_tables_first_page_is_told() {
    let mut first = 0;
    fires(
        table(),
        |facts| {
            with_tables(facts, |tables| {
                first = tables.pages.header.first_page_at;
                tables.pages.header.first_page_at += 1;
            });
        },
        |f| matches!(f, Finding::FirstPageAt { .. }),
    );
    assert_ne!(first, 0);
    let mut facts = table();
    let first_page_object = match dict(&mut facts).o {
        Raw::Integer(o) => o as u32,
        _ => panic!("/O"),
    };
    // An object of its section ahead of the page object, the entry's start.
    let page = (first_section(&mut facts).rows.iter())
        .position(|row| row.number == first_page_object)
        .unwrap();
    let rows = &mut first_section(&mut facts).rows;
    let (a, b) = (rows[page].entry, rows[page + 1].entry);
    rows[page].entry = b;
    rows[page + 1].entry = a;
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::FirstPageNotFirstObject { .. })),
        "{found:?}"
    );
    assert!(!(found.iter()).any(|f| matches!(f, Finding::FirstPageAt { .. })));
}

#[test]
fn a_length_off_is_told_of_that_page_alone() {
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.pages.pages[1].length += 1;
        tables.pages.header.min_length = 0;
        tables.pages.header.length_bits = 32;
    });
    let found = findings(&facts);
    let of_page: Vec<_> = (found.iter())
        .filter(|f| matches!(f, Finding::PageLength { .. } | Finding::PageAt { .. }))
        .collect();
    assert!(
        matches!(
            of_page[..],
            [
                Finding::PageLength { page: 1, .. },
                Finding::PageAt { page: 2, .. }
            ]
        ),
        "{found:?}"
    );
}

#[test]
fn a_count_off_is_told() {
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.pages.pages[1].objects += 1;
        tables.pages.header.min_objects = 0;
        tables.pages.header.objects_bits = 32;
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::PageObjects { page: 1, .. })),
        "{found:?}"
    );
}

#[test]
fn a_shared_entry_past_the_table_is_told() {
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.pages.pages[1].shared[0].0 = tables.shared.header.entries;
        tables.pages.header.id_bits = 16;
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::SharedIdPast { page: 1, .. })),
        "{found:?}"
    );
    let verdict = check(&facts, &Limits::DEFAULT, &mut ());
    assert!(!verdict.holds);
    // Under the width the table says: wide enough to be read.
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.form.as_mut().unwrap().shared = vec![1000];
        tables.form.as_mut().unwrap().id_bits = 16;
    });
    let found = findings(&facts);
    assert!(
        found.iter().any(|f| matches!(
            f,
            Finding::TableIdPast {
                table: "/V",
                id: 1000,
                ..
            }
        )),
        "{found:?}"
    );
}

#[test]
fn the_first_page_with_shared_entries_is_said() {
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.pages.pages[0].shared.push((0, 0));
        tables.pages.header.count_bits = 8;
        tables.pages.header.id_bits = 8;
    });
    let found = findings(&facts);
    assert!(
        found.contains(&Finding::FirstPageShared { entries: 1 }),
        "{found:?}"
    );
    assert_eq!(
        Finding::FirstPageShared { entries: 1 }.grade(),
        Grade::Departure
    );
    assert!(check(&facts, &Limits::DEFAULT, &mut ()).holds);
}

#[test]
fn content_past_a_page_is_said() {
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.pages.pages[0].content_length = u64::from(u32::MAX);
        tables.pages.header.min_content_length = 0;
        tables.pages.header.content_length_bits = 32;
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::ContentPast { page: 0, .. })),
        "{found:?}"
    );
    // A position and a length, which a reader taking the content goes by.
    assert!(!check(&facts, &Limits::DEFAULT, &mut ()).holds);
}

#[test]
fn the_shared_table_is_held_to_where_its_objects_are() {
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.shared.header.first_page_entries = tables.shared.header.entries + 1;
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::SharedCounts { .. })),
        "{found:?}"
    );
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.shared.header.first_at += 5000;
    });
    let found = findings(&facts);
    let at: Vec<_> = (found.iter())
        .filter(|f| {
            matches!(
                f,
                Finding::SharedFirstAt { .. }
                    | Finding::SharedLength { .. }
                    | Finding::SharedObjects { .. }
            )
        })
        .collect();
    // Said of its first entry, and no other's.
    assert!(
        matches!(at[..], [Finding::SharedFirstAt { .. }]),
        "{found:?}"
    );
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.shared.groups.last_mut().unwrap().length += 1;
        tables.shared.header.min_length = 0;
        tables.shared.header.length_bits = 32;
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::SharedLength { .. })),
        "{found:?}"
    );
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        let last = tables.shared.groups.last_mut().unwrap();
        last.objects = 3;
        tables.shared.header.objects_bits = 16;
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::SharedObjects { .. })),
        "{found:?}"
    );
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.shared.groups[0].signature = Some([7; 16]);
    });
    let found = findings(&facts);
    assert!(
        found.contains(&Finding::Signature { entry: 0 }),
        "{found:?}"
    );
    assert!(check(&facts, &Limits::DEFAULT, &mut ()).holds);
}

#[test]
fn a_generic_table_is_held_to_where_its_objects_are() {
    let change = |change: fn(&mut crate::hint::Generic)| {
        let mut facts = table();
        with_tables(&mut facts, |tables| {
            change(&mut tables.form.as_mut().unwrap().generic);
        });
        findings(&facts)
    };
    let found = change(|g| g.first_at += 1);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::TableAt { table: "/V", .. })),
        "{found:?}"
    );
    let found = change(|g| g.first_object = 9000);
    assert!(
        found.iter().any(|f| matches!(
            f,
            Finding::TableObject {
                table: "/V",
                number: 9000
            }
        )),
        "{found:?}"
    );
    let found = change(|g| g.objects = 5);
    assert!(
        found.iter().any(|f| matches!(
            f,
            Finding::TableObjects {
                table: "/V",
                hint: 5,
                ..
            }
        )),
        "{found:?}"
    );
    let found = change(|g| g.length += 1);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::TableLength { table: "/V", .. })),
        "{found:?}"
    );
    // A table of nothing says nothing.
    let found = change(|g| *g = crate::hint::Generic::default());
    assert_eq!(found, []);
    // A count of none, where its length holds what's there: the count's said,
    // once.
    let found = change(|g| g.objects = 0);
    assert!(
        matches!(
            found[..],
            [Finding::TableObjects {
                table: "/V",
                hint: 0,
                computed: 1..,
            }]
        ),
        "{found:?}"
    );
    // Held from where its object is, wherever the table has it.
    let found = change(|g| {
        g.objects = 0;
        g.first_at = u32::MAX - 1;
    });
    assert!(
        matches!(found[..], [Finding::TableObjects { hint: 0, .. }]),
        "{found:?}"
    );
    // A length where nothing's there is the length's.
    let found = change(|g| {
        *g = crate::hint::Generic::default();
        g.first_object = 9000;
        g.first_at = u32::MAX - 1;
        g.length = 7;
    });
    assert!(
        matches!(
            found[..],
            [Finding::TableLength {
                table: "/V",
                hint: 7,
                computed: 0,
            }]
        ),
        "{found:?}"
    );
}

#[test]
fn thumbnails_are_held_to_the_pages_and_where_they_are() {
    let table_of = |entries: Vec<ThumbEntry>, first: u32, at: u32| {
        let mut facts = table();
        with_tables(&mut facts, |tables| {
            let shared = Generic::default();
            tables.thumbnails = Some(Thumbnails::of(first, at, entries, shared));
        });
        findings(&facts)
    };
    let entry = |gap: u32, objects: u64, length: u64| ThumbEntry {
        gap,
        objects,
        length,
    };
    let found = table_of(vec![entry(100, 0, 0)], 0, 0);
    assert!(
        found.contains(&Finding::ThumbnailPages {
            said: 101,
            pages: 4
        }),
        "{found:?}"
    );
    let found = table_of(vec![entry(0, 1, 9)], 9000, 0);
    assert!(
        found.contains(&Finding::TableObject {
            table: "/T",
            number: 9000
        }),
        "{found:?}"
    );
    // With nothing of its own, what thumbnails share is held all the same.
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        let shared = Generic {
            first_object: 9001,
            first_at: 0,
            objects: 1,
            length: 9,
        };
        tables.thumbnails = Some(Thumbnails::of(0, 0, Vec::new(), shared));
    });
    let found = findings(&facts);
    assert!(
        found.contains(&Finding::TableObject {
            table: "/T",
            number: 9001
        }),
        "{found:?}"
    );
}

#[test]
fn a_generic_tables_key_in_the_data_is_read_and_held() {
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        tables.outlines = Some(Generic {
            first_object: 9000,
            first_at: 0,
            objects: 1,
            length: 9,
        });
    });
    let found = findings(&facts);
    assert!(
        found.contains(&Finding::TableObject {
            table: "/O",
            number: 9000
        }),
        "{found:?}"
    );
}

#[test]
fn embedded_files_are_held_to_where_they_are_and_the_shared_table() {
    let mut facts = table();
    with_tables(&mut facts, |tables| {
        let group = FileEntry {
            number: 9000,
            objects: 1,
            length: 9,
            shared: vec![1],
        };
        let other = FileEntry {
            number: 9001,
            objects: 0,
            length: 0,
            shared: vec![7],
        };
        tables.pages.header.id_bits = 8;
        tables.files = Some(EmbeddedFiles::of(9000, 0, vec![group, other]));
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::TableObject { table: "/B", .. })),
        "{found:?}"
    );
    assert!(
        found.iter().any(|f| matches!(
            f,
            Finding::TableIdPast {
                table: "/B",
                id: 7,
                ..
            }
        )),
        "{found:?}"
    );
}

#[test]
fn a_table_overrun_is_said() {
    // Another page's entry in the table's columns runs into the next table,
    // which is still read from its own key.
    let mut facts = table();
    let Raw::Integer(pages) = dict(&mut facts).n else {
        panic!("/N");
    };
    dict(&mut facts).n = Raw::Integer(pages + 8);
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::TableOverruns { .. })),
        "{found:?}"
    );
}

/// The graph, in the numbers `facts` has its objects at: the same objects a
/// plan laid out, what each page reaches then being what it's held to.
fn numbered(kind: Kind) -> (Facts, Graph) {
    let graph = graph(kind);
    let planned = plan(&graph, &Limits::DEFAULT).unwrap();
    let written = graph_by(kind, |number| planned.number(id(number)).unwrap());
    (laid_out(kind), written)
}

#[test]
fn what_a_page_reaches_holds_where_the_graph_is_given() {
    for kind in [Kind::Table, Kind::Stream] {
        let (mut facts, graph) = numbered(kind);
        facts.graph = Some(graph);
        assert_eq!(findings(&facts), []);
    }
}

#[test]
fn a_list_missing_what_a_page_reaches_or_naming_what_it_doesnt_is_said() {
    let (mut facts, graph) = numbered(Kind::Table);
    facts.graph = Some(graph);
    with_tables(&mut facts, |tables| {
        tables.pages.pages[1].shared.clear();
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::SharedNotListed { page: 1, .. })),
        "{found:?}"
    );
    assert!(check(&facts, &Limits::DEFAULT, &mut ()).holds);
    let (mut facts, graph) = numbered(Kind::Table);
    facts.graph = Some(graph);
    with_tables(&mut facts, |tables| {
        tables.pages.pages[1].shared.push((0, 0));
        tables.pages.header.count_bits = 8;
        tables.pages.header.id_bits = 8;
    });
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::SharedNotReached { page: 1, entry: 0 })),
        "{found:?}"
    );
}

#[test]
fn a_wrong_first_shared_place_is_said_once_and_not_of_what_a_page_reaches() {
    let (mut facts, graph) = numbered(Kind::Table);
    facts.graph = Some(graph);
    with_tables(&mut facts, |tables| {
        tables.shared.header.first_at += 1000;
    });
    let found = findings(&facts);
    assert!(
        matches!(found[..], [Finding::SharedFirstAt { .. }]),
        "{found:?}"
    );
}

#[test]
fn an_object_out_of_its_pages_section_is_said() {
    let (mut facts, _) = numbered(Kind::Table);
    let planned = plan(&graph(Kind::Table), &Limits::DEFAULT).unwrap();
    let to = |number: u32| planned.number(id(number)).unwrap();
    // The first page reaches what no row of the first section lists, and a
    // later page what's elsewhere.
    let mut changed = Graph::new(to(1), Kind::Table);
    changed.object(to(1), [to(2), to(30)]);
    changed.object(to(2), [to(3), to(4), to(5), to(6)]);
    changed.object(to(30), []);
    changed.object(to(3), [to(13), to(20), to(30)]);
    changed.object(to(13), []);
    changed.object(to(4), [to(14), to(20), to(21), to(15)]);
    changed.object(to(14), []);
    changed.object(to(5), [to(20), to(21)]);
    changed.object(to(6), [to(16), to(20), to(21)]);
    changed.object(to(16), []);
    changed.object(to(15), []);
    changed.object(to(20), []);
    changed.object(to(21), []);
    for page in [3, 4, 5, 6] {
        changed.page(Page::new(to(page)));
    }
    changed.tree_node(to(2));
    changed.group(Group::Form, [to(30)]);
    facts.graph = Some(changed);
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::PageReaches { page: 1, .. })),
        "{found:?}"
    );
    assert_eq!(
        Finding::PageReaches { page: 1, number: 1 }.grade(),
        Grade::Departure
    );
    // What the first page reaches is first, here: this one's the main
    // section's.
    let (mut facts, graph) = numbered(Kind::Table);
    facts.graph = Some(graph);
    let first = (facts.first.as_ref().unwrap().rows.iter())
        .map(|row| row.number)
        .collect::<Vec<_>>();
    let moved = first[2];
    let first = first_section(&mut facts);
    let row = first.rows.remove(2);
    first.size = Raw::Integer(match first.size {
        Raw::Integer(size) => size - 1,
        _ => 0,
    });
    main_section(&mut facts).rows.push(row);
    let found = findings(&facts);
    assert!(
        found.contains(&Finding::FirstPageReaches { number: moved }),
        "{found:?}"
    );
}

#[test]
fn a_member_of_a_shared_object_stream_is_in_its_entry() {
    // What only one page reaches, in an object stream it shares with another
    // page's: part 8 has the stream, the page's list names it, F.3.1.
    let mut packed = Graph::new(id(1), Kind::Stream);
    packed.object(id(1), [id(2)]);
    packed.object(id(2), [id(3), id(4), id(5)]);
    packed.object(id(3), [id(13)]);
    packed.object(id(13), []);
    packed.object(id(4), [id(14)]);
    packed.object(id(14), []).packable(true);
    packed.object(id(5), [id(15)]);
    packed.object(id(15), []).packable(true);
    for page in [3, 4, 5] {
        packed.page(Page::new(id(page)));
    }
    packed.tree_node(id(2));
    let planned = plan(&packed, &Limits::DEFAULT).unwrap();
    let container = &planned.containers()[0];
    assert_eq!(
        container.members.len(),
        2,
        "one stream of two pages' members"
    );
    let lengths = vec![50; planned.writes().len()];
    let layout = assemble(&planned, &lengths, &Options::new((1, 7)), |stream| {
        Ok::<_, std::convert::Infallible>(Encoded::plain(stream.data.to_vec()))
    })
    .unwrap();
    let mut facts = facts_of(&planned, &layout);
    let to = |number: u32| planned.number(id(number)).unwrap();
    let mut written = Graph::new(to(1), Kind::Table);
    written.object(to(1), [to(2)]);
    written.object(to(2), [to(3), to(4), to(5)]);
    written.object(to(3), [to(13)]);
    written.object(to(13), []);
    written.object(to(4), [to(14)]);
    written.object(to(14), []);
    written.object(to(5), [to(15)]);
    written.object(to(15), []);
    for page in [3, 4, 5] {
        written.page(Page::new(to(page)));
    }
    written.tree_node(to(2));
    facts.graph = Some(written);
    assert_eq!(findings(&facts), []);
}

#[test]
fn a_graph_that_cant_be_planned_is_said() {
    let mut facts = table();
    facts.graph = Some(Graph::new(id(1), Kind::Table));
    let found = findings(&facts);
    assert!(
        found
            .iter()
            .any(|f| matches!(f, Finding::GraphRefused { .. })),
        "{found:?}"
    );
    assert!(check(&facts, &Limits::DEFAULT, &mut ()).holds);
}

/// Told none, as `()` is; counted how often it's asked, which is how far a
/// check looks.
#[derive(Default)]
struct Unwanted {
    told: usize,
    asked: std::cell::Cell<usize>,
}

impl Sink for Unwanted {
    fn finding(&mut self, _: Finding) {
        self.told += 1;
    }

    fn wants(&self) -> bool {
        self.asked.set(self.asked.get() + 1);
        false
    }
}

/// Many faults, of each grade, after an error that settles it.
fn faulty() -> Facts {
    let mut facts = table();
    dict(&mut facts).o = Raw::Other;
    main_section(&mut facts)
        .rows
        .iter_mut()
        .skip(1)
        .for_each(|row| {
            row.entry = RowEntry::Free;
        });
    first_section(&mut facts).size = Raw::Integer(1);
    with_tables(&mut facts, |tables| {
        for page in &mut tables.pages.pages {
            page.length += 1;
        }
        tables.pages.header.min_length = 0;
        tables.pages.header.length_bits = 32;
    });
    facts
}

#[test]
fn a_sink_never_changes_a_verdict() {
    for facts in [table(), laid_out(Kind::Stream), faulty()] {
        let mut kept = Vec::new();
        let verdict = check(&facts, &Limits::DEFAULT, &mut kept);
        assert_eq!(check(&facts, &Limits::DEFAULT, &mut ()), verdict);
        let mut unwanted = Unwanted::default();
        assert_eq!(check(&facts, &Limits::DEFAULT, &mut unwanted), verdict);
        assert_eq!(unwanted.told, 0);
        let borrowed: &mut dyn Sink = &mut kept;
        assert_eq!(check(&facts, &Limits::DEFAULT, borrowed), verdict);
        let mut again = Vec::new();
        check(&facts, &Limits::DEFAULT, &mut &mut again);
        assert_eq!(again.len(), kept.len() / 2);
    }
}

#[test]
fn a_sink_that_wants_nothing_stops_looking_at_what_settles_it() {
    let facts = faulty();
    let mut kept = Vec::new();
    let verdict = check(&facts, &Limits::DEFAULT, &mut kept);
    assert!(!verdict.holds);
    let errors_first = kept.iter().position(|f| f.grade() == Grade::Error).unwrap();
    assert!(kept.len() > errors_first + 10, "{kept:?}");
    let mut unwanted = Unwanted::default();
    assert_eq!(check(&facts, &Limits::DEFAULT, &mut unwanted), verdict);
    // Asked of each finding up to the first error, and of what it takes to be
    // sure of the rest of the verdict: no more.
    assert_eq!(unwanted.told, 0);
    assert_eq!(unwanted.asked.get(), errors_first + 1);
}

#[test]
fn a_sink_that_wants_nothing_still_has_hints_read_said() {
    let mut facts = table();
    dict(&mut facts).o = Raw::Other;
    let verdict = check(&facts, &Limits::DEFAULT, &mut ());
    assert!(verdict.hints_read && !verdict.holds);
}

#[test]
fn hint_tables_that_dont_read_settle_it_for_a_sink_that_wants_nothing() {
    let mut facts = table();
    hint_of(&mut facts).data = HintData::Decoded(vec![0; 8]);
    // Something to say past that, were it looking.
    dict(&mut facts).end = 2000;
    facts.nodes.push((1, Attribute::Rotate));
    let mut unwanted = Unwanted::default();
    let verdict = check(&facts, &Limits::DEFAULT, &mut unwanted);
    assert!(!verdict.holds && !verdict.hints_read);
    assert_eq!(
        unwanted.asked.get(),
        2,
        "no further than what the tables told"
    );
}

#[test]
fn a_keys_name_past_what_it_holds_is_cut_off() {
    let name = KeyName::new(b"AVeryLongKeyIndeedThatGoesOn");
    assert_eq!(name.as_bytes(), b"AVeryLongKeyInd");
    assert_eq!(name.to_string(), "/AVeryLongKeyInd");
    assert_eq!(KeyName::new(b"P").to_string(), "/P");
}

#[test]
fn each_finding_says_its_clause_and_each_error_is_what_a_reader_acts_on() {
    let errors = [
        Finding::Version,
        Finding::PageCount { n: 1, pages: 2 },
        Finding::FirstPageEnd {
            e: 1,
            least: 2,
            most: 3,
        },
        Finding::MainFirstEntry {
            t: 1,
            least: 2,
            most: 3,
        },
        Finding::HintExtent {
            at: 1,
            length: 2,
            least: 3,
            most: 4,
        },
        Finding::PageLength {
            page: 1,
            hint: 2,
            computed: 3,
        },
        Finding::PageObjects {
            page: 1,
            hint: 2,
            computed: 3,
        },
        Finding::SharedIdPast {
            page: 1,
            id: 2,
            entries: 3,
        },
        Finding::SharedLength {
            entry: 1,
            hint: 2,
            computed: 3,
        },
        Finding::TableAt {
            table: "/O",
            hint: 2,
            computed: 3,
        },
    ];
    for finding in errors {
        assert_eq!(finding.grade(), Grade::Error, "{finding:?}");
        assert!(finding.to_string().contains("ISO 32000-2"), "{finding}");
    }
}

#[test]
fn an_n_of_0_reads_as_a_table_of_no_pages_and_is_said() {
    for kind in [Kind::Table, Kind::Stream] {
        let (mut facts, graph) = numbered(kind);
        facts.graph = Some(graph);
        dict(&mut facts).n = Raw::Integer(0);
        let found = findings(&facts);
        let told = |f: &Finding| matches!(f, Finding::PageCount { n: 0, pages: 4 });
        assert!(found.iter().any(told), "{found:?}");
        let verdict = check(&facts, &Limits::DEFAULT, &mut ());
        assert!(verdict.linearized && verdict.hints_read && !verdict.holds);
        // And with no pages to say it of: it's that that's told.
        facts.pages.clear();
        let found = findings(&facts);
        assert!(found.contains(&Finding::NoPages), "{found:?}");
    }
}

#[test]
fn a_row_numbered_the_last_there_can_be_is_no_number_to_add_to() {
    let mut facts = table();
    let row = RowFacts::new(u32::MAX, 0, RowEntry::InUse(1));
    main_section(&mut facts).rows.push(row);
    let found = findings(&facts);
    let next = u64::from(u32::MAX) + 1;
    let told = |f: &Finding| matches!(f, Finding::GroupsApart { next: at, .. } if *at == next);
    assert!(found.iter().any(told), "{found:?}");
    // In either section.
    let mut facts = table();
    let row = RowFacts::new(u32::MAX, 0, RowEntry::Free);
    first_section(&mut facts).rows.push(row);
    findings(&facts);
}

#[test]
fn it_never_panics_on_facts_past_any_position() {
    let mut facts = table();
    let big = i64::MAX;
    dict(&mut facts).h_items = vec![Raw::Integer(big), Raw::Integer(big)];
    dict(&mut facts).e = Raw::Integer(big);
    dict(&mut facts).t = Raw::Integer(big);
    dict(&mut facts).p = Raw::Integer(big);
    dict(&mut facts).o = Raw::Integer(big);
    dict(&mut facts).n = Raw::Integer(big);
    findings(&facts);
    let _ = Options::new((1, 7));
}
