//! Lossless, zero-copy, `#![no_std]` tokenizer for PDF syntax.
//!
//! `pdf-tokenizer` splits raw PDF bytes into lexical [`Token`]s and leaves
//! object grammar, stream filters, and file recovery to higher layers:
//!
//! - **Lossless & zero-copy:** Every input byte belongs to exactly one [`Token`]
//!   (whitespace and comments included), borrowed directly from the input slice.
//! - **Total & non-panicking:** Never rejects or repairs malformed syntax.
//!   Unmatched closing delimiters become [`Kind::Junk`], and unclosed strings
//!   set [`Token::closed`] to `false`.
//! - **Stateless & window-friendly:** Start at any byte offset
//!   ([`Tokenizer::at`]). On a partial file buffer, only a token touching the
//!   end of the slice (`token.end() == bytes.len()`) can change if more bytes
//!   follow.
//! - **Explicit binary spans:** After `stream` or inline-image `ID`, advance
//!   past the delimiter newline/space ([`Tokenizer::eol`],
//!   [`Tokenizer::space_byte`]) and consume binary bytes via [`Tokenizer::data`]
//!   or [`Tokenizer::data_until`].
//! - **ISO 32000-2 or PDFium rules:** Defaults to ISO 32000-2 (§7.2, §7.3)
//!   via [`Iso`]; switch to PDFium's forgiving lexical rules via
//!   [`Tokenizer::with_rules`] ([`Pdfium`] or [`Rules`]).
//! - **Zero-allocation decoding ([`decode`]):** Decode name/string escapes as
//!   an iterator of [`Piece`](decode::Piece)s ([`Pieces::bytes`](decode::Pieces::bytes),
//!   [`Token::verbatim`]) or parse numbers, using either ISO 32000-2 rules or
//!   [`decode::pdfium`].
//!
//! Built with `#![no_std]` and zero heap allocation (`core` + `memchr` +
//! `thiserror`). In `std` applications, enabling `memchr`'s `std` feature
//! unlocks runtime AVX2 SIMD detection on x86_64.
//!
//! ```
//! use pdf_tokenizer::{Kind, Tokenizer};
//!
//! let input = b"<< /Kids [3 0 R] >>";
//! let mut tokens = Tokenizer::new(input).filter(|t| t.kind != Kind::Space);
//! let kids = tokens.nth(1).unwrap();
//! assert_eq!((kids.kind, kids.start, kids.bytes), (Kind::Name, 3, &b"/Kids"[..]));
//! ```

#![no_std]

mod chars;
pub mod decode;
mod rules;
mod token;
mod tokenizer;

#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;

pub use chars::{is_delimiter, is_regular, is_white_space};
pub use rules::{Iso, Pdfium, RuleSet, Rules};
pub use token::{Kind, Token};
pub use tokenizer::{Significant, Skimming, Tokenizer};

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

// The tokenizer and the decoders together, through the fuzz target's
// checks: on a small file, on random input, and on a folder of PDFs.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use crate::{Iso, Kind, Pdfium, RuleSet, Rules, Token, Tokenizer, fuzz};

    const SMALL_FILE: &[u8] = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
        2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n\
        3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612.0 792] /Contents 4 0 R >>\nendobj\n\
        4 0 obj\n<< /Length 4 0 R >>\nstream\r\n\x00(\xff>)\nendstream\nendobj\n\
        xref\n0 5\n0000000000 65535 f \ntrailer\n<< /Size 5 /Root 1 0 R /ID [<a1b2><A1B2>] >>\n\
        startxref\n9\n%%EOF\n";

    #[test]
    fn a_small_file_keeps_every_promise() {
        assert!(fuzz::walk(SMALL_FILE, 0) > 100);
    }

    /// The README is shown on crates.io and docs.rs, where a path beside
    /// the crate reaches nothing: its licence links are the repository's
    /// URLs, the manifest's `repository` and the file, and no link of it
    /// climbs out of the crate.
    #[test]
    fn the_readmes_licence_links_reach_the_files() {
        let readme = include_str!("../README.md");
        let repository = env!("CARGO_PKG_REPOSITORY");
        for file in ["LICENSE-APACHE", "LICENSE-MIT"] {
            let link = std::format!("[{file}]({repository}/blob/main/{file})");
            assert!(
                readme.contains(&link),
                "{file}'s link isn't the repository's URL"
            );
        }
        assert!(!readme.contains("](../"), "a link climbs out of the crate");
    }

    /// The small file cut at every byte, read from its start and from
    /// inside it: only the last token, which reaches the cut, can come out
    /// otherwise than the whole file's.
    #[test]
    fn a_small_file_cut_anywhere_keeps_the_promise_for_a_slice() {
        for offset in [0, 9, 200] {
            for cut in 0..=SMALL_FILE.len() {
                fuzz::prefix(SMALL_FILE, offset, cut);
            }
        }
    }

    /// Inputs made of the fragments where tokens begin and end, run through the
    /// fuzz target's body.
    #[test]
    fn random_inputs_keep_every_promise() {
        const FRAGMENTS: &[&[u8]] = &[
            b"(",
            b")",
            b"<",
            b">",
            b"[",
            b"]",
            b"{",
            b"}",
            b"/",
            b"%",
            b"\\",
            b" ",
            b"\t",
            b"\r",
            b"\n",
            b"\x00",
            b"\x0c",
            b"#",
            b".",
            b"+",
            b"-",
            b"0",
            b"7",
            b"a",
            b"e",
            b"F",
            b"R",
            b"\x80",
            b"\xff",
            b"obj",
            b"stream",
            b"stream\r\n",
            b"endstream",
            b"ID",
            b"EI",
            b"<<",
            b">>",
            b"%%EOF",
            // Where the decoders choose: octal escapes, a name's NUL, and a
            // number too large for an f64.
            b"\\777",
            b"\\0053",
            b"\\53",
            b"#00",
            b"/A#4a",
            &[b'9'; 320],
        ];
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut random = move |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
        for _ in 0..20_000 {
            let pieces = random(40);
            let mut input = Vec::new();
            for _ in 0..pieces + 2 {
                input.extend_from_slice(FRAGMENTS[random(FRAGMENTS.len())]);
            }
            fuzz::tokenize(&input);
        }
    }

    /// Inputs made of the fragments where PDFium's rules part from ISO
    /// 32000-2's, run through the fuzz target's body: number words, hex
    /// strings with bytes that are no digits, 0x80 and 0xFF, names' `#`,
    /// strings' CR and octal escapes, and numbers past 32 bits and past an
    /// `f32`.
    #[test]
    fn random_inputs_keep_every_promise_where_the_rules_part() {
        const FRAGMENTS: &[&[u8]] = &[
            b"<",
            b">",
            b"<<",
            b">>",
            b"(",
            b")",
            b"[",
            b"]",
            b"/",
            b"%",
            b" ",
            b"\r",
            b"\n",
            b"\r\n",
            b"\x80",
            b"\xff",
            b"\x0b",
            b"\x85",
            b"-",
            b"+",
            b".",
            b"--5",
            b"1.2.3",
            b"+-.5",
            b"0",
            b"4",
            b"9",
            b"G",
            b"e",
            b"#",
            b"#3G",
            b"#4",
            b"\\",
            b"\\5",
            b"\\53",
            b"\\\r",
            b"stream\r\n",
            b"endstream",
            b"4294967296",
            b"2147483648",
            &[b'9'; 40],
            b".0",
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut random = move |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
        for _ in 0..20_000 {
            let pieces = random(30);
            let mut input = Vec::new();
            for _ in 0..pieces + 2 {
                input.extend_from_slice(FRAGMENTS[random(FRAGMENTS.len())]);
            }
            fuzz::tokenize(&input);
        }
    }

    /// `file`'s tokens under the rules `set`, taking each stream's data as
    /// a reader might: an end of line, then up to `endstream`.
    fn read<R: RuleSet>(file: &[u8], set: R) -> impl Iterator<Item = Token<'_>> {
        let mut tokens = Tokenizer::new(file).with_rules(set);
        let mut after_stream: [Option<Token<'_>>; 2] = [None, None];
        core::iter::from_fn(move || {
            if let Some(token) = after_stream.iter_mut().find_map(Option::take) {
                return Some(token);
            }
            let token = tokens.next()?;
            if token.kind == Kind::Keyword && token.bytes == b"stream" {
                after_stream = [tokens.eol(), Some(tokens.data_until(b"endstream"))];
            }
            Some(token)
        })
    }

    /// Every PDF under `PDF_TOKENIZER_CORPUS`, tokenized and checked under
    /// each set of rules and the two side by side, with how many tokens
    /// each reads otherwise than the other.
    #[test]
    #[ignore = "needs PDF_TOKENIZER_CORPUS, a folder of PDF files"]
    fn a_corpus_keeps_every_promise() {
        use std::collections::{HashMap, HashSet};
        use std::path::PathBuf;

        use crate::decode::{How, NumberError};
        use std::time::Instant;
        use std::{env, fs, panic, println, vec};

        let dir = env::var_os("PDF_TOKENIZER_CORPUS").expect("PDF_TOKENIZER_CORPUS names a folder");
        let mut folders = vec![PathBuf::from(dir)];
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
                {
                    files.push(path);
                }
            }
        }
        assert!(!files.is_empty(), "no PDF files found");
        const RULES: [Rules; 2] = [Rules::Iso, Rules::Pdfium];
        let (mut bytes, mut tokens, mut plain, mut checked) = (0, [0; 2], [0.0; 2], 0.0);
        let mut quirks: HashMap<How, (usize, usize)> = HashMap::new();
        // Tokens one set of rules reads and the other doesn't, the same
        // place, kind and bytes, by rules and kind: how many, in how many
        // files.
        let mut otherwise: HashMap<(usize, Kind), (usize, usize)> = HashMap::new();
        let mut overflows = 0;
        for path in &files {
            let data = fs::read(path).unwrap();
            // Each set of rules fixed in the tokenizer's type.
            let start = Instant::now();
            tokens[0] += read(&data, Iso).count();
            plain[0] += start.elapsed().as_secs_f64();
            let start = Instant::now();
            tokens[1] += read(&data, Pdfium).count();
            plain[1] += start.elapsed().as_secs_f64();
            let mut seen = HashSet::new();
            for token in read(&data, Iso) {
                let pieces = token.pieces().into_iter().flatten();
                for how in pieces.map(|p| p.how).filter(|h| h.is_quirk()) {
                    quirks.entry(how).or_insert((0, 0)).0 += 1;
                    seen.insert(how);
                }
                if token.integer() == Err(NumberError::Overflow) {
                    overflows += 1;
                }
            }
            for how in seen {
                quirks.entry(how).or_insert((0, 0)).1 += 1;
            }
            let mut seen = HashSet::new();
            let mut count = |n: usize, token: &Token<'_>| {
                otherwise.entry((n, token.kind)).or_insert((0, 0)).0 += 1;
                seen.insert((n, token.kind));
            };
            let (mut iso, mut pdfium) =
                (read(&data, Iso).peekable(), read(&data, Pdfium).peekable());
            loop {
                match (iso.peek().copied(), pdfium.peek().copied()) {
                    (None, None) => break,
                    (Some(a), Some(b)) if a == b => {
                        iso.next();
                        pdfium.next();
                    }
                    (Some(a), Some(b)) if a.start == b.start => {
                        count(0, &a);
                        count(1, &b);
                        iso.next();
                        pdfium.next();
                    }
                    (Some(a), None) => {
                        count(0, &a);
                        iso.next();
                    }
                    (Some(a), Some(b)) if a.start < b.start => {
                        count(0, &a);
                        iso.next();
                    }
                    (_, Some(b)) => {
                        count(1, &b);
                        pdfium.next();
                    }
                }
            }
            for key in seen {
                otherwise.entry(key).or_insert((0, 0)).1 += 1;
            }
            let start = Instant::now();
            if panic::catch_unwind(|| fuzz::walk(&data, 0)).is_err() {
                panic!("{} broke a promise", path.display());
            }
            checked += start.elapsed().as_secs_f64();
            bytes += data.len();
        }
        let mb = bytes as f64 / 1e6;
        println!(
            "{} files, {mb:.0} MB, {:.0} MB/s checked under both rules and side by side",
            files.len(),
            mb / checked,
        );
        for (n, rules) in RULES.into_iter().enumerate() {
            println!(
                "{rules:?}: {} tokens, {:.0} MB/s read",
                tokens[n],
                mb / plain[n]
            );
        }
        let mut quirks: Vec<_> = quirks.into_iter().collect();
        quirks.sort_by_key(|&(_, (count, _))| core::cmp::Reverse(count));
        for (how, (count, in_files)) in quirks {
            println!("{how:?}: {count} in {in_files} files");
        }
        println!("integers outside i64: {overflows}");
        let mut otherwise: Vec<_> = otherwise.into_iter().collect();
        otherwise.sort_by_key(|&((n, _), (count, _))| (n, core::cmp::Reverse(count)));
        for ((n, kind), (count, in_files)) in otherwise {
            let rules = RULES[n];
            println!("read otherwise by {rules:?}'s rules: {kind:?}: {count} in {in_files} files");
        }
    }
}
