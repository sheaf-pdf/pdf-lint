//! Files whose sections or objects aren't where they say, read where the
//! file proves they are ([`Recovery::Verified`](super::Recovery::Verified)),
//! every way a reading reads: each recovery one fault a section, each
//! object read at a shift [`Origin::Shifted`], and nothing taken that the
//! file doesn't prove.

use std::borrow::Cow;

use pdf_objects::fault::Place;
use pdf_objects::object::{Object, ObjectId, Span, Value};

use super::super::lengths::every_way_under;
use super::super::{
    Fault, Faults, Options, ParseKind, Reading, Recovery, SCAN_READ, SectionRows, Unclosing,
    Unwritten, document,
};
use crate::source::{Document, Origin, Source};
use crate::write::{self, Base, Body};

/// The objects the files here hold, each a number and what's between its
/// `n 0 obj` and `endobj`: a catalog, its page tree and one page, and a
/// string after them.
pub(crate) const OBJECTS: [(u32, &str); 4] = [
    (1, "<< /Type /Catalog /Pages 2 0 R >>"),
    (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
    (
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << >> >>",
    ),
    (4, "(four)"),
];

/// A file of `objects` after `prefix` and its header, a table listing each
/// at what `row` writes of its place (its number and where its `n` is in
/// the file's bytes), and `startxref` at what `start` writes of the
/// table's; a free row for each number not given. Also each object's place
/// and the table's.
pub(crate) fn laid_out(
    prefix: &[u8],
    objects: &[(u32, &str)],
    row: &dyn Fn(u32, usize) -> usize,
    start: &dyn Fn(usize) -> usize,
) -> (Vec<u8>, Vec<(u32, usize)>, usize) {
    let mut file = prefix.to_vec();
    file.extend_from_slice(b"%PDF-1.7\n");
    let mut places = Vec::new();
    for &(n, body) in objects {
        places.push((n, file.len()));
        file.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let table = file.len();
    let size = objects.iter().map(|&(n, _)| n).max().unwrap_or(0) + 1;
    file.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for n in 1..size {
        match places.iter().find(|&&(m, _)| m == n) {
            Some(&(_, at)) => {
                file.extend_from_slice(format!("{:010} 00000 n \n", row(n, at)).as_bytes());
            }
            None => file.extend_from_slice(b"0000000000 00000 f \n"),
        }
    }
    let trailer = format!(
        "trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        start(table)
    );
    file.extend_from_slice(trailer.as_bytes());
    (file, places, table)
}

/// [`laid_out`], every offset as ISO 32000-2 7.5.2 writes it: from the
/// header, past `prefix`.
fn plain(prefix: &[u8]) -> (Vec<u8>, Vec<(u32, usize)>, usize) {
    let h = prefix.len();
    laid_out(prefix, &OBJECTS, &|_, at| at - h, &|at| at - h)
}

/// What a test holds of a reading: what reading `file` whole gives, every
/// way a reading reads it alike ([`every_way_under`]), and with faults off the
/// same but no fault, the reading still saying it recovered.
fn read(file: &[u8]) -> Document {
    read_as(file, &Options::DEFAULT)
}

/// [`read`], under `options`.
fn read_as(file: &[u8], options: &Options) -> Document {
    let mut readings = every_way_under(file, options).into_iter();
    let (_, whole) = readings.next().unwrap();
    for (way, doc) in readings {
        assert_eq!(doc, whole, "{way}");
    }
    let quiet = Options {
        faults: Faults::Off,
        ..*options
    };
    let off = document(file, &quiet);
    assert_eq!(off.faults(), [], "faults off");
    assert_eq!(off.facts, whole.facts, "faults off");
    let ids: Vec<ObjectId> = whole.objects().map(|(id, _)| id).collect();
    assert_eq!(off.objects().map(|(id, _)| id).collect::<Vec<_>>(), ids);
    for (id, object) in whole.objects() {
        assert_eq!(off.get(id), Ok(object), "faults off: {id}");
        assert_eq!(off.record(id), whole.record(id), "faults off: {id}");
    }
    whole
}

/// The faults `doc` says that a recovery does.
fn recoveries(doc: &Document) -> Vec<Fault> {
    (doc.faults().iter())
        .filter(|(_, f)| matches!(f, Fault::Moved { .. } | Fault::Shifted { .. }))
        .map(|(_, f)| f.clone())
        .collect()
}

/// That each object of [`OBJECTS`] reads in `doc`, its record at its
/// place in `places` and its origin `origin` of its number.
fn each_object_at(doc: &Document, places: &[(u32, usize)], origin: &dyn Fn(u32) -> Origin) {
    for &(n, at) in places {
        let id = ObjectId::new(n, 0);
        assert!(doc.get(id).is_ok(), "{id}");
        let record = doc.record(id).unwrap();
        assert_eq!(record.at.start, at, "{id}");
        assert_eq!(record.origin, origin(n), "{id}");
        assert!(record.endobj, "{id}");
    }
}

/// That the page tree `doc` holds reaches its one page.
fn reaches_its_page(doc: &Document) {
    let pages = doc.get(ObjectId::new(2, 0)).unwrap().as_dict().unwrap();
    let kids = pages.get(b"Kids").unwrap().as_array().unwrap();
    let page = match kids[0].value() {
        Value::Ref(id) => *id,
        _ => panic!("no page"),
    };
    assert_eq!(page, ObjectId::new(3, 0));
    let page = doc.get(page).unwrap().as_dict().unwrap();
    assert_eq!(
        page.get(b"Type").and_then(|t| t.as_name()),
        Some(&b"Page"[..])
    );
}

/// A file cut after it was written: a first section at the start, listing
/// object 5, whose `/Prev` gives the main section, which lists the rest;
/// the file written whole and then three bytes cut out before object 4,
/// so that the main section's `/Prev`, its row for object 4 and the main
/// section itself are each three bytes past where they are, the rest where
/// they're written. The file, each object's place and the main section's.
pub(crate) fn cut() -> (Vec<u8>, Vec<(u32, usize)>, usize) {
    let mut file = b"%PDF-1.4\n".to_vec();
    let five = file.len();
    file.extend_from_slice(b"5 0 obj\n<< /Linearized 1 >>\nendobj\n");
    let first = file.len();
    // The first section, its /Prev 10 digits to fill in.
    let first_section = format!(
        "xref\n0 1\n0000000000 65535 f \n5 1\n{five:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev 0000000000 >>\nstartxref\n0\n%%EOF\n"
    );
    file.extend_from_slice(first_section.as_bytes());
    let mut places = vec![(5, five)];
    for &(n, body) in &OBJECTS[..3] {
        places.push((n, file.len()));
        file.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    // What's cut out after the file was written.
    let cut = file.len();
    file.extend_from_slice(b"%c\n");
    places.push((4, file.len()));
    file.extend_from_slice(b"4 0 obj\n(four)\nendobj\n");
    let main = file.len();
    file.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for &(_, at) in &places[1..] {
        file.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
    }
    file.extend_from_slice(
        format!("trailer\n<< /Size 5 >>\nstartxref\n{first}\n%%EOF\n").as_bytes(),
    );
    // The first section's /Prev, as written before the cut.
    let prev = String::from_utf8_lossy(&file).find("/Prev ").unwrap() + 6;
    file[prev..prev + 10].copy_from_slice(format!("{main:010}").as_bytes());
    file.drain(cut..cut + 3);
    let places = (places.into_iter())
        .map(|(n, at)| (n, if at > cut { at - 3 } else { at }))
        .collect();
    (file, places, main - 3)
}

/// [`cut`]'s file: the main section, said three bytes past its
/// `xref`, is read there, and of its entries the one past the cut is read
/// three bytes before where it's written, the rest where they are; one
/// fault for the section, and its page reached, every way.
#[test]
fn a_cut_files_section_and_objects_past_the_cut_are_read_where_they_are() {
    let (file, places, main) = cut();
    let doc = read(&file);
    let moved = Fault::Moved {
        at: main + 3,
        found: main,
        from_start: false,
        moved: 1,
        scanned: 0,
        neither: 0,
    };
    assert_eq!(doc.faults(), [(Place::new(None, None), moved)]);
    let origin = |n| match n {
        4 => Origin::Shifted { by: -3 },
        _ => Origin::File,
    };
    each_object_at(&doc, &places, &origin);
    reaches_its_page(&doc);
    assert!(doc.facts.recovered);
    assert_eq!(doc.trailer.unwritten, None);
    assert_eq!(doc.facts.sections, 2);
    // Its sections each on their own, the main one where it was read.
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let sections = reading.sections().unwrap();
    assert_eq!(sections.stopped, None);
    let at: Vec<(usize, Option<usize>)> =
        sections.sections.iter().map(|s| (s.at, s.said)).collect();
    assert_eq!(at[1], (main, Some(main + 3)));
    assert_eq!(at[0].1, None);
    // Its rows as written: object 4's three bytes past where it is.
    let four = row(&sections.sections[1], 4);
    assert_eq!(four, places[4].1 + 3);
}

/// Where `rows` puts object `n`.
fn row(rows: &SectionRows, n: u32) -> usize {
    let row = rows.rows.iter().find(|r| r.number == n).unwrap();
    match row.entry {
        Some(super::super::XrefEntry::InUse { offset, .. }) => offset,
        _ => panic!("{n} isn't in use"),
    }
}

/// A file with bytes before its header whose offsets count from its first
/// byte, against ISO 32000-2 7.5.2: its section read where its `startxref`
/// puts it so counted, past the window a section is looked for in at 900
/// bytes, and each object exactly at its `n g obj` there; one fault, said
/// as such, and every object read where it is.
#[test]
fn a_file_whose_offsets_count_from_its_first_byte_is_read_so() {
    for h in [1, 100, 900] {
        let prefix = vec![b'~'; h];
        let (file, places, table) = laid_out(&prefix, &OBJECTS, &|_, at| at, &|at| at);
        let doc = read(&file);
        let moved = Fault::Moved {
            at: table + h,
            found: table,
            from_start: true,
            moved: 4,
            scanned: 0,
            neither: 0,
        };
        let header = Fault::HeaderAt { at: h };
        let told: Vec<Fault> = doc.faults().iter().map(|(_, f)| f.clone()).collect();
        assert_eq!(told, [header, moved], "{h}");
        let by = -i64::try_from(h).unwrap();
        each_object_at(&doc, &places, &|_| Origin::Shifted { by });
        reaches_its_page(&doc);
        assert_eq!(doc.facts.newest.map(|(at, _)| at), Some(table), "{h}");
        assert!(doc.facts.recovered, "{h}");
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let sections = reading.sections().unwrap();
        assert_eq!(sections.stopped, None, "{h}");
        assert_eq!(sections.sections[0].at, table, "{h}");
        assert_eq!(sections.sections[0].said, Some(table + h), "{h}");
    }
}

/// A file with bytes before its header whose offsets count from its first
/// byte, as [`a_file_whose_offsets_count_from_its_first_byte_is_read_so`]'s,
/// but whose `startxref`, or whose every row, gives the line's end before
/// what it puts there, as a reader that reads past white-space at an
/// offset reads it: its section read where its `startxref` puts it so
/// counted, past the line's end, before the window and past it, and each
/// object read where its row puts it so counted, past the line's end; one
/// fault, and every object read where it is.
#[test]
fn a_file_counted_from_its_first_byte_is_read_past_the_line_end() {
    for h in [10, 100] {
        let prefix = vec![b'~'; h];
        for rows_at_ends in [false, true] {
            let way = match rows_at_ends {
                false => "startxref at the line's end",
                true => "rows at the line's end",
            };
            let row = |_, at: usize| if rows_at_ends { at - 1 } else { at };
            let start = |at: usize| if rows_at_ends { at } else { at - 1 };
            let (file, places, table) = laid_out(&prefix, &OBJECTS, &row, &start);
            let doc = read(&file);
            let moved = Fault::Moved {
                at: start(table) + h,
                found: start(table),
                from_start: true,
                moved: 4,
                scanned: 0,
                neither: 0,
            };
            let header = Fault::HeaderAt { at: h };
            let told: Vec<Fault> = doc.faults().iter().map(|(_, f)| f.clone()).collect();
            assert_eq!(told, [header, moved], "{h}, {way}");
            let by = -i64::try_from(h).unwrap();
            each_object_at(&doc, &places, &|_| Origin::Shifted { by });
            reaches_its_page(&doc);
            assert_eq!(doc.trailer.unwritten, None, "{h}, {way}");
            assert_eq!(
                doc.facts.newest.map(|(at, _)| at),
                Some(start(table)),
                "{h}, {way}"
            );
        }
    }
}

/// A file with two bytes before its header whose offsets are each four
/// bytes short of where they put what's there, counted from the header,
/// and whose table follows an earlier `%%EOF`, so that counted from the
/// file's first byte its `startxref` puts the table in that comment: not
/// taken there, as what's past a comment proves nothing of where the
/// offsets count from, but at the `xref` four bytes after, and each object
/// as far; one fault.
#[test]
fn a_place_from_the_first_byte_inside_a_comment_isnt_taken() {
    let mut file = b"  %PDF-1.7\n".to_vec();
    let mut places = Vec::new();
    for &(n, body) in &OBJECTS {
        places.push((n, file.len()));
        file.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    file.extend_from_slice(b"%%EOF\r\n");
    let table = file.len();
    file.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for &(_, at) in &places {
        file.extend_from_slice(format!("{:010} 00000 n \n", at - 6).as_bytes());
    }
    let tail = format!(
        "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        table - 6
    );
    file.extend_from_slice(tail.as_bytes());
    let doc = read(&file);
    let moved = Fault::Moved {
        at: table - 4,
        found: table,
        from_start: false,
        moved: 4,
        scanned: 0,
        neither: 0,
    };
    let told: Vec<Fault> = doc.faults().iter().map(|(_, f)| f.clone()).collect();
    assert_eq!(told, [Fault::HeaderAt { at: 2 }, moved]);
    each_object_at(&doc, &places, &|_| Origin::Shifted { by: 4 });
    reaches_its_page(&doc);
}

/// A file of [`OBJECTS`] after `prefix` and its header, and a
/// cross-reference stream after them, object 5, listing each and itself at
/// what `row` writes of its place (its number and where its `n` is in the
/// file's bytes), uncompressed, and `startxref` at what `start` writes of
/// the stream's. Also each object's place and the stream's.
fn stream_laid_out(
    prefix: &[u8],
    row: &dyn Fn(u32, usize) -> usize,
    start: &dyn Fn(usize) -> usize,
) -> (Vec<u8>, Vec<(u32, usize)>, usize) {
    let mut file = prefix.to_vec();
    file.extend_from_slice(b"%PDF-1.7\n");
    let mut places = Vec::new();
    for &(n, body) in &OBJECTS {
        places.push((n, file.len()));
        file.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let stream = file.len();
    let mut rows = vec![0, 0, 0, 0, 0, 0xff, 0xff];
    for (n, at) in places.iter().copied().chain([(5, stream)]) {
        let at = u32::try_from(row(n, at)).unwrap();
        rows.push(1);
        rows.extend_from_slice(&at.to_be_bytes());
        rows.extend_from_slice(&[0, 0]);
    }
    let head = format!(
        "5 0 obj\n<< /Type /XRef /Size 6 /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n",
        rows.len()
    );
    file.extend_from_slice(head.as_bytes());
    file.extend_from_slice(&rows);
    file.extend_from_slice(b"\nendstream\nendobj\n");
    let tail = format!("startxref\n{}\n%%EOF\n", start(stream));
    file.extend_from_slice(tail.as_bytes());
    places.push((5, stream));
    (file, places, stream)
}

/// A file with bytes before its header whose offsets count from its first
/// byte, its section a cross-reference stream: the stream read where its
/// `startxref` puts it so counted, and its own row, which at that shift
/// is where the stream is, lists it; one fault, the stream's own row
/// among the entries it says.
#[test]
fn a_cross_reference_stream_read_at_a_shift_lists_itself_there() {
    let prefix = vec![b'~'; 10];
    let (file, places, stream) = stream_laid_out(&prefix, &|_, at| at, &|at| at);
    let doc = read(&file);
    let moved = Fault::Moved {
        at: stream + 10,
        found: stream,
        from_start: true,
        moved: 5,
        scanned: 0,
        neither: 0,
    };
    let told: Vec<Fault> = doc.faults().iter().map(|(_, f)| f.clone()).collect();
    assert_eq!(told, [Fault::HeaderAt { at: 10 }, moved]);
    let record = doc.record(ObjectId::new(5, 0)).unwrap();
    assert_eq!(record.origin, Origin::Shifted { by: -10 });
    each_object_at(&doc, &places[..4], &|_| Origin::Shifted { by: -10 });
    // As written from the header, it lists itself where it is, and says so
    // of a stream that doesn't.
    let (file, _, _) = stream_laid_out(&prefix, &|_, at| at - 10, &|at| at - 10);
    assert_eq!(read(&file).faults().len(), 1);
    let row = |n, at| if n == 5 { 9 } else { at - 10 };
    let (file, _, _) = stream_laid_out(&prefix, &row, &|at| at - 10);
    let told: Vec<Fault> = read(&file)
        .faults()
        .iter()
        .map(|(_, f)| f.clone())
        .collect();
    assert!(
        told.contains(&Fault::Unlisted {
            id: ObjectId::new(5, 0)
        }),
        "{told:?}"
    );
}

/// A table whose rows are each two bytes past their objects, with nothing
/// before the header: the section read where it's said, its first object
/// found two bytes before its row, and each read there; one fault.
#[test]
fn a_tables_rows_two_bytes_off_are_read_where_their_objects_are() {
    for by in [2, -2] {
        let (file, places, table) = laid_out(
            b"",
            &OBJECTS,
            &|_, at| at.checked_add_signed(by).unwrap(),
            &|at| at,
        );
        let doc = read(&file);
        let shifted = Fault::Shifted {
            at: table,
            by: -i64::try_from(by).unwrap(),
            from_start: false,
            moved: 4,
            scanned: 0,
            neither: 0,
        };
        assert_eq!(recoveries(&doc), [shifted], "{by}");
        assert_eq!(doc.faults().len(), 1, "{by}");
        let back = -i64::try_from(by).unwrap();
        each_object_at(&doc, &places, &|_| Origin::Shifted { by: back });
        reaches_its_page(&doc);
        assert_eq!(doc.facts.newest.map(|(at, _)| at), Some(table), "{by}");
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let sections = reading.sections().unwrap();
        assert_eq!(sections.sections[0].said, None, "{by}");
        assert_eq!(
            row(&sections.sections[0], 1),
            places[0].1.checked_add_signed(by).unwrap()
        );
    }
}

/// A `startxref` that gives the line after `xref`: the section read at
/// its `xref`, five bytes before, its entries where they're written.
#[test]
fn a_startxref_past_its_xref_is_read_at_the_xref() {
    let (file, places, table) = laid_out(b"", &OBJECTS, &|_, at| at, &|at| at + 5);
    let doc = read(&file);
    let moved = Fault::Moved {
        at: table + 5,
        found: table,
        from_start: false,
        moved: 0,
        scanned: 0,
        neither: 0,
    };
    assert_eq!(doc.faults(), [(Place::new(None, None), moved)]);
    each_object_at(&doc, &places, &|_| Origin::File);
    assert_eq!(doc.facts.newest.map(|(at, _)| at), Some(table));
}

/// A `startxref` a few bytes off whose `xref` near it doesn't read as a
/// section, the one that does too far: not taken, and the file scanned,
/// as with no recovery.
#[test]
fn a_section_near_that_doesnt_read_isnt_taken() {
    let (mut file, _, table) = laid_out(b"", &OBJECTS, &|_, at| at, &|at| at);
    // An `xref` with no table, and the real one past the window.
    let stray = b"xref\nnot a table\n";
    let filler = vec![b' '; 80];
    file.splice(table..table, [&stray[..], &filler].concat());
    let at = table + 3;
    let startxref = file.windows(9).rposition(|w| w == b"startxref").unwrap();
    let tail = format!("startxref\n{at}\n%%EOF\n");
    file.truncate(startxref);
    file.extend_from_slice(tail.as_bytes());
    let doc = read(&file);
    assert_eq!(recoveries(&doc), []);
    assert!(doc.trailer.unwritten.is_some());
    assert!(doc.facts.recovered, "scanned");
    assert_eq!(doc.facts.newest, None);
}

/// A table whose rows are two bytes past their objects, but object 3's,
/// which at that shift is object 4's `n g obj`, object 5's, which is
/// object 5's at another generation, object 6's, one byte past its
/// object, so that at that shift it's the line's end before its `n g obj`,
/// and object 7's, one byte past its object after a blank line, so that
/// at that shift it's white-space after white-space before its `n g obj`:
/// none of those four taken at the shift, and of them 3, 6 and 7 read
/// where a scan of the file finds them, 5 0 found nowhere.
#[test]
fn a_shift_onto_another_objects_header_isnt_taken() {
    let objects = [
        OBJECTS[0],
        OBJECTS[1],
        OBJECTS[2],
        OBJECTS[3],
        (5, "(five)"),
        (6, "(six)"),
        (7, "(seven)"),
    ];
    let mut file = b"%PDF-1.7\n".to_vec();
    let mut places = Vec::new();
    for &(n, body) in &objects {
        if n == 7 {
            file.extend_from_slice(b"\n");
        }
        places.push((n, file.len()));
        let generation = if n == 5 { 1 } else { 0 };
        file.extend_from_slice(format!("{n} {generation} obj\n{body}\nendobj\n").as_bytes());
    }
    let at = |n: u32| places.iter().find(|&&(m, _)| m == n).unwrap().1;
    let table = file.len();
    file.extend_from_slice(b"xref\n0 8\n0000000000 65535 f \n");
    for n in 1..=7 {
        // Object 3's row is object 4's, two past; 6's and 7's one past.
        let written = match n {
            3 => at(4) + 2,
            6 | 7 => at(n) + 1,
            n => at(n) + 2,
        };
        file.extend_from_slice(format!("{written:010} 00000 n \n").as_bytes());
    }
    file.extend_from_slice(
        format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
    );
    let doc = read(&file);
    let shifted = Fault::Shifted {
        at: table,
        by: -2,
        from_start: false,
        moved: 3,
        scanned: 3,
        neither: 1,
    };
    assert_eq!(recoveries(&doc), [shifted]);
    for n in [1, 2, 4] {
        let record = doc.record(ObjectId::new(n, 0)).unwrap();
        assert_eq!(record.origin, Origin::Shifted { by: -2 }, "{n}");
        assert_eq!(record.at.start, at(n), "{n}");
    }
    // Not object 4's bytes as 3, nor 6 or 7 past the white-space their
    // shift lands on: each where its own `n g obj` is.
    for n in [3, 6, 7] {
        let record = doc.record(ObjectId::new(n, 0)).unwrap();
        assert_eq!(record.origin, Origin::Recovered, "{n}");
        assert_eq!(record.at.start, at(n), "{n}");
    }
    // Nor 5 1 obj as 5 0.
    assert!(doc.get(ObjectId::new(5, 0)).is_err());
}

/// A file whose `startxref` is five bytes past its `xref`, so that its
/// section is read five bytes before, and whose rows for objects 1 and 2
/// are five bytes past them, at the section's move, but those for 3 and 4
/// four bytes before them: a file cut in two places. Also each object's
/// place and the table's.
pub(crate) fn two_shifts() -> (Vec<u8>, Vec<(u32, usize)>, usize) {
    let row = |n: u32, at: usize| if n <= 2 { at + 5 } else { at - 4 };
    laid_out(b"", &OBJECTS, &row, &|at| at + 5)
}

/// [`two_shifts`]: the section read where it is, objects 1 and 2 at its
/// move, and 3 and 4, neither where they're written nor at that move,
/// where a scan of the file finds their `n g obj`; one fault, every object
/// read and the page reached.
#[test]
fn a_moved_sections_entries_at_neither_place_are_read_where_a_scan_finds_them() {
    let (file, places, table) = two_shifts();
    let doc = read(&file);
    let moved = Fault::Moved {
        at: table + 5,
        found: table,
        from_start: false,
        moved: 2,
        scanned: 2,
        neither: 0,
    };
    assert_eq!(doc.faults(), [(Place::new(None, None), moved)]);
    let origin = |n| match n {
        1 | 2 => Origin::Shifted { by: -5 },
        _ => Origin::Recovered,
    };
    each_object_at(&doc, &places, &origin);
    reaches_its_page(&doc);
    assert!(doc.facts.recovered);
    assert_eq!(doc.trailer.unwritten, None);
    // Its rows as written.
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let sections = reading.sections().unwrap();
    assert_eq!(sections.sections[0].said, Some(table + 5));
    assert_eq!(row(&sections.sections[0], 3), places[2].1 - 4);
}

/// An update whose section is five bytes before where its `startxref`
/// says, after a file whose section is where it's said, its first and
/// last rows too, but whose row for object 2 is seven bytes before it: the
/// update's section moved, and the earlier section's entries checked at
/// that move, object 2 at neither place, where a scan finds it; a fault
/// each, the earlier's saying what the scan found though it moved none.
#[test]
fn a_section_checked_at_anothers_shift_says_what_a_scan_finds() {
    let row = |n: u32, at: usize| if n == 2 { at - 7 } else { at };
    let (mut file, mut places, table) = laid_out(b"", &OBJECTS, &row, &|at| at);
    let five = file.len();
    file.extend_from_slice(b"5 0 obj\n(five)\nendobj\n");
    places.push((5, five));
    let update = file.len();
    file.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n5 1\n{five:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {table} >>\nstartxref\n{}\n%%EOF\n",
            update + 5
        )
        .as_bytes(),
    );
    let doc = read(&file);
    let moved = Fault::Moved {
        at: update + 5,
        found: update,
        from_start: false,
        moved: 0,
        scanned: 0,
        neither: 0,
    };
    let shifted = Fault::Shifted {
        at: table,
        by: -5,
        from_start: false,
        moved: 0,
        scanned: 1,
        neither: 0,
    };
    assert_eq!(recoveries(&doc), [moved, shifted]);
    assert_eq!(doc.faults().len(), 2);
    let origin = |n| match n {
        2 => Origin::Recovered,
        _ => Origin::File,
    };
    for &(n, at) in &places {
        let record = doc.record(ObjectId::new(n, 0)).unwrap();
        assert_eq!((record.at.start, record.origin), (at, origin(n)), "{n}");
    }
    reaches_its_page(&doc);
}

/// An update whose `/Prev` gives no section, none near it either: said,
/// the update's section kept, as with no recovery.
#[test]
fn a_later_section_no_shift_proves_is_said_and_the_newer_kept() {
    let (mut file, _, table) = plain(b"");
    let end = file.len();
    let at = end;
    file.extend_from_slice(b"6 0 obj\n(six)\nendobj\n");
    let update = file.len();
    let prev = table + 300;
    file.extend_from_slice(
        format!(
            "xref\n6 1\n{at:010} 00000 n \ntrailer\n<< /Size 7 /Root 1 0 R /Prev {prev} >>\nstartxref\n{update}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let doc = read(&file);
    assert_eq!(recoveries(&doc), []);
    let told: Vec<Fault> = doc.faults().iter().map(|(_, f)| f.clone()).collect();
    assert!(
        matches!(&told[..], [Fault::Section { at, .. }] if *at == prev),
        "{told:?}"
    );
    assert_eq!(doc.trailer.unwritten, None);
    assert!(!doc.facts.recovered);
    assert_eq!(doc.objects().count(), 1);
}

/// A file whose first object's `n g obj` doesn't read, and isn't found
/// near where its row puts it: no shift, and nothing said but what reading
/// that object says, the probe telling nothing of its own.
#[test]
fn a_first_object_that_doesnt_read_and_has_no_shift_is_as_written() {
    let (mut file, _, _) = plain(b"");
    // Object 1's header no header: its bytes the same length.
    let one = file.windows(7).position(|w| w == b"1 0 obj").unwrap();
    file[one..one + 7].copy_from_slice(b"1 0 ob ");
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    assert_eq!(reading.faults(), [], "told at open");
    assert!(!reading.facts().recovered);
    let doc = read(&file);
    assert_eq!(recoveries(&doc), []);
    assert!(doc.get(ObjectId::new(1, 0)).is_err());
    for n in 2..=4 {
        let record = doc.record(ObjectId::new(n, 0)).unwrap();
        assert_eq!(record.origin, Origin::File, "{n}");
    }
}

/// A file with three bytes cut out of object 3's `n g obj`, so that its
/// section and object 4 are three bytes before where it says, and then an
/// update that writes object 3 again: read whole, object 3 the update's;
/// read as of the section the update follows, where the whole reading
/// read it, nothing the update wrote, object 3 at neither place, as no
/// scan of that revision's bytes finds it, and object 4 at the shift, every
/// way.
#[test]
fn a_reading_as_of_a_section_finds_nothing_a_later_update_wrote() {
    let (mut file, _, table) = plain(b"");
    let three = file.windows(7).position(|w| w == b"3 0 obj").unwrap();
    file.drain(three..three + 3);
    let end = file.len();
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 792] /Resources << >> >>";
    file.extend_from_slice(format!("3 0 obj\n{page}\nendobj\n").as_bytes());
    let update = file.len();
    file.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n3 1\n{end:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R /Prev {table} >>\nstartxref\n{update}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let whole = read(&file);
    let id = ObjectId::new(3, 0);
    let record = whole.record(id).map(|r| (r.at.start, r.origin));
    assert_eq!(record, Some((end, Origin::File)));
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let previous = reading.update().unwrap().previous;
    assert_eq!(previous, table - 3);
    let mut options = Options::DEFAULT;
    options.section = Some(previous);
    let shifted = Fault::Shifted {
        at: previous,
        by: -3,
        from_start: false,
        moved: 1,
        scanned: 0,
        neither: 1,
    };
    let mut docs = vec![("whole".to_string(), document(&file, &options))];
    for first in [1, 7, 64] {
        let source = crate::fuzz::Counted::new(&file, first);
        let reading = crate::fuzz::open(&source, &[], &options, true).unwrap();
        docs.push((
            format!("by position, {first}"),
            reading.into_document().unwrap(),
        ));
    }
    for (way, doc) in docs {
        assert_eq!(recoveries(&doc), std::slice::from_ref(&shifted), "{way}");
        assert!(doc.get(id).is_err(), "{way}");
        let four = doc.record(ObjectId::new(4, 0)).unwrap();
        assert_eq!(four.origin, Origin::Shifted { by: -3 }, "{way}");
        assert!(doc.facts.recovered, "{way}");
    }
}

/// A reading opened as of a place a few bytes from a section: taken as
/// it's given, not looked for near, so nothing reads there, and nothing
/// is read ([`as_of_nothing`]).
#[test]
fn a_section_given_a_few_bytes_off_isnt_looked_for() {
    let (file, _, table) = plain(b"");
    let why = Unwritten::Neither { at: table + 3 };
    as_of_nothing(&file, table + 3, why);
    let mut options = Options::DEFAULT;
    options.section = Some(table);
    let doc = document(&file, &options);
    assert_eq!(doc.trailer.unwritten, None);
    assert!(!doc.facts.recovered);
}

/// `file` read as of the section at `at`, which doesn't read for `why`,
/// every way, faults off too: no object, an empty trailer whose reason
/// says so, nothing recovered, and no fault but what reading the section
/// told.
fn as_of_nothing(file: &[u8], at: usize, why: Unwritten) -> Document {
    let mut options = Options::DEFAULT;
    options.section = Some(at);
    let whole = document(file, &options);
    let unwritten = Unwritten::AsOf {
        at,
        why: Box::new(why),
    };
    assert_eq!(whole.trailer.unwritten, Some(unwritten));
    assert_eq!(whole.trailer.dict(), &pdf_objects::object::Dict::new());
    assert_eq!(whole.objects().count(), 0);
    assert_eq!(whole.unread().count(), 0);
    assert!(!whole.facts.recovered);
    assert_eq!(whole.facts.sections, 0);
    assert_eq!(recoveries(&whole), []);
    let reading = Reading::open(file, &options).unwrap();
    assert_eq!(reading.ids().count(), 0);
    assert_eq!(reading.into_document().unwrap(), whole);
    for first in [1, 7, 64] {
        for bound in [true, false] {
            let source = crate::fuzz::Counted::new(file, first);
            let reading = crate::fuzz::open(&source, &[], &options, bound).unwrap();
            assert_eq!(
                reading.into_document().unwrap(),
                whole,
                "by position, {first}"
            );
        }
    }
    options.faults = Faults::Off;
    let quiet = document(file, &options);
    assert_eq!(quiet.faults(), []);
    assert_eq!(quiet.trailer, whole.trailer);
    assert_eq!(quiet.objects().count(), 0);
    whole
}

/// A file whose first section is a cross-reference stream whose rows don't
/// read, and an update after it whose table does: read whole, the update's
/// section and what it lists, the stream's said; read as of the stream, the
/// revision before the update, nothing, not the scan of the whole file,
/// which would find what the update wrote.
#[test]
fn a_reading_as_of_a_section_that_doesnt_read_reads_nothing() {
    let mut file = b"%PDF-1.7\n".to_vec();
    file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    file.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let stream = file.len();
    file.extend_from_slice(
        format!(
            "3 0 obj\n<< /Type /XRef /Size 4 /W [1 2] /Root 1 0 R /Length 0 >>\nstream\n\nendstream\nendobj\nstartxref\n{stream}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let four = file.len();
    file.extend_from_slice(b"4 0 obj\n(the update's)\nendobj\n");
    let table = file.len();
    file.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n4 1\n{four:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R /Prev {stream} >>\nstartxref\n{table}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let whole = document(&file, &Options::DEFAULT);
    assert_eq!(whole.trailer.unwritten, None);
    let ids: Vec<u32> = whole.objects().map(|(id, _)| id.number).collect();
    assert_eq!(ids, [4]);
    let said = whole.faults().iter().map(|(_, f)| f);
    assert!(
        said.into_iter()
            .any(|f| matches!(f, Fault::StreamRows { at, .. } if *at == stream))
    );
    assert_eq!(whole.update.as_ref().map(|u| u.previous), Some(stream));
    let before = as_of_nothing(&file, stream, Unwritten::Widths);
    assert!(before.get(ObjectId::new(4, 0)).is_err());
}

/// A reading as of the newest section, where a reading of the whole file
/// finds it away from where `startxref` says: near it, and where it puts
/// it counted from the file's first byte. Its base is the whole
/// reading's ([`Base::of`]), keeping what `startxref` said, so it holds the
/// file and not the file one byte longer; an update after it writes what
/// one after the whole reading writes, read back alike. As of the section
/// `startxref` says, where none reads, nothing ([`as_of_nothing`]).
#[test]
fn a_base_as_of_the_newest_section_found_elsewhere_is_the_whole_readings() {
    let files = [
        (
            "startxref past",
            laid_out(b"", &OBJECTS, &|_, at| at, &|at| at + 5),
        ),
        (
            "from the first byte",
            laid_out(&[b'~'; 100], &OBJECTS, &|_, at| at, &|at| at),
        ),
    ];
    for (name, (file, _, table)) in files {
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let newest = reading.facts().newest.map(|(at, _)| at);
        assert_eq!(newest, Some(table), "{name}");
        let base = Base::of(&reading).unwrap();
        let mut options = Options::DEFAULT;
        options.section = Some(table);
        let as_of = Reading::open(&file[..], &options).unwrap();
        let mut of = Base::of(&as_of).unwrap();
        assert_eq!(of.source, Some(as_of.id()), "{name}");
        assert!(of.holds(&file[..]).unwrap(), "{name}");
        let longer = [&file[..], b"\n"].concat();
        assert!(!of.holds(&longer[..]).unwrap(), "{name}");
        of.source = base.source;
        assert_eq!(of, base, "{name}");
        // An update after each, read back alike.
        let id = ObjectId::new(base.size, 0);
        let object = Object::new(Value::Integer(7));
        let update = |base: &Base| {
            let bodies = [(id, Body::New(Cow::Borrowed(&object)))];
            let mut out = file.clone();
            let written =
                write::update(&mut out, base, bodies, None, None, &write::Options::DEFAULT);
            let written = written.unwrap().unwrap();
            assert!(written.base.holds(&out[..]).unwrap(), "{name}");
            out
        };
        let after = update(&Base::of(&as_of).unwrap());
        assert_eq!(after, update(&base), "{name}");
        let back = read(&after);
        assert_eq!(back.trailer.unwritten, None, "{name}");
        assert_eq!(
            back.get(id).ok().and_then(Object::as_integer),
            Some(7),
            "{name}"
        );
        reaches_its_page(&back);
    }
    // As of where startxref says, past the table, nothing reads.
    let (file, _, table) = laid_out(b"", &OBJECTS, &|_, at| at, &|at| at + 5);
    let header = pdf_objects::parse::Error::Header { at: table + 5 };
    let why = Unwritten::Unread(crate::source::Unread::Parse(header));
    as_of_nothing(&file, table + 5, why);
}

/// An update after a reading whose sections or objects were read
/// elsewhere ([`Base::of`]): its `/Prev` where the reading read the newest
/// section, so that the update read back reads that section where it's
/// said, and its section listing each object the reading read at a shift,
/// or where a scan found it, where it is, so that it's read there as
/// written; each object as the
/// reading read it, and the update's; and its base holds its file, not
/// the file one byte longer, though its last `startxref` isn't the
/// `/Prev` the update writes.
#[test]
fn an_update_after_a_recovered_reading_reads_back_alike() {
    let (cut, _, main) = cut();
    let prefix = vec![b'~'; 100];
    let (start, _, first) = laid_out(&prefix, &OBJECTS, &|_, at| at, &|at| at);
    let (past, _, _) = laid_out(b"", &OBJECTS, &|_, at| at, &|at| at + 5);
    let (rows, _, table) = laid_out(b"", &OBJECTS, &|_, at| at + 2, &|at| at);
    let (two, _, two_table) = two_shifts();
    let rows_shift = Fault::Shifted {
        at: table,
        by: -2,
        from_start: false,
        moved: 4,
        scanned: 0,
        neither: 0,
    };
    // What the update read back says of the file before it: the newest
    // section is where its /Prev says, and so is no longer moved.
    let files = [
        (
            "cut",
            cut,
            vec![Fault::Moved {
                at: main + 3,
                found: main,
                from_start: false,
                moved: 1,
                scanned: 0,
                neither: 0,
            }],
        ),
        (
            "from the first byte",
            start,
            vec![Fault::Shifted {
                at: first,
                by: -100,
                from_start: true,
                moved: 4,
                scanned: 0,
                neither: 0,
            }],
        ),
        ("startxref past", past, vec![]),
        ("rows past", rows, vec![rows_shift]),
        (
            "two shifts",
            two,
            vec![Fault::Shifted {
                at: two_table,
                by: -5,
                from_start: false,
                moved: 2,
                scanned: 2,
                neither: 0,
            }],
        ),
    ];
    for (name, file, said) in files {
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let before = reading.to_document().unwrap();
        let base = Base::of(&reading).unwrap();
        let newest = reading.facts().newest.map(|(at, _)| at).unwrap();
        assert_eq!(base.prev, newest - base.header, "{name}");
        assert!(base.holds(&file[..]).unwrap(), "{name}");
        let longer = [&file[..], b"\n"].concat();
        assert!(!base.holds(&longer[..]).unwrap(), "{name}");
        let id = ObjectId::new(base.size, 0);
        let object = Object::new(Value::Integer(7));
        let bodies = [(id, Body::New(Cow::Borrowed(&object)))];
        let mut out = file.clone();
        let written = write::update(
            &mut out,
            &base,
            bodies,
            None,
            None,
            &write::Options::DEFAULT,
        );
        let written = written.unwrap().unwrap();
        assert!(written.base.holds(&out[..]).unwrap(), "{name}");
        let back = read(&out);
        assert_eq!(recoveries(&back), said, "{name}");
        // What the faults say of the file before it, faults off too.
        assert_eq!(back.facts.recovered, !said.is_empty(), "{name}");
        assert_eq!(back.trailer.unwritten, None, "{name}");
        for (id, object) in before.objects() {
            let mut got = back.get(id).unwrap().clone();
            let mut want = object.clone();
            got.unplace();
            want.unplace();
            assert_eq!(got, want, "{name}: {id}");
            // One read at a shift, or where a scan found it, the update
            // lists where it is.
            let (got, want) = (back.record(id).unwrap(), before.record(id).unwrap());
            let elsewhere = matches!(want.origin, Origin::Shifted { .. } | Origin::Recovered);
            let origin = if elsewhere { Origin::File } else { want.origin };
            assert_eq!((got.origin, got.at), (origin, want.at), "{name}: {id}");
            let listed = written.section.lists(id.number);
            assert_eq!(listed, elsewhere, "{name}: {id}");
        }
        assert_eq!(
            back.get(id).ok().and_then(Object::as_integer),
            Some(7),
            "{name}"
        );
    }
}

/// A file whose sections don't read, so that it's scanned, of a catalog
/// and objects in two object streams, 10 and 11, after it, and object 4
/// at an offset too: 10, whose data is hexadecimal without its end marker,
/// a fault its decode says, holds 2 and 3; 11, later, holds 3 again and 4.
/// The file, and how many bytes 10 decodes to, which is all the budget
/// spends, as 11 isn't filtered.
pub(crate) fn scanned_with_object_streams() -> (Vec<u8>, usize) {
    let stream = |n: u32, members: &[(u32, &str)], hex: bool| {
        let mut header = String::new();
        let mut body = String::new();
        for (n, value) in members {
            header.push_str(&format!("{n} {} ", body.len()));
            body.push_str(value);
            body.push(' ');
        }
        let data = format!("{header}{body}");
        let first = header.len();
        let (filter, written) = match hex {
            true => {
                let hex: String = data.bytes().map(|b| format!("{b:02X}")).collect();
                (" /Filter /ASCIIHexDecode", hex)
            }
            false => ("", data.clone()),
        };
        let object = format!(
            "{n} 0 obj\n<< /Type /ObjStm /N {} /First {first}{filter} /Length {} >>\nstream\n{written}\nendstream\nendobj\n",
            members.len(),
            written.len()
        );
        (object, data.len())
    };
    let (ten, ten_len) = stream(
        10,
        &[
            (2, "<< /Type /Pages /Kids [] /Count 0 >>"),
            (3, "(three, old)"),
        ],
        true,
    );
    let (eleven, eleven_len) = stream(11, &[(3, "(three)"), (4, "(four, in a stream)")], false);
    let mut file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec();
    file.extend_from_slice(ten.as_bytes());
    file.extend_from_slice(eleven.as_bytes());
    file.extend_from_slice(b"4 0 obj\n(four, at an offset)\nendobj\n");
    file.extend_from_slice(b"trailer\n<< /Size 12 /Root 1 0 R >>\nstartxref\n99999\n%%EOF\n");
    let _ = eleven_len;
    (file, ten_len)
}

/// A scanned file's object streams' members are listed: each by the later
/// stream that has it, the scan's offset of a number winning, every way
/// alike, with one fault; each stream decoded once, its fault said once,
/// and the budget spent once: a budget of the filtered stream's decoded
/// bytes, which a second decode would find spent, reads its members.
#[test]
fn a_scanned_files_object_streams_members_are_listed() {
    let (file, decoded) = scanned_with_object_streams();
    let doc = read(&file);
    assert!(doc.trailer.unwritten.is_some());
    let origin = |n: u32| doc.record(ObjectId::new(n, 0)).map(|r| r.origin);
    assert_eq!(origin(1), Some(Origin::Recovered));
    assert_eq!(
        origin(2),
        Some(Origin::Stream {
            stream: 10,
            index: 0
        })
    );
    assert_eq!(
        origin(3),
        Some(Origin::Stream {
            stream: 11,
            index: 0
        })
    );
    assert_eq!(origin(4), Some(Origin::Recovered));
    let three = doc.get(ObjectId::new(3, 0)).unwrap();
    assert_eq!(three.as_bytes(), Some(&b"three"[..]));
    let members = Fault::Members {
        objects: 2,
        streams: 2,
        unread: 0,
    };
    let told: Vec<&Fault> = doc.faults().iter().map(|(_, f)| f).collect();
    assert_eq!(
        told.iter().filter(|f| ***f == members).count(),
        1,
        "{told:?}"
    );
    let decodes = told
        .iter()
        .filter(|f| matches!(f, Fault::Decode { .. }))
        .count();
    assert_eq!(decodes, 1, "{told:?}");
    // The budget of what 10 decodes to reads its member, decoded once.
    let mut options = Options::DEFAULT;
    options.limits.budget = decoded;
    let reading = Reading::open(&file[..], &options).unwrap();
    let doc = reading.into_document().unwrap();
    let pages = doc.get(ObjectId::new(2, 0)).unwrap().as_dict().unwrap();
    assert_eq!(
        pages.get(b"Type").and_then(Object::as_name),
        Some(&b"Pages"[..])
    );
    let four = doc.get(ObjectId::new(4, 0)).unwrap();
    assert_eq!(four.as_bytes(), Some(&b"four, at an offset"[..]));
}

/// A file whose sections read lists only what they list: an object in an
/// object stream no section names isn't read, as a reference to it is to
/// no object (ISO 32000-2, 7.3.10), and nothing is said of it.
#[test]
fn a_member_no_section_lists_isnt_listed() {
    let (scanned, _) = scanned_with_object_streams();
    let end = String::from_utf8_lossy(&scanned).find("trailer").unwrap();
    let mut file = scanned[..end].to_vec();
    let at = |name: &[u8]| file.windows(name.len()).position(|w| w == name).unwrap();
    let (one, ten, eleven, four) = (
        at(b"1 0 obj"),
        at(b"10 0 obj"),
        at(b"11 0 obj"),
        at(b"4 0 obj"),
    );
    let table = file.len();
    let mut rows = String::from("xref\n0 1\n0000000000 65535 f \n1 1\n");
    rows.push_str(&format!("{one:010} 00000 n \n4 1\n{four:010} 00000 n \n"));
    rows.push_str(&format!(
        "10 2\n{ten:010} 00000 n \n{eleven:010} 00000 n \n"
    ));
    rows.push_str(&format!(
        "trailer\n<< /Size 12 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n"
    ));
    file.extend_from_slice(rows.as_bytes());
    let doc = read(&file);
    assert_eq!(doc.trailer.unwritten, None);
    assert!(doc.get(ObjectId::new(2, 0)).is_err());
    assert!(doc.get(ObjectId::new(3, 0)).is_err());
    assert!(!doc.facts.recovered);
    let members = doc
        .faults()
        .iter()
        .filter(|(_, f)| matches!(f, Fault::Members { .. }));
    assert_eq!(members.count(), 0);
}

/// [`OBJECTS`] written whole under AES-256, its password `user`, each in
/// an object stream, with a cross-reference stream, and its `startxref`
/// past its end, so that it's scanned.
pub(crate) fn scanned_encrypted_with_object_streams() -> Vec<u8> {
    use crate::XrefKind;
    use crate::write::Pack;
    use crate::write::tests::methods;

    let method = (methods().into_iter())
        .find(|m| m.name == "AES-256 created")
        .unwrap();
    let objects: Vec<(u32, Object)> = (OBJECTS.iter())
        .map(|&(n, body)| {
            (
                n,
                pdf_objects::parse::Parser::at(body.as_bytes(), 0)
                    .value(&mut ())
                    .unwrap(),
            )
        })
        .collect();
    let bodies = objects
        .iter()
        .map(|(n, o)| (ObjectId::new(*n, 0), Body::from(o)));
    let mut trailer = pdf_objects::object::Dict::new();
    trailer.set("Root", ObjectId::new(1, 0));
    let mut options = write::Options::DEFAULT;
    options.kind = XrefKind::Stream;
    options.pack = Some(Pack::DEFAULT);
    let mut file = Vec::new();
    write::full(
        &mut file,
        bodies,
        &trailer,
        Some(&method.encryptor),
        None,
        &options,
    )
    .unwrap();
    // Its startxref past its end, so that it's scanned.
    let at = file.windows(9).rposition(|w| w == b"startxref").unwrap();
    file.truncate(at);
    file.extend_from_slice(b"startxref\n999999\n%%EOF\n");
    file
}

/// A scanned encrypted file whose objects are in object streams: opened by
/// its password, its members listed, decrypted; opened by a wrong one, none
/// listed and the streams said unread; then unlocked by the password,
/// read as the file opened by it is, in memory and by position.
#[test]
fn a_scanned_encrypted_files_members_are_listed_once_it_opens() {
    use pdf_crypt::Password;

    let file = scanned_encrypted_with_object_streams();
    let user = [Password("user")];
    let by_password = |passwords: &[Password]| {
        let mut options = Options::DEFAULT;
        options.passwords = passwords;
        document(&file, &options)
    };
    let opened = by_password(&user);
    assert!(opened.trailer.unwritten.is_some());
    let member = |doc: &Document, n: u32| doc.record(ObjectId::new(n, 0)).map(|r| r.origin);
    for n in 1..=3 {
        assert!(
            matches!(member(&opened, n), Some(Origin::Stream { .. })),
            "{n}"
        );
    }
    let four = opened.get(ObjectId::new(4, 0)).unwrap();
    assert_eq!(four.as_bytes(), Some(&b"four"[..]));
    let said = |doc: &Document| -> Vec<Fault> {
        (doc.faults().iter())
            .filter(|(_, f)| matches!(f, Fault::Members { .. }))
            .map(|(_, f)| f.clone())
            .collect()
    };
    let listed = said(&opened);
    assert!(
        matches!(
            &listed[..],
            [Fault::Members {
                objects: 4,
                unread: 0,
                ..
            }]
        ),
        "{listed:?}"
    );
    // Locked: none listed, each stream said unread.
    let locked = by_password(&[Password("wrong")]);
    assert!(locked.get(ObjectId::new(1, 0)).is_err());
    let unread = said(&locked);
    assert!(
        matches!(
            &unread[..],
            [Fault::Members {
                objects: 0,
                streams: 0,
                unread: 1..
            }]
        ),
        "{unread:?}"
    );
    // Unlocked later, as opened by the password, every way.
    for first in [None, Some(7)] {
        let mut options = Options::DEFAULT;
        let wrong = [Password("wrong")];
        options.passwords = &wrong;
        let unlocked = match first {
            None => Reading::open(&file[..], &options)
                .unwrap()
                .unlock(&user[0].encodings()),
            Some(first) => {
                let source = crate::fuzz::Counted::new(&file, first);
                let reading = crate::fuzz::open(&source, &wrong, &Options::DEFAULT, true).unwrap();
                let doc = reading
                    .unlock(&user[0].encodings())
                    .ok()
                    .unwrap()
                    .into_document()
                    .unwrap();
                assert_eq!(doc, opened, "by position");
                continue;
            }
        };
        let doc = unlocked.ok().unwrap().into_document().unwrap();
        assert_eq!(doc, opened);
    }
}

/// A file's header, `body`, objects [`OBJECTS`] and a `trailer` naming the
/// catalog, then `after`: no `startxref`, so the file is scanned. Also
/// each object's place.
fn scanned_after(body: &[u8], after: &[u8]) -> (Vec<u8>, Vec<(u32, usize)>) {
    let mut file = b"%PDF-1.7\n".to_vec();
    file.extend_from_slice(body);
    let mut places = Vec::new();
    for &(n, text) in &OBJECTS {
        places.push((n, file.len()));
        file.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
    }
    file.extend_from_slice(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n");
    file.extend_from_slice(after);
    (file, places)
}

/// The faults `doc` says of the streams a scan ended at their `endobj`, or
/// found no end to.
fn stream_ends(doc: &Document) -> Vec<(Place, Fault)> {
    (doc.faults().iter())
        .filter(|(_, f)| matches!(f, Fault::StreamEnd { .. }))
        .cloned()
        .collect()
}

/// The fault a scan says of a stream whose data starts at byte `start`,
/// ended at its `endobj` at byte `end`, or found no end to where that's
/// none, at its place.
fn ended(id: Option<ObjectId>, start: usize, end: Option<usize>) -> (Place, Fault) {
    let fault = Fault::StreamEnd { id, start, end };
    let at = Span::new(None, start, end.unwrap_or(start));
    (Place::new(None, Some(at)), fault)
}

/// A scanned file whose first object is a stream with no `endstream`, its
/// `/Length` past its data: the scan ends its data at its `endobj`, as
/// PDFium's does, and reads on, so the catalog, its page and the trailer
/// after it are found, every way, and says where it ended it; and one at
/// the file's end without `endobj` either, to which the scan finds no end,
/// and says so. Under [`Recovery::Pdfium`] alike. The first stream's own
/// read takes its data to that `endobj`, as the scan ended it, and reads;
/// the last's finds no `endstream` after its data, and it doesn't read.
#[test]
fn a_scanned_stream_without_endstream_ends_at_its_endobj_and_what_follows_reads() {
    let stream = b"9 0 obj\n<< /Length 999 >>\nstream\nq Q\n";
    let (first, places) = scanned_after(&[&stream[..], b"endobj\n"].concat(), b"");
    let start = b"%PDF-1.7\n".len() + stream.len() - 4;
    let endobj = b"%PDF-1.7\n".len() + stream.len();
    let (last, last_places) = scanned_after(b"", stream);
    let last_start = last.len() - 4;
    let nine = Some(ObjectId::new(9, 0));
    let cases = [
        (
            &first,
            &places,
            ended(nine, start, Some(endobj)),
            Some(endobj),
        ),
        (&last, &last_places, ended(nine, last_start, None), None),
    ];
    for (file, places, said, end) in cases {
        let doc = read(file);
        assert!(doc.trailer.unwritten.is_some());
        each_object_at(&doc, places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert_eq!(stream_ends(&doc), std::slice::from_ref(&said));
        let data = doc.record(ObjectId::new(9, 0)).and_then(|r| r.data);
        assert_eq!(
            data.map(|d| (d.at.start, d.at.end)),
            end.map(|end| (start, end))
        );
        assert_eq!(doc.get(ObjectId::new(9, 0)).is_ok(), end.is_some());
        let mut pdfium = Options::DEFAULT;
        pdfium.recovery = Recovery::Pdfium;
        let doc = document(file, &pdfium);
        each_object_at(&doc, places, &|_| Origin::Recovered);
        assert_eq!(stream_ends(&doc), std::slice::from_ref(&said), "pdfium");
        assert_eq!(
            doc.get(ObjectId::new(9, 0)).is_ok(),
            end.is_some(),
            "pdfium"
        );
    }
}

/// A scanned file's stream whose data holds `endobj` and what reads as an
/// object's `n g obj` after it, its `/Length` the data's, which
/// `endstream` follows: the scan passes over its data by its `/Length`,
/// so what's in it isn't taken for an object, and nothing's said of its
/// end.
#[test]
fn a_scanned_streams_data_holding_endobj_is_passed_by_its_length() {
    let data = "BT (endobj) Tj ET\nendobj\n7 0 obj\n<< /Type /Catalog >>\nendobj\n";
    let stream = format!(
        "9 0 obj\n<< /Length {} >>\nstream\n{data}\nendstream\nendobj\n",
        data.len()
    );
    let (file, places) = scanned_after(stream.as_bytes(), b"");
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert_eq!(stream_ends(&doc), []);
    assert!(doc.get(ObjectId::new(7, 0)).is_err());
    let nine = doc.record(ObjectId::new(9, 0)).unwrap();
    assert!(nine.data.unwrap().declared);
}

/// A scanned file whose last stream's data is a document of its own, an
/// `endobj` and object 3 among it, its `/Length` a reference, then
/// `endstream`: the verified recovery ends the data at that `endobj`, reads
/// on, and meets `endstream` on its own, so the data ran there. What it
/// found inside is taken back, so the file's own object 3 stands, nothing
/// in the data is taken for an object, and nothing's said of its end,
/// every way; the stream's own read takes its data by its `/Length`.
/// Under [`Recovery::Pdfium`], the data's object 3 stands over the file's,
/// as PDFium and qpdf read it, and the scan says where it ended the data.
/// Where the stream has no `endstream` of its own, the first after its data
/// a later stream's, which the scan meets as it passes that one's data, or
/// inside a later object whose `stream` keyword is missing, the objects
/// between stand.
#[test]
fn a_document_in_a_scanned_streams_data_doesnt_stand_over_the_files_own_objects() {
    let data = "%PDF-1.4\n7 0 obj\n<< /Type /Catalog >>\nendobj\n3 0 obj\n<< /Embedded true >>\nendobj\n%%EOF\n";
    let head = "9 0 obj\n<< /Type /EmbeddedFile /Length 8 0 R >>\nstream\n";
    let length = format!("8 0 obj\n{}\nendobj\n", data.len());
    let after = format!("{head}{data}\nendstream\nendobj\n{length}");
    let (file, places) = scanned_after(b"", after.as_bytes());
    let start = file.len() - after.len() + head.len();
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert!(doc.get(ObjectId::new(7, 0)).is_err());
    assert_eq!(stream_ends(&doc), []);
    let nine = doc.record(ObjectId::new(9, 0)).unwrap().data.unwrap();
    assert_eq!((nine.at.start, nine.at.end), (start, start + data.len()));
    assert!(nine.declared);
    let doc = document(&file, &as_pdfium());
    let three = doc.get(ObjectId::new(3, 0)).unwrap().as_dict().unwrap();
    assert!(three.has(b"Embedded"));
    let endobj = start + data.find("endobj").unwrap();
    let said = ended(Some(ObjectId::new(9, 0)), start, Some(endobj));
    assert_eq!(stream_ends(&doc), [said]);

    let unended = "9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n";
    let later = "5 0 obj\n<< /Length 3 >>\nstream\nq Q\nendstream\nendobj\n";
    let (file, mut places) = scanned_after(unended.as_bytes(), later.as_bytes());
    places.push((5, file.len() - later.len()));
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    let start = b"%PDF-1.7\n".len() + unended.len() - "q Q\nendobj\n".len();
    let said = ended(
        Some(ObjectId::new(9, 0)),
        start,
        Some(start + "q Q\n".len()),
    );
    assert_eq!(stream_ends(&doc), std::slice::from_ref(&said));

    let keywordless = "5 0 obj\n<< /Length 3 >>\nq Q\nendstream\nendobj\n";
    let (file, places) = scanned_after(unended.as_bytes(), keywordless.as_bytes());
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert_eq!(stream_ends(&doc), [said]);
}

/// [`two_shifts`], a stream without `endstream` written before objects 3
/// and 4, which are at neither place: the scan that finds them ends the
/// stream's data at its `endobj`, finds them past it and says so, every
/// way.
#[test]
fn an_entry_a_scan_finds_past_a_stream_without_endstream_is_read_there() {
    let five = (5, "<< /Length 999 >>\nstream\nq Q");
    let objects = [OBJECTS[0], OBJECTS[1], five, OBJECTS[2], OBJECTS[3]];
    let row = |n: u32, at: usize| if n <= 2 { at + 5 } else { at - 4 };
    let (file, places, _) = laid_out(b"", &objects, &row, &|at| at + 5);
    let doc = read(&file);
    let origin = |n| match n {
        1 | 2 => Origin::Shifted { by: -5 },
        _ => Origin::Recovered,
    };
    let page = places.iter().filter(|&&(n, _)| n != 5).copied();
    each_object_at(&doc, &page.collect::<Vec<_>>(), &origin);
    reaches_its_page(&doc);
    let (_, at) = places[2];
    let start = at + "5 0 obj\n<< /Length 999 >>\nstream\n".len();
    let end = start + "q Q\n".len();
    let said = ended(Some(ObjectId::new(5, 0)), start, Some(end));
    assert_eq!(stream_ends(&doc), [said]);
}

/// The options of a reading under [`Recovery::Pdfium`].
fn as_pdfium() -> Options<'static> {
    let mut options = Options::DEFAULT;
    options.recovery = Recovery::Pdfium;
    options
}

/// A scanned file whose stream has neither `endstream` nor `endobj`, a
/// word of its own, after its data, then its page, without `endobj`
/// either: the scan finds no end to the stream's data and reads on through
/// it, as PDFium's does, so the page is found, every way, and says so.
/// Under [`Recovery::Pdfium`] alike. Where the data ends in `endstream` run
/// into a byte before it, the verified recovery ends it there, as it ends
/// a stream at `endstream` wherever it's found, and says nothing, where
/// [`Recovery::Pdfium`] reads on through it, as PDFium does, and says so.
#[test]
fn a_scanned_stream_with_no_end_is_read_on_through_and_what_follows_reads() {
    let head = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\ntrailer\n<< /Size 10 /Root 1 0 R >>\n9 0 obj\n<< /Length 999 >>\nstream\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << >> >>\n";
    let nine = Some(ObjectId::new(9, 0));
    let no_end = vec![ended(nine, head.len(), None)];
    for (data, verified) in [(&b"q Q\n"[..], &no_end), (b"q Q\x10endstream\n", &vec![])] {
        let file = [&head[..], data, page].concat();
        for (doc, said) in [
            (read(&file), verified),
            (document(&file, &as_pdfium()), &no_end),
        ] {
            reaches_its_page(&doc);
            let at = doc.record(ObjectId::new(3, 0)).unwrap().at.start;
            assert_eq!(at, head.len() + data.len());
            assert_eq!(&stream_ends(&doc), said);
        }
    }
}

/// A scanned file whose first object is a stream whose data ends in
/// `endstream` run into `endobj`, neither a word of its own, then the
/// catalog, its page and the trailer: the verified recovery ends its data
/// at `endstream`, wherever it's found, so each object is found, every way,
/// and nothing's said; [`Recovery::Pdfium`] ends it at the catalog's
/// `endobj`, the first word of its own after it, as PDFium does, so the
/// catalog isn't found, and says so.
#[test]
fn a_scanned_streams_endstream_run_into_endobj_ends_it_under_the_verified_recovery() {
    let stream = b"9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendstreamendobj\n";
    let (file, places) = scanned_after(stream, b"");
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert_eq!(stream_ends(&doc), []);
    let doc = document(&file, &as_pdfium());
    assert!(doc.get(ObjectId::new(1, 0)).is_err());
    let start = b"%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\n".len();
    let (_, catalog) = places[0];
    let endobj = catalog + format!("1 0 obj\n{}\n", OBJECTS[0].1).len();
    let said = ended(Some(ObjectId::new(9, 0)), start, Some(endobj));
    assert_eq!(stream_ends(&doc), [said]);
}

/// A scanned file whose first stream no `n g obj` comes before, its data
/// up to the catalog's `endobj`: the verified recovery passes over its data
/// to that `endobj`, as it passes over any stream's, and says so, so the
/// catalog isn't found; [`Recovery::Pdfium`] reads on through it, as
/// PDFium's scan reads through any words but an object's, so every object
/// is found and nothing's said. An `endstream` after the trailer, the
/// first after the stream's data, takes nothing back, as the stream is no
/// object's.
#[test]
fn a_stream_no_n_g_obj_comes_before_is_read_through_as_pdfium_reads_it() {
    for after in [&b""[..], b"endstream\n"] {
        let (file, places) = scanned_after(b"<< /Length 999 >>\nstream\n", after);
        let doc = read(&file);
        assert!(doc.get(ObjectId::new(1, 0)).is_err());
        each_object_at(&doc, &places[1..], &|_| Origin::Recovered);
        let start = b"%PDF-1.7\n<< /Length 999 >>\nstream\n".len();
        let (_, catalog) = places[0];
        let endobj = catalog + format!("1 0 obj\n{}\n", OBJECTS[0].1).len();
        assert_eq!(stream_ends(&doc), [ended(None, start, Some(endobj))]);
        let doc = document(&file, &as_pdfium());
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert_eq!(stream_ends(&doc), []);
    }
}

/// A scanned file whose last stream's `/Length` ends at its `endstream`,
/// past an `endstream` inside its data, after which its data holds object
/// 5, found nowhere else, and object 4, found before it: the verified
/// recovery passes the data by its `/Length`, and its second scan, to each
/// stream's first `endstream`, skips that data, which is the stream's, so
/// object 5 isn't read, every way, object 4 is read where the first found
/// it, and nothing's said; under [`Recovery::Pdfium`] alike, as PDFium
/// passes the data by its `/Length`.
#[test]
fn an_object_inside_a_streams_data_passed_by_its_length_isnt_read() {
    let data = "q Q\nendstream\nendobj\n5 0 obj\n(five)\nendobj\n4 0 obj\n(not four)\nendobj\n";
    let head = format!("9 0 obj\n<< /Length {} >>\nstream\n", data.len());
    let (file, places) =
        scanned_after(b"", format!("{head}{data}\nendstream\nendobj\n").as_bytes());
    for options in [Options::DEFAULT, as_pdfium()] {
        let how = options.recovery;
        let doc = read_as(&file, &options);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert!(doc.get(ObjectId::new(5, 0)).is_err(), "{how:?}");
        let nine = doc.record(ObjectId::new(9, 0)).unwrap().data.unwrap();
        assert!(nine.declared);
        assert_eq!(nine.at.end - nine.at.start, data.len());
        assert_eq!(stream_ends(&doc), [], "{how:?}");
    }
}

/// A scanned file whose last stream, after the trailer, holds a document of
/// its own as its data, its `/Length` ending at its `endstream`: the
/// document's stream 7, then its catalog 8, a number the file doesn't use,
/// and its trailer, which names that catalog. The verified recovery passes
/// the data by its `/Length`, over 7's `endstream`, and its second scan
/// skips it, so neither the document's objects nor its trailer are taken,
/// every way: the file's own catalog is the root, its page is reached, and
/// what's said is what's said where the data holds no document but its
/// `%%EOF`. Under [`Recovery::Pdfium`] alike, as PDFium passes the data by
/// its `/Length`.
#[test]
fn a_document_in_data_passed_by_its_length_gives_no_object_nor_trailer() {
    let data = "%PDF-1.4\n7 0 obj\n<< /Length 3 >>\nstream\nq Q\nendstream\nendobj\n8 0 obj\n<< /Type /Catalog /Pages 9 0 R >>\nendobj\ntrailer\n<< /Size 9 /Root 8 0 R >>\n%%EOF\n";
    let embedded = |data: &str| {
        format!(
            "6 0 obj\n<< /Type /EmbeddedFile /Length {} >>\nstream\n{data}\nendstream\nendobj\n",
            data.len()
        )
    };
    let (file, places) = scanned_after(b"", embedded(data).as_bytes());
    // Data as long, ending as the document does: the bytes past the last
    // `%%EOF` are said wherever it is.
    let eof = "%%EOF\n";
    let plain = format!("{}{eof}", "x".repeat(data.len() - eof.len()));
    let (plain, _) = scanned_after(b"", embedded(&plain).as_bytes());
    for options in [Options::DEFAULT, as_pdfium()] {
        let how = options.recovery;
        let doc = read_as(&file, &options);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        let root = doc.trailer.dict().get(b"Root").map(Object::value);
        assert_eq!(root, Some(&Value::Ref(ObjectId::new(1, 0))), "{how:?}");
        for n in [7, 8] {
            assert!(doc.get(ObjectId::new(n, 0)).is_err(), "{n}: {how:?}");
        }
        let six = doc.record(ObjectId::new(6, 0)).unwrap().data.unwrap();
        assert!(six.declared);
        assert_eq!(six.at.end - six.at.start, data.len());
        assert_eq!(doc.faults(), document(&plain, &options).faults());
    }
}

/// A scanned file whose stream 9 has no `/Length` that reads and an
/// `endobj` of its own in its data, after which the data holds stream 7,
/// whose `/Length` ends at its `endstream`, past an `endstream` inside its
/// data, the first after 9's, then object 5, found nowhere else: the
/// verified scan ends 9's data at that `endobj`, and passes 7's by its
/// `/Length`. Its second scan, reading 9's data to its first `endstream`,
/// comes to rest inside 7's data, and skips the rest of it, so 5 isn't
/// read, every way, and the scan says where it ended 9's data. Under
/// [`Recovery::Pdfium`] alike.
#[test]
fn data_passed_by_its_length_is_skipped_where_the_second_scan_comes_to_rest_inside() {
    let inner = "q Q\nendstream\n5 0 obj\n(five)\nendobj\n";
    let body = format!(
        "9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n7 0 obj\n<< /Length {} >>\nstream\n{inner}\nendstream\nendobj\nendstream\nendobj\n",
        inner.len()
    );
    let (file, places) = scanned_after(body.as_bytes(), b"");
    let start = b"%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\n".len();
    let said = ended(NINE, start, Some(start + "q Q\n".len()));
    for options in [Options::DEFAULT, as_pdfium()] {
        let how = options.recovery;
        let doc = read_as(&file, &options);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert!(doc.get(ObjectId::new(5, 0)).is_err(), "{how:?}");
        assert!(doc.get(ObjectId::new(7, 0)).is_ok());
        assert_eq!(stream_ends(&doc), std::slice::from_ref(&said));
    }
}

/// A scanned file whose first stream has no `/Length` that reads and an
/// `endobj` of its own in its data, after which what the data holds, a
/// string closed late or a comment, hides its `endstream` and the catalog,
/// its page and the trailer from a scan reading on from that `endobj`,
/// each object's `n g obj` written after the last one's `endobj` on its
/// line, as a writer running objects together writes them: the verified
/// scan ends the data at that `endobj` and finds nothing past it, so its
/// second scan, to each stream's first `endstream`, gives it every object
/// and the trailer, every way; it says where it ended the data.
#[test]
fn what_a_scan_reading_on_from_an_endobj_loses_its_second_scan_finds() {
    let head = "%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\n";
    let start = head.len();
    let end = start + "q Q\n".len();
    // A string closed late, by a `)` after the trailer, and a comment, to
    // the line's end, each of every object and the trailer.
    let shapes = [
        (
            "(late\nendstream\nendobj",
            "\n",
            " trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n)\n",
        ),
        (
            "%late endstream endobj",
            " ",
            " trailer << /Size 10 /Root 1 0 R >>\n",
        ),
    ];
    for (hiding, line, after) in shapes {
        let mut file = format!("{head}q Q\nendobj\n{hiding}").into_bytes();
        let mut places = Vec::new();
        for &(n, text) in &OBJECTS {
            file.push(b' ');
            places.push((n, file.len()));
            file.extend_from_slice(format!("{n} 0 obj{line}{text}{line}endobj").as_bytes());
        }
        file.extend_from_slice(after.as_bytes());
        let doc = read(&file);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        let root = (doc.trailer.object.as_dict())
            .and_then(|trailer| trailer.get(b"Root"))
            .map(Object::value);
        assert_eq!(root, Some(&Value::Ref(ObjectId::new(1, 0))), "{hiding}");
        let said = ended(Some(ObjectId::new(9, 0)), start, Some(end));
        assert_eq!(stream_ends(&doc), [said], "{hiding}");
        assert_eq!(unclosed(&doc), [], "{hiding}");
    }
}

/// A stream whose `endstream` is the first after the data of a stream
/// written before it without one.
pub(crate) const LATER: &[u8] = b"5 0 obj\n<< /Length 3 >>\nstream\nq Q\nendstream\nendobj\n";

/// A scanned file whose stream 9, its first object, has no `endstream`,
/// its `/Length` 999 past its data: `stream` and then `eol`, its data `q
/// Q` and an end of line, and `endobj` and then `after`; then [`OBJECTS`]
/// and the trailer, and [`LATER`] past them. The file, each of
/// [`OBJECTS`]' places, and where 9's `stream` keyword ends and where its
/// `endobj` is.
pub(crate) fn without_endstream(
    eol: &[u8],
    after: &[u8],
) -> (Vec<u8>, Vec<(u32, usize)>, usize, usize) {
    let head = [&b"9 0 obj\n<< /Length 999 >>\nstream"[..], eol].concat();
    let nine = [&head[..], b"q Q\nendobj", after].concat();
    let (file, places) = scanned_after(&nine, LATER);
    let keyword = b"%PDF-1.7\n".len() + head.len() - eol.len();
    let endobj = b"%PDF-1.7\n".len() + head.len() + b"q Q\n".len();
    (file, places, keyword, endobj)
}

/// The faults `doc` says on object `n`.
fn on(doc: &Document, n: u32) -> Vec<(Place, Fault)> {
    let id = Some(ObjectId::new(n, 0));
    (doc.faults().iter())
        .filter(|(place, _)| place.object == id)
        .cloned()
        .collect()
}

/// The fault that stream `n`'s `/Length`, `length`, doesn't end at
/// `endstream`, at its value, as `doc` holds the stream.
fn length_of(doc: &Document, n: u32, length: usize) -> (Place, Fault) {
    let id = ObjectId::new(n, 0);
    let dict = doc.get(id).unwrap().as_dict().unwrap();
    let at = dict.get(b"Length").unwrap().at;
    let length = Some(length);
    (Place::new(Some(id), at), Fault::Length { length })
}

/// Where stream `n`'s data is, as `doc` read it, and whether its `/Length`
/// ended it.
fn extent(doc: &Document, n: u32) -> (usize, usize, bool) {
    let data = doc.record(ObjectId::new(n, 0)).unwrap().data.unwrap();
    (data.at.start, data.at.end, data.declared)
}

/// [`without_endstream`]'s file: the scan ends stream 9's data at its
/// `endobj`, and 9's own read takes its data to there, as the scan does,
/// not to the first `endstream` after it, [`LATER`]'s, every way, by
/// position in windows of each size the tests read in among them, and
/// with faults off: `q Q` and the end of line before `endobj`, as data
/// ended at `endstream` keeps the one before that, decoded so. Said once,
/// that its `/Length` doesn't end at `endstream`, and once, where the scan
/// ended it. The stream is the one the parser makes of it with `endstream`
/// there, but placed from its dictionary to its data's end; its record from
/// its `n` to past its `endobj`. Under [`Recovery::Pdfium`] alike, as
/// PDFium's read of a stream ends it at the first `endstream` or `endobj`.
#[test]
fn a_scanned_stream_ended_at_its_endobj_has_its_data_to_there() {
    let (file, places, keyword, endobj) = without_endstream(b"\n", b"\n");
    let start = keyword + 1;
    // The file with `endstream` before 9's `endobj`.
    let twin = [&file[..endobj], b"endstream\n", &file[endobj..]].concat();
    let nine = ObjectId::new(9, 0);
    for options in [Options::DEFAULT, as_pdfium()] {
        let how = options.recovery;
        let doc = read_as(&file, &options);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert_eq!(extent(&doc, 9), (start, endobj, false), "{how:?}");
        assert_eq!(&file[start..endobj], b"q Q\n");
        let reading = Reading::open(&file[..], &options).unwrap();
        let decoded = reading.decoded_within(nine, usize::MAX).unwrap();
        assert_eq!(decoded, b"q Q\n", "{how:?}");
        assert_eq!(on(&doc, 9), [length_of(&doc, 9, 999)], "{how:?}");
        let said = ended(NINE, start, Some(endobj));
        assert_eq!(stream_ends(&doc), [said], "{how:?}");
        let record = doc.record(nine).unwrap();
        let n = b"%PDF-1.7\n".len();
        let past = endobj + b"endobj".len();
        assert_eq!(
            (record.at.start, record.at.end, record.endobj),
            (n, past, true)
        );
        let twin = document(&twin, &options);
        assert_eq!(extent(&twin, 9), (start, endobj, false), "{how:?}");
        let (object, made) = (doc.get(nine).unwrap(), twin.get(nine).unwrap());
        assert_eq!(object.value(), made.value(), "{how:?}");
        let (at, made) = (object.at.unwrap(), made.at.unwrap());
        assert_eq!(made.end, endobj + b"endstream".len(), "{how:?}");
        assert_eq!(at, Span::new(None, made.start, endobj), "{how:?}");
    }
}

/// [`without_endstream`]'s file, `stream` followed by spaces before its end
/// of line: 9's data starts right after `stream` as its parser reads it,
/// where the scan's starts past that end of line, and its own read still
/// takes it to its `endobj`, as the scan ended it, every way; the end of
/// line not right after `stream` said, as any stream's.
#[test]
fn a_scanned_stream_ended_at_its_endobj_after_spaces_past_stream_has_its_data_to_there() {
    let (file, places, keyword, endobj) = without_endstream(b"   \n", b"\n");
    for options in [Options::DEFAULT, as_pdfium()] {
        let how = options.recovery;
        let doc = read_as(&file, &options);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        assert_eq!(extent(&doc, 9), (keyword, endobj, false), "{how:?}");
        let said = ended(NINE, keyword + 4, Some(endobj));
        assert_eq!(stream_ends(&doc), [said], "{how:?}");
        let faults = on(&doc, 9);
        assert!(faults.contains(&length_of(&doc, 9, 999)), "{faults:?}");
        let eol = |(_, f): &(Place, Fault)| matches!(f, Fault::Parse { kind, .. } if *kind == ParseKind::NoEol);
        assert_eq!(faults.iter().filter(|f| eol(f)).count(), 1, "{how:?}");
    }
}

/// A scanned file whose stream 9 has its data on its `stream` keyword's
/// line, its `endstream` there too and its `endobj` on the next, its
/// `/Length` 999 past its data, with [`LATER`] after the trailer and
/// without: the scan starts the data on the next line, as PDFium's does,
/// at the `endobj`, and ends it there, empty, and says so; the parser
/// starts it right after `stream`, and the bytes between, which the scan
/// didn't look at, hold the `endstream`. So 9's own read ends its data at
/// that `endstream`, as any stream's is read, not at the `endobj` with the
/// `endstream` inside, every way; under [`Recovery::Pdfium`] alike. The
/// stream is the parser's, placed from its dictionary to past `endstream`,
/// its record from its `n` to past its `endobj`.
#[test]
fn a_scanned_stream_with_endstream_on_its_stream_line_has_its_data_to_that_endstream() {
    let nine = b"9 0 obj\n<< /Length 999 >> stream q Q endstream\nendobj\n";
    let n = b"%PDF-1.7\n".len();
    let find = |word: &[u8]| n + nine.windows(word.len()).position(|w| w == word).unwrap();
    let (start, endstream, endobj) = (find(b" q Q"), find(b"endstream"), find(b"endobj"));
    let id = ObjectId::new(9, 0);
    for after in [LATER, b""] {
        let (file, places) = scanned_after(nine, after);
        assert_eq!(&file[start..endstream], b" q Q ");
        for options in [Options::DEFAULT, as_pdfium()] {
            let how = options.recovery;
            let doc = read_as(&file, &options);
            each_object_at(&doc, &places, &|_| Origin::Recovered);
            reaches_its_page(&doc);
            assert_eq!(extent(&doc, 9), (start, endstream, false), "{how:?}");
            let reading = Reading::open(&file[..], &options).unwrap();
            let decoded = reading.decoded_within(id, usize::MAX).unwrap();
            assert_eq!(decoded, b" q Q ", "{how:?}");
            let said = ended(NINE, endobj, Some(endobj));
            assert_eq!(stream_ends(&doc), [said], "{how:?}");
            let faults = on(&doc, 9);
            assert!(faults.contains(&length_of(&doc, 9, 999)), "{faults:?}");
            let eol = |(_, f): &(Place, Fault)| matches!(f, Fault::Parse { kind, .. } if *kind == ParseKind::NoEol);
            assert_eq!(faults.iter().filter(|f| eol(f)).count(), 1, "{how:?}");
            assert_eq!(faults.len(), 2, "{how:?}: {faults:?}");
            let record = doc.record(id).unwrap();
            let past = endobj + b"endobj".len();
            assert_eq!(
                (record.at.start, record.at.end, record.endobj),
                (n, past, true),
                "{how:?}"
            );
            let dict = n + b"9 0 obj\n".len();
            let at = doc.get(id).unwrap().at;
            let made = Span::new(None, dict, endstream + b"endstream".len());
            assert_eq!(at, Some(made), "{how:?}");
        }
    }
}

/// A scanned file whose stream 9 has `endstream` run into the byte 0x10
/// before it at its data's end, its `endobj` after it, its `/Length` 999
/// past its data, and [`LATER`] after the trailer: the verified recovery's
/// scan ends the data at that `endstream`, as it ends one wherever it's
/// written, and the object's read too, every way, nothing said of it. Under
/// [`Recovery::Pdfium`] the scan takes only a word of its own, as PDFium's
/// does, so it ends the data at the `endobj`, and says so; and the
/// object's read takes the data to there, the `endstream` inside, as
/// PDFium's read of it does, every way: the bytes the scan looked at for
/// an `endstream` are read by its word.
#[test]
fn a_stream_with_endstream_run_into_its_data_is_read_to_its_endobj_as_pdfium_reads_it() {
    let (file, places, keyword, endobj) = without_endstream(b"\n", b"\n");
    let file = [&file[..endobj], b"\x10endstream\n", &file[endobj..]].concat();
    let places: Vec<(u32, usize)> = (places.iter()).map(|&(n, at)| (n, at + 11)).collect();
    let (start, endstream, endobj) = (keyword + 1, endobj + 1, endobj + 11);
    assert_eq!(&file[start..endobj], b"q Q\n\x10endstream\n");
    let verified = read(&file);
    each_object_at(&verified, &places, &|_| Origin::Recovered);
    assert_eq!(extent(&verified, 9), (start, endstream, false));
    assert_eq!(stream_ends(&verified), []);
    let pdfium = read_as(&file, &as_pdfium());
    each_object_at(&pdfium, &places, &|_| Origin::Recovered);
    reaches_its_page(&pdfium);
    assert_eq!(extent(&pdfium, 9), (start, endobj, false));
    assert_eq!(stream_ends(&pdfium), [ended(NINE, start, Some(endobj))]);
}

/// [`without_endstream`]'s file, 9's `endobj` followed by the byte 0x80,
/// white-space to PDFium and a regular byte to ISO 32000-2 (7.2.3): the
/// scan, by PDFium's white-space, ends 9's data at that `endobj`, and its
/// own read takes its data to there, every way; but the reader's tokens
/// read no `endobj` there, so it's said that the object has none, as the
/// parser says it past `endstream`, and its record ends at its data's end.
#[test]
fn a_scanned_stream_ended_at_an_endobj_iso_doesnt_read_says_it_has_none() {
    let (file, places, keyword, endobj) = without_endstream(b"\n", b"\x80\n");
    let start = keyword + 1;
    let nine = ObjectId::new(9, 0);
    for options in [Options::DEFAULT, as_pdfium()] {
        let how = options.recovery;
        let doc = read_as(&file, &options);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert_eq!(extent(&doc, 9), (start, endobj, false), "{how:?}");
        assert_eq!(stream_ends(&doc), [ended(NINE, start, Some(endobj))]);
        let at = doc.get(nine).unwrap().at;
        assert_eq!(at.map(|at| at.end), Some(endobj), "{how:?}");
        let none = Fault::Parse {
            kind: ParseKind::NoEndobj,
            text: "9 0 obj has no endobj".into(),
        };
        let said = [length_of(&doc, 9, 999), (Place::new(Some(nine), at), none)];
        assert_eq!(on(&doc, 9), said, "{how:?}");
        let record = doc.record(nine).unwrap();
        assert_eq!((record.at.end, record.endobj), (endobj, false), "{how:?}");
    }
}

/// A scanned file whose stream 9's `/Length` is a reference that ends its
/// data at `endstream`, the data holding `endobj` and then a comment that
/// `endstream` is in, so the scan ends 9's data at that `endobj` and,
/// reading on, never meets the `endstream`: 9's own read takes its data by
/// its `/Length`, every way, which checks, so nothing's said of it but
/// where the scan ended it. Read by position, a window that holds that
/// `endobj` but not where the `/Length` ends the data is read again in a
/// larger one, as for any stream. Under [`Recovery::Pdfium`] alike.
#[test]
fn a_scanned_stream_whose_length_by_reference_checks_is_read_by_it() {
    let data = format!("q Q endobj %{}", "x".repeat(100));
    let head = "9 0 obj\n<< /Length 8 0 R >>\nstream\n";
    let nine = format!(
        "{head}{data}endstream\nendobj\n8 0 obj\n{}\nendobj\n",
        data.len()
    );
    let (file, places) = scanned_after(nine.as_bytes(), b"");
    let start = b"%PDF-1.7\n".len() + head.len();
    let endobj = start + data.find("endobj").unwrap();
    for options in [Options::DEFAULT, as_pdfium()] {
        let how = options.recovery;
        let doc = read_as(&file, &options);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert_eq!(
            extent(&doc, 9),
            (start, start + data.len(), true),
            "{how:?}"
        );
        assert!(
            doc.record(ObjectId::new(9, 0))
                .unwrap()
                .data
                .unwrap()
                .indirect_length
        );
        assert_eq!(on(&doc, 9), [], "{how:?}");
        assert_eq!(
            stream_ends(&doc),
            [ended(NINE, start, Some(endobj))],
            "{how:?}"
        );
    }
}

/// A scanned file with no `trailer`, whose cross-reference stream 6, its
/// `/Length` a reference no read follows, has no `endstream`, the first
/// after its data [`LATER`]'s: the scan ends 6's data at its `endobj`, and
/// the open's search for the trailer, from the file's end, reads 6 to
/// there, as its own read does, so the trailer is 6 as read, every way.
/// Under [`Recovery::Pdfium`], whose rebuild reads each stream naming
/// `/XRef` for its dictionary, alike.
#[test]
fn a_scanned_cross_reference_stream_ended_at_its_endobj_is_the_trailer_read_to_there() {
    let mut file = b"%PDF-1.7\n".to_vec();
    let mut places = Vec::new();
    for &(n, text) in &OBJECTS {
        places.push((n, file.len()));
        file.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
    }
    let six = b"6 0 obj\n";
    let dict = file.len() + six.len();
    file.extend_from_slice(six);
    file.extend_from_slice(
        b"<< /Type /XRef /Size 7 /W [1 1 1] /Root 1 0 R /Length 8 0 R >>\nstream\n",
    );
    let start = file.len();
    file.extend_from_slice(b"q Q\n");
    let endobj = file.len();
    file.extend_from_slice(b"endobj\n");
    file.extend_from_slice(LATER);
    file.extend_from_slice(b"%%EOF\n");
    let six = ObjectId::new(6, 0);
    for options in [Options::DEFAULT, as_pdfium()] {
        let how = options.recovery;
        let doc = read_as(&file, &options);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert_eq!(extent(&doc, 6), (start, endobj, false), "{how:?}");
        let said = ended(Some(six), start, Some(endobj));
        assert_eq!(stream_ends(&doc), [said], "{how:?}");
        let trailer = doc.trailer.object.at;
        assert_eq!(trailer, Some(Span::new(None, dict, endobj)), "{how:?}");
        assert_eq!(trailer, doc.get(six).unwrap().at, "{how:?}");
        if how == Recovery::Verified {
            assert_eq!(doc.trailer.stream, Some(six));
            assert_eq!(doc.trailer.object.value(), doc.get(six).unwrap().value());
        }
    }
}

/// A scanned file whose `startxref` names its cross-reference stream 6,
/// which has no `endstream` at all, its `/Length` a reference no read
/// follows: its section doesn't read where `startxref` puts it, its data
/// running to the file's end, so the open scans the file, the scan ending
/// 6's data at its `endobj`, and takes 6, read to there, as the trailer.
/// The sections read again ([`Reading::sections`]) read 6 where
/// `startxref` puts it as the open read it, as a section, not as an object
/// the scan listed: it doesn't read there, and the list stops at it.
#[test]
fn a_section_the_scan_ended_at_its_endobj_is_read_again_as_the_open_read_it() {
    let mut file = b"%PDF-1.7\n".to_vec();
    for &(n, text) in &OBJECTS {
        file.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
    }
    let six = file.len();
    file.extend_from_slice(
        b"6 0 obj\n<< /Type /XRef /Size 2 /W [1 1 1] /Root 1 0 R /Length 8 0 R >>\nstream\n",
    );
    // Object 0 free, and object 1 at byte 9.
    file.extend_from_slice(b"\x00\x00\xff\x01\x09\x00\nendobj\n");
    file.extend_from_slice(format!("startxref\n{six}\n%%EOF\n").as_bytes());
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    assert!(reading.trailer().unwritten.is_some());
    assert_eq!(reading.trailer().stream, Some(ObjectId::new(6, 0)));
    let sections = reading.sections().unwrap();
    assert!(sections.sections.is_empty(), "{sections:?}");
    assert!(sections.stopped.is_some());
    reaches_its_page(&read(&file));
}

/// A file read by its section at a shift, whose stream 5, without
/// `endstream`, its `/Length` past its data, is at neither its row's place
/// nor the shift, so a scan finds it, ending its data at its `endobj`;
/// whose stream 6, without `endstream` either, is where its row puts it;
/// and whose stream 8, likewise, is at the shift; the first `endstream`
/// after each 7's. The scan ends 6's and 8's data at their `endobj` too,
/// and says so of each. So each one's own read takes its data to its
/// `endobj`, every way, wherever its entry is read, as the file is being
/// scanned: none runs on to 7's `endstream`.
#[test]
fn each_entry_a_scan_ended_at_its_endobj_is_read_to_there_wherever_it_is() {
    let unended = "<< /Length 999 >>\nstream\nq Q";
    let seven = (7, "<< /Length 3 >>\nstream\nq Q\nendstream");
    let objects = [
        OBJECTS[0],
        OBJECTS[1],
        (5, unended),
        OBJECTS[2],
        OBJECTS[3],
        (6, unended),
        (8, unended),
        seven,
    ];
    let row = |n: u32, at: usize| match n {
        1 | 2 | 8 => at + 5,
        6 | 7 => at,
        _ => at - 4,
    };
    let (file, places, _) = laid_out(b"", &objects, &row, &|at| at + 5);
    let place = |n: u32| places.iter().find(|&&(m, _)| m == n).unwrap().1;
    let head = "5 0 obj\n<< /Length 999 >>\nstream\n".len();
    let doc = read(&file);
    reaches_its_page(&doc);
    let origin = |n: u32| doc.record(ObjectId::new(n, 0)).unwrap().origin;
    assert_eq!(origin(5), Origin::Recovered);
    assert_eq!(origin(6), Origin::File);
    assert_eq!(origin(8), Origin::Shifted { by: -5 });
    let data = |n: u32| place(n) + head;
    for n in [5, 6, 8] {
        assert_eq!(extent(&doc, n), (data(n), data(n) + 4, false), "{n}");
    }
    let id = |n: u32| Some(ObjectId::new(n, 0));
    let said = [5, 6, 8].map(|n| ended(id(n), data(n), Some(data(n) + 4)));
    assert_eq!(stream_ends(&doc), said);
}

/// [`without_endstream`]'s file written whole, its stream 9 read to its
/// `endobj`: written anew with the data it was read with, and read back by
/// its section with nothing to tell, 9's data that.
#[test]
fn a_scanned_stream_ended_at_its_endobj_is_written_with_its_data_to_there() {
    let (file, ..) = without_endstream(b"\n", b"\n");
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    let mut out = Vec::new();
    write::document(
        &mut out,
        &doc,
        None,
        Some(&reading),
        &write::Options::DEFAULT,
    )
    .unwrap();
    let back = read(&out);
    assert_eq!(back.trailer.unwritten, None);
    assert_eq!(back.faults(), []);
    let back = Reading::open(&out[..], &Options::DEFAULT).unwrap();
    let nine = ObjectId::new(9, 0);
    assert_eq!(back.data(nine).unwrap(), &b"q Q\n"[..]);
    for (id, _) in doc.objects() {
        let mut got = back.get(id).unwrap().clone();
        let mut want = reading.get(id).unwrap().clone();
        got.unplace();
        want.unplace();
        if let (Some(got), Some(want)) = (got.as_stream_mut(), want.as_stream_mut()) {
            got.dict.remove(b"Length");
            want.dict.remove(b"Length");
            (got.data, want.data) = Default::default();
        }
        assert_eq!(got, want, "{id}");
        assert_eq!(back.data(id).ok(), reading.data(id).ok(), "{id}");
    }
}

/// [`OBJECTS`] and streams 9 and 10 written whole under AES-256, its
/// password `user`, its `startxref` past its end, so that it's scanned,
/// and 9's `endstream` taken out, so that the scan ends its data at its
/// `endobj` and the first `endstream` after it is 10's: 9's own read takes
/// its data to that `endobj`, its own bytes and the end of line before it
/// alone, and they decrypt cleanly to what was written, every way, as 10's
/// do.
#[test]
fn a_scanned_encrypted_stream_ended_at_its_endobj_decrypts_from_its_own_bytes() {
    use pdf_crypt::Password;
    use pdf_objects::object::{Dict, Stream};

    use crate::write::tests::methods;

    let method = (methods().into_iter())
        .find(|m| m.name == "AES-256 created")
        .unwrap();
    let mut objects: Vec<(u32, Object)> = (OBJECTS.iter())
        .map(|&(n, body)| {
            let object = pdf_objects::parse::Parser::at(body.as_bytes(), 0).value(&mut ());
            (n, object.unwrap())
        })
        .collect();
    objects.push((9, Object::from(Stream::new(Dict::new(), b"q Q".to_vec()))));
    objects.push((10, Object::from(Stream::new(Dict::new(), b"Q q".to_vec()))));
    let bodies = (objects.iter()).map(|(n, o)| (ObjectId::new(*n, 0), Body::from(o)));
    let mut trailer = Dict::new();
    trailer.set("Root", ObjectId::new(1, 0));
    let mut file = Vec::new();
    let options = write::Options::DEFAULT;
    write::full(
        &mut file,
        bodies,
        &trailer,
        Some(&method.encryptor),
        None,
        &options,
    )
    .unwrap();
    let find = |from: usize, word: &[u8]| {
        from + (file[from..].windows(word.len()).position(|w| w == word)).unwrap()
    };
    let nine = find(0, b"9 0 obj");
    let endstream = find(nine, b"endstream");
    let endobj = find(endstream, b"endobj");
    let at = (file.windows(9).rposition(|w| w == b"startxref")).unwrap();
    file.truncate(at);
    file.extend_from_slice(b"startxref\n999999\n%%EOF\n");
    file.drain(endstream..endobj);
    let user = [Password("user")];
    let mut options = Options::DEFAULT;
    options.passwords = &user;
    let doc = read_as(&file, &options);
    assert!(doc.trailer.unwritten.is_some());
    let (_, end, declared) = extent(&doc, 9);
    assert_eq!((end, declared), (endstream, false));
    let reading = Reading::open(&file[..], &options).unwrap();
    for (n, written) in [(9, b"q Q"), (10, b"Q q")] {
        let data = reading.data(ObjectId::new(n, 0)).unwrap();
        assert_eq!(&data[..], written, "{n}");
    }
}

/// The faults `doc` says of the strings a scan found no `)` closing.
fn unclosed(doc: &Document) -> Vec<(Place, Fault)> {
    (doc.faults().iter())
        .filter(|(_, f)| matches!(f, Fault::Unclosed { .. }))
        .cloned()
        .collect()
}

/// The fault a scan says of the strings it found no `)` closing: the
/// first's `(` at byte `start` in object `id`, at its place, and `past` of
/// them read past.
fn read_past(id: Option<ObjectId>, start: usize, past: u32) -> (Place, Fault) {
    let fault = Fault::Unclosed { id, start, past };
    (
        Place::new(None, Some(Span::new(None, start, start + 1))),
        fault,
    )
}

/// Object 9.
const NINE: Option<ObjectId> = Some(ObjectId::new(9, 0));

/// Scanned files whose string that doesn't close comes before the catalog,
/// its page and the trailer: as object 9's dictionary's value, between
/// objects, and where object 9's dictionary has a key. The verified
/// recovery reads on from just past its `(`, so each object after it is
/// found, every way, and says so once, at the `(`; under
/// [`Recovery::Pdfium`] the scan reads the rest of the file as that string,
/// as PDFium's does where it reads a string (a value, or a `(` between
/// objects), so nothing after it is found, and says that. PDFium reads past
/// a `(` where a key goes, which a scan of tokens doesn't tell from a value.
/// Object 9's own read of its string doesn't read.
#[test]
fn a_string_that_doesnt_close_is_read_past_under_the_verified_recovery() {
    let shapes = [
        ("9 0 obj\n<< /Title (open /Producer (x) >>\nendobj\n", NINE),
        ("9 0 obj\n<< /Title (closed) >>\nendobj\n(open\n", None),
        (
            "9 0 obj\n<< /Title (closed) ( /Producer (x) >>\nendobj\n",
            NINE,
        ),
    ];
    for (body, id) in shapes {
        let (file, places) = scanned_after(body.as_bytes(), b"");
        let start = b"%PDF-1.7\n".len() + body.find("(open").or(body.find("( ")).unwrap();
        let doc = read(&file);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        reaches_its_page(&doc);
        assert_eq!(unclosed(&doc), [read_past(id, start, 1)], "{body}");
        assert_eq!(doc.get(ObjectId::new(9, 0)).is_ok(), id.is_none(), "{body}");
        let doc = read_as(&file, &as_pdfium());
        assert!(doc.get(ObjectId::new(1, 0)).is_err(), "{body}");
        assert!(doc.trailer.dict().get(b"Root").is_none(), "{body}");
        assert_eq!(unclosed(&doc), [read_past(id, start, 0)], "pdfium: {body}");
    }
}

/// Scanned files of stream 9, with neither `endstream` nor `endobj` after
/// its data, a string that doesn't close in its data, first or after an
/// operator, then the page without `endobj` either, after the catalog, its
/// page tree and the trailer: the scan finds no end to the data and reads
/// on through it, as PDFium's does, and the verified recovery reads on from
/// just past the string's `(`, so the page is found, every way, and says
/// both; under [`Recovery::Pdfium`] the scan reads the rest of the file as
/// that string, so the page isn't found, and says both. The string is no
/// object's, as the scan reads its data. Its `(` first, the token right
/// after the data the scan passed over to its start, it's read past as one
/// after any other token is.
#[test]
fn a_string_that_doesnt_close_in_data_read_on_through_is_read_past() {
    let head = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\ntrailer\n<< /Size 10 /Root 1 0 R >>\n9 0 obj\n<< /Length 999 >>\nstream\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << >> >>\n";
    let no_end = ended(NINE, head.len(), None);
    for data in [&b"(open\n"[..], b"q Q (open\n"] {
        let file = [&head[..], data, page].concat();
        let start = head.len() + data.iter().position(|&b| b == b'(').unwrap();
        let doc = read(&file);
        reaches_its_page(&doc);
        let at = doc.record(ObjectId::new(3, 0)).unwrap().at.start;
        assert_eq!(at, head.len() + data.len());
        assert_eq!(stream_ends(&doc), std::slice::from_ref(&no_end));
        assert_eq!(unclosed(&doc), [read_past(None, start, 1)]);
        let doc = read_as(&file, &as_pdfium());
        assert!(doc.get(ObjectId::new(3, 0)).is_err());
        assert_eq!(stream_ends(&doc), std::slice::from_ref(&no_end));
        assert_eq!(unclosed(&doc), [read_past(None, start, 0)]);
    }
}

/// A scanned file whose first stream has no `/Length` that reads and an
/// `endobj` of its own in its data, after which the data opens a string it
/// doesn't close, then `endstream` and the catalog, its page and the
/// trailer: the verified scan ends the data at that `endobj`, reads on from
/// just past the string's `(` and meets `endstream` on its own, so the data
/// ran there: what it found inside, that string among it, is taken back,
/// and every object and the trailer are found, every way, nothing said of
/// the scan. Under [`Recovery::Pdfium`] the scan ends the data at that `endobj` and
/// reads the rest of the file as the string, so the catalog isn't found,
/// and says both.
#[test]
fn a_string_that_doesnt_close_in_data_the_take_back_drops_is_said_nowhere() {
    let stream = b"9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n(open\nendstream\nendobj\n";
    let (file, places) = scanned_after(stream, b"");
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert_eq!(stream_ends(&doc), []);
    assert_eq!(unclosed(&doc), []);
    let doc = read_as(&file, &as_pdfium());
    assert!(doc.get(ObjectId::new(1, 0)).is_err());
    let start = b"%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\n".len();
    let end = start + "q Q\n".len();
    assert_eq!(stream_ends(&doc), [ended(NINE, start, Some(end))]);
    assert_eq!(unclosed(&doc), [read_past(None, end + "endobj\n".len(), 0)]);
}

/// A scanned file whose first stream has no `/Length` that reads and an
/// `endobj` of its own in its data, then a comment whose line holds the
/// data's `endstream`, a string that doesn't close, and the catalog, its
/// page and the trailer: the verified scan ends the data at that `endobj`
/// and finds nothing on the comment's line, so its second scan, to each
/// stream's first `endstream`, reads on from just past the string's `(`
/// and gives it every object and the trailer, every way; it says where it
/// ended the data, and nothing of the string, which the first scan didn't
/// meet.
#[test]
fn a_string_that_doesnt_close_met_by_the_second_scan_alone_is_read_past() {
    let mut file =
        b"%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n%\x80 endstream endobj (open"
            .to_vec();
    let mut places = Vec::new();
    for &(n, text) in &OBJECTS {
        file.push(b' ');
        places.push((n, file.len()));
        file.extend_from_slice(format!("{n} 0 obj {text} endobj").as_bytes());
    }
    file.extend_from_slice(b" trailer << /Size 10 /Root 1 0 R >>\n");
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    let start = b"%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\n".len();
    let said = ended(NINE, start, Some(start + "q Q\n".len()));
    assert_eq!(stream_ends(&doc), std::slice::from_ref(&said));
    assert_eq!(unclosed(&doc), []);
    let doc = read_as(&file, &as_pdfium());
    assert!(doc.get(ObjectId::new(1, 0)).is_err());
    assert_eq!(stream_ends(&doc), [said]);
    assert_eq!(unclosed(&doc), []);
}

/// A scanned file with three strings that don't close before the catalog,
/// its page and the trailer: the verified recovery reads on past each, and
/// says so once, at the first, with their count; under
/// [`Recovery::Pdfium`] it reads the rest of the file as the first, and
/// says so once.
#[test]
fn strings_that_dont_close_are_said_once_a_scan_with_their_count() {
    let body = b"(a\n(b (c\n";
    let (file, places) = scanned_after(body, b"");
    let start = b"%PDF-1.7\n".len();
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert_eq!(doc.faults(), [read_past(None, start, 3)]);
    let doc = read_as(&file, &as_pdfium());
    assert_eq!(doc.objects().count(), 0);
    assert_eq!(doc.faults(), [read_past(None, start, 0)]);
}

/// A scanned file where `5 0` and then a string that doesn't close come
/// before `obj`: the scan reads on from just past the `(`, which stands
/// between the numbers and `obj`, so no object 5 is listed; and where they
/// come with a stream's data between, `5 0` its last bytes, not either.
#[test]
fn a_string_read_past_stands_between_an_objects_numbers_and_obj() {
    for body in [&b"5 0 (\nobj\n"[..], b"5 0 (obj\n"] {
        let (file, places) = scanned_after(body, b"");
        let doc = read(&file);
        each_object_at(&doc, &places, &|_| Origin::Recovered);
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        assert!(!reading.ids().any(|id| id.number == 5), "{body:?}");
        assert_eq!(doc.unread().count(), 0);
        assert_eq!(unclosed(&doc), [read_past(None, 9 + 4, 1)]);
    }
}

/// A file with three bytes cut out of object 3's `n g obj`, a string that
/// doesn't close after object 4, and an update that writes object 3 again
/// with another after it: read whole, the scan that looks for object 3 says
/// both; read as of the section the update follows, only the first, before
/// that revision's end, as it takes nothing later updates wrote.
#[test]
fn a_reading_as_of_a_section_says_only_the_strings_before_its_end() {
    let objects = [OBJECTS[0], OBJECTS[1], OBJECTS[2], (4, "(four)\nendobj\n(")];
    let (mut file, places, table) = laid_out(b"", &objects, &|_, at| at, &|at| at);
    let first = places[3].1 + "4 0 obj\n(four)\nendobj\n".len() - 3;
    let three = file.windows(7).position(|w| w == b"3 0 obj").unwrap();
    file.drain(three..three + 3);
    let end = file.len();
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 792] /Resources << >> >>";
    file.extend_from_slice(format!("3 0 obj\n{page}\nendobj\n(\n").as_bytes());
    let update = file.len();
    file.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n3 1\n{end:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R /Prev {table} >>\nstartxref\n{update}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let whole = read(&file);
    assert_eq!(unclosed(&whole), [read_past(None, first, 2)]);
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let previous = reading.update().unwrap().previous;
    let mut options = Options::DEFAULT;
    options.section = Some(previous);
    let doc = read_as(&file, &options);
    assert_eq!(unclosed(&doc), [read_past(None, first, 1)]);
    assert!(doc.get(ObjectId::new(3, 0)).is_err());
}

/// A file with three bytes cut out of object 3's `n g obj`, object 4's
/// string holding object 5's `n g obj` at a line's start, a row for 5 that
/// lands nowhere, and an update that writes object 3 again and object 6,
/// whose string holds object 5's again: read whole, the scan that looks
/// for objects 3 and 5 finds 5 in both strings, the later standing, so 5
/// is read there and only 6's string is said; read as of the section the
/// update follows, 5 is read in 4's, before that revision's end, as it
/// takes nothing later updates wrote, and that string is said.
#[test]
fn a_reading_as_of_a_section_says_the_objects_strings_give_before_its_end() {
    let objects = [
        OBJECTS[0],
        OBJECTS[1],
        OBJECTS[2],
        (4, "(four\n5 0 obj\n(five)\nendobj\n)"),
    ];
    let (mut file, places, table) = laid_out(b"", &objects, &|_, at| at, &|at| at);
    let four = places[3].1 + "4 0 obj\n".len() - 3;
    let three = file.windows(7).position(|w| w == b"3 0 obj").unwrap();
    file.drain(three..three + 3);
    // Object 5's row, at byte 1, after the table's five, now 3 bytes
    // before where they were written.
    let cut = table - 3;
    let rows = cut + "xref\n0 5\n".len() + 5 * 20;
    file.splice(rows..rows, b"0000000001 00000 n \n".iter().copied());
    let size = |file: &mut Vec<u8>, from: &[u8], to: &[u8]| {
        let at = cut
            + file[cut..]
                .windows(from.len())
                .position(|w| w == from)
                .unwrap();
        file[at..at + from.len()].copy_from_slice(to);
    };
    size(&mut file, b"0 5\n", b"0 6\n");
    size(&mut file, b"/Size 5", b"/Size 6");
    let end = file.len();
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 792] /Resources << >> >>";
    let six = end + format!("3 0 obj\n{page}\nendobj\n6 0 obj\n").len();
    let string = "(six\n5 0 obj\n(five)\nendobj\n)";
    file.extend_from_slice(
        format!("3 0 obj\n{page}\nendobj\n6 0 obj\n{string}\nendobj\n").as_bytes(),
    );
    let update = file.len();
    file.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n3 1\n{end:010} 00000 n \ntrailer\n<< /Size 7 /Root 1 0 R /Prev {table} >>\nstartxref\n{update}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let whole = read(&file);
    let id = |n| Some(ObjectId::new(n, 0));
    let said = read_in_string(id(6), six, six + string.len() - 1, 1);
    assert_eq!(in_string(&whole), [said]);
    let five = |doc: &Document| (doc.record(ObjectId::new(5, 0))).map(|r| (r.origin, r.at.start));
    assert_eq!(
        five(&whole),
        Some((Origin::Recovered, six + "(six\n".len()))
    );
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let previous = reading.update().unwrap().previous;
    let mut options = Options::DEFAULT;
    options.section = Some(previous);
    let doc = read_as(&file, &options);
    let body = objects[3].1;
    let said = read_in_string(id(4), four, four + body.len() - 1, 1);
    assert_eq!(in_string(&doc), [said]);
    assert_eq!(
        five(&doc),
        Some((Origin::Recovered, four + "(four\n".len()))
    );
}

/// A file read by its table at a shift, every row 3 bytes past its object
/// but object 3's, which lands nowhere, so a scan finds 3; object 4's
/// `/Subject` holding a line `7 0 obj` inside it: with no row for 7, no
/// entry is moved to it, so 7 isn't read and the string isn't said; with
/// a row for 7 that lands nowhere, 7's entry is moved there, and the string
/// is said with it.
#[test]
fn a_string_whose_object_no_entry_takes_isnt_said_where_the_sections_read() {
    let subject = "<< /Subject (a PDF object:\n7 0 obj\n<< /A 1 >>\nendobj\n) >>";
    let objects = [OBJECTS[0], OBJECTS[1], OBJECTS[2], (4, subject)];
    for listed in [false, true] {
        let (mut file, places, _) = laid_out(
            b"",
            &objects,
            &|n, at| match n {
                3 => 1,
                _ => at + 3,
            },
            &|at| at,
        );
        let string = places[3].1 + "4 0 obj\n<< /Subject ".len();
        let seven = string + "(a PDF object:\n".len();
        if listed {
            let table = file.windows(5).position(|w| w == b"xref\n").unwrap();
            let rows = table + "xref\n0 5\n".len() + 5 * 20;
            file.splice(
                rows..rows,
                b"0000000000 00000 f \n0000000000 00000 f \n0000000001 00000 n \n"
                    .iter()
                    .copied(),
            );
            let count = table + "xref\n".len();
            file[count..count + 3].copy_from_slice(b"0 8");
            let at = file.windows(7).position(|w| w == b"/Size 5").unwrap();
            file[at..at + 7].copy_from_slice(b"/Size 8");
        }
        let doc = read(&file);
        reaches_its_page(&doc);
        let id = ObjectId::new(7, 0);
        let end = string + "(a PDF object:\n7 0 obj\n<< /A 1 >>\nendobj\n".len();
        match listed {
            false => {
                assert_eq!(in_string(&doc), [], "{listed}");
                assert!(doc.get(id).is_err());
            }
            true => {
                let said = read_in_string(Some(ObjectId::new(4, 0)), string, end, 1);
                assert_eq!(in_string(&doc), [said]);
                let record = doc.record(id).unwrap();
                assert_eq!((record.origin, record.at.start), (Origin::Recovered, seven));
            }
        }
    }
}

/// A file in the shape of one a writer damaged: object 46's `/Creator`
/// closed twice, so its `/Producer`, which doesn't close its own, nests the
/// date after it and closes at a `)` in the raw data of stream 40 after it,
/// the scan reading `40 0 obj` and its `stream` as that string's; then a
/// `(` in that data that doesn't close. The verified recovery reads on from
/// just past it, so the catalog, its page and the trailer after the stream
/// are found, every way, and says so once, in object 46, where the scan
/// last found an `n g obj`; and finds stream 40, whose `n g obj` starts a
/// line inside that string, there, and says so; under
/// [`Recovery::Pdfium`] nothing after it is, nor 40.
#[test]
fn objects_after_a_string_closed_late_into_data_that_doesnt_close_are_found() {
    let info = b"46 0 obj\n<< /Creator (MIX)) /Producer (BU\n/ModDate (D:1)\n>>\nendobj\n";
    let data = b"\x9c\x01)\x02\x03(\x04\x05";
    let mut body = info.to_vec();
    body.extend_from_slice(format!("40 0 obj\n<< /Length {} >>\nstream\n", data.len()).as_bytes());
    let opened = body.len() + 5;
    let closed = body.len() + 2;
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    let (file, places) = scanned_after(&body, b"");
    let start = b"%PDF-1.7\n".len() + opened;
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    let forty_six = Some(ObjectId::new(46, 0));
    assert_eq!(unclosed(&doc), [read_past(forty_six, start, 1)]);
    let forty = ObjectId::new(40, 0);
    let header = b"%PDF-1.7\n".len();
    assert_eq!(doc.record(forty).unwrap().at.start, header + info.len());
    assert!(doc.get(forty).is_ok());
    let producer = header + info.windows(3).position(|w| w == b"(BU").unwrap();
    let said = read_in_string(forty_six, producer, header + closed, 1);
    assert_eq!(in_string(&doc), [said]);
    let doc = read_as(&file, &as_pdfium());
    assert!(doc.get(ObjectId::new(1, 0)).is_err());
    assert!(doc.get(forty).is_err());
    assert_eq!(unclosed(&doc), [read_past(forty_six, start, 0)]);
    assert_eq!(in_string(&doc), []);
}

/// Scanned files whose bytes a scan reads past strings that don't close
/// would read again and again, were each string read: runs of `(` closed
/// by a `)` after each third, so that some close and some don't; `(` again
/// and again, then one `)`; streams each with a string that doesn't close
/// in its dictionary, `trailer` dictionaries each with one, and an object
/// written again and again each with one, which the open reads past the
/// first to see which stands; a string that closes, its lines each a `(`,
/// and one whose lines start objects, each a dictionary opening a string
/// that closes only at its end; each with a trailer that reads at the end;
/// and 64 KiB of `(`. Under either recovery the scan reads no more than
/// three times the file's bytes as strings that don't close, in its own
/// parses of each stream's head and each trailer, its looks at each line
/// inside a string and each object it finds there, and in those reads,
/// and finds the catalog first and the trailer last.
#[test]
fn a_scan_reads_past_strings_that_dont_close_in_a_bounded_number_of_bytes() {
    let catalog = "%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n";
    let trailer = "trailer\n<< /Size 2 /Root 1 0 R >>\n";
    let n = 512;
    let mut streams = String::new();
    for k in 0..n {
        let k = k + 2;
        streams.push_str(&format!(
            "{k} 0 obj\n<< /A ( /Length 3 >>\nstream\nabc\nendstream\nendobj\n"
        ));
    }
    let shapes = [
        ("((()".repeat(2048), "runs"),
        (format!("{})", "(".repeat(8192)), "one close"),
        (streams, "streams"),
        ("trailer\n<< /A (\n".repeat(n), "trailers"),
        ("5 0 obj\n<< /A (\nendobj\n".repeat(n), "copies"),
        (
            format!("(lines{}{}", "\n(".repeat(4 * n), ")".repeat(4 * n + 1)),
            "lines",
        ),
        (
            format!(
                "(objects{}{}",
                (0..n)
                    .map(|k| format!("\n{} 0 obj\n<< /A (", k + 10))
                    .collect::<String>(),
                ")".repeat(n + 1)
            ),
            "objects",
        ),
        ("(".repeat(64 << 10), "64 KiB"),
    ];
    for (middle, name) in shapes {
        let file = format!("{catalog}{middle}\n{trailer}").into_bytes();
        for recovery in [Recovery::Verified, Recovery::Pdfium] {
            let options = Options {
                recovery,
                ..Options::DEFAULT
            };
            SCAN_READ.with(|read| read.set(0));
            let reading = Reading::open(&file[..], &options).unwrap();
            let read = SCAN_READ.with(|read| read.get());
            assert!(
                read <= 3 * file.len(),
                "{name}, {recovery:?}: {read} of {}",
                file.len()
            );
            assert!(
                reading.ids().any(|id| id.number == 1),
                "{name}, {recovery:?}"
            );
            if recovery == Recovery::Verified {
                let root = reading.trailer().dict().get(b"Root").map(Object::value);
                assert_eq!(root, Some(&Value::Ref(ObjectId::new(1, 0))), "{name}");
            }
        }
    }
}

/// A scanned file with no `trailer`, its objects each a dictionary opening
/// a string that doesn't close, with a cross-reference stream before them
/// and without: the open's search for a cross-reference stream from the
/// file's end looks at each object's dictionary over its own bytes alone,
/// reading only the cross-reference stream whole, so the open reads no
/// more than four times the file's bytes, where reading each object whole
/// would read it about as many times as there are objects. With the
/// stream, it's the trailer, naming the catalog; without, there's none.
#[test]
fn a_scanned_files_search_for_a_cross_reference_stream_reads_each_object_over_its_bytes() {
    let n = 512;
    let catalog = "%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n";
    let xref = "2 0 obj\n<< /Type /XRef /Size 3 /W [1 1 1] /Root 1 0 R /Length 3 >>\nstream\n\x00\x00\x01\nendstream\nendobj\n";
    let objects: String = (0..n)
        .map(|k| format!("{} 0 obj\n<< /A ( >>\nendobj\n", k + 3))
        .collect();
    for with in [true, false] {
        let stream = if with { xref } else { "" };
        let file = format!("{catalog}{stream}{objects}").into_bytes();
        SCAN_READ.with(|read| read.set(0));
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let read = SCAN_READ.with(|read| read.get());
        assert!(read <= 4 * file.len(), "{with}: {read} of {}", file.len());
        assert_eq!(reading.ids().count(), n + 1 + usize::from(with), "{with}");
        let root = reading.trailer().dict().get(b"Root").map(Object::value);
        match with {
            true => {
                assert_eq!(reading.trailer().stream, Some(ObjectId::new(2, 0)));
                assert_eq!(root, Some(&Value::Ref(ObjectId::new(1, 0))));
            }
            false => {
                assert_eq!(reading.trailer().stream, None);
                assert_eq!(root, None);
            }
        }
    }
}

/// Where a scan reads past strings that don't close, its own parses of an
/// object's head for its `/Length` and of each `trailer`'s dictionary are
/// read over the bytes up to the `stream` keyword and up to the next string
/// it read past: a dictionary whose `/Length` comes after a string that
/// closes reads it, so the stream's data is passed by it, and a trailer
/// before a string that doesn't close reads, so the file's catalog is
/// reached, as over the whole file.
#[test]
fn a_scans_own_parses_read_over_their_bytes_as_over_the_whole_file() {
    let data = "q Q\nendstream\n";
    let stream = format!(
        "9 0 obj\n<< /Title (a (b) c) /Length {} >>\nstream\n{data}\nendstream\nendobj\n(open\n",
        data.len()
    );
    let (file, places) = scanned_after(stream.as_bytes(), b"");
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    let nine = doc.record(ObjectId::new(9, 0)).unwrap().data.unwrap();
    assert!(nine.declared);
    assert_eq!(nine.at.end - nine.at.start, data.len());
    let (file, places) = scanned_after(b"", b"(open\n");
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert!(doc.trailer.dict().has(b"Root"));
}

/// The faults `doc` says of objects a scan found again past a string read
/// past, which don't read there.
fn earlier(doc: &Document) -> Vec<(Place, Fault)> {
    (doc.faults().iter())
        .filter(|(_, f)| matches!(f, Fault::Earlier { .. }))
        .cloned()
        .collect()
}

/// The fault a scan says of object `n`, read at byte `at`, where it found
/// it before it found it again at byte `later`, which doesn't read, at its
/// place.
fn read_earlier(n: u32, at: usize, later: usize) -> (Place, Fault) {
    let fault = Fault::Earlier {
        id: ObjectId::new(n, 0),
        at,
        later,
    };
    (
        Place::new(None, Some(Span::new(None, later, later + 1))),
        fault,
    )
}

/// Page 3, its dictionary cut short.
const CUT_PAGE: &str = "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20]\n";

/// Page 3 whole, 20 wide.
const WIDE_PAGE: &str =
    "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Resources << >> >>\nendobj\n";

/// The width page 3 has in `doc`.
fn width(doc: &Document) -> i64 {
    let page = doc.get(ObjectId::new(3, 0)).unwrap().as_dict().unwrap();
    let media = page.get(b"MediaBox").unwrap().as_array().unwrap();
    media[2].as_integer().unwrap()
}

/// Scanned files whose catalog, page tree and page 3 come before a string
/// that doesn't close, after which the scan finds page 3 again: a revision
/// cut short, its dictionary unclosed, which doesn't read. The verified
/// recovery reads on past the string, and page 3 found there would stand
/// over the one before it, as a later object of a number does; but one
/// found past a string read past stands only where it reads, so page 3 is
/// read where it was found first, as the scan read the file before it read
/// past strings, every way, and that's said. Where a whole page 3 comes
/// between, past the string too, that one's read. Where object 5 doesn't
/// read before the string either, the one before still stands, as the scan
/// read it before, and that's said. Under [`Recovery::Pdfium`] the scan
/// reads no further than the string, and page 3 is read where it was found
/// first, as PDFium reads it.
#[test]
fn an_object_found_again_past_a_string_read_past_stands_only_where_it_reads() {
    let (file, places) = scanned_after(b"", format!("(open\n{CUT_PAGE}").as_bytes());
    let later = file.len() - CUT_PAGE.len();
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert_eq!(width(&doc), 10);
    assert_eq!(earlier(&doc), [read_earlier(3, places[2].1, later)]);
    assert_eq!(unclosed(&doc).len(), 1);
    let doc = read_as(&file, &as_pdfium());
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    assert_eq!(earlier(&doc), []);
    let between = format!("(open\n{WIDE_PAGE}{CUT_PAGE}");
    let (file, places) = scanned_after(b"", between.as_bytes());
    let wide = file.len() - CUT_PAGE.len() - WIDE_PAGE.len();
    let doc = read(&file);
    reaches_its_page(&doc);
    assert_eq!(width(&doc), 20);
    assert_eq!(doc.record(ObjectId::new(3, 0)).unwrap().at.start, wide);
    let later = file.len() - CUT_PAGE.len();
    assert_eq!(earlier(&doc), [read_earlier(3, wide, later)]);
    assert_eq!(
        doc.record(ObjectId::new(1, 0)).unwrap().at.start,
        places[0].1
    );
    let five = b"5 0 obj\n<< /A 1\nendobj\n";
    let (file, _) = scanned_after(five, b"(open\n5 0 obj\n<< /B 2\n");
    let later = file.len() - "5 0 obj\n<< /B 2\n".len();
    let doc = read(&file);
    reaches_its_page(&doc);
    assert!(doc.get(ObjectId::new(5, 0)).is_err());
    assert_eq!(earlier(&doc), [read_earlier(5, 9, later)]);
}

/// A later page 3 that reads, past a string read past, stands over the one
/// before the string, as a later object of a number does, and nothing's
/// said of it; and where no string is read past, page 3 cut short stands
/// over the one before it, as a scan reads an update, and doesn't read.
/// So the rule reaches only what's found past such a string.
#[test]
fn an_object_found_again_stands_as_the_last_where_it_reads_or_no_string_is_read_past() {
    let (file, _) = scanned_after(b"", format!("(open\n{WIDE_PAGE}").as_bytes());
    let doc = read(&file);
    reaches_its_page(&doc);
    assert_eq!(width(&doc), 20);
    assert_eq!(earlier(&doc), []);
    let (file, _) = scanned_after(b"", CUT_PAGE.as_bytes());
    let doc = read(&file);
    assert!(doc.get(ObjectId::new(3, 0)).is_err());
    assert_eq!(earlier(&doc), []);
}

/// [`a_string_that_doesnt_close_met_by_the_second_scan_alone_is_read_past`]'s
/// file, with page 3 before the string too, and the one past it cut short:
/// the second scan alone finds page 3, before the string it reads past and
/// again past it, where it doesn't read; page 3 is read where that scan
/// found it first, every way, and that's said.
#[test]
fn a_number_the_second_scan_finds_again_past_a_string_stands_only_where_it_reads() {
    let mut file =
        b"%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n%\x80 endstream endobj "
            .to_vec();
    let first = file.len();
    file.extend_from_slice(format!("3 0 obj {} endobj (open", OBJECTS[2].1).as_bytes());
    let mut later = 0;
    for &(n, text) in &OBJECTS {
        file.push(b' ');
        match n {
            3 => {
                later = file.len();
                file.extend_from_slice(b"3 0 obj << /Type /Page /Parent 2 0 R");
            }
            _ => file.extend_from_slice(format!("{n} 0 obj {text} endobj").as_bytes()),
        }
    }
    file.extend_from_slice(b" trailer << /Size 10 /Root 1 0 R >>\n");
    let doc = read(&file);
    reaches_its_page(&doc);
    assert_eq!(doc.record(ObjectId::new(3, 0)).unwrap().at.start, first);
    assert_eq!(earlier(&doc), [read_earlier(3, first, later)]);
    assert_eq!(unclosed(&doc), []);
}

/// [`two_shifts`], with a string that doesn't close after it and page 3
/// again, cut short: the section's entries for objects 3 and 4 are at
/// neither place, so a scan of the file finds them, reads on past the
/// string and finds page 3 again, which doesn't read; page 3 is read where
/// the scan found it first, every way, and that's said.
#[test]
fn an_entry_a_scan_finds_again_past_a_string_read_past_is_read_where_it_reads() {
    let (mut file, places, _) = two_shifts();
    let later = file.len() + "(open\n".len();
    file.extend_from_slice(format!("(open\n{CUT_PAGE}").as_bytes());
    let doc = read(&file);
    let origin = |n| match n {
        1 | 2 => Origin::Shifted { by: -5 },
        _ => Origin::Recovered,
    };
    each_object_at(&doc, &places, &origin);
    reaches_its_page(&doc);
    assert_eq!(earlier(&doc), [read_earlier(3, places[2].1, later)]);
}

/// Scanned files whose stream 4, its `/Length` past its data, has its
/// `endobj` before its first `endstream`, a `(` that doesn't close between
/// them; then page 3 again, past that `endstream`, cut short or whole. The
/// scan ends 4's data at its `endobj` and reads on past the string, then
/// meets that `endstream` between objects and takes back what it found
/// inside: the string, said nowhere, and the stream's end, so 4's data
/// runs to the `endstream`, the string's `(` among it. Page 3 found past
/// there is past the string all the same, which a scan that read no
/// further than it doesn't reach. So it stands only where it reads: cut
/// short, the page found before is read, and that's said, though no string
/// is; whole, it's read, as a later copy is, and nothing is said.
#[test]
fn an_object_found_again_past_data_taken_back_stands_only_where_it_reads() {
    let four = "4 0 obj\n<< /Length 99 >>\nstream\nq Q\nendobj\n(open\nendstream\n";
    for (copy, whole) in [(CUT_PAGE, false), (WIDE_PAGE, true)] {
        let mut file = b"%PDF-1.7\n".to_vec();
        let mut places = Vec::new();
        for &(n, text) in &OBJECTS[..3] {
            places.push((n, file.len()));
            file.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
        }
        let data = file.len() + "4 0 obj\n<< /Length 99 >>\nstream\n".len();
        file.extend_from_slice(four.as_bytes());
        let endstream = file.len() - "endstream\n".len();
        let later = file.len();
        file.extend_from_slice(copy.as_bytes());
        file.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\n%%EOF\n");
        let doc = read(&file);
        assert_eq!(unclosed(&doc), [], "{whole}");
        assert_eq!(stream_ends(&doc), [], "{whole}");
        assert_eq!(extent(&doc, 4), (data, endstream, false), "{whole}");
        let three = doc.record(ObjectId::new(3, 0)).unwrap().at.start;
        match whole {
            false => {
                reaches_its_page(&doc);
                assert_eq!(three, places[2].1);
                assert_eq!(earlier(&doc), [read_earlier(3, places[2].1, later)]);
            }
            true => {
                assert_eq!((three, width(&doc)), (later, 20));
                assert_eq!(earlier(&doc), []);
            }
        }
    }
}

/// The faults `doc` says of strings whose lines inside them gave a scan
/// objects.
fn in_string(doc: &Document) -> Vec<(Place, Fault)> {
    (doc.faults().iter())
        .filter(|(_, f)| matches!(f, Fault::InString { .. }))
        .cloned()
        .collect()
}

/// The fault a scan says of the string from its `(` at byte `start` to the
/// `)` at byte `end` that closes it, in object `id`, whose lines inside it
/// gave it `objects` objects, at its place.
fn read_in_string(id: Option<ObjectId>, start: usize, end: usize, objects: u32) -> (Place, Fault) {
    let fault = Fault::InString {
        id,
        start,
        end,
        objects,
    };
    (
        Place::new(None, Some(Span::new(None, start, end + 1))),
        fault,
    )
}

/// Object 6.
const SIX: Option<ObjectId> = Some(ObjectId::new(6, 0));

/// A scanned file of `before`, then object 6, whose `/Title` lacks its
/// `)`, so that its string reads on over `inside`, and object 5 after it,
/// and closes late, at the second `)` of 5's `/Author`, closed twice; then
/// `after`, the catalog, its page tree, object 4 and the trailer. The
/// file, and where 6's string's `(` is and the `)` that closes it.
fn closed_late(before: &str, inside: &str, after: &str) -> (Vec<u8>, usize, usize) {
    let mut file = format!("%PDF-1.7\n{before}6 0 obj\n<< /Title (six >>\nendobj\n").into_bytes();
    let start = file.len() - "(six >>\nendobj\n".len();
    file.extend_from_slice(inside.as_bytes());
    file.extend_from_slice(b"5 0 obj\n<< /Author (five)) >>\nendobj\n");
    let end = file.len() - " >>\nendobj\n".len() - 1;
    file.extend_from_slice(after.as_bytes());
    for &(n, text) in &[OBJECTS[0], OBJECTS[1], OBJECTS[3]] {
        file.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
    }
    file.extend_from_slice(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n");
    (file, start, end)
}

/// Page 3 as [`OBJECTS`] has it, 10 wide.
fn page() -> String {
    format!("3 0 obj\n{}\nendobj\n", OBJECTS[2].1)
}

/// [`closed_late`], page 3 inside the string closed late, and nowhere
/// else: the verified recovery's scan finds page 3's `n g obj` where it
/// starts a line inside the string, and object 5's, and reads them there,
/// every way, saying so once, so the page is reached, and 5, closed twice,
/// doesn't read; under
/// [`Recovery::Pdfium`] neither is found, as PDFium reads the string as a
/// dictionary's value, and nothing's said.
#[test]
fn an_object_starting_a_line_inside_a_string_closed_late_is_read_there() {
    let (file, start, end) = closed_late("", &page(), "");
    let three = start + "(six >>\nendobj\n".len();
    let doc = read(&file);
    reaches_its_page(&doc);
    let record = doc.record(ObjectId::new(3, 0)).unwrap();
    assert_eq!((record.at.start, record.origin), (three, Origin::Recovered));
    assert!(record.endobj);
    // Object 5, `)` where its dictionary's key goes, doesn't read there.
    let unread: Vec<ObjectId> = doc.unread().map(|(id, _)| id).collect();
    assert_eq!(unread, [ObjectId::new(5, 0)]);
    assert_eq!(in_string(&doc), [read_in_string(SIX, start, end, 2)]);
    assert_eq!(unclosed(&doc), []);
    let doc = read_as(&file, &as_pdfium());
    assert!(doc.get(ObjectId::new(1, 0)).is_ok());
    assert!(doc.get(ObjectId::new(3, 0)).is_err());
    assert!(doc.get(ObjectId::new(5, 0)).is_err());
    assert_eq!(in_string(&doc), []);
}

/// [`closed_late`], page 3, 20 wide, inside the string, and 10 wide before
/// it or after it: page 3 as the scan found it as written stands, every
/// way, and only object 5 is read from inside the string, and said.
#[test]
fn an_object_inside_a_string_closed_late_of_a_number_found_elsewhere_isnt_taken() {
    for (before, after) in [(page(), String::new()), (String::new(), page())] {
        let (file, start, end) = closed_late(&before, WIDE_PAGE, &after);
        let doc = read(&file);
        reaches_its_page(&doc);
        assert_eq!(width(&doc), 10);
        let at = file.windows(7).rposition(|w| w == b"3 0 obj").unwrap();
        let at = match before.is_empty() {
            true => at,
            false => file.windows(7).position(|w| w == b"3 0 obj").unwrap(),
        };
        assert_eq!(doc.record(ObjectId::new(3, 0)).unwrap().at.start, at);
        assert_eq!(in_string(&doc), [read_in_string(SIX, start, end, 1)]);
    }
}

/// [`closed_late`], page 3 inside the string twice, 20 wide then 10 wide:
/// the later stands, every way, as a later `n g obj` of a number does, and
/// the string says the objects that stand there, the page and object 5;
/// and with a string closed late before it, in object 8, holding page 3,
/// 20 wide: the later string's page 3 stands over that one too, so 8's
/// string isn't said where the page is all it gave, and says object 7
/// alone where 7 is written there after the page.
#[test]
fn an_object_written_twice_inside_strings_closed_late_is_said_where_it_stands() {
    let twice = format!("{WIDE_PAGE}{}", page());
    let (file, start, end) = closed_late("", &twice, "");
    let doc = read(&file);
    reaches_its_page(&doc);
    assert_eq!(width(&doc), 10);
    let at = file.windows(7).rposition(|w| w == b"3 0 obj").unwrap();
    assert_eq!(doc.record(ObjectId::new(3, 0)).unwrap().at.start, at);
    assert_eq!(in_string(&doc), [read_in_string(SIX, start, end, 2)]);

    for seven in ["", "7 0 obj\n(seven)\nendobj\n"] {
        let eight = format!("8 0 obj\n<< /Title (eight >>\nendobj\n{WIDE_PAGE}{seven})\n");
        let (file, start, end) = closed_late(&eight, &page(), "");
        let doc = read(&file);
        reaches_its_page(&doc);
        assert_eq!(width(&doc), 10);
        let at = file.windows(7).rposition(|w| w == b"3 0 obj").unwrap();
        assert_eq!(doc.record(ObjectId::new(3, 0)).unwrap().at.start, at);
        let six = read_in_string(SIX, start, end, 2);
        match seven.is_empty() {
            true => assert_eq!(in_string(&doc), [six]),
            false => {
                let from = b"%PDF-1.7\n8 0 obj\n<< /Title ".len();
                let to = b"%PDF-1.7\n".len() + eight.len() - ")\n".len();
                let eight = read_in_string(Some(ObjectId::new(8, 0)), from, to, 1);
                assert_eq!(in_string(&doc), [eight, six]);
                assert!(doc.get(ObjectId::new(7, 0)).is_ok());
            }
        }
    }
}

/// [`closed_late`], page 3, 20 wide, inside the string, after stream 9,
/// which has no `/Length` that reads, its data ended at an `endobj` of its
/// own, then a comment whose line holds its `endstream` and page 3, 10
/// wide: the first scan finds page 3 only inside the string, the second,
/// to each stream's first `endstream`, as written on the comment's line,
/// and that one stands, every way, as the scan before either read it;
/// only object 5 is read from inside the string, and said.
#[test]
fn an_object_inside_a_string_closed_late_that_the_second_scan_finds_elsewhere_isnt_taken() {
    let head = "9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n% endstream endobj ";
    let before = format!("{head}3 0 obj {} endobj\n", OBJECTS[2].1);
    let (file, start, end) = closed_late(&before, WIDE_PAGE, "");
    let doc = read(&file);
    reaches_its_page(&doc);
    assert_eq!(width(&doc), 10);
    let at = b"%PDF-1.7\n".len() + head.len();
    assert_eq!(doc.record(ObjectId::new(3, 0)).unwrap().at.start, at);
    assert_eq!(in_string(&doc), [read_in_string(SIX, start, end, 1)]);
    let data = b"%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\n".len();
    let said = ended(NINE, data, Some(data + "q Q\n".len()));
    assert_eq!(stream_ends(&doc), [said]);
}

/// Object stream `n`, unfiltered, of `members`, each a number and its
/// value, written whole.
fn object_stream(n: u32, members: &[(u32, &str)]) -> String {
    let mut header = String::new();
    let mut body = String::new();
    for (m, value) in members {
        header.push_str(&format!("{m} {} ", body.len()));
        body.push_str(value);
        body.push('\n');
    }
    let data = format!("{header}\n{body}");
    format!(
        "{n} 0 obj\n<< /Type /ObjStm /N {} /First {} /Length {} >>\nstream\n{data}\nendstream\nendobj\n",
        members.len(),
        header.len() + 1,
        data.len()
    )
}

/// Page 3, 20 wide, as an object stream's member.
const WIDE: &str = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Resources << >> >>";

/// [`closed_late`], object stream 7, holding page 3, inside the string:
/// 7 is read there, by its `/Type` an object stream, so page 3 is read
/// from it, every way; where object stream 8, as written before the
/// string, holds page 3 too, 10 wide, 8's member stands, though 7 comes
/// later, as what a string closed late holds stands over nothing found as
/// written; and where page 3, 20 wide, is inside the string as an object of
/// its own, 8's member stands over it too, which then isn't said.
#[test]
fn an_object_stream_inside_a_string_closed_late_lists_its_members_under_the_others() {
    let seven = object_stream(7, &[(3, OBJECTS[2].1)]);
    let (file, start, end) = closed_late("", &seven, "");
    let doc = read(&file);
    reaches_its_page(&doc);
    let origin = |doc: &Document| doc.record(ObjectId::new(3, 0)).unwrap().origin;
    let stream = |stream| Origin::Stream { stream, index: 0 };
    assert_eq!(origin(&doc), stream(7));
    assert_eq!(in_string(&doc), [read_in_string(SIX, start, end, 2)]);

    let eight = object_stream(8, &[(3, OBJECTS[2].1)]);
    let seven = object_stream(7, &[(3, WIDE)]);
    let (file, start, end) = closed_late(&eight, &seven, "");
    let doc = read(&file);
    reaches_its_page(&doc);
    assert_eq!(width(&doc), 10);
    assert_eq!(origin(&doc), stream(8));
    assert_eq!(in_string(&doc), [read_in_string(SIX, start, end, 2)]);

    let (file, start, end) = closed_late(&eight, WIDE_PAGE, "");
    let doc = read(&file);
    reaches_its_page(&doc);
    assert_eq!(width(&doc), 10);
    assert_eq!(origin(&doc), stream(8));
    assert_eq!(in_string(&doc), [read_in_string(SIX, start, end, 1)]);
    let members = Fault::Members {
        objects: 1,
        streams: 1,
        unread: 0,
    };
    assert!(doc.faults().iter().any(|(_, f)| *f == members));
}

/// A scanned file whose object 6, after the trailer, holds in a string a
/// document's text, the catalog's and page 3's `n g obj` each starting a
/// line, and its `trailer`, numbers the file uses: nothing in it is taken,
/// every way, the file's own catalog and page standing, and nothing's
/// said; and one whose string holds a `trailer` at a line's start naming
/// another catalog: it isn't taken for one.
#[test]
fn a_string_holding_a_documents_text_gives_no_object_found_elsewhere_nor_a_trailer() {
    let text = "6 0 obj\n<< /Source (%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 9 0 R >>\nendobj\n3 0 obj\n<< /Type /Page >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n) >>\nendobj\n";
    let (file, places) = scanned_after(b"", text.as_bytes());
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert!(doc.get(ObjectId::new(6, 0)).is_ok());
    assert_eq!(in_string(&doc), []);
    let note = "6 0 obj\n<< /Note (a note\ntrailer\n<< /Size 10 /Root 7 0 R >>\n) >>\nendobj\n";
    let (file, places) = scanned_after(b"", note.as_bytes());
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    let root = doc.trailer.dict().get(b"Root").map(Object::value);
    assert_eq!(root, Some(&Value::Ref(ObjectId::new(1, 0))));
    assert_eq!(in_string(&doc), []);
}

/// A scanned file whose stream 9 has no `/Length` that reads and an
/// `endobj` of its own in its data, then a comment whose line holds its
/// `endstream` and opens a string, then stream 8, whose data, passed by its
/// `/Length`, holds object 7, and the string's `)`: the second scan, to
/// each stream's first `endstream`, reads that string, 7's `n g obj`
/// starting a line inside it, but looks at no string's lines, as the scan
/// before either didn't, so 7 isn't read, every way, and nothing's said
/// of it; the first scan finds every object, and passes 8's data by its
/// `/Length`.
#[test]
fn an_object_starting_a_line_inside_a_string_the_second_scan_alone_reads_isnt_taken() {
    let data = "7 0 obj\n(seven)\nendobj\n)";
    let body = format!(
        "9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n% endstream endobj (late\n8 0 obj\n<< /Length {} >>\nstream\n{data}\nendstream\nendobj\n",
        data.len()
    );
    let (file, places) = scanned_after(body.as_bytes(), b"");
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert!(doc.get(ObjectId::new(7, 0)).is_err());
    assert!(
        doc.record(ObjectId::new(8, 0))
            .unwrap()
            .data
            .unwrap()
            .declared
    );
    assert_eq!(in_string(&doc), []);
    let start = b"%PDF-1.7\n9 0 obj\n<< /Length 999 >>\nstream\n".len();
    assert_eq!(stream_ends(&doc), [ended(NINE, start, Some(start + 4))]);
}

/// A scanned file whose stream 9 has no `/Length` that reads and an
/// `endobj` of its own in its data, after which the data holds a string,
/// object 7's `n g obj` starting a line inside it, then `endstream`, before
/// the catalog, its page and the trailer: the verified scan ends the data at
/// that `endobj`, reads the string, and meets `endstream` on its own
/// between objects, so the data ran there, and what it found inside is
/// taken back, object 7 among it, every way, and nothing's said.
#[test]
fn an_object_starting_a_line_inside_a_string_in_data_the_take_back_drops_isnt_taken() {
    let stream =
        b"9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n(late\n7 0 obj\n(seven)\nendobj\n)\nendstream\nendobj\n";
    let (file, places) = scanned_after(stream, b"");
    let doc = read(&file);
    each_object_at(&doc, &places, &|_| Origin::Recovered);
    reaches_its_page(&doc);
    assert!(doc.get(ObjectId::new(7, 0)).is_err());
    assert_eq!(in_string(&doc), []);
    assert_eq!(stream_ends(&doc), []);
}

/// [`Unclosing`] says, at each `(`, what pdf-tokenizer reads of the literal
/// string it starts: that it doesn't close, or that it does. Every input of
/// up to 8 bytes of `(`, `)`, a backslash and another byte, and seeded
/// inputs of up to 4 KiB of them, with ends of line.
#[test]
fn which_strings_dont_close_is_what_the_tokenizer_reads() {
    use pdf_tokenizer::Tokenizer;
    let check = |input: &[u8]| {
        let unclosing = Unclosing::of(input);
        for (q, &b) in input.iter().enumerate() {
            if b != b'(' {
                continue;
            }
            let token = Tokenizer::at(input, q).next().unwrap();
            assert_eq!(unclosing.at(q), !token.closed, "{input:?} at {q}");
        }
    };
    let alphabet = *b"()\\x";
    for len in 0..=8u32 {
        for mut code in 0..4usize.pow(len) {
            let mut input = Vec::new();
            for _ in 0..len {
                input.push(alphabet[code % 4]);
                code /= 4;
            }
            check(&input);
        }
    }
    let alphabet = *b"(()\\x\n\r";
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..32 {
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let len = (next() % 4096) as usize + 1;
        let input: Vec<u8> = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        check(&input);
    }
}

/// A scanned file without a `trailer` dictionary, whose last
/// cross-reference stream has only `/Size`, an earlier one `/Root`, as a
/// linearized file's may: its trailer is the last's, given `/Root` from
/// the earlier one, so its page is reached, every way, and says so.
#[test]
fn a_scanned_files_last_cross_reference_stream_takes_root_from_an_earlier_one() {
    let xref = |n: u32, more: &str| {
        format!(
            "{n} 0 obj\n<< /Type /XRef /Size 8 /W [1 1 1]{more} /Length 0 >>\nstream\nendstream\nendobj\n"
        )
    };
    let mut file = b"%PDF-1.7\n".to_vec();
    file.extend_from_slice(xref(6, " /Root 1 0 R").as_bytes());
    for &(n, text) in &OBJECTS {
        file.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
    }
    let last = file.len();
    file.extend_from_slice(xref(7, "").as_bytes());
    let doc = read(&file);
    reaches_its_page(&doc);
    assert_eq!(doc.trailer.stream, Some(ObjectId::new(7, 0)));
    let taken: Vec<&(Place, Fault)> = (doc.faults().iter())
        .filter(|(_, f)| matches!(f, Fault::Taken { .. }))
        .collect();
    let keys = vec![b"Root".to_vec()];
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].1, Fault::Taken { keys });
    let dict = last + "7 0 obj\n".len();
    assert_eq!(taken[0].0.at.map(|at| at.start), Some(dict));
}

/// A file of [`OBJECTS`] whose table's one subsection is numbered from 1
/// though it starts with object 0's row, the rows after it each the place
/// of the object one less than its number: as the writer meant it, numbered
/// from 0. Also each object's place and the table's.
fn misnumbered() -> (Vec<u8>, Vec<(u32, usize)>, usize) {
    let (mut file, places, table) = plain(b"");
    let rows = &mut file[table..];
    let at = rows.windows(4).position(|w| w == b"0 5\n").unwrap();
    rows[at..at + 3].copy_from_slice(b"1 5");
    (file, places, table)
}

/// [`misnumbered`]: each row in use exactly at its object's `n g obj` as
/// numbered from 0, and none so from 1, so the subsection is read from 0,
/// said once, each object where its row puts it and no object 5, every way
/// and with faults off; the page reached.
#[test]
fn a_first_subsection_from_1_whose_rows_land_from_0_is_read_from_0() {
    let (file, places, table) = misnumbered();
    let doc = read(&file);
    let renumbered = Fault::Renumbered {
        at: table,
        count: 5,
    };
    assert_eq!(doc.faults(), [(Place::new(None, None), renumbered)]);
    each_object_at(&doc, &places, &|_| Origin::Renumbered { by: 0 });
    reaches_its_page(&doc);
    assert!(doc.get(ObjectId::new(5, 0)).is_err());
    let ids: Vec<u32> = doc.objects().map(|(id, _)| id.number).collect();
    assert_eq!(ids, [1, 2, 3, 4]);
    assert!(doc.facts.recovered);
    assert_eq!(doc.trailer.unwritten, None);
    // Its rows as written.
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let sections = reading.sections().unwrap();
    assert_eq!(sections.sections[0].subsections, [(1, 5)]);
    // Four bytes more after the header, so every offset written, its
    // startxref's too, is four short: the table read at its move, and its
    // rows from 0 at that shift.
    let shifted = [&file[..9], b"abc\n", &file[9..]].concat();
    let doc = read(&shifted);
    let moved = Fault::Moved {
        at: table,
        found: table + 4,
        from_start: false,
        moved: 4,
        scanned: 0,
        neither: 0,
    };
    let renumbered = Fault::Renumbered {
        at: table + 4,
        count: 5,
    };
    let none = Place::new(None, None);
    assert_eq!(doc.faults(), [(none, renumbered), (none, moved)]);
    let four_on: Vec<(u32, usize)> = places.iter().map(|&(n, at)| (n, at + 4)).collect();
    each_object_at(&doc, &four_on, &|_| Origin::Renumbered { by: 4 });
    reaches_its_page(&doc);
    assert!(doc.get(ObjectId::new(5, 0)).is_err());
    // Under Recovery::Pdfium, as PDFium reads it: its table's first
    // object in use isn't where it's put, so the file is scanned.
    let mut pdfium = Options::DEFAULT;
    pdfium.recovery = Recovery::Pdfium;
    let doc = document(&file, &pdfium);
    assert!(doc.trailer.unwritten.is_some());
    assert!(
        !doc.faults()
            .iter()
            .any(|(_, f)| matches!(f, Fault::Renumbered { .. }))
    );
}

/// A first subsection numbered from 1 that starts with object 0's row but
/// whose rows land on their own objects as written, from 1, and one whose
/// rows land some from 0 and some from 1: each read as written, nothing
/// renumbered.
#[test]
fn a_first_subsection_from_1_whose_rows_dont_all_land_from_0_is_read_as_written() {
    // Objects 2 to 4, object 1 free at 65535, as written.
    let objects = [
        (2, "<< /Type /Catalog /Pages 3 0 R >>"),
        (3, "<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
        (4, OBJECTS[2].1),
    ];
    let mut file = b"%PDF-1.7\n".to_vec();
    let mut places = Vec::new();
    for (n, text) in objects {
        places.push((n, file.len()));
        file.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
    }
    let table = file.len();
    file.extend_from_slice(b"xref\n1 4\n0000000000 65535 f \n");
    for &(_, at) in &places {
        file.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
    }
    file.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 2 0 R >>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
    );
    let doc = read(&file);
    assert_eq!(doc.faults(), []);
    let ids: Vec<u32> = doc.objects().map(|(id, _)| id.number).collect();
    assert_eq!(ids, [2, 3, 4]);
    for (n, at) in places {
        let record = doc.record(ObjectId::new(n, 0)).unwrap();
        assert_eq!((record.at.start, record.origin), (at, Origin::File), "{n}");
    }
    // [`misnumbered`] with object 3's row the place of object 4, as
    // numbered from 1 as written, the rest from 0: neither way for all.
    let (mut both, _, table) = misnumbered();
    let rows = table
        + both[table..]
            .windows(4)
            .position(|w| w == b" f \n")
            .unwrap()
        + 4;
    let four = both.windows(7).position(|w| w == b"4 0 obj").unwrap();
    let third = rows + 2 * 20;
    assert_eq!(&both[third + 11..third + 20], b"00000 n \n");
    both[third..third + 10].copy_from_slice(format!("{four:010}").as_bytes());
    let doc = document(&both, &Options::DEFAULT);
    assert!(doc.get(ObjectId::new(1, 0)).is_err());
    assert!(
        !doc.faults()
            .iter()
            .any(|(_, f)| matches!(f, Fault::Renumbered { .. }))
    );
}

/// An update after [`misnumbered`] read from 0 ([`Base::of`]): its section
/// lists each object renumbered where it is, so that a reader of the
/// update's newest section reads each there as written; read back, each
/// object as before, the update's too, and the first table still read
/// from 0 and said.
#[test]
fn an_update_after_a_renumbered_subsection_lists_each_object_where_it_is() {
    let (file, places, table) = misnumbered();
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let before = reading.to_document().unwrap();
    let base = Base::of(&reading).unwrap();
    assert!(base.holds(&file[..]).unwrap());
    let id = ObjectId::new(base.size, 0);
    let object = Object::new(Value::Integer(7));
    let bodies = [(id, Body::New(Cow::Borrowed(&object)))];
    let mut out = file.clone();
    let written = write::update(
        &mut out,
        &base,
        bodies,
        None,
        None,
        &write::Options::DEFAULT,
    );
    let written = written.unwrap().unwrap();
    for &(n, _) in &places {
        assert!(written.section.lists(n), "{n}");
    }
    let back = read(&out);
    let renumbered = Fault::Renumbered {
        at: table,
        count: 5,
    };
    assert_eq!(back.faults(), [(Place::new(None, None), renumbered)]);
    each_object_at(&back, &places, &|_| Origin::File);
    for (id, object) in before.objects() {
        let (mut got, mut want) = (back.get(id).unwrap().clone(), object.clone());
        got.unplace();
        want.unplace();
        assert_eq!(got, want, "{id}");
    }
    assert_eq!(back.get(id).ok().and_then(Object::as_integer), Some(7));
    reaches_its_page(&back);
}
