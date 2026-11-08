//! A file read whole with its reading kept beside it ([`WithReading`]): a
//! document that hands out what a document doesn't hold, its streams' data.

use std::borrow::Cow;
use std::fmt;

use pdf_objects::fault::Place;
use pdf_objects::object::{Object, ObjectId};
use pdf_objects::version::Version;
use pdf_objects::{Resolve, Streams, Unreadable};

use super::reading::{missing, viewed};
use super::{ByteSource, Error, Options, Reading};
use crate::read::Fault;
use crate::source::{Document, Facts, Record, Revision, Source, Trailer, Unread};

/// A file read whole, with its reading kept beside it: a [`Document`]
/// ([`WithReading::document`]) that hands out what a document doesn't hold.
/// Made by [`WithReading::new`], which opens the file as [`Reading::open`]
/// opens it and makes its document at once ([`Reading::to_document`]), so
/// that the document is the whole read, [`document`](super::document)'s of
/// the same bytes under the same options, whatever is asked of the reading
/// after.
///
/// As a [`Source`] it answers as its document does, in every answer but
/// one, which a document alone doesn't give: a stream's data decoded
/// ([`Source::decoded_within`]), by the reading
/// ([`Reading::decoded_within`]), decrypted in an encrypted file it opened
/// and decoded through the stream's filters, at most the limit asked for or
/// [`Limits::stream`](super::Limits::stream), whichever is less. What's
/// decoded isn't kept, nor counted against the reading's budget; and the
/// stream is the document's, which is what the reading reads at its number,
/// so the reading doesn't read it again, nor keep a copy of it beside the
/// document's. It isn't its document: what takes a `&Document` is given
/// that, which gives no stream's data.
///
/// ```
/// use pdf_structure::Source;
/// use pdf_structure::pdf_objects::ObjectId;
/// use pdf_structure::read::{self, Options, WithReading};
///
/// let file = b"%PDF-1.7\n1 0 obj\n<< /Length 6 >>\nstream\nabcdef\nendstream\nendobj\n\
///     trailer\n<< /Size 2 >>\n%%EOF\n";
/// let read = WithReading::new(&file[..], &Options::DEFAULT).unwrap();
/// assert_eq!(*read.document(), read::document(file, &Options::DEFAULT));
/// assert_eq!(read.decoded_within(ObjectId::new(1, 0), 6).unwrap().unwrap(), b"abcdef");
/// // Its document alone gives none.
/// assert!(read.document().decoded_within(ObjectId::new(1, 0), 6).is_none());
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
    /// The file `source` holds, opened under `options`, by the passwords
    /// they give where it's encrypted, as [`Reading::open`] opens it, and
    /// read whole beside its reading, as [`Reading::to_document`] reads it.
    ///
    /// # Errors
    ///
    /// It fails only when the source doesn't give the bytes asked of it
    /// ([`Error::Io`]): in memory, never.
    pub fn new(source: S, options: &Options<'_>) -> Result<WithReading<S>, Error> {
        let reading = Reading::open(source, options)?;
        let document = reading.to_document()?;
        Ok(WithReading { reading, document })
    }

    /// The reading kept beside the document, which hands out what the
    /// document doesn't hold: a stream's data, and the bytes at a span.
    #[must_use]
    pub const fn reading(&self) -> &Reading<S> {
        &self.reading
    }

    /// The document read whole, which gives no stream's data.
    #[must_use]
    pub const fn document(&self) -> &Document {
        &self.document
    }

    /// The document, the reading dropped.
    #[must_use]
    pub fn into_document(self) -> Document {
        self.document
    }
}

/// Its document's objects, as a reference resolves them ([`Resolve`] for a
/// `&Document`), so that what reads a file through a resolver and its
/// streams, as a font does, reads it through a `&WithReading` alone.
impl<'o, S: ByteSource> Resolve<'o> for &'o WithReading<S> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        let with: &'o WithReading<S> = self;
        (&with.document).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        let with: &'o WithReading<S> = self;
        (&with.document).try_resolve(id)
    }
}

/// A stream's data as a viewer takes it ([`Streams`]), as its reading gives
/// it ([`Reading`]'s), of the stream its document holds, so that the reading
/// doesn't read it again: within the limit asked for or
/// [`Limits::stream`](super::Limits::stream), whichever is less, owned. An
/// object stream's too, decoded anew: what the document holds of one
/// ([`Document::held`]) is what the whole read decoded, under the
/// reading's budget and with what didn't decrypt cleanly kept, which the
/// objects in it are read from, not the stream's data as a reading gives it
/// at the limit asked. Where it gives none, it says why as a reading does,
/// what isn't there or doesn't read as its document says.
impl<S: ByteSource> Streams for WithReading<S> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        match Resolve::try_resolve(&&self.document, id) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(missing(id)),
            Err(unreadable) => return Err(unreadable),
        }
        let object = Source::get(&self.document, id);
        let object = Some(object.as_ref().map(|o| *o));
        let decoded = self.reading.decoded_within_as(id, object, limit);
        viewed(id, decoded, self.reading.capped_at(limit)).map(Cow::Owned)
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

    fn faults(&self) -> Cow<'_, [(Place, Fault)]> {
        Source::faults(&self.document)
    }

    fn size(&self) -> Option<usize> {
        Source::size(&self.document)
    }

    fn metadata(&self) -> Option<Cow<'_, [u8]>> {
        Source::metadata(&self.document)
    }

    fn record(&self, id: ObjectId) -> Option<&Record> {
        Source::record(&self.document, id)
    }

    fn held(&self, stream: ObjectId) -> Option<&[u8]> {
        Source::held(&self.document, stream)
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
        let object = Source::get(&self.document, id);
        let object = Some(object.as_ref().map(|o| *o));
        Some(self.reading.decoded_within_as(id, object, limit))
    }
}

#[cfg(test)]
mod tests {
    use pdf_crypt::Password;
    use pdf_objects::Why;

    use super::super::lazy::files;
    use super::super::{STREAM, document, shown};
    use super::*;

    /// What a source answers of each object it lists, and of one it
    /// doesn't, but a stream's data decoded, which only a reading gives.
    /// Each object is asked for first, so that a reading has read them all
    /// before it's asked the rest.
    #[derive(Debug, PartialEq)]
    struct Answers<'s> {
        ids: Vec<ObjectId>,
        objects: Vec<Result<&'s Object, Unread>>,
        trailer: &'s Trailer,
        header: Option<Version>,
        faults: Cow<'s, [(Place, Fault)]>,
        size: Option<usize>,
        metadata: Option<Cow<'s, [u8]>>,
        records: Vec<Option<&'s Record>>,
        held: Vec<Option<&'s [u8]>>,
        revisions: &'s [Revision],
        facts: Option<&'s Facts>,
        clear: Vec<bool>,
    }

    fn answers(source: &dyn Source) -> Answers<'_> {
        let ids = source.ids();
        let asked: Vec<ObjectId> = ids
            .iter()
            .copied()
            .chain([ObjectId::new(u32::MAX, 0)])
            .collect();
        Answers {
            objects: asked.iter().map(|&id| source.get(id)).collect(),
            trailer: source.trailer(),
            header: source.header_version(),
            faults: source.faults(),
            size: source.size(),
            metadata: source.metadata(),
            records: asked.iter().map(|&id| source.record(id)).collect(),
            held: asked.iter().map(|&id| source.held(id)).collect(),
            revisions: source.revisions(),
            facts: source.facts(),
            clear: asked.iter().map(|&id| source.in_the_clear(id)).collect(),
            ids,
        }
    }

    /// A stream's data as a viewer takes it ([`Streams`]), or the id and
    /// why there's none.
    fn viewed(given: Result<Cow<'_, [u8]>, Unreadable>) -> Result<Vec<u8>, (ObjectId, Why)> {
        given
            .map(Cow::into_owned)
            .map_err(|unreadable| (unreadable.id, unreadable.why))
    }

    /// Each test file, the fixtures qpdf wrote among them, opened: its
    /// document is the whole read, as [`document`] reads it, and taken,
    /// it's the same; as a source, every answer is its document's; and each
    /// object's data decoded, at the stream's limit and cut short of it, is
    /// what a reading of the file gives, where the document gives none. As
    /// a viewer takes it ([`Streams`]), a reading and the file opened give
    /// the same of every object, and its document alone the same of every
    /// object stream it holds decoded whole, and of what isn't a stream; of
    /// any other stream, the document alone gives none, detached. A
    /// reading of the file, as a source, answers as the file opened, once
    /// it has read every object, its faults in the same order; before, it
    /// holds no object stream's data, and has none in the clear, until it
    /// reads an object in it.
    #[test]
    fn an_opened_file_answers_as_its_document_and_decodes_as_its_reading() {
        let (mut streams, mut held, mut lent, mut decoded) = (0, 0, 0, 0);
        for (name, file, passwords) in files() {
            let opened = WithReading::new(
                &file[..],
                &Options {
                    passwords: &passwords,
                    ..Options::DEFAULT
                },
            )
            .unwrap();
            let whole = document(
                &file,
                &Options {
                    passwords: &passwords,
                    ..Options::DEFAULT
                },
            );
            assert_eq!(*opened.document(), whole, "{name}");
            assert_eq!(answers(&opened), answers(opened.document()), "{name}");
            let alone = opened.document();
            let reading = Reading::open(
                &file[..],
                &Options {
                    passwords: &passwords,
                    ..Options::DEFAULT
                },
            )
            .unwrap();
            let containers: Vec<ObjectId> = (alone.decoded.keys())
                .chain(&alone.in_the_clear)
                .copied()
                .collect();
            for &stream in &containers {
                assert_eq!(Source::held(&reading, stream), None, "{name}: {stream}");
                assert!(!Source::in_the_clear(&reading, stream), "{name}: {stream}");
            }
            for (id, object) in alone.objects() {
                let Some(stream) = object.at.and_then(|at| at.stream) else {
                    continue;
                };
                let first = Source::held(&reading, stream).is_none();
                assert!(Source::get(&reading, id).is_ok(), "{name}: {id}");
                let data = alone.decoded.get(&stream).map(Vec::as_slice);
                assert_eq!(Source::held(&reading, stream), data, "{name}: {id}");
                let clear = alone.in_the_clear.contains(&stream);
                assert_eq!(Source::in_the_clear(&reading, stream), clear, "{name}");
                decoded += usize::from(first);
            }
            assert_eq!(answers(&reading), answers(&opened), "{name}");
            let ids = Source::ids(&opened);
            // And what it doesn't list: a number past them all, and one of
            // its objects at another generation.
            let unlisted = [ObjectId::new(u32::MAX, 0)].into_iter();
            let others =
                (ids.first()).map(|&id| ObjectId::new(id.number, id.generation.wrapping_add(1)));
            for id in ids.iter().copied().chain(unlisted).chain(others) {
                assert!(alone.decoded_within(id, STREAM).is_none(), "{name}");
                // Decoding a stream keeps nothing of it in the reading.
                let kept = opened.reading().record(id).is_some();
                let stream = matches!(
                    opened.get(id).map(Object::value),
                    Ok(pdf_objects::object::Value::Stream(_))
                );
                let holds = alone.decoded.contains_key(&id);
                let lends = holds && alone.whole.contains(&id);
                for limit in [STREAM, 1] {
                    let decoded = opened.decoded_within(id, limit).map(shown);
                    let read = shown(reading.decoded_within(id, limit));
                    assert_eq!(decoded, Some(read), "{name}: {id:?} within {limit}");
                    let read = viewed(Streams::decoded(&reading, id, limit));
                    let given = viewed(Streams::decoded(&opened, id, limit));
                    assert_eq!(given, read, "{name}: {id:?} viewed within {limit}");
                    let alone = viewed(Streams::decoded(alone, id, limit));
                    match stream && !lends {
                        true => assert_eq!(alone, Err((id, Why::Detached)), "{name}: {id:?}"),
                        false => assert_eq!(alone, read, "{name}: {id:?} alone within {limit}"),
                    }
                }
                assert_eq!(
                    opened.reading().record(id).is_some(),
                    kept,
                    "{name}: {id:?}"
                );
                streams += usize::from(stream);
                held += usize::from(holds);
                lent += usize::from(lends);
            }
            assert_eq!(opened.into_document(), whole, "{name}");
        }
        // The writer's files, and qpdf's.
        let floor = 50;
        assert!(streams > floor, "{streams}");
        assert!(held > 0, "{held}");
        assert!(lent > 0, "{lent}");
        // Object streams a reading decoded by reading an object in each.
        assert!(decoded > 0, "{decoded}");
    }

    /// An object stream the whole read cut off as it decoded it, at the
    /// stream's limit, at the reading's budget or where a fault stops its
    /// filter, or whose data didn't decrypt cleanly: its document holds
    /// what decoded, which its objects are read from, but it isn't the
    /// stream's data. As a viewer takes it ([`Streams`]), the file opened
    /// gives what its reading gives at every limit, the stream decoded
    /// anew, and its document alone gives none, detached; where the
    /// whole read decoded it whole, the document lends it, as its reading
    /// gives it.
    #[test]
    fn an_object_stream_the_whole_read_cut_off_is_given_as_its_reading_gives_it() {
        let stream = ObjectId::new(4, 0);
        let (mut objects, _) = super::super::tests::objects_2_and_3();
        objects.resize(200, b' ');
        let zlib = pdf_fixtures::fixture::zlib(&objects);
        let flate = " /Filter /FlateDecode";
        let plain = super::super::tests::filtered(false, flate, &zlib);
        let corrupt = super::super::tests::filtered(false, flate, &zlib[..zlib.len() - 20]);
        // Its ciphertext's whole blocks decrypt to the zlib data; a part
        // block past them is left off, which doesn't decrypt cleanly.
        let encrypted = super::super::lazy::encrypted_objects(flate, None);
        let ciphertext = super::super::lazy::encrypted_container(&zlib);
        let part = [&ciphertext[..], b"XYZ"].concat();
        let unclean = super::super::lazy::encrypted_objects(flate, Some(&part));
        let limited = |stream: usize, budget: usize| {
            let mut options = Options::DEFAULT;
            options.limits.stream = stream;
            options.limits.budget = budget;
            options
        };
        let (default, budget) = (
            Options::DEFAULT.limits.stream,
            Options::DEFAULT.limits.budget,
        );
        let (none, user) = (&[][..], &[Password("user")][..]);
        let cases = [
            ("whole", &plain[..], none, Options::DEFAULT, true),
            (
                "at the stream's limit",
                &plain,
                none,
                limited(64, budget),
                false,
            ),
            ("at the budget", &plain, none, limited(default, 150), false),
            (
                "where a fault stops its filter",
                &corrupt,
                none,
                Options::DEFAULT,
                false,
            ),
            ("encrypted, whole", &encrypted, user, Options::DEFAULT, true),
            (
                "not decrypted cleanly",
                &unclean,
                user,
                Options::DEFAULT,
                false,
            ),
        ];
        for (what, file, passwords, options, whole) in cases {
            let opened = WithReading::new(
                file,
                &Options {
                    passwords,
                    ..options
                },
            )
            .unwrap();
            let reading = Reading::open(
                file,
                &Options {
                    passwords,
                    ..options
                },
            )
            .unwrap();
            let alone = opened.document();
            assert!(alone.decoded.contains_key(&stream), "{what}: held");
            assert!(alone.get(ObjectId::new(2, 0)).is_ok(), "{what}: 2 0 R read");
            assert_eq!(alone.whole.contains(&stream), whole, "{what}");
            for limit in [usize::MAX, 1000, 200, 199, 64, 10] {
                let read = viewed(Streams::decoded(&reading, stream, limit));
                let given = viewed(Streams::decoded(&opened, stream, limit));
                assert_eq!(given, read, "{what}: within {limit}");
                let alone = Streams::decoded(alone, stream, limit);
                match whole {
                    true => {
                        let lent = matches!(alone, Ok(Cow::Borrowed(_)) | Err(_));
                        assert!(lent, "{what}: within {limit}");
                        assert_eq!(viewed(alone), read, "{what}: alone within {limit}");
                    }
                    false => {
                        let alone = viewed(alone);
                        assert_eq!(
                            alone,
                            Err((stream, Why::Detached)),
                            "{what}: within {limit}"
                        );
                    }
                }
            }
        }
        // What each reading gives: the stream whole but past the reader's cap, the
        // budget not its, what decoded of the corrupt one, and none of what
        // didn't decrypt cleanly.
        let given = |file: &[u8], passwords: &[Password], options: &Options| {
            let opened = WithReading::new(
                file,
                &Options {
                    passwords,
                    ..*options
                },
            )
            .unwrap();
            viewed(Streams::decoded(&opened, stream, 1000)).map(|data| data.len())
        };
        assert_eq!(given(&plain, &[], &Options::DEFAULT), Ok(200));
        assert_eq!(
            given(&plain, &[], &limited(64, budget)),
            Err((stream, Why::Capped))
        );
        assert_eq!(given(&plain, &[], &limited(default, 150)), Ok(200));
        assert!(given(&corrupt, &[], &Options::DEFAULT).is_ok_and(|n| n > 0 && n < 200));
        assert_eq!(
            given(&unclean, user, &Options::DEFAULT),
            Err((stream, Why::Locked))
        );
    }
}
