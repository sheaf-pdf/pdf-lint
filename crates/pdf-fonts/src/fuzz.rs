//! What the fuzz target checks, which unit tests check too: the promises a Type 1
//! program's scan and a glyph's or a font's name make, on any input. It's compiled
//! only for tests and under cargo-fuzz's `--cfg fuzzing`.

use crate::fixture::{type1_integers_program, type1_program};
use crate::predefined::{Collection, scheme};
use crate::type1::scan;
use crate::{Encoding, Limits, Std14, cp1252, glyph_text, name_code_point};

/// How many of an input's words are read as names, as a run of the target
/// shouldn't take the square of its input.
const NAMES: usize = 64;

/// How many of an input's bytes are read as codes: there are only 256 of them, so
/// more finds nothing more.
const CODES: usize = 4;

/// `data` as a Type 1 program: what's held no more than what's given, and under
/// smaller caps the same or nothing at all, as a cap refuses a program and never
/// reads it otherwise.
pub fn type1(data: &[u8]) {
    let limits = Limits::DEFAULT;
    let read = scan(data, &limits);
    if let Some(charstrings) = &read {
        assert!(charstrings.contains_key(&b".notdef"[..]));
        let held: usize = charstrings.values().flatten().map(Vec::len).sum();
        // The one `.notdef` a program may have made up for it is 5 bytes.
        assert!(held <= data.len() + 5, "{held} of {}", data.len());
    }
    // A smaller cap on depth alone would do all the work over again.
    let smaller = [
        Limits {
            proc_depth: 2,
            type1_work: 4,
            ..limits
        },
        Limits {
            ints_ahead: 2,
            ..limits
        },
        Limits {
            type1_work: 1,
            ..limits
        },
        Limits {
            proc_depth: 0,
            ints_ahead: 0,
            type1_work: 0,
        },
    ];
    for limits in smaller {
        if let Some(under) = scan(data, &limits) {
            assert_eq!(Some(under), read);
        }
    }
    let larger = Limits {
        proc_depth: 2 * limits.proc_depth,
        ints_ahead: 2 * limits.ints_ahead,
        type1_work: 2 * limits.type1_work,
    };
    if read.is_some() {
        assert_eq!(scan(data, &larger), read);
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
}

/// A code in every encoding: its text is its glyph's, or Windows-1252's.
fn codes(code: u8) {
    use Encoding::{MacRoman, Standard, Symbol, WinAnsi, ZapfDingbats};
    for encoding in [Standard, WinAnsi, MacRoman, Symbol, ZapfDingbats] {
        let (glyph, text) = (encoding.glyph(code), encoding.text(code));
        if encoding == WinAnsi {
            assert_eq!(text, cp1252(code).map(String::from));
        } else {
            assert_eq!(glyph.is_none(), text.is_none(), "{encoding:?} {code:#x}");
        }
    }
}

/// What the fuzzer begins from, and the tests run on: Type 1 programs, as written
/// and in a PFB's segments, integers in a row past and within the caps, and names
/// of each kind.
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
        type1_integers_program(257),
        b"%!PS-AdobeFont-1.0: F\n/N { { { } } } def\n(a(b)\\)) <41> << >> [ ] % c\n\
          8#17 2#1 currentfile eexec\n"
            .to_vec(),
        b"A a20 f_i a.alt uni0416 uni00410042 u1F600 uniD800 .notdef space uni\xe2\x82\xac\xe2\x82\xac\xe2\x82\xac\xe2\x82\xac\n\
          Helvetica ABCDEF+ArialMT arialmt Arial,Bold TimesNewRomanPS-BoldItalicMT SymbolMT\n\
          StandardEncoding WinAnsiEncoding MacRomanEncoding MacExpertEncoding\n\
          90ms-RKSJ-H UniJIS-UTF16-V H V GBK2K-H CNS-EUC-H Identity-H Adobe-Japan1-UCS2\n"
            .to_vec(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mutation of `seed`, from `random`, as a fuzzer might make it.
    fn mutated(seed: &[u8], random: &mut impl FnMut() -> usize) -> Vec<u8> {
        const PIECES: [&[u8]; 16] = [
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
            for _ in 0..50 {
                let data = mutated(&seed, &mut random);
                type1(&data);
                names(&data);
            }
        }
        assert!(found >= 4, "the seeds hold programs that read: {found}");
    }

    #[test]
    fn random_bytes_keep_every_promise() {
        let mut random = generator(0xB17E5);
        for _ in 0..2_000 {
            let data: Vec<u8> = (0..random() % 64).map(|_| random() as u8).collect();
            type1(&data);
            names(&data);
        }
    }

    /// `PDF_FONTS_FUZZ_SEEDS` names a folder: writes the seeds there, for the
    /// fuzzer to begin from (`just fuzz fonts`).
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
    }
}
