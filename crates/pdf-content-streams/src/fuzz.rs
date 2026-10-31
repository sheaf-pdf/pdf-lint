//! What the fuzz target checks, which unit tests check too: the promises
//! the tokens, the operations, the faults and the operator table and its
//! states make, the balance, marked content and names over them, and the
//! writer's, on any input. It's compiled only for tests and under
//! cargo-fuzz's `--cfg fuzzing`.

use std::borrow::Cow;
use std::ops::Range;

use pdf_objects::{Dict, Object, Str, Value, dict};
use pdf_tokenizer::decode;

use crate::data::{RunLength, Searched, a85_stop, ei_scan, find};
use crate::fault::{Fault, Sink};
use crate::inline::{Walks, ei_from};
use crate::scan::{Raw, UNCAPPED, next_raw, next_raw_walked};
use crate::{
    Balance, Blend, Color, Concat, ContentWriter, Error, ErrorKind, FillRule, GState, ImageStream,
    Join, Limits, LineCap, LineJoin, MarkedEvent, MarkedNest, NameSet, Operand, Operation,
    Operations, Operator, Paint, Properties, Props, Res, ResBody, ResCx, ResId, ResKind, Reserved,
    ResourceNeeds, Rewrite, SpanAcross, SpliceError, Spliced, State, TextMode, Tok, balance, kept,
    marked, name, names_used, object, operations, splice, splice_each, tokens, without_keys,
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

/// The caps the fuzz target's body reads under: the default caps, and
/// with the `image-ends` feature, DCT and CCITTFax data's end found by
/// decoding it.
#[cfg(feature = "image-ends")]
const LIMITS: Limits = Limits {
    coded_ends: crate::CodedEnds::Decoded,
    ..Limits::DEFAULT
};

/// The caps the fuzz target's body reads under: the default caps.
#[cfg(not(feature = "image-ends"))]
const LIMITS: Limits = Limits::DEFAULT;

/// The fuzz target's body: `data` as a page's content, read under
/// [`LIMITS`], every token taken: whole, and as two streams split at
/// its middle, joined by [`Join`] and by [`Concat::new`] alike. The
/// resources name a color space of each size an inline image may be drawn
/// in. Each read's tokens tile its bytes ([`tiled`]), and its operations
/// are its tokens grouped ([`grouped`]), each typed by the operator table
/// and stepped through the graphics objects' states ([`typed`],
/// [`stepped`]). Read again under small caps on operands, an inline
/// image's data and what finding images' ends may cost, the tokens are the
/// first read's up to an error, which is the cap's, where the first read's
/// token past it starts, or, the last of these, where its own error is,
/// an image that doesn't end, or the first read's own; and the operations
/// are those tokens grouped. With the `image-ends` feature, read whole by
/// the stand-in too, under the default caps, its tokens tile its bytes.
/// Each read through a sink that keeps its faults is the same as through
/// `()` ([`faulted`]), and one that wants none is told none. And the
/// passes over its operations are their steppers' ([`passes`]). Its
/// searches and skims are plain ones' ([`searched`]). Its operations,
/// written again, read as they did, by the stand-in's reading of DCT and
/// CCITTFax data, which reads no further than an image's token
/// ([`written_again`]); the writer's calls its bytes choose read back
/// as written ([`calls`]); and its marked ranges taken out, and their
/// lists written again, reading as the splice promises ([`spliced`]).
/// Panics on a broken promise.
pub fn content(data: &[u8]) {
    let res = ResCx::new([
        (b"CS1".to_vec(), 1),
        (b"CS3".to_vec(), 3),
        (b"CS4".to_vec(), 4),
    ]);
    let limits = &LIMITS;
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
    // The stand-in's reading too, which a reading gets unless it asks for
    // the decoders'.
    #[cfg(feature = "image-ends")]
    tiled(&whole, &res, &Limits::DEFAULT);
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
        image_ends: 24,
        ..LIMITS
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
                    ErrorKind::ImageEnds => assert!(matches!(tok, Tok::InlineImage(_))),
                    kind => panic!("{kind} where the caps' alone could be"),
                }
            }
            // An image whose data is decoded past the cap before it's found
            // not to end.
            (Err(e), Some(Err(first))) if e.kind == ErrorKind::ImageEnds => {
                assert_eq!((e.at, first.kind), (first.at, ErrorKind::InlineImage));
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
    written_again(data, &res, &Limits::DEFAULT);
    calls(data);
    spliced(data);
}

/// Checks the searches the tokens make in `s`'s bytes, and their skims,
/// against plain ones: from offsets spread over `s` (its first and last
/// 16, and some 48 between), an inline image's end markers (`EI`, AHx's
/// `>`, a JPEG's end of image) where a look at every window finds them,
/// the first `EI` between white-space where a walk of the bytes does, and
/// the remembered searches, taken from those offsets in turn, where the
/// plain ones find (RunLength's walk to its end-of-data byte, where ASCII85
/// data stops, AHx's `>` and a JPEG's end of image), and the remembered
/// walks to the next `EI` operator where plain ones find it, or don't, the
/// bytes each passed within those to the content's end; and from the
/// start and from each `[` or `<` (some 32 of them spread over `s`, where
/// there are more), an array or a dictionary skimmed is read to its end,
/// or to its error, where a walk of every token it holds reads it, and as
/// what it reads it, an operand an object or not, under the default cap on
/// depth and one of two.
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
        let before = i > 0 && iso_white_space(s[i - 1]);
        let after = s
            .get(i + 2)
            .is_none_or(|&c| iso_white_space(c) || b"()<>[]{}/%".contains(&c));
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
    let mut walks = Walks::default();
    let depth = Limits::DEFAULT.depth;
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
        let plain = ei_from(s, at, depth);
        match walks.ei_from(s, at, depth) {
            Ok(ei) => assert_eq!(Some(ei), plain, "EI walked to from {at}"),
            Err(passed) => {
                assert_eq!(plain, None, "EI walked to from {at}");
                assert!(at + passed <= n, "{passed} passed from {at}");
            }
        }
    }
    let openers: Vec<usize> = (0..n).filter(|&i| matches!(s[i], b'[' | b'<')).collect();
    let some = openers.iter().step_by((openers.len() / 32).max(1));
    for &at in std::iter::once(&0).chain(some) {
        for depth in [Limits::DEFAULT.depth, 2] {
            let skimmed = next_raw(s, at, depth);
            assert_eq!(skimmed, next_raw_walked(s, at, depth), "from {at}");
        }
    }
}

/// ISO 32000-2's white-space, the six bytes (7.2.3), written out.
fn iso_white_space(c: u8) -> bool {
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
                    (None, list) => {
                        let dict =
                            |l: &Operand| l.object(limits).is_some_and(|o| o.as_dict().is_some());
                        assert!(list.is_none_or(|l| !l.is_name() && !dict(&l)));
                    }
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

/// `data`'s operations, read whole under `limits`, each written again
/// through [`ContentWriter::operation`], its operands and operator as
/// written, an inline image whole: what's written reads as the same
/// operations, each operand as it was, and each inline image its token
/// as it was, its data and what PDFium keeps of it in the same place in
/// it; and names no resource. Content that doesn't read to its end is
/// passed over, and so is content whose reading may have looked past a
/// token's end, at what isn't written again ([`ContentWriter::operation`]
/// says): content holding an inline image PDFium keeps no stream of
/// ([`ImageStream::Missing`]), and content whose last operation is an
/// inline image read otherwise with a line's end alone after it, all
/// that's written again after the last (Flate data that's a header asking
/// for a preset dictionary, the four bytes naming it past `EI`). Under a
/// reading that decodes DCT and CCITTFax data, an image of either may have
/// looked past its token too, its decoder reading past its `EI`, which
/// `limits` with the stand-in's reading doesn't. Whether it was written
/// again.
pub fn written_again(data: &[u8], res: &ResCx, limits: &Limits) -> bool {
    let Ok(ops) = operations(data, res, limits).collect::<Result<Vec<_>, _>>() else {
        return false;
    };
    // An inline image's place in content, its data's in it and what
    // PDFium keeps of it.
    let image = |s: &[u8], image: crate::InlineImage| {
        let at = image.at();
        let inside = |r: Range<usize>| r.start - at.start..r.end - at.start;
        let kept = match image.stream() {
            ImageStream::Data(kept) => Some(inside(kept)),
            _ => None,
        };
        (s[at.clone()].to_vec(), inside(image.data()), kept)
    };
    let missing = |op: &Operation| {
        (op.inline_image()).is_some_and(|image| image.stream() == ImageStream::Missing)
    };
    if ops.iter().any(missing) {
        return false;
    }
    // The last operation an image read otherwise where what's written
    // again after it, a line's end, is all that follows it.
    if let Some(last) = ops.last().and_then(Operation::inline_image) {
        let cut = [&data[..last.at().end], b"\n"].concat();
        let read: Result<Vec<_>, _> = operations(&cut, res, limits).collect();
        let read = read.ok().and_then(|ops| {
            let i = ops.last()?.inline_image().filter(|i| i.at() == last.at())?;
            Some(image(&cut, i))
        });
        if read != Some(image(data, last)) {
            return false;
        }
    }
    let mut w = ContentWriter::new();
    for op in &ops {
        let operands = op.operands().map(|o| o.bytes());
        match op.inline_image() {
            Some(image) => w.operation(operands, &data[image.at()]),
            None => w.operation(operands, op.name()),
        }
    }
    let (again, needs) = w.finish(&Reserved::default());
    assert_eq!(needs, ResourceNeeds::default(), "no resource named");
    let read: Result<Vec<_>, _> = operations(&again, res, limits).collect();
    let shown = String::from_utf8_lossy(&again);
    let read = read.unwrap_or_else(|e| panic!("{e} written again: {shown}"));
    assert_eq!(read.len(), ops.len(), "as many operations: {shown}");
    for (op, again_op) in ops.iter().zip(&read) {
        assert_eq!(op.name(), again_op.name(), "{shown}");
        let operands = |op: &Operation| {
            op.operands()
                .map(|o| o.bytes().to_vec())
                .collect::<Vec<_>>()
        };
        assert_eq!(operands(op), operands(again_op), "{shown}");
        let images = (op.inline_image(), again_op.inline_image());
        assert_eq!(
            images.0.map(|i| image(data, i)),
            images.1.map(|i| image(&again, i)),
            "{shown}"
        );
    }
    true
}

/// Whether `read`, a number written as
/// [`write_number`](crate::write_number) writes `x`, is `x`: within its
/// rounding to four decimals and to an `f32`, and 0 for what isn't
/// finite.
pub(crate) fn read_as_written(read: f32, x: f32) -> bool {
    let x = if x.is_finite() { f64::from(x) } else { 0.0 };
    (f64::from(read) - x).abs() <= 5e-5 + x.abs() * f64::from(f32::EPSILON)
}

/// Whether `x`, finite, is outside the signed 32-bit range, where it's
/// whole and written with its point, so PDFium reads it as itself.
fn past_32_bits(x: f32) -> bool {
    let x = f64::from(x);
    x.is_finite() && !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&x)
}

/// What the bytes of a fuzz target's input choose.
struct Choices<'a>(&'a [u8]);

impl Choices<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    /// One of `n`, 0 when the input's done.
    fn below(&mut self, n: usize) -> usize {
        usize::from(self.byte().unwrap_or(0)) % n
    }

    /// A number: the bits of the next four bytes, a small whole number, a
    /// number of four decimals, or one of the edges.
    fn number(&mut self) -> f32 {
        let mut word = [0; 4];
        let kind = self.byte().unwrap_or(0);
        for b in &mut word {
            *b = self.byte().unwrap_or(0);
        }
        let bits = u32::from_le_bytes(word);
        match kind % 4 {
            0 => f32::from_bits(bits),
            1 => f32::from(word[0] as i8),
            2 => bits as i32 as f32 / 10_000.0,
            _ => [
                -0.0,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                0.00005,
                -0.00005,
                0.99995,
                2_147_483_520.0,
                2_147_483_648.0,
                4_294_967_040.0,
                4_294_967_296.0,
                -2_147_483_648.0,
                -2_147_483_904.0,
                1e10,
                f32::MAX,
                f32::MIN,
            ][usize::from(word[0]) % 16],
        }
    }

    fn numbers<const N: usize>(&mut self) -> [f32; N] {
        std::array::from_fn(|_| self.number())
    }

    /// Up to `most` bytes.
    fn bytes(&mut self, most: usize) -> &[u8] {
        let n = self.below(most + 1).min(self.0.len());
        let (taken, rest) = self.0.split_at(n);
        self.0 = rest;
        taken
    }
}

/// An operand a call wrote, as it should read back.
#[derive(Debug)]
enum Arg {
    Number(f32),
    Numbers(Vec<f32>),
    Name(Vec<u8>),
    Res(ResKind, ResId),
    Codes(Vec<u8>),
}

/// Calls of a [`ContentWriter`] that `data`'s bytes choose, a few dozen,
/// their numbers from four bytes each, NaN, the infinities, `-0`, the
/// four-decimal edges and the edges of 32 bits among them, their codes and
/// tags from the input; written, and read back: each call's operator and
/// operands, each number within its rounding (and, a whole number outside
/// the signed 32-bit range, as itself), each name and string as given,
/// each resource's name its need's in its category. The needs are
/// one a resource, in the order first drawn; named again with their names
/// taken, the same; and with their names bound to something else, each
/// bumped, the content naming the same needs in the same places.
pub fn calls(data: &[u8]) {
    let mut input = Choices(data);
    let fonts = ["Helvetica", "Courier", "Wide"].map(|base| {
        Res::dict(dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base })
    });
    let lists = ["Header", "Footer", "Watermark"]
        .map(|subtype| Res::dict(dict! { "Type" => "Pagination", "Subtype" => subtype }));
    let alphas = [None, Some(0.0), Some(0.5), Some(1.5), Some(f32::NAN)];
    let blends = [
        None,
        Some(Blend::Normal),
        Some(Blend::Multiply),
        Some(Blend::Color),
    ];
    let paints: [(Paint, &[u8]); 9] = [
        (Paint::Fill, b"f"),
        (Paint::FillEvenOdd, b"f*"),
        (Paint::Stroke, b"S"),
        (Paint::CloseStroke, b"s"),
        (Paint::FillStroke, b"B"),
        (Paint::FillStrokeEvenOdd, b"B*"),
        (Paint::CloseFillStroke, b"b"),
        (Paint::CloseFillStrokeEvenOdd, b"b*"),
        (Paint::None, b"n"),
    ];
    let mut w = ContentWriter::new();
    let mut want: Vec<(&[u8], Vec<Arg>)> = Vec::new();
    let numbers = |xs: &[f32]| xs.iter().map(|&x| Arg::Number(x)).collect::<Vec<_>>();
    for _ in 0..48 {
        let Some(call) = input.byte() else { break };
        match call % 32 {
            0 => {
                w.save();
                want.push((b"q", vec![]));
            }
            1 => {
                w.restore();
                want.push((b"Q", vec![]));
            }
            2 => {
                let m = input.numbers::<6>();
                w.transform(m);
                want.push((b"cm", numbers(&m)));
            }
            3 | 4 => {
                let stroke = call % 32 == 4;
                let (color, op, values): (_, [&[u8]; 2], Vec<f32>) = match input.below(3) {
                    0 => {
                        let v = input.number();
                        (Color::Gray(v), [b"g", b"G"], vec![v])
                    }
                    1 => {
                        let v = input.numbers::<3>();
                        (Color::Rgb(v), [b"rg", b"RG"], v.to_vec())
                    }
                    _ => {
                        let v = input.numbers::<4>();
                        (Color::Cmyk(v), [b"k", b"K"], v.to_vec())
                    }
                };
                if stroke {
                    w.stroke_color(color);
                } else {
                    w.fill_color(color);
                }
                want.push((op[usize::from(stroke)], numbers(&values)));
            }
            5 => {
                let state = GState {
                    fill_alpha: alphas[input.below(alphas.len())],
                    stroke_alpha: alphas[input.below(alphas.len())],
                    blend: blends[input.below(blends.len())],
                };
                w.gstate(state);
                want.push((b"gs", vec![Arg::Res(ResKind::ExtGState, state.res().id)]));
            }
            6 => {
                let x = input.number();
                w.line_width(x);
                want.push((b"w", vec![Arg::Number(x)]));
            }
            7 => {
                let dashes: Vec<f32> = (0..input.below(5)).map(|_| input.number()).collect();
                let phase = input.number();
                w.dash(&dashes, phase);
                want.push((b"d", vec![Arg::Numbers(dashes), Arg::Number(phase)]));
            }
            8 => {
                let (cap, n) = [
                    (LineCap::Butt, 0.0),
                    (LineCap::Round, 1.0),
                    (LineCap::Square, 2.0),
                ][input.below(3)];
                w.line_cap(cap);
                want.push((b"J", vec![Arg::Number(n)]));
            }
            9 => {
                let (join, n) = [
                    (LineJoin::Miter, 0.0),
                    (LineJoin::Round, 1.0),
                    (LineJoin::Bevel, 2.0),
                ][input.below(3)];
                w.line_join(join);
                want.push((b"j", vec![Arg::Number(n)]));
            }
            10 | 11 => {
                let [x, y] = input.numbers::<2>();
                let op: &[u8] = if call % 32 == 10 {
                    w.move_to(x, y);
                    b"m"
                } else {
                    w.line_to(x, y);
                    b"l"
                };
                want.push((op, numbers(&[x, y])));
            }
            12 => {
                let [x1, y1, x2, y2, x3, y3] = input.numbers::<6>();
                w.curve_to(x1, y1, x2, y2, x3, y3);
                want.push((b"c", numbers(&[x1, y1, x2, y2, x3, y3])));
            }
            13 => {
                let [x, y, w_, h] = input.numbers::<4>();
                w.rect(x, y, w_, h);
                want.push((b"re", numbers(&[x, y, w_, h])));
            }
            14 => {
                w.close();
                want.push((b"h", vec![]));
            }
            15 => {
                let (paint, op) = paints[input.below(paints.len())];
                w.paint(paint);
                want.push((op, vec![]));
            }
            16 => {
                let (rule, op): (_, &[u8]) =
                    [(FillRule::NonZero, &b"W"[..]), (FillRule::EvenOdd, b"W*")][input.below(2)];
                w.clip(rule);
                want.push((op, vec![]));
            }
            17 => {
                w.begin_text();
                want.push((b"BT", vec![]));
            }
            18 => {
                w.end_text();
                want.push((b"ET", vec![]));
            }
            19 => {
                let font = fonts[input.below(fonts.len())].clone();
                let size = input.number();
                want.push((
                    b"Tf",
                    vec![Arg::Res(ResKind::Font, font.id), Arg::Number(size)],
                ));
                w.font(font, size);
            }
            20 | 21 => {
                let m = input.numbers::<6>();
                let op: &[u8] = if call % 32 == 20 {
                    w.text_matrix(m);
                    b"Tm"
                } else {
                    w.text_move(m[0], m[1]);
                    b"Td"
                };
                let n = if op == b"Tm" { 6 } else { 2 };
                want.push((op, numbers(&m[..n])));
            }
            22 => {
                w.next_line();
                want.push((b"T*", vec![]));
            }
            23 => {
                let n = input.below(8);
                w.text_mode(TextMode::of(n as i32).expect("a mode"));
                want.push((b"Tr", vec![Arg::Number(n as f32)]));
            }
            24 | 25 => {
                let x = input.number();
                let op: &[u8] = if call % 32 == 24 {
                    w.horizontal_scale(x);
                    b"Tz"
                } else {
                    w.leading(x);
                    b"TL"
                };
                want.push((op, vec![Arg::Number(x)]));
            }
            26 => {
                let codes = input.bytes(40).to_vec();
                w.show(&codes);
                want.push((b"Tj", vec![Arg::Codes(codes)]));
            }
            27 | 28 => {
                let form = Res::written((u32::from(input.byte().unwrap_or(0) % 4) + 1, 0));
                let drawn = Arg::Res(ResKind::XObject, form.id);
                if call % 32 == 27 {
                    w.draw(form);
                    want.push((b"Do", vec![drawn]));
                } else {
                    let [x, y, w_, h] = input.numbers::<4>();
                    w.image(form, x, y, w_, h);
                    want.push((b"q", vec![]));
                    want.push((b"cm", numbers(&[w_, 0.0, 0.0, h, x, y])));
                    want.push((b"Do", vec![drawn]));
                    want.push((b"Q", vec![]));
                }
            }
            29 | 30 => {
                let tag = String::from_utf8_lossy(input.bytes(24)).into_owned();
                let name = Arg::Name(tag.as_bytes().to_vec());
                if call % 32 == 29 {
                    w.begin_tagged(&tag);
                    want.push((b"BMC", vec![name]));
                } else {
                    let list = lists[input.below(lists.len())].clone();
                    let ResBody::Objects(objects) = &list.body else {
                        unreachable!("a list is its objects")
                    };
                    let props = objects[0].as_dict().expect("a list").clone();
                    want.push((b"BDC", vec![name, Arg::Res(ResKind::Properties, list.id)]));
                    w.begin_marked(&tag, props);
                }
            }
            _ => {
                w.end_marked();
                want.push((b"EMC", vec![]));
            }
        }
    }

    let (content, needs) = w.named(&Reserved::default());
    let shown = String::from_utf8_lossy(&content);
    // One need a resource, in the order first drawn.
    let mut drawn: Vec<(ResKind, ResId)> = Vec::new();
    for arg in want.iter().flat_map(|(_, args)| args) {
        if let Arg::Res(kind, id) = arg {
            if !drawn.iter().any(|(_, d)| d == id) {
                drawn.push((*kind, *id));
            }
        }
    }
    let given: Vec<_> = needs.0.iter().map(|n| (n.kind, n.res.id)).collect();
    assert_eq!(given, drawn, "a need a resource, first drawn first");
    let mut names: Vec<_> = needs.0.iter().map(|n| (n.kind, n.name.clone())).collect();
    for (_, name) in &names {
        assert!(name.len() == 8 && name.starts_with(b"Sh"), "{name:?}");
    }
    names.sort();
    names.dedup();
    assert_eq!(names.len(), needs.0.len(), "a name a need");

    let read: Result<Vec<_>, _> = operations(&content, &ResCx::none(), &LIMITS).collect();
    let read = read.unwrap_or_else(|e| panic!("{e} in {shown}"));
    assert_eq!(read.len(), want.len(), "an operation a call: {shown}");
    let need_of = |name: &[u8]| needs.0.iter().position(|n| n.name == name);
    let mut named_at = Vec::new();
    for (op, (name, args)) in read.iter().zip(&want) {
        assert_eq!(op.name(), *name, "{shown}");
        assert_eq!(op.operands().len(), args.len(), "{op:?} in {shown}");
        for (operand, arg) in op.operands().zip(args) {
            let as_written =
                |read: f32, x: f32| read_as_written(read, x) && (!past_32_bits(x) || read == x);
            match arg {
                Arg::Number(x) => {
                    let read = operand.number();
                    assert!(as_written(read, *x), "{x} read {read}: {operand:?}");
                }
                Arg::Numbers(xs) => {
                    let read: Vec<f32> = operand.elements().map(|e| e.number()).collect();
                    assert_eq!(read.len(), xs.len(), "{operand:?}");
                    for (&read, &x) in read.iter().zip(xs) {
                        assert!(as_written(read, x), "{x} read {read}: {operand:?}");
                    }
                }
                Arg::Name(tag) => assert_eq!(operand.name().as_deref(), Some(&tag[..])),
                Arg::Codes(codes) => {
                    let read = object(operand.bytes(), &LIMITS).map(Object::into_value);
                    assert!(
                        matches!(&read, Some(Value::String(Str::Bytes(b), _)) if b == codes),
                        "{read:?} for {codes:?}"
                    );
                }
                Arg::Res(kind, id) => {
                    let name = operand.name().expect("a resource's name");
                    let need = need_of(&name).expect("a need's name");
                    assert_eq!((needs.0[need].kind, needs.0[need].res.id), (*kind, *id));
                    assert_eq!(op.resource(), Some((*kind, name.clone())));
                    named_at.push(need);
                }
            }
        }
    }

    // Named with their names taken, the same; with each bound to
    // something else, each bumped, in the same places.
    let mut kept = Reserved::default();
    kept.take(&needs);
    assert_eq!(
        w.named(&kept),
        (content.clone(), needs.clone()),
        "taken, the same"
    );
    let (bumped, again) = w.named(&bound_elsewhere(&needs));
    for (need, was) in again.0.iter().zip(&needs.0) {
        assert_ne!(need.name, was.name, "bumped past a name bound to another");
        assert_eq!((need.kind, need.res.id), (was.kind, was.res.id));
    }
    // The places are the operands that name a resource, as above: a
    // marked-content tag the input made a bumped name is none.
    let none = ResCx::none();
    let read = operations(&bumped, &none, &LIMITS);
    let names_at: Vec<usize> = read
        .flatten()
        .filter_map(|op| op.resource())
        .map(|(kind, name)| {
            again
                .0
                .iter()
                .position(|n| (n.kind, &n.name[..]) == (kind, &name[..]))
                .expect("a need's name")
        })
        .collect();
    assert_eq!(names_at, named_at, "the same needs in the same places");
}

/// Names reserved as a page's `/Resources` binding each of `needs`' names,
/// in its category, to something else: what a writer naming them bumps
/// each past.
fn bound_elsewhere(needs: &ResourceNeeds) -> Reserved {
    let mut bound = Dict::new();
    for need in &needs.0 {
        let category = match bound.get_mut(need.kind.key()) {
            Some(category) => category,
            None => {
                bound.push(need.kind.key(), Dict::new());
                bound.get_mut(need.kind.key()).expect("just put")
            }
        };
        let category = category.as_dict_mut().expect("a category");
        category.push(&need.name, Object::null());
    }
    let resources = Object::from(bound);
    Reserved::read(&|_| None, Some(&resources))
}

/// What a tool might mark the property lists it writes with, its own keys:
/// those [`spliced`] takes out of each list.
pub(crate) const KEYS: [&[u8]; 2] = [b"Tool_Gen", b"Tool_Block"];

/// The most marked ranges at the top [`spliced`] takes out one at a time
/// and writes the lists of again, so content of many costs no more than
/// these.
const RANGES: usize = 8;

/// `data` as a page's content, two streams split at its middle, read
/// under [`LIMITS`] with resources that name a property list and a color
/// space of each size an inline image may be drawn in: its marked ranges
/// at the top, found under the same caps, the first [`RANGES`] of them
/// each taken out alone, and all of them widened to touch ([`touching`]);
/// each splice made reads as the tokens outside its cuts ([`assert_kept`]).
/// [`splice_each`] of those ranges, each a group of its own, takes what
/// [`splice`] takes of them, all together, or those it takes alone, if
/// it takes them together, or none, and gives the streams that splice
/// gives, or none where it takes none. And each of those ranges with
/// [`KEYS`] taken out of its list: the content reads as it did but for that
/// list, which reads as it did without them ([`assert_rewritten`]); given
/// twice, it's made once, and two lists written again of its list, each
/// without one of the keys, are refused. And the lists written again made
/// with what `splice_each` takes out, where they're made: what's kept reads
/// as it did, each list as it did without the keys ([`assert_made`]).
/// Panics on a broken promise; returns how many splices were made and
/// lists written again alone.
pub fn spliced(data: &[u8]) -> usize {
    let res = ResCx::new([
        (b"CS1".to_vec(), 1),
        (b"CS3".to_vec(), 3),
        (b"CS4".to_vec(), 4),
    ]);
    let props = Properties::new([(
        b"P0".to_vec(),
        dict! { "Type" => "Pagination", "Subtype" => "Header" },
    )]);
    let limits = &LIMITS;
    let (first, second) = data.split_at(data.len() / 2);
    let page = Concat::new(&[first, second]);
    let Ok(found) = marked(&page, &props, &res, limits) else {
        return 0;
    };
    let top: Vec<SpanAcross> = found
        .iter()
        .filter(|m| m.depth == 0)
        .map(|m| m.span)
        .collect();
    let groups: Vec<Vec<SpanAcross>> = top.iter().take(RANGES).map(|&s| vec![s]).collect();
    let mut drops = groups.clone();
    if top.len() > 1 {
        drops.push(touching(&page, &top));
    }
    let mut made = 0;
    for drop in &drops {
        if let Ok(kept) = splice(&page, &res, limits, drop) {
            assert_kept(&page, &res, &kept, drop);
            made += 1;
        }
    }
    let read = "content marked reads";
    let each = splice_each(&page, &res, limits, &groups).expect(read);
    if !groups.is_empty() {
        let takes = |cuts: &[SpanAcross]| splice(&page, &res, limits, cuts).is_ok();
        let alone: Vec<bool> = groups.iter().map(|g| takes(g)).collect();
        let together: Vec<SpanAcross> = (groups.iter().zip(&alone))
            .filter(|(_, taken)| **taken)
            .flat_map(|(g, _)| g.iter().copied())
            .collect();
        let (taken, cuts) = if takes(&groups.concat()) {
            (vec![true; groups.len()], groups.concat())
        } else if together.is_empty() || takes(&together) {
            (alone, together)
        } else {
            (vec![false; groups.len()], Vec::new())
        };
        assert_eq!(each.taken, taken, "what splice_each takes");
        for (g, &t) in groups.iter().zip(&taken) {
            assert_eq!(each.takes(&page, g[0]), t, "where it takes");
        }
        let streams = each.streams(&page, &res, limits, &[]).expect(read);
        let want = (!cuts.is_empty()).then(|| splice(&page, &res, limits, &cuts).expect("taken"));
        assert_eq!(streams, want, "the streams splice_each gives");
    }
    let nothing = Spliced::default();
    let mut lists = Vec::new();
    for &span in top.iter().take(RANGES) {
        if let Ok(Some(rewrite)) = without_keys(&page, &res, limits, span, &KEYS) {
            let alone = nothing.streams(&page, &res, limits, std::slice::from_ref(&rewrite));
            let rewritten = alone.expect("a list written again").expect("made");
            assert_rewritten(&page, &res, &rewritten);
            let twice = [rewrite.clone(), rewrite.clone()];
            let twice = nothing.streams(&page, &res, limits, &twice);
            assert_eq!(
                twice,
                Ok(Some(rewritten)),
                "a list written again given twice"
            );
            let one = |key| without_keys(&page, &res, limits, span, &KEYS[key..=key]);
            if let (Ok(Some(a)), Ok(Some(b))) = (one(0), one(1)) {
                let both = nothing.streams(&page, &res, limits, &[a, b]);
                assert!(matches!(both, Err(SpliceError::Overlap(_))), "{both:?}");
            }
            lists.push(rewrite);
            made += 1;
        }
    }
    if !lists.is_empty() {
        let drop: Vec<SpanAcross> = (groups.iter().zip(&each.taken))
            .filter(|(_, taken)| **taken)
            .flat_map(|(g, _)| g.iter().copied())
            .collect();
        match each.streams(&page, &res, limits, &lists) {
            Ok(Some(streams)) => assert_made(&page, &res, &streams, &drop, &lists),
            Ok(None) => panic!("lists written again, made none"),
            Err(SpliceError::Unsafe(_) | SpliceError::List(_)) => {}
            Err(e) => panic!("lists written again with what's taken out: {e:?}"),
        }
    }
    made
}

/// `spans`, in order and apart, each widened over the white-space either
/// side of it, white-space between two split at its middle: cuts that
/// touch, so whether one needs a space in its place depends on bytes its
/// neighbour takes.
fn touching(content: &Concat, spans: &[SpanAcross]) -> Vec<SpanAcross> {
    let s = content.bytes();
    let space = |c: &&u8| iso_white_space(**c);
    let ranges: Vec<Range<usize>> = spans
        .iter()
        .filter_map(|&span| content.offsets(span))
        .collect();
    let mut cuts: Vec<Range<usize>> = ranges
        .iter()
        .map(|r| {
            let before = s[..r.start].iter().rev().take_while(space).count();
            r.start - before..r.end + s[r.end..].iter().take_while(space).count()
        })
        .collect();
    for i in 1..cuts.len() {
        if cuts[i].start < cuts[i - 1].end {
            let middle = (ranges[i - 1].end + ranges[i].start) / 2;
            (cuts[i - 1].end, cuts[i].start) = (middle, middle);
        }
    }
    cuts.into_iter().map(|r| content.span(r)).collect()
}

/// A token as read, wherever it is: its kind and bytes, and an inline
/// image's data and what PDFium keeps of it, each from its start.
#[derive(Debug, PartialEq)]
enum Seen<'a> {
    Op(&'a [u8]),
    Operand(&'a [u8]),
    Image(&'a [u8], Range<usize>, ImageStream),
}

/// `content`'s tokens under [`LIMITS`], each where it is and as
/// [`Seen`]; `what` names it, where it doesn't read to its end.
fn seen<'a>(content: &'a Concat, res: &ResCx, what: &str) -> Vec<(Range<usize>, Seen<'a>)> {
    let s = content.bytes();
    let mut read = content.tokens(res, &LIMITS);
    let mut out = Vec::new();
    while let Some(token) = read.next() {
        let (r, tok) = token.unwrap_or_else(|e| panic!("{what}: {e}"));
        let seen = match tok {
            Tok::Op(op) => Seen::Op(op),
            Tok::Operand(o) => Seen::Operand(o),
            Tok::InlineImage(data) => {
                let image = crate::InlineImage::new(s, r.clone(), data.clone(), read.found());
                let inside = |d: Range<usize>| d.start - r.start..d.end - r.start;
                let stream = match image.stream() {
                    ImageStream::Data(kept) => ImageStream::Data(inside(kept)),
                    other => other,
                };
                Seen::Image(&s[r.clone()], inside(data), stream)
            }
        };
        out.push((r, seen));
    }
    out
}

/// That `kept`, `content` spliced without `drop`, reads as the tokens of
/// `content` outside `drop`: each its kind and bytes, an inline image its
/// data and what PDFium keeps of it in the same places in it.
fn assert_kept(content: &Concat, res: &ResCx, kept: &[Cow<[u8]>], drop: &[SpanAcross]) {
    let drop: Vec<Range<usize>> = drop.iter().filter_map(|&d| content.offsets(d)).collect();
    let before: Vec<Seen> = seen(content, res, "a splice read it")
        .into_iter()
        .filter(|(r, _)| !drop.iter().any(|d| d.start <= r.start && r.end <= d.end))
        .map(|(_, t)| t)
        .collect();
    let kept: Vec<&[u8]> = kept.iter().map(|s| &**s).collect();
    let kept = Concat::new(&kept);
    let after: Vec<Seen> = (seen(&kept, res, "what a splice keeps reads").into_iter())
        .map(|(_, t)| t)
        .collect();
    assert_eq!(before, after, "the tokens a splice kept");
}

/// That `made`, `content` with `drop` taken out and `lists` made, reads
/// as the tokens of `content` outside `drop`: each as [`assert_kept`] has
/// it, but each list kept, one operand, which reads as it did without
/// [`KEYS`].
fn assert_made(
    content: &Concat,
    res: &ResCx,
    made: &[Cow<[u8]>],
    drop: &[SpanAcross],
    lists: &[Rewrite],
) {
    let drop: Vec<Range<usize>> = drop.iter().filter_map(|&d| content.offsets(d)).collect();
    let before: Vec<(Range<usize>, Seen)> = seen(content, res, "what's made was read")
        .into_iter()
        .filter(|(r, _)| !drop.iter().any(|d| d.start <= r.start && r.end <= d.end))
        .collect();
    let made: Vec<&[u8]> = made.iter().map(|s| &**s).collect();
    let made = Concat::new(&made);
    let after = seen(&made, res, "what's made reads");
    assert_eq!(before.len(), after.len(), "the tokens made");
    for ((r, was), (_, now)) in before.iter().zip(&after) {
        match lists.iter().any(|l| l.range == *r) {
            true => without_keys_read(was, now),
            false => assert_eq!(was, now, "a token kept"),
        }
    }
}

/// That `now`, a list written again, is one operand that reads as `was`
/// did without [`KEYS`].
fn without_keys_read(was: &Seen, now: &Seen) {
    let (Seen::Operand(was), Seen::Operand(now)) = (was, now) else {
        panic!("a list written again is one operand: {was:?}, {now:?}")
    };
    let read = |list: &[u8]| object(list, &LIMITS);
    let mut left = (read(was).as_ref().and_then(Object::as_dict).cloned())
        .expect("a list written again was one");
    for key in KEYS {
        left.remove(key);
    }
    let now = read(now);
    assert_eq!(
        now.as_ref().and_then(Object::as_dict),
        Some(&left),
        "the list written again"
    );
}

/// That `rewritten`, `content` with a list written again without
/// [`KEYS`], reads as `content` does but for that list, one operand, which
/// reads as it did without them.
fn assert_rewritten(content: &Concat, res: &ResCx, rewritten: &[Cow<[u8]>]) {
    let streams: Vec<&[u8]> = rewritten.iter().map(|s| &**s).collect();
    let rewritten = Concat::new(&streams);
    let before = seen(content, res, "a list written again was read");
    let after = seen(&rewritten, res, "a list written again reads");
    assert_eq!(
        before.len(),
        after.len(),
        "the tokens of a list written again"
    );
    let changed: Vec<_> = (before.iter().zip(&after))
        .filter(|((_, b), (_, a))| b != a)
        .collect();
    let [((_, was), (_, now))] = changed[..] else {
        panic!("a list written again is one token: {changed:?}")
    };
    without_keys_read(was, now);
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
            Raw::Operand { .. } if s[key.start] != b'/' => return Some((key, false)),
            Raw::Operand { .. } => at = next_raw(s, key.end, depth)?.ok()?.0.end,
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
            | Fault::NestedArray { written }
            | Fault::NoDict { written }
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
            // What PDFium keeps of its data starts with it, and ends at it
            // or past it, before `EI`.
            if let ImageStream::Data(kept) = image.stream() {
                assert_eq!(kept.start, data.start, "{kept:?} from {data:?}");
                assert!(
                    data.end <= kept.end && kept.end <= at.end - 2,
                    "{kept:?} in {at:?}"
                );
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
/// and an array's elements tile what's inside it, each where it is, the
/// last ending at its `]` or with it, read as the array's elements do,
/// each its number too, and each but a keyword or a `[` that opens none,
/// no objects, read again as itself ([`operand_reads`]).
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
            at <= e.start && e.start < e.end && e.end <= r.end,
            "{e:?} in {r:?}"
        );
        assert!(blank(&s[at..e.start]), "only white-space between");
        assert_eq!(element.bytes(), &s[e.clone()]);
        number(element);
        match next_raw(element.bytes(), 0, UNCAPPED) {
            Some(Ok((_, Raw::Op))) => assert_eq!(element.object(limits), None, "a keyword"),
            _ if element.bytes() == b"[" => assert_eq!(element.object(limits), None),
            _ => operand_reads(element.bytes(), limits),
        }
        at = e.end;
    }
    assert_eq!(s[r.end - 1], b']', "an array ends at a `]`");
    if at < r.end {
        assert!(blank(&s[at..r.end - 1]), "only white-space after");
    }
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
/// too, by no more than the 255 bytes PDFium reads. Its bytes read again
/// as a token are it, whole, and it's an object, a dictionary among them,
/// where that reading makes one.
fn operand_reads(operand: &[u8], limits: &Limits) {
    let read = object(operand, limits).map(|o| o.into_value());
    let made = match next_raw(operand, 0, UNCAPPED) {
        Some(Ok((r, Raw::Operand { object }))) => {
            assert_eq!(r, 0..operand.len(), "read again, itself");
            object
        }
        again => panic!("read again, {again:?}"),
    };
    assert_eq!(
        read.is_some(),
        made,
        "an object where its reading makes one"
    );
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
            assert_eq!(
                matches!(read, Some(Value::Dict(_))),
                made,
                "a dictionary reads"
            );
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

/// Flate data holding ` EI `: a zlib header, a last stored block of
/// those four bytes, and its checksum.
pub(crate) const FLATE_EI: &[u8] = b"x\x01\x01\x04\x00\xfb\xff EI \x02\x05\x00\xcf";

/// LZW data of ` EI `: a clear code, a code for each byte and the end,
/// each 9 bits wide.
pub(crate) const LZW_EI: &[u8] = b"\x80\x08\x08\xa4\x91\x04\x04";

/// A JPEG of `w` by `h` gray samples, written by hand: its start of
/// image, `before` (segments ahead of its tables), quantizers all 1, a
/// table of 4-bit codes for each DC difference's size, 0 to 11, and one for
/// each AC value's, 1 to 10, the end of a block 0; its frame, one scan of
/// `coded`, its blocks' coded bits, and its end of image.
pub(crate) fn jpeg(w: u16, h: u16, before: &[u8], coded: &[u8]) -> Vec<u8> {
    let segment = |marker: u8, body: &[u8]| {
        let n = u16::try_from(body.len() + 2).expect("a segment's length");
        [&[0xFF, marker][..], &n.to_be_bytes(), body].concat()
    };
    let table = |class: u8, symbols: u8| {
        let mut body = vec![class, 0, 0, 0, symbols];
        body.extend([0; 12]);
        body.extend(0..symbols);
        segment(0xC4, &body)
    };
    let quantizers = [&[0][..], &[1; 64]].concat();
    let frame = [
        &[8][..],
        &h.to_be_bytes(),
        &w.to_be_bytes(),
        &[1, 1, 0x11, 0],
    ]
    .concat();
    [
        &b"\xff\xd8"[..],
        before,
        &segment(0xDB, &quantizers),
        &table(0x00, 12),
        &table(0x10, 11),
        &segment(0xC0, &frame),
        &segment(0xDA, &[1, 1, 0, 0, 63, 0]),
        coded,
        b"\xff\xd9",
    ]
    .concat()
}

/// One 8-by-8 block coded in [`jpeg`]'s tables: a DC difference of 5, AC
/// values 3, -2 and 1, and its end.
pub(crate) const BLOCK: &[u8] = b"\x3a\x59\x23\x0f";

/// A JPEG's comment segment holding ` EI `.
pub(crate) const COMMENT_EI: &[u8] = b"\xff\xfe\x00\x06 EI ";

/// Fax data of four rows 16 pixels wide, Group 4 coded (`/K -1`): a white
/// row, two with two runs of black, and one whose first four pixels are.
pub(crate) const G4: &[u8] = b"\x97\x8e\x76\xee\x08\x21\x80";

/// Flate data: a zlib header, `data` in a stored block, the last if
/// `last`, and, if it's the last, its checksum.
pub(crate) fn stored(data: &[u8], last: bool) -> Vec<u8> {
    let n = u16::try_from(data.len()).expect("a stored block's length");
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    let check = (b << 16 | a).to_be_bytes();
    let check: &[u8] = if last { &check } else { &[] };
    let header = [u8::from(last)];
    [
        &b"x\x01"[..],
        &header,
        &n.to_le_bytes(),
        &(!n).to_le_bytes(),
        data,
        check,
    ]
    .concat()
}

/// Page content with an inline image found by each of PDFium's rules,
/// ` EI ` in its data where the data is counted or its tokens read, or
/// its Flate data decoded, one decoding to more than the fuzz body's
/// small caps allow, a JPEG and fax data, which the `image-ends` feature
/// decodes, strings, arrays, dictionaries and marked content, an image
/// whose filter's end is in a comment after it, one whose Flate header
/// asks for a dictionary named past its `EI`; marked content to take out
/// and lists to write again: pagination artifacts, one opening with the
/// `Q` that ends the page's `q`, ranges inside text objects, with
/// white-space between, across the two streams the body splits a seed
/// into, and around forms drawn, and lists with a tool's own keys
/// ([`KEYS`]); a lone `BI` whose would-be dictionary a range is in, and an
/// ASCIIHex image whose `>` a range holds in a comment, which taking the
/// range out would make read otherwise; and a color of 32 components and
/// a pattern, more operands than an operation holds inline: the fuzz
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
        image(
            "/F /DCT /W 8 /H 8 /BPC 8 /CS /G",
            &jpeg(8, 8, COMMENT_EI, BLOCK),
        ),
        image("/F /Fl /L 15", FLATE_EI),
        image("/F /LZW /DP <</EarlyChange 1>> /W 4 /H 1", LZW_EI),
        image(
            "/F [/Fl] /DP [<</Predictor 12 /Columns 2>>] /W 2 /H 1",
            FLATE_EI,
        ),
        image(
            "/F /Fl /W 8 /H 4",
            &stored(b"0 1 2 3 EI 4 5 6 7 8 9 EI a b c d", true),
        ),
        image("/F /CCF /DP <</K -1>>", b"xEI EIy"),
        image("/F /CCF /DP <</K -1 /Columns 16>> /W 16 /H 4", G4),
        image("/F /XYZ", b"(x EI y)"),
        b"%c\nq /Artifact <</Type /Pagination /Subtype /Header /Attached [/Top]>> BDC\n\
          q BT /F1 12 Tf 72 720 Td [(a\\)(b) c) -250 <4142>] TJ (\\\\) Tj ET Q EMC\n\
          [1 [2 [3]] <</A <</B (c)>>>>] true false null Q { } ) ]"
            .to_vec(),
        // Arrays and dictionaries PDFium ends sooner than ISO 32000-2.
        b"[<< 1 2 >> 3] 0 d << (x) 1 >> [<< /A ] 5 >> 6] 3 4 re\n\
          /Span <</MCID 0 1 /Alt>> BDC << /A [[1] 2] /B << /C [<< /D 1 >>] >> >> EMC\n\
          BI /W 1 /H 1 /BPC 8 /CS /G /D [[0 1]] ID a EI 1 [2 <4>] <</A <</B ]>>> >> n"
            .to_vec(),
        // An image PDFium keeps no stream of, whose first `>` is in a
        // comment after it: written again, it would run on to the last
        // `EI`, which the writer's round trip passes over.
        b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 41 EI q % > (\n Q 0 0 9 9 re f <41> Tj EI".to_vec(),
        // Flate data that's a header asking for a preset dictionary, the
        // four bytes naming it past `EI`: kept to `EI` where they're there,
        // as here, and not written again, a line's end alone after it, so
        // the writer's round trip passes it over.
        b"BI /F /Fl /L 6 ID x EI\n\n".to_vec(),
        // Marked content to take out, and lists to write again.
        b"q /Artifact /P0 BDC q BT /F1 9 Tf (Head) Tj ET Q EMC Q 0 0 9 9 re f".to_vec(),
        b"q 0 g 0 0 9 9 re f 1 1 8 8 re f 2 2 7 7 re f 3 3 6 6 re f 0 0 1 rg 9 9 m 4 4 l h S  \
          /Artifact <</Type /Pagination /Subtype /Header>> BDC Q q 1 0 0 1 9 9 cm /X0 Do Q EMC"
            .to_vec(),
        b"0 g/A <<>> BDC EMC  /B <<>> BDC EMC 1 g 0 0 9 9 re f 1 1 8 8 re f 2 2 7 7 re f".to_vec(),
        b"/Artifact <</Type /Pagination /Subtype /Header>> BDC q /X0 Do Q EMC /X1 Do".to_vec(),
        b"BT /F1 12 Tf 72 700 Td /Artifact <</Subtype /Footer>> BDC (f) Tj EMC \
          0 -14 Td (Body) Tj ET"
            .to_vec(),
        b"/Artifact <</Subtype /Watermark /Tool_Gen (g1)>> BDC q 0.5 g \
          BT /F1 9 Tf (wm) Tj ET Q EMC /Span <</MCID 0>> BDC (x) Tj EMC"
            .to_vec(),
        b"1 g /Artifact <</Subtype /Background>> BDC 0 0 m 9 9 l S \
          BI /W 2 /H 1 /BPC 8 /CS /CS3 ID ab EIxyz EI EMC Q"
            .to_vec(),
        b"/Artifact <</Type/Pagination/Subtype/Background/Tool_Gen (g1)\
          /Tool_Block({\"note\":\"/a (b)\"}) % kept\n/D <</Tool_Gen 1>>>> BDC\n\
          q\n0.5 g 0 0 9 9 re f\nQ\nEMC\n"
            .to_vec(),
        // A range in a lone `BI`'s would-be dictionary: taken out, the
        // `BI` would begin an image.
        b"BI /A BMC EMC /W 1 /H 1 /CS /G /BPC 8 ID x EI Q".to_vec(),
        // An ASCIIHex image's first `>` in a comment a range holds: taken
        // out, the image would run on to the last `EI`.
        b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 41 EI /A BMC % > (\n EMC 0 0 9 9 re f <41> Tj EI"
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
        b"/F /Fl ",
        b"/F /LZW ",
        b"/DP <</Predictor 12 /Columns 2>> ",
        b"x\x01",
        b"/F /CCF ",
        b"/DP <</K -1 /Columns 16>> ",
        b"\xff\xd8",
        b"\xff\xd0",
        b"\xff\xfe\x00\x03(",
        FLATE_EI,
        LZW_EI,
        b" +2147483648 ",
        b" 400000000000000000000000000000000000000.0 ",
    ];

    /// The seeds read to their end, each of the first fifteen one inline
    /// image between `q` and `Q`, and the last an `scn` of 33 operands;
    /// each written again; and those with marked content spliced.
    #[test]
    fn the_seeds_read_to_their_end() {
        let res = ResCx::new([(b"CS3".to_vec(), 3)]);
        let seeds = fixtures();
        for (i, seed) in seeds.iter().enumerate() {
            let read: Vec<_> = tokens(seed, &res, &Limits::DEFAULT)
                .collect::<Result<_, _>>()
                .unwrap_or_else(|e| panic!("seed {i}: {e}"));
            if i < 15 {
                assert_eq!(read.len(), 3, "seed {i}");
                assert!(matches!(read[1].1, Tok::InlineImage(_)), "seed {i}");
            }
            // Each written again but those holding an image PDFium keeps
            // no stream of, of a filter it has no decoder for, and two
            // whose filter's end is past its `EI`; and the one whose last
            // image's Flate header asks for a dictionary named past it.
            let passed = matches!(i, 14 | 17 | 18 | 28);
            assert_eq!(
                written_again(seed, &res, &Limits::DEFAULT),
                !passed,
                "seed {i}"
            );
            content(seed);
        }
        // The splices and lists written again of those with marked content
        // to take out: none of the header that opens with the `Q` ending
        // the page's `q`, alone, nor of the two whose ranges would leave
        // what's kept read otherwise; and not the watermark that shows
        // text, alone, as text is shown after it outside a text object.
        let made: Vec<usize> = seeds[19..29].iter().map(|seed| spliced(seed)).collect();
        assert_eq!(made, [1, 0, 3, 1, 1, 3, 1, 1, 0, 0]);
        let last = seeds.last().unwrap();
        let scn = operations(last, &res, &Limits::DEFAULT)
            .nth(2)
            .unwrap()
            .unwrap();
        assert_eq!((scn.name(), scn.operands().len()), (&b"scn"[..], 33));
    }

    /// A marked-content tag that's the name a font is bumped to, a fuzzer
    /// having found it, is no place of the font's: the writer's calls,
    /// bumped, name the font where they did, the tag left as it is.
    #[test]
    fn a_tag_named_as_a_bumped_resource_is_none() {
        let font =
            Res::dict(dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let mut w = ContentWriter::new();
        w.font(font, 12.0);
        let (_, needs) = w.named(&Reserved::default());
        let (_, bumped) = w.named(&bound_elsewhere(&needs));
        let tag = &bumped.0[0].name;
        assert_ne!(tag, &needs.0[0].name);
        // `BMC` with that tag, then Helvetica's `Tf` at 12.
        let mut data = vec![29, tag.len() as u8];
        data.extend(tag);
        data.extend([19, 0, 1, 12, 0, 0, 0]);
        calls(&data);
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
