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
//! Before the walk, the fonts the model's `fn:FontHasLatinChars()` asks of
//! are read ([`latin`]): what a font descriptor's `/CapHeight` is required
//! by (ISO 32000-2, 9.8.1, Table 120) only a reader of its fonts can say,
//! and the predicates are given it ([`FileCx::latin`]) before any row is
//! evaluated. What's read then is the pass's after the walk, each font read
//! once.
//!
//! [`Source::decoded_within`]: crate::source::Source::decoded_within
//! [`FileCx::latin`]: crate::FileCx::latin

use std::cell::Cell;
use std::collections::HashMap;
use std::mem;
use std::ops::RangeInclusive;
use std::ptr;

use pdf_fonts::{Decoded, Error, Fault, Font, Limits, With};
use unicode_script::{Script, UnicodeScript};

use super::kept::Kept;
use super::{NESTING, Objects, REFERENCES, Walk};
use crate::bytes::{self, Unfiltered};
use crate::object::{Dict, Object, ObjectId, Value};
use crate::pdf_filters;
use crate::source::Source;
use crate::{Latin, Resolve, Unreadable, Why};

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

/// A font as it's kept once read: by its number, where it's an indirect
/// object, else by where it's kept, which the walk reaches it as too, the
/// source keeping each object it gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Key {
    /// The indirect object it is.
    Id(ObjectId),
    /// The dictionary, written in place.
    Kept(*const Object),
}

/// The fonts [`latin`] read, each by its [`Key`], with what its read told,
/// for the pass after the walk; none where it asked a stream's data of a
/// source that gives none.
pub(super) type Read = HashMap<Key, Option<(Result<Font, Error>, Kept<Fault>)>>;

/// Which font descriptors' fonts have Latin characters, as the model's
/// `fn:FontHasLatinChars()` reads it, of each descriptor it's asked of,
/// one without `/CapHeight` (ISO 32000-2, 9.8.1, Table 120); and what was
/// read for it ([`Read`]). Every object `source` lists is looked in, and
/// each dictionary written in one, no deeper than [`NESTING`]: each
/// TrueType font, and each Type 0 font, by its first descendant's
/// descriptor; not a Type 3 font's, which Table 120 doesn't ask, nor a Type
/// 1 font's, which the model doesn't. Each is read once, by
/// [`Font::read_for_text`], within what's left of `budget`. Its font has
/// Latin characters where a code it draws has a letter of the Latin script
/// in its text ([`latin_in`]), and hasn't where a code it draws has text
/// and none has one; nothing is said of it where no code has text, or the
/// read fails, so the function can't be told. A descriptor two fonts name
/// has them where either has.
pub(super) fn latin(source: &dyn Source, budget: &mut usize) -> (Latin, Read) {
    let (mut latin, mut read) = (Latin::default(), Read::new());
    for id in source.ids() {
        let Ok(object) = source.get(id) else {
            continue;
        };
        let mut stack = vec![(object, 0)];
        while let Some((o, depth)) = stack.pop() {
            let inside = match o.value() {
                Value::Dict(d) => {
                    if let Some((descriptor, descriptor_id, composite)) = asked(source, d) {
                        let key = match depth {
                            0 => Key::Id(id),
                            _ => Key::Kept(ptr::from_ref(o)),
                        };
                        let font =
                            (read.entry(key)).or_insert_with(|| self::read(source, d, budget));
                        if let Some((Ok(font), _)) = font
                            && let Some(has) = latin_in(source, d, font, composite)
                        {
                            latin.insert(descriptor, descriptor_id, has);
                        }
                    }
                    d
                }
                Value::Stream(s) => &s.dict,
                Value::Array(a) if depth < NESTING => {
                    stack.extend(a.iter().rev().map(|e| (e, depth + 1)));
                    continue;
                }
                _ => continue,
            };
            if depth < NESTING {
                stack.extend(inside.iter().rev().map(|(_, v)| (v, depth + 1)));
            }
        }
    }
    (latin, read)
}

/// The descriptor `font` is asked `fn:FontHasLatinChars()` of, where it's
/// a font whose descriptor is, and lacks `/CapHeight`, absent or null: a
/// TrueType font's own, or a Type 0 font's first descendant's; with the
/// number its references end at, where it's an indirect object, and
/// whether the font is a Type 0 one.
fn asked<'a>(
    source: &'a dyn Source,
    font: &'a Dict,
) -> Option<(&'a Object, Option<ObjectId>, bool)> {
    let (_, subtype) = through(source, font.get(b"Subtype")?)?;
    let (holder, composite) = match subtype.as_name()? {
        b"TrueType" => (font, false),
        b"Type0" => {
            let (_, descendants) = through(source, font.get(b"DescendantFonts")?)?;
            let (_, first) = through(source, descendants.as_array()?.first()?)?;
            (first.as_dict()?, true)
        }
        _ => return None,
    };
    let (id, descriptor) = through(source, holder.get(b"FontDescriptor")?)?;
    let cap_height = descriptor.as_dict()?.get(b"CapHeight");
    match cap_height.and_then(|value| through(source, value)) {
        Some(_) => None,
        None => Some((descriptor, id, composite)),
    }
}

/// `value` through its references, at most [`REFERENCES`]: the number of
/// the last, none where it's written in place, and what it comes to; none
/// for null, a reference to what the file hasn't, which is null (ISO
/// 32000-2, 7.3.10), or one past the last.
fn through<'a>(
    source: &'a dyn Source,
    value: &'a Object,
) -> Option<(Option<ObjectId>, &'a Object)> {
    let (mut id, mut at) = (None, value);
    for _ in 0..REFERENCES {
        match at.value() {
            Value::Ref(to) => {
                at = source.get(*to).ok()?;
                id = Some(*to);
            }
            Value::Null => return None,
            _ => return Some((id, at)),
        }
    }
    None
}

/// Whether `read`, the font `font` reads as, has Latin characters: a letter
/// of the Latin script, by Unicode's Script property, in the text of a code
/// it draws ([`Font::code_text`]); none where no code it draws has text. A
/// simple font draws the codes from `/FirstChar` that its `/Widths` gives
/// a width that isn't 0, as far as `/LastChar` ([`Font::width`]), or every
/// code where it has no `/Widths`; a Type 0 font (`composite`) every code
/// of two bytes, whose text is its `/ToUnicode`'s, a predefined CMap's
/// being one the build hasn't. ISO 32000-2 doesn't say what a font has;
/// without its program, a font says it only by the codes it gives widths
/// and text.
fn latin_in(source: &dyn Source, font: &Dict, read: &Font, composite: bool) -> Option<bool> {
    let (codes, widths) = match (composite, given_widths(source, font)) {
        (true, _) => (0..=0xffff, false),
        (false, Some(codes)) => (codes, true),
        (false, None) => (0..=255, false),
    };
    let mut text = false;
    for code in codes {
        if widths && read.width(code) == 0.0 {
            continue;
        }
        let Some(said) = read.code_text(code) else {
            continue;
        };
        if said.chars().any(|c| c.script() == Script::Latin) {
            return Some(true);
        }
        text |= !said.is_empty();
    }
    text.then_some(false)
}

/// The codes a simple font's `/Widths` gives widths for: from its
/// `/FirstChar`, 0 where it has none, as many as `/Widths` has, as far as
/// its `/LastChar` where that's within them and above 0, as PDFium reads
/// them, no code past 255; none where it has no `/Widths`.
fn given_widths(source: &dyn Source, font: &Dict) -> Option<RangeInclusive<u32>> {
    let integer = |key: &[u8]| {
        let (_, value) = through(source, font.get(key)?)?;
        value.as_integer()
    };
    let (_, widths) = through(source, font.get(b"Widths")?)?;
    let widths = widths.as_array()?;
    let first = integer(b"FirstChar").unwrap_or(0);
    let end = first.saturating_add(widths.len() as i64);
    let last = match integer(b"LastChar") {
        Some(last) if last > 0 && last < end => last,
        _ => end - 1,
    };
    // None where the first is below 0, which is past any code, as PDFium
    // reads it into a `size_t`, or the last is.
    let (first, last) = match (u32::try_from(first), u32::try_from(last.min(255))) {
        (Ok(first), Ok(last)) => (first, last),
        _ => (1, 0),
    };
    Some(first..=last)
}

impl Walk<'_> {
    /// Reads each font the walk reached, in the order it reached them, its
    /// streams within `budget` bytes decoded in all, but those [`latin`]
    /// read already, which are as it read them: see the module's
    /// documentation.
    pub(super) fn fonts(&mut self, budget: usize) {
        let fonts = mem::take(&mut self.fonts);
        let mut read = mem::take(&mut self.fonts_read);
        let mut left = budget;
        for font in &fonts {
            let Some(dict) = font.object.as_dict() else {
                continue;
            };
            let key = match font.id {
                Some(id) => Key::Id(id),
                None => Key::Kept(ptr::from_ref(font.object)),
            };
            let _ = match read.remove(&key) {
                Some(kept) => kept,
                None => self::read(self.source, dict, &mut left),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    //! What a font is given of a stream's data, from a reading and from a
    //! document alone; what's read of the fonts the walk reaches; and which
    //! fonts have Latin characters, as a descriptor's `/CapHeight` asks.

    use std::borrow::Cow;
    use std::io;
    use std::ops::Range;
    use std::sync::atomic::{AtomicBool, Ordering};

    use pdf_structure::fixture::{self, Body, Xref};

    use super::*;
    use crate::bytes::{ByteSource, Options, WithReading};
    use crate::check::{Hold, Made, Report, Rule, check};
    use crate::dict;
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

    fn r(n: u32) -> Object {
        Object::reference(n)
    }

    fn numbers(n: &[i64]) -> Object {
        let numbers: Vec<Object> = n.iter().map(|&n| Object::from(n)).collect();
        numbers.into()
    }

    /// The catalog, object 1, its page tree, 2, and its page, 3, whose
    /// resources' fonts are `fonts`; and `more`.
    fn on_a_page(fonts: Dict, more: Vec<(u32, Body)>) -> Vec<(u32, Body)> {
        let page = dict! {
            "Type" => "Page",
            "Parent" => r(2),
            "MediaBox" => numbers(&[0, 0, 612, 792]),
            "Resources" => dict! { "Font" => fonts },
        };
        let mut objects = vec![
            (1, dict! { "Type" => "Catalog", "Pages" => r(2) }.into()),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }.into(),
            ),
            (3, page.into()),
        ];
        objects.extend(more);
        objects
    }

    /// A font descriptor of `flags` (32 nonsymbolic, 4 symbolic), with a
    /// `/CapHeight` where `cap_height`.
    fn descriptor(flags: i64, cap_height: bool) -> Dict {
        let mut descriptor = dict! {
            "Type" => "FontDescriptor",
            "FontName" => "Arial",
            "Flags" => flags,
            "FontBBox" => numbers(&[-200, -200, 1000, 900]),
            "ItalicAngle" => 0,
            "Ascent" => 900,
            "Descent" => -200,
            "StemV" => 80,
        };
        if cap_height {
            descriptor.set("CapHeight", 700);
        }
        descriptor
    }

    /// A TrueType font, of `encoding` where it has one, its `/Widths` from
    /// code `first`, its descriptor `descriptor`.
    fn truetype(encoding: Option<Object>, first: i64, widths: &[i64], descriptor: Object) -> Dict {
        let mut font = dict! {
            "Type" => "Font",
            "Subtype" => "TrueType",
            "BaseFont" => "Arial",
            "FirstChar" => first,
            "LastChar" => first + widths.len() as i64 - 1,
            "Widths" => numbers(widths),
            "FontDescriptor" => descriptor,
        };
        if let Some(encoding) = encoding {
            font.set("Encoding", encoding);
        }
        font
    }

    /// A TrueType font in WinAnsiEncoding drawing the codes from the space
    /// to `~`, its descriptor `descriptor`: Latin letters among them.
    fn win_ansi(descriptor: Object) -> Dict {
        truetype(
            Some(Object::from("WinAnsiEncoding")),
            32,
            &[500; 95],
            descriptor,
        )
    }

    /// A TrueType font drawing only the codes past `z` it names `uni4E00`,
    /// `uni4E01` and `uni4E02` in its `/Differences`, all Han: the letters
    /// its base encoding names at other codes it gives no width.
    fn han(descriptor: Object) -> Dict {
        let names = ["uni4E00", "uni4E01", "uni4E02"].map(Object::from);
        let mut differences = vec![Object::from(123)];
        differences.extend(names);
        let encoding = dict! { "Type" => "Encoding", "Differences" => differences };
        truetype(Some(encoding.into()), 123, &[1000; 3], descriptor)
    }

    /// A Type 0 font in Identity-H, its CIDFont object `cid_font`, and its
    /// `/ToUnicode` where it has one.
    fn type0(cid_font: u32, to_unicode: Option<Object>) -> Dict {
        let mut font = dict! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => "Batang",
            "Encoding" => "Identity-H",
            "DescendantFonts" => vec![r(cid_font)],
        };
        if let Some(to_unicode) = to_unicode {
            font.set("ToUnicode", to_unicode);
        }
        font
    }

    /// A CIDFont of TrueType glyphs, its descriptor object `descriptor`.
    fn cid_font(descriptor: u32) -> Dict {
        let info = dict! {
            "Registry" => Object::string("Adobe"),
            "Ordering" => Object::string("Identity"),
            "Supplement" => 0,
        };
        dict! {
            "Type" => "Font",
            "Subtype" => "CIDFontType2",
            "BaseFont" => "Batang",
            "CIDSystemInfo" => info,
            "FontDescriptor" => r(descriptor),
            "CIDToGIDMap" => "Identity",
        }
    }

    /// A `/ToUnicode` CMap mapping codes 1 and up, of two bytes, to each
    /// of `text` in turn.
    fn to_unicode(text: &[u16]) -> Body {
        let mut data = String::from("1 begincodespacerange <0000> <FFFF> endcodespacerange\n");
        data += &format!("{} beginbfchar\n", text.len());
        for (i, unit) in text.iter().enumerate() {
            data += &format!("<{:04X}> <{unit:04X}>\n", i + 1);
        }
        data += "endbfchar\n";
        Body::stream(dict! {}, data.as_bytes())
    }

    /// `objects`, the catalog object 1 among them, written at `version` and
    /// checked beside its reading, every object held as built.
    fn checked(version: &str, objects: &[(u32, Body)]) -> Report {
        let file = fixture::file(version, objects, dict! { "Root" => r(1) }, Xref::Table);
        let read = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        check(
            &read,
            &Hold::All(Made::Built),
            &crate::check::Options::default(),
        )
    }

    /// What a report says of fonts' `/CapHeight`s: each `missing` finding's
    /// object and table; and each table's rows not made for
    /// `fn:FontHasLatinChars`, with how many.
    type CapHeight = (Vec<(Option<u32>, String)>, Vec<(String, usize)>);

    /// [`CapHeight`] of `report`.
    fn cap_height(report: &Report) -> CapHeight {
        let missing = (report.findings.iter())
            .filter(|f| f.key == "CapHeight")
            .inspect(|f| assert_eq!(f.rule, Rule::Missing, "{f}"))
            .map(|f| (f.object.map(|(n, _)| n), f.table.clone()))
            .collect();
        let unmade = (report.unchecked.iter())
            .filter(|(u, _)| u.why == "fn:FontHasLatinChars")
            .inspect(|(u, _)| assert_eq!((u.column, u.key.as_str()), ("Required", "CapHeight")))
            .map(|(u, &n)| (u.table.clone(), n))
            .collect();
        (missing, unmade)
    }

    /// `missing CapHeight` in object `n` of `table`, and no row unmade.
    fn missing(n: u32, table: &str) -> CapHeight {
        (vec![(Some(n), table.to_string())], Vec::new())
    }

    /// Nothing missing, and no row unmade.
    fn made() -> CapHeight {
        (Vec::new(), Vec::new())
    }

    /// A row of `table` not made, and nothing missing.
    fn unmade(table: &str) -> CapHeight {
        (Vec::new(), vec![(table.to_string(), 1)])
    }

    /// A TrueType font's descriptor without `/CapHeight` is found where a
    /// code the font draws, one its `/Widths` gives a width that isn't 0,
    /// has a Latin letter in its text, and not where it has one; it isn't
    /// where the codes it draws have text and none has one, though its
    /// encoding names Latin letters at codes it gives no width, or a width
    /// of 0; and
    /// where no code has text, a symbolic font that embeds its program,
    /// without an encoding or a `/ToUnicode`, it can't be told, as before.
    #[test]
    fn a_truetype_font_without_cap_height_is_found_where_it_draws_latin_letters() {
        let a_font = |font: Dict, flags, with_cap_height| {
            let objects = on_a_page(
                dict! { "F1" => r(4) },
                vec![
                    (4, font.into()),
                    (5, descriptor(flags, with_cap_height).into()),
                ],
            );
            cap_height(&checked("1.7", &objects))
        };
        let truetype_table = "FontDescriptorTrueType";
        assert_eq!(
            a_font(win_ansi(r(5)), 32, false),
            missing(5, truetype_table)
        );
        assert_eq!(a_font(win_ansi(r(5)), 32, true), made());
        assert_eq!(a_font(han(r(5)), 32, false), made());
        // A symbolic font that embeds its program, without an encoding,
        // leaves its codes' text to the program, which isn't read: as
        // before. One that embeds none reads in WinAnsiEncoding, as PDFium
        // reads it.
        let symbolic = |embedded| {
            let mut descriptor = descriptor(4, false);
            if embedded {
                descriptor.set("FontFile2", r(6));
            }
            let objects = on_a_page(
                dict! { "F1" => r(4) },
                vec![
                    (4, truetype(None, 32, &[500; 95], r(5)).into()),
                    (5, descriptor.into()),
                    (6, Body::stream(dict! {}, b"not read")),
                ],
            );
            cap_height(&checked("1.7", &objects))
        };
        assert_eq!(symbolic(true), unmade(truetype_table));
        assert_eq!(symbolic(false), missing(5, truetype_table));
        // Its letters given a width of 0 aren't drawn: its digits and
        // punctuation have text, and no Latin letter.
        let widths: Vec<i64> = (32..127u8)
            .map(|code| match code.is_ascii_alphabetic() {
                true => 0,
                false => 500,
            })
            .collect();
        let no_letters = truetype(Some(Object::from("WinAnsiEncoding")), 32, &widths, r(5));
        assert_eq!(a_font(no_letters, 32, false), made());
        // A /CapHeight of null is absent (ISO 32000-2, 7.3.7).
        let mut null = descriptor(32, false);
        null.set("CapHeight", Object::null());
        let objects = on_a_page(
            dict! { "F1" => r(4) },
            vec![(4, win_ansi(r(5)).into()), (5, null.into())],
        );
        assert_eq!(
            cap_height(&checked("1.7", &objects)),
            missing(5, truetype_table)
        );
        // No /Widths: every code drawn, Latin letters among them.
        let mut no_widths = han(r(5));
        no_widths.remove(b"Widths");
        assert_eq!(a_font(no_widths, 32, false), missing(5, truetype_table));
    }

    /// A Type 0 font's descendant's descriptor without `/CapHeight` is
    /// found where its `/ToUnicode` gives a code a Latin letter, and not
    /// where it gives text of Han alone; without a `/ToUnicode`, its codes
    /// have no text, and it can't be told, as before.
    #[test]
    fn a_type_0_font_without_cap_height_is_found_where_its_text_is_latin() {
        let a_font = |to_unicode_of: Option<&[u16]>| {
            let mut more = vec![(5, cid_font(6).into()), (6, descriptor(4, false).into())];
            if let Some(text) = to_unicode_of {
                more.push((7, to_unicode(text)));
            }
            let font = type0(5, to_unicode_of.map(|_| r(7)));
            more.push((4, font.into()));
            cap_height(&checked("1.7", &on_a_page(dict! { "F1" => r(4) }, more)))
        };
        let table = "FontDescriptorCIDType2";
        assert_eq!(a_font(Some(&[0x4e00, 0x4e01])), made());
        assert_eq!(a_font(Some(&[0x4e00, 0x41])), missing(6, table));
        assert_eq!(a_font(None), unmade(table));
    }

    /// A Type 3 font's descriptor isn't answered, as ISO 32000-2 asks no
    /// `/CapHeight` of it (9.8.1, Table 120): its row stays not made, as
    /// before, though its glyphs' names are Latin letters.
    #[test]
    fn a_type_3_font_s_descriptor_is_asked_nothing_of() {
        let font = dict! {
            "Type" => "Font",
            "Subtype" => "Type3",
            "FontBBox" => numbers(&[0, 0, 1000, 1000]),
            "FontMatrix" => vec![
                Object::from(0.001), Object::from(0), Object::from(0),
                Object::from(0.001), Object::from(0), Object::from(0),
            ],
            "CharProcs" => dict! { "a" => r(6) },
            "Encoding" => dict! { "Type" => "Encoding", "Differences" => vec![Object::from(97), Object::from("a")] },
            "FirstChar" => 97,
            "LastChar" => 97,
            "Widths" => numbers(&[1000]),
            "FontDescriptor" => r(5),
            "Resources" => dict! {},
        };
        let mut type3 = descriptor(32, false);
        type3.remove(b"FontBBox");
        let objects = on_a_page(
            dict! { "F1" => r(4) },
            vec![
                (4, font.into()),
                (5, type3.into()),
                (6, Body::stream(dict! {}, b"1000 0 d0")),
            ],
        );
        assert_eq!(
            cap_height(&checked("1.7", &objects)),
            unmade("FontDescriptorType3")
        );
    }

    /// A descriptor two fonts name has Latin characters where either font
    /// has, whichever comes first: found once.
    #[test]
    fn a_descriptor_two_fonts_share_is_found_where_either_is_latin() {
        let objects = on_a_page(
            dict! { "F1" => r(4), "F2" => r(5) },
            vec![
                (4, han(r(6)).into()),
                (5, win_ansi(r(6)).into()),
                (6, descriptor(32, false).into()),
            ],
        );
        assert_eq!(
            cap_height(&checked("1.7", &objects)),
            missing(6, "FontDescriptorTrueType")
        );
    }

    /// A font written in place in its resources is read as one of its own
    /// is, and so is a descriptor written in place in its font, known by
    /// where it was written: each found, in the object it's written in.
    #[test]
    fn a_font_or_a_descriptor_written_in_place_is_read() {
        let in_place = on_a_page(
            dict! { "F1" => win_ansi(r(5)) },
            vec![(5, descriptor(32, false).into())],
        );
        assert_eq!(
            cap_height(&checked("1.7", &in_place)),
            missing(5, "FontDescriptorTrueType")
        );
        let its_descriptor = on_a_page(
            dict! { "F1" => r(4) },
            vec![(4, win_ansi(descriptor(32, false).into()).into())],
        );
        assert_eq!(
            cap_height(&checked("1.7", &its_descriptor)),
            missing(4, "FontDescriptorTrueType")
        );
    }

    /// A font whose `/ToUnicode` takes a password that wasn't given isn't
    /// read, and its descriptor's `/CapHeight` can't be told, as before.
    #[test]
    fn a_font_whose_stream_takes_a_password_isn_t_told() {
        let settings = Settings {
            kind: Kind::Aes256,
            user: b"user",
            owner: b"owner",
            p: -4,
            id: b"0123456789abcdef",
            encrypt_metadata: true,
        };
        let objects = on_a_page(
            dict! { "F1" => r(4) },
            vec![
                (4, type0(5, Some(r(7))).into()),
                (5, cid_font(6).into()),
                (6, descriptor(4, false).into()),
                (7, to_unicode(&[0x41])),
            ],
        );
        let file = fixture::encrypted(&settings, &objects, dict! { "Root" => r(1) });
        let locked = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        let report = check(
            &locked,
            &Hold::All(Made::Built),
            &crate::check::Options::default(),
        );
        assert_eq!(cap_height(&report), unmade("FontDescriptorCIDType2"));
    }
}
