//! pdf-objects timed on inputs made here, so that a change to its parser,
//! its writer, its dictionaries, its walks of references or its text
//! strings can be judged before and after it, by `cargo bench -p
//! pdf-objects`:
//!
//! - `parse`: indirect objects as a file's body holds them (a page, its
//!   content stream, the page tree, fonts and annotations), each read
//!   through its header, its body and a stream's data by its `/Length`,
//!   then past its `endobj`; arrays of numbers; arrays of literal and
//!   hexadecimal strings, with escapes; arrays of names, with `#` escapes;
//!   and dictionaries of 1,000 keys (`wide`). Each is read by ISO 32000-2's
//!   rules (`iso`) and PDFium's (`pdfium`) with a sink that wants no
//!   faults, and by PDFium's with one that wants them (`pdfium-told`), for
//!   which the parser looks at each token as ISO 32000-2 reads it too.
//!   Objects with 0x80 and 0xFF between their tokens, white-space by
//!   PDFium's rules alone, are read by PDFium's (`high`).
//! - `write`: the objects `parse` reads, written back as indirect objects;
//!   arrays of reals; and arrays of strings, plain, with escapes, and
//!   hexadecimal.
//! - `dict`: a dictionary of 16 entries and one of 1,000 (`few`, `many`),
//!   each with keys written twice: its every key looked up (`get`), its
//!   entries walked (`iter`, `iter_mut`, `last_entries`), those a later
//!   one overrides found (`duplicates`), and its keys counted (`len`).
//! - `object`: the references in a page tree's pages walked (`references`)
//!   and each named anew (`renumber`).
//! - `text`: text strings decoded, and texts encoded, in PDFDocEncoding,
//!   UTF-16BE and UTF-8.
//!
//! Throughput is in bytes of what's read or written, but for `dict` and
//! `object`, which read none: there it's in entries and references.
//! Nothing is read from disk, and the inputs are the same on every run.

use std::hint::black_box;

use criterion::measurement::WallTime;
use criterion::{BatchSize, BenchmarkGroup, BenchmarkId, Criterion, Throughput};
use pdf_objects::parse::{Body, Fault, Parser, Sink, StreamHead};
use pdf_objects::text::{self, Encoding};
use pdf_objects::{Dict, Object, ObjectId, Span, serialize};
use pdf_tokenizer::{Rules, Tokenizer};

/// A page, its content stream, the page tree's root, the fonts and the
/// annotations the page names, as a writer lays them out.
const OBJECTS: &[u8] = b"1 0 obj\n\
    << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [0 0 612.0 792.0]\n\
    /Resources << /Font << /F1 5 0 R /F2 6 0 R >> /ProcSet [/PDF /Text /ImageC] /XObject << /Im0 7 0 R >> >>\n\
    /Contents 4 0 R /Annots [8 0 R 9 0 R] /Rotate 0 /Group << /S /Transparency /CS /DeviceRGB >> >>\n\
    endobj\n\
    2 0 obj\n<< /Type /Pages /Kids [1 0 R 10 0 R 11 0 R 12 0 R] /Count 4 >>\nendobj\n\
    4 0 obj\n<< /Length 86 /Filter /FlateDecode >>\nstream\r\n\
    q 1 0 0 1 72 720 cm BT /F1 12 Tf 14.4 TL (Hello, world) Tj T* [(W) 120 (orld)] TJ ET Q\n\
    endstream\nendobj\n\
    5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n\
    6 0 obj\n<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Arial#20Bold /FirstChar 32 /LastChar 47\n\
    /Widths [278 278 355 556 556 889 667 191 333 333 389 584 278 333 278 278] /FontDescriptor 13 0 R >>\nendobj\n\
    8 0 obj\n<< /Type /Annot /Subtype /Link /Rect [72 700.5 300 712] /Border [0 0 0] /A << /S /URI /URI (https://example.org/a#b) >> >>\nendobj\n\
    9 0 obj\n<< /Type /Annot /Subtype /Text /Rect [72 650 92 670] /Contents <FEFF00480065006C006C006F> /M (D:20261106120000+00'00') >>\nendobj\n\
    % a comment between objects\n";

/// Objects with 0x80 and 0xFF where white-space goes: between a header and
/// its value, a dictionary's keys and values, an array's elements and a
/// reference's parts, before `endobj`, and around a stream's `stream` and
/// `endstream`. White-space by PDFium's rules, and regular characters by ISO
/// 32000-2's, which then reads other objects.
const HIGH: &[u8] =
    b"1 0 obj\x80<< /Type\xff/Page /Parent 2\x800 R /MediaBox [0\xff0 612\x80792] >>\x80endobj\n\
    4 0 obj\n<< /Length 10 >>\xffstream\n0123456789\n\x80endstream\nendobj\n\
    5 0 obj\n<<\xff/Type /Font /Subtype\x80/Type1 /BaseFont /Helvetica\xff>>\nendobj\n";

/// Integers and reals, as a page's boxes and a font's widths hold them.
const NUMBERS: &[u8] = b"[0 1 -1 42 612 792 65535 123456789 -2147483648 14.4 -0.5 .5 1.0 0.0001 720.25 -72.5 +17 -.002 0 0 1 0 0 1 72 720 278 556 889]\n";

/// Literal strings, plain and with escapes, a line continuation and
/// parentheses nested, and hexadecimal ones, with white-space too.
const STRINGS: &[u8] = b"[(Hello, world) (caf\\351 \\(1\\) \\\\ and a\\nbreak) <48656C6C6F20776F726C64> <FEFF00480069> \
    (D:20261106120000+00'00') (https://example.org/a#b) (nested (parens) here) (a line \\\ncontinued) <4 8 6 5 6 C>]\n";

/// Names, half of them with an escape.
const NAMES: &[u8] = b"[/Type /Name#20with#20spaces /A#2FB /Caf#E9 /Plain /Hash#23 /Font /F1 /Length /Lo#6Eg#20escaped#20name /MediaBox /B#41SE]\n";

/// A dictionary of 1,000 keys, each value an integer, a name or a
/// reference in turn.
fn wide() -> Vec<u8> {
    let mut dict = b"<<".to_vec();
    for i in 0..1000 {
        let value = match i % 3 {
            0 => format!(" /K{i} {i}"),
            1 => format!(" /K{i} /N{i}"),
            _ => format!(" /K{i} {i} 0 R"),
        };
        dict.extend_from_slice(value.as_bytes());
    }
    dict.extend_from_slice(b" >>\n");
    dict
}

/// About 1 MB, so that a run of the parser is a few milliseconds and the
/// input is past the faster caches.
const INPUT_LEN: usize = 1 << 20;

/// `piece` repeated until the input holds `len` bytes or more, and how
/// many times it is.
fn repeated(piece: &[u8], len: usize) -> (Vec<u8>, usize) {
    let mut input = Vec::with_capacity(len + piece.len());
    let mut times = 0;
    while input.len() < len {
        input.extend_from_slice(piece);
        times += 1;
    }
    (input, times)
}

/// A sink that wants every fault and counts them, as a checker that keeps
/// them is told them.
#[derive(Default)]
struct Counted(usize);

impl Sink<'_> for Counted {
    fn fault(&mut self, _: Span, _: Fault<'_>) {
        self.0 += 1;
    }
}

/// How a `parse` input is read.
trait Reading {
    /// What `input` holds, read by `rules`, its faults told to `sink`: how
    /// many it read before the first it couldn't.
    fn read<'a>(input: &'a [u8], rules: Rules, sink: &mut (impl Sink<'a> + ?Sized)) -> usize;
}

/// Indirect objects, one after another, as a file's reader reads them
/// ([`next_object`]).
struct Objects;

impl Reading for Objects {
    fn read<'a>(input: &'a [u8], rules: Rules, sink: &mut (impl Sink<'a> + ?Sized)) -> usize {
        let mut parser = Parser::at(input, 0).rules(rules);
        let mut n = 0;
        while let Some(object) = next_object(&mut parser, input, rules, sink) {
            black_box(object);
            n += 1;
        }
        n
    }
}

/// The next indirect object of `input` that `parser` reads by `rules`, as
/// a file's reader reads one: its header, then its body, and a stream's
/// data by its `/Length`, or up to `endstream` where that's wrong; and the
/// parser then past its `endobj`, which the parser looks for without
/// reading. None where any of them isn't there.
fn next_object<'a>(
    parser: &mut Parser<'a>,
    input: &'a [u8],
    rules: Rules,
    sink: &mut (impl Sink<'a> + ?Sized),
) -> Option<(ObjectId, Object)> {
    let id = parser.header().ok()?;
    let object = match parser.body(id, sink).ok()? {
        Body::Object(object) => object,
        Body::Stream(head) => {
            let data = data(&head);
            parser.stream(head, data, sink).ok()?
        }
    };
    let mut t = Tokenizer::at(input, parser.offset()).with_rules(rules);
    if !t.next_significant()?.is_keyword(b"endobj") {
        return None;
    }
    *parser = Parser::at(input, t.offset()).rules(rules);
    Some((id, object))
}

/// A stream's data by its `/Length`, or up to `endstream` where that's
/// wrong, as a file's reader chooses it.
fn data(head: &StreamHead<'_>) -> Span {
    (head.dict().get(b"Length"))
        .and_then(Object::as_integer)
        .and_then(|length| usize::try_from(length).ok())
        .and_then(|length| head.declared(length))
        .unwrap_or_else(|| head.scanned())
}

/// Values, one after another.
struct Values;

impl Reading for Values {
    fn read<'a>(input: &'a [u8], rules: Rules, sink: &mut (impl Sink<'a> + ?Sized)) -> usize {
        let mut parser = Parser::at(input, 0).rules(rules);
        let mut n = 0;
        while let Ok(value) = parser.value(sink) {
            black_box(value);
            n += 1;
        }
        n
    }
}

/// `piece`, repeated to [`INPUT_LEN`], read as `R` reads it under each
/// side: by ISO 32000-2's rules, where `iso`, and PDFium's, with a sink
/// that wants no faults; and by PDFium's with one that wants them. Each
/// side is held first to reading every one of the input's objects or
/// values, so that none is timed reading less.
fn parse_benches<R: Reading>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    piece: &[u8],
    iso: bool,
) {
    let each = R::read(piece, Rules::Pdfium, &mut ());
    let (input, times) = repeated(piece, INPUT_LEN);
    let input = input.as_slice();
    let count = each * times;
    assert!(each > 0);
    if iso {
        assert_eq!(R::read(piece, Rules::Iso, &mut ()), each, "{name}");
        assert_eq!(R::read(input, Rules::Iso, &mut ()), count, "{name}");
    }
    assert_eq!(R::read(input, Rules::Pdfium, &mut ()), count, "{name}");
    let mut sink = Counted::default();
    assert_eq!(R::read(input, Rules::Pdfium, &mut sink), count, "{name}");
    group.throughput(Throughput::Bytes(input.len() as u64));
    if iso {
        group.bench_function(BenchmarkId::new("iso", name), |b| {
            b.iter(|| R::read(input, Rules::Iso, &mut ()))
        });
    }
    group.bench_function(BenchmarkId::new("pdfium", name), |b| {
        b.iter(|| R::read(input, Rules::Pdfium, &mut ()))
    });
    group.bench_function(BenchmarkId::new("pdfium-told", name), |b| {
        b.iter(|| {
            let mut sink = Counted::default();
            R::read(input, Rules::Pdfium, &mut sink) + sink.0
        })
    });
}

fn parse_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    parse_benches::<Objects>(&mut group, "objects", OBJECTS, true);
    parse_benches::<Values>(&mut group, "numbers", NUMBERS, true);
    parse_benches::<Values>(&mut group, "strings", STRINGS, true);
    parse_benches::<Values>(&mut group, "names", NAMES, true);
    parse_benches::<Values>(&mut group, "wide", &wide(), true);
    parse_benches::<Objects>(&mut group, "high", HIGH, false);
    group.finish();
}

/// Each indirect object of `input` with its number, as the parser reads
/// them by ISO 32000-2's rules, a stream's data then held in memory, so
/// that each is written whole.
fn read_back(input: &[u8]) -> Vec<(ObjectId, Object)> {
    let mut parser = Parser::at(input, 0);
    let mut objects = Vec::new();
    while let Some((id, mut object)) = next_object(&mut parser, input, Rules::Iso, &mut ()) {
        if let Some(stream) = object.as_stream_mut() {
            let at = stream.data.at().expect("its data's place");
            stream.set_bytes(input[at.start..at.end].to_vec());
        }
        objects.push((id, object));
    }
    objects
}

/// `objects` written as indirect objects, at the end of `out`.
fn write_objects(out: &mut Vec<u8>, objects: &[(ObjectId, Object)]) {
    for (id, object) in objects {
        serialize::indirect(out, *id, object).expect("written");
    }
}

/// Reals as a page's content and its objects hold them.
const REALS: [f64; 16] = [
    0.5, -0.5, 2.5, 14.4, 720.25, -72.5, 612.0, 0.0001, 1.0, 100.125, 0.333333, -1234.5678, 25.4,
    0.07, 65535.5, 1e-5,
];

fn write_group(c: &mut Criterion) {
    let (input, _) = repeated(OBJECTS, INPUT_LEN);
    let objects = read_back(&input);
    let reals: Vec<Object> = (REALS.iter().cycle().take(1 << 16))
        .map(|&x| Object::from(x))
        .collect();
    let strings: Vec<Object> = (0..1 << 14)
        .map(|i| match i % 4 {
            0 => Object::bytes(&b"Hello, world"[..]),
            1 => Object::bytes(&b"caf\xe9 (1) \\ a line\rbreak and (one unbalanced"[..]),
            2 => Object::hex(&b"\xfe\xff\x00H\x00i\x00!"[..]),
            _ => Object::text("R\u{e9}sum\u{e9} \u{2014} 2026"),
        })
        .collect();
    let reals = Object::from(reals);
    let strings = Object::from(strings);
    let mut group = c.benchmark_group("write");
    let mut out = Vec::new();
    write_objects(&mut out, &objects);
    group.throughput(Throughput::Bytes(out.len() as u64));
    group.bench_function("objects", |b| {
        b.iter(|| {
            out.clear();
            write_objects(&mut out, &objects);
            out.len()
        })
    });
    for (name, value) in [("reals", &reals), ("strings", &strings)] {
        out.clear();
        serialize::write(&mut out, value).expect("written");
        group.throughput(Throughput::Bytes(out.len() as u64));
        group.bench_function(name, |b| {
            b.iter(|| {
                out.clear();
                serialize::write(&mut out, value).expect("written");
                out.len()
            })
        });
    }
    group.finish();
}

/// A font's keys, for a dictionary of few entries.
const FONT_KEYS: [&str; 14] = [
    "Type",
    "Subtype",
    "BaseFont",
    "FirstChar",
    "LastChar",
    "Widths",
    "FontDescriptor",
    "Encoding",
    "ToUnicode",
    "Name",
    "Resources",
    "FontMatrix",
    "FontBBox",
    "CharProcs",
];

/// A dictionary of `len` entries, a font's keys and numbered ones past
/// them, and every key it holds; one entry in every eight written again
/// later, as a key written twice.
fn dictionary(len: usize) -> (Dict, Vec<Vec<u8>>) {
    let mut keys: Vec<Vec<u8>> = Vec::new();
    let mut dict = Dict::new();
    for i in 0..len {
        let key = match (i % 8, FONT_KEYS.get(i)) {
            (7, _) => keys[i / 2].clone(),
            (_, Some(key)) => key.as_bytes().to_vec(),
            (_, None) => format!("K{i}").into_bytes(),
        };
        if !keys.contains(&key) {
            keys.push(key.clone());
        }
        dict.push(&key, i as i64);
    }
    (dict, keys)
}

fn dict_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("dict");
    for (name, len) in [("few", 16), ("many", 1000)] {
        let (mut dict, keys) = dictionary(len);
        assert!(dict.len() < len && dict.duplicates().count() == len - dict.len());
        group.throughput(Throughput::Elements(keys.len() as u64));
        group.bench_function(BenchmarkId::new("get", name), |b| {
            b.iter(|| keys.iter().filter_map(|key| dict.get(key)).count())
        });
        group.throughput(Throughput::Elements(len as u64));
        group.bench_function(BenchmarkId::new("iter", name), |b| {
            b.iter(|| dict.iter().map(|(key, _)| key.len()).sum::<usize>())
        });
        group.bench_function(BenchmarkId::new("last_entries", name), |b| {
            b.iter(|| {
                dict.last_entries()
                    .map(|entry| entry.key.len())
                    .sum::<usize>()
            })
        });
        group.bench_function(BenchmarkId::new("duplicates", name), |b| {
            b.iter(|| dict.duplicates().count())
        });
        group.bench_function(BenchmarkId::new("len", name), |b| b.iter(|| dict.len()));
        group.bench_function(BenchmarkId::new("iter_mut", name), |b| {
            b.iter(|| {
                (dict.iter_mut())
                    .map(|(key, value)| key.len() + usize::from(value.is_null()))
                    .sum::<usize>()
            })
        });
    }
    group.finish();
}

/// A page as a page tree holds it, naming its parent, its content, its
/// fonts, an image and its annotations.
const PAGE: &[u8] = b"<< /Type /Page /Parent 2 0 R /Contents 10 0 R /MediaBox [0 0 612 792]\n\
    /Resources << /Font << /F1 5 0 R /F2 6 0 R >> /XObject << /Im0 7 0 R >> >> /Annots [8 0 R 9 0 R] >>\n";

fn object_group(c: &mut Criterion) {
    let mut pages = b"[".to_vec();
    for _ in 0..1000 {
        pages.extend_from_slice(PAGE);
    }
    pages.push(b']');
    let pages = Parser::at(&pages, 0).value(&mut ()).expect("the pages");
    let references = pages.references().count();
    assert_eq!(references, 7000);
    let mut group = c.benchmark_group("object");
    group.throughput(Throughput::Elements(references as u64));
    group.bench_function("references", |b| b.iter(|| pages.references().count()));
    group.bench_function("renumber", |b| {
        b.iter_batched(
            || pages.clone(),
            |mut pages| {
                pages.renumber(|id| Some(ObjectId::new(id.number + 100, id.generation)));
                pages
            },
            BatchSize::LargeInput,
        )
    });
    group.finish();
}

/// Texts as a document's outline and information hold them, in characters
/// PDFDocEncoding has.
const LATIN: [&str; 4] = [
    "Annual Report 2026",
    "Chapter 3: Results and Discussion",
    "R\u{e9}sum\u{e9} \u{2014} caf\u{e9} \u{2022} na\u{ef}ve",
    "\u{201c}Quoted\u{201d} \u{2026} \u{fb01}nal \u{2013} \u{dc}ber",
];

/// Texts with characters PDFDocEncoding hasn't.
const WIDE: [&str; 4] = [
    "\u{6771}\u{4eac} 2026 \u{5e74}\u{6b21}\u{5831}\u{544a}",
    "\u{395}\u{3bb}\u{3bb}\u{3b7}\u{3bd}\u{3b9}\u{3ba}\u{3ac} \u{3ba}\u{3b5}\u{3af}\u{3bc}\u{3b5}\u{3bd}\u{3b1}",
    "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}, \u{43c}\u{438}\u{440}",
    "Mixed: caf\u{e9} \u{6771}\u{4eac} \u{2713}",
];

fn text_group(c: &mut Criterion) {
    let sides: [(&str, &[&str], Encoding); 3] = [
        ("pdfdoc", &LATIN, Encoding::PdfDoc),
        ("utf16be", &WIDE, Encoding::Utf16Be),
        ("utf8", &WIDE, Encoding::Utf8),
    ];
    let mut group = c.benchmark_group("text");
    for (name, texts, encoding) in sides {
        let texts: Vec<&str> = texts.iter().copied().cycle().take(1024).collect();
        let strings: Vec<Vec<u8>> = (texts.iter())
            .map(|text| text::encode_as(text, encoding).expect("encodable"))
            .collect();
        assert!((strings.iter().zip(&texts)).all(|(s, t)| text::decode(s).text == *t));
        let encoded: usize = strings.iter().map(Vec::len).sum();
        let written: usize = texts.iter().map(|t| t.len()).sum();
        group.throughput(Throughput::Bytes(encoded as u64));
        group.bench_function(BenchmarkId::new("decode", name), |b| {
            b.iter(|| {
                (strings.iter())
                    .map(|s| text::decode(s).text.len())
                    .sum::<usize>()
            })
        });
        // UTF-16BE is what `encode` writes a text PDFDocEncoding can't
        // take as it is in; the other two are asked for.
        group.throughput(Throughput::Bytes(written as u64));
        group.bench_function(BenchmarkId::new("encode", name), |b| match encoding {
            Encoding::Utf16Be => {
                b.iter(|| (texts.iter()).map(|t| text::encode(t).len()).sum::<usize>())
            }
            _ => b.iter(|| {
                (texts.iter())
                    .map(|t| text::encode_as(t, encoding).map_or(0, |s| s.len()))
                    .sum::<usize>()
            }),
        });
    }
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    parse_group(&mut criterion);
    write_group(&mut criterion);
    dict_group(&mut criterion);
    object_group(&mut criterion);
    text_group(&mut criterion);
    criterion.final_summary();
}
