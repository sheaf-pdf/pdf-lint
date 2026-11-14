//! pdf-fonts timed on fonts made here, so that a change to how a font is read,
//! how a string is split into its codes, or how a code is measured or read as
//! text can be judged before and after it, by `cargo bench -p pdf-fonts`:
//!
//! - `read`: `Font::read_for_text` of a standard 14 font by its name alone
//!   (`std14`); of a TrueType font with `/Widths` for 224 codes and
//!   `/Differences` naming 100 glyphs (`simple`); of a Type 0 font in
//!   Identity-H with `/W` for 10,000 CIDs and a `/ToUnicode` of 10,000
//!   `bfchar` mappings (`identity`); of a Type 0 font whose embedded CMap
//!   holds 10,000 `cidrange`s (`cmap`); and of a Type 3 font of 256 CharProcs
//!   (`type3`). With the `programs` feature, on by default, `Font::read` of a
//!   font embedding each kind of program the crate's PDFium cases are written
//!   with, TrueType (`program/truetype`), CFF (`program/cff`) and Type 1
//!   (`program/type1`).
//! - `codes`: a string of 64 KiB through `Font::codes`, walked by `next`
//!   (`.../next`) and counted (`.../count`): in a simple font (`simple`), in
//!   Identity-H (`identity`), in an embedded CMap of codes of a byte and of two
//!   (`cmap`), and in a Type 0 font whose `/Encoding` is neither a name nor a
//!   stream, which says nothing of its codes, so that they're split by its
//!   `/ToUnicode`'s codespace (`unknown`).
//! - `text`: `Font::text` of a string of 64 KiB through a `/ToUnicode`
//!   (`to-unicode`) and through an encoding (`encoding`), and `Font::code_text`
//!   of each code from 0 to 255 (`code-text`).
//! - `widths`: `Font::width` of each code of a simple font (`simple`, 256) and
//!   of a Type 0 font (`type0`, 65,536).
//! - `names`: `glyph_text` of each name the Adobe Glyph List gives
//!   (`glyph-text`), `name_code_point` of each (`code-point`), and
//!   `Std14::named` of each of the 89 names it takes for a standard 14 font
//!   (`std14`).
//! - `scope`: `FontScope::font` picking 100 fonts by name, 10,000 picks in a
//!   scope of their own, over fonts read once (`pick`).
//! - `shown`: `shown` of a page whose content shows 6,000 strings in four fonts,
//!   two with a `/ToUnicode`, saving and restoring the font, and draws a form of
//!   ten strings 100 times, over fonts read once (`page`).
//!
//! Throughput is in fonts, bytes of a string, codes, names, picks or strings
//! shown. Each input is held to what it gives before it's timed: a font to its
//! kind, the faults told of it (none), what it cost, and its widths, CIDs,
//! boxes and text; a string to each of its codes and to its text; a name to its
//! text, its code point or its font; a pick to the font it names; a page to the
//! strings it shows and their text. The objects are made here into a map, which
//! is the resolver and gives each stream's data as it's held, and the programs
//! are the crate's own (`tests/pdfium/programs`); nothing is read from disk,
//! and the inputs are the same on every run. None names a predefined CMap, so a
//! build with `predefined-cmaps` times the same work.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hint::black_box;
use std::ops::Range;

use criterion::{Criterion, Throughput};
use pdf_fonts::pdf_content_streams::{Concat, ResCx};
use pdf_fonts::pdf_objects::fault::Place;
use pdf_fonts::pdf_objects::object::Stream;
use pdf_fonts::pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable, Why, dict};
use pdf_fonts::{
    Descendant, Encoding, Fault, Font, FontKind, Fonts, Limits, Rect, Std14, glyph_text,
    name_code_point, shown,
};

/// Each departure a font's reading tells, kept.
type Faults = Vec<(Place, Fault)>;

/// A string's codes, each with where it is in the string.
type Split = Vec<(Range<usize>, u32)>;

/// The budget a font is read within: a page's, as pdf-lint reads one.
const BUDGET: usize = 256 << 20;

/// How long each string timed is.
const STRING: usize = 64 << 10;

/// How many CIDs `/W` gives, and how many codes the `/ToUnicode` maps, in the
/// Type 0 font in Identity-H.
const CIDS: u32 = 10_000;

/// How many `cidrange`s the embedded CMap holds, each of four codes.
const RANGES: u32 = 10_000;

/// The objects of a file, by number: the resolver, and the streams' data as
/// each is held.
#[derive(Default)]
struct Pdf {
    objects: HashMap<ObjectId, Object>,
    last: u32,
}

impl Pdf {
    /// `object`, as the file's next object.
    fn add(&mut self, object: impl Into<Object>) -> ObjectId {
        self.last += 1;
        let id = ObjectId::new(self.last, 0);
        self.objects.insert(id, object.into());
        id
    }

    /// A stream of `data` and nothing in its dictionary, as the file's next
    /// object.
    fn stream(&mut self, data: impl Into<Vec<u8>>) -> ObjectId {
        self.add(Stream::new(Dict::new(), data.into()))
    }
}

impl<'o> Resolve<'o> for &'o Pdf {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        let pdf: &'o Pdf = self;
        pdf.objects.get(&id)
    }
}

impl Streams for Pdf {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        let Some(stream) = self.objects.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "no such stream"));
        };
        let data = stream.data.bytes().unwrap_or_default();
        match data.len() > limit {
            true => Err(Unreadable::new(id, Why::Limit, "past the limit")),
            false => Ok(Cow::Borrowed(data)),
        }
    }
}

/// The array of `numbers`.
fn numbers<T: Into<Object> + Copy>(numbers: &[T]) -> Object {
    Object::from(numbers.iter().map(|&n| n.into()).collect::<Vec<Object>>())
}

/// A font's descriptor, its heights and its box given, nonsymbolic.
fn descriptor(name: &str) -> Dict {
    dict! {
        "Type" => "FontDescriptor", "FontName" => name, "Flags" => 32,
        "FontBBox" => numbers(&[-100, -250, 1000, 900]), "Ascent" => 900,
        "Descent" => -250, "CapHeight" => 700, "ItalicAngle" => 0, "StemV" => 80,
    }
}

/// A font read for text as pdf-lint reads one, its faults kept.
fn read_for_text(pdf: &Pdf, font: &Dict) -> (Font, Faults) {
    let mut faults = Faults::new();
    let mut budget = BUDGET;
    let read = Font::read_for_text(pdf, font, &Limits::DEFAULT, &mut budget, &mut faults);
    (read.expect("within the budget"), faults)
}

/// The fonts timed, each with its file.
struct Inputs {
    /// Helvetica in WinAnsiEncoding, by its name alone.
    std14: (Pdf, Dict),
    /// A TrueType font with `/Widths` and `/Differences`, and each code's width.
    simple: (Pdf, Dict, Vec<f32>),
    /// A Type 0 font in Identity-H with `/W` and a `/ToUnicode`, and each CID's
    /// width.
    identity: (Pdf, Dict, Vec<f32>),
    /// A Type 0 font whose embedded CMap holds `RANGES` ranges.
    cmap: (Pdf, Dict),
    /// A Type 0 font whose embedded CMap has codes of a byte and of two.
    mixed: (Pdf, Dict),
    /// A Type 0 font whose `/Encoding` is a dictionary, its `/ToUnicode`'s
    /// codespace of a byte and of two.
    unknown: (Pdf, Dict),
    /// A Type 3 font of 256 CharProcs, and each code's box.
    type3: (Pdf, Dict, Vec<Rect>),
}

/// A width for each code or CID, from a pattern.
fn width_of(n: u32) -> f32 {
    (400 + (n % 13) * 50) as f32
}

/// A `/ToUnicode` CMap: `codespace`'s ranges, then `mappings`' `bfchar`s, a
/// hundred to a block.
fn to_unicode(codespace: &str, mappings: &[(u32, u32)]) -> Vec<u8> {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n",
    );
    let ranges = codespace.split(' ').count() / 2;
    cmap.push_str(&format!(
        "{ranges} begincodespacerange\n{codespace}\nendcodespacerange\n"
    ));
    for block in mappings.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", block.len()));
        for &(code, unicode) in block {
            cmap.push_str(&format!("<{code:04X}> <{unicode:04X}>\n"));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap.into_bytes()
}

/// A Type 0 font over `encoding`, its CIDFont `cid_font`.
fn type0(pdf: &mut Pdf, encoding: Object, cid_font: Dict) -> Dict {
    let cid_font = pdf.add(cid_font);
    dict! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "BenchCJK",
        "Encoding" => encoding, "DescendantFonts" => vec![cid_font.into()],
    }
}

/// A CIDFont of glyphs in TrueType, its widths `w`.
fn cid_font(pdf: &mut Pdf, w: Object) -> Dict {
    let descriptor = pdf.add(descriptor("BenchCJK"));
    dict! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "BenchCJK",
        "CIDSystemInfo" => dict! {
            "Registry" => Object::bytes("Adobe"), "Ordering" => Object::bytes("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => descriptor, "DW" => 1000, "W" => w,
    }
}

fn inputs() -> Inputs {
    let mut pdf = Pdf::default();
    let helvetica = dict! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    };
    let std14 = (Pdf::default(), helvetica);

    // TrueType, no program: `/Widths` for codes 32 to 255, and over
    // WinAnsiEncoding, 100 glyphs of Latin Extended-A by name from 128.
    let widths: Vec<f32> = (32..256).map(width_of).collect();
    let mut differences: Vec<Object> = vec![128.into()];
    differences.extend((0..100).map(|i| Object::from(format!("uni{:04X}", 0x100 + i))));
    let descriptor_id = pdf.add(descriptor("BenchSans"));
    let simple = dict! {
        "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "BenchSans",
        "FirstChar" => 32, "LastChar" => 255, "Widths" => numbers(&widths),
        "Encoding" => dict! {
            "Type" => "Encoding", "BaseEncoding" => "WinAnsiEncoding",
            "Differences" => differences,
        },
        "FontDescriptor" => descriptor_id,
    };
    let simple = (pdf, simple, widths);

    // Identity-H: `/W` for CIDs 1 to 10,000, a run of 100 at a time, and a
    // `/ToUnicode` mapping each of those codes to a CJK ideograph.
    let mut pdf = Pdf::default();
    let cid_widths: Vec<f32> = (1..=CIDS).map(width_of).collect();
    let mut w = Vec::new();
    for (run, chunk) in cid_widths.chunks(100).enumerate() {
        w.push(Object::from(1 + 100 * run as u32));
        w.push(numbers(chunk));
    }
    let mappings: Vec<(u32, u32)> = (1..=CIDS).map(|code| (code, 0x4dff + code)).collect();
    let to_unicode_id = pdf.stream(to_unicode("<0000> <FFFF>", &mappings));
    let cid_font_dict = cid_font(&mut pdf, Object::from(w));
    let mut identity = type0(&mut pdf, "Identity-H".into(), cid_font_dict);
    identity.set("ToUnicode", to_unicode_id);
    let identity = (pdf, identity, cid_widths);

    // An embedded CMap of 10,000 ranges of four codes, each code's CID one
    // past it.
    let mut pdf = Pdf::default();
    let mut program = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n\
         /CMapName /Bench-H def\n/CMapType 1 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for block in (0..RANGES).collect::<Vec<_>>().chunks(100) {
        program.push_str(&format!("{} begincidrange\n", block.len()));
        for &range in block {
            let first = 4 * range;
            program.push_str(&format!(
                "<{first:04X}> <{:04X}> {}\n",
                first + 3,
                first + 1
            ));
        }
        program.push_str("endcidrange\n");
    }
    program.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let cmap_id = pdf.add(Stream::new(
        dict! { "Type" => "CMap", "CMapName" => "Bench-H" },
        program.into_bytes(),
    ));
    let cid_font_dict = cid_font(&mut pdf, Object::from(Vec::<Object>::new()));
    let cmap = type0(&mut pdf, cmap_id.into(), cid_font_dict);
    let cmap = (pdf, cmap);

    // An embedded CMap of ASCII's codes, a byte each, and of codes of two
    // bytes from 0x8000.
    let mut pdf = Pdf::default();
    let mixed_cmap = pdf.add(Stream::new(
        dict! { "Type" => "CMap", "CMapName" => "Bench-Mixed-H" },
        b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
          /CMapName /Bench-Mixed-H def\n\
          2 begincodespacerange\n<00> <7F>\n<8000> <FFFF>\nendcodespacerange\n\
          2 begincidrange\n<20> <7E> 1\n<8000> <FFFF> 100\nendcidrange\n\
          endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n"
            .to_vec(),
    ));
    let cid_font_dict = cid_font(&mut pdf, Object::from(Vec::<Object>::new()));
    let mixed = type0(&mut pdf, mixed_cmap.into(), cid_font_dict);
    let mixed = (pdf, mixed);

    // An `/Encoding` that's a dictionary, which PDFium doesn't load: its codes
    // split by its `/ToUnicode`'s codespace, of a byte and of two.
    let mut pdf = Pdf::default();
    let to_unicode_id = pdf.stream(to_unicode(
        "<00> <7F> <8000> <FFFF>",
        &[(0x41, 0x41), (0x8001, 0x4e00)],
    ));
    let cid_font_dict = cid_font(&mut pdf, Object::from(Vec::<Object>::new()));
    let encoding = dict! { "CMapName" => "Bench-H" };
    let mut unknown = type0(&mut pdf, encoding.into(), cid_font_dict);
    unknown.set("ToUnicode", to_unicode_id);
    let unknown = (pdf, unknown);

    // Type 3: a CharProc for each code, each drawing a box of its own, by
    // name through `/Differences`.
    let mut pdf = Pdf::default();
    let mut procs = Dict::new();
    let mut boxes = Vec::new();
    let mut names: Vec<Object> = vec![0.into()];
    for code in 0..256u32 {
        let (x0, y0) = ((code % 7) as i32 * 10, -((code % 5) as i32) * 20);
        let (x1, y1) = (
            x0 + 300 + (code % 11) as i32 * 20,
            500 + (code % 3) as i32 * 100,
        );
        let proc = format!(
            "{} 0 {x0} {y0} {x1} {y1} d1\n{x0} {y0} {} {} re f\n",
            width_of(code),
            x1 - x0,
            y1 - y0
        );
        let name = format!("g{code}");
        procs.set(name.as_str(), pdf.stream(proc));
        names.push(Object::from(name.as_str()));
        boxes.push(Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32));
    }
    let type3_widths: Vec<f32> = (0..256).map(width_of).collect();
    let type3 = dict! {
        "Type" => "Font", "Subtype" => "Type3",
        "FontBBox" => numbers(&[0, -100, 600, 800]),
        "FontMatrix" => numbers(&[0.001, 0.0, 0.0, 0.001, 0.0, 0.0]),
        "CharProcs" => procs,
        "Encoding" => dict! { "Type" => "Encoding", "Differences" => names },
        "FirstChar" => 0, "LastChar" => 255, "Widths" => numbers(&type3_widths),
        "Resources" => Dict::new(),
    };
    let type3 = (pdf, type3, boxes);

    Inputs {
        std14,
        simple,
        identity,
        cmap,
        mixed,
        unknown,
        type3,
    }
}

/// `font` read over `pdf`, its program where `programs`, as one timing of it.
fn timed_read(pdf: &Pdf, font: &Dict, programs: bool) -> (Option<Font>, Faults) {
    let mut faults = Faults::new();
    let mut budget = BUDGET;
    let read = match programs {
        true => Font::read(pdf, font, &Limits::DEFAULT, &mut budget, &mut faults),
        false => Font::read_for_text(pdf, font, &Limits::DEFAULT, &mut budget, &mut faults),
    };
    (read.ok(), faults)
}

/// What every stream of `pdf` holds, in bytes: what reading a font that reads
/// them all costs.
fn stream_bytes(pdf: &Pdf) -> usize {
    (pdf.objects.values())
        .filter_map(Object::as_stream)
        .map(|stream| stream.data.bytes().unwrap_or_default().len())
        .sum()
}

/// The text of the code point `n`.
fn char_of(n: u32) -> String {
    String::from(char::from_u32(n).expect("a character"))
}

fn read_group(c: &mut Criterion) {
    let inputs = inputs();
    let mut group = c.benchmark_group("read");
    group.throughput(Throughput::Elements(1));

    let (pdf, font) = &inputs.std14;
    let (read, faults) = read_for_text(pdf, font);
    assert!(faults.is_empty(), "read/std14: no fault");
    assert_eq!(read.kind(), FontKind::Type1, "read/std14: its kind");
    assert_eq!(read.width(u32::from(b'A')), 667.0, "read/std14: A's width");
    let text = read.text(b"A\x80");
    assert_eq!(text.as_deref(), Some("A\u{20ac}"), "read/std14: its text");
    assert_eq!(read.cost(), 0, "read/std14: its cost");
    group.bench_function("std14", |b| {
        b.iter(|| timed_read(black_box(pdf), black_box(font), false))
    });

    let (pdf, font, widths) = &inputs.simple;
    let (read, faults) = read_for_text(pdf, font);
    assert!(faults.is_empty(), "read/simple: no fault");
    assert_eq!(read.kind(), FontKind::TrueType, "read/simple: its kind");
    for (code, &width) in (32..256).zip(widths) {
        assert_eq!(read.width(code), width, "read/simple: code {code}'s width");
    }
    for code in 128..228 {
        let text = read.code_text(code);
        let expected = char_of(code - 128 + 0x100);
        assert_eq!(
            text.as_deref(),
            Some(expected.as_str()),
            "read/simple: code {code}'s text"
        );
    }
    assert_eq!(
        read.code_text(u32::from(b'A')).as_deref(),
        Some("A"),
        "read/simple: A's text"
    );
    assert_eq!(read.cost(), 0, "read/simple: its cost");
    group.bench_function("simple", |b| {
        b.iter(|| timed_read(black_box(pdf), black_box(font), false))
    });

    let (pdf, font, widths) = &inputs.identity;
    let (read, faults) = read_for_text(pdf, font);
    assert!(faults.is_empty(), "read/identity: no fault");
    let kind = FontKind::Type0(Descendant::CidFontType2);
    assert_eq!(read.kind(), kind, "read/identity: its kind");
    for (code, &width) in (1..=CIDS).zip(widths) {
        assert_eq!(
            read.width(code),
            width,
            "read/identity: code {code}'s width"
        );
        let expected = char_of(0x4dff + code);
        let text = read.code_text(code);
        assert_eq!(
            text.as_deref(),
            Some(expected.as_str()),
            "read/identity: code {code}'s text"
        );
    }
    assert_eq!(read.width(0), 1000.0, "read/identity: /DW");
    assert_eq!(read.width(CIDS + 1), 1000.0, "read/identity: /DW");
    assert_eq!(read.cost(), stream_bytes(pdf), "read/identity: its cost");
    group.bench_function("identity", |b| {
        b.iter(|| timed_read(black_box(pdf), black_box(font), false))
    });

    let (pdf, font) = &inputs.cmap;
    let (read, faults) = read_for_text(pdf, font);
    assert!(faults.is_empty(), "read/cmap: no fault");
    assert_eq!(read.kind(), kind, "read/cmap: its kind");
    assert!(!read.approx().codes, "read/cmap: its codes as PDFium's");
    for code in 0..4 * RANGES {
        assert_eq!(
            read.cid(code),
            Some(code + 1),
            "read/cmap: code {code}'s CID"
        );
    }
    assert_eq!(
        read.cid(4 * RANGES),
        Some(0),
        "read/cmap: a code in no range"
    );
    assert_eq!(read.cost(), stream_bytes(pdf), "read/cmap: its cost");
    group.bench_function("cmap", |b| {
        b.iter(|| timed_read(black_box(pdf), black_box(font), false))
    });

    let (pdf, font, boxes) = &inputs.type3;
    let (read, faults) = read_for_text(pdf, font);
    assert!(faults.is_empty(), "read/type3: no fault");
    assert_eq!(read.kind(), FontKind::Type3, "read/type3: its kind");
    for (code, &expected) in (0..256).zip(boxes) {
        assert_eq!(
            read.glyph_box(code),
            Some(expected),
            "read/type3: code {code}'s box"
        );
        assert_eq!(
            read.width(code),
            width_of(code),
            "read/type3: code {code}'s width"
        );
    }
    assert_eq!(read.cost(), stream_bytes(pdf), "read/type3: its cost");
    group.bench_function("type3", |b| {
        b.iter(|| timed_read(black_box(pdf), black_box(font), false))
    });

    #[cfg(feature = "programs")]
    for (name, pdf, font, a) in programs() {
        let mut faults = Faults::new();
        let mut budget = BUDGET;
        let read = Font::read(&pdf, &font, &Limits::DEFAULT, &mut budget, &mut faults);
        let read = read.unwrap_or_else(|e| panic!("read/program/{name}: {e}"));
        assert!(faults.is_empty(), "read/program/{name}: no fault");
        let program = read.program();
        assert_eq!(
            program,
            pdf_fonts::FontProgram::Read,
            "read/program/{name}: its program read"
        );
        assert!(read.outlined(), "read/program/{name}: its outlines read");
        assert_eq!(
            read.char_box(u32::from(b'A')),
            Some(a),
            "read/program/{name}: A's box"
        );
        group.bench_function(format!("program/{name}"), |b| {
            b.iter(|| timed_read(black_box(&pdf), black_box(&font), true))
        });
    }
    group.finish();
}

/// A font embedding each program the PDFium cases are written with, by name, its
/// file, and the box its `A` is drawn in: as the cases embed them, `/Widths` for
/// codes 32 to 103, in WinAnsiEncoding, the descriptor nonsymbolic with its
/// heights and box.
#[cfg(feature = "programs")]
fn programs() -> Vec<(&'static str, Pdf, Dict, Rect)> {
    let truetype = include_bytes!("../tests/pdfium/programs/truetype");
    let cff = include_bytes!("../tests/pdfium/programs/cff");
    let type1 = include_bytes!("../tests/pdfium/programs/type1");
    let each = [
        (
            "truetype",
            "TrueType",
            "FontFile2",
            &truetype[..],
            Rect::new(0.0, 0.0, 600.0, 750.0),
        ),
        (
            "cff",
            "Type1",
            "FontFile3",
            &cff[..],
            Rect::new(0.0, 0.0, 600.0, 750.0),
        ),
        (
            "type1",
            "Type1",
            "FontFile",
            &type1[..],
            Rect::new(0.0, 0.0, 600.0, 750.0),
        ),
    ];
    let widths = [500; 72];
    (each.into_iter())
        .map(|(name, subtype, key, program, a)| {
            let mut pdf = Pdf::default();
            let program = pdf.stream(program);
            let mut descriptor = dict! {
                "Type" => "FontDescriptor", "FontName" => "Fix", "Flags" => 32,
                "FontBBox" => numbers(&[0, -200, 700, 800]), "Ascent" => 800, "Descent" => -200,
            };
            descriptor.set(key, program);
            let descriptor = pdf.add(descriptor);
            let font = dict! {
                "Type" => "Font", "Subtype" => subtype, "BaseFont" => "Fix",
                "FirstChar" => 32, "LastChar" => 103, "Widths" => numbers(&widths),
                "Encoding" => "WinAnsiEncoding", "FontDescriptor" => descriptor,
            };
            (name, pdf, font, a)
        })
        .collect()
}

/// A string of 64 KiB in a font of codes of a byte and of two, by a codespace
/// of `<00> <7F>` and `<8000> <FFFF>`, and its codes: a byte of ASCII, then a
/// code of two bytes, over and over, and a last byte of ASCII.
fn mixed_string() -> (Vec<u8>, Split) {
    let mut s = Vec::with_capacity(STRING);
    let mut codes = Vec::new();
    let mut k = 0u32;
    while s.len() + 3 < STRING {
        let at = s.len();
        let ascii = 0x20 + (k % 95) as u8;
        let two = [0x80 | (k / 95 % 0x80) as u8, (k % 256) as u8];
        s.push(ascii);
        s.extend(two);
        codes.push((at..at + 1, u32::from(ascii)));
        codes.push((at + 1..at + 3, u32::from(u16::from_be_bytes(two))));
        k += 1;
    }
    codes.push((s.len()..s.len() + 1, u32::from(b'A')));
    s.push(b'A');
    assert_eq!(s.len(), STRING);
    (s, codes)
}

/// A string of 64 KiB in Identity-H, each code from 1 to `CIDS` over and over,
/// and its codes.
fn identity_string() -> (Vec<u8>, Split) {
    let codes: Split = (0..STRING / 2)
        .map(|k| (2 * k..2 * k + 2, k as u32 % CIDS + 1))
        .collect();
    let s = codes
        .iter()
        .flat_map(|(_, code)| (*code as u16).to_be_bytes())
        .collect();
    (s, codes)
}

fn codes_group(c: &mut Criterion) {
    let inputs = inputs();
    let simple = read_for_text(&inputs.std14.0, &inputs.std14.1).0;
    let identity = read_for_text(&inputs.identity.0, &inputs.identity.1).0;
    let (mixed, faults) = read_for_text(&inputs.mixed.0, &inputs.mixed.1);
    assert!(faults.is_empty(), "codes/cmap: no fault");
    assert!(!mixed.approx().codes, "codes/cmap: its codes as PDFium's");
    let (unknown, faults) = read_for_text(&inputs.unknown.0, &inputs.unknown.1);
    assert!(!unknown.loaded(), "codes/unknown: not loaded");
    assert!(unknown.approx().codes, "codes/unknown: its codes guessed");
    let told: Vec<&Fault> = faults.iter().map(|(_, fault)| fault).collect();
    assert_eq!(
        told,
        [&Fault::CMap],
        "codes/unknown: an /Encoding of no CMap"
    );
    let bytes: Vec<u8> = (0..STRING).map(|i| i as u8).collect();
    let byte_codes = (0..STRING)
        .map(|i| (i..i + 1, u32::from(i as u8)))
        .collect();
    let (two, two_codes) = identity_string();
    let (mixed_bytes, mixed_codes) = mixed_string();
    let strings: [(&str, &Font, &[u8], Split); 4] = [
        ("simple", &simple, &bytes, byte_codes),
        ("identity", &identity, &two, two_codes),
        ("cmap", &mixed, &mixed_bytes, mixed_codes.clone()),
        ("unknown", &unknown, &mixed_bytes, mixed_codes),
    ];
    let mut group = c.benchmark_group("codes");
    group.throughput(Throughput::Bytes(STRING as u64));
    for (name, font, s, expected) in &strings {
        let split: Split = font.codes(s).collect();
        assert_eq!(&split, expected, "codes/{name}: its codes");
        group.bench_function(format!("{name}/next"), |b| {
            b.iter(|| {
                let mut sum = 0u64;
                for (range, code) in font.codes(black_box(s)) {
                    sum = sum.wrapping_add(range.start as u64 ^ u64::from(code));
                }
                sum
            })
        });
        group.bench_function(format!("{name}/count"), |b| {
            b.iter(|| font.codes(black_box(s)).count())
        });
    }
    group.finish();
}

fn text_group(c: &mut Criterion) {
    let inputs = inputs();
    let mut group = c.benchmark_group("text");

    let identity = read_for_text(&inputs.identity.0, &inputs.identity.1).0;
    let (s, codes) = identity_string();
    let expected: String = codes
        .iter()
        .map(|&(_, code)| char_of(0x4dff + code))
        .collect();
    assert_eq!(
        identity.text(&s),
        Some(expected),
        "text/to-unicode: its text"
    );
    group.throughput(Throughput::Bytes(STRING as u64));
    group.bench_function("to-unicode", |b| b.iter(|| identity.text(black_box(&s))));

    let simple = read_for_text(&inputs.std14.0, &inputs.std14.1).0;
    let ascii: Vec<u8> = (0..STRING).map(|i| b' ' + (i % 95) as u8).collect();
    let expected = String::from_utf8(ascii.clone()).expect("ASCII");
    assert_eq!(
        simple.text(&ascii),
        Some(expected),
        "text/encoding: its text"
    );
    group.bench_function("encoding", |b| b.iter(|| simple.text(black_box(&ascii))));

    let mut said = 0;
    for code in 0..=255u8 {
        let text = simple.code_text(u32::from(code));
        let expected = Encoding::WinAnsi.text(code);
        assert_eq!(
            text.as_deref(),
            expected.as_deref(),
            "text/code-text: code {code}"
        );
        said += usize::from(text.is_some());
    }
    assert_eq!(said, 224, "text/code-text: the codes with text");
    group.throughput(Throughput::Elements(256));
    group.bench_function("code-text", |b| {
        b.iter(|| {
            let font = black_box(&simple);
            (0..256).filter_map(|code| font.code_text(code)).count()
        })
    });
    group.finish();
}

fn widths_group(c: &mut Criterion) {
    let inputs = inputs();
    let mut group = c.benchmark_group("widths");

    let (pdf, font, widths) = &inputs.simple;
    let simple = read_for_text(pdf, font).0;
    let expected: Vec<f32> = (0..256)
        .map(|code| match code {
            32.. => widths[code - 32],
            _ => 0.0,
        })
        .collect();
    let each: Vec<f32> = (0..256).map(|code| simple.width(code)).collect();
    assert_eq!(each, expected, "widths/simple: each code's");
    group.throughput(Throughput::Elements(256));
    group.bench_function("simple", |b| {
        b.iter(|| {
            let font = black_box(&simple);
            (0..256).map(|code| font.width(code)).sum::<f32>()
        })
    });

    let (pdf, font, widths) = &inputs.identity;
    let identity = read_for_text(pdf, font).0;
    let expected: Vec<f32> = (0..1 << 16)
        .map(|code: usize| match code {
            1..=10_000 => widths[code - 1],
            _ => 1000.0,
        })
        .collect();
    let each: Vec<f32> = (0..1 << 16).map(|code| identity.width(code)).collect();
    assert_eq!(each, expected, "widths/type0: each code's");
    group.throughput(Throughput::Elements(1 << 16));
    group.bench_function("type0", |b| {
        b.iter(|| {
            let font = black_box(&identity);
            (0..1 << 16).map(|code| font.width(code)).sum::<f32>()
        })
    });
    group.finish();
}

/// The Adobe Glyph List's names, each with its text: `assets/glyphlist.txt`'s
/// lines but its comments, a name and the code points it stands for.
fn agl() -> Vec<(&'static str, String)> {
    (include_str!("../assets/glyphlist.txt").lines())
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (name, points) = line.split_once(';').expect("a name and its code points");
            let text = (points.split(' '))
                .map(|p| char::from_u32(u32::from_str_radix(p, 16).unwrap()).unwrap())
                .collect();
            (name, text)
        })
        .collect()
}

/// Each name PDFium takes for a standard 14 font, and the font it names: the 14
/// and 75 others.
const STD14_NAMES: [(&str, Std14); 89] = {
    use Std14::*;
    [
        ("Arial", Helvetica),
        ("Arial,Bold", HelveticaBold),
        ("Arial,BoldItalic", HelveticaBoldOblique),
        ("Arial,Italic", HelveticaOblique),
        ("Arial-Bold", HelveticaBold),
        ("Arial-BoldItalic", HelveticaBoldOblique),
        ("Arial-BoldItalicMT", HelveticaBoldOblique),
        ("Arial-BoldMT", HelveticaBold),
        ("Arial-Italic", HelveticaOblique),
        ("Arial-ItalicMT", HelveticaOblique),
        ("ArialBold", HelveticaBold),
        ("ArialBoldItalic", HelveticaBoldOblique),
        ("ArialItalic", HelveticaOblique),
        ("ArialMT", Helvetica),
        ("ArialMT,Bold", HelveticaBold),
        ("ArialMT,BoldItalic", HelveticaBoldOblique),
        ("ArialMT,Italic", HelveticaOblique),
        ("ArialRoundedMTBold", HelveticaBold),
        ("Courier", Courier),
        ("Courier,Bold", CourierBold),
        ("Courier,BoldItalic", CourierBoldOblique),
        ("Courier,Italic", CourierOblique),
        ("Courier-Bold", CourierBold),
        ("Courier-BoldOblique", CourierBoldOblique),
        ("Courier-Oblique", CourierOblique),
        ("CourierBold", CourierBold),
        ("CourierBoldItalic", CourierBoldOblique),
        ("CourierItalic", CourierOblique),
        ("CourierNew", Courier),
        ("CourierNew,Bold", CourierBold),
        ("CourierNew,BoldItalic", CourierBoldOblique),
        ("CourierNew,Italic", CourierOblique),
        ("CourierNew-Bold", CourierBold),
        ("CourierNew-BoldItalic", CourierBoldOblique),
        ("CourierNew-Italic", CourierOblique),
        ("CourierNewBold", CourierBold),
        ("CourierNewBoldItalic", CourierBoldOblique),
        ("CourierNewItalic", CourierOblique),
        ("CourierNewPS-BoldItalicMT", CourierBoldOblique),
        ("CourierNewPS-BoldMT", CourierBold),
        ("CourierNewPS-ItalicMT", CourierOblique),
        ("CourierNewPSMT", Courier),
        ("CourierStd", Courier),
        ("CourierStd-Bold", CourierBold),
        ("CourierStd-BoldOblique", CourierBoldOblique),
        ("CourierStd-Oblique", CourierOblique),
        ("Helvetica", Helvetica),
        ("Helvetica,Bold", HelveticaBold),
        ("Helvetica,BoldItalic", HelveticaBoldOblique),
        ("Helvetica,Italic", HelveticaOblique),
        ("Helvetica-Bold", HelveticaBold),
        ("Helvetica-BoldItalic", HelveticaBoldOblique),
        ("Helvetica-BoldOblique", HelveticaBoldOblique),
        ("Helvetica-Italic", HelveticaOblique),
        ("Helvetica-Oblique", HelveticaOblique),
        ("HelveticaBold", HelveticaBold),
        ("HelveticaBoldItalic", HelveticaBoldOblique),
        ("HelveticaItalic", HelveticaOblique),
        ("Symbol", Symbol),
        ("SymbolMT", Symbol),
        ("Times-Bold", TimesBold),
        ("Times-BoldItalic", TimesBoldItalic),
        ("Times-Italic", TimesItalic),
        ("Times-Roman", TimesRoman),
        ("TimesBold", TimesBold),
        ("TimesBoldItalic", TimesBoldItalic),
        ("TimesItalic", TimesItalic),
        ("TimesNewRoman", TimesRoman),
        ("TimesNewRoman,Bold", TimesBold),
        ("TimesNewRoman,BoldItalic", TimesBoldItalic),
        ("TimesNewRoman,Italic", TimesItalic),
        ("TimesNewRoman-Bold", TimesBold),
        ("TimesNewRoman-BoldItalic", TimesBoldItalic),
        ("TimesNewRoman-Italic", TimesItalic),
        ("TimesNewRomanBold", TimesBold),
        ("TimesNewRomanBoldItalic", TimesBoldItalic),
        ("TimesNewRomanItalic", TimesItalic),
        ("TimesNewRomanPS", TimesRoman),
        ("TimesNewRomanPS-Bold", TimesBold),
        ("TimesNewRomanPS-BoldItalic", TimesBoldItalic),
        ("TimesNewRomanPS-BoldItalicMT", TimesBoldItalic),
        ("TimesNewRomanPS-BoldMT", TimesBold),
        ("TimesNewRomanPS-Italic", TimesItalic),
        ("TimesNewRomanPS-ItalicMT", TimesItalic),
        ("TimesNewRomanPSMT", TimesRoman),
        ("TimesNewRomanPSMT,Bold", TimesBold),
        ("TimesNewRomanPSMT,BoldItalic", TimesBoldItalic),
        ("TimesNewRomanPSMT,Italic", TimesItalic),
        ("ZapfDingbats", ZapfDingbats),
    ]
};

fn names_group(c: &mut Criterion) {
    let agl = agl();
    let mut group = c.benchmark_group("names");
    group.throughput(Throughput::Elements(agl.len() as u64));
    for (name, text) in &agl {
        let read = glyph_text(name, false);
        assert_eq!(
            read.as_deref(),
            Some(text.as_str()),
            "names/glyph-text: {name}"
        );
    }
    group.bench_function("glyph-text", |b| {
        b.iter(|| {
            (agl.iter())
                .filter_map(|(name, _)| glyph_text(black_box(name), false))
                .count()
        })
    });
    for (name, text) in &agl {
        let mut chars = text.chars();
        let one = chars.next().filter(|_| chars.next().is_none());
        let expected = one.map(|c| (u32::from(c), false));
        let read = name_code_point(name.as_bytes());
        assert_eq!(read, expected, "names/code-point: {name}");
    }
    group.bench_function("code-point", |b| {
        b.iter(|| {
            (agl.iter())
                .filter_map(|(name, _)| name_code_point(black_box(name.as_bytes())))
                .count()
        })
    });
    for (name, font) in STD14_NAMES {
        assert_eq!(
            Std14::named(name.as_bytes()),
            Some(font),
            "names/std14: {name}"
        );
    }
    group.throughput(Throughput::Elements(STD14_NAMES.len() as u64));
    group.bench_function("std14", |b| {
        b.iter(|| {
            (STD14_NAMES.iter())
                .filter_map(|(name, _)| Std14::named(black_box(name.as_bytes())))
                .count()
        })
    });
    group.finish();
}

/// How many fonts the resources name, and how many times they're picked.
const PICKED: usize = 100;
const PICKS: usize = 10_000;

fn scope_group(c: &mut Criterion) {
    // Each a standard 14 font by name, an object of its own, named `/F0` to
    // `/F99` in a page's resources.
    let mut pdf = Pdf::default();
    let mut named = Dict::new();
    let mut ids = Vec::new();
    for at in 0..PICKED {
        let font = dict! {
            "Type" => "Font", "Subtype" => "Type1",
            "BaseFont" => Std14::ALL[at % 14].name(), "Encoding" => "WinAnsiEncoding",
        };
        let id = pdf.add(font);
        named.set(format!("F{at}").as_str(), id);
        ids.push(id);
    }
    let resources = Object::from(dict! { "Font" => named });
    let page = pdf.add(Dict::new());
    let names: Vec<Vec<u8>> = (0..PICKED)
        .map(|at| format!("F{at}").into_bytes())
        .collect();
    // Picked in an order that isn't theirs: 37 is prime to 100.
    let order: Vec<usize> = (0..PICKS).map(|pick| pick * 37 % PICKED).collect();
    let fonts = Fonts::for_text();
    let mut budget = BUDGET;
    let mut scope = fonts.scope();
    for &at in &order {
        let picked = scope.font(
            &pdf,
            page,
            Some(&resources),
            &names[at],
            &Limits::DEFAULT,
            &mut budget,
        );
        let picked = picked
            .expect("within the budget")
            .expect("a font of the name");
        assert_eq!(
            scope.get(picked).id(),
            Some(ids[at]),
            "scope/pick: F{at}'s font"
        );
    }
    let mut group = c.benchmark_group("scope");
    group.throughput(Throughput::Elements(PICKS as u64));
    group.bench_function("pick", |b| {
        b.iter(|| {
            let mut budget = BUDGET;
            let mut scope = fonts.scope();
            for &at in &order {
                let picked = scope.font(
                    &pdf,
                    page,
                    Some(&resources),
                    black_box(&names[at]),
                    &Limits::DEFAULT,
                    &mut budget,
                );
                black_box(picked.ok());
            }
            budget
        })
    });
    group.finish();
}

/// How many text objects the page `shown` reads draws, each three strings.
const TEXT_OBJECTS: usize = 2_000;

/// How many times the page draws its form.
const FORM_USES: usize = 100;

fn shown_group(c: &mut Criterion) {
    // Four fonts by object, two with a `/ToUnicode` of the codes shown, and a form of
    // ten strings with no resources of its own.
    let mut pdf = Pdf::default();
    let map = to_unicode("<00> <FF>", &[(0x41, 0x61), (0x42, 0x62)]);
    let mut fonts = Dict::new();
    for at in 0..4 {
        let mut font = dict! {
            "Type" => "Font", "Subtype" => "Type1",
            "BaseFont" => Std14::ALL[at].name(), "Encoding" => "WinAnsiEncoding",
        };
        if at % 2 == 1 {
            font.set("ToUnicode", pdf.stream(map.clone()));
        }
        fonts.set(format!("F{at}").as_str(), pdf.add(font));
    }
    let form_content: String = (0..10).map(|i| format!("(Form{i}) Tj ")).collect();
    let form = pdf.add(Stream::new(
        dict! { "Subtype" => "Form" },
        form_content.into_bytes(),
    ));
    let resources = Object::from(dict! { "Font" => fonts, "XObject" => dict! { "X" => form } });
    let mut content = String::new();
    for at in 0..TEXT_OBJECTS {
        let font = at % 4;
        content += &format!(
            "q BT /F{font} 12 Tf 72 {} Td (AB, a line {at}) Tj [(AB) -20 (C)] TJ ET Q\n",
            at % 700
        );
        if at % (TEXT_OBJECTS / FORM_USES) == 0 {
            content += "/F0 9 Tf /X Do\n";
        }
    }
    let content = Concat::new(&[content.as_bytes()]);
    let res = ResCx::none();
    let strings = 3 * TEXT_OBJECTS + 10 * FORM_USES;
    let fonts = Fonts::for_text();
    let walk = |budget: &mut usize| {
        shown(
            &pdf,
            &fonts,
            Some(&resources),
            &content,
            &res,
            &Limits::DEFAULT,
            budget,
        )
    };
    let read = walk(&mut BUDGET.clone());
    assert!(read.is_whole(), "shown/page: all read");
    assert_eq!(read.strings.len(), strings, "shown/page: its strings");
    let text = read.text();
    assert!(
        text.starts_with("AB, a line 0ABCForm0"),
        "shown/page: {}",
        &text[..20]
    );
    assert!(
        text.contains("ab, a line 1abC"),
        "shown/page: the /ToUnicode's text"
    );
    let mut group = c.benchmark_group("shown");
    group.throughput(Throughput::Elements(strings as u64));
    group.bench_function("page", |b| {
        b.iter(|| {
            let mut budget = BUDGET;
            black_box(walk(&mut budget).strings.len())
        })
    });
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    read_group(&mut criterion);
    codes_group(&mut criterion);
    text_group(&mut criterion);
    widths_group(&mut criterion);
    names_group(&mut criterion);
    scope_group(&mut criterion);
    shown_group(&mut criterion);
    criterion.final_summary();
}
