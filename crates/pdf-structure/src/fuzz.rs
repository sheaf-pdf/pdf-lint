//! What the fuzz target checks, which a unit test checks too: on any
//! bytes, a file is read without a panic, in the same way twice, with
//! every place the reading gives inside what was read; and so are its
//! revision before an incremental update, and the file opened by an
//! owner's password. It's compiled only for tests and under cargo-fuzz's
//! `--cfg fuzzing`, and only reads.

use pdf_objects::object::{ObjectId, Span, Value};

use crate::bytes;
use crate::source::Document;

/// The owner's password the fuzz target opens a file by, when its input
/// says to: the one the unit test's encrypted files are written with.
const OWNER: &[u8] = b"owner";

/// The fuzz target's body: `data`'s first two bytes choose how the file
/// after them is read, and the rest is the file. Read by [`bytes::read`],
/// it's the same twice, and every place the reading gives is inside what
/// was read: each object's, each value's and key's in it, each stream's
/// data's, the trailer's and each warning's, in the file or in the
/// decoded data of the object stream it names, which the reading keeps.
/// When the file has an incremental update, so is the revision before it
/// ([`bytes::read_from`]); and with the choice's lowest bit set, so is the
/// file opened by the owner's password `owner` ([`bytes::read_opened`]),
/// and its revision before an update. Panics on a broken promise.
pub fn read_file(data: &[u8]) {
    let (choice, file) = match data {
        [a, b, file @ ..] => (u16::from_le_bytes([*a, *b]), file),
        _ => (0, data),
    };
    alike(file, |section| match section {
        None => bytes::read(file),
        Some(section) => bytes::read_from(file, section),
    });
    if choice & 1 != 0 {
        let passwords = &[OWNER];
        alike(file, |section| match section {
            None => bytes::read_opened(file, passwords),
            Some(section) => bytes::read_from_opened(file, passwords, section),
        });
    }
}

/// `read` of the whole file (`None`) twice, the same each time, every
/// place it gives inside `file`'s bytes or what it decoded; and when the
/// file has an update, the same of the revision before it (`Some` of where
/// that revision's section starts).
fn alike(file: &[u8], read: impl Fn(Option<usize>) -> Document) {
    let doc = read(None);
    assert_eq!(doc, read(None), "a reading is the same twice");
    within(file, &doc);
    if let Some(update) = &doc.update {
        let previous = read(Some(update.previous));
        assert_eq!(
            previous,
            read(Some(update.previous)),
            "the revision before an update reads the same twice"
        );
        within(file, &previous);
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
    let objects = (doc.objects.iter()).filter_map(|(id, o)| Some((Some(*id), o.as_ref().ok()?)));
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
