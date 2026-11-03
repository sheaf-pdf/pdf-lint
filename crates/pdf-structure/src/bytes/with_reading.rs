//! A file read whole with its reading kept beside it ([`WithReading`]): a
//! document that hands out what a document doesn't hold, its streams' data.

use std::fmt;

use pdf_crypt::Password;
use pdf_objects::object::{Object, ObjectId};
use pdf_objects::version::Version;

use super::{ByteSource, Error, Options, Reading};
use crate::source::{Document, Facts, Record, Revision, Source, Trailer, Unread, Warning};

/// A file read whole, with its reading kept beside it: a [`Document`]
/// ([`WithReading::document`]) that hands out what a document doesn't hold.
/// Made by [`WithReading::new`], which opens the file as
/// [`Reading::open_with`] opens it and makes its document at once
/// ([`Reading::to_document`]), so that the document is the whole read,
/// [`read_opened`](super::read_opened)'s of the same bytes, whatever is
/// asked of the reading after.
///
/// As a [`Source`] it answers as its document does, in every answer but
/// one, which a document alone doesn't give: a stream's data decoded
/// ([`Source::decoded_within`]), by the reading
/// ([`Reading::decoded_within`]), decrypted in an encrypted file it opened
/// and decoded through the stream's filters, at most the limit asked for or
/// [`Limits::stream`](super::Limits::stream), whichever is less. What's
/// decoded isn't kept, nor counted against the reading's budget; the stream
/// itself is kept by the reading, as [`Reading::get`] keeps an object,
/// beside the document's copy of it. It isn't its document: what takes a
/// `&Document` is given that, which gives no stream's data.
///
/// ```
/// use pdf_structure::Source;
/// use pdf_structure::bytes::{Options, WithReading, read_opened};
///
/// let file = b"%PDF-1.7\n1 0 obj\n<< /Length 6 >>\nstream\nabcdef\nendstream\nendobj\n\
///     trailer\n<< /Size 2 >>\n%%EOF\n";
/// let read = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
/// assert_eq!(*read.document(), read_opened(file, &[]));
/// assert_eq!(read.decoded_within((1, 0), 6).unwrap().unwrap(), b"abcdef");
/// // Its document alone gives none.
/// assert!(read.document().decoded_within((1, 0), 6).is_none());
/// ```
pub struct WithReading<S> {
    reading: Reading<S>,
    document: Document,
}

impl<S> fmt::Debug for WithReading<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WithReading")
            .field("reading", &self.reading)
            .finish_non_exhaustive()
    }
}

impl<S: ByteSource> WithReading<S> {
    /// The file `source` holds, opened by `passwords` under `options`, as
    /// [`Reading::open_with`] opens it, and read whole beside its reading, as
    /// [`Reading::to_document`] reads it. It fails only when the source
    /// doesn't give the bytes asked of it ([`Error::Io`]): in memory, never.
    pub fn new(
        source: S,
        passwords: &[Password],
        options: &Options,
    ) -> Result<WithReading<S>, Error> {
        let reading = Reading::open_with(source, passwords, options)?;
        let document = reading.to_document()?;
        Ok(WithReading { reading, document })
    }

    /// The reading kept beside the document, which hands out what the
    /// document doesn't hold: a stream's data, and the bytes at a span.
    pub fn reading(&self) -> &Reading<S> {
        &self.reading
    }

    /// The document read whole, which gives no stream's data.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The document, the reading dropped.
    pub fn into_document(self) -> Document {
        self.document
    }
}

/// Its document's answers, and a stream's data decoded by its reading.
impl<S: ByteSource> Source for WithReading<S> {
    fn ids(&self) -> Vec<ObjectId> {
        Source::ids(&self.document)
    }

    fn get(&self, id: ObjectId) -> Result<&Object, Unread> {
        Source::get(&self.document, id)
    }

    fn trailer(&self) -> &Trailer {
        Source::trailer(&self.document)
    }

    fn header_version(&self) -> Option<Version> {
        Source::header_version(&self.document)
    }

    fn warnings(&self) -> &[Warning] {
        Source::warnings(&self.document)
    }

    fn size(&self) -> Option<usize> {
        Source::size(&self.document)
    }

    fn metadata(&self) -> Option<&[u8]> {
        Source::metadata(&self.document)
    }

    fn record(&self, id: ObjectId) -> Option<&Record> {
        Source::record(&self.document, id)
    }

    fn decoded(&self, stream: ObjectId) -> Option<&[u8]> {
        Source::decoded(&self.document, stream)
    }

    fn revisions(&self) -> &[Revision] {
        Source::revisions(&self.document)
    }

    fn facts(&self) -> Option<&Facts> {
        Source::facts(&self.document)
    }

    fn in_the_clear(&self, stream: ObjectId) -> bool {
        Source::in_the_clear(&self.document, stream)
    }

    fn decoded_within(&self, id: ObjectId, limit: usize) -> Option<Result<Vec<u8>, Error>> {
        Some(self.reading.decoded_within(id, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::super::lazy::files;
    use super::super::{STREAM, read_opened};
    use super::*;

    /// What a source answers of each object it lists, and of one it
    /// doesn't, but a stream's data decoded, which only a reading gives.
    #[derive(Debug, PartialEq)]
    struct Answers<'s> {
        ids: Vec<ObjectId>,
        objects: Vec<Result<&'s Object, Unread>>,
        trailer: &'s Trailer,
        header: Option<Version>,
        warnings: &'s [Warning],
        size: Option<usize>,
        metadata: Option<&'s [u8]>,
        records: Vec<Option<&'s Record>>,
        decoded: Vec<Option<&'s [u8]>>,
        revisions: &'s [Revision],
        facts: Option<&'s Facts>,
        clear: Vec<bool>,
    }

    fn answers(source: &dyn Source) -> Answers<'_> {
        let ids = source.ids();
        let asked: Vec<ObjectId> = ids.iter().copied().chain([(u32::MAX, 0)]).collect();
        Answers {
            objects: asked.iter().map(|&id| source.get(id)).collect(),
            trailer: source.trailer(),
            header: source.header_version(),
            warnings: source.warnings(),
            size: source.size(),
            metadata: source.metadata(),
            records: asked.iter().map(|&id| source.record(id)).collect(),
            decoded: asked.iter().map(|&id| source.decoded(id)).collect(),
            revisions: source.revisions(),
            facts: source.facts(),
            clear: asked.iter().map(|&id| source.in_the_clear(id)).collect(),
            ids,
        }
    }

    /// A stream's data decoded, or what's wrong, as text, as an error's
    /// I/O kind can't be compared.
    fn shown(decoded: Result<Vec<u8>, Error>) -> Result<Vec<u8>, String> {
        decoded.map_err(|error| format!("{error:?}"))
    }

    /// Each test file, the fixtures qpdf wrote among them, opened: its
    /// document is the whole read, as [`read_opened`] reads it, and taken,
    /// it's the same; as a source, every answer is its document's; and each
    /// object's data decoded, at the stream's limit and cut short of it, is
    /// what a reading of the file gives, where the document gives none.
    #[test]
    fn an_opened_file_answers_as_its_document_and_decodes_as_its_reading() {
        let mut streams = 0;
        for (name, file, passwords) in files() {
            let opened = WithReading::new(&file[..], &passwords, &Options::DEFAULT).unwrap();
            let whole = read_opened(&file, &passwords);
            assert_eq!(*opened.document(), whole, "{name}");
            assert_eq!(answers(&opened), answers(opened.document()), "{name}");
            let reading = Reading::open_with(&file[..], &passwords, &Options::DEFAULT).unwrap();
            let alone = opened.document();
            for id in Source::ids(&opened) {
                assert!(alone.decoded_within(id, STREAM).is_none(), "{name}");
                for limit in [STREAM, 1] {
                    let decoded = opened.decoded_within(id, limit).map(shown);
                    let read = shown(reading.decoded_within(id, limit));
                    assert_eq!(decoded, Some(read), "{name}: {id:?} within {limit}");
                }
                streams += usize::from(matches!(
                    opened.get(id).map(Object::value),
                    Ok(pdf_objects::object::Value::Stream(_))
                ));
            }
            assert_eq!(opened.into_document(), whole, "{name}");
        }
        assert!(streams > 50, "{streams}");
    }
}
