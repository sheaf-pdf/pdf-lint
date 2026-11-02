//! A stream's data, which a font reads and [`Resolve`] doesn't
//! give: its `/ToUnicode`, a CMap and those it uses, its CharProcs.

use pdf_objects::{Object, ObjectId, Resolve, Unreadable};

/// What gives a stream's data, decrypted and decoded, for a font to read: what
/// reads a file implements this, a reference to it being one too, and [`Resolve`], and a
/// font is read over the reference. [`With`] makes one of a resolver and a function. With
/// the `structure` feature, a pdf-structure `Reading` is one.
///
/// A caller's cache of what's decoded goes behind this, a hit copied into
/// [`Decoded::Data`], which a font keeps or drops as it likes. A font doesn't trust what
/// it's given: data longer than was asked for is
/// [`Error::TooLarge`](crate::Error::TooLarge).
pub trait Streams {
    /// Stream `id`'s data, at most `limit` bytes of it. `id` is the stream's own
    /// number: a font has followed any reference to it.
    fn decoded(&self, id: ObjectId, limit: usize) -> Decoded;
}

/// A reference to what gives a stream's data gives it, as a font is read over a reference
/// for its objects' lifetime.
impl<S: Streams + ?Sized> Streams for &S {
    fn decoded(&self, id: ObjectId, limit: usize) -> Decoded {
        (**self).decoded(id, limit)
    }
}

/// What [`Streams::decoded`] says of a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decoded {
    /// Its data, or as much of it as decodes: PDFium takes what it can of a
    /// stream that doesn't decode to its end.
    Data(Vec<u8>),
    /// It decodes past the limit.
    TooLarge,
    /// There's no such stream, or it doesn't decode at all.
    None,
    /// Its data doesn't read, for a reason that's the caller's to know
    /// ([`Error::Unreadable`](crate::Error::Unreadable)). A font reads
    /// [`Why::Damaged`](crate::Why::Damaged) here as [`Decoded::None`].
    Unreadable(Unreadable),
}

/// A resolver and a function from a stream's number and a limit to its data, as
/// one, for what reads a file and has its streams some other way.
///
/// ```
/// use pdf_fonts::{Decoded, Font, Limits, Object, ObjectId, With};
///
/// let none = |_: ObjectId| -> Option<&Object> { None };
/// let objects = With::new(&none, |_, _| Decoded::None);
/// let font = pdf_objects::dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" };
/// let mut budget = 1 << 20;
/// let read = Font::read(objects, &font, &Limits::DEFAULT, &mut budget, &mut ()).unwrap();
/// assert_eq!(read.width(u32::from(b'A')), 667.0);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct With<R, F> {
    objects: R,
    streams: F,
}

impl<R, F> With<R, F> {
    /// `objects`, its streams' data by `streams`.
    pub fn new(objects: R, streams: F) -> With<R, F> {
        With { objects, streams }
    }
}

impl<'o, R: Resolve<'o>, F> Resolve<'o> for With<R, F> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        self.objects.try_resolve(id)
    }
}

impl<R, F: Fn(ObjectId, usize) -> Decoded> Streams for With<R, F> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Decoded {
        (self.streams)(id, limit)
    }
}

#[cfg(feature = "structure")]
mod reading {
    use pdf_objects::{ObjectId, Resolve, Unreadable, Why};
    use pdf_structure::bytes::{ByteSource, Error, Reading, Unfiltered};
    use pdf_structure::pdf_filters::Fault;

    use super::{Decoded, Streams};

    /// A stream's data by [`Reading::decoded_within`]. A stream that decodes in
    /// part is what decoded, as PDFium takes it, unless what stopped it is the
    /// limit. A stream past the reading's own
    /// [`Limits::stream`](pdf_structure::bytes::Limits::stream) is
    /// [`Decoded::TooLarge`] too, whatever limit is asked for. A locked file's,
    /// a source that failed and a stream listing more filters than the reader
    /// takes are [`Decoded::Unreadable`]; an object that's not a stream, one
    /// [`Why::Damaged`], and filters the reader doesn't take are
    /// [`Decoded::None`].
    impl<S: ByteSource> Streams for Reading<S> {
        fn decoded(&self, id: ObjectId, limit: usize) -> Decoded {
            match self.try_resolve(id) {
                Ok(Some(_)) => {}
                Ok(None) => return Decoded::None,
                Err(unreadable) if unreadable.why == Why::Damaged => return Decoded::None,
                Err(unreadable) => return Decoded::Unreadable(unreadable),
            }
            let unreadable = |why: Why, error: &Error| {
                let message = format!("{} {} R doesn't read: {error}", id.0, id.1);
                Decoded::Unreadable(Unreadable::new(id, why, message))
            };
            match self.decoded_within(id, limit) {
                Ok(data) => Decoded::Data(data),
                Err(Error::Decode(error)) => {
                    let limited = error.faults.iter().any(|(_, f)| *f == Fault::Limit);
                    match (limited, error.data.is_empty()) {
                        (true, _) => Decoded::TooLarge,
                        (false, true) => Decoded::None,
                        (false, false) => Decoded::Data(error.data),
                    }
                }
                Err(error @ Error::Io(_)) => unreadable(Why::Source, &error),
                Err(
                    error @ (Error::Locked
                    | Error::Encrypted
                    | Error::Unsupported
                    | Error::Decrypt(_)),
                ) => unreadable(Why::Locked, &error),
                Err(error @ Error::Filters(Unfiltered::Filters { .. })) => {
                    unreadable(Why::Limit, &error)
                }
                Err(_) => Decoded::None,
            }
        }
    }
}
