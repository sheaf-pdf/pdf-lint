//! What the fuzz target checks, which a unit test checks too: on any
//! bytes, a file is read without a panic, in the same way twice, with
//! every place the reading gives inside what was read; read on demand, in
//! an order the input chooses, it's the whole read, so long as no stream
//! either decodes reaches the budget; read by position, a window at a time
//! from a first window the input chooses, it's the whole read, and each
//! stream's data and decoded data are as in memory; pruned, it reaches what
//! it did, and an object added to it is at a number none held; and so are
//! its revision before an incremental update, and the file opened by an
//! owner's password. It reads under small limits, so that a run tries more
//! inputs. It's compiled only for tests and under cargo-fuzz's `--cfg
//! fuzzing`, and only reads.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use pdf_objects::object::{Object, ObjectId, Span, Value};

use crate::bytes::{ByteSource, Error, Options, Reading, Whole, XrefEntry};
use crate::source::{Document, Source};

/// The owner's password the fuzz target opens a file by, when its input
/// says to: the one the unit test's encrypted files are written with.
const OWNER: &[u8] = b"owner";

/// The limits the fuzz target reads under: a stream decodes to at most
/// 1 MiB, and a reading's streams to 4 MiB, so that a run tries more
/// inputs.
pub fn options() -> Options {
    let mut options = Options::DEFAULT;
    options.limits.stream = 1 << 20;
    options.limits.budget = 4 << 20;
    options
}

/// Bytes in memory, read a window at a time as a file is, by position,
/// the first window `first` bytes: counting the bytes read.
#[derive(Debug)]
pub struct Counted<'a> {
    bytes: &'a [u8],
    first: usize,
    read: AtomicUsize,
}

impl<'a> Counted<'a> {
    /// `bytes`, read by position, `first` bytes first.
    pub fn new(bytes: &'a [u8], first: usize) -> Counted<'a> {
        Counted {
            bytes,
            first,
            read: AtomicUsize::new(0),
        }
    }

    /// How many bytes have been read.
    pub fn read(&self) -> usize {
        self.read.load(Ordering::Relaxed)
    }
}

impl ByteSource for Counted<'_> {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        self.read.fetch_add(range.len(), Ordering::Relaxed);
        let (start, end) = (range.start, range.end);
        self.bytes.get(range).map(Cow::Borrowed).ok_or_else(|| {
            let message = format!("bytes {start}..{end} aren't within the file");
            io::Error::new(io::ErrorKind::UnexpectedEof, message)
        })
    }

    fn first_window(&self) -> usize {
        self.first
    }
}

/// `source` opened as [`Reading::open`] opens it, but that an object read
/// by position is first given a window to a byte past the next offset the
/// cross-references list only when `bound`: without, a small first window
/// reaches every read's windows.
pub fn open<S: ByteSource>(
    source: S,
    passwords: &[&[u8]],
    options: &Options,
    bound: bool,
) -> Result<Reading<S>, Error> {
    Reading::open_by(source, passwords, options, bound)
}

/// The fuzz target's body: `data`'s first two bytes choose how the file
/// after them is read, and the rest is the file. Read whole under
/// [`options`], it's the same twice, and every place the reading gives is
/// inside what was read: each object's, each value's and key's in it, each
/// stream's data's, the trailer's and each warning's, in the file or in
/// the decoded data of the object stream it names, which the reading
/// keeps. Read on demand, its objects asked for in an order the choice's
/// next bits give, some read unkept first, each the object and record it's
/// kept as, it's the whole read, when neither reading's decoding reached
/// the budget. Read by position, through [`Counted`], its first window and
/// whether an object's reaches the next listed offset as the choice's
/// bits from the fourth give, it's the whole read, and each stream's data
/// and decoded data are as in memory. Pruned, what its trailer reaches is
/// the same, and every object it keeps is among it, and an object added
/// is at a number no entry held ([`pruned`]). When the file has an
/// incremental update, so is the revision before it ([`Options::section`]);
/// and with the choice's lowest bit set, so is the file opened by the
/// owner's password `owner`, and its revision before an update. Panics on
/// a broken promise.
pub fn read_file(data: &[u8]) {
    let (choice, file) = match data {
        [a, b, file @ ..] => (u16::from_le_bytes([*a, *b]), file),
        _ => (0, data),
    };
    alike(file, &[], choice);
    if choice & 1 != 0 {
        alike(file, &[OWNER], choice);
    }
}

/// `file` read whole under [`options`], opened by `passwords`, as of the
/// section at `section` when there's one.
fn whole(file: &[u8], passwords: &[&[u8]], section: Option<usize>) -> Whole {
    let mut options = options();
    options.section = section;
    Reading::in_memory(file, passwords, &options, true, None).whole(None)
}

/// The file read whole twice, the same each time, every place it gives
/// inside `file`'s bytes or what it decoded, and read on demand in the
/// order `choice` gives, the same; and when the file has an update, the
/// same of the revision before it.
fn alike(file: &[u8], passwords: &[&[u8]], choice: u16) {
    let read = whole(file, passwords, None);
    let doc = &read.document;
    assert_eq!(
        *doc,
        whole(file, passwords, None).document,
        "a reading is the same twice"
    );
    within(file, doc);
    pruned(doc);
    on_demand(file, passwords, None, &read, choice);
    by_position(file, passwords, None, doc, choice);
    streams(file, passwords, choice);
    if let Some(update) = &doc.update {
        let section = Some(update.previous);
        let previous = whole(file, passwords, section);
        assert_eq!(
            previous.document,
            whole(file, passwords, section).document,
            "the revision before an update reads the same twice"
        );
        within(file, &previous.document);
        pruned(&previous.document);
        on_demand(file, passwords, section, &previous, choice);
        by_position(file, passwords, section, &previous.document, choice);
    }
}

/// The file, as of `section`, opened by `passwords`, read by position
/// through [`Counted`], its first window 1 to 64 bytes as `choice`'s bits
/// from the fifth give, and an object's first window to the next listed
/// offset when its fourth is set: read whole, it's `doc`, the whole read,
/// which decodes its streams in the same order, so whether or not the
/// budget is reached.
fn by_position(
    file: &[u8],
    passwords: &[&[u8]],
    section: Option<usize>,
    doc: &Document,
    choice: u16,
) {
    let mut options = options();
    options.section = section;
    let first = 1 + usize::from((choice >> 4) & 63);
    let source = Counted::new(file, first);
    let read = open(&source, passwords, &options, choice & 8 != 0)
        .and_then(|reading| reading.into_document());
    let read = read.expect("bytes in memory are read by position without fail");
    assert_eq!(read, *doc, "a reading by position is the whole read");
}

/// The file, opened by `passwords`, in memory and read by position through
/// [`Counted`] as [`by_position`] reads it: each object's data, and its
/// data decoded, the same each way, or the same error; the data, where
/// it's borrowed, a borrow of the file's bytes.
fn streams(file: &[u8], passwords: &[&[u8]], choice: u16) {
    let options = options();
    let reading = Reading::in_memory(file, passwords, &options, true, None);
    let first = 1 + usize::from((choice >> 4) & 63);
    let source = Counted::new(file, first);
    let by_position = open(&source, passwords, &options, choice & 8 != 0)
        .expect("bytes in memory are read by position without fail");
    for id in reading.ids() {
        let data = reading.data(id);
        if let Ok(Cow::Borrowed(data)) = &data {
            let within = file.as_ptr_range();
            assert!(
                data.is_empty() || within.contains(&data.as_ptr()),
                "{id:?}'s data is borrowed from elsewhere"
            );
        }
        let read = by_position.data(id);
        assert_eq!(
            format!("{data:?}"),
            format!("{read:?}"),
            "{id:?}'s data by position"
        );
        let decoded = format!("{:?}", reading.decoded(id));
        let read = format!("{:?}", by_position.decoded(id));
        assert_eq!(decoded, read, "{id:?} decoded by position");
    }
}

/// The file, as of `section`, opened by `passwords`, read on demand: its
/// objects asked for in the order `choice`'s bits from the second give (by
/// number, from the last, from the middle out, or the objects in object
/// streams first), every third read unkept first, which gives the object
/// and record it's then kept as; then read whole, it's `whole`'s, when
/// neither reached the budget.
fn on_demand(file: &[u8], passwords: &[&[u8]], section: Option<usize>, whole: &Whole, choice: u16) {
    let mut options = options();
    options.section = section;
    let reading = Reading::in_memory(file, passwords, &options, true, None);
    let mut ids: Vec<ObjectId> = reading.ids().collect();
    match (choice >> 1) & 3 {
        0 => {}
        1 => ids.reverse(),
        2 => {
            let half = ids.len() / 2;
            ids.rotate_left(half);
        }
        _ => {
            let members: Vec<u32> = (reading.entries())
                .filter(|(_, _, entry)| matches!(entry, XrefEntry::Compressed { .. }))
                .map(|(n, _, _)| n)
                .collect();
            ids.sort_by_key(|id| members.binary_search(&id.0).is_err());
        }
    }
    for (i, &id) in ids.iter().enumerate() {
        let unkept = (i % 3 == 0).then(|| reading.object_unkept(id));
        let got = reading.get(id);
        if let Some(unkept) = unkept {
            match (unkept, got) {
                (Ok((object, record)), Ok(kept)) => {
                    assert_eq!(object, *kept, "{id:?} read unkept is the object kept");
                    assert_eq!(Some(&record), reading.record(id), "{id:?}'s record");
                }
                (Err(unkept), Err(kept)) => {
                    assert_eq!(unkept.to_string(), kept.to_string(), "{id:?}'s failure");
                }
                (unkept, got) => panic!("{id:?} read unkept as {unkept:?}, and kept as {got:?}"),
            }
        }
    }
    let lazily = reading.whole(None);
    if !whole.reached && !lazily.reached {
        assert_eq!(
            lazily.document, whole.document,
            "a reading on demand is the whole read"
        );
    }
}

/// Checks that `doc`, pruned, reaches what it reached from its trailer, in
/// the same order, every id it keeps among it and none it took; and that
/// two objects added to it, or to it pruned, are each at a number no entry
/// held, neither 0, and not the same.
fn pruned(doc: &Document) {
    let reached = doc.reachable();
    let mut pruned = doc.clone();
    let gone = pruned.prune();
    assert_eq!(
        pruned.reachable(),
        reached,
        "what's reached is the same pruned"
    );
    let reached: HashSet<ObjectId> = reached.into_iter().collect();
    let ids = pruned.ids();
    assert!(
        ids.iter().all(|id| reached.contains(id)),
        "each id kept is reached"
    );
    assert_eq!(
        ids.len() + gone.len(),
        doc.ids().len(),
        "each id kept or taken"
    );
    assert!(
        gone.iter().all(|id| !reached.contains(id)),
        "each id taken unreached"
    );
    for mut doc in [doc.clone(), pruned] {
        let held: HashSet<u32> = doc.ids().into_iter().map(|(n, _)| n).collect();
        let (one, two) = (doc.add(Object::null()), doc.add(Object::null()));
        for id in [one, two] {
            assert!(
                id.0 != 0 && id.1 == 0 && !held.contains(&id.0),
                "{id:?} added"
            );
        }
        assert_ne!(one, two, "two objects added at one id");
    }
}

/// Checks that every place `doc` gives is inside what was read, the file
/// or the decoded data of the object stream it names: each object's, and
/// each value's and key's in it, each stream's data's, the trailer's, and
/// each warning's.
fn within(file: &[u8], doc: &Document) {
    let inside = |at: Span, of: Option<ObjectId>| {
        let len = match at.stream {
            None => Some(file.len()),
            Some(stream) => doc.decoded.get(&stream).map(Vec::len),
        };
        assert!(
            len.is_some_and(|len| at.start <= at.end && at.end <= len),
            "{at:?}, in {of:?}, is out of what was read"
        );
    };
    let objects = doc.objects().map(|(id, o)| (Some(id), o));
    let trailer = (doc.trailer.stream, &doc.trailer.object);
    for (of, object) in objects.chain([trailer]) {
        let mut stack = vec![object];
        while let Some(o) = stack.pop() {
            if let Some(at) = o.at {
                inside(at, of);
            }
            let dict = match o.value() {
                Value::Array(a) => {
                    stack.extend(a);
                    continue;
                }
                Value::Dict(d) => d,
                Value::Stream(s) => {
                    if let Some(at) = s.data.at() {
                        inside(at, of);
                    }
                    &s.dict
                }
                _ => continue,
            };
            for entry in dict.entries() {
                if let Some(at) = entry.key_at {
                    inside(at, of);
                }
                stack.push(&entry.value);
            }
        }
    }
    for warning in &doc.warnings {
        if let Some(at) = warning.at {
            inside(at, warning.object);
        }
    }
}

#[cfg(test)]
mod tests {
    use pdf_crypt::writer::{Kind, Settings};
    use pdf_objects::dict;
    use pdf_objects::object::{Dict, Object};

    use super::*;
    use crate::bytes;
    use crate::source::{Encryption, Source};
    use crate::write::{self, Body, Xref};

    /// A small, fast generator of bytes, the same each run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
        }
    }

    fn numbers(n: &[i64]) -> Object {
        n.iter()
            .map(|&n| Object::from(n))
            .collect::<Vec<_>>()
            .into()
    }

    fn root() -> Dict {
        dict! { "Root" => Object::reference(1) }
    }

    /// A catalog, its page tree, a page with an annotation, and the
    /// annotation's appearance stream.
    fn objects() -> Vec<(u32, Body)> {
        vec![
            (
                1,
                dict! {
                    "Type" => "Catalog",
                    "Pages" => Object::reference(2),
                    "Lang" => Object::string("en"),
                }
                .into(),
            ),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![Object::reference(3)], "Count" => 1 }
                    .into(),
            ),
            (
                3,
                dict! {
                    "Type" => "Page",
                    "Parent" => Object::reference(2),
                    "MediaBox" => numbers(&[0, 0, 612, 792]),
                    "Annots" => vec![Object::reference(4)],
                }
                .into(),
            ),
            (
                4,
                dict! {
                    "Type" => "Annot",
                    "Subtype" => "Text",
                    "Rect" => numbers(&[10, 10, 30, 30]),
                    "Contents" => Object::string("A note"),
                    "AP" => dict! { "N" => Object::reference(5) },
                }
                .into(),
            ),
            (
                5,
                Body::stream(
                    dict! {
                        "Type" => "XObject",
                        "Subtype" => "Form",
                        "BBox" => numbers(&[0, 0, 20, 20]),
                    },
                    b"0 0 m 20 20 l S",
                ),
            ),
        ]
    }

    /// [`objects`] with a table, then an incremental update that rewrites
    /// the page with a second annotation, and adds it.
    fn updated() -> Vec<u8> {
        let mut file = write::file("1.7", &objects(), root(), Xref::Table);
        let text = String::from_utf8_lossy(&file);
        let previous: usize = (text.rsplit("startxref\n").next())
            .and_then(|tail| tail.lines().next()?.parse().ok())
            .expect("the writer's startxref");
        let page = file.len();
        file.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Annots [4 0 R 6 0 R] >>\nendobj\n",
        );
        let six = file.len();
        file.extend_from_slice(
            b"6 0 obj\n<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] /IC [1 0 0] >>\nendobj\n",
        );
        let xref = file.len();
        file.extend_from_slice(
            format!(
                "xref\n3 1\n{page:010} 00000 n \n6 1\n{six:010} 00000 n \n\
                 trailer\n<< /Size 7 /Root 1 0 R /Prev {previous} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        file
    }

    /// A file whose page tree and page are in object stream 4, compressed,
    /// which cross-reference stream 5 lists: objects read from, and placed
    /// in, the stream's decoded data.
    fn in_an_object_stream() -> Vec<u8> {
        let pages = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents (A page) >>";
        let header = format!("2 0 3 {} ", pages.len() + 1);
        let data = write::zlib(format!("{header}{pages} {page}").as_bytes());
        let mut file = b"%PDF-1.7\n".to_vec();
        let one = file.len();
        file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let four = file.len();
        let dict = format!(
            "<< /Type /ObjStm /N 2 /First {} /Filter /FlateDecode /Length {} >>",
            header.len(),
            data.len()
        );
        file.extend_from_slice(format!("4 0 obj\n{dict}\nstream\n").as_bytes());
        file.extend_from_slice(&data);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let five = file.len();
        let row = |kind: u8, field: usize, index: u8| {
            let [high, low] = u16::try_from(field).unwrap_or(0).to_be_bytes();
            [kind, high, low, index]
        };
        let rows = [
            row(0, 0, 255),
            row(1, one, 0),
            row(2, 4, 0),
            row(2, 4, 1),
            row(1, four, 0),
            row(1, five, 0),
        ]
        .concat();
        let dict = format!(
            "<< /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R /Length {} >>",
            rows.len()
        );
        file.extend_from_slice(format!("5 0 obj\n{dict}\nstream\n").as_bytes());
        file.extend_from_slice(&rows);
        file.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes(),
        );
        file
    }

    /// Files to mutate: [`objects`] written with a table and with a
    /// cross-reference stream, and encrypted by each kind of pdf-crypt's
    /// writer, the user's password empty and the owner's `owner`; the
    /// update; and the object stream.
    fn files() -> Vec<Vec<u8>> {
        let mut files = vec![
            write::file("1.7", &objects(), root(), Xref::Table),
            write::file("1.7", &objects(), root(), Xref::Stream),
        ];
        let kinds = [
            Kind::Rc4By40,
            Kind::Rc4By128,
            Kind::Aes128,
            Kind::Aes256R5,
            Kind::Aes256,
        ];
        for kind in kinds {
            let settings = Settings {
                kind,
                user: b"",
                owner: OWNER,
                p: -4,
                id: b"0123456789abcdef",
                encrypt_metadata: true,
            };
            files.push(write::encrypted(&settings, &objects(), root()));
        }
        files.push(updated());
        files.push(in_an_object_stream());
        files
    }

    /// The fuzz target's checks on files the tests' writer makes, each
    /// mutated many times as a fuzzer might: bytes changed, cut, repeated,
    /// and moved, each file read by the empty password and, in turn, by
    /// the owner's too.
    #[test]
    fn mutated_files_read_within_their_bytes_the_same_twice() {
        // As written, each reads without a warning, an encrypted one opens
        // by either password, and the update and the object stream are
        // read as one.
        let files = files();
        let mut encrypted = 0;
        for file in &files {
            let doc = bytes::read(file);
            assert!(doc.trailer.unwritten.is_none(), "{:?}", doc.trailer);
            assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
            if doc.encryption != Encryption::None {
                encrypted += 1;
                assert_eq!(doc.encryption, Encryption::Opened { owner: false });
                let opened = bytes::read_opened(file, &[OWNER]).encryption;
                assert_eq!(opened, Encryption::Opened { owner: true });
            }
            read_file(&[&[1, 0][..], file].concat());
        }
        assert_eq!(encrypted, 5, "a file of each kind");
        let doc = bytes::read(&updated());
        assert!(doc.update.is_some(), "the update is one");
        let doc = bytes::read(&in_an_object_stream());
        let at = doc.get((3, 0)).map(|o| o.at.and_then(|at| at.stream));
        assert_eq!(at, Ok(Some((4, 0))), "the page is in the object stream");

        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        let fragments: &[&[u8]] = &[
            b"<<",
            b">>",
            b"[",
            b"]",
            b"(",
            b")",
            b"/",
            b" ",
            b"\n",
            b"\r",
            b"0",
            b"9",
            b"-1",
            b"R",
            b"obj",
            b"endobj",
            b"stream",
            b"endstream",
            b"null",
            b"/Kids",
            b"/Parent",
            b"/Type",
            b"/Subtype",
            b"/Length",
            b"/Prev",
            b"1 0 R",
            b"%%EOF",
            b"xref",
            b"trailer",
            b"startxref",
            b"\xff",
        ];
        for n in 0..2000 {
            // Each file in turn, the choice's lowest bit set every other
            // round: by the empty password alone, then the owner's too.
            let choice = u16::try_from(n / files.len()).unwrap_or(0);
            let mut file = [&choice.to_le_bytes()[..], &files[n % files.len()]].concat();
            for _ in 0..1 + random.below(6) {
                let at = 2 + random.below(file.len() - 2);
                match random.below(5) {
                    0 => file[at] = random.next() as u8,
                    1 => {
                        let end = (at + random.below(16)).min(file.len());
                        file.drain(at..end);
                    }
                    2 => {
                        let fragment = fragments[random.below(fragments.len())];
                        file.splice(at..at, fragment.iter().copied());
                    }
                    3 => {
                        let end = (at + random.below(64)).min(file.len());
                        let piece = file[at..end].to_vec();
                        file.splice(at..at, piece);
                    }
                    _ => {
                        let to = 2 + random.below(file.len() - 2);
                        let end = (at + random.below(32)).min(file.len());
                        let piece: Vec<u8> = file.drain(at..end).collect();
                        let to = to.min(file.len());
                        file.splice(to..to, piece);
                    }
                }
                if file.len() < 3 {
                    break;
                }
            }
            read_file(&file);
        }
    }
}
