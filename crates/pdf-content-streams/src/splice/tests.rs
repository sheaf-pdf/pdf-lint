//! Splices: ranges taken out across streams, every other byte kept; what
//! would change the rest of the page refused; and property lists written
//! again without some of their keys.

use super::*;
use crate::{Pos, Properties, marked};

const LIM: &Limits = &Limits::DEFAULT;

fn span(start: (u32, u32), end: (u32, u32)) -> SpanAcross {
    let pos = |(stream, at)| Pos { stream, at };
    SpanAcross {
        start: pos(start),
        end: pos(end),
    }
}

/// The span of `part` in `s`, one stream.
fn span_of(s: &[u8], part: &[u8]) -> SpanAcross {
    let at = s.windows(part.len()).position(|w| w == part).unwrap() as u32;
    span((0, at), (0, at + part.len() as u32))
}

/// The marked ranges of `c`, wherever they are.
fn ranges(c: &Concat) -> Vec<SpanAcross> {
    (marked(c, &Properties::default(), &ResCx::none(), LIM).unwrap())
        .iter()
        .map(|m| m.span)
        .collect()
}

fn spliced<'a>(c: &'a Concat, drop: &[SpanAcross]) -> Result<Vec<Cow<'a, [u8]>>, SpliceError> {
    splice(c, &ResCx::none(), LIM, drop)
}

/// `s` with `part` taken out.
fn without(s: &[u8], part: &[u8]) -> Result<Vec<u8>, SpliceError> {
    let c = Concat::new(&[s]);
    let mut out = spliced(&c, &[span_of(s, part)])?;
    Ok(out.remove(0).into_owned())
}

fn is_unsafe(r: Result<Vec<u8>, SpliceError>) -> bool {
    matches!(r, Err(SpliceError::Unsafe(_)))
}

#[test]
fn a_splice_of_nothing_is_the_streams_as_they_are() {
    let streams: [&[u8]; 3] = [b"q /P <</MCID 0>> BDC (ab", b"c) Tj", b"EMC Q % end"];
    let c = Concat::new(&streams);
    let out = spliced(&c, &[]).unwrap();
    assert!(out.iter().all(|s| matches!(s, Cow::Borrowed(_))));
    assert_eq!(out, streams.map(Cow::Borrowed));
}

#[test]
fn a_range_across_streams_is_taken_out_of_both() {
    let streams: [&[u8]; 3] = [b"q /A BMC q 0 g", b"0 0 9 9 re f Q EMC 1 g", b"Q"];
    let c = Concat::new(&streams);
    let out = spliced(&c, &[ranges(&c)[0]]).unwrap();
    assert_eq!(out[0], Cow::<[u8]>::Owned(b"q ".to_vec()));
    assert_eq!(out[1], Cow::<[u8]>::Owned(b" 1 g".to_vec()));
    assert!(matches!(out[2], Cow::Borrowed(b"Q")));
}

#[test]
fn a_splice_inside_a_text_object_keeps_the_text_where_it_was() {
    let body =
        |artifact: &str| format!("BT /F1 12 Tf 72 700 Td {artifact} (Body) Tj ET").into_bytes();
    // Color in its own `q`…`Q` can go.
    let s = body("/Artifact BMC q 1 0 0 rg Q EMC");
    assert_eq!(
        without(&s, b"/Artifact BMC q 1 0 0 rg Q EMC").unwrap(),
        b"BT /F1 12 Tf 72 700 Td  (Body) Tj ET"
    );
    // A color, a font, a move, or a line moved by `'` would change the
    // body text; a move would even inside a `q`.
    for artifact in [
        "/Artifact BMC 1 0 0 rg EMC",
        "/Artifact BMC /F2 9 Tf EMC",
        "/Artifact BMC 0 50 Td EMC",
        "/Artifact BMC q 0 50 Td Q EMC",
        "/Artifact BMC (x) ' EMC",
        "/Artifact BMC 2 Tr EMC",
    ] {
        assert!(
            is_unsafe(without(&body(artifact), artifact.as_bytes())),
            "{artifact}"
        );
    }
    // Text it shows moves the body text on, unless a line is moved first:
    // setting text state doesn't move one.
    let shows = "/Artifact BMC (Head) Tj EMC";
    assert!(is_unsafe(without(&body(shows), shows.as_bytes())));
    let moved = format!("BT /F1 12 Tf 72 700 Td {shows} 0 -14 Td (Body) Tj ET").into_bytes();
    assert!(without(&moved, shows.as_bytes()).is_ok());
    for state in [
        "/F1 12 Tf",
        "0 Tc",
        "0 Tw",
        "100 Tz",
        "14 TL",
        "0 Tr",
        "0 Ts",
    ] {
        let s = format!("BT 100 700 Td {shows} {state} (Body) Tj ET").into_bytes();
        assert!(is_unsafe(without(&s, shows.as_bytes())), "{state}");
    }
    let last = format!("BT /F1 12 Tf 72 700 Td (Body) Tj {shows} ET").into_bytes();
    assert!(without(&last, shows.as_bytes()).is_ok());
    // Out of the text object it's in: an end without a start.
    let s = b"BT /A BMC (x) Tj ET EMC";
    assert!(is_unsafe(without(s, b"/A BMC (x) Tj ET EMC")));
}

#[test]
fn a_range_that_doesnt_balance_is_refused() {
    let s = b"q /A BMC Q q EMC Q";
    assert!(is_unsafe(without(s, b"/A BMC Q q EMC")));
    let s = b"/A BMC BT /F1 9 Tf EMC (x) Tj ET";
    assert!(is_unsafe(without(s, b"/A BMC BT /F1 9 Tf EMC")));
    // Its own marked content, and q and Q, balanced: it goes.
    let s = b"0 g /A BMC q /B BMC 1 g EMC Q EMC 0 0 m";
    assert_eq!(
        without(s, b"/A BMC q /B BMC 1 g EMC Q EMC").unwrap(),
        b"0 g  0 0 m"
    );
    // Half a range.
    let s = b"/A BMC /B BMC EMC EMC";
    assert!(is_unsafe(without(s, b"/A BMC /B BMC EMC")));
}

/// Graphics state a range sets outside a `q`…`Q` of its own would last
/// past it: the CTM, a color, a line width, a clip, a `gs`, a font.
#[test]
fn a_range_that_sets_state_is_refused() {
    for state in [
        "2 0 0 2 0 0 cm",
        "1 0 0 rg",
        "/CS0 cs 0 scn",
        "3 w",
        "/GS0 gs",
        "0 0 9 9 re W n",
        "/F1 9 Tf",
    ] {
        let range = format!("/Artifact BMC {state} EMC");
        let s = format!("0 g {range} 0 0 9 9 re f");
        assert!(
            is_unsafe(without(s.as_bytes(), range.as_bytes())),
            "{state}"
        );
        let inside = format!("/Artifact BMC q {state} Q EMC");
        let s = format!("0 g {inside} 0 0 9 9 re f");
        assert_eq!(
            without(s.as_bytes(), inside.as_bytes()).unwrap(),
            b"0 g  0 0 9 9 re f",
            "{state}"
        );
    }
}

/// A path is painted by the operator that ends it: a range can't take
/// part of one.
#[test]
fn a_range_inside_a_path_is_refused() {
    let s = b"0 0 m /A BMC EMC 9 9 l S";
    assert!(is_unsafe(without(s, b"/A BMC EMC")));
    let s = b"/A BMC 0 0 m 9 9 l EMC S";
    assert!(is_unsafe(without(s, b"/A BMC 0 0 m 9 9 l EMC")));
    let s = b"/A BMC 0 0 m 9 9 l S EMC 0 0 9 9 re f";
    assert_eq!(
        without(s, b"/A BMC 0 0 m 9 9 l S EMC").unwrap(),
        b" 0 0 9 9 re f"
    );
}

/// Marked content draws nothing, but for optional content: one range may
/// take out the start of an artifact and another its end, if no marked
/// content is kept between them. An artifact may open with the `Q` that
/// ends the page's `q`, as Acrobat's first on a page does; keeping it
/// leaves the page as it was.
#[test]
fn marked_content_may_open_in_one_range_and_close_in_another() {
    let s: &[u8] = b"q 0 g /Artifact <</Subtype /Header>> BDC Q q 1 0 0 1 9 9 cm /X0 Do Q EMC 1 g";
    let c = Concat::new(&[s]);
    let header = ranges(&c)[0];
    // Alone, it would end the page's `q`.
    assert!(matches!(
        spliced(&c, &[header]),
        Err(SpliceError::Unsafe(_))
    ));
    // Its start, and what's after its `Q`, the `Q` kept.
    let cuts = [
        span_of(s, b"/Artifact <</Subtype /Header>> BDC "),
        span_of(s, b" q 1 0 0 1 9 9 cm /X0 Do Q EMC"),
    ];
    assert_eq!(spliced(&c, &cuts).unwrap()[0], &b"q 0 g Q 1 g"[..]);
    // Marked content kept between them, or an end with no start taken.
    let s = b"/A BMC /B BMC EMC EMC";
    let c = Concat::new(&[s]);
    let ends = [span_of(s, b"/A BMC"), span((0, 18), (0, 21))];
    assert!(matches!(spliced(&c, &ends), Err(SpliceError::Unsafe(_))));
    let s = b"/A BMC EMC EMC";
    assert!(is_unsafe(without(s, b"EMC EMC")));
    let s = b"/A BMC 0 g EMC";
    let c = Concat::new(&[s]);
    let ends = [span_of(s, b"/A BMC"), span_of(s, b"EMC")];
    assert_eq!(spliced(&c, &ends).unwrap()[0], &b" 0 g "[..]);
    // Optional content hides what it marks: its ends stay together.
    let s = b"/OC /oc1 BDC 0 g EMC";
    let c = Concat::new(&[s]);
    let ends = [span_of(s, b"/OC /oc1 BDC"), span_of(s, b"EMC")];
    assert!(matches!(spliced(&c, &ends), Err(SpliceError::Unsafe(_))));
}

#[test]
fn ranges_must_lie_between_tokens() {
    let s: &[u8] = b"(a b) Tj % note\n0 g";
    // Inside a string.
    assert!(is_unsafe(without(s, b"b) Tj")));
    // Ending in a comment: what's left of it would draw.
    assert!(is_unsafe(without(s, b"Tj % no")));
    // Starting in one: what follows would be commented out.
    assert!(is_unsafe(without(s, b"ote\n0 g")));
    assert_eq!(without(s, b"% note\n").unwrap(), b"(a b) Tj 0 g");
    // Overlapping, outside the content, or ending before it starts.
    let c = Concat::new(&[s]);
    let overlap = spliced(&c, &[span((0, 0), (0, 8)), span((0, 6), (0, 10))]);
    assert!(matches!(overlap, Err(SpliceError::Overlap(_))));
    for outside in [
        span((0, 0), (0, 99)),
        span((1, 0), (1, 0)),
        span((0, 5), (0, 2)),
    ] {
        assert!(matches!(
            spliced(&c, &[outside]),
            Err(SpliceError::OutOfRange(_))
        ));
    }
}

#[test]
fn tokens_either_side_of_a_range_stay_apart() {
    assert_eq!(
        without(b"Q/A BMC EMC/F1 9 Tf", b"/A BMC EMC").unwrap(),
        b"Q /F1 9 Tf"
    );
    assert_eq!(
        without(b"(x)Tj/A BMC EMC(y)Tj", b"/A BMC EMC").unwrap(),
        b"(x)Tj (y)Tj"
    );
    assert_eq!(
        without(b"Q\n/A BMC EMC/F1 9 Tf", b"/A BMC EMC").unwrap(),
        b"Q\n/F1 9 Tf"
    );
    // Two ranges that touch: the bytes either side of both decide.
    let s = b"0 g/A Do  /B Do 0 0 1 1 re f";
    let c = Concat::new(&[s]);
    let out = spliced(&c, &[span((0, 3), (0, 9)), span((0, 9), (0, 16))]).unwrap();
    assert_eq!(out[0], &b"0 g 0 0 1 1 re f"[..]);
}

/// The operands before a range, or its last ones, would be the next
/// operator's.
#[test]
fn ranges_must_lie_between_steps() {
    let s: &[u8] = b"1 2 3 /A BMC EMC 4 5 6 re f";
    assert!(is_unsafe(without(s, b"/A BMC EMC")));
    assert!(is_unsafe(without(s, b"/A BMC EMC 4 5")));
    assert_eq!(without(s, b"1 2 3 /A BMC EMC").unwrap(), b" 4 5 6 re f");
}

/// What a tool might mark the lists it writes with, its own keys.
const KEYS: [&[u8]; 2] = [b"Tool_Gen", b"Tool_Block"];

/// The list of the range that opens `streams` written again without
/// [`KEYS`], and the streams as they're then.
fn rewritten(streams: &[&[u8]]) -> Result<(Option<Rewrite>, Vec<Vec<u8>>), SpliceError> {
    let c = Concat::new(streams);
    let rewrite = without_keys(&c, &ResCx::none(), LIM, ranges(&c)[0], &KEYS)?;
    let rewrites: Vec<Rewrite> = rewrite.iter().cloned().collect();
    Ok((rewrite, content_with(&c, &[], &rewrites)))
}

/// `c`'s streams with `groups` taken out and `rewrites` made.
fn content_with(c: &Concat, groups: &[Vec<SpanAcross>], rewrites: &[Rewrite]) -> Vec<Vec<u8>> {
    let spliced = splice_each(c, &ResCx::none(), LIM, groups);
    match spliced.streams(c, rewrites) {
        Some(streams) => streams.into_iter().map(Cow::into_owned).collect(),
        None => (0..c.stream_count())
            .map(|i| c.stream(i).to_vec())
            .collect(),
    }
}

/// A list loses the keys asked and keeps every other byte as it was
/// written: comments, spacing, line ends, a key written with an escape,
/// and a dictionary inside it, whose own keys are its own.
#[test]
fn a_list_is_written_again_without_its_keys() {
    // With no space between entries.
    let s: &[u8] = b"/Artifact <</Type/Pagination/Subtype/Watermark/Tool_Gen true\
        /Tool_Block({\"note\":\"(a) b\"})>> BDC\nq\n/GS0 gs Q\nEMC\n";
    let (rewrite, made) = rewritten(&[s]).unwrap();
    assert!(rewrite.is_some());
    assert_eq!(
        made,
        [b"/Artifact <</Type/Pagination/Subtype/Watermark>> BDC\nq\n/GS0 gs Q\nEMC\n"]
    );
    let s: &[u8] = b"q /Artifact << /Type /Pagination % its kind\n  /Subtype /Watermark\t\
        /Tool_Gen true /Inner <</Tool_Gen 1>> % a note\n /Tool#5FBlock (x)\n>> BDC Q EMC";
    let c = Concat::new(&[s]);
    let range = span_of(s, &s[2..]);
    let rewrite = without_keys(&c, &ResCx::none(), LIM, range, &KEYS).unwrap();
    assert_eq!(
        content_with(&c, &[], &[rewrite.unwrap()]),
        [
            &b"q /Artifact << /Type /Pagination % its kind\n  /Subtype /Watermark \
            /Inner <</Tool_Gen 1>> % a note\n\n>> BDC Q EMC"[..]
        ]
    );
    // With none of them, there's nothing to write.
    let s: &[u8] = b"/Artifact <</Type /Pagination /Subtype /Watermark>> BDC EMC";
    assert_eq!(rewritten(&[s]).unwrap(), (None, vec![s.to_vec()]));
}

/// Only a list written in the content, in one stream, whose entries are
/// names and objects, is written again.
#[test]
fn a_list_that_cant_be_written_again_is_refused() {
    let refused = |streams: &[&[u8]]| matches!(rewritten(streams), Err(SpliceError::List(_)));
    // Named, so in the resources; or marked with no list at all.
    assert!(refused(&[b"/Artifact /P0 BDC EMC"]));
    assert!(refused(&[b"/Artifact BMC EMC"]));
    // Across two streams.
    assert!(refused(&[
        b"/Artifact <</Subtype /Watermark /Tool_Gen",
        b" true>> BDC EMC"
    ]));
    // An entry whose key isn't a name, or whose value isn't an object.
    assert!(refused(&[b"/Artifact <</Tool_Gen true 1 2>> BDC EMC"]));
    assert!(refused(&[b"/Artifact <</Tool_Gen true /A b>> BDC EMC"]));
    // A range that doesn't start at its tag.
    let s: &[u8] = b"/Artifact <</Tool_Gen true>> BDC 0 0 9 9 re f EMC";
    let c = Concat::new(&[s]);
    let at = without_keys(&c, &ResCx::none(), LIM, span_of(s, b"0 0 9 9 re f"), &KEYS);
    assert!(matches!(at, Err(SpliceError::List(_))));
}

/// A list written again goes with the cuts a splice takes: in a range
/// taken out, it goes with it; elsewhere, it's made in what's kept, a
/// range a splice refuses among it.
#[test]
fn a_list_written_again_goes_with_what_s_taken_out() {
    let s: &[u8] = b"/Artifact <</Subtype /Watermark /Tool_Gen true>> BDC Q EMC \
        /Artifact <</Subtype /Background /Tool_Gen true>> BDC 0 0 9 9 re f EMC";
    let c = Concat::new(&[s]);
    let ranges = ranges(&c);
    let rewrites: Vec<Rewrite> = (ranges.iter())
        .map(|&r| {
            without_keys(&c, &ResCx::none(), LIM, r, &KEYS)
                .unwrap()
                .unwrap()
        })
        .collect();
    let spliced = splice_each(&c, &ResCx::none(), LIM, &[vec![ranges[1]]]);
    assert_eq!(spliced.taken, [true]);
    assert!(spliced.takes(&c, ranges[1]) && !spliced.takes(&c, ranges[0]));
    assert_eq!(
        content_with(&c, &[vec![ranges[1]]], &rewrites),
        [&b"/Artifact <</Subtype /Watermark>> BDC Q EMC "[..]]
    );
    // The first's `Q` would end what's before it: refused, it stays.
    let refused = splice_each(&c, &ResCx::none(), LIM, &[vec![ranges[0]]]);
    assert_eq!(refused.taken, [false]);
    assert_eq!(
        content_with(&c, &[vec![ranges[0]]], &rewrites),
        [&b"/Artifact <</Subtype /Watermark>> BDC Q EMC \
            /Artifact <</Subtype /Background>> BDC 0 0 9 9 re f EMC"[..]]
    );
}

/// A list's value nesting arrays, as arrays nest inside a dictionary, is
/// read whole, the key after it found where it is.
#[test]
fn a_list_whose_values_nest_arrays_is_written_again() {
    let s: &[u8] = b"/Artifact <</A [[1] 2] /Tool_Gen [[3] [4 [5]]] /B [[6]]>> BDC EMC";
    let (rewrite, made) = rewritten(&[s]).unwrap();
    assert!(rewrite.is_some());
    assert_eq!(made, [&b"/Artifact <</A [[1] 2] /B [[6]]>> BDC EMC"[..]]);
    let list = object(b"<</A [[1] 2] /B [[6]]>>", LIM).unwrap();
    let a = list.as_dict().and_then(|d| d.get(b"A")).unwrap();
    assert_eq!(a.as_array().map(<[_]>::len), Some(2));
}

/// Where neither byte kept either side of a cut is white-space by ISO
/// 32000-2's rules, a space goes between them, though PDFium reads 0x80
/// and 0xFF as white-space: so a 0xFF and a 0xD9 kept either side don't
/// join into the end of image a JPEG's data is searched for.
#[test]
fn a_space_keeps_apart_what_iso_would_join() {
    assert_eq!(
        without(b"q\x80/A BMC EMC\x80Q", b"/A BMC EMC").unwrap(),
        b"q\x80 \x80Q"
    );
    // A JPEG with no end of image in its data, ended by the stand-in at its
    // first `EI`; joined, a 0xFF and a 0xD9 after it would be one.
    let s: &[u8] = b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /DCT ID \xff\xd8 EI \xff/A BMC EMC \xd9 EI Q";
    let kept = without(s, b"/A BMC EMC ").unwrap();
    assert!(kept.ends_with(b"EI \xff \xd9 EI Q"));
    fn read(s: &[u8]) -> Vec<Tok<'_>> {
        (tokens(s, &ResCx::none(), LIM).map(|t| t.unwrap().1))
            .take(2)
            .collect()
    }
    assert_eq!(read(&kept), read(s));
}

/// An array inside an array, outside any dictionary, opens none, so
/// `[[1]` is an operand, and what's after it the next operator's: a range
/// after it is between an operator and its operands.
#[test]
fn a_range_after_an_array_ended_sooner_is_refused() {
    let s: &[u8] = b"[[1] /A BMC EMC 2] 0 d";
    let read: Vec<_> = (tokens(s, &ResCx::none(), LIM).map(|t| t.unwrap().1)).collect();
    assert_eq!(read[0], Tok::Operand(b"[[1]"));
    assert!(is_unsafe(without(s, b"/A BMC EMC")));
    // With the array whole in a dictionary, the range is after a step.
    let s: &[u8] = b"/P <</D [[1] 2]>> DP /A BMC EMC 0 g";
    assert_eq!(
        without(s, b"/A BMC EMC").unwrap(),
        b"/P <</D [[1] 2]>> DP  0 g"
    );
}

/// A range is judged by the reading `limits` names: a JPEG whose scan a
/// comment holding `EI` follows ends at that `EI` where it's decoded, as
/// PDFium's decoder stops past the marker after its scan, so a range after
/// the JPEG is content; by the stand-in, the JPEG's data ends past its end
/// of image, and the range is between its data and its `EI`, inside the
/// image.
#[cfg(feature = "image-ends")]
#[test]
fn a_range_is_judged_by_the_reading_asked() {
    use crate::CodedEnds;
    use crate::fuzz::{BLOCK, jpeg};

    // Its length, 32, is two bytes of white-space.
    let body = format!("{:<30}", " EI").into_bytes();
    let n = u16::try_from(body.len() + 2).unwrap();
    let comment = [&[0xff, 0xfe][..], &n.to_be_bytes(), &body].concat();
    let whole = jpeg(8, 8, b"", BLOCK);
    let data = [&whole[..whole.len() - 2], &comment, b"\xff\xd9"].concat();
    let s = [
        &b"q BI /W 8 /H 8 /BPC 8 /CS /G /F /DCT ID "[..],
        &data,
        b" /A BMC EMC\nEI Q",
    ]
    .concat();
    let c = Concat::new(&[&s]);
    let range = [span_of(&s, b"/A BMC EMC")];
    let decoded = Limits {
        coded_ends: CodedEnds::Decoded,
        ..Limits::DEFAULT
    };
    let kept = splice(&c, &ResCx::none(), &decoded, &range).unwrap();
    assert_eq!(kept[0].len(), s.len() - b"/A BMC EMC".len());
    let refused = splice(&c, &ResCx::none(), LIM, &range);
    assert!(matches!(refused, Err(SpliceError::Unsafe(_))));
}
