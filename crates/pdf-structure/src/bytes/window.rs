//! How the reader is given a file's bytes: in memory, every byte at once;
//! read by position, a window at a time from where a read starts, the
//! window doubled while what's read reaches its end where more of the file
//! could change it, until it reaches the file's end.

use std::borrow::Cow;
use std::cell::Cell;
use std::convert::Infallible;
use std::io;
use std::ops::Range;

use pdf_objects::parse::{self, Parser};
use pdf_tokenizer::{Token, Tokenizer};

use super::{ByteSource, Cx, row_shape, significant};

/// The file a reading reads, as each read is given it: in memory
/// ([`InMemory`]), which can't fail; or read by position, a window at a
/// time ([`ByPosition`]), which fails where its source does.
pub(super) trait Bytes<'a>: Copy {
    /// What stops a read: nothing in memory, the source's error by
    /// position.
    type Error;

    /// How many bytes the file has.
    fn len(self) -> usize;

    /// Every byte, when they're in memory: for a decoder, which only a
    /// reading in memory is given.
    fn memory(self) -> Option<&'a [u8]>;

    /// Whether an object's first window reaches the next offset the
    /// cross-references list.
    fn bounded(self) -> bool;

    /// The bytes in `range`, or none when it isn't within the file.
    fn get(self, range: Range<usize>) -> Result<Option<Cow<'a, [u8]>>, Self::Error>;

    /// Every byte, for a read that needs the whole file: the scan.
    fn all(self) -> Result<Cow<'a, [u8]>, Self::Error>;

    /// `read` of the file from byte `start`: in memory, given the whole
    /// file; by position, a window from `start` of at least `min` bytes and
    /// the source's first window, doubled while `read` touches its end
    /// ([`Window::touch`]), until it reaches the file's end. What `read`
    /// warned of in `cx` in a window it touched the end of is dropped.
    fn read_from<T>(
        self,
        cx: &mut Cx,
        start: usize,
        min: usize,
        read: impl FnMut(&Window<'_>, &mut Cx) -> Result<T, Self::Error>,
    ) -> Result<T, Self::Error>;

    /// `read` of the file's last bytes: in memory, given the whole file; by
    /// position, a window that ends the file, of the source's first
    /// window's bytes, doubled back while `read` touches it, needing what's
    /// before it, until it starts the file.
    fn read_back<T>(self, read: impl FnMut(&Window<'_>) -> T) -> Result<T, Self::Error>;
}

/// A file in memory, read whole by every read, at base 0.
#[derive(Clone, Copy)]
pub(super) struct InMemory<'a>(pub(super) &'a [u8]);

impl<'a> Bytes<'a> for InMemory<'a> {
    type Error = Infallible;

    fn len(self) -> usize {
        self.0.len()
    }

    fn memory(self) -> Option<&'a [u8]> {
        Some(self.0)
    }

    fn bounded(self) -> bool {
        false
    }

    fn get(self, range: Range<usize>) -> Result<Option<Cow<'a, [u8]>>, Infallible> {
        Ok(self.0.get(range).map(Cow::Borrowed))
    }

    fn all(self) -> Result<Cow<'a, [u8]>, Infallible> {
        Ok(Cow::Borrowed(self.0))
    }

    fn read_from<T>(
        self,
        cx: &mut Cx,
        _: usize,
        _: usize,
        mut read: impl FnMut(&Window<'_>, &mut Cx) -> Result<T, Infallible>,
    ) -> Result<T, Infallible> {
        read(&Window::new(self.0, 0, true), cx)
    }

    fn read_back<T>(self, mut read: impl FnMut(&Window<'_>) -> T) -> Result<T, Infallible> {
        Ok(read(&Window::new(self.0, 0, true)))
    }
}

/// A file a source holds, read by position.
#[derive(Clone, Copy)]
pub(super) struct ByPosition<'a> {
    source: &'a dyn ByteSource,
    /// The source's first window, at least a byte.
    first: usize,
    /// Whether an object's first window reaches the next listed offset.
    bounded: bool,
}

impl<'a> ByPosition<'a> {
    /// `source`, its objects' first windows reaching the next listed
    /// offset when `bounded`.
    pub(super) fn new(source: &'a dyn ByteSource, bounded: bool) -> ByPosition<'a> {
        ByPosition {
            source,
            first: source.first_window().max(1),
            bounded,
        }
    }

    /// The bytes in `range`, within the file: as many as it holds, or the
    /// source's error, or an error when it gives another number of them.
    fn read(self, range: Range<usize>) -> io::Result<Cow<'a, [u8]>> {
        let asked = range.len();
        let bytes = self.source.read_at(range)?;
        if bytes.len() != asked {
            let message = format!("the source gave {} bytes of {asked} asked", bytes.len());
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, message));
        }
        Ok(bytes)
    }
}

impl<'a> Bytes<'a> for ByPosition<'a> {
    type Error = io::Error;

    fn len(self) -> usize {
        self.source.len()
    }

    fn memory(self) -> Option<&'a [u8]> {
        None
    }

    fn bounded(self) -> bool {
        self.bounded
    }

    fn get(self, range: Range<usize>) -> io::Result<Option<Cow<'a, [u8]>>> {
        match range.start <= range.end && range.end <= self.len() {
            true => self.read(range).map(Some),
            false => Ok(None),
        }
    }

    fn all(self) -> io::Result<Cow<'a, [u8]>> {
        self.read(0..self.len())
    }

    fn read_from<T>(
        self,
        cx: &mut Cx,
        start: usize,
        min: usize,
        mut read: impl FnMut(&Window<'_>, &mut Cx) -> io::Result<T>,
    ) -> io::Result<T> {
        let len = self.len();
        // Past the file's end there's nothing to read, and what's read
        // there is placed where it was asked for, as in memory.
        if start >= len {
            return read(&Window::new(&[], start, true), cx);
        }
        let mut size = self.first.max(min);
        loop {
            let end = start.saturating_add(size).min(len);
            let last = end == len;
            let bytes = self.read(start..end)?;
            let window = Window::new(&bytes, start, last);
            let said = cx.warnings.len();
            let read = read(&window, cx)?;
            if last || !window.touched.get() {
                return Ok(read);
            }
            cx.warnings.truncate(said);
            size = size.saturating_mul(2);
        }
    }

    fn read_back<T>(self, mut read: impl FnMut(&Window<'_>) -> T) -> io::Result<T> {
        let len = self.len();
        let mut size = self.first;
        loop {
            let start = len.saturating_sub(size);
            let bytes = self.read(start..len)?;
            let window = Window::new(&bytes, start, true);
            let read = read(&window);
            if start == 0 || !window.touched.get() {
                return Ok(read);
            }
            size = size.saturating_mul(2);
        }
    }
}

/// Bytes of the file a read is given: in memory, the whole file; read by
/// position, a window of it from `base`, which ends the file when `last`.
/// Its tokens and offsets are the window's, from `base`. A read whose
/// token runs to the window's end, or that finds none where it looks for
/// one, touches it ([`Window::touch`]): then, in a window that doesn't end
/// the file, more of the file could change what was read, and it's read
/// again in a larger one. A window read back from the file's end
/// ([`Bytes::read_back`]) is touched where what's read needs what's
/// before it.
pub(super) struct Window<'w> {
    /// Its bytes.
    pub(super) bytes: &'w [u8],
    /// Where they start in the file.
    pub(super) base: usize,
    /// Whether they end the file.
    pub(super) last: bool,
    touched: Cell<bool>,
}

impl<'w> Window<'w> {
    fn new(bytes: &'w [u8], base: usize, last: bool) -> Window<'w> {
        Window {
            bytes,
            base,
            last,
            touched: Cell::new(false),
        }
    }

    /// Says that what was read reached the window's end, where more of the
    /// file could change it: in a window that doesn't end the file, it's
    /// read again in a larger one.
    pub(super) fn touch(&self) {
        self.touched.set(true);
    }

    /// Byte `i` of the window, in the file.
    pub(super) fn place(&self, i: usize) -> usize {
        self.base.saturating_add(i)
    }

    /// The window's tokens from byte `offset` of the file.
    pub(super) fn tokenizer(&self, offset: usize) -> Tokenizer<'w> {
        Tokenizer::at(self.bytes, offset - self.base)
    }

    /// A parser of the window from byte `offset` of the file, its spans
    /// and errors the file's.
    pub(super) fn parser(&self, offset: usize) -> Parser<'w> {
        Parser::at(self.bytes, offset - self.base).window(self.base, self.last)
    }

    /// The next token of `t` that isn't white-space or a comment, the window
    /// touched when it runs to the window's end or there's none.
    pub(super) fn significant(&self, t: &mut Tokenizer<'w>) -> Option<Token<'w>> {
        let token = significant(t);
        let end = token.as_ref().map(Token::end);
        if end.is_none_or(|end| end == self.bytes.len()) {
            self.touch();
        }
        token
    }

    /// Whether the cross-reference table's row at byte `row` of the file is
    /// written as ISO 32000-2 7.5.4 writes one ([`row_shape`]), the window
    /// touched when the row's 20 bytes run past its end.
    pub(super) fn row_shape(&self, row: usize) -> Result<(), &'static str> {
        let row = row - self.base;
        if row.saturating_add(20) > self.bytes.len() {
            self.touch();
        }
        row_shape(self.bytes, row)
    }

    /// What a parser's call gave, the window touched when it's short of
    /// the file's end ([`parse::Error::Short`]).
    pub(super) fn parsed<T>(&self, read: Result<T, parse::Error>) -> Result<T, parse::Error> {
        if let Err(parse::Error::Short { .. }) = read {
            self.touch();
        }
        read
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::io;
    use std::ops::Range;
    use std::sync::Mutex;

    use pdf_objects::object::ObjectId;

    use super::super::lazy::{files, length_by_reference};
    use super::super::tests::{clean, compressed, objects_2_and_3, root, xref_row};
    use super::super::*;
    use crate::fuzz::{Counted, open};
    use crate::source::Source;
    use crate::write::{self, Xref};

    /// The windows the tests read in: each of 1 to 64 bytes, and 4096.
    fn windows() -> impl Iterator<Item = usize> {
        (1..=64).chain([4096])
    }

    /// Every test file, and the revision before an update, read by position
    /// in windows of each size the tests read in, an object's first window
    /// reaching the next listed offset and not, is the whole read; and so
    /// is a reading by position read on demand from the last object back.
    #[test]
    fn a_file_read_by_position_in_any_window_is_the_whole_read() {
        let mut compared = 0;
        for (name, file, passwords) in files() {
            let mut sections = vec![None];
            if let Some(update) = read_opened(&file, &passwords).update {
                sections.push(Some(update.previous));
            }
            for section in sections {
                let mut options = Options::DEFAULT;
                options.section = section;
                let whole = Reading::open(&file[..], &passwords, &options)
                    .unwrap()
                    .into_document()
                    .unwrap();
                for first in windows() {
                    for bound in [true, false] {
                        let source = Counted::new(&file, first);
                        let reading = open(&source, &passwords, &options, bound).unwrap();
                        let ids: Vec<ObjectId> = reading.ids().collect();
                        if first % 8 == 1 {
                            ids.iter().rev().for_each(|&id| drop(reading.get(id)));
                        }
                        let doc = reading.into_document().unwrap();
                        let what = format!("{name} as of {section:?}, {first} bytes, {bound}");
                        assert_eq!(doc, whole, "{what}");
                        compared += 1;
                    }
                }
            }
        }
        assert!(compared > 5000, "{compared}");
    }

    /// A file of a catalog, its page tree and, after them, an 8 MiB stream,
    /// then its cross-reference table: opened by position, it reads little
    /// of the file, a window at its head for the header and the
    /// linearization, two at its tail for `startxref` and `%%EOF`, and one
    /// at its table; and its catalog, little more.
    #[test]
    fn opening_by_position_reads_little_of_a_large_file() {
        let data = vec![b'x'; 8 << 20];
        let stream = [
            format!("<< /Length {} >>\nstream\n", data.len()).as_bytes(),
            &data,
            b"\nendstream",
        ]
        .concat();
        let file = write::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog /Pages 2 0 R >>"[..]),
                (2, b"<< /Type /Pages /Kids [] /Count 0 >>"),
                (3, &stream),
            ],
            "<< /Size 4 /Root 1 0 R >>",
        );
        let source = Counted::new(&file, 4096);
        let reading = Reading::open(&source, &[], &Options::DEFAULT).unwrap();
        assert!(reading.trailer().unwritten.is_none());
        let opened = source.read();
        assert!(opened <= 5 * 4096, "{opened} bytes read to open it");
        assert!(reading.get((1, 0)).is_ok());
        let read = source.read();
        assert!(
            read < opened + (8 << 10),
            "{read} bytes read to read its catalog"
        );
        assert!(reading.get((3, 0)).is_ok());
        assert!(source.read() > 8 << 20);
        assert_eq!(reading.into_document().unwrap(), read_opened(&file, &[]));
    }

    /// Bytes in memory read by position, which fail the next read that
    /// starts at a chosen byte, once, and record where each read starts.
    struct Failing<'a> {
        bytes: &'a [u8],
        first: usize,
        fail: Mutex<Option<usize>>,
        starts: Mutex<Vec<usize>>,
    }

    impl<'a> Failing<'a> {
        fn new(bytes: &'a [u8], first: usize) -> Failing<'a> {
            Failing {
                bytes,
                first,
                fail: Mutex::new(None),
                starts: Mutex::new(Vec::new()),
            }
        }

        /// Fails the next read that starts at `at`.
        fn fail_at(&self, at: usize) {
            *self.fail.lock().unwrap() = Some(at);
        }

        /// Whether it has failed the read it was to fail.
        fn failed(&self) -> bool {
            self.fail.lock().unwrap().is_none()
        }
    }

    impl ByteSource for Failing<'_> {
        fn len(&self) -> usize {
            self.bytes.len()
        }

        fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
            self.starts.lock().unwrap().push(range.start);
            let mut fail = self.fail.lock().unwrap();
            if *fail == Some(range.start) {
                *fail = None;
                return Err(io::Error::other(format!("no byte {}", range.start)));
            }
            Ok(Cow::Borrowed(&self.bytes[range]))
        }

        fn first_window(&self) -> usize {
            self.first
        }
    }

    /// Whether `error` is the source's, failing a read at `at`.
    fn failed_at(error: Error, at: usize) -> bool {
        matches!(error, Error::Io(e) if e.to_string() == format!("no byte {at}"))
    }

    /// Each read a file's opening makes by position, failed, fails the
    /// opening; opened again, it's the whole read. Each test file, in
    /// windows of 1, 7 and 4096 bytes.
    #[test]
    fn a_read_at_open_that_fails_fails_the_opening_and_keeps_nothing() {
        let mut failed = 0;
        for (name, file, passwords) in files() {
            let whole = read_opened(&file, &passwords);
            for first in [1, 7, 4096] {
                let source = Failing::new(&file, first);
                drop(open(&source, &passwords, &Options::DEFAULT, false).unwrap());
                let mut starts = std::mem::take(&mut *source.starts.lock().unwrap());
                starts.sort_unstable();
                starts.dedup();
                for at in starts {
                    source.fail_at(at);
                    let opened = open(&source, &passwords, &Options::DEFAULT, false);
                    assert!(failed_at(opened.unwrap_err(), at), "{name}, at {at}");
                    let reading = open(&source, &passwords, &Options::DEFAULT, false).unwrap();
                    assert_eq!(reading.into_document().unwrap(), whole, "{name}, at {at}");
                    failed += 1;
                }
            }
        }
        assert!(failed > 500, "{failed}");
    }

    /// `file` opened by position through `source` and read on demand: the
    /// read of object `id` fails when the read at `at` does, keeps nothing
    /// (no warning, no object, no record, no object stream decoded), and
    /// asked again it reads; and the reading is the whole read.
    fn fails_and_reads_again(file: &[u8], id: ObjectId, at: usize) {
        for bound in [true, false] {
            let source = Failing::new(file, 1);
            let reading = open(&source, &[], &Options::DEFAULT, bound).unwrap();
            let warnings = reading.warnings();
            let containers = |reading: &Reading<&Failing>| {
                (reading.reader.containers.values())
                    .filter(|c| c.get().is_some())
                    .count()
            };
            source.fail_at(at);
            let error = reading.get(id).unwrap_err();
            assert!(source.failed(), "{id:?}: no read at {at}");
            assert!(failed_at(error, at), "{id:?}, at {at}");
            assert_eq!(reading.warnings(), warnings, "{id:?}, at {at}");
            assert!(reading.record(id).is_none(), "{id:?}, at {at}");
            assert_eq!(containers(&reading), 0, "{id:?}, at {at}");
            let doc = read(file);
            assert_eq!(reading.get(id).unwrap(), doc.get(id).unwrap());
            assert_eq!(reading.into_document().unwrap(), doc);
        }
    }

    /// An object whose read fails keeps nothing, and reads again.
    #[test]
    fn an_object_whose_read_fails_keeps_nothing_and_reads_again() {
        let file = write::file("1.7", &clean(), root(), Xref::Table);
        let at = find(&file, b"2 0 obj").unwrap();
        fails_and_reads_again(&file, (2, 0), at);
    }

    /// A stream whose `/Length` by reference fails to read keeps nothing,
    /// and reads again.
    #[test]
    fn a_stream_whose_length_by_reference_fails_to_read_keeps_nothing() {
        let file = length_by_reference();
        let at = find(&file, b"4 0 obj").unwrap();
        fails_and_reads_again(&file, (3, 0), at);
    }

    /// An object in an object stream whose `/Filter` is a reference that
    /// fails to read keeps nothing, and its object stream isn't decoded;
    /// it reads again.
    #[test]
    fn an_object_streams_filter_by_reference_that_fails_to_read_keeps_nothing() {
        let file = filter_by_reference();
        let at = find(&file, b"6 0 obj").unwrap();
        fails_and_reads_again(&file, (3, 0), at);
    }

    /// An object in an object stream whose data fails to read keeps
    /// nothing, and its object stream isn't decoded; it reads again.
    #[test]
    fn an_object_streams_data_that_fails_to_read_keeps_nothing() {
        let (file, _) = compressed(false);
        let at = find(&file, b"4 0 obj").unwrap();
        let at = at + find(&file[at..], b"stream\n").unwrap() + b"stream\n".len();
        fails_and_reads_again(&file, (3, 0), at);
    }

    /// [`super::super::tests::filtered`]'s file, its object stream's
    /// `/Filter` a reference to object 6, `/FlateDecode`.
    fn filter_by_reference() -> Vec<u8> {
        let (objects, first) = objects_2_and_3();
        let data = write::zlib(&objects);
        let mut file = b"%PDF-1.7\n".to_vec();
        let one = file.len();
        file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let six = file.len();
        file.extend_from_slice(b"6 0 obj\n/FlateDecode\nendobj\n");
        let four = file.len();
        let dict = format!(
            "<< /Type /ObjStm /N 2 /First {first} /Filter 6 0 R /Length {} >>",
            data.len()
        );
        file.extend_from_slice(format!("4 0 obj\n{dict}\nstream\n").as_bytes());
        file.extend_from_slice(&data);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let five = file.len();
        let rows = [
            xref_row(0, 0, 255),
            xref_row(1, one, 0),
            xref_row(2, 4, 0),
            xref_row(2, 4, 1),
            xref_row(1, four, 0),
            xref_row(1, five, 0),
            xref_row(1, six, 0),
        ]
        .concat();
        let dict = format!(
            "<< /Type /XRef /Size 7 /W [1 2 1] /Root 1 0 R /Length {} >>",
            rows.len()
        );
        file.extend_from_slice(format!("5 0 obj\n{dict}\nstream\n").as_bytes());
        file.extend_from_slice(&rows);
        file.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes(),
        );
        file
    }

    /// The catalog's metadata, whose data fails to read, isn't given, and
    /// asked again it is; and so are the bytes at a span.
    #[test]
    fn metadata_and_bytes_that_fail_to_read_are_given_when_asked_again() {
        let file = write::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog /Metadata 2 0 R >>"[..]),
                (
                    2,
                    b"<< /Type /Metadata /Subtype /XML /Length 5 >>\nstream\nhello\nendstream",
                ),
            ],
            "<< /Size 3 /Root 1 0 R >>",
        );
        let at = find(&file, b"hello").unwrap();
        let source = Failing::new(&file, 4096);
        let reading = Reading::open(&source, &[], &Options::DEFAULT).unwrap();
        source.fail_at(at);
        assert!(failed_at(reading.metadata().unwrap_err(), at));
        assert_eq!(reading.metadata().unwrap().as_deref(), Some(&b"hello"[..]));
        source.fail_at(at);
        assert!(failed_at(reading.bytes(span(at, at + 5)).unwrap_err(), at));
        assert_eq!(reading.bytes(span(at, at + 5)).unwrap(), &b"hello"[..]);
        let doc = reading.into_document().unwrap();
        assert_eq!(doc.metadata.as_deref(), Some(&b"hello"[..]));
        assert_eq!(doc, read(&file));
    }

    /// A stream's data, and its filters' values by reference, read by
    /// position: where a read of them fails, the call fails, and asked
    /// again it reads them; the reading is the whole read.
    #[test]
    fn a_streams_data_that_fails_to_read_is_given_when_asked_again() {
        let data = write::zlib(b"hello");
        let stream = [
            format!("<< /Filter 3 0 R /Length {} >>\nstream\n", data.len()).as_bytes(),
            &data,
            b"\nendstream",
        ]
        .concat();
        let file = write::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog >>"[..]),
                (2, &stream),
                (3, b"/FlateDecode"),
            ],
            "<< /Size 4 /Root 1 0 R >>",
        );
        let at = find(&file, &data).unwrap();
        let filter = find(&file, b"3 0 obj").unwrap();
        for bound in [true, false] {
            let source = Failing::new(&file, 4096);
            let reading = open(&source, &[], &Options::DEFAULT, bound).unwrap();
            source.fail_at(at);
            assert!(failed_at(reading.data((2, 0)).unwrap_err(), at));
            assert_eq!(reading.data((2, 0)).unwrap(), data);
            source.fail_at(at);
            assert!(failed_at(reading.decoded((2, 0)).unwrap_err(), at));
            source.fail_at(filter);
            assert!(failed_at(reading.decoded((2, 0)).unwrap_err(), filter));
            assert!(source.failed());
            assert_eq!(reading.decoded((2, 0)).unwrap(), b"hello");
            assert_eq!(reading.into_document().unwrap(), read(&file));
        }
    }
}
