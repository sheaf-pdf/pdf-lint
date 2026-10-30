//! What the fuzz target checks, which unit tests check too: the promises
//! the tokens and the operations make, on any input. It's compiled only
//! for tests and under cargo-fuzz's `--cfg fuzzing`.

use std::ops::Range;

use pdf_objects::{Object, Value};

use crate::{
    Concat, Error, ErrorKind, Join, Limits, Operand, Operation, Operations, ResCx, Tok, name,
    object, operations, tokens,
};

/// What one read gave: each token and where it is, then the error, if
/// one stopped it.
type Read<'a> = Vec<Result<(Range<usize>, Tok<'a>), Error>>;

/// The fuzz target's body: `data` as a page's content, read under the
/// default caps, every token taken: whole, and as two streams split at
/// its middle, joined by [`Join`] and by [`Concat::new`] alike. The
/// resources name a color space of each size an inline image may be drawn
/// in. Each read's tokens tile its bytes ([`tiled`]), and its operations
/// are its tokens grouped ([`grouped`]). Read again under small caps on
/// operands and an inline image's data, the tokens are the first read's up
/// to an error, which is the cap's, where the first read's token past it
/// starts, or the first read's own; and the operations are those tokens
/// grouped. Panics on a broken promise.
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
    grouped(data, whole.operations(&res, limits), &read, limits);
    let halves = tiled(&split, &res, limits);
    grouped(
        split.bytes(),
        split.operations(&res, limits),
        &halves,
        limits,
    );
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
    grouped(data, operations(data, &res, &small), &capped, &small);
}

/// The operations `read` under `limits` against its tokens, `tokens` (as
/// [`tiled`] gives them, under the same caps): each operator or inline
/// image where its token is, its name its operator's, with the operands
/// since the last, each where its token is, in order and inside its
/// range; [`Operation::arg`] the operands from the last; operands at the
/// end no operation's; the same error, and nothing after. Each operand
/// and each inline image as [`operation`] checks it.
fn grouped(s: &[u8], mut read: Operations, tokens: &Read, limits: &Limits) {
    let mut operands = Vec::new();
    for token in tokens {
        let (r, tok) = match token {
            Ok(token) => token,
            Err(e) => {
                let got = read.next().map(|op| op.err());
                assert_eq!(got, Some(Some(*e)), "the tokens' error");
                break;
            }
        };
        let image = match tok {
            Tok::Operand(_) => {
                operands.push(r.clone());
                continue;
            }
            Tok::Op(op) => {
                assert!(*op != b"BI");
                None
            }
            Tok::InlineImage(data) => Some(data.clone()),
        };
        let op = match read.next() {
            Some(Ok(op)) => op,
            other => panic!("{other:?} where an operation's token is, {r:?}"),
        };
        operation(s, &op, r.clone(), image, &operands, limits);
        operands.clear();
    }
    assert!(read.next().is_none(), "nothing more");
}

/// `op`, read from `s`, is the operator or inline image at `at`, with
/// `image`'s data if it's one, and the operands at `operands`; each read
/// as [`operand`] checks it, an image's dictionary between its `BI` and
/// `ID`, its data after it and its entries in its dictionary.
fn operation(
    s: &[u8],
    op: &Operation,
    at: Range<usize>,
    image: Option<Range<usize>>,
    operands: &[Range<usize>],
    limits: &Limits,
) {
    assert_eq!(op.at(), at);
    let start = operands.first().map_or(at.start, |o| o.start);
    assert_eq!(op.range(), start..at.end, "from its first operand");
    match image {
        None => {
            assert_eq!(op.name(), &s[at.clone()]);
            assert!(op.inline_image().is_none());
        }
        Some(data) => {
            assert_eq!(op.name(), b"BI");
            let image = op.inline_image().expect("an inline image");
            assert_eq!((image.at(), image.data()), (at.clone(), data.clone()));
            let dict = image.dict();
            assert!(at.start + 2 <= dict.start && dict.end + 2 <= data.start);
            assert_eq!(&s[dict.end..dict.end + 2], b"ID");
            for keys in [
                &[&b"W"[..], b"Width"][..],
                &[b"CS", b"ColorSpace"],
                &[b"F", b"Filter"],
            ] {
                if let Some(value) = image.entry(keys) {
                    let r = value.range();
                    assert!(
                        dict.start <= r.start && r.end <= dict.end,
                        "{r:?} in {dict:?}"
                    );
                    assert_eq!(value.bytes(), &s[r]);
                }
            }
            if let Some(length) = image.length() {
                assert!(length < 1 << 32);
            }
        }
    }
    let read: Vec<Operand> = op.operands().collect();
    assert_eq!(
        (read.len(), op.operands().len()),
        (operands.len(), operands.len())
    );
    for (operand_read, r) in read.iter().zip(operands) {
        assert_eq!(operand_read.range(), *r, "in order");
        assert_eq!(operand_read.bytes(), &s[r.clone()]);
        operand(s, operand_read, limits);
    }
    assert!(operands.last().is_none_or(|o| o.end <= at.start));
    for i in 0..=read.len() {
        let from_the_last = read.len().checked_sub(i + 1).map(|k| read[k]);
        assert_eq!(op.arg(i), from_the_last, "arg {i}");
    }
    assert_eq!(op.arg(usize::MAX), None);
}

/// An operand, read from `s`, is the kind its first bytes say, at most
/// one; its name as [`name`] reads it; and an array's elements tile what's
/// inside it, each where it is, and read as the array's elements do.
fn operand(s: &[u8], operand: &Operand, limits: &Limits) {
    let bytes = operand.bytes();
    let kinds = [
        operand.is_number(),
        operand.is_name(),
        operand.is_string(),
        operand.is_array(),
        operand.is_dict(),
    ];
    assert!(kinds.iter().filter(|&&k| k).count() <= 1, "{kinds:?}");
    if operand.is_number() {
        assert!(matches!(bytes[0], b'0'..=b'9' | b'+' | b'-' | b'.'));
    }
    assert_eq!(operand.name(), name(bytes));
    let r = operand.range();
    if !operand.is_array() {
        assert_eq!(operand.elements().count(), 0);
        return;
    }
    let mut at = r.start + 1;
    let elements: Vec<Operand> = operand.elements().collect();
    for element in &elements {
        let e = element.range();
        assert!(
            at <= e.start && e.start < e.end && e.end < r.end,
            "{e:?} in {r:?}"
        );
        assert!(blank(&s[at..e.start]), "only white-space between");
        assert_eq!(element.bytes(), &s[e.clone()]);
        at = e.end;
    }
    assert!(blank(&s[at..r.end - 1]), "only white-space after");
    if let Some(Value::Array(read)) = operand.object(limits.depth).map(Object::into_value) {
        let depth = limits.depth.saturating_sub(1);
        let each: Vec<Object> = elements.iter().filter_map(|e| e.object(depth)).collect();
        assert_eq!(each, read, "the elements read as the array's");
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
/// ` EI ` in its data where the data is counted, strings, arrays,
/// dictionaries and marked content, and a color of 32 components and a
/// pattern, more operands than an operation holds inline: the fuzz
/// target's seeds.
pub fn fixtures() -> Vec<Vec<u8>> {
    let image = |dict: &str, data: &[u8]| {
        [format!("q BI {dict} ID ").as_bytes(), data, b"\nEI Q\n"].concat()
    };
    let components: String = (0..32).map(|i| format!("0.{i} ")).collect();
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
        format!("q /CS0 cs {components}/P0 scn 0 0 9 9 re f Q\n").into_bytes(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Random;

    /// What a mutation of content may write: its tokens, the bytes that
    /// end an inline image's data, what moves text and draws forms, and
    /// runs of operands, of which two are more than an operation holds
    /// inline.
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
        b" 1 2 3 4 5 6 7 8 9 ",
    ];

    /// The seeds read to their end, each of the first ten one inline
    /// image between `q` and `Q`, and the last an `scn` of 33 operands.
    #[test]
    fn the_seeds_read_to_their_end() {
        let res = ResCx::new([(b"CS3".to_vec(), 3)]);
        let seeds = fixtures();
        for (i, seed) in seeds.iter().enumerate() {
            let read: Vec<_> = tokens(seed, &res, &Limits::DEFAULT)
                .collect::<Result<_, _>>()
                .unwrap_or_else(|e| panic!("seed {i}: {e}"));
            if i < 10 {
                assert_eq!(read.len(), 3, "seed {i}");
                assert!(matches!(read[1].1, Tok::InlineImage(_)), "seed {i}");
            }
            content(seed);
        }
        let last = seeds.last().unwrap();
        let scn = operations(last, &res, &Limits::DEFAULT)
            .nth(2)
            .unwrap()
            .unwrap();
        assert_eq!((scn.name(), scn.operands().len()), (&b"scn"[..], 33));
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
