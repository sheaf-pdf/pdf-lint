//! What the fuzz target checks, which unit tests check too: the promises
//! the tokens make, on any input. It's compiled only for tests and under
//! cargo-fuzz's `--cfg fuzzing`.

use std::ops::Range;

use pdf_objects::Value;

use crate::{Concat, Error, ErrorKind, Join, Limits, ResCx, Tok, name, object, tokens};

/// What one read gave: each token and where it is, then the error, if
/// one stopped it.
type Read<'a> = Vec<Result<(Range<usize>, Tok<'a>), Error>>;

/// The fuzz target's body: `data` as a page's content, read under the
/// default caps, every token taken: whole, and as two streams split at
/// its middle, joined by [`Join`] and by [`Concat::new`] alike. The
/// resources name a color space of each size an inline image may be drawn
/// in. Each read's tokens tile its bytes ([`tiled`]). Read again under
/// small caps on operands and an inline image's data, the tokens are the
/// first read's up to an error, which is the cap's, where the first read's
/// token past it starts, or the first read's own. Panics on a broken
/// promise.
pub fn content(data: &[u8]) {
    let res = ResCx::new([
        (b"CS1".to_vec(), 1),
        (b"CS3".to_vec(), 3),
        (b"CS4".to_vec(), 4),
    ]);
    let limits = &Limits::DEFAULT;
    let (first, second) = data.split_at(data.len() / 2);
    let split = Concat::new(&[first, second]);
    let mut join = Join::new(limits);
    if join.push(first).is_ok() && join.push(second).is_ok() {
        let joined = join.finish();
        assert_eq!(joined.bytes(), split.bytes(), "joined alike");
        assert_eq!(joined.stream_count(), 2);
        assert_eq!(
            (joined.range(0), joined.range(1)),
            (split.range(0), split.range(1))
        );
        assert_eq!((joined.stream(0), joined.stream(1)), (first, second));
    }
    let whole = Concat::new(&[data]);
    let read = tiled(&whole, &res, limits);
    tiled(&split, &res, limits);
    let small = Limits {
        operands: 3,
        inline_image: 8,
        ..Limits::DEFAULT
    };
    let capped: Read = tokens(data, &res, &small).collect();
    for (i, token) in capped.iter().enumerate() {
        match (token, read.get(i)) {
            (Err(e), Some(Ok((r, tok)))) => {
                assert_eq!(e.at, r.start, "the cap's error where its token starts");
                match e.kind {
                    ErrorKind::Operands => assert!(matches!(tok, Tok::Operand(_))),
                    ErrorKind::InlineImageSize => {
                        assert!(matches!(tok, Tok::InlineImage(d) if d.len() > small.inline_image));
                    }
                    kind => panic!("{kind} where the caps' alone could be"),
                }
            }
            (token, first) => assert_eq!(Some(token), first, "the same under small caps"),
        }
    }
    if capped.last().is_none_or(Result::is_ok) {
        assert_eq!(capped.len(), read.len(), "every token under small caps");
    }
}

/// `content`'s tokens, read under `limits` until the end or an error,
/// which tile its bytes: in order, each inside them, only white-space and
/// comments between them, and after the last one too; nothing after an
/// error. An operator is a word; an operand reads as an object, as the
/// array, dictionary or string it starts as one, and as the name it is,
/// decoded, when it's one; an inline image is `BI` to `EI`, its data
/// inside it and under the cap. Each token's range is mapped to a span of
/// the streams and back.
fn tiled<'a>(content: &'a Concat, res: &ResCx, limits: &Limits) -> Read<'a> {
    let s = content.bytes();
    let mut at = 0;
    let mut read = Vec::new();
    let mut lexed = content.tokens(res, limits);
    for token in lexed.by_ref() {
        let Ok((r, tok)) = &token else {
            assert!(lexed.next().is_none(), "nothing after an error");
            read.push(token);
            return read;
        };
        assert!(
            at <= r.start && r.start < r.end && r.end <= s.len(),
            "{r:?} after {at}"
        );
        assert!(blank(&s[at..r.start]), "only white-space between");
        assert_eq!(content.offsets(content.span(r.clone())), Some(r.clone()));
        match tok {
            Tok::Op(op) => {
                assert_eq!(*op, &s[r.clone()]);
                assert!(!matches!(*op, b"BI" | b"true" | b"false" | b"null"));
                assert!(
                    op.iter()
                        .all(|&c| !blank(&[c]) && !b"()<>[]{}/%".contains(&c))
                );
            }
            Tok::Operand(operand) => {
                assert_eq!(*operand, &s[r.clone()]);
                operand_reads(operand, limits);
            }
            Tok::InlineImage(data) => {
                assert!(s[r.clone()].starts_with(b"BI") && s[r.clone()].ends_with(b"EI"));
                assert!(
                    r.start + 2 <= data.start && data.end <= r.end - 2,
                    "{data:?} in {r:?}"
                );
                assert!(data.len() <= limits.inline_image);
            }
        }
        at = r.end;
        read.push(token);
    }
    assert!(blank(&s[at..]), "only white-space after");
    read
}

/// An operand read as an object: as the array, dictionary or string it
/// starts as one, and as the name it is, decoded.
fn operand_reads(operand: &[u8], limits: &Limits) {
    let read = object(operand, limits.depth).map(|o| o.into_value());
    match operand.first() {
        Some(b'[') => assert!(matches!(read, Some(Value::Array(_))), "an array reads"),
        Some(b'<') if operand.starts_with(b"<<") => {
            assert!(matches!(read, Some(Value::Dict(_))), "a dictionary reads");
        }
        Some(b'(' | b'<') => assert!(matches!(read, Some(Value::String(..))), "a string reads"),
        Some(b'/') => {
            let decoded = name(operand).map(|n| n.into_owned());
            assert_eq!(read, decoded.map(Value::Name), "a name reads as decoded");
        }
        _ => {}
    }
}

/// Whether `gap` is white-space and comments only.
pub(crate) fn blank(gap: &[u8]) -> bool {
    let mut comment = false;
    gap.iter().all(|&c| {
        comment = match c {
            b'%' => true,
            b'\r' | b'\n' => false,
            _ => comment,
        };
        comment || matches!(c, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
    })
}

/// Page content with an inline image found by each of PDFium's rules,
/// ` EI ` in its data where the data is counted, and strings, arrays,
/// dictionaries and marked content: the fuzz target's seeds.
pub fn fixtures() -> Vec<Vec<u8>> {
    let image = |dict: &str, data: &[u8]| {
        [format!("q BI {dict} ID ").as_bytes(), data, b"\nEI Q\n"].concat()
    };
    vec![
        image("/W 4 /H 1 /BPC 8 /CS /G", b" EI "),
        image("/W 2 /H 1 /BPC 8 /CS /CS3", b"a EI b"),
        image("/IM true /W 10 /H 2", b" EI "),
        image("/W 2 /H 2 /BPC 8 /CS [/I /RGB 1 <000000FFFFFF>]", b"EI E"),
        image("/F /AHx /W 2 /H 2 /BPC 8 /CS /G", b"0E EI 0>"),
        image("/F [/A85 /Fl] /W 9 /H 9", b"9jqo^ EI ~>"),
        image("/F /RL /W 4 /H 1 /BPC 8 /CS /G", b"\x03 EI \x80"),
        image("/F /DCT /W 9 /H 9 /BPC 8 /CS /RGB", b"\xff\xd8 EI \xff\xd9"),
        image("/F /Fl /L 6", b"x EI y"),
        image("/F /CCF /DP <</K -1>>", b"xEI EIy"),
        b"%c\nq /Artifact <</Type /Pagination /Subtype /Header /Attached [/Top]>> BDC\n\
          q BT /F1 12 Tf 72 720 Td [(a\\)(b) c) -250 <4142>] TJ (\\\\) Tj ET Q EMC\n\
          [1 [2 [3]] <</A <</B (c)>>>>] true false null Q { } ) ]"
            .to_vec(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Random;

    /// What a mutation of content may write: its tokens, the bytes that
    /// end an inline image's data, and what moves text and draws forms.
    const CONTENT_TOKENS: &[&[u8]] = &[
        b" ",
        b"(",
        b")",
        b"\\",
        b"<",
        b">",
        b"<<",
        b">>",
        b"[",
        b"]",
        b"%",
        b"\n",
        b" BI ",
        b" ID ",
        b" EI ",
        b"/W 4 /H 1 /BPC 8 /CS /CS3 ",
        b"/F /DCT ",
        b"/L 9 ",
        b"~>",
        b"\xff\xd9",
        b"\x80",
        b" BDC ",
        b" EMC ",
        b" q ",
        b" Q ",
        b" BT ",
        b" ET ",
        b" /F1 9 Tf ",
        b" 0 -14 Td ",
        b" (x) Tj ",
        b" /X0 Do ",
        b" /Artifact /P0 BDC ",
        b" /Artifact <</Type /Pagination /Subtype /Footer>> BDC ",
        b"4294967296",
        b" 12 TL ",
        b" 5 -14 TD ",
        b" T* ",
        b" (a) ' ",
        b" 2 1 (a b) \" ",
        b" 0 1 -1 0 300 500 Tm ",
        b" /T3 10 Tf ",
        b" /X1 Do ",
        b" /Fm1 Do ",
        b" /OC /OC0 BDC ",
    ];

    /// The seeds read to their end, each of the first ten one inline
    /// image between `q` and `Q`.
    #[test]
    fn the_seeds_read_to_their_end() {
        let res = ResCx::new([(b"CS3".to_vec(), 3)]);
        for (i, seed) in fixtures().iter().enumerate() {
            let read: Vec<_> = tokens(seed, &res, &Limits::DEFAULT)
                .collect::<Result<_, _>>()
                .unwrap_or_else(|e| panic!("seed {i}: {e}"));
            if i < 10 {
                assert_eq!(read.len(), 3, "seed {i}");
                assert!(matches!(read[1].1, Tok::InlineImage(_)), "seed {i}");
            }
            content(seed);
        }
    }

    /// The fuzz target's checks on the seeds, each mutated many times as a
    /// fuzzer might: bytes changed, cut, repeated and moved, and content's
    /// tokens put in.
    #[test]
    fn mutated_seeds_keep_every_promise() {
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        for seed in fixtures().into_iter().cycle().take(5000) {
            let mut data = seed;
            for _ in 0..1 + random.below(6) {
                let at = random.below(data.len());
                match random.below(5) {
                    0 => data[at] = random.next() as u8,
                    1 => {
                        let end = (at + random.below(16)).min(data.len());
                        data.drain(at..end);
                    }
                    2 => {
                        let token = CONTENT_TOKENS[random.below(CONTENT_TOKENS.len())];
                        data.splice(at..at, token.iter().copied());
                    }
                    3 => {
                        let end = (at + random.below(64)).min(data.len());
                        let piece = data[at..end].to_vec();
                        data.splice(at..at, piece);
                    }
                    _ => {
                        let to = random.below(data.len());
                        let end = (at + random.below(32)).min(data.len());
                        let piece: Vec<u8> = data.drain(at..end).collect();
                        let to = to.min(data.len());
                        data.splice(to..to, piece);
                    }
                }
                if data.is_empty() {
                    break;
                }
            }
            content(&data);
        }
    }

    /// Inputs made of content's tokens run together, run through the
    /// fuzz target's body.
    #[test]
    fn random_fragments_keep_every_promise() {
        let mut random = Random(0x2545_f491_4f6c_dd1d);
        for _ in 0..10_000 {
            let pieces = random.below(48);
            let data: Vec<u8> = (0..pieces)
                .flat_map(|_| CONTENT_TOKENS[random.below(CONTENT_TOKENS.len())])
                .copied()
                .collect();
            content(&data);
        }
    }

    /// With PDF_CONTENT_STREAMS_FUZZ_SEEDS naming a folder, writes the
    /// seeds there, for the fuzzer to begin from (`just fuzz content`).
    #[test]
    #[ignore = "needs PDF_CONTENT_STREAMS_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir = std::env::var_os("PDF_CONTENT_STREAMS_FUZZ_SEEDS")
            .expect("PDF_CONTENT_STREAMS_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in fixtures().into_iter().enumerate() {
            std::fs::write(dir.join(format!("content-{i}")), seed).unwrap();
        }
    }
}
