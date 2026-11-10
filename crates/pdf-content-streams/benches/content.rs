//! pdf-content-streams timed on content made here, so that a change to its
//! reading, its passes, its resources, its writer, its splices or its joins
//! can be judged before and after it, by `cargo bench -p
//! pdf-content-streams`:
//!
//! - `tokens`: `tokens` over a page of each kind: text, a line of it shown
//!   and kerned (`text`); paths, stroked and filled (`paths`); inline images
//!   of 32 by 32 gray samples, unfiltered (`images-unfiltered`), Flate
//!   (`images-flate`), Flate with a PNG predictor (`images-flate-png`) and
//!   LZW (`images-lzw`), and one in an Indexed space written as an array in
//!   its dictionary (`images-array-space`); and marked content whose
//!   property lists nest dictionaries and arrays (`nested`).
//! - `operations`: `operations` over the same pages.
//! - `passes`: `balance`, `marked` and `names_used` over a tagged page, its
//!   lists written inline and named.
//! - `rescx`: `ResCx::read` of a resources dictionary naming 100 color
//!   spaces, seven kinds in turn, some by reference.
//! - `res`: a font's objects, its program's data in memory, digested by
//!   `ResId::of` (`id`), and compared as resources, with a clone
//!   (`eq-clone`) and with one made apart of equal objects (`eq-apart`).
//! - `write`: a page written by `ContentWriter`'s calls and finished
//!   (`calls`); the names a resources dictionary of 100 binds, read by
//!   `Reserved::read` (`reserved-100`); and the page's resources named clear
//!   of them (`named-100`).
//! - `splice`: 20 marked ranges taken out of a page by `splice`
//!   (`splice-20`), and by `splice_each`, each its own group, all taken
//!   (`each-20`) and with one refused (`each-20-one-refused`).
//! - `join`: 20 streams joined by `Join::push` (`push-20`) and by
//!   `Join::push_with` (`push-with-20`).
//!
//! Throughput is in bytes: of the content read, written, spliced or
//! joined, of the objects as written, or of what's digested and compared.
//! Each input is held to what it gives before it's timed: a page to its
//! tokens and operations, counted from how it's written, each inline
//! image's stream to its data's bytes; a pass to its balance, its ranges
//! and its names; a resources dictionary to each name's count; two
//! resources to one digest and to equality; written content to its
//! operations, its needs and names clear of those bound; a splice to the
//! content with the ranges taken out and to the groups it takes; a join to
//! its streams with a space between each. Everything is made here from
//! text and numbers written in it: nothing is read from disk, and the
//! inputs are the same on every run.

use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::ops::Range;

use criterion::{Criterion, Throughput};
use pdf_content_streams::{
    Color, Concat, ContentWriter, GState, ImageStream, Join, Limits, Paint, Properties, Props, Res,
    ResCx, ResId, ResKind, Reserved, SpanAcross, balance, marked, names_used, operations, splice,
    splice_each, tokens,
};
use pdf_filters::{Backend, Encoder, Layout, Level, Prediction, Row, encode};
use pdf_objects::parse::Parser;
use pdf_objects::{Dict, Object, ObjectId, Stream, dict};

const LIM: &Limits = &Limits::DEFAULT;

/// A page of content, and what reading it gives, counted from how it's
/// written: its tokens, its operations, and where each inline image's
/// data is, which is its stream.
struct Page {
    name: &'static str,
    bytes: Vec<u8>,
    tokens: usize,
    operations: usize,
    images: Vec<Range<usize>>,
}

/// A page of `lines` lines, each `line(i)`, which reads as `tokens` tokens
/// and `operations` operations.
fn lined(
    name: &'static str,
    lines: usize,
    tokens: usize,
    operations: usize,
    line: impl Fn(usize) -> String,
) -> Page {
    let bytes: String = (0..lines).map(line).collect();
    Page {
        name,
        bytes: bytes.into_bytes(),
        tokens: lines * tokens,
        operations: lines * operations,
        images: Vec::new(),
    }
}

/// How many inline images a page of them holds.
const IMAGES: usize = 64;

/// The pixels of each image: 32 rows of 32 gray samples of 8 bits, a ramp.
fn pixels() -> Vec<u8> {
    (0..32u32)
        .flat_map(|y| (0..32u32).map(move |x| ((x * 7 + y * 3) % 256) as u8))
        .collect()
}

/// A page of [`IMAGES`] inline images, each in a `q`...`Q` of its own, its
/// dictionary `dict` and its data `data`: ten tokens and four operations
/// each.
fn images(name: &'static str, dict: &str, data: &[u8]) -> Page {
    let mut bytes = Vec::new();
    let mut images = Vec::new();
    for i in 0..IMAGES {
        let (x, y) = (72 + 36 * (i % 12), 72 + 36 * (i / 12));
        bytes.extend(format!("q 32 0 0 32 {x} {y} cm BI {dict} ID ").bytes());
        images.push(bytes.len()..bytes.len() + data.len());
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(b" EI Q\n");
    }
    Page {
        name,
        bytes,
        tokens: IMAGES * 10,
        operations: IMAGES * 4,
        images,
    }
}

/// `data` encoded by `encoder`.
fn encoded(data: &[u8], encoder: Encoder) -> Vec<u8> {
    let mut out = Vec::new();
    let made = encode(data, &[encoder], Backend::ZlibRs, &mut out);
    made.unwrap_or_else(|e| panic!("an image's data encoded: {e}"));
    out
}

/// `data` as LZW codes (ISO 32000-2, 7.4.4.2), each 9 bits wide, the most
/// significant bit first: a clear code first, and again before the table
/// grows to where a decoder would widen its codes, so either `/EarlyChange`
/// reads them alike; an end code last, its byte filled out with zeros.
fn lzw(data: &[u8]) -> Vec<u8> {
    const CLEAR: u16 = 256;
    const END: u16 = 257;
    // The entry at which the table is cleared, never made: with early
    // change, codes widen once entry 510 is.
    const CLEARED: u16 = 500;
    let code = |table: &HashMap<Vec<u8>, u16>, w: &[u8]| match w {
        [byte] => u16::from(*byte),
        _ => table[w],
    };
    let mut codes = vec![CLEAR];
    let mut table: HashMap<Vec<u8>, u16> = HashMap::new();
    let mut w: Vec<u8> = Vec::new();
    for &byte in data {
        let mut next = w.clone();
        next.push(byte);
        if w.is_empty() || table.contains_key(&next) {
            w = next;
            continue;
        }
        codes.push(code(&table, &w));
        match 258 + table.len() as u16 {
            CLEARED => {
                codes.push(CLEAR);
                table.clear();
            }
            entry => {
                table.insert(next, entry);
            }
        }
        w = vec![byte];
    }
    if !w.is_empty() {
        codes.push(code(&table, &w));
    }
    codes.push(END);
    let (mut out, mut bits, mut held) = (Vec::new(), 0u32, 0u32);
    for c in codes {
        bits = bits << 9 | u32::from(c);
        held += 9;
        while held >= 8 {
            held -= 8;
            out.push((bits >> held) as u8);
        }
        bits &= (1 << held) - 1;
    }
    if held > 0 {
        out.push((bits << (8 - held)) as u8);
    }
    out
}

/// The pages `tokens` and `operations` read.
fn pages() -> Vec<Page> {
    let gray = "/W 32 /H 32 /BPC 8 /CS /G";
    let raw = pixels();
    let layout = Layout {
        colors: 1,
        bits_per_component: 8,
        columns: 32,
    };
    let flate = |predictor| {
        let encoder = Encoder::Flate {
            level: Level::DEFAULT,
            predictor,
        };
        encoded(&raw, encoder)
    };
    vec![
        // BT /F 12 Tf 14.4 TL 72 y Td (...) Tj T* [...] TJ ET: 15 tokens,
        // an array one of them, and 8 operations.
        lined("text", 600, 15, 8, |i| {
            format!(
                "BT /F{} 12 Tf 14.4 TL 72 {} Td (The quick brown fox jumps over the lazy dog.) Tj \
                 T* [(Kerned) -120 (text,) 250 (spaced.)] TJ ET\n",
                i % 4,
                720 - i % 50 * 14
            )
        }),
        // q, RG's 4, w's 2, J's 2, j's 2, d's 3, m's 3, two l's 3 each,
        // c's 7, h, S, rg's 4, re's 5, f and Q: 43 tokens, 16 operations.
        lined("paths", 550, 43, 16, |i| {
            let y = i % 70 * 10;
            format!(
                "q 0.5 0.25 0 RG 2 w 1 J 0 j [3 2] 0 d 10 {y} m 100 {y} l 100 100 l \
                 55.5 120 10 100 10 10 c h S 0 0 1 rg 20 {y} 60 60 re f Q\n"
            )
        }),
        images("images-unfiltered", gray, &raw),
        images(
            "images-flate",
            &format!("{gray} /F /Fl"),
            &flate(Prediction::None),
        ),
        images(
            "images-flate-png",
            &format!("{gray} /F /Fl /DP << /Predictor 14 /Columns 32 >>"),
            &flate(Prediction::Png(layout, Row::Paeth)),
        ),
        images("images-lzw", &format!("{gray} /F /LZW"), &lzw(&raw)),
        images(
            "images-array-space",
            "/W 32 /H 32 /BPC 8 /CS [/Indexed /DeviceRGB 3 <FF000000FF000000FFFFFFFF>]",
            &raw,
        ),
        // The tag, the list, BDC, the array, TJ and EMC: 6 tokens, 3
        // operations.
        lined("nested", 400, 6, 3, |i| {
            format!(
                "/Span << /MCID {i} /ActualText (nested) /Attr << /O /Layout /BBox [0 0 612 792] \
                 /Inner << /Deep [1 2 3 << /K [4 5 6] /L << /M [7 8] >> >>] >> >> >> BDC \
                 [(Kerned) -120 (text) 30 (here)] TJ EMC\n"
            )
        }),
    ]
}

/// `page` read whole, held to what it gives: its tokens, its operations,
/// and each inline image's stream its data, with no error.
fn held(group: &str, page: &Page) {
    let name = page.name;
    let none = ResCx::none();
    let read: Result<Vec<_>, _> = tokens(&page.bytes, &none, LIM).collect();
    let read = read.unwrap_or_else(|e| panic!("{group}/{name}: {e}"));
    assert_eq!(read.len(), page.tokens, "{group}/{name}: its tokens");
    let read: Result<Vec<_>, _> = operations(&page.bytes, &none, LIM).collect();
    let read = read.unwrap_or_else(|e| panic!("{group}/{name}: {e}"));
    assert_eq!(
        read.len(),
        page.operations,
        "{group}/{name}: its operations"
    );
    let streams: Vec<ImageStream> = (read.iter())
        .filter_map(|op| Some(op.inline_image()?.stream()))
        .collect();
    let expected: Vec<ImageStream> = (page.images.iter().cloned())
        .map(ImageStream::Data)
        .collect();
    assert_eq!(
        streams, expected,
        "{group}/{name}: each image's stream its data"
    );
}

fn tokens_group(c: &mut Criterion) {
    let none = ResCx::none();
    let mut group = c.benchmark_group("tokens");
    for page in pages() {
        held("tokens", &page);
        group.throughput(Throughput::Bytes(page.bytes.len() as u64));
        group.bench_function(page.name, |b| {
            b.iter(|| tokens(black_box(&page.bytes), &none, LIM).count())
        });
    }
    group.finish();
}

fn operations_group(c: &mut Criterion) {
    let none = ResCx::none();
    let mut group = c.benchmark_group("operations");
    for page in pages() {
        held("operations", &page);
        group.throughput(Throughput::Bytes(page.bytes.len() as u64));
        group.bench_function(page.name, |b| {
            b.iter(|| operations(black_box(&page.bytes), &none, LIM).count())
        });
    }
    group.finish();
}

/// How many lines the tagged page has, each two marked ranges.
const TAGGED: usize = 300;

/// A tagged page: each line text in a `q`...`Q` and a text object, marked
/// with a list written inline, in one of eight fonts and four ExtGStates;
/// then an artifact marked with one of four lists by name, in a `q`...`Q`
/// of its own.
fn tagged() -> Vec<u8> {
    let line = |i: usize| {
        format!(
            "/P << /MCID {i} >> BDC q 1 0 0 1 72 {} cm BT /F{} 12 Tf /GS{} gs \
             (Body text, tagged.) Tj ET Q EMC\n\
             /Artifact /MC{} BDC q 0.9 g 0 0 612 20 re f Q EMC\n",
            720 - i % 50 * 14,
            i % 8,
            i % 4,
            i % 4
        )
    };
    (0..TAGGED).map(line).collect::<String>().into_bytes()
}

/// The lists the tagged page names, `MC0` to `MC3`.
fn lists() -> Properties {
    Properties::new((0..4).map(|i| {
        let list = dict! { "Type" => "Pagination", "Subtype" => "Header" };
        (format!("MC{i}").into_bytes(), list)
    }))
}

fn passes_group(c: &mut Criterion) {
    let bytes = tagged();
    let page = Concat::new(&[&bytes]);
    let (none, props) = (ResCx::none(), lists());
    let read = balance(&page, &none, LIM).unwrap_or_else(|e| panic!("passes/balance: {e}"));
    assert_eq!(
        (read.net, read.min_prefix),
        (0, 0),
        "passes/balance: each q restored"
    );
    let found = marked(&page, &props, &none, LIM).unwrap_or_else(|e| panic!("passes/marked: {e}"));
    assert_eq!(found.len(), 2 * TAGGED, "passes/marked: its ranges");
    let listed = (found.iter())
        .filter(|m| m.props.as_ref().and_then(Props::dict).is_some())
        .count();
    assert_eq!(listed, 2 * TAGGED, "passes/marked: each range's list");
    let names = names_used(&page, &none, LIM, &[]).unwrap_or_else(|e| panic!("passes/names: {e}"));
    let counted =
        [ResKind::Font, ResKind::ExtGState, ResKind::Properties].map(|kind| names.of(kind).count());
    assert_eq!(
        (counted, names.len()),
        ([8, 4, 4], 16),
        "passes/names-used: fonts, ExtGStates, lists"
    );
    let mut group = c.benchmark_group("passes");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("balance", |b| {
        b.iter(|| balance(black_box(&page), &none, LIM))
    });
    group.bench_function("marked", |b| {
        b.iter(|| marked(black_box(&page), &props, &none, LIM))
    });
    group.bench_function("names-used", |b| {
        b.iter(|| names_used(black_box(&page), &none, LIM, &[]))
    });
    group.finish();
}

/// `text` as one value, read by pdf-objects' parser.
fn value(text: &str) -> Object {
    let mut parser = Parser::at(text.as_bytes(), 0);
    (parser.value(&mut ())).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// The objects the spaces refer to, by number: an ICC profile's stream
/// (5), a spot colorant's exponential tint transform into DeviceCMYK (6),
/// and a PostScript tint transform of three inputs into DeviceCMYK (7). A
/// dictionary with `/Length` is a stream's; no stream's data is read, so
/// none is placed.
const SPACE_OBJECTS: &[(u32, &str)] = &[
    (
        5,
        "<< /N 3 /Alternate /DeviceRGB /Range [0 1 0 1 0 1] /Length 3144 >>",
    ),
    (
        6,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0.53 1 0] /N 1 >>",
    ),
    (
        7,
        "<< /FunctionType 4 /Domain [0 1 0 1 0 1] /Range [0 1 0 1 0 1 0 1] /Length 62 >>",
    ),
];

/// A space of each kind, as a `/ColorSpace` dictionary holds it, with its
/// components as PDFium counts them.
const SPACES: &[(&str, u32)] = &[
    ("/DeviceGray", 1),
    (
        "[/CalRGB << /WhitePoint [0.9505 1 1.089] /Gamma [2.2 2.2 2.2] \
         /Matrix [0.4124 0.2126 0.0193 0.3576 0.7152 0.1192 0.1805 0.0722 0.9505] >>]",
        3,
    ),
    ("[/ICCBased 5 0 R]", 3),
    ("[/Indexed /DeviceRGB 3 <FFFFFF FF8000 0080FF 000000>]", 1),
    ("[/Separation /Spot#20Orange /DeviceCMYK 6 0 R]", 1),
    (
        "[/DeviceN [/Cyan /Magenta /Spot#20Orange] /DeviceCMYK 7 0 R]",
        3,
    ),
    ("[/Pattern /DeviceRGB]", 4),
];

/// How many color spaces the resources dictionary `rescx` reads names.
const SPACE_NAMES: usize = 100;

fn rescx_group(c: &mut Criterion) {
    let objects: BTreeMap<ObjectId, Object> = (SPACE_OBJECTS.iter())
        .map(|&(number, text)| {
            let object = value(text);
            let stream = (object.as_dict())
                .filter(|dict| dict.has(b"Length"))
                .map(|dict| Object::stream(dict.clone()));
            (ObjectId::new(number, 0), stream.unwrap_or(object))
        })
        .collect();
    let resolve = |id: ObjectId| objects.get(&id);
    let mut text = String::from("<< /ProcSet [/PDF /ImageC] /ColorSpace <<");
    for (at, (space, _)) in (0..SPACE_NAMES).zip(SPACES.iter().cycle()) {
        text.push_str(&format!(" /CS{at} {space}"));
    }
    text.push_str(" >> >>");
    let resources = value(&text);
    let read = ResCx::read(resolve, Some(&resources), LIM);
    let read = read.unwrap_or_else(|e| panic!("rescx/read-100: {e}"));
    let expected = ResCx::new(
        (0..SPACE_NAMES)
            .zip(SPACES.iter().cycle())
            .map(|(at, &(_, count))| (format!("CS{at}").into_bytes(), count)),
    );
    assert!(read == expected, "rescx/read-100: each name's count");
    assert_eq!(read.capped().count(), 0, "rescx/read-100: no cap met");
    let written = text.len() + SPACE_OBJECTS.iter().map(|(_, t)| t.len()).sum::<usize>();
    let mut group = c.benchmark_group("rescx");
    group.throughput(Throughput::Bytes(written as u64));
    group.bench_function("read-100", |b| {
        b.iter(|| ResCx::read(resolve, Some(black_box(&resources)), LIM))
    });
    group.finish();
}

/// The bytes of the font program `res` digests.
const PROGRAM: usize = 32 << 10;

/// A TrueType font's objects, as a resource holds them: the font, its
/// widths, its descriptor and its program, a stream whose data is in
/// memory; and how many bytes they are, as written.
fn font() -> (Vec<Object>, usize) {
    let widths: Vec<String> = (0..224).map(|i| (250 + i * 37 % 700).to_string()).collect();
    let texts = [
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Body /FirstChar 32 \
         /LastChar 255 /Widths 1 0 R /FontDescriptor 2 0 R /Encoding /WinAnsiEncoding >>"
            .to_string(),
        format!("[{}]", widths.join(" ")),
        "<< /Type /FontDescriptor /FontName /ABCDEF+Body /Flags 32 /FontBBox [-166 -225 1000 931] \
         /ItalicAngle 0 /Ascent 931 /Descent -225 /CapHeight 718 /StemV 88 /FontFile2 3 0 R >>"
            .to_string(),
    ];
    let program: Vec<u8> = (0..PROGRAM).map(|i| (i * 31 % 251) as u8).collect();
    let mut objects: Vec<Object> = texts.iter().map(String::as_str).map(value).collect();
    let stream = Stream::new(dict! { "Length1" => PROGRAM as i64 }, program);
    objects.push(Object::from(stream));
    let written = texts.iter().map(String::len).sum::<usize>() + PROGRAM;
    (objects, written)
}

fn res_group(c: &mut Criterion) {
    let (objects, written) = font();
    let a = Res::of(objects.clone()).expect("res: its data in memory");
    let b = Res::of(font().0).expect("res: its data in memory");
    let clone = a.clone();
    assert_eq!(ResId::of(&objects), a.id, "res/id: the resource's digest");
    assert_eq!(a.id, b.id, "res/eq-apart: one digest");
    assert!(a == b, "res/eq-apart: equal");
    assert!(a == clone, "res/eq-clone: equal");
    let mut group = c.benchmark_group("res");
    group.throughput(Throughput::Bytes(written as u64));
    group.bench_function("id", |bench| bench.iter(|| ResId::of(black_box(&objects))));
    group.bench_function("eq-clone", |bench| {
        bench.iter(|| black_box(&a) == black_box(&clone))
    });
    group.bench_function("eq-apart", |bench| {
        bench.iter(|| black_box(&a) == black_box(&b))
    });
    group.finish();
}

/// How many times the written page's calls are made, each a line of text
/// marked with a list of its own, a filled band, and an image.
const WRITTEN: usize = 100;

/// The operations each of the written page's lines is: q, rg, re, f, BDC,
/// BT, Tf, Td, Tj, ET, EMC, gs, the image's q, cm, Do and Q, and Q.
const WRITTEN_OPERATIONS: usize = 17;

/// The written page's resources: its font and its image, an image stream
/// whose data is in memory.
fn drawn() -> (Res, Res) {
    let font = dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" };
    let image = dict! {
        "Type" => "XObject", "Subtype" => "Image", "Width" => 32, "Height" => 32,
        "BitsPerComponent" => 8, "ColorSpace" => "DeviceGray",
    };
    let image = Object::from(Stream::new(image, pixels()));
    let font = Res::dict(font).expect("write/calls: a font of names");
    let image = Res::of([image]).expect("write/calls: an image in memory");
    (font, image)
}

/// The written page's calls.
fn calls(font: &Res, image: &Res) -> ContentWriter {
    let mut w = ContentWriter::new();
    for i in 0..WRITTEN {
        let y = 700.0 - i as f32 * 6.5;
        w.save();
        w.fill_color(Color::rgb8([(i * 2) as u8, 64, 128]));
        w.rect(72.0, y, 468.0, 5.5);
        w.paint(Paint::Fill);
        let list = Res::dict(dict! { "MCID" => i as i64 }).expect("write/calls: a list");
        w.begin_marked("Span", list);
        w.begin_text();
        w.font(font.clone(), 12.0);
        w.text_move(72.0, y);
        w.show(b"The quick brown fox jumps over the lazy dog.");
        w.end_text();
        w.end_marked();
        w.gstate(GState {
            fill_alpha: Some(0.5),
            ..GState::default()
        });
        w.image(image.clone(), 500.0, y, 6.0, 6.0);
        w.restore();
    }
    w
}

/// How many of the names a resources dictionary binds `named-100` keeps
/// clear of are the page's own names for its resources, bound there to
/// another object; the rest are other names.
const BOUND_ELSEWHERE: usize = 50;

fn write_group(c: &mut Criterion) {
    let (font, image) = drawn();
    let w = calls(&font, &image);
    let (content, needs) = w.named(&Reserved::default());
    let read: Result<Vec<_>, _> = operations(&content, &ResCx::none(), LIM).collect();
    let read = read.unwrap_or_else(|e| panic!("write/calls: {e}"));
    let expected = WRITTEN * WRITTEN_OPERATIONS;
    assert_eq!(read.len(), expected, "write/calls: its operations");
    // A list for each line, the font, the ExtGState and the image.
    assert_eq!(needs.0.len(), WRITTEN + 3, "write/calls: its needs");

    // A resources dictionary binding the first needs' names to another
    // font, object 1, and as many other fonts' names again.
    let mut bound: BTreeMap<ResKind, Dict> = BTreeMap::new();
    for need in &needs.0[..BOUND_ELSEWHERE] {
        let category = bound.entry(need.kind).or_default();
        category.push(&need.name, Object::reference(1));
    }
    let fonts = bound.entry(ResKind::Font).or_default();
    for i in 0..100 - BOUND_ELSEWHERE {
        fonts.push(format!("R{i}"), Object::reference(1));
    }
    let mut resources = Dict::new();
    for (kind, category) in bound {
        resources.push(kind.key(), category);
    }
    let resources = Object::from(resources);
    let other = Object::from(dict! { "Type" => "Font", "BaseFont" => "Courier" });
    let resolve = |id: ObjectId| (id == ObjectId::new(1, 0)).then_some(&other);
    let reserved = Reserved::read(resolve, Some(&resources), LIM);
    assert_eq!(
        reserved.names().count(),
        100,
        "write/reserved-100: the names it binds"
    );
    let (named, renamed) = w.named(&reserved);
    assert_eq!(renamed.0.len(), needs.0.len(), "write/named-100: its needs");
    for (i, (was, now)) in needs.0.iter().zip(&renamed.0).enumerate() {
        assert_eq!(
            (was.kind, &was.res),
            (now.kind, &now.res),
            "write/named-100: need {i} the same resource"
        );
        let clear = !(needs.0[..BOUND_ELSEWHERE].iter())
            .any(|taken| taken.kind == now.kind && taken.name == now.name);
        assert!(
            clear,
            "write/named-100: need {i} named clear of what's bound"
        );
    }

    let mut group = c.benchmark_group("write");
    group.throughput(Throughput::Bytes(content.len() as u64));
    group.bench_function("calls", |b| {
        b.iter(|| calls(black_box(&font), &image).finish(&Reserved::default()))
    });
    group.bench_function("reserved-100", |b| {
        b.iter(|| Reserved::read(resolve, Some(black_box(&resources)), LIM))
    });
    group.throughput(Throughput::Bytes(named.len() as u64));
    group.bench_function("named-100", |b| {
        b.iter(|| black_box(&w).named(black_box(&reserved)))
    });
    group.finish();
}

/// How many marked ranges the spliced page has, each an artifact.
const SPLICED: usize = 20;

/// The spliced page: each line a line of text, then a band drawn as an
/// artifact, a marked range in a `q`...`Q` of its own but where `unsafe_at`
/// says, whose color it sets outside one, so that taking it out would
/// color the text after it; and the page with every artifact taken out.
fn spliced(unsafe_at: Option<usize>) -> (Vec<u8>, Vec<u8>) {
    let (mut page, mut kept) = (String::new(), String::new());
    for i in 0..SPLICED {
        let y = 700 - 30 * i;
        let text = format!("BT /F1 12 Tf 72 {y} Td (Body line {i}.) Tj ET\n");
        let band = match unsafe_at == Some(i) {
            false => format!("/Artifact BMC q 0.8 g 0 {} 612 12 re f Q EMC", y - 15),
            true => format!("/Artifact BMC 0.8 g 0 {} 612 12 re f EMC", y - 15),
        };
        page.push_str(&format!("{text}{band}\n"));
        kept.push_str(&format!("{text}\n"));
    }
    (page.into_bytes(), kept.into_bytes())
}

/// The spans of `page`'s marked ranges.
fn spans(page: &Concat, group: &str) -> Vec<SpanAcross> {
    let (props, none) = (Properties::default(), ResCx::none());
    let found = marked(page, &props, &none, LIM);
    let found = found.unwrap_or_else(|e| panic!("{group}: {e}"));
    assert_eq!(found.len(), SPLICED, "{group}: its marked ranges");
    found.iter().map(|m| m.span).collect()
}

fn splice_group(c: &mut Criterion) {
    let none = ResCx::none();
    let (bytes, kept) = spliced(None);
    let page = Concat::new(&[&bytes]);
    let drop = spans(&page, "splice/splice-20");
    let made = splice(&page, &none, LIM, &drop).unwrap_or_else(|e| panic!("splice/splice-20: {e}"));
    assert_eq!(made.len(), 1, "splice/splice-20: one stream");
    assert!(*made[0] == kept[..], "splice/splice-20: every artifact out");
    let groups: Vec<Vec<SpanAcross>> = drop.iter().map(|&span| vec![span]).collect();
    let each = splice_each(&page, &none, LIM, &groups);
    let each = each.unwrap_or_else(|e| panic!("splice/each-20: {e}"));
    assert_eq!(each.taken, [true; SPLICED], "splice/each-20: every group");
    let made = each.streams(&page, &none, LIM, &[]);
    let made = made.unwrap_or_else(|e| panic!("splice/each-20: {e}"));
    let made = made.unwrap_or_else(|| panic!("splice/each-20: a stream changed"));
    assert!(*made[0] == kept[..], "splice/each-20: every artifact out");

    let refused_at = SPLICED / 2;
    let (unsafe_bytes, _) = spliced(Some(refused_at));
    let unsafe_page = Concat::new(&[&unsafe_bytes]);
    let unsafe_drop = spans(&unsafe_page, "splice/each-20-one-refused");
    let unsafe_groups: Vec<Vec<SpanAcross>> = unsafe_drop.iter().map(|&s| vec![s]).collect();
    let each = splice_each(&unsafe_page, &none, LIM, &unsafe_groups);
    let each = each.unwrap_or_else(|e| panic!("splice/each-20-one-refused: {e}"));
    let mut expected = [true; SPLICED];
    expected[refused_at] = false;
    assert_eq!(
        each.taken, expected,
        "splice/each-20-one-refused: all but the one"
    );

    let mut group = c.benchmark_group("splice");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("splice-20", |b| {
        b.iter(|| splice(black_box(&page), &none, LIM, &drop))
    });
    group.bench_function("each-20", |b| {
        b.iter(|| splice_each(black_box(&page), &none, LIM, &groups))
    });
    group.throughput(Throughput::Bytes(unsafe_bytes.len() as u64));
    group.bench_function("each-20-one-refused", |b| {
        b.iter(|| splice_each(black_box(&unsafe_page), &none, LIM, &unsafe_groups))
    });
    group.finish();
}

/// How many streams are joined, each 30 lines of the text page.
const JOINED: usize = 20;

fn join_group(c: &mut Criterion) {
    let pages = pages();
    let rows: Vec<&[u8]> = pages[0].bytes.split_inclusive(|&b| b == b'\n').collect();
    let streams: Vec<Vec<u8>> = (rows.chunks(rows.len() / JOINED).take(JOINED))
        .map(|chunk| chunk.concat())
        .collect();
    let expected = streams.join(&b' ');
    let push = |streams: &[Vec<u8>]| {
        let mut join = Join::new(LIM);
        for stream in streams {
            join.push(stream)
                .unwrap_or_else(|e| panic!("join/push-20: {e}"));
        }
        join.finish()
    };
    let push_with = |streams: &[Vec<u8>]| {
        let mut join = Join::new(LIM);
        for stream in streams {
            let copied = join.push_with(|out, room| {
                if stream.len() > room {
                    return Err(());
                }
                out.extend_from_slice(stream);
                Ok(())
            });
            copied.unwrap_or_else(|_| panic!("join/push-with-20: refused"));
        }
        join.finish()
    };
    for (name, joined) in [
        ("push-20", push(&streams)),
        ("push-with-20", push_with(&streams)),
    ] {
        assert_eq!(joined.stream_count(), JOINED, "join/{name}: its streams");
        assert!(
            joined.bytes() == expected,
            "join/{name}: a space between each"
        );
    }
    let mut group = c.benchmark_group("join");
    group.throughput(Throughput::Bytes(expected.len() as u64));
    group.bench_function("push-20", |b| b.iter(|| push(black_box(&streams))));
    group.bench_function("push-with-20", |b| {
        b.iter(|| push_with(black_box(&streams)))
    });
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    tokens_group(&mut criterion);
    operations_group(&mut criterion);
    passes_group(&mut criterion);
    rescx_group(&mut criterion);
    res_group(&mut criterion);
    write_group(&mut criterion);
    splice_group(&mut criterion);
    join_group(&mut criterion);
    criterion.final_summary();
}
