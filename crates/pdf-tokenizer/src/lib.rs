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
//!   layer's work. [`Tokenizer::skim`] passes an array's or a dictionary's
//!   insides without a token each, giving only the tokens that open or
//!   close one, but judges nothing: which closes which is the caller's.
//! - **Data it can't measure:** after `stream` or `ID`, the caller says
//!   where the data ends, by `/Length` or by scanning for `endstream` or
//!   `EI` ([`Tokenizer::data`], [`Tokenizer::data_until`]). That's where
//!   readers differ, so the choice is the caller's.
//! - **A slice of a file:** the input may be as much of a file as a reader
//!   has read. The tokenizer never rejects, so it closes whatever token
//!   the slice ends in, and only a token that reaches the slice's end
//!   (`token.end() == bytes.len()`) can come out otherwise with more bytes
//!   after it: `12` of `123`, `>` of `>>`, a string cut short, white-space
//!   that goes on, data that runs on to its marker. Every other token is
//!   settled by the bytes inside the slice: it's the whole file's token at
//!   the same place, as the tokenizer carries nothing from one token to
//!   the next. A reader that meets a token at the end reads again with
//!   more.
//! - **ISO 32000-2's rules, or PDFium's:** it follows ISO 32000-2's
//!   lexical conventions (7.2, 7.3) unless the caller gives it PDFium's
//!   ([`Tokenizer::with_rules`], [`Rules`]), as a reader that means to see
//!   what PDFium sees would. They part on a word of digits, signs and
//!   points (`--5` and `1.2.3` are numbers to PDFium, keywords to ISO
//!   32000-2), on where a hex string ends (at its `>`, over any byte, to
//!   PDFium), and on two bytes, 0x80 and 0xFF, that PDFium reads as
//!   white-space. Every promise here holds under either. The rules are
//!   fixed in the tokenizer's type ([`Iso`], the default, or [`Pdfium`]),
//!   or chosen as it runs (a [`Rules`] value), at a branch a token
//!   ([`RuleSet`]).
//!
//! Decoding a token into what it stands for is a separate step, in
//! [`decode`]: a name's or a string's bytes, as [`Piece`](decode::Piece)s
//! that say how each was written, or a number, as ISO 32000-2 reads them;
//! and in [`decode::pdfium`], their values as PDFium reads them.
//!
//! It needs only `core` and never allocates, so whatever the input,
//! tokenizing and decoding it can't allocate memory or do I/O: a file it
//! reads, untrusted, can't make it use memory. Its two dependencies are
//! built without `std`: thiserror, which derives
//! [`NumberError`](decode::NumberError)'s traits, and memchr, which finds
//! where a comment or a string ends, and a marker after data, on SIMD where
//! the target has it. A build against `core` alone, for a target with
//! neither `std` nor `alloc`, checks it. A value's bytes come as an iterator
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
mod rules;
mod token;
mod tokenizer;

#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;

pub use chars::{is_delimiter, is_regular, is_white_space};
pub use rules::{Iso, Pdfium, RuleSet, Rules};
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
