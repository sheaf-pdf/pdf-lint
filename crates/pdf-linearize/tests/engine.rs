//! Files written from hand-written bodies with no adapter, and read here with
//! no reader but a few lines: every promise of [`assemble`] held on each case,
//! from the file's bytes, then what each case is there for.

use std::convert::Infallible;

use pdf_linearize::hint::{self, Tables, located};
use pdf_linearize::layout::{Encoded, Error, Options, Piece, Pieces, Stream, StreamKind};
use pdf_linearize::pdf_objects::ObjectId;
use pdf_linearize::pdf_objects::version::Version;
use pdf_linearize::plan::{Group, Kind, Page, Part, What};
use pdf_linearize::{Graph, Limits, assemble, plan};

mod cases;
use cases::{
    Doc, Written, all_tables, first_page_other, form, hexed, long, outline, plain, shared,
    thumbnails, two_pages,
};

/// The bytes of `bytes` at `at`, as text, as far as a line's or the file's end.
fn line_at(bytes: &[u8], at: usize) -> String {
    let rest = &bytes[at..];
    let end = rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len());
    String::from_utf8_lossy(&rest[..end]).into_owned()
}

/// A key's number in a dictionary's text.
fn number_of(text: &str, key: &str) -> Option<u64> {
    let rest = text.split(&format!("{key} ")).nth(1)?;
    rest.split_ascii_whitespace().next()?.parse().ok()
}

/// What a section's rows say of a number: free, at a position, or in a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Free(u64),
    At(u64),
    In(u64, u64),
}

/// A stream's data as it stands, by its dictionary's text.
fn stream_data(bytes: &[u8], at: usize) -> (String, Vec<u8>) {
    let start = at + (bytes[at..].windows(7).position(|w| w == b"stream\n")).unwrap();
    let dictionary = String::from_utf8_lossy(&bytes[at..start]).into_owned();
    let length = number_of(&dictionary, "/Length").unwrap() as usize;
    let data = &bytes[start + 7..start + 7 + length];
    assert_eq!(
        &bytes[start + 7 + length..start + 7 + length + 17],
        b"\nendstream\nendobj"
    );
    let data = match dictionary.contains("/ASCIIHexDecode") {
        true => {
            let digit = |b: &u8| (*b as char).to_digit(16).map(|digit| digit as u8);
            let digits: Vec<u8> = data.iter().filter_map(digit).collect();
            let pair = |pair: &[u8]| pair[0] << 4 | pair[1];
            digits.chunks(2).map(pair).collect()
        }
        false => data.to_vec(),
    };
    (dictionary, data)
}

/// The rows at a section at `at`, as many as it says, by their numbers, and
/// what follows them: its trailer's text, or its stream's dictionary.
fn section_at(bytes: &[u8], at: usize) -> (Vec<(u64, Row)>, String) {
    if bytes[at..].starts_with(b"xref\n") {
        let header = line_at(bytes, at + 5);
        let mut words = header.split(' ');
        let first: u64 = words.next().unwrap().parse().unwrap();
        let count: usize = words.next().unwrap().parse().unwrap();
        let rows_at = at + 5 + header.len() + 1;
        let row = |i: usize| {
            let row = &bytes[rows_at + i * 20..rows_at + (i + 1) * 20];
            let text = std::str::from_utf8(row).unwrap();
            assert!(text.ends_with(" \n"), "{text:?}");
            let offset: u64 = text[..10].parse().unwrap();
            match &text[17..18] {
                "n" => Row::At(offset),
                "f" => Row::Free(text[11..16].parse().unwrap()),
                other => panic!("{other}"),
            }
        };
        let rows = (0..count).map(|i| (first + i as u64, row(i))).collect();
        let after = rows_at + count * 20;
        assert!(bytes[after..].starts_with(b"trailer\n"));
        let end = after
            + (bytes[after..]
                .windows(5)
                .position(|w| w == b"%%EOF")
                .unwrap());
        (
            rows,
            String::from_utf8_lossy(&bytes[after..end]).into_owned(),
        )
    } else {
        let (dictionary, data) = stream_data(bytes, at);
        assert!(dictionary.contains("/Type /XRef"));
        assert!(dictionary.contains("/W [1 4 2]"), "{dictionary}");
        let index = dictionary.split("/Index [").nth(1).unwrap();
        let mut words = index.split_ascii_whitespace();
        let first: u64 = words.next().unwrap().parse().unwrap();
        let count: usize = words.next().unwrap().trim_end_matches(']').parse().unwrap();
        assert_eq!(data.len(), count * 7);
        let rows = (data.chunks(7).enumerate())
            .map(|(i, row)| {
                let field = u32::from_be_bytes(row[1..5].try_into().unwrap()) as u64;
                let last = u16::from_be_bytes(row[5..7].try_into().unwrap()) as u64;
                let row = match row[0] {
                    0 => Row::Free(last),
                    1 => {
                        assert_eq!(last, 0);
                        Row::At(field)
                    }
                    2 => Row::In(field, last),
                    other => panic!("{other}"),
                };
                (first + i as u64, row)
            })
            .collect();
        (rows, dictionary)
    }
}

/// The file's own say of itself, read from its bytes.
struct Read {
    dictionary: String,
    tables: Tables,
    hint_at: u64,
    hint_len: u64,
}

impl Read {
    fn value(&self, key: &str) -> u64 {
        number_of(&self.dictionary, key).unwrap()
    }
}

/// The promises every file keeps, from its bytes, and what's read of it.
fn holds(written: &Written) -> Read {
    let Written {
        bytes,
        plan,
        layout,
        ..
    } = written;
    assert_eq!(layout.len(), bytes.len() as u64);
    assert!(!layout.is_empty());

    // The header, then the dictionary, whole and within the first 1024 bytes,
    // and a fixed length (F.3.3).
    assert!(bytes.starts_with(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n"));
    let header = 15;
    let dictionary_end =
        header + (bytes[header..].windows(6).position(|w| w == b"endobj")).unwrap() + 6;
    assert!(dictionary_end < 1024);
    let dictionary = String::from_utf8_lossy(&bytes[header..dictionary_end]).into_owned();
    assert!(
        dictionary.starts_with(&format!(
            "{} 0 obj\n<< /Linearized 1 /L {} /H [",
            plan.dictionary_number(),
            bytes.len(),
        )),
        "{dictionary}"
    );
    assert_eq!(
        bytes[dictionary_end + 1..header + 199],
        vec![b' '; header + 199 - dictionary_end - 1]
    );
    assert_eq!(bytes[header + 199], b'\n');
    assert_eq!(layout.first_section(), header as u64 + 200);

    // Its values: where the hint stream is, whole; its numbers (Table F.1).
    let h = dictionary.split("/H [").nth(1).unwrap();
    let mut h = h.split([' ', ']']);
    let hint_at: u64 = h.next().unwrap().parse().unwrap();
    let hint_len: u64 = h.next().unwrap().parse().unwrap();
    assert_eq!((hint_at, hint_len), layout.hint_extent());
    assert_eq!(
        line_at(bytes, hint_at as usize),
        format!("{} 0 obj", plan.hint_number())
    );
    assert!(bytes[..(hint_at + hint_len) as usize].ends_with(b"endobj\n"));
    assert_eq!(
        number_of(&dictionary, "/O"),
        Some(u64::from(plan.first_page_number()))
    );
    assert_eq!(
        number_of(&dictionary, "/N"),
        Some(u64::from(plan.page_count()))
    );
    let said = (plan.first_page() != 0).then_some(u64::from(plan.first_page()));
    assert_eq!(number_of(&dictionary, "/P"), said);

    // The first-page section: one subsection of no free entries, from the
    // dictionary to the hint stream (F.3.4), and a dummy `startxref`.
    let first_at = layout.first_section() as usize;
    let (first_rows, first_trailer) = section_at(bytes, first_at);
    assert_eq!(first_rows[0].0, u64::from(plan.dictionary_number()));
    assert_eq!(first_rows.last().unwrap().0, u64::from(plan.hint_number()));
    assert_eq!(first_rows[0].1, Row::At(header as u64));
    assert_eq!(first_rows.last().unwrap().1, Row::At(hint_at));
    assert!(
        first_rows
            .iter()
            .all(|(_, row)| !matches!(row, Row::Free(_)))
    );
    assert_eq!(
        number_of(&first_trailer, "/Size"),
        Some(u64::from(plan.size()))
    );
    assert_eq!(
        number_of(&first_trailer, "/Root"),
        Some(u64::from(plan.catalog_number()))
    );
    let main_at = number_of(&first_trailer, "/Prev").unwrap();
    assert_eq!(main_at, layout.main_section());
    let eof = (bytes[first_at..].windows(5).position(|w| w == b"%%EOF")).unwrap() + first_at;
    assert!(bytes[..eof].ends_with(b"startxref\n0\n"));
    let rest = bytes[eof..].iter().position(|b| *b == b'\n').unwrap() + eof + 1;
    assert_eq!(
        &bytes[rest..].split(|b| *b == b'\n').next().unwrap(),
        &format!("{} 0 obj", plan.catalog_number()).as_bytes(),
        "the catalog's right after the section"
    );

    // The main section: from 0, entry 0 free, the rest in use, a `/Size` alone
    // in a table's trailer, no `/Prev`, and a `startxref` to the first-page
    // section (F.3.11).
    let (main_rows, main_trailer) = section_at(bytes, main_at as usize);
    assert_eq!(main_rows[0], (0, Row::Free(65535)));
    assert!(
        main_rows[1..]
            .iter()
            .all(|(_, row)| !matches!(row, Row::Free(_)))
    );
    assert_eq!(
        main_rows.last().unwrap().0 + 1,
        u64::from(plan.dictionary_number())
    );
    assert_eq!(
        number_of(&main_trailer, "/Size"),
        Some(u64::from(plan.dictionary_number()))
    );
    assert!(!main_trailer.contains("/Prev"), "{main_trailer}");
    assert!(!main_trailer.contains("/Root"), "{main_trailer}");
    let tail = format!("startxref\n{}\n%%EOF\n", layout.first_section());
    assert!(bytes.ends_with(tail.as_bytes()));

    // `/T` by what lists the file (Table F.1).
    let t = number_of(&dictionary, "/T").unwrap() as usize;
    match plan.kind() {
        Kind::Table => {
            assert_eq!(bytes[t], b'\n', "/T is the white space before row 0");
            assert!(bytes[t + 1..].starts_with(b"0000000000 65535 f \n"));
            assert_eq!(
                &bytes[main_at as usize..t],
                format!("xref\n0 {}", main_rows.len()).as_bytes()
            );
        }
        Kind::Stream => {
            assert_eq!(t as u64, main_at);
            assert_eq!(
                line_at(bytes, t),
                format!("{} 0 obj", plan.main_xref_number().unwrap())
            );
        }
        kind => panic!("{kind:?}"),
    }

    // Every number once, each row what's there: `n 0 obj` where it says, or in
    // the stream it says at the place it says.
    let mut rows = vec![None; plan.size() as usize];
    for (number, row) in first_rows.iter().chain(&main_rows) {
        assert!(rows[*number as usize].is_none(), "{number} listed twice");
        rows[*number as usize] = Some(*row);
    }
    assert!(rows.iter().all(Option::is_some));
    for (number, row) in rows.iter().enumerate().skip(1) {
        match row.unwrap() {
            Row::At(at) => {
                assert_eq!(line_at(bytes, at as usize), format!("{number} 0 obj"));
                assert_eq!(layout.position(number as u32), Some(at));
            }
            Row::In(stream, index) => {
                assert_eq!(layout.position(number as u32), None);
                let Some(Row::At(at)) = rows[stream as usize] else {
                    panic!("object stream {stream} isn't in use");
                };
                let (dictionary, data) = stream_data(bytes, at as usize);
                assert!(dictionary.contains("/ObjStm"));
                let pairs = String::from_utf8_lossy(&data);
                let mut words = pairs.split_ascii_whitespace();
                let at_index = words.nth(index as usize * 2).unwrap();
                assert_eq!(at_index.parse::<usize>().unwrap(), number);
            }
            Row::Free(_) => panic!("{number} is free"),
        }
    }
    assert_eq!(layout.position(0), None);
    assert_eq!(layout.position(plan.size()), None);

    // The hint stream's data is what's assembled, its keys what's said.
    let (hint_dictionary, data) = stream_data(bytes, hint_at as usize);
    assert_eq!(data, layout.hint_data());
    let keys = layout.hint_keys();
    assert_eq!(number_of(&hint_dictionary, "/S"), Some(keys.shared));
    for (key, at) in [
        ("/T", keys.thumbnails),
        ("/O", keys.outlines),
        ("/A", keys.threads),
        ("/E", keys.dests),
        ("/V", keys.form),
        ("/I", keys.info),
        ("/C", keys.structure),
        ("/L", keys.labels),
        ("/R", keys.renditions),
        ("/B", keys.files),
    ] {
        assert_eq!(number_of(&hint_dictionary, key), at, "{key}");
    }
    let tables = hint::tables(&data, keys, plan.page_count(), &Limits::DEFAULT).unwrap();

    // The hints: where each page is by adding up lengths and numbers, as Table
    // F.4 has a reader do, each landing on the page's object.
    let found = |position: u32| located(u64::from(position), hint_at, hint_len);
    let first_number = plan.first_page_number();
    let first_at_said = found(tables.pages.header.first_page_at);
    assert_eq!(layout.position(first_number), Some(first_at_said));
    assert_eq!(tables.pages.header.numerator_bits, 0, "no numerators");
    assert_eq!(tables.pages.header.denominator, 4);
    let mut at = layout.first_page_end();
    let mut number = 1;
    for (page, entry) in tables.pages.pages.iter().enumerate() {
        if page as u32 == plan.first_page() {
            assert!(entry.shared.is_empty(), "the first page lists none (F.4.2)");
            assert_eq!(first_at_said + entry.length, layout.first_page_end());
            continue;
        }
        assert_eq!(layout.position(number), Some(at), "page {page}");
        let page_write = plan.page_writes(page as u32).start;
        assert_eq!(plan.writes()[page_write].number, number);
        at += entry.length;
        number += entry.objects as u32;
        assert!((entry.shared.iter()).all(|r| r.0 < tables.shared.header.entries));
        assert!(entry.shared.iter().all(|r| r.1 == 0));
    }
    assert_eq!(
        Some(at),
        (layout.position(number)).or(Some(layout.main_section())),
        "the pages end where what follows starts"
    );

    // The shared table's entries the same way: from the first page, then part
    // 8.
    let shared = &tables.shared;
    let in_first = shared.header.first_page_entries as usize;
    assert_eq!(shared.groups.len(), plan.entries().len());
    let mut at = first_at_said;
    let mut number = first_number;
    for group in &shared.groups[..in_first] {
        assert_eq!(layout.position(number), Some(at));
        at += group.length;
        number += group.objects as u32;
    }
    assert_eq!(at, layout.first_page_end(), "the entries tile part 6");
    let mut at = found(shared.header.first_at);
    let mut number = shared.header.first_object;
    assert_ne!(number, 0, "PDFium refuses a table with a 0 here");
    assert_ne!(shared.header.first_at, 0);
    for group in &shared.groups[in_first..] {
        assert_eq!(layout.position(number), Some(at));
        at += group.length;
        number += group.objects as u32;
        assert_eq!(group.signature, None);
    }
    let part8 = plan.part_writes(Part::Shared);
    assert_eq!(shared.groups.len() - in_first, part8.len());

    // No width a PDFium's reader refuses.
    let header = &tables.pages.header;
    assert!(header.objects_bits > 0 && header.length_bits > 0);
    assert!(header.count_bits > 0 && header.id_bits > 0);
    assert!(shared.header.length_bits > 0);
    assert!(header.content_at_bits + header.content_length_bits >= header.length_bits);

    Read {
        dictionary,
        tables,
        hint_at,
        hint_len,
    }
}

/// Every case keeps every promise.
#[test]
fn every_file_says_what_it_is() {
    for (name, doc) in cases::all() {
        println!("{name}");
        holds(&doc.written());
    }
}

/// Where a write is in the file, from the lengths alone, the hint stream's
/// after part 4.
fn write_at(written: &Written, at: usize) -> u64 {
    let before: u64 = written.lengths[..at].iter().sum();
    let hint = match at >= written.plan.part_writes(Part::FirstPage).start {
        true => written.layout.hint_extent().1,
        false => 0,
    };
    written.layout.first_section() + section_room(written) + hint + before
}

/// How much room the first-page section took: to the first write, part 4's.
fn section_room(written: &Written) -> u64 {
    written
        .layout
        .position(written.plan.catalog_number())
        .unwrap()
        - written.layout.first_section()
}

/// The pieces are the file: in order, the hint stream after part 4 and before
/// part 6 (F.3.6), each body whole, each write where the plan and its length
/// say.
#[test]
fn the_pieces_are_the_file_in_order() {
    for (name, doc) in cases::all() {
        let written = doc.written();
        let hint = written.layout.hint_extent();
        assert!(
            hint.0
                >= written
                    .layout
                    .position(written.plan.catalog_number())
                    .unwrap(),
            "{name}"
        );
        assert_eq!(
            hint.0 + hint.1,
            write_at(&written, written.plan.part_writes(Part::FirstPage).start),
            "{name}"
        );
        for (at, write) in written.plan.writes().iter().enumerate() {
            assert_eq!(
                written.layout.position(write.number),
                Some(write_at(&written, at)),
                "{name} {at}"
            );
        }
        let mut counted = 0;
        let mut bodies = 0;
        for piece in written.layout.pieces() {
            counted += match piece {
                Piece::Bytes(part) => part.len() as u64,
                Piece::Body(at) => {
                    assert_eq!(at, bodies, "{name}");
                    bodies += 1;
                    written.lengths[at]
                }
            };
        }
        assert_eq!(counted, written.layout.len(), "{name}");
        assert_eq!(bodies, written.plan.writes().len(), "{name}");
    }
}

/// The pieces walked one by one and folded are the same, from wherever the walk
/// is: the head, the bodies before the hint stream, the hint stream's object,
/// the bodies after it and the main section, every body once and in order.
/// How many are left is exact at every step, a clone goes on from where it's
/// taken, and past the last there's nothing, however often it's asked.
#[test]
fn the_pieces_are_the_same_walked_or_folded_and_count_whats_left() {
    fn folded(pieces: Pieces<'_>) -> Vec<Piece<'_>> {
        pieces.fold(Vec::new(), |mut all, piece| {
            all.push(piece);
            all
        })
    }
    for (name, doc) in cases::all() {
        let written = doc.written();
        let (layout, plan) = (&written.layout, &written.plan);
        let writes = plan.writes().len();
        let mut walked = Vec::new();
        let mut pieces = layout.pieces();
        for piece in pieces.by_ref() {
            walked.push(piece);
        }
        assert_eq!(walked.len(), writes + 3, "{name}");
        assert_eq!(pieces.next(), None, "{name}");
        assert_eq!(pieces.size_hint(), (0, Some(0)), "{name}");
        assert_eq!(folded(layout.pieces()), walked, "{name}");
        for at in 0..=walked.len() {
            let mut pieces = layout.pieces();
            for _ in 0..at {
                pieces.next();
            }
            let left = walked.len() - at;
            assert_eq!(pieces.size_hint(), (left, Some(left)), "{name} {at}");
            assert_eq!(pieces.len(), left, "{name} {at}");
            assert_eq!(pieces.clone().count(), left, "{name} {at}");
            assert_eq!(folded(pieces.clone()), walked[at..], "{name} {at}");
            assert_eq!(pieces.collect::<Vec<_>>(), walked[at..], "{name} {at}");
        }
        // Three of the layout's own: the head, the hint stream and the main
        // section, which ends the file.
        let own: Vec<usize> = (walked.iter().enumerate())
            .filter_map(|(at, piece)| matches!(piece, Piece::Bytes(_)).then_some(at))
            .collect();
        let before_hint = plan.part_writes(Part::FirstPage).start;
        assert_eq!(own, [0, before_hint + 1, writes + 2], "{name}");
        let Piece::Bytes(hint) = walked[before_hint + 1] else {
            unreachable!()
        };
        let object = format!("{} 0 obj", plan.hint_number());
        assert!(hint.starts_with(object.as_bytes()), "{name}");
        assert_eq!(hint.len() as u64, layout.hint_extent().1, "{name}");
        let bodies: Vec<usize> = (walked.iter())
            .filter_map(|piece| match piece {
                Piece::Body(at) => Some(*at),
                Piece::Bytes(_) => None,
            })
            .collect();
        assert_eq!(bodies, (0..writes).collect::<Vec<_>>(), "{name}");
        assert!(
            format!("{pieces:?}", pieces = layout.pieces()).contains(&(writes + 3).to_string())
        );
    }
}

/// Whether `shown` holds `marker`, as text or as a `Debug` of its bytes gives
/// them, a list of numbers.
fn shows(shown: &str, marker: &[u8]) -> bool {
    let numbers = format!("{marker:?}");
    let numbers = &numbers[1..numbers.len() - 1];
    shown.contains(&*String::from_utf8_lossy(marker)) || shown.contains(numbers)
}

/// A layout's `Debug` says where everything is and how long, never a byte it
/// holds: not a trailer entry a caller gives, nor what a stream of its own was
/// encoded to, nor the hint data. A stream's, an encoded stream's and the
/// pieces' hold none either.
#[test]
fn a_layouts_debug_holds_none_of_its_bytes() {
    const MARKER: &[u8] = b"a marker of what's written";
    let doc = cases::two_pages(Kind::Stream, false);
    let plan = plan(&doc.graph(), &Limits::DEFAULT).unwrap();
    let lengths = vec![40; plan.writes().len()];
    let trailer = [&b"/Info << /Title ("[..], MARKER, b") >>"].concat();
    let mut options = Options::new(Version::new(1, 7));
    options.trailer = &trailer;
    let mut shown = Vec::new();
    let layout = assemble(&plan, &lengths, &options, |stream: Stream<'_>| {
        let mut data = stream.data.to_vec();
        data.extend_from_slice(MARKER);
        let encoded = Encoded::new([b"/Marker (", MARKER, b")"].concat(), data);
        shown.push(format!("{stream:?}"));
        shown.push(format!("{encoded:?} {encoded:#?}"));
        Ok::<_, Infallible>(encoded)
    })
    .unwrap();
    // The marker is in the layout's bytes, the hint data is too, and neither
    // is in its `Debug`.
    let pieces = layout.pieces().filter_map(|piece| match piece {
        Piece::Bytes(bytes) => Some(bytes),
        Piece::Body(_) => None,
    });
    let bytes = pieces.collect::<Vec<_>>().concat();
    assert!(bytes.windows(MARKER.len()).any(|w| w == MARKER));
    let data = layout.hint_data();
    assert!(data.len() >= 8);
    shown.push(format!("{layout:?} {layout:#?}"));
    shown.push(format!("{:?}", layout.pieces()));
    for shown in &shown {
        assert!(!shows(shown, MARKER), "{shown}");
        assert!(!shows(shown, data), "{shown}");
        assert!(!shows(shown, &trailer), "{shown}");
    }
    assert!(shown.iter().any(|shown| shown.contains("bytes")));
}

/// `/E` is the end of part 6: where what follows it starts, or the main
/// section.
#[test]
fn the_first_page_ends_where_part_6_does() {
    for (name, doc) in cases::all() {
        let written = doc.written();
        let read = holds(&written);
        let e = read.value("/E");
        assert_eq!(e, written.layout.first_page_end(), "{name}");
        let next = written.plan.part_writes(Part::FirstPage).end;
        let starts = match written.plan.writes().get(next) {
            Some(write) => written.layout.position(write.number),
            None => Some(written.layout.main_section()),
        };
        assert_eq!(starts, Some(e), "{name}");
    }
}

/// Two pages sharing nothing: each page's own; none listed, nothing in part 8.
#[test]
fn two_pages_sharing_nothing() {
    for kind in [Kind::Table, Kind::Stream] {
        let doc = two_pages(kind, false);
        let written = doc.written();
        let read = holds(&written);
        let plan = &written.plan;
        assert_eq!(plan.number(ObjectId::new(4, 0)), Some(ObjectId::new(1, 0)));
        assert_eq!(plan.number(ObjectId::new(6, 0)), Some(ObjectId::new(2, 0)));
        assert_eq!(read.tables.pages.pages.len(), 2);
        assert_eq!(read.tables.shared.groups.len(), 1, "the first page's run");
        assert_eq!(read.tables.shared.header.first_page_entries, 1);
        assert_eq!(read.tables.shared.groups[0].objects, 2);
        // Part 8 is empty, so where it would be: the tree's, after part 7.
        assert_eq!(read.tables.shared.header.first_object, 3);
        let entry = &read.tables.pages.pages[1];
        assert_eq!(entry.objects, 2);
        assert_eq!(entry.shared, []);
    }
}

/// Part 8 empty, and nothing after it but the section, under either kind.
#[test]
fn the_shared_tables_first_object_is_never_0() {
    for (kind, first_object) in [(Kind::Table, 3), (Kind::Stream, 3)] {
        let mut doc = two_pages(kind, false);
        doc.tree.clear();
        // No tree node given: it's still written last, as what's left.
        let read = holds(&doc.written());
        assert_eq!(read.tables.shared.header.first_object, first_object);
    }
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(2, 0), []);
    graph.page(Page::new(ObjectId::new(2, 0)));
    let plan = plan(&graph, &Limits::DEFAULT).unwrap();
    let layout = assemble(
        &plan,
        &[10, 10],
        &Options::new(Version::new(1, 7)),
        |stream| Ok::<_, Infallible>(plain(stream)),
    )
    .unwrap();
    let tables = hint::tables(layout.hint_data(), layout.hint_keys(), 1, &Limits::DEFAULT).unwrap();
    assert_eq!(tables.shared.header.first_object, plan.dictionary_number());
    assert_eq!(
        u64::from(tables.shared.header.first_at),
        layout.main_section() - layout.hint_extent().1
    );
}

/// What's shared: with the first page, in part 6 and listed by its own entry;
/// by later pages, in part 8; each page's list what it reaches.
#[test]
fn what_pages_share_is_listed_by_entry() {
    for (kind, packed) in [
        (Kind::Table, false),
        (Kind::Stream, false),
        (Kind::Stream, true),
    ] {
        let doc = shared(kind, packed);
        let written = doc.written();
        let read = holds(&written);
        let plan = &written.plan;
        let tables = &read.tables;
        let entry_of = |id| {
            let write = (plan.writes().iter()).position(|write| match write.what {
                What::Object(object) => object == id,
                What::Container(c) => plan.containers()[c].members.iter().any(|m| m.0 == id),
            });
            let write = write.unwrap();
            plan.entries()
                .iter()
                .position(|e| e.contains(&write))
                .map(|e| e as u32)
        };
        let ids = |page: usize| -> Vec<u32> {
            let shared = &tables.pages.pages[page].shared;
            shared.iter().map(|r| r.0).collect()
        };
        let font8 = entry_of(ObjectId::new(8, 0)).unwrap();
        let font7 = entry_of(ObjectId::new(7, 0)).unwrap();
        assert!(font8 < tables.shared.header.first_page_entries);
        assert!(font7 >= tables.shared.header.first_page_entries);
        assert_eq!(
            entry_of(ObjectId::new(9, 0)).is_some(),
            packed,
            "9's its page's alone"
        );
        assert_eq!(ids(0), Vec::<u32>::new());
        if packed {
            // Its own and the shared font in one object stream of part 8, whose
            // entry both lists name.
            assert_eq!(entry_of(ObjectId::new(9, 0)), Some(font7));
            assert_eq!(ids(1), [font8, font7]);
        } else {
            assert_eq!(ids(1), [font8, font7]);
        }
        assert_eq!(ids(2), [font7]);
        // An image's last on its page.
        let section = plan.page_writes(1);
        assert_eq!(
            plan.writes()[section.end - 1].what,
            What::Object(ObjectId::new(10, 0))
        );
    }
}

/// A later page's own content stream: from the page, to its last's end. One in
/// part 8 isn't its own, and it says none (F.4.2).
#[test]
fn a_pages_content_items_are_its_own_streams() {
    let mut doc = Doc::new(Kind::Table, false, &[3, 4, 5, 6]);
    doc.page(3, &[7], "<< >>", "");
    doc.page(4, &[8, 9], "<< >>", "");
    // Page 5's stream is page 6's too.
    doc.page(5, &[10], "<< >>", "");
    doc.page(6, &[10], "<< >>", "");
    doc.stream(7, "", &[b' '; 11]);
    doc.stream(8, "", &[b' '; 22]);
    doc.stream(9, "", &[b' '; 33]);
    doc.stream(10, "", &[b' '; 44]);
    let written = doc.written();
    let read = holds(&written);
    let pages = &read.tables.pages.pages;
    let position = |id: u32| {
        written
            .layout
            .position(written.plan.number(ObjectId::new(id, 0)).unwrap().number)
            .unwrap()
    };
    let length = |id: u32| {
        let number = written.plan.number(ObjectId::new(id, 0)).unwrap().number;
        let write = (written.plan.writes().iter())
            .position(|w| w.number == number)
            .unwrap();
        written.lengths[write]
    };
    assert_eq!(pages[0].content_at, position(7) - position(3));
    assert_eq!(pages[0].content_length, length(7));
    assert_eq!(pages[1].content_at, position(8) - position(4));
    assert_eq!(pages[1].content_length, length(8) + length(9));
    assert_eq!((pages[2].content_at, pages[2].content_length), (0, 0));
    assert_eq!((pages[3].content_at, pages[3].content_length), (0, 0));
    assert_eq!(written.plan.page_contents(2), 0..0);
}

/// Another page opening first: the dictionary's `/P`, its section part 6, its
/// entry in the table at its place, the rest of part 7 in page order (F.3.7,
/// F.3.8). Sections are of different lengths, so an entry in the wrong place
/// shows.
#[test]
fn a_first_page_other_than_0_keeps_its_place_in_the_tables() {
    for (kind, packed) in [
        (Kind::Table, false),
        (Kind::Stream, false),
        (Kind::Stream, true),
    ] {
        let doc = first_page_other(kind, packed);
        let written = doc.written();
        let read = holds(&written);
        let plan = &written.plan;
        assert_eq!(read.value("/P"), 1);
        assert_eq!(
            read.value("/O"),
            u64::from(plan.number(ObjectId::new(4, 0)).unwrap().number)
        );
        assert_eq!(plan.number(ObjectId::new(3, 0)), Some(ObjectId::new(1, 0)));
        assert_eq!(plan.number(ObjectId::new(7, 0)), Some(ObjectId::new(3, 0)));
        let length = |range: std::ops::Range<usize>| written.lengths[range].iter().sum::<u64>();
        let pages = &read.tables.pages.pages;
        assert_eq!(pages[0].length, length(plan.page_writes(0)));
        assert_eq!(pages[1].length, length(plan.part_writes(Part::FirstPage)));
        assert_eq!(pages[2].length, length(plan.page_writes(2)));
        assert_ne!(pages[0].length, pages[1].length);
        assert_ne!(pages[1].length, pages[2].length);
    }
}

/// A form: its dictionary in part 4, alone (F.3.5), its table saying where the
/// field is and that the group refers to no more.
#[test]
fn a_forms_dictionary_is_at_open_and_its_table_says_where_its_fields_are() {
    for (kind, packed) in [
        (Kind::Table, false),
        (Kind::Stream, false),
        (Kind::Stream, true),
    ] {
        let doc = form(kind, packed);
        let written = doc.written();
        let read = holds(&written);
        let plan = &written.plan;
        let open = plan.part_writes(Part::Open);
        assert_eq!(
            plan.writes()[open.start + 1].what,
            match packed {
                true => What::Container(0),
                false => What::Object(ObjectId::new(9, 0)),
            }
        );
        let table = read.tables.form.as_ref().unwrap();
        let range = plan.group_writes(Group::Form).unwrap();
        assert_eq!(table.generic.objects as usize, range.len());
        let number = plan.writes()[range.start].number;
        assert_eq!(table.generic.first_object, number);
        assert_eq!(
            located(
                u64::from(table.generic.first_at),
                read.hint_at,
                read.hint_len
            ),
            written.layout.position(number).unwrap()
        );
        assert_eq!(
            u64::from(table.generic.length),
            written.lengths[range].iter().sum::<u64>()
        );
        assert_eq!(table.shared, plan.group_shared(Group::Form));
    }
}

/// An outline: in part 9, or under `/UseOutlines` in part 6 and in `/E`, the
/// table saying where either way, the first page's entry counting what's in
/// part 6.
#[test]
fn an_outlines_table_says_where_it_is_in_part_6_or_9() {
    for first in [false, true] {
        for (kind, packed) in [
            (Kind::Table, false),
            (Kind::Stream, false),
            (Kind::Stream, true),
        ] {
            let doc = outline(kind, packed, first);
            let written = doc.written();
            let read = holds(&written);
            let plan = &written.plan;
            let range = plan.group_writes(Group::Outlines).unwrap();
            let table = read.tables.outlines.unwrap();
            assert_eq!(table.objects as usize, range.len());
            let number = plan.writes()[range.start].number;
            assert_eq!(table.first_object, number);
            assert_eq!(
                located(u64::from(table.first_at), read.hint_at, read.hint_len),
                written.layout.position(number).unwrap()
            );
            let in_part_6 = plan.writes()[range.start].part == Part::FirstPage;
            assert_eq!(in_part_6, first);
            let entry = &read.tables.pages.pages[0];
            let part6 = plan.part_writes(Part::FirstPage);
            assert_eq!(entry.objects as usize, part6.len());
            let page = plan.page_writes(0);
            assert_eq!(part6.len() > page.len(), first);
        }
    }
}

/// Thumbnails: by page from page 0, whichever page opens first, each entry
/// saying how many pages before it have none; what several share counted apart
/// (F.4.4).
#[test]
fn the_thumbnail_table_counts_pages_without_one() {
    for (kind, packed) in [
        (Kind::Table, false),
        (Kind::Stream, false),
        (Kind::Stream, true),
    ] {
        let mut doc = thumbnails(kind, packed);
        doc.first = 2;
        doc.catalog_says("/OpenAction [{!5} /Fit]");
        let written = doc.written();
        let read = holds(&written);
        let plan = &written.plan;
        let table = read.tables.thumbnails.as_ref().unwrap();
        assert_eq!(table.header.pages, 2);
        assert_eq!(table.pages[0].gap, 0);
        assert_eq!(table.pages[1].gap, 1);
        let first = plan.thumb_writes(0);
        assert_eq!(
            plan.writes()[first.start].what,
            What::Object(ObjectId::new(11, 0))
        );
        assert_eq!(table.header.first_object, plan.writes()[first.start].number);
        let second = plan.thumb_writes(2);
        assert_eq!(
            plan.writes()[second.start].what,
            What::Object(ObjectId::new(12, 0))
        );
        assert_eq!(
            table.pages[0].length,
            written.lengths[first].iter().sum::<u64>()
        );
        assert_eq!(
            table.pages[1].length,
            written.lengths[second].iter().sum::<u64>()
        );
        assert_eq!(table.header.shared_objects, 1);
        let shared = plan.thumb_shared_writes();
        assert_eq!(
            table.header.shared_first_object,
            plan.writes()[shared.start].number
        );
        assert_eq!(
            located(
                u64::from(table.header.shared_first_at),
                read.hint_at,
                read.hint_len
            ),
            written
                .layout
                .position(plan.writes()[shared.start].number)
                .unwrap()
        );
        assert_eq!(
            u64::from(table.header.shared_length),
            written.lengths[shared].iter().sum::<u64>()
        );
    }
}

/// No thumbnail, no table; a table only for a group the graph was given.
#[test]
fn only_whats_given_has_a_table() {
    let read = holds(&two_pages(Kind::Table, false).written());
    let tables = &read.tables;
    assert_eq!(tables.thumbnails, None);
    assert_eq!(tables.outlines, None);
    assert_eq!(tables.threads, None);
    assert_eq!(tables.dests, None);
    assert_eq!(tables.form, None);
    assert_eq!(tables.info, None);
    assert_eq!(tables.structure, None);
    assert_eq!(tables.labels, None);
    assert_eq!(tables.renditions, None);
    assert_eq!(tables.files, None);
    // A group given with nothing left of it is still told: its table's empty.
    let mut doc = two_pages(Kind::Table, false);
    doc.groups.push((Group::Form, Vec::new()));
    doc.groups.push((Group::Other, Vec::new()));
    let read = holds(&doc.written());
    let form = read.tables.form.unwrap();
    assert_eq!(form.generic, hint::Generic::default());
    assert_eq!(form.shared, []);
}

/// Every table but what's its own test, where each group is, each file of the
/// embedded file table a group of its own.
#[test]
fn every_table_says_where_its_group_is() {
    for (kind, packed) in [
        (Kind::Table, false),
        (Kind::Stream, false),
        (Kind::Stream, true),
    ] {
        let doc = all_tables(kind, packed);
        let written = doc.written();
        let read = holds(&written);
        let plan = &written.plan;
        let of_range = |range: std::ops::Range<usize>| {
            let length = written.lengths[range.clone()].iter().sum::<u64>();
            (
                range.len() as u32,
                length as u32,
                plan.writes()[range.start].number,
            )
        };
        let check = |table: hint::Generic, group: Group| {
            let (objects, length, number) = of_range(plan.group_writes(group).unwrap());
            assert_eq!(table.objects, objects, "{group:?}");
            assert_eq!(table.length, length, "{group:?}");
            assert_eq!(table.first_object, number, "{group:?}");
            assert_eq!(
                written.layout.position(number),
                Some(located(
                    u64::from(table.first_at),
                    read.hint_at,
                    read.hint_len
                )),
                "{group:?}"
            );
        };
        check(read.tables.threads.unwrap(), Group::ThreadInfo);
        check(read.tables.dests.unwrap(), Group::Dests);
        check(read.tables.info.unwrap(), Group::Info);
        check(
            read.tables.structure.as_ref().unwrap().generic,
            Group::Structure,
        );
        check(
            read.tables.renditions.as_ref().unwrap().generic,
            Group::Renditions,
        );
        assert_eq!(read.tables.labels, None, "no page labels table's written");

        // The trailer's `/Info` is in the numbers written.
        let first = section_at(&written.bytes, written.layout.first_section() as usize).1;
        assert_eq!(
            number_of(&first, "/Info"),
            Some(u64::from(plan.number(ObjectId::new(11, 0)).unwrap().number))
        );

        let files = read.tables.files.as_ref().unwrap();
        let file = |id: u32| plan.number(ObjectId::new(id, 0)).unwrap().number;
        assert_eq!(files.header.first_object, plan.files()[0].number);
        assert_eq!(files.header.groups, 3);
        assert_eq!(files.groups[0].number, file(16));
        assert_eq!(files.groups[0].objects, 1);
        // The second has objects of its own, and a group of none for what it
        // shares (F.4.7).
        assert_eq!(files.groups[1].number, file(17));
        assert!(files.groups[1].objects >= 2 - u32::from(packed));
        assert_eq!(files.groups[1].shared, []);
        assert_eq!(files.groups[2].number, file(17));
        assert_eq!((files.groups[2].objects, files.groups[2].length), (0, 0));
        assert_eq!(files.groups[2].shared, plan.file_shared(1));
        assert!(!plan.file_shared(1).is_empty());
        assert_eq!(plan.file_shared(0), []);
        // Together they tile the group's stretch of the file.
        let whole = plan.group_writes(Group::EmbeddedFile).unwrap();
        assert_eq!(plan.files()[0].writes.start, whole.start);
        assert_eq!(plan.files()[1].writes.end, whole.end);
        let length = files
            .groups
            .iter()
            .map(|g| u64::from(g.length))
            .sum::<u64>();
        assert_eq!(length, written.lengths[whole].iter().sum::<u64>());
    }
}

/// A table's first-page section is the same length wherever the file ends, with
/// room in its `/Prev`.
#[test]
fn a_tables_prev_has_room_for_any_position() {
    let small = two_pages(Kind::Table, false);
    let mut large = two_pages(Kind::Table, false);
    large.objects[3].data = Some(vec![b' '; 100_000]);
    let (small, large) = (small.written(), large.written());
    let room = |written: &Written| section_room(written);
    assert_eq!(room(&small), room(&large));
    for written in [&small, &large] {
        let (_, trailer) = section_at(&written.bytes, written.layout.first_section() as usize);
        let prev = format!("/Prev {:<20} /Root", written.layout.main_section());
        assert!(trailer.contains(&prev), "{trailer}");
    }
}

/// The embedded file table has its identifiers in the page offset table's width
/// with none of its own (F.4.7), so that width's as wide as they want, though
/// no page's lists go so high. Here the first page's last stretch is entry 2,
/// which no later page reaches, and the file does.
#[test]
fn a_files_identifiers_widen_the_page_tables() {
    let mut doc = Doc::new(Kind::Table, false, &[3, 4]);
    doc.page(3, &[5], "<< /Font << /F1 {7} /F2 {8} >> >>", "");
    doc.page(4, &[6], "<< /Font << /F1 {7} >> >>", "");
    doc.stream(5, "", b"0 0 m 1 1 l S");
    doc.stream(6, "", b"0 0 m 2 2 l S");
    doc.object(7, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    doc.object(8, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>");
    doc.stream(9, "/Type /EmbeddedFile /X {8}", b"a file");
    doc.groups.push((Group::EmbeddedFile, vec![9]));
    let read = holds(&doc.written());
    assert_eq!(read.tables.pages.pages[1].shared, [(1, 0)]);
    assert_eq!(read.tables.pages.header.id_bits, 2);
    let files = read.tables.files.unwrap();
    assert_eq!(files.groups[1].shared, [2]);
}

/// A closure's next encoding may be longer than the room the first was given:
/// the room grows and it goes round, everything then where it says.
#[test]
fn a_stream_sections_room_grows_until_it_fits() {
    for grows_for in [0, 1, 2, 5] {
        let mut calls = 0;
        let mut longer = 0;
        let doc = shared(Kind::Stream, true);
        let written = doc
            .written_with(|stream| {
                let mut encoded = hexed(stream);
                if stream.what == StreamKind::FirstXref {
                    calls += 1;
                    if calls > 1 && calls <= grows_for + 1 {
                        longer += 800 << calls;
                    }
                    // Spaces, which the filter passes over.
                    encoded.data.splice(0..0, std::iter::repeat_n(b' ', longer));
                }
                Ok::<_, Infallible>(encoded)
            })
            .unwrap();
        // The first call's a guess, and each round after it asks again.
        assert_eq!(calls, grows_for + 2, "{grows_for}");
        holds(&written);
    }
}

/// What fits leaves spaces after its `endobj` and before its `startxref`, all
/// else where it was said to be.
#[test]
fn a_stream_section_is_padded_after_its_object() {
    let mut calls = 0;
    let written = shared(Kind::Stream, false)
        .written_with(|stream| {
            let mut encoded = plain(stream);
            if stream.what == StreamKind::FirstXref {
                // Shorter than its guess, so it has room to pad.
                calls += 1;
                if calls > 1 {
                    encoded = Encoded::new(b"".to_vec(), encoded.data);
                } else {
                    encoded.data.extend_from_slice(&[0; 70]);
                }
            }
            Ok::<_, Infallible>(encoded)
        })
        .unwrap();
    holds(&written);
    let first = written.layout.first_section() as usize;
    let catalog = written
        .layout
        .position(written.plan.catalog_number())
        .unwrap() as usize;
    let section = &written.bytes[first..catalog];
    let end = section.windows(9).rposition(|w| w == b"startxref").unwrap();
    let object = section.windows(7).rposition(|w| w == b"endobj\n").unwrap() + 7;
    assert!(end > object + 70, "{end} {object}");
    assert_eq!(section[end - 1], b'\n');
    assert!(section[object..end - 1].iter().all(|b| *b == b' '));
}

/// A closure that's longer every time is refused, however long that's been let
/// go: and the room's what grew, so a table's closure can't do this.
#[test]
fn a_stream_section_that_never_fits_is_refused() {
    let mut longer = 0;
    let refused = shared(Kind::Stream, false).written_with(|stream| {
        let mut encoded = plain(stream);
        if stream.what == StreamKind::FirstXref {
            longer = longer * 2 + 100;
            encoded.data.extend(std::iter::repeat_n(0, longer));
        }
        Ok::<_, Infallible>(encoded)
    });
    assert!(matches!(refused, Err(Error::Unsettled)));
    let mut longer = 0;
    let written = shared(Kind::Table, false).written_with(|stream| {
        let mut encoded = plain(stream);
        assert_eq!(stream.what, StreamKind::Hint);
        assert_eq!(stream.columns, None);
        longer = longer * 2 + 1000;
        encoded.data.extend(std::iter::repeat_n(0, longer));
        Ok::<_, Infallible>(encoded)
    });
    let written = written.unwrap();
    assert!(written.layout.hint_extent().1 > longer as u64);
}

/// Each stream's what and number, a section's bytes per row with it, for the
/// closure to work from.
#[test]
fn the_closure_is_told_what_each_stream_is() {
    let mut seen = Vec::new();
    let doc = two_pages(Kind::Stream, false);
    let written = doc
        .written_with(|stream| {
            seen.push((stream.what, stream.number, stream.columns));
            Ok::<_, Infallible>(plain(stream))
        })
        .unwrap();
    let plan = &written.plan;
    let first = plan.first_xref_number().unwrap();
    let main = plan.main_xref_number().unwrap();
    assert_eq!(
        seen,
        [
            (StreamKind::FirstXref, first, Some(7)),
            (StreamKind::Hint, plan.hint_number(), None),
            (StreamKind::FirstXref, first, Some(7)),
            (StreamKind::MainXref, main, Some(7)),
        ]
    );
}

/// What can't be assembled says why, and a refusal of the closure's is the
/// closure's.
#[test]
fn what_cant_be_assembled_is_refused() {
    let doc = two_pages(Kind::Table, false);
    let plan = plan(&doc.graph(), &Limits::DEFAULT).unwrap();
    let options = Options::new(Version::new(1, 7));
    let fine = |stream: Stream<'_>| Ok::<_, Infallible>(plain(stream));
    assert_eq!(
        assemble(&plan, &[1], &options, fine),
        Err(Error::Lengths {
            lengths: 1,
            writes: plan.writes().len(),
        })
    );
    let mut lengths = vec![1; plan.writes().len()];
    lengths[1] = u32::MAX.into();
    let refused = assemble(&plan, &lengths, &options, fine);
    assert!(
        matches!(refused, Err(Error::TooLarge { .. })),
        "{refused:?}"
    );
    lengths[1] = u32::MAX as u64 - 300;
    let refused = assemble(&plan, &lengths, &options, fine);
    assert!(
        matches!(refused, Err(Error::TooLarge { .. })),
        "{refused:?}"
    );
    lengths[1] = u32::MAX as u64 - 8000;
    assert!(assemble(&plan, &lengths, &options, fine).is_ok());
    lengths[1] = u64::MAX;
    let refused = assemble(&plan, &lengths, &options, fine);
    assert!(
        matches!(refused, Err(Error::TooLarge { .. })),
        "{refused:?}"
    );

    // At the edge exactly: the file's last byte is what's counted, the bodies'
    // sum before that, and the hint stream's length moves with what they come
    // to.
    let mut lengths = vec![1; plan.writes().len()];
    lengths[1] += u64::from(u32::MAX) - 2000;
    let largest = loop {
        match assemble(&plan, &lengths, &options, fine) {
            Ok(layout) if layout.len() == u32::MAX.into() => break layout,
            Ok(layout) => lengths[1] += u64::from(u32::MAX) - layout.len(),
            Err(Error::TooLarge { len }) => lengths[1] -= len - u64::from(u32::MAX),
            Err(other) => panic!("{other:?}"),
        }
    };
    assert_eq!(largest.len(), u32::MAX.into());
    lengths[1] += 1;
    assert_eq!(
        assemble(&plan, &lengths, &options, fine),
        Err(Error::TooLarge {
            len: u64::from(u32::MAX) + 1,
        })
    );

    let lengths = vec![1; plan.writes().len()];
    let refused = assemble(&plan, &lengths, &options, |_| Err::<Encoded, _>("no"));
    assert_eq!(refused, Err(Error::Encode("no")));
    assert_eq!(refused.unwrap_err().to_string(), "no");
    let stream = two_pages(Kind::Stream, false);
    let stream = pdf_linearize::plan(&stream.graph(), &Limits::DEFAULT).unwrap();
    let lengths = vec![1; stream.writes().len()];
    for fails in [
        StreamKind::Hint,
        StreamKind::FirstXref,
        StreamKind::MainXref,
    ] {
        let refused = assemble(&stream, &lengths, &options, |stream| {
            match stream.what == fails {
                true => Err("no"),
                false => Ok(plain(stream)),
            }
        });
        assert_eq!(refused, Err(Error::Encode("no")), "{fails:?}");
    }
}

/// A long document: what a viewer goes by adds up over many pages and many
/// widths.
#[test]
fn many_pages_add_up() {
    for first in [0, 5] {
        for (kind, packed) in [
            (Kind::Table, false),
            (Kind::Stream, false),
            (Kind::Stream, true),
        ] {
            let read = holds(&long(kind, packed, first).written());
            assert_eq!(read.tables.pages.pages.len(), 24);
            assert!(read.tables.pages.header.length_bits >= 8);
        }
    }
}
