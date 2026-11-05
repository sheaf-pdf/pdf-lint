//! The tokenizer and the decoders timed on inputs made here, so that a
//! change to either can be judged before and after it, by
//! `cargo bench -p pdf-tokenizer`:
//!
//! - `tokens`: every token of an object repeated to about 4 MB, and of a
//!   content stream repeated to as much;
//! - `read`: the objects as a reader takes them, a token at a time, and
//!   after each `stream` the end of line and the data to `endstream`;
//! - `significant` and `skim`: nested arrays and dictionaries passed by
//!   `next_significant` and by `skim`;
//! - `data_until`: a stream of 32 MB scanned to its `endstream`, and one
//!   with no marker, scanned to the end;
//! - `strings`: a long literal string, one with escapes, and a long
//!   hexadecimal string, each tokenized, its value's bytes counted, and
//!   its value borrowed as written where nothing needs decoding;
//! - `decode`: names with escapes, and integers and reals, read by ISO
//!   32000-2's rules and by PDFium's.
//!
//! Each run of tokens is under four sides: ISO 32000-2's rules fixed in
//! the tokenizer's type (`iso`), chosen as it runs (`iso-chosen`), and
//! PDFium's each way (`pdfium`, `pdfium-chosen`). Throughput is in bytes
//! of input. Nothing is read from disk, and the inputs are the same on
//! every run.

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, Throughput};
use pdf_tokenizer::decode::{self, pdfium};
use pdf_tokenizer::{Iso, Kind, Pdfium, RuleSet, Rules, Tokenizer};

/// A page object with its content stream, and the objects a page refers
/// to, as a writer lays them out.
const OBJECTS: &[u8] = b"1 0 obj\n\
    << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [0 0 612.0 792.0]\n\
    /Resources << /Font << /F1 5 0 R /F2 6 0 R >> /ProcSet [/PDF /Text /ImageC] /XObject << /Im0 7 0 R >> >>\n\
    /Contents 4 0 R /Annots [8 0 R 9 0 R] /Rotate 0 /Group << /S /Transparency /CS /DeviceRGB >> >>\n\
    endobj\n\
    4 0 obj\n<< /Length 86 /Filter /FlateDecode >>\nstream\r\n\
    q 1 0 0 1 72 720 cm BT /F1 12 Tf 14.4 TL (Hello, world) Tj T* [(W) 120 (orld)] TJ ET Q\n\
    endstream\nendobj\n\
    5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n\
    8 0 obj\n<< /Type /Annot /Subtype /Link /Rect [72 700.5 300 712] /Border [0 0 0] /A << /S /URI /URI (https://example.org/a#b) >> >>\nendobj\n\
    % a comment between objects\n";

/// A content stream: text, a path, graphics state and an image.
const CONTENT: &[u8] =
    b"q 1 0 0 1 72 720 cm BT /F1 12 Tf 14.4 TL (Hello, world) Tj T* [(W) 120 (orld)] TJ ET Q\n\
    0.5 0 0 RG 0 0 1 rg 1 w 72 72 m 540 72 l 540 100 l h B\n\
    /GS0 gs q 200 0 0 100 72 400 cm /Im0 Do Q\n\
    BT /F2 9.5 Tf 1 0 0 1 72 380 Tm 0.2 Tc (caf\\351 \\(1\\) \\\\) Tj <48656C6C6F> Tj ET\n\
    /Span << /MCID 3 /Lang (en-US) >> BDC EMC\n";

/// Arrays and dictionaries nested in each other, with strings and names
/// among them.
const NESTED: &[u8] = b"[1 2 [3 4 [5 6 [7 8]] 9] << /A [10 11 << /B [12] /C (s) >>] /D << /E << /F <4142> >> >> /G /H >> (t) <43> /N 1.5 -2 true null]\n";

/// Names, half of them with an escape.
const NAMES: &[u8] = b"/Type /Name#20with#20spaces /A#2FB /Caf#E9 /Plain /Hash#23 /Font /F1 /Length /Lo#6Eg#20escaped#20name /MediaBox /B#41SE ";

/// A mix of integers and reals, as objects and content hold them.
const NUMBERS: &[u8] = b"0 1 -1 42 612 792 65535 123456789 -2147483648 4294967296 3.14 -0.5 .5 1.0 0.0001 14.4 720.25 -72.5 100000000000 0 ";

/// `piece` repeated until the input holds `len` bytes or more.
fn repeated(piece: &[u8], len: usize) -> Vec<u8> {
    let mut input = Vec::with_capacity(len + piece.len());
    while input.len() < len {
        input.extend_from_slice(piece);
    }
    input
}

/// Every token of `input`, the bytes they hold summed.
fn tokens<R: RuleSet>(input: &[u8], rules: R) -> usize {
    Tokenizer::new(input)
        .with_rules(rules)
        .map(|t| t.bytes.len())
        .sum()
}

/// `input` as a reader takes it: a token at a time, and after each
/// `stream` the end of line and then the data, to `endstream`.
fn read<R: RuleSet>(input: &[u8], rules: R) -> usize {
    let mut tokens = Tokenizer::new(input).with_rules(rules);
    let mut n = 0;
    while let Some(token) = tokens.next() {
        n += 1;
        if token.kind == Kind::Keyword && token.bytes == b"stream" {
            n += usize::from(tokens.eol().is_some());
            n += usize::from(!tokens.data_until(b"endstream").bytes.is_empty());
        }
    }
    n
}

/// The tokens of `input` that are neither white-space nor a comment.
fn significant<R: RuleSet>(input: &[u8], rules: R) -> usize {
    let mut tokens = Tokenizer::new(input).with_rules(rules);
    let mut n = 0;
    while tokens.next_significant().is_some() {
        n += 1;
    }
    n
}

/// The tokens of `input` that open or close an array or a dictionary,
/// what they hold skimmed.
fn skim<R: RuleSet>(input: &[u8], rules: R) -> usize {
    let mut tokens = Tokenizer::new(input).with_rules(rules);
    let mut n = 0;
    while tokens.skim().is_some() {
        n += 1;
    }
    n
}

/// `f(input, rules)` under each of the four sides, the input's bytes its
/// throughput.
macro_rules! under_each_rules {
    ($group:expr, $name:expr, $input:expr, $f:ident) => {{
        let input: &[u8] = $input;
        $group.throughput(Throughput::Bytes(input.len() as u64));
        $group.bench_with_input(BenchmarkId::new($name, "iso"), &input, |b, input| {
            b.iter(|| $f(input, Iso))
        });
        $group.bench_with_input(BenchmarkId::new($name, "iso-chosen"), &input, |b, input| {
            b.iter(|| $f(input, Rules::Iso))
        });
        $group.bench_with_input(BenchmarkId::new($name, "pdfium"), &input, |b, input| {
            b.iter(|| $f(input, Pdfium))
        });
        $group.bench_with_input(
            BenchmarkId::new($name, "pdfium-chosen"),
            &input,
            |b, input| b.iter(|| $f(input, Rules::Pdfium)),
        );
    }};
}

/// About 4 MB, so that a run is a few milliseconds and the input is well
/// past any cache.
const INPUT_LEN: usize = 4 << 20;

fn tokens_group(c: &mut Criterion) {
    let objects = repeated(OBJECTS, INPUT_LEN);
    let content = repeated(CONTENT, INPUT_LEN);
    let mut group = c.benchmark_group("tokens");
    under_each_rules!(group, "objects", &objects, tokens);
    under_each_rules!(group, "content", &content, tokens);
    group.finish();
}

fn read_group(c: &mut Criterion) {
    let objects = repeated(OBJECTS, INPUT_LEN);
    let mut group = c.benchmark_group("read");
    under_each_rules!(group, "objects", &objects, read);
    group.finish();
}

fn significant_and_skim_groups(c: &mut Criterion) {
    let nested = repeated(NESTED, INPUT_LEN);
    let mut group = c.benchmark_group("significant");
    under_each_rules!(group, "nested", &nested, significant);
    group.finish();
    let mut group = c.benchmark_group("skim");
    under_each_rules!(group, "nested", &nested, skim);
    group.finish();
}

fn data_until_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("data_until");
    let without = repeated(CONTENT, 32 << 20);
    let mut with = without.clone();
    with.extend_from_slice(b"\nendstream");
    for (side, input) in [("marker-at-end", &with), ("no-marker", &without)] {
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("stream", side),
            &input.as_slice(),
            |b, input| b.iter(|| Tokenizer::new(input).data_until(b"endstream").bytes.len()),
        );
    }
    group.finish();
}

/// A string's token, its value's bytes counted, and, where nothing in it
/// needs decoding, its value as written: a search of the whole string,
/// where a string with an escape ends the search at its first.
fn string_benches(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    input: &[u8],
    verbatim: bool,
) {
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_with_input(BenchmarkId::new(name, "tokenized"), &input, |b, input| {
        b.iter(|| Tokenizer::new(input).next().map(|t| t.end()))
    });
    let token = Tokenizer::new(input).next().unwrap();
    group.bench_with_input(BenchmarkId::new(name, "bytes"), &token, |b, token| {
        b.iter(|| token.pieces().unwrap().bytes().count())
    });
    if verbatim {
        group.bench_with_input(BenchmarkId::new(name, "verbatim"), &token, |b, token| {
            b.iter(|| token.verbatim().map(<[u8]>::len))
        });
    }
}

fn strings_group(c: &mut Criterion) {
    const LEN: usize = 1 << 20;
    let mut literal = b"(".to_vec();
    literal.extend_from_slice(&repeated(
        b"Lorem ipsum dolor sit amet, (consectetur) adipiscing elit. ",
        LEN,
    ));
    literal.push(b')');
    let mut escaped = b"(".to_vec();
    escaped.extend_from_slice(&repeated(
        b"Lorem ipsum \\(dolor\\) sit amet, \\101\\n elit\\\r\n. ",
        LEN,
    ));
    escaped.push(b')');
    let mut hex = b"<".to_vec();
    hex.extend_from_slice(&repeated(b"48656C6C6F20 776F726C64 0A\n", LEN));
    hex.push(b'>');
    let mut group = c.benchmark_group("strings");
    string_benches(&mut group, "literal", &literal, true);
    string_benches(&mut group, "escaped", &escaped, false);
    string_benches(&mut group, "hex", &hex, false);
    // A hexadecimal string's end is found otherwise under PDFium's rules:
    // at its `>`, over any byte.
    group.bench_with_input(
        BenchmarkId::new("hex", "tokenized-pdfium"),
        &hex.as_slice(),
        |b, input| {
            b.iter(|| {
                Tokenizer::new(input)
                    .with_rules(Pdfium)
                    .next()
                    .map(|t| t.end())
            })
        },
    );
    group.finish();
}

fn decode_group(c: &mut Criterion) {
    // The lists repeated, so that a run is microseconds, not nanoseconds.
    let names = repeated(NAMES, 8 << 10);
    let words = repeated(NUMBERS, 8 << 10);
    let names: Vec<&[u8]> = Tokenizer::new(&names)
        .filter(|t| t.kind == Kind::Name)
        .map(|t| t.bytes)
        .collect();
    let words: Vec<&[u8]> = Tokenizer::new(&words)
        .filter(|t| t.kind != Kind::Space)
        .map(|t| t.bytes)
        .collect();
    let bytes = |words: &[&[u8]]| words.iter().map(|w| w.len() as u64).sum::<u64>();
    let mut group = c.benchmark_group("decode");
    group.throughput(Throughput::Bytes(bytes(&names)));
    group.bench_with_input(BenchmarkId::new("names", "iso"), &names, |b, names| {
        b.iter(|| {
            names
                .iter()
                .map(|n| decode::name(n).bytes().count())
                .sum::<usize>()
        })
    });
    group.bench_with_input(BenchmarkId::new("names", "pdfium"), &names, |b, names| {
        b.iter(|| names.iter().map(|n| pdfium::name(n).count()).sum::<usize>())
    });
    group.throughput(Throughput::Bytes(bytes(&words)));
    group.bench_with_input(BenchmarkId::new("integer", "iso"), &words, |b, words| {
        b.iter(|| {
            words
                .iter()
                .filter_map(|w| decode::integer(w).ok())
                .fold(0i64, i64::wrapping_add)
        })
    });
    group.bench_with_input(BenchmarkId::new("number", "iso"), &words, |b, words| {
        b.iter(|| {
            words
                .iter()
                .filter_map(|w| decode::number(w).ok())
                .sum::<f64>()
        })
    });
    group.bench_with_input(BenchmarkId::new("number", "pdfium"), &words, |b, words| {
        b.iter(|| words.iter().map(|w| pdfium::number(w).float()).sum::<f32>())
    });
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    tokens_group(&mut criterion);
    read_group(&mut criterion);
    significant_and_skim_groups(&mut criterion);
    data_until_group(&mut criterion);
    strings_group(&mut criterion);
    decode_group(&mut criterion);
    criterion.final_summary();
}
