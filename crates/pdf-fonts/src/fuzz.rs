//! What the fuzz target checks, which unit tests check too: the promises a Type 1
//! program's scan, a glyph's or a font's name and a font's reading, its program's
//! included, make, on any input. It's compiled only for tests and under cargo-fuzz's
//! `--cfg fuzzing`.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

use pdf_objects::fault::Place;
use pdf_objects::object::Stream;
use pdf_objects::parse::Parser;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable, Why, dict};

use crate::fixture::{
    OpenType, cff_program, cid_cff_program, composite_bomb_program, composite_of_program,
    heavy_cff_program, loca_past_end_program, long_cff_program, moving_cff_program,
    opentype_cid_program, opentype_program, truetype_program, type1_accents_program,
    type1_integers_program, type1_lookups_program, type1_program,
};
use crate::predefined::{Collection, scheme};
use crate::type1::{Refusal, scan};
use crate::{
    Encoding, Error, Fault, Font, Fonts, Limits, Std14, cp1252, glyph_text, name_code_point,
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
/// large; and it fails only for what was made to fail, as it was made to. `by_number`
/// is what refers to it, where the file has one. Whether it read.
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
        let read = Font::read(failing, dict, &limits, &mut budget, &mut ());
        if read.is_err() {
            assert_eq!(budget, BUDGET);
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
    let programs: [(&str, &str, Vec<u8>); 15] = [
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
    .collect()
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
            fonts(&seed);
            for _ in 0..50 {
                let data = mutated(&seed, &mut random);
                type1(&data);
                names(&data);
                fonts(&data);
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
            fonts(&data);
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
}
