//! Fonts, read through pdf-fonts (ISO 32000-2, 9.5 to 9.10): each the walk
//! reached in the newest revision as one of the model's font tables, in the
//! order it reached them, read again by [`Font::read_for_text`], as fonts
//! made for text are, no program read, under pdf-fonts' caps. Its objects
//! are the file's as the predicates resolve them ([`Objects`]), and its
//! streams' data, a `/ToUnicode`, an embedded CMap and those it uses, a
//! Type 3 font's glyphs, comes decoded from the source
//! ([`Source::decoded_within`]), as a font reads a reading's ([`decoded`]),
//! the file's fonts' within one budget. A source that gives no stream's
//! data, as a document alone doesn't, leaves a font that asks one unread,
//! as it leaves content. What the read tells is kept ([`Kept`]); none of it
//! is found yet.
//!
//! [`Source::decoded_within`]: crate::source::Source::decoded_within

use std::cell::Cell;
use std::mem;

use pdf_fonts::{Decoded, Error, Fault, Font, Limits, With};

use super::kept::Kept;
use super::{Objects, Walk};
use crate::bytes::{self, Unfiltered};
use crate::object::{Dict, ObjectId};
use crate::pdf_filters;
use crate::source::Source;
use crate::{Resolve, Unreadable, Why};

/// The model's font tables: what the walk reaches as one is a font, read
/// again after it. A Type 0 font's CIDFont, descriptor and CMap are read as
/// its parts, and so are a simple font's descriptor and encoding.
pub(super) const FONTS: [&str; 5] = [
    "FontType1",
    "FontTrueType",
    "FontMultipleMaster",
    "FontType3",
    "FontType0",
];

/// What fonts are read under: pdf-fonts' caps, which ISO 32000-2 doesn't
/// set.
const LIMITS: Limits = Limits::DEFAULT;

/// Stream `id`'s data, at most `limit` bytes of it, as `source` gives it
/// ([`Source::decoded_within`]), said as pdf-fonts' [`Streams`] for a
/// pdf-structure `Reading` says a reading's: a stream that decodes in part
/// is what decoded, as PDFium takes it, unless what stopped it is the
/// limit, which is [`Decoded::TooLarge`]; what takes a password, a source
/// that failed and filters past the reader's cap, [`Decoded::Unreadable`],
/// as the predicates' resolver says of an object that doesn't read
/// ([`Objects`]); what isn't there or isn't a stream, what doesn't read,
/// and filters the reader doesn't take, [`Decoded::None`]. A source that
/// gives no stream's data, as a document alone doesn't, gives none, which
/// `ungiven` notes, so that what's read without it isn't taken for the
/// file's.
///
/// [`Source::decoded_within`]: crate::source::Source::decoded_within
/// [`Streams`]: pdf_fonts::Streams
pub(super) fn decoded(
    source: &dyn Source,
    id: ObjectId,
    limit: usize,
    ungiven: &Cell<bool>,
) -> Decoded {
    match Objects(source).try_resolve(id) {
        Ok(Some(_)) => {}
        Ok(None) => return Decoded::None,
        Err(unreadable) if unreadable.why == Why::Damaged => return Decoded::None,
        Err(unreadable) => return Decoded::Unreadable(unreadable),
    }
    let unreadable = |why: Why, error: &bytes::Error| {
        let message = format!("{} {} R doesn't read: {error}", id.0, id.1);
        Decoded::Unreadable(Unreadable::new(id, why, message))
    };
    let Some(decoded) = source.decoded_within(id, limit) else {
        ungiven.set(true);
        return Decoded::None;
    };
    match decoded {
        Ok(data) => Decoded::Data(data),
        Err(bytes::Error::Decode(error)) => {
            let limited = (error.faults.iter()).any(|(_, f)| *f == pdf_filters::Fault::Limit);
            match (limited, error.data.is_empty()) {
                (true, _) => Decoded::TooLarge,
                (false, true) => Decoded::None,
                (false, false) => Decoded::Data(error.data),
            }
        }
        Err(error @ bytes::Error::Io(_)) => unreadable(Why::Source, &error),
        Err(
            error @ (bytes::Error::Locked
            | bytes::Error::Encrypted
            | bytes::Error::Unsupported
            | bytes::Error::Decrypt(_)),
        ) => unreadable(Why::Locked, &error),
        Err(error @ bytes::Error::Filters(Unfiltered::Filters { .. })) => {
            unreadable(Why::Limit, &error)
        }
        Err(_) => Decoded::None,
    }
}

/// `font`, a font dictionary of `source`'s, read for text within what's
/// left of `budget`, what it tells kept; none where it asked a stream's data
/// of a source that gives none ([`decoded`]), as what it read then isn't
/// the file's font.
fn read<'a>(
    source: &'a dyn Source,
    font: &'a Dict,
    budget: &mut usize,
) -> Option<(Result<Font, Error>, Kept<Fault>)> {
    let ungiven = Cell::new(false);
    let streams = With::new(Objects(source), |id, limit| {
        decoded(source, id, limit, &ungiven)
    });
    let mut kept = Kept::default();
    let read = Font::read_for_text(streams, font, &LIMITS, budget, &mut kept);
    (!ungiven.get()).then_some((read, kept))
}

impl Walk<'_> {
    /// Reads each font the walk reached, in the order it reached them, its
    /// streams within `budget` bytes decoded in all: see the module's
    /// documentation.
    pub(super) fn fonts(&mut self, budget: usize) {
        let fonts = mem::take(&mut self.fonts);
        let mut left = budget;
        for font in &fonts {
            let Some(dict) = font.object.as_dict() else {
                continue;
            };
            let _ = read(self.source, dict, &mut left);
        }
    }
}

#[cfg(test)]
mod tests {
    //! What a font is given of a stream's data, from a reading and from a
    //! document alone; and what's read of the fonts the walk reaches.

    use std::borrow::Cow;
    use std::io;
    use std::ops::Range;
    use std::sync::atomic::{AtomicBool, Ordering};

    use pdf_structure::fixture::{self, Body, Xref};

    use super::*;
    use crate::bytes::{ByteSource, Options, WithReading};
    use crate::dict;
    use crate::object::Object;
    use crate::pdf_crypt::writer::{Kind, Settings};

    /// A `/ToUnicode`'s data, as each stream here holds it.
    const PLAIN: &[u8] = b"1 beginbfchar <41> <0041> endbfchar";

    /// What [`decoded`] gives of stream `id` of `source`, within `limit`,
    /// and whether it noted that the source gives no stream's data.
    fn asked(source: &dyn Source, id: ObjectId, limit: usize) -> (Decoded, bool) {
        let ungiven = Cell::new(false);
        (decoded(source, id, limit, &ungiven), ungiven.get())
    }

    /// Why what [`asked`] gave doesn't read, where it's that.
    fn why(asked: (Decoded, bool)) -> Why {
        match asked {
            (Decoded::Unreadable(unreadable), false) => unreadable.why,
            other => panic!("{other:?}"),
        }
    }

    /// A font, object 1, its `/ToUnicode` object 2, and its data through
    /// Flate, object 3.
    fn objects() -> Vec<(u32, Body)> {
        vec![
            (
                1,
                dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => Object::reference(2) }
                    .into(),
            ),
            (2, Body::stream(dict! {}, PLAIN)),
            (
                3,
                Body::stream(dict! { "Filter" => "FlateDecode" }, &fixture::zlib(PLAIN)),
            ),
        ]
    }

    /// Every table the walk names a font by is the model's, so a font
    /// reached as one is read.
    #[test]
    fn each_font_table_is_the_model_s() {
        for name in FONTS {
            assert!(crate::grammar().table(name).is_some(), "{name}");
        }
    }

    /// A stream within its limit is its data, with filters or without, and
    /// past it too large, by a byte; a document alone gives none, noted,
    /// and a font that asks it isn't read, where a font that asks none is.
    #[test]
    fn a_stream_s_data_within_its_limit_and_past_it() {
        let file = fixture::file("1.7", &objects(), dict! {}, Xref::Table);
        let read = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        for id in [(2, 0), (3, 0)] {
            let at = PLAIN.len();
            let data = Decoded::Data(PLAIN.to_vec());
            assert_eq!(asked(&read, id, at), (data, false), "{id:?}");
            assert_eq!(asked(&read, id, at - 1), (Decoded::TooLarge, false));
            assert_eq!(asked(read.document(), id, at), (Decoded::None, true));
        }
        // Not a stream: none. Not there: none, and nothing noted, as the
        // source isn't asked for its data.
        assert_eq!(asked(&read, (1, 0), 100), (Decoded::None, false));
        assert_eq!(asked(read.document(), (9, 0), 100), (Decoded::None, false));

        let font = read.get((1, 0)).unwrap().as_dict().unwrap();
        let mut budget = PLAIN.len();
        let (font_read, kept) = super::read(&read, font, &mut budget).unwrap();
        assert_eq!(font_read.unwrap().code_text(0x41).as_deref(), Some("A"));
        assert!(kept.told().is_empty());
        assert_eq!(budget, 0);
        let mut budget = PLAIN.len() - 1;
        let (too_large, _) = super::read(&read, font, &mut budget).unwrap();
        assert_eq!(too_large.err(), Some(Error::TooLarge));
        let mut budget = PLAIN.len();
        assert!(super::read(read.document(), font, &mut budget).is_none());
        let plain = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" };
        assert!(super::read(read.document(), &plain, &mut budget).is_some());
    }

    /// A stream that decodes in part is what decoded; none where nothing
    /// did, or the reader doesn't take its filters; past the reader's cap on
    /// filters, a cap's.
    #[test]
    fn a_stream_that_doesn_t_decode_whole() {
        let many: Vec<Object> = (0..26).map(|_| Object::from("ASCIIHexDecode")).collect();
        let objects = vec![
            (
                1,
                Body::stream(dict! { "Filter" => "ASCIIHexDecode" }, b"4142 zz"),
            ),
            (
                2,
                Body::stream(dict! { "Filter" => "ASCIIHexDecode" }, b"zz"),
            ),
            (
                3,
                Body::stream(dict! { "Filter" => "JBIG2Decode" }, b"data"),
            ),
            (5, Body::stream(dict! { "Filter" => many }, b"41>")),
        ];
        let file = fixture::file("1.7", &objects, dict! {}, Xref::Table);
        let read = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        let data = Decoded::Data(b"AB".to_vec());
        assert_eq!(asked(&read, (1, 0), 100), (data, false));
        assert_eq!(asked(&read, (2, 0), 100), (Decoded::None, false));
        assert_eq!(asked(&read, (3, 0), 100), (Decoded::None, false));
        assert_eq!(why(asked(&read, (5, 0), 100)), Why::Limit);
    }

    /// A locked file's stream takes a password, whether it's an object of
    /// an object stream that doesn't read or its data that doesn't decrypt;
    /// a font that asks it isn't read, and its budget is as it was.
    #[test]
    fn a_locked_file_s_streams() {
        let settings = Settings {
            kind: Kind::Aes256,
            user: b"user",
            owner: b"owner",
            p: -4,
            id: b"0123456789abcdef",
            encrypt_metadata: true,
        };
        let file = fixture::encrypted(&settings, &objects(), dict! {});
        let locked = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        assert!(matches!(
            locked.decoded_within((2, 0), 100),
            Some(Err(bytes::Error::Locked))
        ));
        assert_eq!(why(asked(&locked, (2, 0), 100)), Why::Locked);
        let font = locked.get((1, 0)).unwrap().as_dict().unwrap();
        let mut budget = 100;
        let (font_read, _) = super::read(&locked, font, &mut budget).unwrap();
        let Err(Error::Unreadable(unreadable)) = font_read else {
            panic!("{font_read:?}");
        };
        assert_eq!((unreadable.id, unreadable.why), ((2, 0), Why::Locked));
        assert_eq!(budget, 100);
    }

    /// A file read by position, whose source fails once it's told to.
    struct Failing {
        bytes: Vec<u8>,
        failing: AtomicBool,
    }

    impl ByteSource for Failing {
        fn len(&self) -> usize {
            self.bytes.len()
        }

        fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
            match self.failing.load(Ordering::Relaxed) {
                true => Err(io::Error::other("no bytes")),
                false => Ok(Cow::Borrowed(&self.bytes[range])),
            }
        }

        fn first_window(&self) -> usize {
            64
        }
    }

    /// A source that fails to give a stream's data is the source's, and
    /// given them again, the data's read.
    #[test]
    fn a_source_that_fails() {
        let source = Failing {
            bytes: fixture::file("1.7", &objects(), dict! {}, Xref::Table),
            failing: AtomicBool::new(false),
        };
        let read = WithReading::new(&source, &[], &Options::DEFAULT).unwrap();
        source.failing.store(true, Ordering::Relaxed);
        assert!(matches!(
            read.decoded_within((2, 0), 100),
            Some(Err(bytes::Error::Io(_)))
        ));
        assert_eq!(why(asked(&read, (2, 0), 100)), Why::Source);
        source.failing.store(false, Ordering::Relaxed);
        let data = Decoded::Data(PLAIN.to_vec());
        assert_eq!(asked(&read, (2, 0), 100), (data, false));
    }
}
