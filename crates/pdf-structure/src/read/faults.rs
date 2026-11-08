//! The reader's faults as a reading keeps and tells them: none looked for
//! where a caller wants none, the parser asked for none and the reads made
//! only to say one left out, every object read alike all the same; told to
//! any sink in the whole read's order; and what an encrypted file grants,
//! as the reading says it opened.

use std::cell::Cell;
use std::ops::Range;
use std::sync::Mutex;

use pdf_crypt::{Encoding, Permissions};
use pdf_fixtures::fixture;

use super::lazy::files;
use super::tests::{KINDS, root, settings, to_encrypt};
use super::*;
use crate::fuzz::Counted;
use crate::source::Source;

/// `options` but for whether faults are looked for, `faults`.
fn looking(options: &Options<'_>, faults: Faults) -> Options<'static> {
    Options {
        passwords: &[],
        faults,
        ..*options
    }
}

/// `doc` with no faults, to be set beside one read with none looked for.
fn without_faults(mut doc: Document) -> Document {
    doc.faults.clear();
    doc
}

/// A parser's sink that counts how often it's asked whether faults are
/// wanted and how many it's told, passing each on to the read's.
struct Counting<'r> {
    read: Warned<'r>,
    asked: Cell<usize>,
    told: usize,
}

impl parse::Sink<'_> for Counting<'_> {
    fn fault(&mut self, at: Span, fault: parse::Fault<'_>) {
        self.told += 1;
        self.read.fault(at, fault);
    }

    fn wants(&self) -> bool {
        self.asked.set(self.asked.get() + 1);
        self.read.wants()
    }
}

/// A read that looks for no faults asks pdf-objects' parser for none: the
/// parser asks, and is told it wants none, so it tells none, and the read
/// keeps none; the value read is the one read with faults looked for, each
/// of them told then.
#[test]
fn with_faults_off_the_parser_is_asked_and_told_none() {
    let text = b"<< /A#zz 1 /C 99999999999999999999 >>";
    let id = ObjectId::new(3, 0);
    let mut values = Vec::new();
    for wants in [true, false] {
        let mut cx = Cx::new(wants);
        let mut counting = Counting {
            read: cx.parsed(Some(id)),
            asked: Cell::new(0),
            told: 0,
        };
        let value = Parser::at(text, 0).value(&mut counting).unwrap();
        let (asked, told) = (counting.asked.get(), counting.told);
        assert!(asked > 0, "the parser asks whether faults are wanted");
        match wants {
            true => {
                assert_eq!(told, 2, "{:?}", cx.faults);
                assert_eq!(cx.faults.len(), 2);
                assert!(cx.faults.iter().all(|(place, fault)| {
                    place.object == Some(id) && matches!(fault, Fault::Parse { .. })
                }));
            }
            false => {
                assert_eq!(told, 0, "asked for none, the parser tells none");
                assert_eq!(cx.faults, []);
            }
        }
        values.push(value);
    }
    assert_eq!(values[0], values[1], "the value read either way");
}

/// Every test file, read whole with no faults looked for, in memory and by
/// position a window at a time, keeps none, and reads every object, record,
/// trailer, revision, decoded object stream and the rest as it does with
/// them; and with them, most of the files have some.
#[test]
fn with_faults_off_none_is_kept_and_every_object_is_read_alike() {
    let mut faulty = 0;
    for (name, file, passwords) in files() {
        let options = Options {
            passwords: &passwords,
            ..Options::DEFAULT
        };
        let kept = document(&file, &options);
        faulty += usize::from(!kept.faults().is_empty());
        let quiet = Options {
            faults: Faults::Off,
            ..options
        };
        let none = document(&file, &quiet);
        assert_eq!(none.faults(), [], "{name}");
        assert_eq!(none, without_faults(kept.clone()), "{name}");
        for first in [1, 64] {
            let source = Counted::new(&file, first);
            let read = Reading::open(&source, &quiet).and_then(Reading::into_document);
            let read = read.expect("bytes in memory are read by position without fail");
            assert_eq!(read.faults(), [], "{name}, by position from {first}");
            assert_eq!(read, none, "{name}, by position from {first}");
        }
        // Read on demand, its faults none, and the document it makes the
        // whole read's.
        let reading = Reading::open(&file[..], &quiet).unwrap();
        for id in reading.ids().collect::<Vec<_>>().into_iter().rev() {
            let _ = reading.get(id);
        }
        assert_eq!(reading.faults(), [], "{name}, on demand");
        assert_eq!(reading.to_document().unwrap(), none, "{name}, on demand");
    }
    assert!(faulty >= 10, "{faulty} files with faults");
}

/// Bytes in memory read by position, the first window `first` bytes, each
/// range read recorded.
struct Ranges<'a> {
    bytes: &'a [u8],
    first: usize,
    read: Mutex<Vec<Range<usize>>>,
}

impl ByteSource for Ranges<'_> {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&self, range: Range<usize>) -> std::io::Result<Cow<'_, [u8]>> {
        self.read.lock().unwrap().push(range.clone());
        Ok(Cow::Borrowed(&self.bytes[range]))
    }

    fn first_window(&self) -> usize {
        self.first
    }
}

/// A reading by position that looks for no faults doesn't read the file's
/// end back for its `%%EOF`, which it would read only to say what's wrong
/// there: every range it reads one read with faults looked for reads too,
/// and the one more that one reads is the end's, back from the file's end;
/// and what it reads of the file is the same.
#[test]
fn with_faults_off_the_files_end_isnt_read_back() {
    let mut file = fixture::file("1.7", &super::tests::clean(), root(), fixture::Xref::Table);
    file.extend_from_slice(b"% more\n");
    let len = file.len();
    let ranges = |faults: Faults| {
        let source = Ranges {
            bytes: &file,
            first: len,
            read: Mutex::new(Vec::new()),
        };
        let options = looking(&Options::DEFAULT, faults);
        let doc = Reading::open(&source, &options)
            .and_then(Reading::into_document)
            .unwrap();
        let mut read = source.read.into_inner().unwrap();
        read.sort_by_key(|range| (range.start, range.end));
        (doc, read)
    };
    let (kept, mut on) = ranges(Faults::Kept);
    let (none, off) = ranges(Faults::Off);
    assert!(
        (kept.faults().iter()).any(|(_, fault)| *fault == Fault::AfterEof { bytes: 8 }),
        "{:?}",
        kept.faults()
    );
    assert_eq!(none, without_faults(kept));
    // What's read with faults looked for, less what's read without.
    for range in &off {
        let at = on.iter().position(|r| r == range);
        on.remove(at.unwrap_or_else(|| panic!("{range:?} read with no faults looked for alone")));
    }
    assert_eq!(on, vec![(0..len)], "the one more read is the end's");
}

/// A sink that keeps each fault it's told, and says whether it wants them.
struct Told {
    faults: Vec<(Place, Fault)>,
    wants: bool,
    asked: Cell<usize>,
}

impl Sink<Fault> for Told {
    fn fault(&mut self, at: Place, fault: Fault) {
        self.faults.push((at, fault));
    }

    fn wants(&self) -> bool {
        self.asked.set(self.asked.get() + 1);
        self.wants
    }
}

/// A sink that wants the first fault it's told alone.
struct First(Vec<(Place, Fault)>);

impl Sink<Fault> for First {
    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.push((at, fault));
    }

    fn wants(&self) -> bool {
        self.0.is_empty()
    }
}

/// What a source tells a sink is its faults, in the whole read's order: a
/// reading's of what it has read, whatever order it read in, which once it
/// has read every object is its document's; a sink that wants none is
/// told none, and one that stops wanting them after the first is told the
/// first alone.
#[test]
fn a_source_tells_its_faults_in_the_whole_reads_order() {
    for (name, file, passwords) in files() {
        let options = Options {
            passwords: &passwords,
            ..Options::DEFAULT
        };
        let doc = document(&file, &options);
        let reading = Reading::open(&file[..], &options).unwrap();
        // Read from the last object back, each object stream's members
        // before the objects at offsets.
        let mut ids: Vec<ObjectId> = reading.ids().collect();
        ids.reverse();
        let members: Vec<u32> = (reading.entries())
            .filter(|(_, _, entry)| matches!(entry, XrefEntry::Compressed { .. }))
            .map(|(n, _, _)| n)
            .collect();
        ids.sort_by_key(|id| members.binary_search(&id.number).is_err());
        for &id in &ids {
            let _ = reading.get(id);
        }
        let sources: [(&str, &dyn Source); 2] = [("the reading", &reading), ("its document", &doc)];
        for (which, source) in sources {
            let mut told = Vec::new();
            source.tell_faults(&mut told);
            assert_eq!(told, doc.faults(), "{name}: {which}");
            let mut unwanted = Told {
                faults: Vec::new(),
                wants: false,
                asked: Cell::new(0),
            };
            source.tell_faults(&mut unwanted);
            assert_eq!(unwanted.faults, [], "{name}: {which}");
            assert!(unwanted.asked.get() > 0, "{name}: {which}");
            let mut first = First(Vec::new());
            source.tell_faults(&mut first);
            let faults = doc.faults();
            assert_eq!(first.0, faults[..faults.len().min(1)], "{name}: {which}");
        }
    }
}

/// The whole read's order, of a file with a fault of each kind it
/// assembles but the decryptions': what the open told, the file's end
/// among it, then each object at an offset, by number, then each member of
/// an object stream, by number, then the members that overlap; and a
/// reading read in another order says them so.
#[test]
fn faults_are_told_opened_then_objects_then_members_then_overlaps() {
    // Objects 2 and 3 in object stream 4 at one offset, a stray # in what
    // they read; object 1 without its endobj; a line after the %%EOF.
    let file = super::tests::holding(b"2 0 3 0 << /A#zz 1 >>", 2, 8, &[(2, 0), (3, 1)]);
    let at = find(&file, b"endobj").unwrap();
    let mut file = [&file[..at], b"Endobj", &file[at + 6..]].concat();
    file.extend_from_slice(b"% more\n");
    let doc = document(&file, &Options::DEFAULT);
    let order: Vec<_> = (doc.faults().iter())
        .map(|(place, fault)| (place.object.map(|id| id.number), arm(fault)))
        .collect();
    assert_eq!(
        order,
        [
            (None, "the end"),
            (Some(1), "the parser's"),
            (Some(2), "the parser's"),
            (Some(3), "the parser's"),
            (Some(3), "an overlap"),
        ],
        "{:?}",
        doc.faults()
    );
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    for n in [3, 2, 1] {
        let _ = reading.get(ObjectId::new(n, 0));
    }
    assert_eq!(reading.faults(), doc.faults());
}

/// What a fault is, for
/// [`faults_are_told_opened_then_objects_then_members_then_overlaps`].
fn arm(fault: &Fault) -> &'static str {
    match fault {
        Fault::AfterEof { .. } => "the end",
        Fault::Parse { .. } => "the parser's",
        Fault::Inside { .. } => "an overlap",
        _ => "another",
    }
}

/// The revision of an encrypted test file of `kind`'s security handler.
fn revision(kind: pdf_crypt::writer::Kind) -> i64 {
    match kind {
        pdf_crypt::writer::Kind::Rc4By40 => 2,
        pdf_crypt::writer::Kind::Rc4By128 => 3,
        pdf_crypt::writer::Kind::Aes128 => 4,
        pdf_crypt::writer::Kind::Aes256R5 => 5,
        _ => 6,
    }
}

/// A file opened, by the user's password or the owner's, says what its
/// `/P` grants one who opens it by the user's, by its revision's meaning
/// of the bits, as its security handler reads them; and so it does
/// unlocked by either, and read whole. `/P` here grants printing, changes,
/// copying and notes, and none of the bits revision 3 adds, which
/// revision 2 reads as those four's.
#[test]
fn an_opened_file_says_what_its_p_grants_by_either_password() {
    let p = -3844;
    for kind in KINDS {
        let mut written = settings(kind, b"user");
        written.p = p;
        let file = fixture::encrypted(&written, &to_encrypt(&[]), root());
        let r = revision(kind);
        let (_, handler) = read_opened_by(&file, &[Password("user")]);
        let granted = handler.unwrap().permissions();
        assert_eq!(granted, Permissions::from_p(p, r), "{kind:?}");
        let low = Permissions::PRINT | Permissions::MODIFY | Permissions::COPY;
        let low = low | Permissions::ANNOTATE;
        let expected = if r == 2 { Permissions::ALL } else { low };
        assert_eq!(granted, expected, "{kind:?}");
        for (password, owner) in [("user", false), ("owner", true)] {
            let given = [Password(password)];
            let options = Options {
                passwords: &given,
                ..Options::DEFAULT
            };
            let by = Some((0, Encoding::Utf8));
            let opened = Encryption::opened(owner).by(by).permitting(granted);
            let reading = Reading::open(&file[..], &options).unwrap();
            assert_eq!(
                reading.encryption().as_opened(),
                opened,
                "{kind:?}, {password}"
            );
            assert_eq!(document(&file, &options).encryption().as_opened(), opened);
            let locked = Reading::open(&file[..], &Options::DEFAULT).unwrap();
            assert_eq!(*locked.encryption(), Encryption::Locked, "{kind:?}");
            let unlocked = locked.unlock(&given[0].encodings()).unwrap();
            let encryption = unlocked.encryption().clone();
            let Encryption::Opened {
                owner: by_owner,
                permissions,
                ..
            } = encryption
            else {
                panic!("{kind:?}, {password}: {encryption:?}");
            };
            assert_eq!((by_owner, permissions), (owner, granted), "{kind:?}");
            let whole = unlocked.into_document().unwrap();
            assert_eq!(*whole.encryption(), encryption, "{kind:?}, {password}");
        }
    }
}
