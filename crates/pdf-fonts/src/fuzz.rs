//! What the fuzz targets check, which unit tests check too: the promises a Type 1
//! program's scan, a glyph's or a font's name, text written in a standard 14 font, a
//! font's reading, its program's included, and the strings content shows make, on any
//! input; and, with `embed`, those a TrueType program opened for embedding and the font
//! written of it make. It's compiled only for tests and under cargo-fuzz's
//! `--cfg fuzzing`.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use pdf_content_streams::{Concat, ResCx};
use pdf_objects::fault::Place;
use pdf_objects::object::Stream;
use pdf_objects::parse::Parser;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable, Why, dict};

use crate::fixture::{
    OpenType, cff_program, cid_cff_program, composite_bomb_program, composite_of_program,
    doubled_sign_type1_program, heavy_cff_program, loca_past_end_program, long_cff_program,
    moving_cff_program, opentype_cid_program, opentype_program, rolling_type1_program,
    truetype_program, type1_accents_program, type1_integers_program, type1_lookups_program,
    type1_program,
};
use crate::predefined::{Collection, scheme};
use crate::type1::{Refusal, scan};
use crate::{
    Encoding, Error, Fault, Font, Fonts, Limits, Shown, Std14, Unread, cp1252, glyph_text,
    name_code_point,
};

/// How many of an input's words are read as names, as a run of the target
/// shouldn't take the square of its input.
const NAMES: usize = 64;

/// How many of an input's bytes are read as codes: there are only 256 of them, so
/// more finds nothing more.
const CODES: usize = 4;

/// `data` as a Type 1 program: what's held no more than what's given; and a cap
/// refuses a program and never reads it otherwise, by the scan alone, never read-fonts,
/// whose work under a larger cap is what the cap bounds. A program read under a cap is
/// read the same under a larger, one malformed under a cap is malformed under a larger,
/// and one past a cap is past every smaller: the scan's fuel says when it stops, never
/// what it reads.
pub fn type1(data: &[u8]) {
    let limits = Limits::DEFAULT;
    let read = scan(data, &limits);
    if let Some(charstrings) = &read {
        assert!(charstrings.contains_key(&b".notdef"[..]));
        let held: usize = charstrings.values().flatten().map(Vec::len).sum();
        // The one `.notdef` a program may have made up for it is 5 bytes.
        assert!(held <= data.len() + 5, "{held} of {}", data.len());
    }
    // From the largest cap to none, each what the scan comes to.
    let caps = [2 * limits.type1_work, limits.type1_work, 4, 1, 0];
    let scanned = caps.map(|type1_work| {
        let limits = Limits {
            type1_work,
            ..limits
        };
        crate::type1::read(data, &limits).map(|scanned| scanned.charstrings)
    });
    assert_eq!(scanned[1].as_ref().ok(), read.as_ref());
    for (i, smaller) in scanned.iter().enumerate() {
        for larger in &scanned[..i] {
            match smaller {
                Ok(_) => assert_eq!(larger, smaller),
                Err(Refusal::Malformed) => assert_eq!(larger, smaller),
                Err(Refusal::Capped) => {}
            }
            if larger == &Err(Refusal::Capped) {
                assert_eq!(smaller, larger);
            }
        }
    }
}

/// `data`'s words, and the whole of it, each as a glyph's name and a font's, and
/// its bytes as codes: never a panic, each reading what it says.
pub fn names(data: &[u8]) {
    let words = data.split(|c| c.is_ascii_whitespace()).take(NAMES);
    for name in words.chain([data]) {
        a_name(name);
    }
    for &code in data.iter().take(CODES) {
        codes(code);
    }
}

fn a_name(name: &[u8]) {
    if let Some((point, period)) = name_code_point(name) {
        assert!(point <= 0xFF_FFFF, "{point:#x}");
        assert!(!period || name.contains(&b'.'));
    }
    if let Ok(text) = std::str::from_utf8(name) {
        for dingbats in [false, true] {
            let read = glyph_text(text, dingbats);
            assert!(read.as_ref().is_none_or(|t| !t.is_empty()));
            // What follows a period is passed over.
            let before = text.split('.').next().unwrap_or_default();
            assert_eq!(glyph_text(before, dingbats), read);
        }
    }
    if let Some(font) = Std14::named(name) {
        // In any case, and after a subset's tag, where it hasn't one.
        assert_eq!(Std14::named(&name.to_ascii_lowercase()), Some(font));
        if !name.contains(&b'+') {
            let tagged = [&b"ABCDEF+"[..], name].concat();
            assert_eq!(Std14::named(&tagged), Some(font));
        }
    }
    if Encoding::named(name).is_some() {
        assert!(name.ends_with(b"Encoding"));
    }
    if let Some(read) = scheme(name) {
        // A code's first byte, any, says one or two.
        let _ = read.split.leads(name.first().copied().unwrap_or(0));
    }
    let _ = Collection::ordered(name);
    #[cfg(feature = "predefined-cmaps")]
    if let Some(cmap) = crate::predefined::bundled(name) {
        let _ = cmap.metadata();
    }
    // The bundle's names, had in every build, are what it has; and a CMap the build
    // hasn't is the build's only where the bundle has it and the build is without it.
    let bundles = crate::predefined::bundles(name);
    #[cfg(feature = "predefined-cmaps")]
    assert_eq!(crate::predefined::bundled(name).is_some(), bundles);
    if crate::predefined::missing(name) == Fault::PredefinedNotBuilt {
        assert!(bundles && !cfg!(feature = "predefined-cmaps"));
    }
}

/// A code in every encoding: its text is its glyph's, or Windows-1252's where that
/// has one.
fn codes(code: u8) {
    use Encoding::{MacRoman, Standard, Symbol, WinAnsi, ZapfDingbats};
    for encoding in [Standard, WinAnsi, MacRoman, Symbol, ZapfDingbats] {
        let (glyph, text) = (encoding.glyph(code), encoding.text(code));
        if encoding == WinAnsi {
            let glyph_s = glyph.and_then(|name| glyph_text(name, false));
            assert_eq!(text, cp1252(code).map(String::from).or(glyph_s));
        } else {
            assert_eq!(glyph.is_none(), text.is_none(), "{encoding:?} {code:#x}");
        }
    }
}

/// How many of an input's bytes are written as text in each standard 14 font: past
/// what an encoding has, more finds nothing more.
const TEXT: usize = 512;

/// `data` as text written in each standard 14 font ([`Std14::codes`]): its bytes as
/// UTF-8, as Latin-1, and each as the text its code is in the font's encoding. The
/// codes read back as the text in the font its dictionary is ([`Std14::dict`]), one a
/// character; or the first character with no code is refused where it is, and what's
/// before it is written and reads back.
pub fn written(data: &[u8]) {
    static READ: OnceLock<Vec<Font>> = OnceLock::new();
    let read = READ.get_or_init(|| {
        let read = |font: Std14| {
            let (dict, none) = (font.dict(), crate::fonts::NoObjects);
            let read = Font::read(none, &dict, &Limits::DEFAULT, &mut 0, &mut ());
            read.expect("a font by name alone has no stream to decompress")
        };
        Std14::ALL.map(read).into()
    });
    let data = &data[..data.len().min(TEXT)];
    let utf8 = String::from_utf8_lossy(data);
    let latin1: String = data.iter().copied().map(char::from).collect();
    for (font, read) in Std14::ALL.into_iter().zip(read) {
        let encoding = font.written_in();
        let own = data.iter().filter_map(|&code| encoding.text(code));
        let own: String = own.collect();
        for text in [&utf8[..], &latin1, &own] {
            let codes = match font.codes(text) {
                Ok(codes) => codes,
                Err(refused) => {
                    assert_eq!(refused.font, font);
                    let at = &text[refused.at..];
                    assert_eq!(at.chars().next(), Some(refused.character));
                    assert_eq!(encoding.code(refused.character), None);
                    let before = &text[..refused.at];
                    let codes = font.codes(before).expect("what's before it is drawn");
                    assert_eq!(read.text(&codes).as_deref(), Some(before), "{font:?}");
                    continue;
                }
            };
            assert_eq!(codes.len(), text.chars().count(), "one a character");
            assert_eq!(read.text(&codes).as_deref(), Some(text), "{font:?}");
        }
    }
}

/// What splits a font's input into its parts ([`fonts`]).
pub const FONT_PART: &[u8] = b"\n%%font\n";

/// How many numbers of a file are read as fonts, from what [`over`] is given, as a
/// run of the target shouldn't take the square of its input.
const NUMBERS: usize = 32;

/// How many values of a part are read as objects, past its first.
const VALUES: usize = 16;

/// What a font is read within here: enough for what an input may hold, and little
/// enough for a run to try many.
const BUDGET: usize = 1 << 20;

/// `data` split at each [`FONT_PART`], into six parts at most.
fn font_parts(data: &[u8]) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut rest = data;
    while parts.len() < 5 {
        let at = rest.windows(FONT_PART.len()).position(|w| w == FONT_PART);
        let Some(at) = at else {
            break;
        };
        parts.push(&rest[..at]);
        rest = &rest[at + FONT_PART.len()..];
    }
    parts.push(rest);
    parts
}

/// The values of `part`, read by pdf-objects' parser, the first as `first` and those
/// after it each from `rest` on; a value that doesn't read ends them.
fn values(part: &[u8], first: u32, rest: u32, objects: &mut BTreeMap<ObjectId, Object>) {
    let mut parser = Parser::at(part, 0);
    for i in 0..=VALUES as u32 {
        let Ok(value) = parser.value(&mut ()) else {
            return;
        };
        let n = if i == 0 { first } else { rest + i };
        objects.insert(ObjectId::new(n, 0), value);
    }
}

/// `data`'s parts as a file's objects, each where it's there: its font dictionary, 1,
/// and its descendant's, 2, each with values after it, from 10 and from 30, for
/// either to refer to; a `/ToUnicode`, 3; a CMap or a CharProc, 4, which uses 6 by
/// its `/UseCMap`; a program, 5; and a CMap or a `/CIDToGIDMap`, 6. A
/// reference at 9 to the font.
fn objects_of(data: &[u8]) -> BTreeMap<ObjectId, Object> {
    let parts = font_parts(data);
    let mut objects = BTreeMap::new();
    let part = |i: usize| parts.get(i).copied().unwrap_or_default();
    values(part(0), 1, 10, &mut objects);
    values(part(1), 2, 30, &mut objects);
    let mut stream = |n: u32, dict: Dict, part: &[u8]| {
        objects.insert(ObjectId::new(n, 0), Stream::new(dict, part.to_vec()).into());
    };
    stream(3, dict! {}, part(2));
    stream(4, dict! { "UseCMap" => Object::reference(6) }, part(3));
    stream(5, dict! {}, part(4));
    stream(6, dict! {}, part(5));
    objects.insert(ObjectId::new(9, 0), Object::reference(1));
    objects
}

/// A file's objects in a map.
#[derive(Clone, Copy)]
struct Map<'m>(&'m BTreeMap<ObjectId, Object>);

impl<'m> Resolve<'m> for Map<'m> {
    fn resolve(&self, id: ObjectId) -> Option<&'m Object> {
        self.0.get(&id)
    }
}

impl Streams for Map<'_> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        let Some(stream) = self.0.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "no such stream"));
        };
        let data = stream.data.bytes().unwrap_or_default();
        match data.len() > limit {
            true => Err(Unreadable::new(id, Why::Limit, "past the limit")),
            false => Ok(Cow::Borrowed(data)),
        }
    }
}

/// Objects and streams that count what they're asked: one, for each object and each
/// stream.
#[derive(Clone, Copy)]
struct Counting<'c, O> {
    objects: O,
    asked: &'c Cell<usize>,
}

impl<'o, O: Resolve<'o>> Resolve<'o> for Counting<'_, O> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.asked.set(self.asked.get() + 1);
        self.objects.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        self.asked.set(self.asked.get() + 1);
        self.objects.try_resolve(id)
    }
}

impl<O: Streams> Streams for Counting<'_, O> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        self.asked.set(self.asked.get() + 1);
        self.objects.decoded(id, limit)
    }
}

/// Objects and streams that fail for `why`: the first object asked, or the first
/// stream, and whether they did.
#[derive(Clone, Copy)]
struct FailingFirst<'c, O> {
    objects: O,
    why: Why,
    streams: bool,
    failed: &'c Cell<bool>,
}

impl<O> FailingFirst<'_, O> {
    fn fails(&self, id: ObjectId) -> Option<Unreadable> {
        (!self.failed.replace(true)).then(|| Unreadable::new(id, self.why, "made to fail"))
    }
}

impl<'o, O: Resolve<'o>> Resolve<'o> for FailingFirst<'_, O> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.try_resolve(id).ok().flatten()
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        if !self.streams
            && let Some(unreadable) = self.fails(id)
        {
            return Err(unreadable);
        }
        self.objects.try_resolve(id)
    }
}

impl<O: Streams> Streams for FailingFirst<'_, O> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        if self.streams
            && let Some(unreadable) = self.fails(id)
        {
            return Err(unreadable);
        }
        self.objects.decoded(id, limit)
    }
}

/// Streams that give more than they're asked for, and what asked.
#[derive(Clone, Copy)]
struct Overgiving<'c, O> {
    objects: O,
    asked: &'c Cell<bool>,
}

impl<'o, O: Resolve<'o>> Resolve<'o> for Overgiving<'_, O> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        self.objects.try_resolve(id)
    }
}

impl<O> Streams for Overgiving<'_, O> {
    fn decoded(&self, _: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        self.asked.set(true);
        Ok(Cow::Owned(vec![0; limit + 1]))
    }
}

/// What a font's accessors say of every code of a byte, a sample of those of more, and
/// a short string, each holding what it promises of the others.
pub(crate) fn digest(font: &Font) -> String {
    const SAMPLE: [u32; 8] = [
        0x100,
        0x4141,
        0x8140,
        0x8200,
        0xffff,
        0x10000,
        0xd800_0000,
        u32::MAX,
    ];
    let s = b"\x00\x41 \x81\x41\xa0\xff\x81";
    let extent = font.extent();
    assert!(extent >= 0.0, "{extent}");
    let by_code = |code: u32| {
        let width = font.width(code);
        assert!(width.is_nan() || width.abs() <= extent, "{width} {extent}");
        if code > 255 {
            assert!(!font.width_guessed(code));
        }
        let char_box = font.char_box(code);
        if let Some(r) = char_box {
            for side in [r.x0, r.y0, r.x1, r.y1] {
                assert!(side.is_nan() || side.abs() <= extent, "{side} {extent}");
            }
        }
        // None says what no program read, or what a program's boxes aren't known to be
        // PDFium's: what's past a CIDFont's map draws nothing, whatever its program.
        if !font.outlined() || font.approx().boxes && font.byte_widths().is_some() {
            assert_eq!(char_box, None);
        }
        format!(
            "{:?} {width:?} {:?} {:?} {char_box:?} {:?} {:?} {:?} {:?}\n",
            font.cid(code),
            font.width_guessed(code),
            font.vertical(code),
            font.glyph_box(code),
            font.code_text(code),
            font.word_spaced(code, 1),
            font.word_spaced(code, 2),
        )
    };
    let mut out: String = (0..=255).chain(SAMPLE).map(by_code).collect();
    if let Some(widths) = font.byte_widths() {
        for (code, width) in widths.iter().enumerate() {
            assert_eq!(width.to_bits(), font.width(code as u32).to_bits());
        }
    }
    let mut at = 0;
    let mut split = Vec::new();
    let (fewest, most) = font.codes(s).size_hint();
    for (range, code) in font.codes(s) {
        assert_eq!(range.start, at);
        assert!(range.end > at && range.end <= s.len(), "{range:?}");
        assert!(range.len() <= 4, "{range:?}");
        at = range.end;
        out.push_str(&by_code(code));
        split.push((range, code));
    }
    assert_eq!(at, s.len());
    // Its hint holds how many codes there are.
    assert!(fewest <= split.len() && most.is_some_and(|most| split.len() <= most));
    assert_eq!(
        font.width_guessed_in(s),
        font.codes(s).any(|(_, code)| font.width_guessed(code))
    );
    assert_eq!(font.is_vertical(), font.vertical(0x41).is_some());
    let read = font.program() == crate::FontProgram::Read;
    assert!(read || !font.outlined());
    assert!(cfg!(feature = "programs") || font.program() == crate::FontProgram::None);
    // Showing what's drawn already costs what its first charge says, and nothing after.
    let (mut none, mut all, mut most) = (0, usize::MAX, usize::MAX);
    assert_eq!(font.charge_outlines(s, |_| false, &mut none), Ok(()));
    assert_eq!(font.charge_outlines(s, |_| true, &mut all), Ok(()));
    let mut seen = std::collections::HashSet::new();
    assert_eq!(
        font.charge_outlines(s, |glyph| seen.insert(glyph), &mut most),
        Ok(())
    );
    assert!(none == 0 && most >= all && (font.outlined() || all == usize::MAX));
    let more = format!(
        "{} {:?} {:?} {:?} {extent:?} {:?} {:?} {:?} {:?} {:?} {}\n",
        font.loaded(),
        font.kind(),
        font.approx(),
        font.metrics(),
        font.cost(),
        split,
        font.text(s),
        font.program(),
        font.outlined(),
        usize::MAX - all,
    );
    out.push_str(&more);
    out
}

/// Whether `data` is a Type 1 program as FreeType tells one apart: its header, bare or
/// in a PFB's first segment.
fn type1_by_its_bytes(data: &[u8]) -> bool {
    let header = |d: &[u8]| d.starts_with(b"%!PS-AdobeFont") || d.starts_with(b"%!FontType");
    header(data) || data.starts_with(&[0x80, 0x01]) && data.get(6..).is_some_and(header)
}

/// What was read, as what can be held the same: a font's digest and its id, or why it
/// wasn't.
fn said(read: &Result<Font, Error>) -> Result<String, Error> {
    match read {
        Ok(font) => Ok(digest(font)),
        Err(error) => Err(error.clone()),
    }
}

/// `font` read over `objects`, every promise of a reading held: it never panics; the
/// budget never rises, and what's taken off it is the font's cost, or nothing where
/// it fails; a sink changes nothing; `Fonts` read it the same, twice, charged alike;
/// smaller caps do no more work; a `Streams` that gives more than it's asked is too
/// large; and it fails only for what was made to fail, as it was made to, telling nothing
/// after it. `by_number` is what refers to it, where the file has one. Whether it read.
fn a_font<'o, O>(
    objects: O,
    font: &'o Object,
    by_number: Option<&'o Object>,
    failing_is_all: bool,
) -> bool
where
    O: Resolve<'o> + Streams + Copy,
{
    let Some(dict) = font.as_dict() else {
        return false;
    };
    let limits = Limits::DEFAULT;
    let asked = Cell::new(0);
    let counting = Counting {
        objects,
        asked: &asked,
    };
    let mut budget = BUDGET;
    let read = Font::read(counting, dict, &limits, &mut budget, &mut ());
    let work = asked.get();
    match &read {
        Ok(read) => assert_eq!(BUDGET - budget, read.cost()),
        Err(_) => assert_eq!(budget, BUDGET),
    }
    if failing_is_all {
        // These objects and streams don't fail, so only the budget can stop it.
        assert!(matches!(&read, Ok(_) | Err(Error::TooLarge)), "{read:?}");
    }
    let first = said(&read);

    // The same with a sink: what's read, what it costs and what's looked up.
    asked.set(0);
    let mut told: Vec<(Place, Fault)> = Vec::new();
    let mut kept = BUDGET;
    let with_sink = Font::read(counting, dict, &limits, &mut kept, &mut told);
    assert_eq!(said(&with_sink), first);
    assert_eq!((kept, asked.get()), (budget, work));
    for (_, fault) in &told {
        assert!(!fault.to_string().is_empty());
    }
    // A predefined CMap the build hasn't, never in a build that has the bundle, and the
    // codes it would have read guessed.
    let not_built = told.iter().any(|(_, f)| *f == Fault::PredefinedNotBuilt);
    assert!(!not_built || !cfg!(feature = "predefined-cmaps"));
    if not_built && let Ok(font) = &with_sink {
        assert!(font.approx().codes);
    }
    // A program refused for a cap is a Type 1 program, whose scan alone has one that
    // refuses it, and it's what the font says of its program.
    let mut capped = false;
    for (place, _) in told.iter().filter(|(_, f)| *f == Fault::ProgramCapped) {
        let data = place.object.and_then(|id| objects.decoded(id, BUDGET).ok());
        assert!(
            data.is_some_and(|data| type1_by_its_bytes(&data)),
            "{place:?}"
        );
        capped = true;
    }
    if let Ok(font) = &with_sink {
        assert_eq!(capped, font.program() == crate::FontProgram::Capped);
    }

    // Written in the resources, read anew each time and charged each time; by number,
    // once, and charged each time.
    let fonts = Fonts::default();
    let (mut one, mut two) = (BUDGET, BUDGET);
    let once = fonts.get(objects, font, &limits, &mut one);
    let twice = fonts.get(objects, font, &limits, &mut two);
    assert_eq!(one, two);
    let digests = |got: &Result<Option<Arc<Font>>, Error>| match got {
        Ok(font) => Ok(font.as_deref().map(digest)),
        Err(error) => Err(error.clone()),
    };
    assert_eq!(digests(&once), digests(&twice));
    if let Some(by_number) = by_number {
        let (mut one, mut two) = (BUDGET, BUDGET);
        let once = fonts.get(objects, by_number, &limits, &mut one);
        let twice = fonts.get(objects, by_number, &limits, &mut two);
        assert_eq!(one, two);
        if let (Ok(Some(once)), Ok(Some(twice))) = (&once, &twice) {
            assert!(Arc::ptr_eq(once, twice));
        }
        assert_eq!(digests(&once).is_ok(), digests(&twice).is_ok());
    }

    // Smaller caps and a smaller budget: no more work, and a font or too large.
    let mut small = limits;
    small.cmaps = 1;
    small.content.depth = 2;
    small.content.operands = 4;
    small.content.values = 8;
    asked.set(0);
    let mut budget = 16;
    let under = Font::read(counting, dict, &small, &mut budget, &mut ());
    assert!(asked.get() <= work, "{} past {work}", asked.get());
    if let Ok(under) = &under {
        digest(under);
    }
    if failing_is_all {
        assert!(matches!(&under, Ok(_) | Err(Error::TooLarge)), "{under:?}");
    }

    // Streams that give past what's asked.
    let gave = Cell::new(false);
    let overgiving = Overgiving {
        objects,
        asked: &gave,
    };
    let mut budget = BUDGET;
    let over = Font::read(overgiving, dict, &limits, &mut budget, &mut ());
    if gave.get() && !matches!(&over, Err(Error::Unreadable(_))) {
        assert_eq!(over.err(), Some(Error::TooLarge));
        assert_eq!(budget, BUDGET);
    }

    // Made to fail, each way: the first object asked, and the first stream; a stream
    // through a filter the reader doesn't decode too, and one a reader holds no data of
    // and has nothing to read from, which only a stream's data is, as a resolver never
    // says `Why::Filter` or `Why::Detached`.
    let objects_fail =
        [Why::Damaged, Why::Capped, Why::Locked, Why::Source].map(|why| (why, false));
    let streams_fail = [
        Why::Damaged,
        Why::Limit,
        Why::Capped,
        Why::Locked,
        Why::Source,
        Why::Filter,
        Why::Detached,
    ];
    for (why, streams) in objects_fail
        .into_iter()
        .chain(streams_fail.map(|why| (why, true)))
    {
        let failed = Cell::new(false);
        let failing = FailingFirst {
            objects,
            why,
            streams,
            failed: &failed,
        };
        let mut budget = BUDGET;
        let mut failing_told: Vec<(Place, Fault)> = Vec::new();
        let read = Font::read(failing, dict, &limits, &mut budget, &mut failing_told);
        if read.is_err() {
            assert_eq!(budget, BUDGET);
        }
        // What fails is the whole answer: up to it the reading is the one that didn't
        // fail, and it tells nothing after it. Not so where it's read as none.
        if !matches!(why, Why::Damaged | Why::Filter) {
            assert!(
                told.starts_with(&failing_told),
                "{why:?}: {failing_told:?} against {told:?}"
            );
        }
        if failing_is_all {
            match &read {
                // What fails first is the first thing that's met.
                Err(Error::Unreadable(unreadable)) => {
                    assert_eq!(unreadable.why, why);
                    assert!(!matches!(why, Why::Damaged | Why::Filter), "{why:?}");
                    assert!(failed.get());
                }
                Err(_) => {}
                // A stream that's damaged, or through a filter the reader doesn't
                // decode, is read as none, and so is an object that's damaged.
                Ok(_) => assert!(!failed.get() || matches!(why, Why::Damaged | Why::Filter)),
            }
            // A stream past the budget is too large, and one past a cap of what gives
            // it the caller's to know, as an object past one is.
            if streams && failed.get() {
                match why {
                    Why::Limit => assert!(matches!(&read, Err(Error::TooLarge)), "{read:?}"),
                    Why::Capped => assert!(
                        matches!(&read, Err(Error::Unreadable(u)) if u.why == why),
                        "{read:?}"
                    ),
                    _ => {}
                }
            }
        }
    }
    first.is_ok()
}

/// `data` as a font's parts ([`FONT_PART`], [`objects_of`]): the font each makes, every
/// promise of a reading held ([`a_font`]). Whether it read.
pub fn fonts(data: &[u8]) -> bool {
    let objects = objects_of(data);
    let Some(font) = objects.get(&ObjectId::new(1, 0)) else {
        return false;
    };
    a_font(Map(&objects), font, objects.get(&ObjectId::new(9, 0)), true)
}

/// The objects of `ids` that are dictionaries with a `/Subtype`, read over `objects`, a
/// file's, every promise of a reading held but that it only fails as it's made to, as
/// a file's objects can say anything. How many read.
pub fn over<'o, O>(objects: O, ids: impl Iterator<Item = ObjectId>) -> usize
where
    O: Resolve<'o> + Streams + Copy,
{
    let mut fonts = 0;
    let mut tried = 0;
    for id in ids {
        let Ok(Some(object)) = objects.try_resolve(id) else {
            continue;
        };
        if !object.as_dict().is_some_and(|dict| dict.has(b"Subtype")) {
            continue;
        }
        fonts += usize::from(a_font(objects, object, None, false));
        tried += 1;
        if tried == NUMBERS {
            break;
        }
    }
    fonts
}

/// `data` as a page's content, drawn with the fonts its parts make ([`objects_of`]),
/// Helvetica, two forms, one of `data` too, which may draw itself, and one with no
/// resources of its own, and two graphics states, one picking a font by its name and
/// one by a reference, which PDFium reads as the stock Helvetica: what it shows
/// ([`shown`](crate::shown)), every promise held. It never panics. The budget never
/// rises. It's the same twice, the second time over the fonts the first read and kept,
/// and charged alike. Nothing fails to read for the caller's reason, as nothing is
/// made to. Under a cap of fewer forms, under the content's caps made small and within
/// a smaller budget, what's shown is what's shown under the defaults, less some. How
/// many strings it shows.
pub fn shown(data: &[u8]) -> usize {
    let mut objects = objects_of(data);
    let resources = Object::from(dict! {
        "Font" => dict! {
            "F1" => Object::reference(9), "F2" => Object::reference(1),
            "H" => dict! {
                "Subtype" => Object::name("Type1"), "BaseFont" => Object::name("Helvetica"),
            },
        },
        "XObject" => dict! { "X" => Object::reference(7), "Y" => Object::reference(8) },
        "ExtGState" => dict! {
            "G" => dict! { "Font" => vec![Object::name("F2"), Object::from(9)] },
            "R" => dict! { "Font" => vec![Object::reference(9), Object::from(9)] },
        },
    });
    let form = |resources: Option<&Object>, content: &[u8]| {
        let mut dict = dict! { "Subtype" => Object::name("Form") };
        if let Some(resources) = resources {
            dict.set("Resources", resources.clone());
        }
        Object::from(Stream::new(dict, content.to_vec()))
    };
    objects.insert(ObjectId::new(7, 0), form(Some(&resources), data));
    objects.insert(ObjectId::new(8, 0), form(None, b"(Y) Tj /F2 9 Tf (y) Tj"));
    let file = Map(&objects);
    let content = Concat::new(&[data]);
    let walk = |fonts: &Fonts, limits: &Limits, budget: usize| {
        let mut left = budget;
        let read = crate::shown(
            file,
            fonts,
            Some(&resources),
            &content,
            &ResCx::none(),
            limits,
            &mut left,
        );
        assert!(left <= budget);
        for unread in &read.unread {
            let failed = match unread {
                Unread::Form { error, .. }
                | Unread::Font { error, .. }
                | Unread::State { error, .. } => error,
                _ => continue,
            };
            assert_eq!(failed, &Error::TooLarge, "nothing was made to fail");
        }
        let _ = read.text();
        (said_of(&read), budget - left)
    };
    let limits = Limits::DEFAULT;
    let fonts = Fonts::default();
    let (read, spent) = walk(&fonts, &limits, SHOWN_BUDGET);
    assert_eq!(walk(&fonts, &limits, SHOWN_BUDGET), (read.clone(), spent));
    assert_eq!(
        walk(&Fonts::default(), &limits, SHOWN_BUDGET),
        (read.clone(), spent)
    );
    let mut small = Limits::DEFAULT;
    small.forms = 2;
    small.form_uses = 8;
    small.content.operands = 4;
    small.content.depth = 4;
    small.content.values = 16;
    let fewer = [
        walk(&fonts, &small, SHOWN_BUDGET).0,
        walk(&fonts, &limits, spent / 2).0,
    ];
    for fewer in fewer {
        assert!(
            within(&fewer.0, &read.0),
            "{} of {}",
            fewer.0.len(),
            read.0.len()
        );
    }
    read.0.len()
}

/// What content is read within here ([`shown`]): a form drawn within itself twice is
/// drawn until this is spent, which a run spends in a few milliseconds.
const SHOWN_BUDGET: usize = 1 << 16;

/// A string shown, as what can be held the same across fonts read apart: its bytes,
/// its form, where it's written, its font's object and its text.
type Said = (
    Vec<u8>,
    Option<ObjectId>,
    Range<usize>,
    Option<ObjectId>,
    Option<String>,
);

/// What's shown, said ([`Said`]), and what wasn't read.
fn said_of(shown: &Shown) -> (Vec<Said>, Vec<Unread>) {
    let strings = shown
        .strings
        .iter()
        .map(|s| (s.bytes.clone(), s.form, s.at.clone(), s.font.id(), s.text()));
    (strings.collect(), shown.unread.clone())
}

/// Whether `fewer` is `all` with none or some of its strings left out, the rest in
/// the same order. A string left out isn't shown where something wasn't read, which
/// shows nothing in its place; a string shown is the same, as each is shown by what
/// was read before it.
fn within(fewer: &[Said], all: &[Said]) -> bool {
    let mut all = all.iter();
    fewer.iter().all(|said| all.any(|other| other == said))
}

/// A page's content, as the fuzzer's first inputs: each operator the walk reads, the
/// forms it draws, and a font's parts after it.
fn content_fixtures() -> Vec<Vec<u8>> {
    vec![
        b"BT /F1 12 Tf (Hello) Tj [(W) -20 (orld)] TJ 14 TL (a) ' 1 2 (b) \" ET".to_vec(),
        b"/H 9 Tf q /F2 9 Tf (A) Tj Q (A) Tj Q Q /Y Do /F9 9 Tf (A) Tj".to_vec(),
        b"/H 9 Tf (A) Tj /X Do".to_vec(),
        b"/G gs (A) Tj q /R gs (A) Tj Q (A) Tj /H 9 Tf (A) Tj /G gs [(A) 9 (B)] TJ".to_vec(),
        parts(&[
            b"<< /Subtype /Type1 /BaseFont /Courier /ToUnicode 3 0 R >>",
            b"",
            b"1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfchar <41> <0042> endbfchar",
            b"/F2 9 Tf (A) Tj /Y Do",
        ]),
    ]
}

/// A font's parts as one input.
fn parts(parts: &[&[u8]]) -> Vec<u8> {
    parts.join(FONT_PART)
}

/// A font of each kind, as the fuzzer's first inputs.
fn font_fixtures() -> Vec<Vec<u8>> {
    let to_unicode: &[u8] = b"/CIDInit /ProcSet findresource begin\n\
        1 begincodespacerange <00> <FF> endcodespacerange\n\
        2 beginbfchar <41> <0041> <42> /B endbfchar\n\
        2 beginbfrange <61> <7a> <0061> <80> <82> [<0041> <0042> <D83DDE00>] endbfrange\n";
    let cmap: &[u8] = b"/CIDInit /ProcSet findresource begin begincmap /WMode 1 def\n\
        2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange\n\
        1 begincidchar <41> 500 endcidchar\n\
        1 begincidrange <8140> <8142> 1000 endcidrange endcmap\n";
    let descendant: &[u8] = b"<< /Subtype /CIDFontType0 /W [1 [500 600] 10 20 700 15 [900]] \
        /W2 [1 [-900 300 800] 5 9 -800 250 700] /DW 900 /DW2 [900 -1100] \
        /FontDescriptor << /FontBBox [0 -200 1000 900] /Ascent 900 /Descent -200 >> \
        /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) >> >>";
    vec![
        parts(&[b"<< /Subtype /Type1 /BaseFont /Helvetica /Encoding << \
            /BaseEncoding /WinAnsiEncoding /Differences [1 /A /B 300 /C /uni0416] >> >>"]),
        parts(&[b"<< /Subtype /Type1 /BaseFont /ABCDEF+Symbol /Encoding /WinAnsiEncoding >>"]),
        parts(&[
            b"<< /Subtype /TrueType /BaseFont /Foo /FirstChar 65 /LastChar 66 /Widths 10 0 R \
              /FontDescriptor 11 0 R /ToUnicode 3 0 R >>\n\
              [600 700.9]\n\
              << /MissingWidth 500 /FontBBox [0 -200 1000 800] /Ascent 800 /Descent -200 >>",
            b"",
            to_unicode,
        ]),
        parts(&[
            b"<< /Subtype /Type3 /FontMatrix [0.001 0 0 0.001 0 0] /FontBBox [0 0 1000 1000] \
              /FirstChar 97 /Widths [500] /Encoding << /Differences [97 /a /b 65 /c] >> \
              /CharProcs << /a 4 0 R /b 4 0 R /c 6 0 R >> >>",
            b"",
            b"",
            b"100 0 0 0 100 100 d1 0 0 100 100 re f",
            b"",
            b"100 0 d0 q Q",
        ]),
        parts(&[
            b"<< /Subtype /Type0 /Encoding /Identity-V /DescendantFonts [2 0 R] >>",
            descendant,
        ]),
        parts(&[
            b"<< /Subtype /Type0 /Encoding 4 0 R /DescendantFonts [2 0 R] /ToUnicode 3 0 R >>",
            descendant,
            to_unicode,
            cmap,
            b"",
            b"/Identity-H usecmap 1 begincidchar <0041> 7 endcidchar",
        ]),
        parts(&[
            b"<< /Subtype /Type0 /Encoding /90ms-RKSJ-V /DescendantFonts [2 0 R] >>",
            descendant,
        ]),
        parts(&[
            b"<< /Subtype /Type0 /Encoding /UniJIS-UTF16-H /DescendantFonts [2 0 R] >>",
            descendant,
        ]),
        parts(&[
            b"<< /Subtype /Type0 /Encoding /Identity-H /DescendantFonts [2 0 R] >>",
            b"<< /Subtype /CIDFontType2 /W [0 2147483647 500 0 4294967295 7] >>",
        ]),
    ]
    .into_iter()
    .chain(program_fixtures())
    .collect()
}

/// A font over each of the programs built by hand, simple and a CIDFont's.
fn program_fixtures() -> Vec<Vec<u8>> {
    let names = OpenType {
        post_names: true,
        cmap: false,
    };
    let programs: [(&str, &str, Vec<u8>); 17] = [
        ("TrueType", "FontFile2", truetype_program()),
        ("Type1", "FontFile3", cff_program()),
        ("Type1", "FontFile3", cid_cff_program()),
        ("Type1", "FontFile3", opentype_program(names)),
        ("Type1", "FontFile3", opentype_cid_program(names)),
        ("Type1", "FontFile", type1_program()),
        ("Type1", "FontFile3", heavy_cff_program(4)),
        ("Type1", "FontFile3", moving_cff_program(3)),
        ("Type1", "FontFile3", long_cff_program(8)),
        ("TrueType", "FontFile2", composite_bomb_program()),
        ("TrueType", "FontFile2", composite_of_program(1, 1, 1)),
        ("Type1", "FontFile3", loca_past_end_program()),
        ("MMType1", "FontFile", type1_integers_program(8)),
        ("Type1", "FontFile", type1_accents_program(4, 4)),
        ("Type1", "FontFile", type1_lookups_program(16)),
        ("Type1", "FontFile", doubled_sign_type1_program()),
        ("Type1", "FontFile", rolling_type1_program()),
    ];
    let mut out = Vec::new();
    for (subtype, key, program) in programs {
        for flags in [32, 4] {
            let font = format!(
                "<< /Subtype /{subtype} /BaseFont /Fix /FirstChar 32 /Widths [250 300] \
                 /Encoding << /Differences [65 /A /g] >> /FontDescriptor \
                 << /Flags {flags} /{key} 5 0 R >> /ToUnicode 3 0 R >>"
            );
            out.push(parts(&[font.as_bytes(), b"", b"", b"", &program, b""]));
        }
        let descendant = format!(
            "<< /Subtype /CIDFontType2 /BaseFont /Fix /W [1 [500]] /CIDToGIDMap 6 0 R \
             /FontDescriptor << /Flags 4 /{key} 5 0 R >> >>"
        );
        out.push(parts(&[
            b"<< /Subtype /Type0 /Encoding /Identity-H /DescendantFonts [2 0 R] >>",
            descendant.as_bytes(),
            b"",
            b"",
            &program,
            b"\0\x01\0\x02",
        ]));
    }
    out
}

/// What the fuzzer begins from, and the tests run on: Type 1 programs, as written
/// and in a PFB's segments, integers in a row, names of each kind, and a font of each
/// kind in its parts.
pub fn fixtures() -> Vec<Vec<u8>> {
    let program = type1_program();
    let eexec = program.windows(6).position(|w| w == b"eexec\n").unwrap() + 6;
    let segment = |tag: u8, data: &[u8]| {
        let mut out = vec![0x80, tag];
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    };
    let pfb = [
        segment(1, &program[..eexec]),
        segment(2, &program[eexec..]),
        vec![0x80, 3],
    ]
    .concat();
    let hex: Vec<u8> = (program[eexec..].iter())
        .flat_map(|b| format!("{b:02x}").into_bytes())
        .collect();
    vec![
        program.clone(),
        pfb,
        [&program[..eexec], &hex[..]].concat(),
        type1_integers_program(8),
        b"%!PS-AdobeFont-1.0: F\n/N { { { } } } def\n(a(b)\\)) <41> << >> [ ] % c\n\
          8#17 2#1 currentfile eexec\n"
            .to_vec(),
        b"A a20 f_i a.alt uni0416 uni00410042 u1F600 uniD800 .notdef space uni\xe2\x82\xac\xe2\x82\xac\xe2\x82\xac\xe2\x82\xac\n\
          Helvetica ABCDEF+ArialMT arialmt Arial,Bold TimesNewRomanPS-BoldItalicMT SymbolMT\n\
          StandardEncoding WinAnsiEncoding MacRomanEncoding MacExpertEncoding\n\
          90ms-RKSJ-H UniJIS-UTF16-V H V GBK2K-H CNS-EUC-H Identity-H Adobe-Japan1-UCS2\n"
            .to_vec(),
    ]
    .into_iter()
    .chain(font_fixtures())
    .chain(content_fixtures())
    .collect()
}

/// The tables an input's bytes may be spliced into [`truetype_program`] as ([`embed`]),
/// in place of its own of the tag or beside them: its eight; `name`, `OS/2` and the
/// hinting tables, which it hasn't; and those that say a program's outlines aren't
/// TrueType's.
#[cfg(feature = "embed")]
pub const TABLES: [[u8; 4]; 18] = [
    *b"cmap", *b"glyf", *b"head", *b"hhea", *b"hmtx", *b"loca", *b"maxp", *b"post", *b"name",
    *b"OS/2", *b"cvt ", *b"fpgm", *b"prep", *b"CFF ", *b"CFF2", *b"EBDT", *b"CBDT", *b"sbix",
];

/// How many bytes before an input's program say how it's read ([`embed`]).
#[cfg(feature = "embed")]
pub const HEADER: usize = 4;

/// How many of an input's last bytes are read as a text ([`embed`]): drawn, and the
/// text of the glyph it names.
#[cfg(feature = "embed")]
const TAIL: usize = 32;

/// How many of the characters a program's `cmap` maps are drawn ([`embed`]): those of a
/// few of its segments, and few enough for a run to try many programs.
#[cfg(feature = "embed")]
const MAPPINGS: usize = 64;

/// What's drawn in every program ([`embed`]): the fixture programs' characters (Latin,
/// a composite of two glyphs and one of composites among them, Hebrew, Arabic and
/// Chinese letters, and one past the Basic Multilingual Plane), and what a program has
/// no glyph for, or may have: a noncharacter, a control character, a line feed and a
/// variation selector.
#[cfg(feature = "embed")]
const DRAWN: &str =
    "ABC g Ag H\u{C4}\u{1DE}\u{A8}\u{AF} שלום سلام 中文世界 𝄞 \u{FFFF}\u{0}\n\u{FE0F}";

/// What a font written for embedding is read back within ([`embed`]): more than any
/// program an input can be cut to.
#[cfg(feature = "embed")]
const EMBEDDED_BUDGET: usize = 1 << 26;

/// The program `data` is ([`embed`]), and the face of it asked for. Past its [`HEADER`],
/// where its first byte is even, its bytes, whole, face 0 to 3 as its second byte says;
/// where it's odd, its bytes as one table of [`truetype_program`]'s, the one of
/// [`TABLES`] its second byte picks, put in its place or among them, so that what's
/// mutated is one table of a program whose others read.
#[cfg(feature = "embed")]
fn embedded(data: &[u8]) -> (Vec<u8>, u32) {
    let (mode, pick) = (data.first().copied(), data.get(1).copied().unwrap_or(0));
    let rest = data.get(HEADER..).unwrap_or_default();
    if mode.unwrap_or(0) % 2 == 0 {
        return (rest.to_vec(), u32::from(pick % 4));
    }
    let tag = TABLES[usize::from(pick) % TABLES.len()];
    let mut tables = crate::fixture::truetype_tables();
    match tables.iter_mut().find(|table| table.0 == tag) {
        Some(table) => table.1 = rest.to_vec(),
        None => tables.push((tag, rest.to_vec())),
    }
    (crate::fixture::sfnt(&mut tables), 0)
}

/// `data` as a TrueType program embedded ([`crate::embed`]), as [`embedded`] reads it:
/// opened, or refused, saying why. Opened: its widths whole thousandths, none past its
/// glyphs, and its heights finite. A fixed text, the input's last [`TAIL`] bytes as
/// text and the first characters its `cmap` maps drawn in it: each character its
/// glyph's code, or, where it has none, refused where it is, nothing of the text kept,
/// and what's after it drawn on; the glyphs kept the same whatever order the texts are
/// drawn in. The font of what's drawn written under each choice of
/// [`Options`](crate::embed::Options), each twice the same, and refused only where the
/// program's licence restricts it and the right isn't given, as every glyph of a
/// program opened is cut: the right changing nothing else, a `/CIDSet` one more object
/// and nothing else, and a program whose licence asks to be embedded whole the same cut
/// or not. Each font written read back by [`Font::read`] with no fault: each glyph's
/// width the program's, and its text the one the [`Glyphs`](crate::embed::Glyphs)
/// kept, so each string's text its glyphs'; with `programs`, its program read, and
/// each glyph's box the same cut as whole, and as the program given reads it, where it
/// reads one. Then the glyph the input's third and fourth bytes name drawn too, its
/// text the tail: refused where it's past the program's glyphs, else read back so. How
/// many fonts were written and read back.
///
/// # Errors
///
/// Why the program wasn't opened.
#[cfg(feature = "embed")]
pub fn embed(data: &[u8]) -> Result<usize, crate::embed::Error> {
    use crate::embed::{Error as Refused, Glyphs, Level, Options, Program, write};

    let (bytes, index) = embedded(data);
    let program = match Program::open(bytes, index) {
        Ok(program) => program,
        Err(error) => {
            assert!(!error.to_string().is_empty());
            return Err(error);
        }
    };
    let count = program.glyph_count();
    assert!(count > 0);
    for glyph in [0, 1, count - 1, count, u16::MAX] {
        let width = program.width(glyph);
        assert!(width >= 0.0 && width.fract() == 0.0, "{glyph}: {width}");
        assert!(glyph < count || width == 0.0, "{glyph}: {width}");
    }
    let metrics = program.metrics();
    let bbox = metrics.bbox.unwrap_or_default();
    for v in [
        metrics.ascent,
        metrics.descent,
        bbox.x0,
        bbox.y0,
        bbox.x1,
        bbox.y1,
    ] {
        assert!(v.is_finite(), "{metrics:?}");
    }

    let tail = String::from_utf8_lossy(&data[data.len().saturating_sub(TAIL)..]);
    let mapped = mapped(&program, index);
    let mut glyphs = Glyphs::new();
    let mut strings = Vec::new();
    for text in [DRAWN, &*tail, &*mapped] {
        strings.extend(drawn(&program, &mut glyphs, text));
    }
    // Drawn in another order, the same glyphs and texts.
    let mut again = Glyphs::new();
    for text in [&*mapped, &*tail, DRAWN] {
        drawn(&program, &mut again, text);
    }
    assert_eq!(again.texts, glyphs.texts);

    let embedding = program.embedding();
    let restricted = embedding.level == Level::Restricted || embedding.bitmaps_only;
    let options = |bits: usize| Options {
        cid_set: bits & 1 != 0,
        whole: bits & 2 != 0,
        licensed: bits & 4 != 0,
    };
    let fonts: [Result<Vec<Object>, Refused>; 8] = std::array::from_fn(|bits| {
        let options = options(bits);
        let written = write(&program, &glyphs, &options);
        assert_eq!(write(&program, &glyphs, &options), written, "{options:?}");
        assert_eq!(embedding.allows(&options), !restricted || options.licensed);
        match &written {
            Err(Refused::Restricted(refused)) => {
                assert!(restricted && !options.licensed, "{options:?}");
                assert_eq!(*refused, embedding);
            }
            Err(other) => panic!("{options:?}: {other:?}"),
            Ok(_) => assert!(!restricted || options.licensed, "{options:?}"),
        }
        written
    });
    for bits in 0..8 {
        // The right to embed a restricted program changes nothing else.
        if bits & 4 == 0 && !matches!(fonts[bits], Err(Refused::Restricted(_))) {
            assert_eq!(fonts[bits], fonts[bits | 4]);
        }
        // A program whose licence asks to be embedded whole is, either way.
        if bits & 2 == 0 && !embedding.subsetting {
            assert_eq!(fonts[bits], fonts[bits | 2]);
        }
        // A `/CIDSet` is one more object, its descriptor naming it, and nothing else.
        if bits & 1 == 0 {
            match (&fonts[bits], &fonts[bits | 1]) {
                (Ok(without), Ok(with)) => with_cid_set(without, with),
                (without, with) => assert_eq!(without.as_ref().err(), with.as_ref().err()),
            }
        }
    }

    let mut read = 0;
    let mut cut_and_whole = [None, None];
    for (bits, font) in fonts.iter().enumerate() {
        let Ok(objects) = font else {
            continue;
        };
        let font = read_back(&program, &glyphs, &strings, objects);
        if bits == 0 {
            digest(&font);
        }
        if bits < 4 && bits & 1 == 0 {
            cut_and_whole[bits / 2] = Some(font);
        }
        read += 1;
    }
    // Each glyph's box the same cut as whole, with `programs`: the subsetter's glyphs
    // the program's, by the reader's own outlines. Without, neither has one.
    if let [Some(cut), Some(whole)] = &cut_and_whole {
        for &glyph in glyphs.texts.keys() {
            let code = u32::from(glyph);
            assert_eq!(cut.char_box(code), whole.char_box(code), "glyph {glyph}");
        }
    }
    // And the program's as given, where it draws one with an area: so a table the cut
    // takes from it and writes wrong shows, which it doesn't between two cuts. A glyph
    // drawn as nothing, or none, may be one of the program's whose bytes the cut keeps
    // only as far as they're a glyph's, which the reader, as FreeType, takes otherwise.
    #[cfg(feature = "programs")]
    if let (Some(cut), Ok(written)) = (&cut_and_whole[0], &fonts[6])
        && let Some(given) = given(&program, index, written)
    {
        for &glyph in glyphs.texts.keys() {
            let code = u32::from(glyph);
            let given = given
                .char_box(code)
                .filter(|b| *b != crate::Rect::default());
            if let Some(given) = given {
                assert_eq!(cut.char_box(code), Some(given), "glyph {glyph} given");
            }
        }
    }

    // A glyph the input names, its tail its text; refused where it's past the glyphs.
    let named = u16::from_be_bytes([data.get(2), data.get(3)].map(|b| b.copied().unwrap_or(0)));
    glyphs.insert(named, &tail);
    let right = Options {
        licensed: true,
        ..Options::default()
    };
    let written = write(&program, &glyphs, &right);
    match &written {
        _ if named >= count => {
            let (&glyph, _) = glyphs.texts.last_key_value().unwrap();
            assert_eq!(written, Err(Refused::Glyph { glyph, count }));
        }
        Ok(objects) => {
            read_back(&program, &glyphs, &strings, objects);
            read += 1;
        }
        Err(other) => panic!("glyph {named}: {other:?}"),
    }
    Ok(read)
}

/// The font `written`, `program` written whole, is with its `/FontFile2` the program as
/// it's given, read: none where it's face `index` of a collection, or its `loca`'s
/// format is one read-fonts, and so this crate's reader, reads otherwise than the
/// subsetter and FreeType (any but 0 or 1), as then it's the cut that's read as they
/// read it.
#[cfg(all(feature = "embed", feature = "programs"))]
fn given(program: &crate::embed::Program, index: u32, written: &[Object]) -> Option<Font> {
    use skrifa::raw::{FontRef, TableProvider};

    let font = FontRef::new(&program.data).ok()?;
    let format = font.head().ok()?.index_to_loc_format();
    if index != 0 || !matches!(format, 0 | 1) {
        return None;
    }
    let mut objects = written.to_vec();
    let length = program.data.len() as i64;
    let file = Stream::new(dict! { "Length1" => length }, program.data.to_vec());
    objects[3] = Object::from(file);
    let objects = numbered(&objects);
    let dict = objects[&ObjectId::new(1, 0)].as_dict()?;
    let mut budget = EMBEDDED_BUDGET;
    let read = Font::read(Map(&objects), dict, &Limits::DEFAULT, &mut budget, &mut ());
    read.ok()
}

/// The characters `program`'s `cmap`, face `index`'s, maps first: at most [`MAPPINGS`].
#[cfg(feature = "embed")]
fn mapped(program: &crate::embed::Program, index: u32) -> String {
    use skrifa::MetadataProvider;
    use skrifa::raw::FontRef;

    let Ok(font) = FontRef::from_index(&program.data, index) else {
        return String::new();
    };
    let mappings = font.charmap().mappings().take(MAPPINGS);
    mappings.filter_map(|(c, _)| char::from_u32(c)).collect()
}

/// `text` drawn in `program`, its glyphs kept in `glyphs`: each run of characters the
/// program has glyphs for, its codes each character's glyph; each character it hasn't
/// refused where it is, as the first of what's left, nothing of that kept. Each run's
/// codes.
#[cfg(feature = "embed")]
fn drawn(
    program: &crate::embed::Program,
    glyphs: &mut crate::embed::Glyphs,
    text: &str,
) -> Vec<Vec<u8>> {
    let mut strings = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let kept = glyphs.texts.clone();
        let refused = match glyphs.add(program, rest) {
            Ok(codes) => {
                assert_eq!(program.codes(rest).as_ref(), Ok(&codes));
                let pairs = codes.as_chunks::<2>().0;
                assert_eq!(pairs.len(), rest.chars().count(), "a code a character");
                for (c, &code) in rest.chars().zip(pairs) {
                    let glyph = u16::from_be_bytes(code);
                    assert_eq!(program.glyph(c), Some(glyph));
                    let text = glyphs.texts.get(&glyph).map(String::as_str);
                    assert!(text.is_some_and(|text| text <= &*c.encode_utf8(&mut [0; 4])));
                }
                strings.push(codes);
                break;
            }
            Err(refused) => refused,
        };
        assert_eq!(glyphs.texts, kept, "nothing of a text refused kept");
        assert_eq!(program.codes(rest), Err(refused));
        assert_eq!(rest[refused.at..].chars().next(), Some(refused.character));
        assert_eq!(program.glyph(refused.character), None);
        let (run, after) = rest.split_at(refused.at);
        if !run.is_empty() {
            strings.push(glyphs.add(program, run).expect("what's before it is drawn"));
        }
        rest = &after[refused.character.len_utf8()..];
    }
    strings
}

/// `with`, a font written with a `/CIDSet`, holds what `without` does, the same font
/// written without one, and the `/CIDSet` after it, its descriptor naming it.
#[cfg(feature = "embed")]
fn with_cid_set(without: &[Object], with: &[Object]) {
    assert_eq!(with.len(), without.len() + 1);
    for (i, (with, without)) in with.iter().zip(without).enumerate() {
        if i != 2 {
            assert_eq!(with, without, "object {i}");
        }
    }
    let mut descriptor = without[2].as_dict().unwrap().clone();
    descriptor.push("CIDSet", Object::reference(without.len() as u32));
    assert_eq!(with[2].as_dict(), Some(&descriptor));
    assert!(with.last().and_then(Object::as_stream).is_some());
}

/// `objects`, a font as [`write`](crate::embed::write) gives it, numbered from 1: each
/// reference `(i, 0)` among them made `(i + 1, 0)`.
#[cfg(feature = "embed")]
fn numbered(objects: &[Object]) -> BTreeMap<ObjectId, Object> {
    fn renumber(object: &mut Object) {
        match object.value_mut() {
            pdf_objects::Value::Ref(id) => *id = ObjectId::new(id.number + 1, 0),
            pdf_objects::Value::Array(items) => items.iter_mut().for_each(renumber),
            pdf_objects::Value::Dict(dict) => dict.iter_mut().for_each(|(_, o)| renumber(o)),
            pdf_objects::Value::Stream(stream) => {
                stream.dict.iter_mut().for_each(|(_, o)| renumber(o));
            }
            _ => {}
        }
    }
    (1..)
        .zip(objects)
        .map(|(n, object)| {
            let mut object = object.clone();
            renumber(&mut object);
            (ObjectId::new(n, 0), object)
        })
        .collect()
}

/// `objects`, a font written of `glyphs` in `program`, read: without a fault; each
/// glyph's width `program`'s, and its text the one `glyphs` keeps, where it keeps one,
/// but U+FFFF alone, which PDFium, and so this crate's reader, takes for no text; each
/// of `strings`, codes drawn, the texts of its glyphs; its program read, with `programs`.
#[cfg(feature = "embed")]
fn read_back(
    program: &crate::embed::Program,
    glyphs: &crate::embed::Glyphs,
    strings: &[Vec<u8>],
    objects: &[Object],
) -> Font {
    let objects = numbered(objects);
    let dict = objects[&ObjectId::new(1, 0)].as_dict().unwrap();
    let mut budget = EMBEDDED_BUDGET;
    let mut told: Vec<(Place, Fault)> = Vec::new();
    let font = Font::read(
        Map(&objects),
        dict,
        &Limits::DEFAULT,
        &mut budget,
        &mut told,
    );
    let font = font.expect("a font written reads");
    assert!(told.is_empty(), "{told:?}");
    let text_of = |glyph: u16| {
        let text = glyphs.texts.get(&glyph).map(String::as_str);
        text.filter(|text| !matches!(*text, "" | "\u{FFFF}"))
    };
    for &glyph in glyphs.texts.keys() {
        let code = u32::from(glyph);
        assert_eq!(font.width(code), program.width(glyph), "glyph {glyph}");
        if let Some(text) = text_of(glyph) {
            assert_eq!(font.code_text(code).as_deref(), Some(text), "glyph {glyph}");
        }
    }
    for codes in strings {
        let pairs = codes.as_chunks::<2>().0.iter();
        let text: Option<String> = pairs
            .map(|&code| text_of(u16::from_be_bytes(code)))
            .collect();
        if let Some(text) = text {
            assert_eq!(font.text(codes), Some(text));
        }
    }
    let read = match cfg!(feature = "programs") {
        true => crate::FontProgram::Read,
        false => crate::FontProgram::None,
    };
    assert_eq!(font.program(), read);
    font
}

#[cfg(test)]
mod tests {
    use pdf_fixtures::fixture::{self, Body, Xref};

    use super::*;

    /// A mutation of `seed`, from `random`, as a fuzzer might make it.
    fn mutated(seed: &[u8], random: &mut impl FnMut() -> usize) -> Vec<u8> {
        const PIECES: [&[u8]; 32] = [
            b"[",
            b"]",
            b"<<",
            b">>",
            b"/Widths",
            b"/FirstChar",
            b"/Differences",
            b"/DescendantFonts",
            b"/ToUnicode",
            b"/UseCMap",
            b"/W",
            b"/CharProcs",
            b"/Encoding",
            b"begincodespacerange",
            b"endcidrange",
            b"3 0 R",
            b"{",
            b"}",
            b"(",
            b")",
            b"<",
            b">",
            b"[",
            b"/",
            b"%",
            b" ",
            b"\n",
            b"1 ",
            b"eexec",
            b"/CharStrings",
            b" RD ",
            b"/lenIV",
        ];
        let mut data = seed.to_vec();
        for _ in 0..1 + random() % 4 {
            let at = random() % (data.len() + 1);
            let end = match random() % 3 {
                0 => at,
                _ => data.len().min(at + 1 + random() % 8),
            };
            let piece: &[u8] = match random() % 3 {
                1 => &[],
                _ => PIECES[random() % PIECES.len()],
            };
            data.splice(at..end, piece.iter().copied());
        }
        data
    }

    fn generator(mut state: u64) -> impl FnMut() -> usize {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize
        }
    }

    #[test]
    fn mutated_seeds_keep_every_promise() {
        let mut random = generator(0x5EED);
        let mut found = 0;
        for seed in fixtures() {
            found += usize::from(scan(&seed, &Limits::DEFAULT).is_some());
            type1(&seed);
            names(&seed);
            written(&seed);
            fonts(&seed);
            shown(&seed);
            for _ in 0..50 {
                let data = mutated(&seed, &mut random);
                type1(&data);
                names(&data);
                written(&data);
                fonts(&data);
                shown(&data);
            }
        }
        assert!(found >= 4, "the seeds hold programs that read: {found}");
    }

    /// Each kind of font among the seeds reads.
    #[test]
    fn the_fonts_among_the_seeds_read() {
        for seed in font_fixtures() {
            assert!(fonts(&seed), "{}", seed.escape_ascii());
        }
    }

    /// What each page's content among the seeds shows: strings, in each font.
    #[test]
    fn the_contents_among_the_seeds_show_strings() {
        let shows: Vec<usize> = content_fixtures().iter().map(|seed| shown(seed)).collect();
        assert_eq!(shows, [5, 5, 41, 6, 3]);
    }

    /// A seed's objects as a file, each stream's data as it is; a seed that's no such
    /// thing as it is.
    fn file_of(seed: &[u8]) -> Vec<u8> {
        let bodies: Vec<(u32, Body)> = (objects_of(seed).iter())
            .map(|(id, object)| {
                let body = match object.as_stream() {
                    Some(Stream { dict, data, .. }) => {
                        Body::stream(dict.clone(), data.bytes().unwrap_or_default())
                    }
                    None => Body::Object(object.clone()),
                };
                (id.number, body)
            })
            .collect();
        fixture::file("1.7", &bodies, dict! {}, Xref::Table)
    }

    /// Each kind of font among the seeds, as a file over a reading, reads as it does in a
    /// map; and the same bytes after a mutation keep every promise.
    #[test]
    fn files_read_alike_through_a_reading_and_a_map() {
        use pdf_structure::read::{Options, Reading};

        let mut random = generator(0xF11E5);
        let limits = Limits::DEFAULT;
        for seed in font_fixtures() {
            let objects = objects_of(&seed);
            let dict = objects[&ObjectId::new(1, 0)].as_dict().unwrap();
            let mut budget = BUDGET;
            let in_map = Font::read(Map(&objects), dict, &limits, &mut budget, &mut ());

            let file = file_of(&seed);
            let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
            let dict = reading.get(ObjectId::new(1, 0)).unwrap().as_dict().unwrap();
            let mut read = BUDGET;
            let in_file = Font::read(&reading, dict, &limits, &mut read, &mut ());
            assert_eq!(said(&in_file), said(&in_map), "{}", seed.escape_ascii());
            assert_eq!(read, budget);
            assert!(
                over(&reading, reading.ids()) >= 1,
                "{}",
                seed.escape_ascii()
            );

            for _ in 0..20 {
                let data = mutated(&file, &mut random);
                if let Ok(reading) = Reading::open(&data[..], &Options::DEFAULT) {
                    over(&reading, reading.ids());
                }
            }
        }
    }

    /// A font whose Type 1 program is past `type1_work` keeps every promise, its
    /// program refused for the cap and told so, and the program itself keeps the scan's.
    #[cfg(feature = "programs")]
    #[test]
    fn a_program_past_its_cap_keeps_every_promise() {
        let program = type1_lookups_program(3000);
        let font: &[u8] = b"<< /Subtype /Type1 /BaseFont /Fix /FirstChar 32 /Widths [250 300] \
            /FontDescriptor << /Flags 32 /FontFile 5 0 R >> >>";
        let seed = parts(&[font, b"", b"", b"", &program, b""]);
        assert!(fonts(&seed));
        type1(&program);
        let objects = objects_of(&seed);
        let dict = objects[&ObjectId::new(1, 0)].as_dict().unwrap();
        let (mut budget, mut told) = (BUDGET, Vec::new());
        let read = Font::read(
            Map(&objects),
            dict,
            &Limits::DEFAULT,
            &mut budget,
            &mut told,
        );
        assert_eq!(read.unwrap().program(), crate::FontProgram::Capped);
        let told: Vec<Fault> = told.into_iter().map(|(_, fault)| fault).collect();
        assert_eq!(told, [Fault::ProgramCapped]);
    }

    #[test]
    fn random_bytes_keep_every_promise() {
        let mut random = generator(0xB17E5);
        for _ in 0..2_000 {
            let data: Vec<u8> = (0..random() % 64).map(|_| random() as u8).collect();
            type1(&data);
            names(&data);
            written(&data);
            fonts(&data);
            shown(&data);
        }
    }

    /// `PDF_FONTS_FUZZ_SEEDS` names a folder: writes the seeds there, a font's also as
    /// a file, for the fuzzer to begin from (`just fuzz fonts`).
    #[test]
    #[ignore = "needs PDF_FONTS_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_FONTS_FUZZ_SEEDS").expect("PDF_FONTS_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in fixtures().into_iter().enumerate() {
            std::fs::write(dir.join(format!("seed-{i}")), seed).unwrap();
        }
        for (i, seed) in font_fixtures().into_iter().enumerate() {
            std::fs::write(dir.join(format!("file-{i}")), file_of(&seed)).unwrap();
        }
    }

    /// An input of `program` whole, face `face`, naming `glyph` ([`embed`]).
    #[cfg(feature = "embed")]
    fn whole(program: &[u8], face: u8, glyph: u16) -> Vec<u8> {
        [&[0, face][..], &glyph.to_be_bytes(), program].concat()
    }

    /// An input of `table` as the fixture program's table `tag`, naming `glyph`
    /// ([`embed`]).
    #[cfg(feature = "embed")]
    fn spliced(tag: &[u8; 4], table: &[u8], glyph: u16) -> Vec<u8> {
        let pick = TABLES.iter().position(|t| t == tag).unwrap() as u8;
        [&[1, pick][..], &glyph.to_be_bytes(), table].concat()
    }

    /// The fixture program's table `tag`.
    #[cfg(feature = "embed")]
    fn fixture_table(tag: &[u8; 4]) -> Vec<u8> {
        let tables = crate::fixture::truetype_tables();
        tables.into_iter().find(|t| &t.0 == tag).unwrap().1
    }

    /// `font`'s program with its table `tag` made what `change` makes of it.
    #[cfg(feature = "embed")]
    fn changed(
        font: &pdf_fixtures::font::TrueType,
        tag: &[u8; 4],
        change: impl FnOnce(&mut Vec<u8>),
    ) -> Vec<u8> {
        use pdf_fixtures::font::{TRUETYPE, sfnt};
        let mut tables = font.tables();
        let table = tables.iter_mut().find(|t| &t.0 == tag).unwrap();
        change(&mut table.1);
        sfnt(TRUETYPE, &tables)
    }

    /// The programs the embedding's fuzzing begins from ([`embed`]), each named, with
    /// what it comes to: pdf-fixtures' programs and this crate's, whole, each of the
    /// fixture program's tables, and the sample's it hasn't, spliced; and programs built
    /// to be hostile, each refused for what it holds or cut where it can be.
    #[cfg(feature = "embed")]
    fn embed_seeds() -> Vec<(&'static str, Vec<u8>, Result<usize, crate::embed::Error>)> {
        use crate::embed::{Error::*, Outlines};
        use pdf_fixtures::font::{self, Glyph, Os2, TrueType};

        let sample = font::sample(1000);
        let with = |change: &dyn Fn(&mut TrueType)| {
            let mut font = font::sample(1000);
            change(&mut font);
            font.build()
        };
        let licence = |fs_type: u16| {
            with(&|font| {
                let mut os2 = Os2::new(1000);
                os2.fs_type = fs_type;
                font.os2 = Some(os2);
            })
        };
        // Text after the program, which the input's tail is.
        let text = "Ag \u{1DE} \u{4E2D}\u{1D11E}".as_bytes();
        let mut seeds = vec![
            (
                "sample",
                [&whole(&sample.build(), 0, 2), text].concat(),
                Ok(9),
            ),
            (
                "sample-2048",
                whole(&font::sample(2048).build(), 0, 320),
                Ok(9),
            ),
            (
                "format-4",
                whole(&with(&|font| font.format_12 = false), 0, 0),
                Ok(9),
            ),
            (
                "format-12",
                whole(&with(&|font| font.format_4 = false), 0, 6),
                Ok(9),
            ),
            (
                "named-glyphs",
                whole(
                    &with(&|font| {
                        font.post.names = true;
                        font.long_loca = true;
                        font.hinting = false;
                    }),
                    0,
                    8,
                ),
                Ok(9),
            ),
            ("restricted", whole(&licence(0x0002), 0, 2), Ok(5)),
            ("whole", whole(&licence(0x0100), 0, 2), Ok(9)),
            ("bitmaps-only", whole(&licence(0x0200), 0, 2), Ok(5)),
            (
                "no-os2",
                whole(
                    &with(&|font| {
                        font.os2 = None;
                        font.mac_style = 2;
                        font.post.fixed_pitch = true;
                        font.post.italic_angle = -12 << 16;
                    }),
                    0,
                    3,
                ),
                Ok(9),
            ),
            (
                "old-os2",
                whole(
                    &with(&|font| {
                        let mut os2 = Os2::new(1000);
                        (os2.version, os2.weight, os2.family_class) = (1, 700, 0x0105);
                        font.os2 = Some(os2);
                    }),
                    0,
                    4,
                ),
                Ok(9),
            ),
            (
                "fewer-metrics",
                whole(&with(&|font| font.long_metrics = Some(3)), 0, 300),
                Ok(9),
            ),
            (
                "collection",
                whole(
                    &font::collection(&[sample.build(), font::sample(2048).build()]),
                    1,
                    2,
                ),
                Ok(9),
            ),
            ("past-the-glyphs", whole(&sample.build(), 0, 321), Ok(8)),
            ("past-the-most", whole(&sample.build(), 0, u16::MAX), Ok(8)),
            ("fixture", whole(&truetype_program(), 0, 5), Ok(9)),
            (
                "composite-bomb",
                whole(&composite_bomb_program(), 0, 1),
                Ok(9),
            ),
            (
                "composite-of",
                whole(&composite_of_program(2, 3, 2), 0, 2),
                Ok(9),
            ),
            (
                "opentype",
                whole(
                    &opentype_program(OpenType {
                        post_names: false,
                        cmap: true,
                    }),
                    0,
                    1,
                ),
                Err(Outlines(Outlines::Cff)),
            ),
            (
                "loca-past-end",
                whole(&loca_past_end_program(), 0, 1),
                Err(Outlines(Outlines::Cff)),
            ),
        ];

        // A composite naming every glyph number, in a program of 65,535 glyphs.
        let mut most = TrueType::new(1000);
        let mut composite = vec![0xFF, 0xFF, 0, 0, 0, 0, 0, 100, 0, 100];
        for glyph in 0..=u16::MAX {
            let flags: u16 = if glyph < u16::MAX { 0x0022 } else { 0x0002 };
            composite.extend(flags.to_be_bytes());
            composite.extend(glyph.to_be_bytes());
            composite.extend([0, 0]);
        }
        most.glyph(Glyph::Raw(composite), 500);
        while most.glyphs.len() < 65_535 {
            most.glyph(Glyph::Empty, 500);
        }
        let malformed = |table| Err(Malformed { table });
        seeds.push((
            "every-component",
            whole(&most.build(), 0, 1),
            malformed("glyf"),
        ));

        // `name` tables a subset couldn't write: a record past 65,535 bytes, more
        // records than a table holds, and strings past 65,535 bytes together.
        let storage: Vec<u8> = (0..=0xFFFFu32).map(|i| (i % 251) as u8).collect();
        let kept = |id: u16, length: u16, offset: u16| [3, 1, 0x409, id, length, offset];
        let names = [
            ("past-the-end", vec![kept(1, 1, 0xFFFF)], &storage[..]),
            ("records", [kept(1, 2, 0)].repeat(5461), &storage[..2]),
            (
                "strings",
                vec![kept(1, 40_000, 0), kept(4, 30_000, 35_535)],
                &storage[..],
            ),
        ];
        for (name, records, storage) in names {
            let table = font::name_table(&records, storage);
            seeds.push((name, spliced(b"name", &table, 1), malformed("name")));
        }

        // A `loca` placing every even glyph on all of `glyf`, every odd one backwards.
        let glyf = fixture_table(b"glyf").len() as u32;
        let zigzag: Vec<u8> = (0..7u32)
            .flat_map(|i| (if i % 2 == 0 { 0 } else { glyf }).to_be_bytes())
            .collect();
        seeds.push(("zigzag", spliced(b"loca", &zigzag, 1), malformed("loca")));
        // A `loca` out of order, `A`'s end before its start, so that the glyphs on
        // each side of it take its bytes too.
        let backwards = changed(&sample, b"loca", |loca| {
            loca.as_chunks_mut::<2>().0.swap(2, 3)
        });
        seeds.push((
            "loca-out-of-order",
            whole(&backwards, 0, 3),
            malformed("loca"),
        ));
        // `loca` placing the glyphs past `B` past the end of `glyf`, and `.notdef`'s
        // start past its end, which no cut could take.
        let past = changed(&sample, b"loca", |loca| {
            let past = (u16::MAX / 2).to_be_bytes();
            loca.as_chunks_mut::<2>().0[10..].fill(past);
        });
        seeds.push(("glyf-past-the-table", whole(&past, 0, 2), malformed("loca")));
        let notdef = changed(&sample, b"loca", |loca| {
            let end = u16::from_be_bytes([loca[2], loca[3]]);
            loca[..2].copy_from_slice(&(end + 1).to_be_bytes());
        });
        seeds.push(("notdef-reversed", whole(&notdef, 0, 2), malformed("loca")));

        // Tables a cut rewrites wrong, or refuses for some glyph: an `hmtx` without
        // the last glyph's side bearing, an `hhea` past its 36 bytes, a `maxp` of
        // version 0.5, a `post` naming three glyphs; and that `post` naming them all.
        let mut fewer = font::sample(1000);
        fewer.long_metrics = Some(3);
        let bearings = changed(&fewer, b"hmtx", |hmtx| hmtx.truncate(hmtx.len() - 2));
        seeds.push(("no-bearing", whole(&bearings, 0, 320), malformed("hmtx")));
        let long = changed(&sample, b"hhea", |hhea| hhea.extend([0; 4]));
        seeds.push(("long-hhea", whole(&long, 0, 300), malformed("hhea")));
        let count = &fixture_table(b"maxp")[4..6];
        let mut maxp = 0x0000_5000u32.to_be_bytes().to_vec();
        maxp.extend(count);
        seeds.push(("maxp-0.5", spliced(b"maxp", &maxp, 1), malformed("maxp")));
        let count = u16::from_be_bytes([count[0], count[1]]);
        let post = |named: u16| {
            let mut post = 0x0002_0000u32.to_be_bytes().to_vec();
            post.extend([0; 28]);
            post.extend(named.to_be_bytes());
            post.extend((0..named).flat_map(u16::to_be_bytes));
            post
        };
        let short = post(count - 1);
        seeds.push(("post-short", spliced(b"post", &short, 1), malformed("post")));
        seeds.push(("post-names", spliced(b"post", &post(count), 1), Ok(9)));

        // Units per em outside OpenType's range, of which FreeType loads no program,
        // and each end of it.
        for (name, units, outcome) in [
            ("units-per-em-15", 15u16, malformed("head")),
            ("units-per-em-16", 16, Ok(9)),
            ("units-per-em-16384", 16_384, Ok(9)),
            ("units-per-em-16385", 16_385, malformed("head")),
        ] {
            let program = changed(&sample, b"head", |head| {
                head[18..20].copy_from_slice(&units.to_be_bytes());
            });
            seeds.push((name, whole(&program, 0, 2), outcome));
        }

        // `hhea` with no advance in `hmtx`, and one cut short.
        let mut no_advances = fixture_table(b"hhea");
        no_advances[34..36].fill(0);
        seeds.push((
            "no-advances",
            spliced(b"hhea", &no_advances, 1),
            malformed("hhea"),
        ));
        let short = &fixture_table(b"hhea")[..30];
        seeds.push(("cut-short", spliced(b"hhea", short, 1), malformed("hhea")));

        // A format 12 `cmap` claiming 2^32 - 1 groups, of which it holds one.
        let mut cmap = [0u16, 1, 3, 10].map(u16::to_be_bytes).concat();
        cmap.extend(12u32.to_be_bytes());
        cmap.extend([12u16, 0].map(u16::to_be_bytes).concat());
        cmap.extend(
            [28, 0, u32::MAX, 0x41, 0x5A, 1]
                .map(u32::to_be_bytes)
                .concat(),
        );
        seeds.push(("groups", spliced(b"cmap", &cmap, 1), Ok(9)));

        // A collection whose one face is the collection itself.
        let mut ring = b"ttcf".to_vec();
        ring.extend([0x0001_0000u32, 1, 0].map(u32::to_be_bytes).concat());
        seeds.push(("collection-loop", whole(&ring, 0, 1), Err(NotSfnt)));

        // Each of the fixture program's tables as itself, and the sample's others.
        for (tag, table) in crate::fixture::truetype_tables() {
            seeds.push(("fixture", spliced(&tag, &table, 2), Ok(9)));
        }
        for (tag, table) in sample.tables() {
            if [*b"name", *b"OS/2", *b"cvt ", *b"fpgm", *b"prep"].contains(&tag) {
                seeds.push(("sample", spliced(&tag, &table, 2), Ok(9)));
            }
        }
        seeds
    }

    /// Each program among the embedding's seeds comes to what it's built for: each
    /// fixture program written under every choice, its named glyph too, and read back,
    /// fewer where its licence restricts it or the glyph it names is past its own; each
    /// hostile one, and each a cut couldn't take whole, refused for what it holds.
    #[cfg(feature = "embed")]
    #[test]
    fn the_embed_seeds_come_to_what_they_hold() {
        for (i, (name, seed, outcome)) in embed_seeds().into_iter().enumerate() {
            assert_eq!(embed(&seed), outcome, "{i} {name}");
        }
    }

    /// `seed` changed as a fuzzer might change a program past its header: a byte set, a
    /// word set to a value at an edge, a run cut out or doubled, or its end cut off.
    #[cfg(feature = "embed")]
    fn flipped(seed: &[u8], random: &mut impl FnMut() -> usize) -> Vec<u8> {
        const WORDS: [u16; 6] = [0, 1, 0x7FFF, 0x8000, 0xFFFE, 0xFFFF];
        let mut data = seed.to_vec();
        for _ in 0..1 + random() % 3 {
            let at = HEADER + random() % data.len().saturating_sub(HEADER).max(1);
            let at = at.min(data.len());
            let end = data.len().min(at + 1 + random() % 64);
            match random() % 5 {
                0 if at < data.len() => data[at] = random() as u8,
                1 if at + 2 <= data.len() => {
                    let word = WORDS[random() % WORDS.len()].to_be_bytes();
                    data[at..at + 2].copy_from_slice(&word);
                }
                2 => drop(data.drain(at..end)),
                3 => {
                    let run = data[at..end].to_vec();
                    data.splice(at..at, run);
                }
                _ => data.truncate(at),
            }
        }
        data
    }

    /// The embedding's seeds of a few KiB, mutated, keep every promise ([`embed`]).
    #[cfg(feature = "embed")]
    #[test]
    fn mutated_programs_keep_every_promise() {
        let mut random = generator(0xE3BED);
        let mut opened = 0;
        for (_, seed, _) in embed_seeds() {
            if seed.len() > 16 << 10 {
                continue;
            }
            for _ in 0..16 {
                opened += usize::from(embed(&flipped(&seed, &mut random)).is_ok());
            }
        }
        assert!(opened >= 100, "the mutations reach what's opened: {opened}");
    }

    /// `PDF_FONTS_FUZZ_SEEDS` names a folder: writes the embedding's seeds there, each
    /// by its number and its name, and a table's by its tag too, for the fuzzer to begin
    /// from (`just fuzz embed`).
    #[cfg(feature = "embed")]
    #[test]
    #[ignore = "needs PDF_FONTS_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_embed_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_FONTS_FUZZ_SEEDS").expect("PDF_FONTS_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, (name, seed, _)) in embed_seeds().into_iter().enumerate() {
            let file = match seed[0] % 2 {
                0 => format!("{i:02}-{name}"),
                _ => {
                    let tag = TABLES[usize::from(seed[1]) % TABLES.len()];
                    let tag = String::from_utf8_lossy(&tag).replace(['/', ' '], "");
                    format!("{i:02}-{tag}-{name}")
                }
            };
            std::fs::write(dir.join(file), seed).unwrap();
        }
    }
}
