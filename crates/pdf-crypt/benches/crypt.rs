//! pdf-crypt timed on inputs made here, so that a change to the handler, a
//! password's forms or the primitives under them can be judged before and
//! after it, by `cargo bench -p pdf-crypt --features insecure-test-writer`, as the
//! dictionaries of revisions 2 to 5 are the writer's:
//!
//! - `open`: `Handler::open` of the writer's dictionary of each kind, by
//!   the user's password, which is tried as the owner's first, and by the
//!   owner's (`rc4-40`, `rc4-128`, `aes-128`, `aes-256-r5`, `aes-256-r6`);
//!   and of revision 6's by a wrong one, hashed both ways
//!   (`aes-256-r6/wrong`). Revision 6's hash, thousands of AES blocks and
//!   SHA-2 digests a password, is the crate's heaviest path.
//! - `create`: `Handler::create`, a new file's revision 6 dictionary, from
//!   a generator seeded afresh each time.
//! - `passwords`: a password's forms (`encodings`) and its prepared form
//!   (`prepared`): ASCII, Latin text typed composed and decomposed, and
//!   Japanese with full-width digits, which SASLprep makes ASCII.
//! - `decrypt`: 10,000 strings of 8 to 64 bytes over 2,000 objects, five
//!   an object, as an encrypted file's strings are decrypted, each under
//!   its object's key where the method makes one (`strings/*`: RC4 of 40
//!   and 128 bits, AES-128, AES-256); and 1 MiB of a page's content as 256
//!   streams of 4 KiB and as one stream (`stream/*/4k`, `stream/*/1m`).
//! - `encrypt`: the strings, and the 1 MiB stream under AES, as a writer
//!   encrypts them, AES's IV by HMAC-SHA256.
//!
//! Throughput is in bytes of plaintext for decrypting and encrypting, and
//! in calls for the rest. Each input is held to what it gives before it's
//! timed: a handler to opening as the owner's or the user's, by the
//! password given; a dictionary made to opening by its password; each
//! password to its forms; each string and stream to decrypting to its
//! plaintext, and encrypting back. Nothing is read from disk, and the
//! inputs are the same on every run.

use std::hint::black_box;

use chacha20::ChaCha20Rng;
use criterion::{BatchSize, Criterion, Throughput};
use pdf_crypt::rand_core::SeedableRng;
use pdf_crypt::writer::{self, Kind, Settings};
use pdf_crypt::{Encoded, Encoding, Handler, Password, Protection, Refused};

/// The file's first `/ID` element.
const ID: &[u8] = b"0123456789abcdef";

/// An object's number and generation, as the handler takes them.
type Object = (u32, u16);

/// Each kind the writer writes, by the name its benchmarks carry.
const KINDS: &[(&str, Kind)] = &[
    ("rc4-40", Kind::Rc4By40),
    ("rc4-128", Kind::Rc4By128),
    ("aes-128", Kind::Aes128),
    ("aes-256-r5", Kind::Aes256R5),
    ("aes-256-r6", Kind::Aes256),
];

/// The kinds strings are decrypted under: each method, AES-256 by
/// revision 6, whose strings decrypt as revision 5's do.
const STRING_KINDS: &[(&str, Kind)] = &[
    ("rc4-40", Kind::Rc4By40),
    ("rc4-128", Kind::Rc4By128),
    ("aes-128", Kind::Aes128),
    ("aes-256", Kind::Aes256),
];

/// The kinds streams are decrypted under, and strings encrypted: RC4 of
/// one key length, AES-128 and AES-256.
const STREAM_KINDS: &[(&str, Kind)] = &[
    ("rc4-128", Kind::Rc4By128),
    ("aes-128", Kind::Aes128),
    ("aes-256", Kind::Aes256),
];

/// The writer's settings for `kind`: a user's and an owner's password,
/// every permission, the metadata encrypted.
fn settings(kind: Kind) -> Settings<'static> {
    Settings {
        kind,
        user: b"user",
        owner: b"owner",
        p: -4,
        id: ID,
        encrypt_metadata: true,
    }
}

/// The handler for `kind`'s dictionary, opened by the user's password, as
/// a reader opens a file.
fn opened(kind: Kind) -> Handler {
    let dictionary = writer::dictionary(&settings(kind));
    let opened = Handler::open(&dictionary, ID, &Password("user").encodings());
    opened.expect("the writer's dictionary opens by its user's password")
}

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

/// Words a document's strings and content are made of.
const WORDS: &[&str] = &[
    "the", "of", "a", "and", "to", "in", "is", "that", "for", "it", "as", "with", "page", "title",
    "author", "report", "annual", "draft", "final", "figure", "table", "section", "chapter",
    "Acme", "Corp.", "2026", "D:2026", "Untitled", "Word", "en-US",
];

/// How many strings, and how many an object has.
const STRINGS: usize = 10_000;
const PER_OBJECT: usize = 5;

/// The strings: `STRINGS` of 8 to 64 bytes of words, `PER_OBJECT` to each
/// object from 1, each by its object's number and generation.
fn strings() -> Vec<(Object, Vec<u8>)> {
    let mut noise = Noise(0x9e37_79b9_7f4a_7c15);
    (0..STRINGS)
        .map(|i| {
            let id = ((i / PER_OBJECT + 1) as u32, 0);
            let len = 8 + noise.below(57) as usize;
            let mut string = Vec::with_capacity(len + 24);
            while string.len() < len {
                let word = WORDS[noise.below(WORDS.len() as u64) as usize];
                string.extend_from_slice(word.as_bytes());
                string.push(b' ');
            }
            string.truncate(len);
            (id, string)
        })
        .collect()
}

/// What AES's IV is derived from beside the data, as a writer names an
/// object: `n g obj`.
fn context((n, g): Object) -> Vec<u8> {
    format!("{n} {g} obj").into_bytes()
}

/// 1 MiB, so that a stream's run is a millisecond or so and past the
/// faster caches.
const TEXT_LEN: usize = 1 << 20;

/// How long each of the small streams is: 256 of them make the text.
const SMALL_STREAM: usize = 4 << 10;

/// A page's content, as a writer writes it: lines of text shown in one of
/// four fonts, with rectangles filled and lines stroked between them, at
/// places that vary as a page's do; `TEXT_LEN` bytes of it.
fn text() -> Vec<u8> {
    let mut noise = Noise(0x2545_f491_4f6c_dd1d);
    let mut text = Vec::with_capacity(TEXT_LEN + 256);
    while text.len() < TEXT_LEN {
        let line = match noise.below(8) {
            0 => format!(
                "0.{} 0.{} 0.{} rg {} {} {} {} re f\n",
                noise.below(10),
                noise.below(10),
                noise.below(10),
                72 + noise.below(400),
                72 + noise.below(600),
                10 + noise.below(200),
                2 + noise.below(30),
            ),
            1 => format!(
                "q 1 0 0 1 {} {} cm 0 0 m {} {} l S Q\n",
                72 + noise.below(400),
                72 + noise.below(600),
                noise.below(450),
                noise.below(20),
            ),
            _ => {
                let words: Vec<&str> = (0..4 + noise.below(10))
                    .map(|_| WORDS[noise.below(WORDS.len() as u64) as usize])
                    .collect();
                format!(
                    "BT /F{} {} Tf {} {} Td ({}) Tj ET\n",
                    1 + noise.below(4),
                    8 + noise.below(7),
                    72 + noise.below(36),
                    720 - noise.below(650),
                    words.join(" "),
                )
            }
        };
        text.extend_from_slice(line.as_bytes());
    }
    text.truncate(TEXT_LEN);
    text
}

fn open_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("open");
    group.throughput(Throughput::Elements(1));
    for &(name, kind) in KINDS {
        let dictionary = writer::dictionary(&settings(kind));
        for (who, owner) in [("user", false), ("owner", true)] {
            let id = format!("{name}/{who}");
            let passwords = Password(who).encodings();
            let opened = Handler::open(&dictionary, ID, &passwords);
            let handler = opened.unwrap_or_else(|refused| panic!("open/{id}: {refused}"));
            assert_eq!(handler.owner(), owner, "open/{id}: the {who}'s");
            let by = handler.opened_by();
            assert_eq!(by, Some((0, Encoding::Utf8)), "open/{id}: by its UTF-8");
            group.bench_function(&id, |b| {
                b.iter(|| Handler::open(black_box(&dictionary), ID, &passwords))
            });
        }
    }
    let dictionary = writer::dictionary(&settings(Kind::Aes256));
    let wrong = Password("wrong").encodings();
    let opened = Handler::open(&dictionary, ID, &wrong);
    assert!(
        matches!(opened, Err(Refused::Password)),
        "open/aes-256-r6/wrong: refused, as no password opens it"
    );
    group.bench_function("aes-256-r6/wrong", |b| {
        b.iter(|| Handler::open(black_box(&dictionary), ID, &wrong))
    });
    group.finish();
}

/// The seed a new file's key and salts are drawn from.
const SEED: u64 = 7;

fn create_group(c: &mut Criterion) {
    let password = Password(ASCII).prepared();
    let password = password.expect("create/aes-256-r6: a password SASLprep takes");
    let protection = Protection::new(&password);
    let rng = &mut ChaCha20Rng::seed_from_u64(SEED);
    let created = Handler::create(&protection, rng);
    let (_, dictionary) = created.expect("create/aes-256-r6: made");
    let r = (dictionary.v, dictionary.r);
    assert_eq!(r, (5, 6), "create/aes-256-r6: revision 6's");
    let opened = Handler::open(&dictionary, b"", &Password(ASCII).encodings());
    let reader = opened.expect("create/aes-256-r6: opens by its password");
    assert!(reader.owner(), "create/aes-256-r6: the owner's");
    let mut group = c.benchmark_group("create");
    group.throughput(Throughput::Elements(1));
    group.bench_function("aes-256-r6", |b| {
        b.iter_batched(
            || ChaCha20Rng::seed_from_u64(SEED),
            |mut rng| Handler::create(black_box(&protection), &mut rng),
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

/// A password in ASCII: its one form is itself, in UTF-8.
const ASCII: &str = "correct horse battery staple";
/// "Pässwörd Çafé", each accent composed with its letter, as most
/// keyboards type it.
const LATIN: &str = "P\u{e4}ssw\u{f6}rd \u{c7}af\u{e9}";
/// "Pässwörd", each diaeresis typed apart from its letter (U+0308).
const DECOMPOSED: &str = "Pa\u{308}sswo\u{308}rd";
/// "パスワード２０２６", katakana and full-width digits, as a Japanese
/// keyboard types them; SASLprep makes the digits ASCII.
const CJK: &str = "\u{30d1}\u{30b9}\u{30ef}\u{30fc}\u{30c9}\u{ff12}\u{ff10}\u{ff12}\u{ff16}";
/// `CJK` prepared: its digits ASCII.
const CJK_PREPARED: &str = "\u{30d1}\u{30b9}\u{30ef}\u{30fc}\u{30c9}2026";

/// A password's forms: each one's encoding and bytes.
type Forms = &'static [(Encoding, &'static [u8])];

/// Each password, by the name its benchmarks carry, and the forms
/// `Password::encodings` gives it, in its order: SASLprep's where it's
/// another text, UTF-8 as typed, then each single-byte encoding that has
/// every character composed and bytes not given before (WinAnsiEncoding
/// has these letters' bytes as PDFDocEncoding does).
const ENCODINGS: &[(&str, &str, Forms)] = &[
    ("ascii", ASCII, &[(Encoding::Utf8, ASCII.as_bytes())]),
    (
        "latin",
        LATIN,
        &[
            (Encoding::Utf8, LATIN.as_bytes()),
            (Encoding::PdfDoc, b"P\xe4ssw\xf6rd \xc7af\xe9"),
            (Encoding::MacRoman, b"P\x8assw\x9ard \x82af\x8e"),
        ],
    ),
    (
        "decomposed",
        DECOMPOSED,
        &[
            (Encoding::Saslprep, "P\u{e4}ssw\u{f6}rd".as_bytes()),
            (Encoding::Utf8, DECOMPOSED.as_bytes()),
            (Encoding::PdfDoc, b"P\xe4ssw\xf6rd"),
            (Encoding::MacRoman, b"P\x8assw\x9ard"),
        ],
    ),
    (
        "cjk",
        CJK,
        &[
            (Encoding::Saslprep, CJK_PREPARED.as_bytes()),
            (Encoding::Utf8, CJK.as_bytes()),
        ],
    ),
];

/// Each password a new file can take, by the name its benchmark carries,
/// and its prepared form.
const PREPARED: &[(&str, &str, &str)] = &[
    ("ascii", ASCII, ASCII),
    ("latin", LATIN, LATIN),
    ("cjk", CJK, CJK_PREPARED),
];

/// Each form's encoding and bytes.
fn forms(encoded: &[Encoded]) -> Vec<(Encoding, &[u8])> {
    (encoded.iter())
        .map(|form| (form.encoding(), form.as_bytes()))
        .collect()
}

fn passwords_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("passwords");
    group.throughput(Throughput::Elements(1));
    for &(name, text, expected) in ENCODINGS {
        let encodings = Password(text).encodings();
        assert_eq!(forms(&encodings), expected, "passwords/encodings/{name}");
        let id = format!("encodings/{name}");
        group.bench_function(&id, |b| b.iter(|| Password(black_box(text)).encodings()));
    }
    for &(name, text, expected) in PREPARED {
        let prepared = Password(text).prepared();
        let form = prepared.as_ref().map(std::slice::from_ref).map(forms);
        let held = [(Encoding::Saslprep, expected.as_bytes())];
        assert_eq!(form, Ok(held.to_vec()), "passwords/prepared/{name}");
        let id = format!("prepared/{name}");
        group.bench_function(&id, |b| b.iter(|| Password(black_box(text)).prepared()));
    }
    group.finish();
}

/// The strings encrypted by `handler`, each held to decrypting to its
/// plaintext.
fn encrypted_strings(
    name: &str,
    handler: &Handler,
    strings: &[(Object, Vec<u8>)],
) -> Vec<(Object, Vec<u8>)> {
    let encrypted: Vec<_> = (strings.iter())
        .map(|(id, plain)| {
            let encrypted = handler.encrypt_string(*id, &context(*id), plain);
            (*id, encrypted.expect("a method that's one"))
        })
        .collect();
    for (i, ((id, data), (_, plain))) in encrypted.iter().zip(strings).enumerate() {
        let decrypted = handler.decrypt_string(*id, data);
        assert!(
            decrypted.as_deref() == Ok(&plain[..]),
            "{name}: string {i}'s plaintext"
        );
    }
    encrypted
}

/// The streams `streams` encrypted by `handler`, each held to decrypting
/// to its plaintext.
fn encrypted_streams(
    name: &str,
    handler: &Handler,
    streams: &[(Object, &[u8])],
) -> Vec<(Object, Vec<u8>)> {
    let encrypted: Vec<_> = (streams.iter())
        .map(|&(id, plain)| {
            let encrypted = handler.encrypt_stream(id, &context(id), plain, None);
            (id, encrypted.expect("a method that's one"))
        })
        .collect();
    for (i, ((id, data), (_, plain))) in encrypted.iter().zip(streams).enumerate() {
        let decrypted = handler.decrypt_stream(*id, data, None);
        assert!(
            decrypted.as_deref() == Ok(*plain),
            "{name}: stream {i}'s plaintext"
        );
    }
    encrypted
}

/// Streams' data, each by its object.
type Streams<'a> = Vec<(Object, &'a [u8])>;

/// `text` as 256 streams of 4 KiB, objects 1 to 256, and as one, object
/// 300, each by the name its benchmarks carry.
fn streams(text: &[u8]) -> [(&'static str, Streams<'_>); 2] {
    let small = (text.chunks(SMALL_STREAM).enumerate())
        .map(|(i, chunk)| ((i as u32 + 1, 0), chunk))
        .collect();
    [("4k", small), ("1m", vec![((300, 0), text)])]
}

fn decrypt_group(c: &mut Criterion) {
    let strings = strings();
    let plain: usize = strings.iter().map(|(_, string)| string.len()).sum();
    let mut group = c.benchmark_group("decrypt");
    group.throughput(Throughput::Bytes(plain as u64));
    for &(name, kind) in STRING_KINDS {
        let handler = opened(kind);
        let id = format!("strings/{name}");
        let encrypted = encrypted_strings(&format!("decrypt/{id}"), &handler, &strings);
        group.bench_function(&id, |b| {
            b.iter(|| {
                let mut len = 0;
                for (id, data) in &encrypted {
                    let decrypted = handler.decrypt_string(*id, black_box(data));
                    len += decrypted.map_or(0, |data| data.len());
                }
                len
            })
        });
    }
    let text = text();
    group.throughput(Throughput::Bytes(text.len() as u64));
    for &(name, kind) in STREAM_KINDS {
        let handler = opened(kind);
        for (size, streams) in streams(&text) {
            let id = format!("stream/{name}/{size}");
            let encrypted = encrypted_streams(&format!("decrypt/{id}"), &handler, &streams);
            group.bench_function(&id, |b| {
                b.iter(|| {
                    let mut len = 0;
                    for (id, data) in &encrypted {
                        let decrypted = handler.decrypt_stream(*id, black_box(data), None);
                        len += decrypted.map_or(0, |data| data.len());
                    }
                    len
                })
            });
        }
    }
    group.finish();
}

fn encrypt_group(c: &mut Criterion) {
    let strings = strings();
    let contexts: Vec<Vec<u8>> = strings.iter().map(|(id, _)| context(*id)).collect();
    let plain: usize = strings.iter().map(|(_, string)| string.len()).sum();
    let mut group = c.benchmark_group("encrypt");
    group.throughput(Throughput::Bytes(plain as u64));
    for &(name, kind) in STREAM_KINDS {
        let handler = opened(kind);
        let id = format!("strings/{name}");
        encrypted_strings(&format!("encrypt/{id}"), &handler, &strings);
        group.bench_function(&id, |b| {
            b.iter(|| {
                let mut len = 0;
                for ((id, string), context) in strings.iter().zip(&contexts) {
                    let encrypted = handler.encrypt_string(*id, context, black_box(string));
                    len += encrypted.map_or(0, |data| data.len());
                }
                len
            })
        });
    }
    let text = text();
    let stream = [((300, 0), &text[..])];
    let context = context(stream[0].0);
    group.throughput(Throughput::Bytes(text.len() as u64));
    for &(name, kind) in &STREAM_KINDS[1..] {
        let handler = opened(kind);
        let id = format!("stream/{name}/1m");
        encrypted_streams(&format!("encrypt/{id}"), &handler, &stream);
        group.bench_function(&id, |b| {
            b.iter(|| handler.encrypt_stream((300, 0), &context, black_box(&text), None))
        });
    }
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    open_group(&mut criterion);
    create_group(&mut criterion);
    passwords_group(&mut criterion);
    decrypt_group(&mut criterion);
    encrypt_group(&mut criterion);
    criterion.final_summary();
}
