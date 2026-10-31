//! What the fuzz target checks, which unit tests check too: the promises
//! the tokens, the operations, the faults and the operator table and its
//! states make, and the balance, marked content and names over them, on
//! any input. It's compiled only for tests and under cargo-fuzz's
//! `--cfg fuzzing`.

use std::ops::Range;

use pdf_objects::{Object, Value, dict};
use pdf_tokenizer::decode;

use crate::data::{RunLength, Searched, a85_stop, ei_scan, find};
use crate::fault::{Fault, Sink};
use crate::scan::{Raw, next_raw, next_raw_walked};
use crate::{
    Balance, Concat, Error, ErrorKind, Join, Limits, MarkedEvent, MarkedNest, NameSet, Operand,
    Operation, Operations, Operator, Properties, Props, ResCx, ResKind, SpanAcross, State, Tok,
    balance, kept, marked, name, names_used, object, operations, tokens,
};

/// A sink that keeps every fault it's told, in order, where it is.
#[derive(Debug, Default)]
pub struct Collected<'a>(pub Vec<(Range<usize>, Fault<'a>)>);

impl<'a> Sink<'a> for Collected<'a> {
    fn fault(&mut self, at: Range<usize>, fault: Fault<'a>) {
        self.0.push((at, fault));
    }
}

/// A sink that wants no faults, and keeps any it's told.
#[derive(Debug, Default)]
pub struct Unwanted<'a>(pub Vec<(Range<usize>, Fault<'a>)>);

impl<'a> Sink<'a> for Unwanted<'a> {
    fn fault(&mut self, at: Range<usize>, fault: Fault<'a>) {
        self.0.push((at, fault));
    }

    fn wants(&self) -> bool {
        false
    }
}

/// What one read gave: each token and where it is, then the error, if
/// one stopped it.
type Read<'a> = Vec<Result<(Range<usize>, Tok<'a>), Error>>;

/// The fuzz target's body: `data` as a page's content, read under the
/// default caps, every token taken: whole, and as two streams split at
/// its middle, joined by [`Join`] and by [`Concat::new`] alike. The
/// resources name a color space of each size an inline image may be drawn
/// in. Each read's tokens tile its bytes ([`tiled`]), and its operations
/// are its tokens grouped ([`grouped`]), each typed by the operator table
/// and stepped through the graphics objects' states ([`typed`],
/// [`stepped`]). Read again under small caps on operands and an inline
/// image's data, the tokens are the first read's up to an error, which is
/// the cap's, where the first read's token past it starts, or the first
/// read's own; and the operations are those tokens grouped. Each read
/// through a sink that keeps its faults is the same as through `()`
/// ([`faulted`]), and one that wants none is told none. And the passes
/// over its operations are their steppers' ([`passes`]). Its searches
/// and skims are plain ones' ([`searched`]). Panics on a broken promise.
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
    faulted(&whole, &res, limits);
    let halves = tiled(&split, &res, limits);
    grouped(
        split.bytes(),
        split.operations(&res, limits),
        &halves,
        limits,
    );
    faulted(&split, &res, limits);
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
    faulted(&whole, &res, &small);
    passes(data);
    searched(data);
}

/// Checks the searches the tokens make in `s`'s bytes, and their skims,
/// against plain ones: from offsets spread over `s` (its first and last
/// 16, and some 48 between), an inline image's end markers (`EI`, AHx's
/// `>`, a JPEG's end of image) where a look at every window finds them,
/// the first `EI` between white-space where a walk of the bytes does, and
/// the remembered searches, taken from those offsets in turn, where the
/// plain ones find (RunLength's walk to its end-of-data byte, where ASCII85
/// data stops, AHx's `>` and a JPEG's end of image); and from the start
/// and from each `[` or `<` (some 32 of them spread over `s`, where there
/// are more), an array or a dictionary skimmed is read to its end, or to
/// its error, where a walk of every token it holds reads it, under the
/// default cap on depth and one of two.
pub fn searched(s: &[u8]) {
    let n = s.len();
    // Where each marker next is from each offset, found from the end.
    let mut next = [
        vec![None; n + 1],
        vec![None; n + 1],
        vec![None; n + 1],
        vec![None; n + 1],
    ];
    for i in (0..n).rev() {
        let ei = s[i..].starts_with(b"EI");
        let before = i > 0 && decode_space(s[i - 1]);
        let after = s
            .get(i + 2)
            .is_none_or(|&c| decode_space(c) || b"()<>[]{}/%".contains(&c));
        let here = [
            ei,
            s[i] == b'>',
            s[i..].starts_with(b"\xff\xd9"),
            ei && before && after,
        ];
        for (k, found) in here.into_iter().enumerate() {
            next[k][i] = if found { Some(i) } else { next[k][i + 1] };
        }
    }
    let stride = (n / 48).max(1);
    let offsets = (0..=n.min(16))
        .chain((0..=n).step_by(stride))
        .chain(n.saturating_sub(16)..=n);
    let mut run_length = RunLength::default();
    let mut remembered: [Searched; 3] = Default::default();
    for at in offsets {
        for (k, needle) in [&b"EI"[..], b">", b"\xff\xd9"].into_iter().enumerate() {
            assert_eq!(find(s, at, needle), next[k][at], "{needle:?} from {at}");
        }
        assert_eq!(ei_scan(s, at), next[3][at], "EI from {at}");
        // A run copies the n + 1 bytes after a length byte n under 128, or
        // repeats the one after a length byte over it.
        let mut walked = at;
        let walked = loop {
            match s.get(walked) {
                Some(&n @ 0..=127) => walked += usize::from(n) + 2,
                Some(128) => break Some(walked + 1),
                Some(_) => walked += 2,
                None => break None,
            }
        };
        assert_eq!(run_length.end(s, at), walked, "RunLength from {at}");
        let a85 = |c: &u8| matches!(c, b'!'..=b'u' | b'z' | b'\r' | b'\n' | b' ' | b'\t');
        let stop = s
            .get(at..)
            .and_then(|rest| rest.iter().position(|c| !a85(c)));
        let plain = [stop.map(|n| at + n), next[1][at], next[2][at]];
        for (k, plain) in plain.into_iter().enumerate() {
            let found = remembered[k].from(at, |at| match k {
                0 => a85_stop(s, at),
                1 => find(s, at, b">"),
                _ => find(s, at, b"\xff\xd9"),
            });
            assert_eq!(found, plain, "search {k} from {at}");
        }
    }
    let read = |r: Option<Result<(Range<usize>, Raw), Error>>| r.map(|r| r.map(|(at, _)| at));
    let openers: Vec<usize> = (0..n).filter(|&i| matches!(s[i], b'[' | b'<')).collect();
    let some = openers.iter().step_by((openers.len() / 32).max(1));
    for &at in std::iter::once(&0).chain(some) {
        for depth in [Limits::DEFAULT.depth, 2] {
            let skimmed = read(next_raw(s, at, depth));
            assert_eq!(skimmed, read(next_raw_walked(s, at, depth)), "from {at}");
        }
    }
}

/// ISO 32000-2's white-space, the six bytes (7.2.3), written out.
fn decode_space(c: u8) -> bool {
    matches!(c, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

/// `data` as a page's content, two streams split at its middle, drawn
/// with resources that name a property list and a color space: its
/// `q`/`Q` balance, its marked ranges and the resource names it uses, each
/// what its stepper gives, driven an operation at a time ([`Balance::take`],
/// [`stepped_marked`], [`NameSet::take`]), and each name the one its
/// operator's category says ([`resource_typed`]); its marked ranges again
/// under a cap of two deep. With its ranges at the top taken out, and
/// with every range and ranges across two at the top, in no order and
/// overlapping, what it keeps is what the operations outside them give,
/// and the names only those inside use are the rest ([`kept_by_hand`]).
/// Panics on a broken promise.
pub fn passes(data: &[u8]) {
    let (first, second) = data.split_at(data.len() / 2);
    let page = Concat::new(&[first, second]);
    let res = ResCx::new([(b"CS3".to_vec(), 3)]);
    let props = Properties::new([(
        b"P0".to_vec(),
        dict! { "Type" => "Pagination", "Subtype" => "Header" },
    )]);
    let limits = &Limits::DEFAULT;
    let ops: Vec<Result<Operation, Error>> = page.operations(&res, limits).collect();
    let error = ops.iter().find_map(|op| op.as_ref().err()).copied();
    let mut counted = Balance::default();
    let mut names = NameSet::default();
    for op in ops.iter().flatten() {
        counted.take(op);
        names.take(op);
        resource_typed(op);
    }
    let read = balance(&page, &res, limits);
    assert_eq!(read, error.map_or(Ok(counted), Err), "balance");
    let read = names_used(&page, &res, limits, &[]);
    assert_eq!(read, error.map_or(Ok(names.clone()), Err), "names used");
    let found = stepped_marked(&page, &ops, &props, limits);
    assert_eq!(marked(&page, &props, &res, limits), found, "marked");
    let shallow = Limits {
        depth: 2,
        ..*limits
    };
    let ops_shallow: Vec<_> = page.operations(&res, &shallow).collect();
    assert_eq!(
        marked(&page, &props, &res, &shallow),
        stepped_marked(&page, &ops_shallow, &props, &shallow),
        "marked under a cap"
    );

    let Ok(found) = found else { return };
    let top: Vec<SpanAcross> = found
        .iter()
        .filter(|m| m.depth == 0)
        .map(|m| m.span)
        .collect();
    kept_by_hand(&page, &ops, &res, &top, &names);
    // Every range, the innermost first, and ranges each across two at the
    // top: in no order, overlapping.
    let mut every: Vec<SpanAcross> = found.iter().rev().map(|m| m.span).collect();
    let tops: Vec<Range<usize>> = top.iter().filter_map(|&s| page.offsets(s)).collect();
    for pair in tops.windows(2) {
        let middle = |r: &Range<usize>| r.start + (r.end - r.start) / 2;
        every.push(page.span(middle(&pair[0])..middle(&pair[1])));
    }
    kept_by_hand(&page, &ops, &res, &every, &names);
}

/// What `page`, its operations `ops`, keeps once the ranges `drop` are
/// taken out, as [`kept`] gives it, is what each operation held to each
/// range by hand gives: the balance and names of those outside every
/// range, and the names only those inside one use; and [`names_used`]
/// with the same ranges, its names. `names` are those the whole uses.
fn kept_by_hand(
    page: &Concat,
    ops: &[Result<Operation, Error>],
    res: &ResCx,
    drop: &[SpanAcross],
    names: &NameSet,
) {
    let limits = &Limits::DEFAULT;
    let ranges: Vec<Range<usize>> = drop.iter().filter_map(|&s| page.offsets(s)).collect();
    assert_eq!(ranges.len(), drop.len(), "each range in the content");
    let mut kept_balance = Balance::default();
    let (mut kept_names, mut inside) = (NameSet::default(), NameSet::default());
    for op in ops.iter().flatten() {
        let r = op.range();
        if ranges.iter().any(|d| d.start <= r.start && r.end <= d.end) {
            inside.take(op);
        } else {
            kept_balance.take(op);
            kept_names.take(op);
        }
    }
    let k = kept(page, res, limits, drop).expect("read as before");
    assert_eq!((k.balance, &k.names), (kept_balance, &kept_names), "kept");
    for (kind, name) in inside.iter() {
        let only_inside = !kept_names.contains(kind, name);
        assert_eq!(k.dropped.contains(kind, name), only_inside, "dropped");
    }
    assert!(k.dropped.iter().all(|(kind, n)| inside.contains(kind, n)));
    assert_eq!(k.names.len() + k.dropped.len(), names.len(), "every name");
    assert_eq!(names_used(page, res, limits, drop), Ok(k.names));
}

/// `ops`, `page`'s operations under `limits`, stepped through marked
/// content by hand ([`MarkedNest`]): each `BMC` or `BDC` opens the next
/// range, as deep as those open, its tag and list its operands', spanning
/// where it starts until the `EMC` that ends it, the last open, spans it
/// through that; nothing else opens or ends one. An `EMC` with none open,
/// or a range opened past the cap, is an error where its operator is; a
/// range open at the end, where the first of those starts; and the
/// operations' own error is theirs.
fn stepped_marked<'p>(
    page: &Concat,
    ops: &[Result<Operation, Error>],
    props: &'p Properties,
    limits: &Limits,
) -> Result<Vec<crate::Marked<'p>>, Error> {
    let mut nest = MarkedNest::new(props, limits);
    let mut open: Vec<usize> = Vec::new();
    let mut opened = 0;
    for op in ops {
        let op = op.as_ref().map_err(|e| *e)?;
        let name = op.name();
        let opens = matches!(name, b"BMC" | b"BDC");
        let event = match nest.step(page, op) {
            Ok(event) => event,
            Err(e) => {
                assert_eq!(e.at, op.at().start, "{e} where its operator is");
                match e.kind {
                    ErrorKind::Emc => assert!(name == b"EMC" && open.is_empty()),
                    ErrorKind::Depth => assert!(opens && open.len() >= limits.depth),
                    kind => panic!("{kind} stepping"),
                }
                return Err(e);
            }
        };
        match event {
            Some(MarkedEvent::Opened(i)) => {
                assert!(opens, "{name:?} opens none");
                assert_eq!(i, opened, "the next range");
                opened += 1;
                let m = nest.get(i).expect("found");
                let start = page.pos(op.range().start);
                assert_eq!((m.span.start, m.span.end), (start, start));
                assert_eq!(usize::from(m.depth), open.len());
                let tag = op.arg(usize::from(name == b"BDC"));
                let tag = tag.and_then(|t| t.name()).unwrap_or_default();
                assert_eq!(m.tag, *tag, "its tag");
                let list = op.arg(0).filter(|_| name == b"BDC");
                match (&m.props, list) {
                    (Some(Props::Named(n, read)), Some(list)) => {
                        assert_eq!(list.name().as_deref(), Some(&n[..]));
                        assert_eq!(*read, props.get(n));
                    }
                    (Some(Props::Inline(_)), Some(list)) => assert!(list.is_dict()),
                    (None, list) => assert!(list.is_none_or(|l| !l.is_name() && !l.is_dict())),
                    (props, list) => panic!("{props:?} from {list:?}"),
                }
                open.push(i);
            }
            Some(MarkedEvent::Ended(i)) => {
                assert_eq!(name, b"EMC");
                assert_eq!(open.pop(), Some(i), "the last open");
                let m = nest.get(i).expect("found");
                assert_eq!(m.span.end, page.pos(op.at().end));
                assert!(m.span.start <= m.span.end);
            }
            None => assert!(!opens && name != b"EMC", "{name:?} told"),
        }
        assert!(nest.open().eq(open.iter().copied()), "those open");
    }
    assert!(nest.get(opened).is_none());
    let first_open = open
        .first()
        .map(|&i| nest.get(i).expect("found").span.start);
    let done = nest.done();
    match first_open {
        Some(start) => {
            let e = done.as_ref().expect_err("one open");
            assert_eq!((e.kind, page.pos(e.at)), (ErrorKind::Marked, start));
        }
        None => assert_eq!(done.as_ref().map(Vec::len), Ok(opened)),
    }
    done
}

/// The resource `op` names is the one its operator's category says, by
/// the table, not by its name: a font for `Tf`, an external object for
/// `Do`, and so on.
fn resource_typed(op: &Operation) {
    let Some((kind, _)) = op.resource() else {
        return;
    };
    let says = match op.operator() {
        Operator::Font => ResKind::Font,
        Operator::Draw => ResKind::XObject,
        Operator::ExtGState => ResKind::ExtGState,
        Operator::Shade => ResKind::Shading,
        Operator::StrokeColorSpace | Operator::FillColorSpace => ResKind::ColorSpace,
        Operator::BeginInlineImage => ResKind::ColorSpace,
        Operator::StrokeColorN | Operator::FillColorN => ResKind::Pattern,
        Operator::BeginMarkedProps | Operator::MarkedPointProps => ResKind::Properties,
        other => panic!("{other:?} names no resource"),
    };
    assert_eq!(kind, says, "{op:?}");
}

/// `content`'s tokens and operations through a sink that keeps their
/// faults are those through `()`; the operations' faults are the tokens'
/// and, at the end, operands no operator takes, told once; each fault is
/// inside the content, what it borrows what's where it is, and they come
/// in the order of their ends; and each `BI`'s dictionary ends where
/// PDFium's walk of it ends, told where an object ends it or it's no
/// image ([`dict_ends`]). Every operand read with a sink
/// ([`faulted_operand`]) reads as it does without. Through a sink that
/// wants none, none is told.
pub(crate) fn faulted(content: &Concat, res: &ResCx, limits: &Limits) {
    let s = content.bytes();
    let plain: Vec<_> = content.tokens(res, limits).collect();
    let mut kept = Collected::default();
    let read: Vec<_> = content.tokens(res, limits).with_sink(&mut kept).collect();
    assert_eq!(read, plain, "the same tokens through a sink");
    in_order(s, &kept.0);
    divided(content, &plain, &kept.0);
    dict_ends(s, &plain, &kept.0, limits);
    // The operands after the last operator or inline image, if no error
    // ends the tokens.
    let at_end: Vec<Range<usize>> = match plain.last() {
        Some(Err(_)) => Vec::new(),
        _ => {
            let last = plain.iter().flatten();
            let operands = last.rev().take_while(|(_, t)| matches!(t, Tok::Operand(_)));
            let mut operands: Vec<_> = operands.map(|(r, _)| r.clone()).collect();
            operands.reverse();
            operands
        }
    };

    let shown = |op: &Result<Operation, Error>| format!("{op:?}");
    let plain: Vec<String> = content
        .operations(res, limits)
        .map(|op| shown(&op))
        .collect();
    let mut faults = Collected::default();
    let read: Vec<_> = content
        .operations(res, limits)
        .with_sink(&mut faults)
        .collect();
    let shown_read: Vec<String> = read.iter().map(shown).collect();
    assert_eq!(shown_read, plain, "the same operations through a sink");
    in_order(s, &faults.0);
    let (trailing, theirs): (Vec<_>, Vec<_>) = faults
        .0
        .iter()
        .cloned()
        .partition(|(_, f)| matches!(f, Fault::Trailing { .. }));
    assert_eq!(theirs, kept.0, "the operations' faults the tokens'");
    let told: Vec<_> = trailing.iter().map(|(at, f)| (at.clone(), *f)).collect();
    let operands_at_end = match (at_end.first(), at_end.last()) {
        (Some(first), Some(last)) => {
            let count = at_end.len();
            vec![(first.start..last.end, Fault::Trailing { count })]
        }
        _ => Vec::new(),
    };
    assert_eq!(told, operands_at_end, "operands at the end told once");
    for op in read.iter().flatten() {
        for operand in op.operands() {
            faulted_operand(s, &operand, limits);
        }
    }

    let mut unwanted = Unwanted::default();
    let count = content
        .operations(res, limits)
        .with_sink(&mut unwanted)
        .count();
    assert_eq!(count, plain.len());
    assert!(unwanted.0.is_empty(), "none told where none is wanted");
}

/// An operand read with a sink that keeps its faults reads as it does
/// without, its faults inside it, in the order of their ends; a sink that
/// wants none is told none; and an operand that's no array or
/// dictionary, whose faults are its token's, has none told here.
fn faulted_operand(s: &[u8], operand: &Operand, limits: &Limits) {
    let mut kept = Collected::default();
    let read = operand.object_with(limits, &mut kept);
    assert_eq!(read, operand.object(limits), "the same object");
    let r = operand.range();
    for (at, _) in &kept.0 {
        assert!(r.start <= at.start && at.end <= r.end, "{at:?} in {r:?}");
    }
    in_order(s, &kept.0);
    if !operand.is_array() && !operand.is_dict() {
        assert!(kept.0.is_empty(), "a token's own faults are the tokens'");
    }
    let mut unwanted = Unwanted::default();
    operand.object_with(limits, &mut unwanted);
    assert!(unwanted.0.is_empty(), "none told where none is wanted");
}

/// Each place a page's streams divide what ISO 32000-2 doesn't let them
/// divide, among `faults`, holds a join, the space before a stream, and
/// is a string, a comment or an inline image; and each string token, and
/// each inline image whose data, holds one is told so.
fn divided(content: &Concat, tokens: &Read, faults: &[(Range<usize>, Fault)]) {
    let joins: Vec<usize> = (1..content.stream_count())
        .map(|i| content.range(i).start - 1)
        .collect();
    let holds = |r: &Range<usize>| joins.iter().any(|j| r.contains(j));
    let string = |w: &[u8]| {
        w.first() == Some(&b'(') || (w.first() == Some(&b'<') && w.get(1) != Some(&b'<'))
    };
    let mut told = Vec::new();
    for (at, fault) in faults {
        if let Fault::Across { written } = *fault {
            assert!(holds(at), "{at:?} holds a join");
            let comment = written.first() == Some(&b'%');
            let image = written.starts_with(b"BI");
            assert!(
                string(written) || comment || image,
                "{fault:?}: what's divided"
            );
            told.push(at.clone());
        }
    }
    for (r, tok) in tokens.iter().flatten() {
        let divided = match tok {
            Tok::Operand(w) => string(w) && holds(r),
            Tok::InlineImage(data) => holds(data),
            Tok::Op(_) => false,
        };
        assert!(!divided || told.contains(r), "{r:?} told divided");
    }
}

/// What ends the dictionary of the inline image whose `BI` is at `bi`, as
/// PDFium's `Handle_BeginImage` reads it, written out: from after `BI`, a
/// name and the token after it, its value, and so on, until what's where
/// a key should be isn't a name. Where that is, and whether it's a
/// keyword other than `ID`, which makes the `BI` no image; none where the
/// content ends first or a token there is an error.
fn dict_end(s: &[u8], bi: usize, depth: usize) -> Option<(Range<usize>, bool)> {
    let mut at = bi + 2;
    loop {
        let (key, kind) = next_raw(s, at, depth)?.ok()?;
        match kind {
            Raw::Op => return Some((key.clone(), &s[key] != b"ID")),
            Raw::Operand if s[key.start] != b'/' => return Some((key, false)),
            Raw::Operand => at = next_raw(s, key.end, depth)?.ok()?.0.end,
        }
    }
}

/// Each `BI` among `tokens`, `s`'s, read through a sink that kept
/// `faults`: an image's dictionary ends at `ID` or at an object that's no
/// name, told as [`Fault::DictEnd`] there; a `BI` that's an operator
/// alone has a keyword where a key should be, told as [`Fault::LoneBi`],
/// from the `BI` to the keyword, where reading reaches the keyword: where
/// no error stops it before. And nothing else is told as either.
fn dict_ends(s: &[u8], tokens: &Read, faults: &[(Range<usize>, Fault)], limits: &Limits) {
    let stop = tokens.last().and_then(|t| t.as_ref().err()).map(|e| e.at);
    let mut told = Vec::new();
    for (r, tok) in tokens.iter().flatten() {
        let is_bi = match tok {
            Tok::Op(op) => *op == b"BI",
            Tok::InlineImage(_) => true,
            Tok::Operand(_) => false,
        };
        if !is_bi {
            continue;
        }
        let (end, keyword) = dict_end(s, r.start, limits.depth).expect("a BI read");
        assert_eq!(
            keyword,
            matches!(tok, Tok::Op(_)),
            "{r:?}: a lone BI has a keyword"
        );
        if keyword && stop.is_none_or(|stop| stop >= end.start) {
            let written = &s[r.start..end.end];
            told.push((r.start..end.end, Fault::LoneBi { written }));
        }
        if !keyword && &s[end.clone()] != b"ID" {
            let written = &s[end.clone()];
            told.push((end, Fault::DictEnd { written }));
        }
    }
    let found: Vec<_> = (faults.iter())
        .filter(|(_, f)| matches!(f, Fault::DictEnd { .. } | Fault::LoneBi { .. }))
        .cloned()
        .collect();
    told.sort_by_key(|(at, _)| at.end);
    assert_eq!(found, told, "each dictionary's end and each lone BI told");
}

/// Faults told while reading `s`: each inside it, what it borrows what's
/// where it is, in the order of their ends, each with words.
fn in_order(s: &[u8], faults: &[(Range<usize>, Fault)]) {
    let mut end = 0;
    for (at, fault) in faults {
        assert!(at.start <= at.end && at.end <= s.len(), "{at:?} inside");
        assert!(at.end >= end, "{at:?} in the order of their ends");
        end = at.end;
        let written = match *fault {
            Fault::Number { written }
            | Fault::OutOfRange { written }
            | Fault::HexString { written }
            | Fault::Name { written, .. }
            | Fault::StringCr { written }
            | Fault::Long { written }
            | Fault::Closer { written }
            | Fault::Brace { written }
            | Fault::Keyword { written }
            | Fault::Key { written }
            | Fault::NoValue { written }
            | Fault::Across { written }
            | Fault::EscapedKey { written }
            | Fault::Abbreviation { written, .. }
            | Fault::BeforeEi { written }
            | Fault::DictEnd { written }
            | Fault::LoneBi { written } => Some(written),
            Fault::Trailing { .. } | Fault::ImageLength { .. } | Fault::AfterId => None,
        };
        if let Some(written) = written {
            assert_eq!(written, &s[at.clone()], "{fault:?} where it is");
        }
        match *fault {
            Fault::StringCr { written } => {
                assert!(
                    written.starts_with(b"(") && written.contains(&b'\r'),
                    "{fault:?}"
                );
            }
            Fault::Long { written } => assert!(written.len() > 255, "{fault:?}"),
            _ => {}
        }
        assert!(!fault.to_string().is_empty());
    }
}

/// The operations `read` under `limits` against its tokens, `tokens` (as
/// [`tiled`] gives them, under the same caps): each operator or inline
/// image where its token is, its name its operator's, with the operands
/// since the last, each where its token is, in order and inside its
/// range; [`Operation::arg`] the operands from the last; operands at the
/// end no operation's; the same error, and nothing after. Each operand
/// and each inline image as [`operation`] checks it, and each operation
/// stepped through the graphics objects' states ([`stepped`]).
fn grouped(s: &[u8], mut read: Operations, tokens: &Read, limits: &Limits) {
    let mut states = [State::ContentStream, State::GlyphStart];
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
            Tok::Op(_) => None,
            Tok::InlineImage(data) => Some(data.clone()),
        };
        let op = match read.next() {
            Some(Ok(op)) => op,
            other => panic!("{other:?} where an operation's token is, {r:?}"),
        };
        operation(s, &op, r.clone(), image, &operands, limits);
        for state in &mut states {
            *state = stepped(*state, &op);
        }
        operands.clear();
    }
    assert!(read.next().is_none(), "nothing more");
}

/// `op`, read from `s`, is the operator or inline image at `at`, with
/// `image`'s data if it's one, and the operands at `operands`; each read
/// as [`operand`] checks it, an image's dictionary between its `BI` and
/// what ends it, `ID` or an object that's no name, its data after that
/// and one white-space byte at most, and its entries in its dictionary.
/// A `BI` with no image is an operation of its own, the operands before
/// it its.
fn operation(
    s: &[u8],
    op: &Operation,
    at: Range<usize>,
    image: Option<Range<usize>>,
    operands: &[Range<usize>],
    limits: &Limits,
) {
    assert_eq!(op.at(), at);
    typed(op, image.is_some());
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
            let (end, keyword) = dict_end(s, at.start, limits.depth).expect("a BI read");
            assert!(!keyword, "an image's dictionary ends at ID or an object");
            assert_eq!(dict, at.start + 2..end.start, "BI to what ends it");
            let space = data.start.checked_sub(end.end).expect("data after it");
            assert!(space == 0 || (space == 1 && blank(&s[end.end..data.start])));
            // Either spelling of a key finds the one value.
            for [full, short] in [
                [&b"Width"[..], b"W"],
                [b"ColorSpace", b"CS"],
                [b"Filter", b"F"],
            ] {
                let value = image.entry(full);
                assert_eq!(value, image.entry(short), "either spelling");
                if let Some(value) = value {
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

/// `op`'s operator is the one its name names, an inline image's `BI`'s,
/// with its name; or `Other`, for a name none has, with nothing from the
/// table; and it's in each set its category's, and only those.
fn typed(op: &Operation, image: bool) {
    let operator = op.operator();
    assert_eq!(operator, Operator::of(op.name()));
    assert!(!image || operator == Operator::BeginInlineImage);
    match operator.name() {
        Some(name) => {
            assert_eq!(name.as_bytes(), op.name());
            assert!(operator.operands().is_some() && operator.clause().is_some());
        }
        None => {
            assert_eq!(operator, Operator::Other);
            let named = Operator::ALL
                .iter()
                .any(|o| o.name().map(str::as_bytes) == Some(op.name()));
            assert!(!named, "Other for a name none has");
            assert_eq!((operator.operands(), operator.category()), (None, None));
        }
    }
    if let Some(category) = operator.category() {
        assert!(category.operators().contains(operator));
        assert_eq!(operator.clause(), Some(category.clause()));
    }
    assert!(
        operator.paint().is_none() || operator.category() == Some(crate::Category::PathPainting)
    );
}

/// Where content is after `op`, from `state`: [`State::step`], which is
/// `after` its operator, or for an inline image `BI`, `ID` and `EI` in
/// turn, and for a `BI` that begins none the state as it was, where `BI`
/// is allowed; each state's `allows` and `allowed` agreeing with `after`.
/// Where it's not allowed, the state stays as it was, as a reader passing
/// over it would keep it.
fn stepped(state: State, op: &Operation) -> State {
    let operator = op.operator();
    let after = state.after(operator);
    assert_eq!(state.allows(operator), after.is_some());
    assert_eq!(state.allowed().contains(operator), after.is_some());
    let step = state.step(op);
    match op.inline_image() {
        Some(_) => {
            let image = after
                .and_then(|s| s.after(Operator::InlineImageData))
                .and_then(|s| s.after(Operator::EndInlineImage));
            assert_eq!(step, image, "an image is BI, ID and EI");
            assert!(image.is_none_or(|s| s == state));
        }
        None if operator == Operator::BeginInlineImage => {
            assert_eq!(step, after.map(|_| state), "a lone BI is nothing");
        }
        None => assert_eq!(step, after),
    }
    step.unwrap_or(state)
}

/// An operand, read from `s`, is the kind its first bytes say, at most
/// one; its name as [`name`] reads it; its number as [`number`] holds it;
/// and an array's elements tile what's inside it, each where it is, read
/// as the array's elements do, each its number too.
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
    number(operand);
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
        number(element);
        at = e.end;
    }
    assert!(blank(&s[at..r.end - 1]), "only white-space after");
    if let Some(Value::Array(read)) = operand.object(limits).map(Object::into_value) {
        let inside = Limits {
            depth: limits.depth.saturating_sub(1),
            ..*limits
        };
        let each: Vec<Object> = elements.iter().filter_map(|e| e.object(&inside)).collect();
        assert_eq!(each, read, "the elements read as the array's");
    }
}

/// An operand's number as PDFium reads one ([`Operand::number`]): 0 for
/// what's no number word; and for a word of no more than the 255 bytes
/// PDFium reads that ISO 32000-2 reads as a number too, ISO's value, an
/// integer in `i32`'s range exactly and a real to an `f32`'s precision.
fn number(operand: &Operand) {
    let (bytes, n) = (operand.bytes(), operand.number());
    if !operand.is_number() {
        assert_eq!(n, 0.0, "{operand:?} is no number");
        return;
    }
    if bytes.len() > 255 {
        return;
    }
    if !bytes.contains(&b'.') {
        if let Some(i) = (decode::integer(bytes).ok()).and_then(|i| i32::try_from(i).ok()) {
            assert_eq!(n, i as f32, "{operand:?}");
        }
    } else if let Ok(x) = decode::number(bytes) {
        let x = x as f32;
        if x.is_finite() {
            assert!((n - x).abs() <= x.abs() * 1e-6 + 1e-37, "{operand:?}: {n}");
        }
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
                assert!(!matches!(*op, b"true" | b"false" | b"null"));
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
/// starts as one, as the name it is, decoded, and a number word as a
/// number: an integer the number PDFium reads ([`Operand::number`]), and
/// a real finite, ISO 32000-2's value where ISO reads the word as a number
/// too, by no more than the 255 bytes PDFium reads.
fn operand_reads(operand: &[u8], limits: &Limits) {
    let read = object(operand, limits).map(|o| o.into_value());
    let as_operand = Operand::new(operand, 0..operand.len());
    if as_operand.is_number() {
        match read {
            Some(Value::Integer(n)) => assert_eq!(n as f32, as_operand.number(), "an integer"),
            Some(Value::Real(r)) => {
                assert!(r.is_finite(), "a real {r}");
                if let (true, Ok(iso)) = (operand.len() <= 255, decode::number(operand)) {
                    assert_eq!(r, iso, "a real as ISO 32000-2 reads it");
                }
            }
            _ => panic!("a number word reads as a number"),
        }
    }
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

/// Whether `gap` is white-space and comments only: white-space by
/// PDFium's rules, ISO 32000-2's six bytes and 0x80 and 0xFF.
pub(crate) fn blank(gap: &[u8]) -> bool {
    let mut comment = false;
    gap.iter().all(|&c| {
        comment = match c {
            b'%' => true,
            b'\r' | b'\n' => false,
            _ => comment,
        };
        comment
            || matches!(
                c,
                b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | 0x80 | 0xFF
            )
    })
}

/// Page content with an inline image found by each of PDFium's rules,
/// ` EI ` in its data where the data is counted or its tokens read,
/// strings, arrays, dictionaries and marked content, and a color of 32
/// components and a pattern, more operands than an operation holds
/// inline: the fuzz target's seeds.
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
        image("/F /Fl /L 4", b"xEIy"),
        image("/F /CCF /DP <</K -1>>", b"xEI EIy"),
        image("/F /XYZ", b"(x EI y)"),
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
    /// end an inline image's data, what moves text and draws forms, runs
    /// of operands, of which two are more than an operation holds inline,
    /// marked content and what names resources, and what PDFium reads
    /// otherwise than ISO 32000-2: a string's CR, a name's `#`, an inline
    /// image's numbers, numbers past 32 bits and past an `f32`.
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
        b" /Span BMC ",
        b" /X /P0 DP ",
        b" /Tag MP ",
        b" /GS0 gs ",
        b" /CS3 cs /P0 scn ",
        b" /Sh0 sh ",
        b" (a\rb\r\nc) ",
        b" /F#3G ",
        b" /W -1 /H true /BPC 2.9 ",
        b" +2147483648 ",
        b" 400000000000000000000000000000000000000.0 ",
    ];

    /// The seeds read to their end, each of the first eleven one inline
    /// image between `q` and `Q`, and the last an `scn` of 33 operands.
    #[test]
    fn the_seeds_read_to_their_end() {
        let res = ResCx::new([(b"CS3".to_vec(), 3)]);
        let seeds = fixtures();
        for (i, seed) in seeds.iter().enumerate() {
            let read: Vec<_> = tokens(seed, &res, &Limits::DEFAULT)
                .collect::<Result<_, _>>()
                .unwrap_or_else(|e| panic!("seed {i}: {e}"));
            if i < 11 {
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
