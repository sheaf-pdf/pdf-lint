//! Where each revision's bytes end, as the reader finds it after each
//! section it reads, and the file as each revision was written: its bytes
//! up to its end read as a file of their own ([`Reading::prefix`]), keyed
//! by the reading's security handler where they're encrypted as its file
//! is.

use pdf_crypt::Encoding;

use super::tests::{
    KINDS, clean, compressed, encryption_updated, linearized, root, settings, startxref,
    to_encrypt, updated, xref_row,
};
use super::*;
use crate::fixture::{self, Xref};
use crate::fuzz::{Counted, open};

/// Where each of `file`'s revisions ends, the first first, as read whole.
fn ends(file: &[u8]) -> Vec<Option<(usize, usize)>> {
    read(file).revisions.iter().map(|r| r.end).collect()
}

/// [`clean`]'s objects with a table, cut after its `startxref`'s offset,
/// then `after`; and where `after` starts.
fn after_offset(after: &[u8]) -> (Vec<u8>, usize) {
    let file = fixture::file("1.7", &clean(), root(), Xref::Table);
    let at = rfind(&file, b"startxref").unwrap() + b"startxref\n".len();
    let cut = at + file[at..].iter().take_while(|b| b.is_ascii_digit()).count();
    ([&file[..cut], after].concat(), cut)
}

/// [`clean`]'s objects with a table, cut where its `startxref` starts, then
/// `tail`, given where the table starts.
fn from_startxref(tail: impl Fn(usize) -> String) -> Vec<u8> {
    let file = fixture::file("1.7", &clean(), root(), Xref::Table);
    let xref = startxref(&file);
    let at = rfind(&file, b"startxref").unwrap();
    [&file[..at], tail(xref).as_bytes()].concat()
}

/// A hybrid file's one section: a table whose trailer's `/XRefStm` is a
/// cross-reference stream before it, listing the objects in object stream
/// 4 that the table leaves free.
fn hybrid() -> Vec<u8> {
    let (file, _) = compressed(false);
    let mut file = file[..find(&file, b"5 0 obj").unwrap()].to_vec();
    let one = find(&file, b"1 0 obj").unwrap();
    let four = find(&file, b"4 0 obj").unwrap();
    let rows = [
        xref_row(0, 0, 255),
        xref_row(1, one, 0),
        xref_row(2, 4, 0),
        xref_row(2, 4, 1),
    ]
    .concat();
    let five = file.len();
    file.extend_from_slice(b"5 0 obj\n<< /Type /XRef /Size 4 /W [1 2 1] /Length 16 >>\nstream\n");
    file.extend_from_slice(&rows);
    file.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = file.len();
    file.extend_from_slice(
        format!(
            "xref\n0 6\n0000000000 65535 f \n{one:010} 00000 n \n0000000000 00000 f \n\
             0000000000 00000 f \n{four:010} 00000 n \n{five:010} 00000 n \n\
             trailer\n<< /Size 6 /Root 1 0 R /XRefStm {five} >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    file
}

/// A linearized file whose first-page section's trailer is followed by
/// `startxref`, `0` and `%%EOF`, as some writers put them, and whose main
/// section, after it, ends the file; and where that first `%%EOF`'s line
/// ends.
fn linearized_with_a_first_eof() -> (Vec<u8>, usize) {
    let mut file = b"%PDF-1.7\n".to_vec();
    let one = file.len();
    file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
    let first = file.len();
    let last = first + 160;
    let section = format!(
        "xref\n0 2\n0000000000 65535 f \n{one:010} 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R /Prev {last} >>\nstartxref\n0\n%%EOF\n"
    );
    file.extend_from_slice(section.as_bytes());
    let eof = file.len();
    file.resize(last, b' ');
    file.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 2 >>\n");
    file.extend_from_slice(format!("startxref\n{first}\n%%EOF\n").as_bytes());
    (file, eof)
}

/// A file of three revisions: [`clean`]'s objects with a cross-reference
/// stream, then an update that writes 2 again with a table, then one that
/// writes 6; and where each revision ends, past its `%%EOF`'s line.
fn three_revisions() -> (Vec<u8>, [usize; 3]) {
    let mut file = fixture::file("1.7", &clean(), root(), Xref::Stream);
    let first = (startxref(&file), file.len());
    let changed = file.len();
    file.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let second = file.len();
    file.extend_from_slice(
        format!(
            "xref\n2 1\n{changed:010} 00000 n \n\
             trailer\n<< /Size 7 /Root 1 0 R /Prev {} >>\nstartxref\n{second}\n%%EOF\r\n",
            first.0
        )
        .as_bytes(),
    );
    let two = file.len();
    let (file, _) = updated(file, 6, second);
    let three = file.len();
    (file, [first.1, two, three])
}

/// The five bytes `%%EOF` before where a revision ends, and the line's
/// end, CR LF, CR or LF, or none, from there.
#[test]
fn a_revision_ends_past_its_eof_and_past_a_line_end_right_after_it() {
    let cases: [(&[u8], usize, usize); 11] = [
        (b"\n%%EOF\r\n", 6, 8),
        (b"\n%%EOF\r", 6, 7),
        (b"\n%%EOF\n", 6, 7),
        (b"\n%%EOF", 6, 6),
        // One line's end, not the lines after it.
        (b"\n%%EOF\n\n", 6, 7),
        (b"\n%%EOF\r\r\n", 6, 7),
        (b"\n%%EOF\n\r\n", 6, 7),
        // Bytes after it on its line: its line's end isn't counted.
        (b"\n%%EOF junk\n", 6, 6),
        (b"\n%%EOFjunk\r\n", 6, 6),
        // White-space between the offset and `%%EOF`, or none at all.
        (b" \t\r\n\x0c\n%%EOF\n", 11, 12),
        (b"%%EOF\r\n", 5, 7),
    ];
    for (after, eof, line) in cases {
        let (file, at) = after_offset(after);
        let what = String::from_utf8_lossy(after);
        assert_eq!(ends(&file), [Some((at + eof, at + line))], "{what:?}");
    }
}

/// None where anything but white-space comes between `startxref`'s offset
/// and `%%EOF`, a comment or a NUL among it, or no `%%EOF` follows; nor
/// between `startxref` and its offset, which is digits alone. Its section
/// is read all the same, and so is its revision.
#[test]
fn a_revision_whose_startxref_isnt_followed_by_eof_alone_has_no_end() {
    let afters: [&[u8]; 8] = [
        b"\n% a comment\n%%EOF\n",
        b"\n\0%%EOF\n",
        b"\n",
        b"",
        b"\n%%EO",
        b"\n%EOF\n",
        b"\nxref\n%%EOF\n",
        b" 0\n%%EOF\n",
    ];
    for after in afters {
        let (file, _) = after_offset(after);
        let what = String::from_utf8_lossy(after);
        let doc = read(&file);
        assert!(doc.trailer.unwritten.is_none(), "{what:?}");
        assert_eq!(ends(&file), [None], "{what:?}");
    }
    let tails: [fn(usize) -> String; 3] = [
        |xref| format!("startxref % a comment\n{xref}\n%%EOF\n"),
        |xref| format!("startxref\n+{xref}\n%%EOF\n"),
        |xref| format!("startxref\0{xref}\n%%EOF\n"),
    ];
    for tail in tails {
        let file = from_startxref(tail);
        let what = String::from_utf8_lossy(&file[file.len() - 20..]).into_owned();
        assert!(read(&file).trailer.unwritten.is_none(), "{what:?}");
        assert_eq!(ends(&file), [None], "{what:?}");
    }
    // White-space alone between them, on lines of any end, ends it.
    let file = from_startxref(|xref| format!("startxref\r\n{xref}\r\n%%EOF\r\n"));
    assert_eq!(ends(&file), [Some((file.len() - 2, file.len()))]);
    let file = from_startxref(|xref| format!("startxref {xref} %%EOF"));
    assert_eq!(ends(&file), [Some((file.len(), file.len()))]);
}

/// A table, a cross-reference stream past its `endobj` and a hybrid's
/// table each end their revision where the `%%EOF` after them does.
#[test]
fn each_kind_of_section_ends_its_revision_at_the_eof_after_it() {
    let files = [
        fixture::file("1.7", &clean(), root(), Xref::Table),
        fixture::file("1.7", &clean(), root(), Xref::Stream),
        hybrid(),
    ];
    for file in files {
        let doc = read(&file);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(ends(&file), [Some((file.len() - 1, file.len()))]);
    }
    let doc = read(&hybrid());
    assert!(doc.facts.hybrid);
}

/// A linearized file's one revision ends where its main section's `%%EOF`
/// does, the section written last: whether or not its first-page
/// section's trailer is followed by a `startxref` and `%%EOF` of its own,
/// which may be left out (ISO 32000-2, F.3.4). Updated, the update ends
/// another.
#[test]
fn a_linearized_revision_ends_where_its_main_section_does() {
    let (file, first) = linearized();
    let doc = read(&file);
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    assert_eq!(doc.facts.sections, 2);
    assert_eq!(ends(&file), [Some((file.len() - 1, file.len()))]);
    let (with, eof) = linearized_with_a_first_eof();
    let doc = read(&with);
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    assert_eq!(doc.facts.sections, 2);
    assert_ne!(eof, with.len());
    assert_eq!(ends(&with), [Some((with.len() - 1, with.len()))]);
    let length = file.len();
    let (file, _) = updated(file, 2, first);
    assert_eq!(
        ends(&file),
        [
            Some((length - 1, length)),
            Some((file.len() - 1, file.len()))
        ]
    );
}

/// Each of three revisions ends at its own `%%EOF`, each after the one
/// before; read as of an earlier section, the revisions up to it end as
/// the whole's do, and so do a reading's on demand and by position. A file
/// scanned has none.
#[test]
fn each_revision_ends_at_its_own_eof_however_its_read() {
    let (file, [one, two, three]) = three_revisions();
    let doc = read(&file);
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    let expected = [
        Some((one - 1, one)),
        Some((two - 2, two)),
        Some((three - 1, three)),
    ];
    assert_eq!(ends(&file), expected);
    for k in 0..3 {
        let as_of = read_from(&file, doc.revisions[k].at);
        assert_eq!(as_of.revisions, doc.revisions[..=k], "{k}");
        let mut options = Options::DEFAULT;
        options.section = Some(doc.revisions[k].at);
        let reading = Reading::open(&file[..], &options).unwrap();
        assert_eq!(reading.revisions(), &doc.revisions[..=k], "{k}");
    }
    for first in [1, 2, 5, 13, 64, 4096] {
        for bound in [true, false] {
            let source = Counted::new(&file, first);
            let reading = open(&source, &[], &Options::DEFAULT, bound).unwrap();
            assert_eq!(reading.revisions(), &doc.revisions[..], "{first} {bound}");
        }
    }
    // Scanned, as its section doesn't read: none.
    let file = fixture::file("1.7", &clean(), root(), Xref::Table);
    let at = rfind(&file, b"startxref").unwrap();
    let scanned = [&file[..at], b"startxreF", &file[at + 9..]].concat();
    let doc = read(&scanned);
    assert!(doc.trailer.unwritten.is_some());
    assert!(doc.revisions.is_empty());
}

/// A source's first bytes: as many as it has, where it has fewer; in
/// memory where it holds them, cut there; read by position within them,
/// and an error past them, as a read past a source's own bytes is.
#[test]
fn a_prefix_is_its_sources_first_bytes_and_no_more() {
    let bytes = b"%PDF-1.7\nsome bytes\n";
    let prefix = Prefix::new(&bytes[..], 5);
    assert_eq!(prefix.len(), 5);
    assert_eq!(prefix.whole(), Some(&b"%PDF-"[..]));
    assert_eq!(prefix.read_at(1..5).unwrap(), &b"PDF-"[..]);
    assert_eq!(prefix.read_at(5..5).unwrap(), &b""[..]);
    let past = prefix.read_at(3..6).unwrap_err();
    assert_eq!(past.kind(), std::io::ErrorKind::UnexpectedEof);
    assert_eq!(past.to_string(), "bytes 3..6 aren't within the 5 there are");
    assert_eq!(*prefix.source(), &bytes[..]);
    assert_eq!(Prefix::new(&bytes[..], 999).len(), bytes.len());
    assert_eq!(Prefix::new(&bytes[..], 999).whole(), Some(&bytes[..]));
    // Read by position, its first window the source's.
    let source = Counted::new(bytes, 7);
    let prefix = Prefix::new(&source, 12);
    assert_eq!((prefix.len(), prefix.whole()), (12, None));
    assert_eq!(prefix.first_window(), 7);
    assert_eq!(prefix.read_at(9..12).unwrap(), &b"som"[..]);
    assert!(prefix.read_at(9..13).is_err());
    assert!(prefix.unchanged().unwrap());
}

/// Each revision's prefix, at its end, is the file as it was written: its
/// trailer the revision's, its revisions the whole's up to it, ends and
/// all, every object no later revision lists the whole's, and one a later
/// one wrote again its own; cut at the `%%EOF` alone, the same, but that
/// its newest revision's end has no line's end. Cut where no revision
/// ends, it's read from the last `startxref` in it, as any file is.
#[test]
fn each_revisions_prefix_is_the_file_as_it_was_written() {
    let (file, _) = three_revisions();
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let revisions = reading.revisions();
    assert_eq!(revisions.len(), 3);
    for (k, revision) in revisions.iter().enumerate() {
        let (eof, line) = revision.end.unwrap();
        let prefix = reading.prefix(line, reading.limits()).unwrap();
        assert_eq!(prefix.size(), line, "{k}");
        assert!(prefix.warnings().is_empty(), "{k}: {:?}", prefix.warnings());
        assert_eq!(prefix.trailer(), &revision.trailer, "{k}");
        assert_eq!(prefix.revisions(), &revisions[..=k], "{k}");
        assert_eq!(prefix.update().is_some(), k > 0, "{k}");
        let later: Vec<u32> = (revisions[k + 1..].iter())
            .flat_map(|r| r.numbers.as_deref().unwrap_or_default().iter().copied())
            .collect();
        let ids: Vec<ObjectId> = prefix.ids().collect();
        assert!(!ids.is_empty());
        for id in ids {
            let own = prefix.get(id).unwrap();
            match later.contains(&id.0) {
                false => assert_eq!(own, reading.get(id).unwrap(), "{k}: {id:?}"),
                true => assert_ne!(own, reading.get(id).unwrap(), "{k}: {id:?}"),
            }
        }
        // At the `%%EOF` alone.
        let cut = reading.prefix(eof, reading.limits()).unwrap();
        assert!(cut.warnings().is_empty(), "{k}: {:?}", cut.warnings());
        assert_eq!(cut.revisions()[..k], revisions[..k], "{k}");
        let newest = cut.revisions().last().unwrap();
        assert_eq!(newest.end, Some((eof, eof)), "{k}");
        assert_eq!(newest.trailer, revision.trailer, "{k}");
        assert_eq!(
            cut.into_document().unwrap().objects().count(),
            prefix.into_document().unwrap().objects().count()
        );
    }
    // Into the third revision's body: the second's, and the bytes after its
    // last `%%EOF`, its line's end and seven more, said.
    let (_, second) = revisions[1].end.unwrap();
    let prefix = reading.prefix(second + 7, reading.limits()).unwrap();
    assert_eq!(prefix.revisions(), &revisions[..2]);
    let said = prefix.warnings();
    assert!(
        said.iter()
            .any(|w| w.message == "the file goes on for 9 bytes after its last %%EOF"),
        "{said:?}"
    );
    // Short of the first's `%%EOF`: its section read, its end not known.
    let (eof, _) = revisions[0].end.unwrap();
    let prefix = reading.prefix(eof - 1, reading.limits()).unwrap();
    let first = &prefix.revisions()[0];
    assert_eq!(
        (first.at, &first.trailer),
        (revisions[0].at, &revisions[0].trailer)
    );
    assert_eq!(first.end, None);
    // Past the file, the file.
    let whole = reading.prefix(file.len() + 9, reading.limits()).unwrap();
    assert_eq!(whole.size(), file.len());
    assert_eq!(whole.revisions(), revisions);
}

/// A prefix reads under the limits it's given, not its reading's: under a
/// budget too small for the object stream its page is in, the page
/// doesn't read in it, though it does under the reading's own.
#[test]
fn a_prefix_reads_under_the_limits_its_given() {
    let (file, _) = compressed(false);
    let five = startxref(&file);
    let (file, _) = updated(file, 6, five);
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    assert!(reading.get((3, 0)).is_ok());
    let (_, end) = reading.revisions()[0].end.unwrap();
    let full = reading.prefix(end, reading.limits()).unwrap();
    assert_eq!(full.limits(), reading.limits());
    assert!(full.get((3, 0)).is_ok());
    // The cross-reference stream's 24 bytes of rows, then 40 for the object
    // stream.
    let mut limits = *reading.limits();
    limits.budget = 24 + 40;
    let small = reading.prefix(end, &limits).unwrap();
    assert_eq!(small.limits(), &limits);
    assert!(small.get((1, 0)).is_ok());
    assert!(small.get((3, 0)).is_err());
    assert!(reading.get((3, 0)).is_ok());
}

/// A reading by position gives the prefixes a reading in memory does, in
/// windows of any size, an object's first window reaching the next listed
/// offset or not.
#[test]
fn a_prefix_by_position_is_the_prefix_in_memory() {
    let (file, _) = three_revisions();
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    for revision in reading.revisions() {
        let (_, end) = revision.end.unwrap();
        let in_memory = reading.prefix(end, reading.limits()).unwrap();
        let in_memory = in_memory.into_document().unwrap();
        for first in [1, 3, 8, 21, 64, 4096] {
            for bound in [true, false] {
                let source = Counted::new(&file, first);
                let by = open(&source, &[], &Options::DEFAULT, bound).unwrap();
                let prefix = by.prefix(end, by.limits()).unwrap();
                assert!(prefix.source().whole().is_none());
                let read = prefix.into_document().unwrap();
                assert_eq!(read, in_memory, "{end} {first} {bound}");
            }
        }
    }
}

/// `file`, [`fixture::encrypted`]'s with its encryption dictionary 6 0 R,
/// and an update after it that writes object 7, a dictionary of no
/// strings, under a trailer whose `/ID` is `id` twice.
fn encrypted_updated(file: &[u8], id: &str) -> Vec<u8> {
    let prev = startxref(file);
    let mut file = file.to_vec();
    let seven = file.len();
    file.extend_from_slice(b"7 0 obj\n<< /N 7 >>\nendobj\n");
    let xref = file.len();
    file.extend_from_slice(
        format!(
            "xref\n7 1\n{seven:010} 00000 n \n\
             trailer\n<< /Size 8 /Root 1 0 R /Encrypt 6 0 R /ID [({id}) ({id})] /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    file
}

/// A password that opens none of the tests' files, in more encodings than
/// one, so that the index of a password after it among all their
/// encodings isn't its own.
const NAIVE: Password<'static> = Password("naïve");

/// An encrypted file's prefix, encrypted by the same dictionary and first
/// `/ID` element, is opened by its reading's handler, whichever password
/// opened that: its strings decrypted as the reading's, its encryption the
/// reading's, which password opened it by that password's index among
/// those given, never asked for again.
#[test]
fn an_encrypted_files_prefix_is_opened_by_its_readings_handler() {
    assert!(NAIVE.encodings().len() > 1);
    for kind in KINDS {
        let base = fixture::encrypted(&settings(kind, b"user"), &to_encrypt(&[]), root());
        let file = encrypted_updated(&base, "0123456789abcdef");
        for (password, owner) in [("user", false), ("owner", true)] {
            let passwords = [NAIVE, Password(password)];
            let reading = Reading::open_with(&file[..], &passwords, &Options::DEFAULT).unwrap();
            let by = Some((1, Encoding::Utf8));
            let opened = Encryption::Opened { owner, by };
            assert_eq!(reading.encryption(), &opened, "{kind:?}");
            let (_, end) = reading.revisions()[0].end.unwrap();
            assert_eq!(end, base.len());
            let prefix = reading.prefix(end, reading.limits()).unwrap();
            assert_eq!(prefix.encryption(), &opened, "{kind:?} {password}");
            assert!(prefix.warnings().is_empty(), "{:?}", prefix.warnings());
            let note = prefix.get((4, 0)).unwrap().as_dict().unwrap();
            let contents = note.get(b"Contents").unwrap().value();
            assert!(
                matches!(contents, Value::String(Str::Bytes(b), _) if b == b"A note"),
                "{kind:?}: {contents:?}"
            );
            for n in 1..=5 {
                let (own, whole) = (prefix.get((n, 0)), reading.get((n, 0)));
                assert_eq!(own.unwrap(), whole.unwrap(), "{kind:?} {n}");
            }
        }
    }
}

/// A prefix encrypted otherwise than its reading's file, by another
/// encryption dictionary or under another first `/ID` element, is opened
/// as any file is, by the empty password, and not by the reading's
/// handler; a locked reading's prefix is opened so too.
#[test]
fn a_prefix_encrypted_otherwise_is_opened_as_any_file() {
    let kind = pdf_crypt::writer::Kind::Aes256;
    let base = fixture::encrypted(&settings(kind, b"user"), &to_encrypt(&[]), root());
    let end = base.len();
    let user = [Password("user")];
    let opened = Encryption::Opened {
        owner: false,
        by: Some((0, Encoding::Utf8)),
    };
    // Another first /ID element.
    let file = encrypted_updated(&base, "fedcba9876543210");
    let reading = Reading::open_with(&file[..], &user, &Options::DEFAULT).unwrap();
    assert_eq!(reading.encryption(), &opened);
    assert_eq!(reading.revisions()[0].end, Some((end - 1, end)));
    let prefix = reading.prefix(end, reading.limits()).unwrap();
    assert_eq!(prefix.encryption(), &Encryption::Locked);
    // Another encryption dictionary: its permissions another.
    let file = encryption_updated(&base, "/P -4", "/P -8", &[]);
    let reading = Reading::open_with(&file[..], &user, &Options::DEFAULT).unwrap();
    assert!(matches!(reading.encryption(), Encryption::Opened { .. }));
    let prefix = reading.prefix(end, reading.limits()).unwrap();
    assert_eq!(prefix.encryption(), &Encryption::Locked);
    // The same dictionary written again: keyed.
    let file = encryption_updated(&base, "/P -4", "/P -4", &[]);
    let reading = Reading::open_with(&file[..], &user, &Options::DEFAULT).unwrap();
    let prefix = reading.prefix(end, reading.limits()).unwrap();
    assert_eq!(prefix.encryption(), &opened);
    // A locked reading's: locked, and unlocked by the password as any is.
    let file = encrypted_updated(&base, "0123456789abcdef");
    let wrong = [Password("wrong")];
    let reading = Reading::open_with(&file[..], &wrong, &Options::DEFAULT).unwrap();
    assert_eq!(reading.encryption(), &Encryption::Locked);
    let prefix = reading.prefix(end, reading.limits()).unwrap();
    assert_eq!(prefix.encryption(), &Encryption::Locked);
    let encoded = [pdf_crypt::Encoded::verbatim(b"user")];
    let unlocked = prefix.unlock(&encoded).unwrap();
    let by = Some((0, Encoding::Verbatim));
    assert_eq!(unlocked.encryption(), &Encryption::opened(false).by(by));
    // A file the empty password opens: its prefix too, as the empty
    // password opens it, whichever opened the reading.
    let base = fixture::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
    let file = encrypted_updated(&base, "0123456789abcdef");
    let end = base.len();
    let owner = [Password("owner")];
    let reading = Reading::open_with(&file[..], &owner, &Options::DEFAULT).unwrap();
    assert_eq!(
        reading.encryption(),
        &Encryption::opened(true).by(Some((0, Encoding::Utf8)))
    );
    let prefix = reading.prefix(end, reading.limits()).unwrap();
    assert_eq!(prefix.encryption(), &Encryption::opened(false));
}
