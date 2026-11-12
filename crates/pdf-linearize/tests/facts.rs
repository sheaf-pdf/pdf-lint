//! [`Facts`] built from outside the crate, with no document model, an object's id the
//! one type of the family's it takes (pdf-objects' `ObjectId`), as a caller of any
//! model builds them: each of the files `tests/engine.rs` writes by hand, read here
//! from its bytes with a few lines, holds, and a fact of each sort changed says so.
//! Each part has its constructor, so a way in that's missing doesn't compile.

use pdf_linearize::check::{
    Attribute, DictKey, Extent, Facts, Finding, FirstObject, Grade, HintData, HintFacts, PageFacts,
    Raw, RowEntry, RowFacts, SectionFacts, Sink,
};
use pdf_linearize::pdf_objects::ObjectId;
use pdf_linearize::plan::{Kind, Part};
use pdf_linearize::{Limits, check};

mod cases;
use cases::{Written, all, two_pages};

/// The facts of `written`, its positions from its bytes where a reader would have
/// them from there.
fn facts_of(written: &Written) -> Facts {
    let (bytes, plan, layout) = (&written.bytes, &written.plan, &written.layout);
    let mut facts = Facts::new(bytes.len() as u64);

    let dictionary = plan.dictionary_number();
    let at = layout.position(dictionary).unwrap();
    let text = String::from_utf8_lossy(&bytes[at as usize..at as usize + 200]);
    assert!(text.starts_with(&format!("{dictionary} 0 obj")), "{text}");
    let end = at + (text.find("endobj").unwrap() + 6) as u64;
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
    let items = text
        .split("/H [")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    first.h_items = (items.split_ascii_whitespace())
        .map(|item| Raw::Integer(item.parse().unwrap()))
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
    section.root = Raw::Reference(ObjectId::new(plan.catalog_number(), 0));
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
            let mut said = PageFacts::new(Some(ObjectId::new(number, 0)));
            said.resources = true;
            said.media_box = true;
            said
        })
        .collect();
    // Where what's last in the first page's section ends, as read.
    let part6 = plan.part_writes(Part::FirstPage);
    if !part6.is_empty() {
        let last = plan.writes()[part6.end - 1].number;
        let start = layout.position(last).unwrap();
        let end = start + written.lengths[part6.end - 1];
        facts.objects.push(Extent::new(last, start, end));
    }
    facts
}

/// Findings counted, and none wanted.
#[derive(Default)]
struct Counted(usize);

impl Sink for Counted {
    fn finding(&mut self, _: Finding) {
        self.0 += 1;
    }

    fn wants(&self) -> bool {
        false
    }
}

#[test]
fn every_case_holds_with_nothing_to_say() {
    let mut cases = 0;
    for (name, doc) in all() {
        let written = doc.written();
        let facts = facts_of(&written);
        let mut findings: Vec<Finding> = Vec::new();
        let verdict = check(&facts, &Limits::DEFAULT, &mut findings);
        assert_eq!(findings, [], "{name}");
        assert!(
            verdict.linearized && verdict.holds && verdict.hints_read,
            "{name}"
        );
        cases += 1;
    }
    assert!(cases > 30, "{cases}");
}

#[test]
fn a_fact_changed_is_said() {
    let written = two_pages(Kind::Table, false).written();
    let said = |change: &dyn Fn(&mut Facts)| {
        let mut facts = facts_of(&written);
        change(&mut facts);
        let mut findings = Vec::new();
        let verdict = check(&facts, &Limits::DEFAULT, &mut findings);
        (verdict, findings)
    };
    let (verdict, findings) = said(&|facts| {
        facts.first_object.as_mut().unwrap().n = Raw::Integer(3);
        facts.pages.truncate(1);
    });
    assert!(!verdict.holds);
    assert!(
        findings.contains(&Finding::PageCount { n: 3, pages: 1 }),
        "{findings:?}"
    );

    let (verdict, findings) = said(&|facts| facts.len += 1);
    assert!(!verdict.linearized && !verdict.holds);
    assert!(
        matches!(findings[..], [Finding::LengthNotFile { .. }]),
        "{findings:?}"
    );

    let (_, findings) = said(&|facts| facts.first_object.as_mut().unwrap().o = Raw::Other);
    assert!(findings.contains(&Finding::BadValue { key: DictKey::O }));

    let (verdict, findings) = said(&|facts| facts.nodes.push((1, Attribute::CropBox)));
    assert!(verdict.holds, "a departure");
    assert_eq!(findings[0].grade(), Grade::Departure);
    assert!(
        findings[0].to_string().contains("F.3.10"),
        "{}",
        findings[0]
    );
}

#[test]
fn a_sink_outside_the_crate_is_a_sink() {
    let written = two_pages(Kind::Stream, true).written();
    let mut facts = facts_of(&written);
    facts.first_object.as_mut().unwrap().o = Raw::Other;
    let mut counted = Counted::default();
    let verdict = check(&facts, &Limits::DEFAULT, &mut counted);
    assert_eq!(counted.0, 0, "it wants none");
    assert_eq!(verdict, check(&facts, &Limits::DEFAULT, &mut ()));
    let dyn_sink: &mut dyn Sink = &mut Vec::<Finding>::new();
    assert_eq!(verdict, check(&facts, &Limits::DEFAULT, dyn_sink));
    let mut kept: Vec<Finding> = Vec::new();
    assert_eq!(verdict, check(&facts, &Limits::DEFAULT, &mut &mut kept));
    assert!(!kept.is_empty());
}

#[test]
fn nothing_known_is_nothing_said() {
    let mut findings: Vec<Finding> = Vec::new();
    let verdict = check(&Facts::new(0), &Limits::DEFAULT, &mut findings);
    assert!(!verdict.linearized && !verdict.holds && !verdict.hints_read);
    assert_eq!(findings, []);
}
