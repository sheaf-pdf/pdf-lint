//! A lossless, zero-copy tokenizer for PDF syntax. It splits bytes into
//! tokens and decides nothing else:
//!
//! - **Lossless:** every byte is in exactly one token, white-space and
//!   comments included, so the tokens join back into the input.
//! - **Total:** it never rejects and never repairs. A `)` or `>` that
//!   closes nothing is a [`Kind::Junk`] token, a string that doesn't end is
//!   marked unclosed, and every token moves it on.
//! - **Zero-copy, from any offset:** a [`Token`] is a kind, an offset, and
//!   its bytes as written, borrowed from the input.
//! - **No grammar:** `obj`, `R`, `stream`, `xref` and `trailer` are
//!   keywords like any other. Joining `n g R` or `n g obj` is the next
//!   layer's work.
//! - **Data it can't measure:** after `stream` or `ID`, the caller says
//!   where the data ends, by `/Length` or by scanning for `endstream` or
//!   `EI` ([`Tokenizer::data`], [`Tokenizer::data_until`]). That's where
//!   readers differ, so the choice is the caller's.
//!
//! Decoding a token into what it stands for is a separate step, in
//! [`decode`]: a name's or a string's bytes, as [`Piece`](decode::Piece)s
//! that say how each was written, or a number.
//!
//! It follows ISO 32000-2's lexical conventions (7.2). It needs only
//! `core` and never allocates, so whatever the input, tokenizing and
//! decoding it can't allocate memory or do I/O: a file it reads,
//! untrusted, can't make it use memory. Its one dependency, thiserror,
//! which derives [`NumberError`](decode::NumberError)'s traits, is built
//! without `std`. A build against `core` alone, for a target with neither
//! `std` nor `alloc`, checks it. A value's bytes come as an iterator
//! ([`Pieces::bytes`](decode::Pieces::bytes)), to collect as the caller
//! likes, or as written, borrowed, when nothing in it needs decoding
//! ([`Token::verbatim`]).
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
mod token;
mod tokenizer;

#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;

pub use chars::{is_delimiter, is_regular, is_white_space};
pub use token::{Kind, Token};
pub use tokenizer::Tokenizer;

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

    use crate::{Kind, Token, Tokenizer, fuzz};

    #[test]
    fn a_small_file_keeps_every_promise() {
        let file = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
        2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n\
        3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612.0 792] /Contents 4 0 R >>\nendobj\n\
        4 0 obj\n<< /Length 4 0 R >>\nstream\r\n\x00(\xff>)\nendstream\nendobj\n\
        xref\n0 5\n0000000000 65535 f \ntrailer\n<< /Size 5 /Root 1 0 R /ID [<a1b2><A1B2>] >>\n\
        startxref\n9\n%%EOF\n";
        assert!(fuzz::walk(file, 0) > 100);
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

    /// How many tokens `file` is, taking each stream's data as a reader might:
    /// an end of line, then up to `endstream`.
    fn read(file: &[u8], mut each: impl FnMut(&Token<'_>)) -> usize {
        let mut tokens = Tokenizer::new(file);
        let mut count = 0;
        while let Some(token) = tokens.next() {
            each(&token);
            count += 1;
            if token.kind == Kind::Keyword && token.bytes == b"stream" {
                tokens.eol();
                tokens.data_until(b"endstream");
                count += 2;
            }
        }
        count
    }

    /// Every PDF under `PDF_TOKENIZER_CORPUS`, tokenized and checked.
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
        let (mut bytes, mut tokens, mut checked) = (0, 0, 0.0);
        let mut quirks: HashMap<How, (usize, usize)> = HashMap::new();
        let mut overflows = 0;
        let mut plain = 0.0;
        for path in &files {
            let data = fs::read(path).unwrap();
            let start = Instant::now();
            tokens += read(&data, |_| {});
            plain += start.elapsed().as_secs_f64();
            let mut seen = HashSet::new();
            read(&data, |token| {
                let pieces = token.pieces().into_iter().flatten();
                for how in pieces.map(|p| p.how).filter(|h| h.is_quirk()) {
                    quirks.entry(how).or_insert((0, 0)).0 += 1;
                    seen.insert(how);
                }
                if token.integer() == Err(NumberError::Overflow) {
                    overflows += 1;
                }
            });
            for how in seen {
                quirks.entry(how).or_insert((0, 0)).1 += 1;
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
            "{} files, {mb:.0} MB, {tokens} tokens: {:.0} MB/s read, {:.0} MB/s checked",
            files.len(),
            mb / plain,
            mb / checked,
        );
        let mut quirks: Vec<_> = quirks.into_iter().collect();
        quirks.sort_by_key(|&(_, (count, _))| core::cmp::Reverse(count));
        for (how, (count, in_files)) in quirks {
            println!("{how:?}: {count} in {in_files} files");
        }
        println!("integers outside i64: {overflows}");
    }
}
