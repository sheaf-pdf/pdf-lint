//! Each finding, from the facts of a file laid out by a plan with one thing changed:
//! it fires for that and nothing else that isn't its own consequence.

use super::*;
use crate::fuzz::facts_of;
use crate::hint::{EmbeddedFiles, FileEntry, Generic, ThumbEntry, Thumbnails};
use crate::layout::{Encoded, Options, assemble};
use crate::plan::{Graph, Group, Kind, Page, plan};
use crate::{Id, Limits};

fn id(number: u32) -> Id {
    (number, 0)
}

/// Four pages, each its own object and a content stream, what all three share, what
/// the later two share, a form's fields and a catalog, each object by what `to` makes
/// of its number.
fn graph_by(kind: Kind, to: impl Fn(u32) -> Id) -> Graph {
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

/// That `change` makes `facts` tell what `told` says, and the verdict what it should.
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

/// `facts`' hint data's tables, changed by `change`, and written back. A table's
/// widths stay what they were, or say so themselves.
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
    dict(&mut facts).l = Raw::Reference((1, 0));
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
            |facts| set(dict(facts), Raw::Reference((1, 0))),
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
        |facts| dict(facts).p = Raw::Reference((1, 0)),
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
        |facts| dict(facts).h_items[1] = Raw::Reference((1, 0)),
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
    // Written at the file's end, though not numbered so: F.3.6 has it either way, and
    // whatever else moved by that, the first group's last object doesn't.
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
    // An object numbered past it, as an indirect /Length written after it is, within
    // what /H says.
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
    let Some(Raw::Reference((number, _))) = facts.first.as_ref().map(|first| first.root) else {
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
        |facts| first_section(facts).root = Raw::Reference((1, 0)),
        |f| *f == Finding::CatalogNotFirst { number: 1 },
    );
    let mut facts = table();
    let (number, _) = facts.pages[1].id.unwrap();
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
        |facts| hint_of(facts).keys[0].1 = Raw::Reference((1, 0)),
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
    length.1 = Raw::Reference((1, 0));
    assert_eq!(findings(&facts), []);
}

#[test]
fn a_hint_stream_with_no_key_for_it_says_nothing_and_isnt_read() {
    let mut facts = table();
    hint_of(&mut facts).data = HintData::Locked;
    facts.pages.clear();
    assert_eq!(findings(&facts), []);
    let verdict = check(&facts, &Limits::DEFAULT, &mut ());
    assert!(verdict.linearized && verdict.holds && !verdict.hints_read);
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

#[test]
fn the_first_pages_object_not_listed_first_is_told() {
    let mut facts = table();
    let (number, _) = facts.pages[0].id.unwrap();
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
    // A count of none, where its length holds what's there: the count's said, once.
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
    // Another page's entry in the table's columns runs into the next table, which is
    // still read from its own key.
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

/// The graph, in the numbers `facts` has its objects at: the same objects a plan laid
/// out, what each page reaches then being what it's held to.
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
    // The first page reaches what no row of the first section lists, and a later page
    // what's elsewhere.
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
    // What the first page reaches is first, here: this one's the main section's.
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
    // What only one page reaches, in an object stream it shares with another page's:
    // part 8 has the stream, the page's list names it, F.3.1.
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

/// Told none, as `()` is; counted how often it's asked, which is how far a check
/// looks.
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
    // Asked of each finding up to the first error, and of what it takes to be sure of
    // the rest of the verdict: no more.
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
