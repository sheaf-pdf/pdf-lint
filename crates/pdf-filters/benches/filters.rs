//! pdf-filters timed on inputs made here, so that a change to a decoder,
//! a predictor or the encoder can be judged before and after it, by
//! `cargo bench -p pdf-filters --features zlib-rs`, which compiles both
//! Flate backends in:
//!
//! - `decode`: a page's content, about 1 MiB of text, decoded through
//!   `ASCIIHexDecode` (`hex/text`, wrapped every 64 digits, as writers wrap
//!   it; and `hex/spaced`, a space after each byte's two digits),
//!   `ASCII85Decode` (wrapped every 75 characters), `RunLengthDecode` and
//!   `LZWDecode`; through `FlateDecode` on each backend, with incompressible
//!   bytes beside it (`noise`); cross-reference rows under PNG's Up
//!   predictor (`flate-png/*/xref-up`) and an RGB image's rows under Paeth
//!   (`flate-png/*/image-paeth`) on each backend, and the image under TIFF's
//!   predictor on the first backend; and the text deflated then written in
//!   hex, decoded by the two filters in turn (`chain`).
//! - `prefix`: 1,000 small images, each decoded by `decode_prefix_into`
//!   from where its data starts, given all the content that follows it, as
//!   a reader finds where an inline image's data ends: deflated, on each
//!   backend, and LZW-encoded.
//! - `encode`: the text deflated on each backend at levels 1, 6 and 9, and
//!   the cross-reference rows deflated under PNG's Up predictor, as a
//!   writer writes a cross-reference stream.
//!
//! Each backend's benchmarks exist only with its feature, so the file
//! builds with either backend or both; "the first backend" is miniz_oxide
//! where it's compiled in, else zlib-rs. Throughput is in bytes of the data
//! decoded or encoded, so a decode and an encode of the same data read
//! alike. Each encoded input is held to decoding to its data before it's
//! timed, and each encoder's output to decoding back to what it encoded.
//! Nothing is read from disk, and the inputs are the same on every run.

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdf_filters::{
    Backend, Encoder, Filter, Layout, Level, Prediction, Predictor, Row, decode,
    decode_prefix_into, encode,
};

/// Each Flate backend compiled in, by the name its benchmarks carry.
const BACKENDS: &[(&str, Backend)] = &[
    #[cfg(feature = "miniz_oxide")]
    ("miniz", Backend::MinizOxide),
    #[cfg(feature = "zlib-rs")]
    ("zlib", Backend::ZlibRs),
];

/// The limit each decode is given: a stream's, as a reader of files sets
/// it by default, far over any input here.
const LIMIT: usize = 256 << 20;

/// About 1 MiB, so that a run of a decoder is a millisecond or more and
/// the input is past the faster caches.
const TEXT_LEN: usize = 1 << 20;

/// Incompressible bytes: 256 KiB.
const NOISE_LEN: usize = 256 << 10;

/// How many cross-reference rows, each `/W [1 4 2]`'s seven bytes.
const XREF_ROWS: usize = 100_000;

/// The image's pixels a row.
const IMAGE_COLUMNS: usize = 1024;
/// Its rows.
const IMAGE_ROWS: usize = 256;
/// Its samples a pixel, each 8 bits: RGB.
const IMAGE_COLORS: usize = 3;

/// How many small images `prefix` decodes.
const SMALL_IMAGES: usize = 1000;
/// Each one's pixels a side, a square of 8-bit gray.
const SMALL_SIDE: usize = 32;

/// Numbers that look random, the same on every run: xorshift64.
struct Noise(u64);

impl Noise {
    /// The next number.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A page's content, as a writer writes it: lines of text shown in one of
/// four fonts, words chosen from a few dozen, with rectangles filled and
/// lines stroked between them, at places and in colors that vary as a
/// page's do; whole lines, to `TEXT_LEN` bytes or a line more.
fn text() -> Vec<u8> {
    const WORDS: &[&str] = &[
        "the", "of", "a", "and", "to", "in", "is", "that", "for", "it", "as", "with", "page",
        "stream", "filter", "data", "decoded", "reader", "writer", "file", "object", "font",
        "content", "image", "each", "byte", "row", "table", "limit", "fault", "said", "kept",
        "PDF", "Flate", "LZW", "(1)", "2026", "ISO", "32000-2,", "7.4.4.",
    ];
    let mut noise = Noise(0x9e37_79b9_7f4a_7c15);
    let mut text = Vec::with_capacity(TEXT_LEN + 256);
    while text.len() < TEXT_LEN {
        let line = match noise.below(8) {
            0 => format!(
                "{}.{} {}.{} {}.{} rg {} {} {} {} re f\n",
                noise.below(2),
                noise.below(10),
                noise.below(2),
                noise.below(10),
                noise.below(2),
                noise.below(10),
                72 + noise.below(400),
                72 + noise.below(600),
                10 + noise.below(200),
                2 + noise.below(30),
            ),
            1 => format!(
                "q 1 0 0 1 {} {} cm 0 0 m {}.{} {} l S Q\n",
                72 + noise.below(400),
                72 + noise.below(600),
                noise.below(450),
                noise.below(100),
                noise.below(20),
            ),
            _ => {
                let words: Vec<&str> = (0..4 + noise.below(10))
                    .map(|_| WORDS[noise.below(WORDS.len() as u64) as usize])
                    .collect();
                format!(
                    "BT /F{} {} Tf {} {}.{} Td ({}) Tj ET\n",
                    1 + noise.below(4),
                    8 + noise.below(7),
                    72 + noise.below(36),
                    720 - noise.below(650),
                    noise.below(100),
                    words.join(" "),
                )
            }
        };
        text.extend_from_slice(line.as_bytes());
    }
    text
}

/// Incompressible bytes, `NOISE_LEN` of them.
fn noise() -> Vec<u8> {
    let mut noise = Noise(0x2545_f491_4f6c_dd1d);
    (0..NOISE_LEN).map(|_| noise.next() as u8).collect()
}

/// A cross-reference stream's rows, `/W [1 4 2]`: entry 0 free, then one
/// object in four at an offset that grows as a file's do, and three in
/// four in object streams, by the stream's number and an index in it.
fn xref() -> Vec<u8> {
    let mut noise = Noise(0x0123_4567_89ab_cdef);
    let mut rows = Vec::with_capacity(XREF_ROWS * 7);
    rows.extend([0, 0, 0, 0, 0, 0xff, 0xff]);
    let mut offset = 15u32;
    for i in 1..XREF_ROWS as u32 {
        match i % 4 {
            0 => {
                rows.push(1);
                rows.extend(offset.to_be_bytes());
                rows.extend([0, 0]);
                offset += 40 + noise.below(2000) as u32;
            }
            _ => {
                rows.push(2);
                rows.extend((i / 400 * 400).to_be_bytes());
                rows.extend(((i % 400) as u16).to_be_bytes());
            }
        }
    }
    rows
}

/// An RGB image's rows, 8 bits a sample, `IMAGE_COLUMNS` by `IMAGE_ROWS`:
/// a smooth gradient, each sample following its place.
fn image() -> Vec<u8> {
    let mut image = Vec::with_capacity(IMAGE_COLUMNS * IMAGE_ROWS * IMAGE_COLORS);
    for y in 0..IMAGE_ROWS {
        for x in 0..IMAGE_COLUMNS {
            image.extend([(x / 4) as u8, y as u8, ((x + y) / 5) as u8]);
        }
    }
    image
}

/// The image's layout, as its `/DecodeParms` would say it.
const IMAGE: Layout = Layout {
    colors: IMAGE_COLORS as u32,
    bits_per_component: 8,
    columns: IMAGE_COLUMNS as u32,
};

/// The cross-reference rows' layout: seven bytes a row.
const XREF: Layout = Layout {
    colors: 1,
    bits_per_component: 8,
    columns: 7,
};

/// `bytes` with a line feed after every `width` of them, as a writer wraps
/// an ASCII filter's lines.
fn wrapped(bytes: &[u8], width: usize) -> Vec<u8> {
    let mut lines = Vec::with_capacity(bytes.len() + bytes.len() / width + 1);
    for line in bytes.chunks(width) {
        lines.extend_from_slice(line);
        lines.push(b'\n');
    }
    lines
}

/// `data` in hex digits, each byte's two followed by `between`, then `>`.
fn hex(data: &[u8], between: &str) -> Vec<u8> {
    let mut digits = String::with_capacity(data.len() * (2 + between.len()) + 1);
    for byte in data {
        digits.push_str(&format!("{byte:02X}{between}"));
    }
    digits.push('>');
    digits.into_bytes()
}

/// `data` in base-85, `z` for a group of four zeros, a short last group
/// in as many characters and one more, then `~>`, a line feed every 75
/// characters.
fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut chars = Vec::with_capacity(data.len() / 4 * 5 + 7);
    for group in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..group.len()].copy_from_slice(group);
        let mut value = u32::from_be_bytes(word);
        if group.len() == 4 && value == 0 {
            chars.push(b'z');
            continue;
        }
        let mut digits = [0u8; 5];
        for digit in digits.iter_mut().rev() {
            *digit = b'!' + (value % 85) as u8;
            value /= 85;
        }
        chars.extend_from_slice(&digits[..group.len() + 1]);
    }
    chars.extend_from_slice(b"~>");
    wrapped(&chars, 75)
}

/// `data` in runs: a byte repeated 2 to 128 times as its run, the bytes
/// between as copies of up to 128, then the end, 128.
fn run_length(data: &[u8]) -> Vec<u8> {
    let mut runs = Vec::with_capacity(data.len() + data.len() / 128 + 2);
    let mut at = 0;
    while at < data.len() {
        let byte = data[at];
        let run = data[at..].iter().take(128).take_while(|&&b| b == byte);
        let run = run.count();
        if run > 1 {
            runs.extend([(257 - run) as u8, byte]);
            at += run;
            continue;
        }
        // Copied up to where a run of two starts.
        let mut n = 1;
        while at + n < data.len() && n < 128 && data.get(at + n + 1) != Some(&data[at + n]) {
            n += 1;
        }
        runs.push((n - 1) as u8);
        runs.extend_from_slice(&data[at..at + n]);
        at += n;
    }
    runs.push(128);
    runs
}

/// LZW's clear code.
const CLEAR: u16 = 256;
/// LZW's end-of-data code.
const END: u16 = 257;

/// Codes written as the decoder reads them, most significant bit first,
/// each as wide as the decoder's table makes it: one entry early with
/// `early_change`, and no entry past 4,095.
struct Codes {
    bits: Vec<bool>,
    width: u32,
    // The decoder's next entry, and whether it's had a code since a
    // clear code.
    next: usize,
    first: bool,
    early: usize,
}

impl Codes {
    fn new(early_change: bool) -> Codes {
        Codes {
            bits: Vec::new(),
            width: 9,
            next: 258,
            first: true,
            early: usize::from(early_change),
        }
    }

    fn emit(&mut self, code: u16) {
        assert!(
            u32::from(code) < 1 << self.width,
            "{code} in {} bits",
            self.width
        );
        self.bits
            .extend((0..self.width).rev().map(|i| code >> i & 1 == 1));
        match code {
            CLEAR => (self.width, self.next, self.first) = (9, 258, true),
            END => {}
            _ if self.first => self.first = false,
            _ if self.next < 4096 => {
                if matches!(self.next + self.early, 511 | 1023 | 2047) {
                    self.width += 1;
                }
                self.next += 1;
            }
            _ => {}
        }
    }

    fn bytes(&self) -> Vec<u8> {
        (self.bits.chunks(8))
            .map(|byte| (0..8).fold(0, |b, i| b << 1 | u8::from(byte.get(i) == Some(&true))))
            .collect()
    }
}

/// `data` encoded, a clear code when the table nears full, as the decoder
/// reads codes: an encoder of its own, for the benchmarks.
fn lzw(data: &[u8], early_change: bool) -> Vec<u8> {
    let mut codes = Codes::new(early_change);
    let mut dictionary: HashMap<Vec<u8>, u16> = HashMap::new();
    let code = |dictionary: &HashMap<Vec<u8>, u16>, w: &[u8]| match w {
        [byte] => u16::from(*byte),
        _ => dictionary[w],
    };
    codes.emit(CLEAR);
    let mut w: Vec<u8> = Vec::new();
    for &byte in data {
        let mut wc = w.clone();
        wc.push(byte);
        if wc.len() == 1 || dictionary.contains_key(&wc) {
            w = wc;
            continue;
        }
        codes.emit(code(&dictionary, &w));
        match dictionary.len() + 258 {
            4000 => {
                codes.emit(CLEAR);
                dictionary.clear();
            }
            next => {
                dictionary.insert(wc, next as u16);
            }
        }
        w = vec![byte];
    }
    if !w.is_empty() {
        codes.emit(code(&dictionary, &w));
    }
    codes.emit(END);
    codes.bytes()
}

/// `image`'s rows as TIFF's predictor leaves them (`/Predictor 2`): each
/// 8-bit sample less the one a pixel before it in its row.
fn tiff(image: &[u8]) -> Vec<u8> {
    let mut predicted = image.to_vec();
    for row in predicted.chunks_mut(IMAGE_COLORS * IMAGE_COLUMNS) {
        for i in (IMAGE_COLORS..row.len()).rev() {
            row[i] = row[i].wrapping_sub(row[i - IMAGE_COLORS]);
        }
    }
    predicted
}

/// `data` deflated by `backend` at the default level, predicted first as
/// `predictor` says.
fn deflated(data: &[u8], predictor: Prediction, backend: Backend) -> Vec<u8> {
    let level = Level::DEFAULT;
    let flate = Encoder::Flate { level, predictor };
    let mut out = Vec::new();
    encode(data, &[flate], backend, &mut out).expect("whole rows, of a layout that's one");
    out
}

/// `Flate` with no predictor.
const FLATE: Filter = Filter::Flate {
    predictor: Predictor::None,
};

/// `Flate` with PNG's predictors, rows laid out as `layout` says.
const fn png(layout: Layout) -> Filter {
    Filter::Flate {
        predictor: Predictor::Png(layout),
    }
}

/// `LZW`, codes widening one entry early, with no predictor.
const LZW: Filter = Filter::Lzw {
    early_change: true,
    predictor: Predictor::None,
};

/// Hex digits, then zlib data in them.
const CHAIN: &[Filter] = &[Filter::AsciiHex, FLATE];

/// `encoded` held to decoding by `filters` to `plain`, then timed as
/// `name`, in bytes of `plain`, each run decoding into a `Vec` of its own,
/// as a reader of files decodes a stream.
fn decoding(
    c: &mut Criterion,
    name: &str,
    encoded: &[u8],
    filters: &[Filter],
    backend: Backend,
    plain: &[u8],
) {
    let decoded = decode(encoded, filters, LIMIT, backend);
    assert!(decoded.as_deref() == Ok(plain), "{name}: its data");
    let mut group = c.benchmark_group("decode");
    group.throughput(Throughput::Bytes(plain.len() as u64));
    group.bench_function(name, |b| {
        b.iter(|| decode(black_box(encoded), filters, LIMIT, backend))
    });
    group.finish();
}

fn decode_group(c: &mut Criterion) {
    let (first, backend) = BACKENDS[0];
    let text = text();
    let others = [
        ("hex/text", wrapped(&hex(&text, ""), 64), Filter::AsciiHex),
        ("hex/spaced", hex(&text, " "), Filter::AsciiHex),
        ("ascii85/text", ascii85(&text), Filter::Ascii85),
        ("run-length/text", run_length(&text), Filter::RunLength),
        ("lzw/text", lzw(&text, true), LZW),
    ];
    for (name, encoded, filter) in &others {
        decoding(c, name, encoded, &[*filter], backend, &text);
    }

    let noise = noise();
    let xref = xref();
    let image = image();
    let flate_text = deflated(&text, Prediction::None, backend);
    let flate_noise = deflated(&noise, Prediction::None, backend);
    let up = deflated(&xref, Prediction::Png(XREF, Row::Up), backend);
    let paeth = deflated(&image, Prediction::Png(IMAGE, Row::Paeth), backend);
    let flate = [
        ("flate", "text", &flate_text, FLATE, &text),
        ("flate", "noise", &flate_noise, FLATE, &noise),
        ("flate-png", "xref-up", &up, png(XREF), &xref),
        ("flate-png", "image-paeth", &paeth, png(IMAGE), &image),
    ];
    for &(name, backend) in BACKENDS {
        for (kind, input, encoded, filter, plain) in &flate {
            let id = format!("{kind}/{name}/{input}");
            decoding(c, &id, encoded, &[*filter], backend, plain);
        }
    }

    let tiff_image = deflated(&tiff(&image), Prediction::None, backend);
    let tiff_filter = Filter::Flate {
        predictor: Predictor::Tiff(IMAGE),
    };
    let id = format!("flate-tiff/{first}/image");
    decoding(c, &id, &tiff_image, &[tiff_filter], backend, &image);

    let chain = wrapped(&hex(&flate_text, ""), 64);
    decoding(c, "chain", &chain, CHAIN, backend, &text);
}

/// The small image numbered `i`, 8-bit gray: a gradient, each image's
/// its own.
fn small(i: usize) -> Vec<u8> {
    (0..SMALL_SIDE * SMALL_SIDE)
        .map(|p| (p % SMALL_SIDE * 4 + p / SMALL_SIDE * 2 + i) as u8)
        .collect()
}

/// A page's content showing `SMALL_IMAGES` inline images, and where each
/// image's data starts in it, with its data's length.
struct Inline {
    content: Vec<u8>,
    images: Vec<(usize, usize)>,
}

impl Inline {
    /// The images, each image's data encoded by `encoded`, `filter` its
    /// name for `/F`.
    fn new(filter: &str, encoded: impl Fn(&[u8]) -> Vec<u8>) -> Inline {
        let side = SMALL_SIDE;
        let mut content = Vec::new();
        let mut images = Vec::with_capacity(SMALL_IMAGES);
        for i in 0..SMALL_IMAGES {
            let (x, y) = (72 + i % 15 * side, 72 + i / 15 % 20 * side);
            let head = format!(
                "q {side} 0 0 {side} {x} {y} cm BI /W {side} /H {side} /CS /G /BPC 8 /F /{filter} ID "
            );
            content.extend_from_slice(head.as_bytes());
            let data = encoded(&small(i));
            images.push((content.len(), data.len()));
            content.extend_from_slice(&data);
            content.extend_from_slice(b"\nEI Q\n");
        }
        Inline { content, images }
    }

    /// Each image held to ending where its data does and decoding to its
    /// own, then all of them timed as `name`, each given all the content
    /// from its start, into one buffer, as a reader of content decodes
    /// them to find where each ends.
    fn time(&self, c: &mut Criterion, name: &str, filter: Filter, backend: Backend) {
        let mut out = Vec::new();
        for (i, &(start, len)) in self.images.iter().enumerate() {
            let data = &self.content[start..];
            let (prefix, decoded) = decode_prefix_into(data, filter, LIMIT, backend, &mut out);
            assert!(decoded.is_ok(), "{name}: image {i}, clean");
            assert!(prefix.ended, "{name}: image {i}, ended");
            assert_eq!(prefix.read, len, "{name}: image {i}, read to its end");
            assert_eq!(out, small(i), "{name}: image {i}'s data");
            out.clear();
        }
        let plain = SMALL_IMAGES * SMALL_SIDE * SMALL_SIDE;
        let mut group = c.benchmark_group("prefix");
        group.throughput(Throughput::Bytes(plain as u64));
        group.bench_function(name, |b| {
            b.iter(|| {
                let mut read = 0;
                for &(start, _) in &self.images {
                    let data = black_box(&self.content[start..]);
                    let (prefix, _) = decode_prefix_into(data, filter, LIMIT, backend, &mut out);
                    read += prefix.read;
                    out.clear();
                }
                read
            })
        });
        group.finish();
    }
}

fn prefix_group(c: &mut Criterion) {
    let (_, first) = BACKENDS[0];
    let flate = Inline::new("Fl", |data| deflated(data, Prediction::None, first));
    for &(name, backend) in BACKENDS {
        flate.time(c, &format!("flate/{name}"), FLATE, backend);
    }
    let lzw = Inline::new("LZW", |data| lzw(data, true));
    lzw.time(c, "lzw", LZW, first);
}

/// `data` encoded by `encoder` on `backend`, held to decoding back to it
/// by the filter the encoder names, then timed as `name`, in bytes of
/// `data`, into one buffer, as a writer encodes into its own.
fn encoding(c: &mut Criterion, name: &str, data: &[u8], encoder: Encoder, backend: Backend) {
    let mut out = Vec::new();
    let encoded = encode(data, &[encoder], backend, &mut out);
    assert!(encoded.is_ok(), "{name}: {encoded:?}");
    let decoded = decode(&out, &[encoder.filter()], LIMIT, backend);
    assert!(decoded.as_deref() == Ok(data), "{name}: its data");
    let mut group = c.benchmark_group("encode");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function(name, |b| {
        b.iter(|| {
            out.clear();
            encode(black_box(data), &[encoder], backend, &mut out)
        })
    });
    group.finish();
}

fn encode_group(c: &mut Criterion) {
    let text = text();
    let xref = xref();
    for &(name, backend) in BACKENDS {
        for level in [1, 6, 9] {
            let level = Level::new(level).expect("0 to 9");
            let predictor = Prediction::None;
            let id = format!("flate/{name}/{}/text", level.get());
            encoding(c, &id, &text, Encoder::Flate { level, predictor }, backend);
        }
        let level = Level::DEFAULT;
        let predictor = Prediction::Png(XREF, Row::Up);
        let id = format!("flate-png-up/{name}/xref");
        encoding(c, &id, &xref, Encoder::Flate { level, predictor }, backend);
    }
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    decode_group(&mut criterion);
    prefix_group(&mut criterion);
    encode_group(&mut criterion);
    criterion.final_summary();
}
