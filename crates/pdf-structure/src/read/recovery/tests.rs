//! Files whose sections or objects aren't where they say, read where the
//! file proves they are ([`Recovery::Verified`]), every way a reading
//! reads: each recovery one fault a section, each object read at a shift
//! [`Origin::Shifted`], and nothing taken that the file doesn't prove.

use pdf_objects::fault::Place;
use std::borrow::Cow;

use pdf_objects::object::{Object, ObjectId, Value};

use super::super::lengths::every_way;
use super::super::{Fault, Faults, Options, Reading, SectionRows, document};
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
/// way a reading reads it alike ([`every_way`]), and with faults off the
/// same but no fault, the reading still saying it recovered.
fn read(file: &[u8]) -> Document {
    let mut readings = every_way(file, &[]).into_iter();
    let (_, whole) = readings.next().unwrap();
    for (way, doc) in readings {
        assert_eq!(doc, whole, "{way}");
    }
    let mut quiet = Options::DEFAULT;
    quiet.faults = Faults::Off;
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

/// A file cut as hayro's issue53 is: a first section at the start, listing
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

/// hayro's issue53's shape: the main section, said three bytes past its
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
            section: table,
            by: -i64::try_from(by).unwrap(),
            from_start: false,
            moved: 4,
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
/// object 5's at another generation, and object 6's, one byte past its
/// object, so that at that shift it's the line's end before its `n g obj`:
/// those three aren't taken, read where they're written, and said as
/// neither.
#[test]
fn a_shift_onto_another_objects_header_isnt_taken() {
    let objects = [
        OBJECTS[0],
        OBJECTS[1],
        OBJECTS[2],
        OBJECTS[3],
        (5, "(five)"),
        (6, "(six)"),
    ];
    let mut file = b"%PDF-1.7\n".to_vec();
    let mut places = Vec::new();
    for &(n, body) in &objects {
        places.push((n, file.len()));
        let generation = if n == 5 { 1 } else { 0 };
        file.extend_from_slice(format!("{n} {generation} obj\n{body}\nendobj\n").as_bytes());
    }
    let at = |n: u32| places.iter().find(|&&(m, _)| m == n).unwrap().1;
    let table = file.len();
    file.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for n in 1..=6 {
        // Object 3's row is object 4's, two past; object 6's one past.
        let written = match n {
            3 => at(4) + 2,
            6 => at(6) + 1,
            n => at(n) + 2,
        };
        file.extend_from_slice(format!("{written:010} 00000 n \n").as_bytes());
    }
    file.extend_from_slice(
        format!("trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
    );
    let doc = read(&file);
    let shifted = Fault::Shifted {
        section: table,
        by: -2,
        from_start: false,
        moved: 3,
        neither: 3,
    };
    assert_eq!(recoveries(&doc), [shifted]);
    for n in [1, 2, 4] {
        let record = doc.record(ObjectId::new(n, 0)).unwrap();
        assert_eq!(record.origin, Origin::Shifted { by: -2 }, "{n}");
        assert_eq!(record.at.start, at(n), "{n}");
    }
    // None read: not object 4's bytes as 3, nor 5 1 obj as 5 0, nor 6
    // past the white-space its shift lands on.
    for n in [3, 5, 6] {
        assert!(doc.get(ObjectId::new(n, 0)).is_err(), "{n}");
    }
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

/// A reading opened as of a place a few bytes from a section: taken as
/// it's given, not looked for near, so the file is scanned.
#[test]
fn a_section_given_a_few_bytes_off_isnt_looked_for() {
    let (file, _, table) = plain(b"");
    let mut options = Options::DEFAULT;
    options.section = Some(table + 3);
    let doc = document(&file, &options);
    assert_eq!(recoveries(&doc), []);
    assert!(doc.trailer.unwritten.is_some());
    options.section = Some(table);
    let doc = document(&file, &options);
    assert_eq!(doc.trailer.unwritten, None);
    assert!(!doc.facts.recovered);
}

/// An update after a reading whose sections or objects were read
/// elsewhere ([`Base::of`]): its `/Prev` where the reading read the newest
/// section, so that the update read back reads that section where it's
/// said, and its section listing each object the reading read at a shift
/// where it is, so that it's read there as written; each object as the
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
    let rows_shift = Fault::Shifted {
        section: table,
        by: -2,
        from_start: false,
        moved: 4,
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
                neither: 0,
            }],
        ),
        (
            "from the first byte",
            start,
            vec![Fault::Shifted {
                section: first,
                by: -100,
                from_start: true,
                moved: 4,
                neither: 0,
            }],
        ),
        ("startxref past", past, vec![]),
        ("rows past", rows, vec![rows_shift]),
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
        assert_eq!(back.trailer.unwritten, None, "{name}");
        for (id, object) in before.objects() {
            let mut got = back.get(id).unwrap().clone();
            let mut want = object.clone();
            got.unplace();
            want.unplace();
            assert_eq!(got, want, "{name}: {id}");
            // One read at a shift the update lists where it is.
            let (got, want) = (back.record(id).unwrap(), before.record(id).unwrap());
            let origin = match want.origin {
                Origin::Shifted { .. } => Origin::File,
                origin => origin,
            };
            assert_eq!((got.origin, got.at), (origin, want.at), "{name}: {id}");
            let listed = written.section.lists(id.number);
            assert_eq!(
                listed,
                matches!(want.origin, Origin::Shifted { .. }),
                "{name}: {id}"
            );
        }
        assert_eq!(
            back.get(id).ok().and_then(Object::as_integer),
            Some(7),
            "{name}"
        );
    }
}
