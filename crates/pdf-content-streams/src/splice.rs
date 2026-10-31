//! Splicing (ISO 32000-2, 7.8.2): a page's streams with ranges taken out,
//! every other byte as it was, never read into operations and written
//! again; and a marked range's property list written again without some
//! of its keys (14.6.2). [`splice`] has the rules.

use std::borrow::Cow;
use std::ops::Range;

use pdf_tokenizer::is_white_space;

use crate::error::Error;
use crate::inline::ResCx;
use crate::limits::Limits;
use crate::operand::{name, object};
use crate::page::{Concat, SpanAcross};
use crate::scan::{Event, Raw, TOP, Visit, next_raw, walk};
use crate::tokens::{Tok, tokens};

/// Why a splice, or a property list written again, was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum SpliceError {
    /// The content can't be read under the caps given: its error, at an
    /// offset into the joined bytes.
    #[error(transparent)]
    Read(#[from] Error),
    /// A range that isn't in the content: an end past its stream's end or
    /// past the last stream, or before its start.
    #[error("{0} is outside the content")]
    OutOfRange(SpanAcross),
    /// A range that overlaps another taken out with it.
    #[error("{0} overlaps another range taken out")]
    Overlap(SpanAcross),
    /// A range whose taking out could change what's drawn after it, by
    /// [`splice`]'s rules.
    #[error("taking out {0} would change what's drawn after it")]
    Unsafe(SpanAcross),
    /// A marked range whose property list can't be written again without
    /// the keys asked, as [`without_keys`] says.
    #[error("the property list of the range at {0} can't be written again without the keys asked")]
    List(SpanAcross),
}

/// `content`'s streams with the ranges `drop` taken out, its tokens read
/// with `res` under `limits`, every other byte as it was: content isn't
/// read into operations and written again. A stream nothing is taken from
/// is borrowed as it is. Where the bytes kept either side of a range taken
/// out would join, neither of them white-space by ISO 32000-2's rules
/// (7.2.3), a space keeps them apart; ISO's six bytes are white-space to
/// PDFium too, so what's kept stays apart by either reading.
///
/// A range is taken out only where that can't change what's drawn after
/// it:
/// - it sets no graphics state outside a `q`…`Q` of its own (8.4.2),
///   which would last past it: no `cm`, line style, rendering intent,
///   flatness or `gs` (8.4.4), color or color space (8.6.8), clip
///   (8.5.4), or text state (`Tc`, `Tw`, `Tz`, `TL`, `Tf`, `Tr`, `Ts`, and
///   `"`, which sets two; 9.3);
/// - outside a text object, its `q`s and `Q`s balance on their own, and so
///   do its `BT`s and `ET`s (9.4.1);
/// - inside one, it stays inside, its `q`s and `Q`s balance, and it moves
///   no line (`Td`, `TD`, `Tm`, `T*`, `'`, `"`; 9.4.2, 9.4.3), even inside
///   a `q`…`Q` of its own, as ISO 32000-2 puts the text matrices on the
///   graphics state stack only by its errata (9.4.1). Text it shows moved
///   the text matrix on (9.4.3), so text shown after it, in its text
///   object, before a line is moved, would move back: that refuses it too;
/// - it starts and ends outside a path being built (8.5.2), which the
///   operator after it would paint (8.5.3);
/// - marked content draws nothing (14.6), but for optional content
///   (`/OC`), which shows or hides what it marks (8.11.3.2): so marked
///   content a range opens, none of it optional, may be closed by a later
///   range, as long as no marked content is kept between them, and a
///   range closes only what it or an earlier one opened. The start and the
///   end of an artifact may go while what's between stays.
///
/// Otherwise it's refused ([`SpliceError::Unsafe`]), as it is when it
/// starts or ends inside a token or a comment, or between an operator and
/// its operands, which the next operator would take. So taking out a
/// running head drawn in the middle of a text object can't move the body
/// text. Ranges that overlap are refused too ([`SpliceError::Overlap`]),
/// and one that isn't in the content ([`SpliceError::OutOfRange`]); an
/// empty one is no cut.
///
/// The tokens are read as [`tokens`] reads them, by PDFium's rules, so a
/// range is judged by where those end each token, an inline image's among
/// them, by the reading `limits` names: its DCT and CCITTFax data's end by
/// the stand-in, or decoded ([`Limits::coded_ends`]). A splice judged by
/// one reading holds for that one: a caller that wants what PDFium draws
/// asks for the decoders' (`CodedEnds::Decoded`, with the `image-ends`
/// feature). Content that can't be read under `limits` is refused with its
/// error ([`SpliceError::Read`]). It costs a reading of `content`.
///
/// ```
/// use pdf_content_streams::{Concat, Limits, Properties, ResCx, SpliceError, marked, splice};
///
/// let (res, limits, props) = (ResCx::none(), Limits::DEFAULT, Properties::default());
/// let page = Concat::new(&[b"0 g /Artifact BMC q 1 0 0 rg 0 0 9 9 re f Q EMC 9 9 m 20 20 l S"]);
/// let found = marked(&page, &props, &res, &limits).unwrap();
/// let kept = splice(&page, &res, &limits, &[found[0].span]).unwrap();
/// assert_eq!(&*kept[0], b"0 g  9 9 m 20 20 l S");
///
/// // A color set outside a `q`...`Q` of its own would color what's after it.
/// let page = Concat::new(&[b"/Artifact BMC 1 0 0 rg EMC 0 0 9 9 re f"]);
/// let found = marked(&page, &props, &res, &limits).unwrap();
/// let refused = splice(&page, &res, &limits, &[found[0].span]);
/// assert!(matches!(refused, Err(SpliceError::Unsafe(_))));
/// ```
pub fn splice<'a>(
    content: &'a Concat,
    res: &ResCx,
    limits: &Limits,
    drop: &[SpanAcross],
) -> Result<Vec<Cow<'a, [u8]>>, SpliceError> {
    let cuts = checked(content, res, limits, drop)?;
    Ok(written(content, &cuts, &[]))
}

/// What [`splice_each`] takes out of a page's content.
#[derive(Debug, Clone)]
pub struct Spliced {
    /// Whether each group was taken out.
    pub taken: Vec<bool>,
    /// The cuts of those taken, in order.
    cuts: Vec<Cut>,
}

impl Spliced {
    /// Whether where `span` starts in `content` is taken out.
    pub fn takes(&self, content: &Concat, span: SpanAcross) -> bool {
        let at = content.offset(span.start);
        (self.cuts.iter()).any(|c| at.is_some_and(|at| c.range.contains(&at)))
    }

    /// `content`'s streams with what's taken out taken out, and `rewrites`
    /// made in what's kept; one in what's taken out goes with it. None when
    /// no stream changes.
    pub fn streams<'a>(
        &self,
        content: &'a Concat,
        rewrites: &[Rewrite],
    ) -> Option<Vec<Cow<'a, [u8]>>> {
        let rewrites: Vec<&Rewrite> = (rewrites.iter())
            .filter(|r| !(self.cuts.iter()).any(|c| overlap(&c.range, &r.range)))
            .collect();
        (!self.cuts.is_empty() || !rewrites.is_empty())
            .then(|| written(content, &self.cuts, &rewrites))
    }
}

/// `content`'s streams with as many of `groups` taken out as can be, its
/// tokens read with `res` under `limits`: each group the ranges that take
/// one thing out, all of them where [`splice`] takes them together; else
/// those it takes alone, together, or none where it refuses those
/// together. A group it refuses stays as it is ([`Spliced::taken`]).
///
/// It costs a reading of `content` for all of them together, and where
/// that's refused, one for each group and one for those taken: a caller
/// bounds how many groups it gives.
pub fn splice_each(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    groups: &[Vec<SpanAcross>],
) -> Spliced {
    let none = |taken| Spliced {
        taken,
        cuts: Vec::new(),
    };
    if groups.is_empty() {
        return none(Vec::new());
    }
    if let Ok(cuts) = checked(content, res, limits, &groups.concat()) {
        return Spliced {
            taken: vec![true; groups.len()],
            cuts,
        };
    }
    let taken: Vec<bool> = (groups.iter())
        .map(|cuts| checked(content, res, limits, cuts).is_ok())
        .collect();
    let cuts: Vec<SpanAcross> = (groups.iter().zip(&taken))
        .filter(|(_, taken)| **taken)
        .flat_map(|(cuts, _)| cuts.iter().copied())
        .collect();
    if cuts.is_empty() {
        return none(taken);
    }
    match checked(content, res, limits, &cuts) {
        Ok(cuts) => Spliced { taken, cuts },
        Err(_) => none(vec![false; groups.len()]),
    }
}

/// A marked range's property list written again ([`without_keys`]):
/// where it is in the joined bytes, and what it's now. It's made by
/// [`Spliced::streams`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    range: Range<usize>,
    with: Vec<u8>,
}

/// The property list of the `BDC` that opens `span` in `content` (14.6.2),
/// written again without the entries whose keys `keys` names: each entry
/// goes with the spaces and tabs before it on its line, and every other
/// byte stays as written, comments, line ends and what the dictionaries
/// it holds hold among them. None when it has none of them.
///
/// The `BDC`'s operands are read from `span`'s start as [`tokens`] reads
/// them, with `res` under `limits`, its list the last of them; the list's
/// entries as PDFium reads a dictionary, so arrays nest in it, and its
/// keys decoded as [`name`](crate::name) decodes them. Refused
/// ([`SpliceError::List`]) when the range doesn't open with a `BDC` whose
/// list is written in the content, in one stream, and reads as a
/// dictionary ([`object`](crate::object)), or when what's left of it isn't
/// one operand that reads as the list without those entries; every entry
/// of a key written twice goes.
///
/// ```
/// use pdf_content_streams::{Concat, Limits, Properties, ResCx, marked, splice_each, without_keys};
///
/// let (res, limits, props) = (ResCx::none(), Limits::DEFAULT, Properties::default());
/// let page = Concat::new(&[b"/Artifact <</Subtype /Watermark /Tool_Gen 1>> BDC q 0 g Q EMC"]);
/// let span = marked(&page, &props, &res, &limits).unwrap()[0].span;
/// let keys: [&[u8]; 1] = [b"Tool_Gen"];
/// let rewrite = without_keys(&page, &res, &limits, span, &keys).unwrap().unwrap();
/// // Made with nothing taken out.
/// let nothing = splice_each(&page, &res, &limits, &[]);
/// let made = nothing.streams(&page, &[rewrite]).unwrap();
/// assert_eq!(&*made[0], b"/Artifact <</Subtype /Watermark>> BDC q 0 g Q EMC");
/// ```
pub fn without_keys(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    span: SpanAcross,
    keys: &[&[u8]],
) -> Result<Option<Rewrite>, SpliceError> {
    let range = content.offsets(span).ok_or(SpliceError::OutOfRange(span))?;
    let refused = || SpliceError::List(span);
    let at = |e: Error| Error {
        at: e.at + range.start,
        ..e
    };
    let s = content.bytes();
    // Its operands, the list last, to its `BDC`.
    let mut list = None;
    let mut read = tokens(&s[range.clone()], res, limits);
    loop {
        match read.next().transpose().map_err(at)? {
            Some((r, Tok::Operand(_))) => list = Some(r),
            Some((_, Tok::Op(b"BDC"))) => break,
            _ => return Err(refused()),
        }
    }
    let list = list.map(|r| range.start + r.start..range.start + r.end);
    let list = list
        .filter(|r| s[r.clone()].starts_with(b"<<"))
        .ok_or_else(refused)?;
    let at = content.span(list.clone());
    if at.start.stream != at.end.stream {
        return Err(refused());
    }
    let with = without_entries(&s[list.clone()], keys, limits).ok_or_else(refused)?;
    Ok((with != s[list.clone()]).then_some(Rewrite { range: list, with }))
}

/// A dictionary operand as written, but for its entries whose keys are
/// among `keys`: each goes with the spaces and tabs before it on its line.
/// Its entries are found as PDFium reads it ([`walk`]), each key at its
/// first level and where its value ends. None when it doesn't read as a
/// dictionary under `limits`, or what's left isn't one operand that reads
/// as it without those entries.
fn without_entries(dict: &[u8], keys: &[&[u8]], limits: &Limits) -> Option<Vec<u8>> {
    let mut left = object(dict, limits)?.as_dict()?.clone();
    let mut entries = Entries::default();
    walk(dict, 0, TOP, limits.depth, false, &mut entries)?.ok()?;
    let mut cut = Vec::new();
    // Past the `<<`.
    let mut at = 2;
    for (key, end) in entries.found {
        let decoded = name(&dict[key.clone()])?;
        if keys.contains(&&*decoded) {
            let line = (dict[at..key.start].iter()).rposition(|&c| !matches!(c, b' ' | b'\t'));
            cut.push(line.map_or(at, |i| at + i + 1)..end);
        }
        at = end;
    }
    let mut out = Vec::with_capacity(dict.len());
    let mut from = 0;
    for r in cut {
        out.extend_from_slice(&dict[from..r.start]);
        from = r.end;
    }
    out.extend_from_slice(&dict[from..]);
    for key in keys {
        left.remove(key);
    }
    let whole = matches!(
        next_raw(&out, 0, limits.depth),
        Some(Ok((r, Raw::Operand { .. }))) if r == (0..out.len())
    );
    let read = object(&out, limits)?;
    (whole && read.as_dict() == Some(&left)).then_some(out)
}

/// A dictionary's entries as a [`walk`] from its `<<` reads them: each key
/// at its first level, and where its value ends.
#[derive(Default)]
struct Entries {
    /// How deep the walk is: 1 inside the dictionary.
    depth: usize,
    /// The key read last at the first level, its value not yet read.
    key: Option<Range<usize>>,
    found: Vec<(Range<usize>, usize)>,
}

impl<'a> Visit<'a> for Entries {
    fn visit(&mut self, event: Event<'a>) {
        let end = match event {
            Event::Key(token) if self.depth == 1 => {
                self.key = Some(token.range());
                return;
            }
            Event::Open(_) => {
                self.depth += 1;
                return;
            }
            Event::Close(token) => {
                self.depth = self.depth.saturating_sub(1);
                token.end()
            }
            Event::Unmade => {
                self.depth = self.depth.saturating_sub(1);
                return;
            }
            Event::Leaf(token) | Event::Word(token) => token.end(),
            Event::Key(_) | Event::NotKey(_) => return,
        };
        if let Some(key) = self.key.take_if(|_| self.depth == 1) {
            self.found.push((key, end));
        }
    }
}

/// A range to take out, in the joined bytes.
#[derive(Debug, Clone)]
struct Cut {
    range: Range<usize>,
    span: SpanAcross,
}

/// The cuts that take `drop` out of `content`, its tokens read with `res`
/// under `limits`, in order, once they're found safe to take out together.
fn checked(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    drop: &[SpanAcross],
) -> Result<Vec<Cut>, SpliceError> {
    let mut cuts = drop
        .iter()
        .map(|&span| {
            let range = content.offsets(span).ok_or(SpliceError::OutOfRange(span))?;
            Ok(Cut { range, span })
        })
        .collect::<Result<Vec<_>, SpliceError>>()?;
    cuts.retain(|c| !c.range.is_empty());
    cuts.sort_by_key(|c| c.range.start);
    if let Some(pair) = cuts
        .windows(2)
        .find(|pair| pair[1].range.start < pair[0].range.end)
    {
        return Err(SpliceError::Overlap(pair[1].span));
    }
    check(content, res, limits, &cuts)?;
    Ok(cuts)
}

fn overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// A running count of opens and closes: where it is, and the lowest it's
/// been.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Count {
    net: i32,
    min: i32,
}

impl Count {
    fn add(&mut self, by: i32) {
        self.net = self.net.saturating_add(by);
        self.min = self.min.min(self.net);
    }

    fn balanced(self) -> bool {
        self == Count::default()
    }
}

/// What a range taken out holds.
#[derive(Default)]
struct Tally {
    /// Whether it starts inside a text object.
    in_text: bool,
    q: Count,
    text: Count,
    /// Whether it begins or ends a text object.
    text_objects: bool,
    marked: Count,
    /// Whether it opens optional content.
    optional: bool,
    /// Whether it sets graphics state outside a `q`…`Q` of its own.
    sets_state: bool,
    /// Whether it moves a line.
    moves: bool,
    /// Whether it shows text.
    shows: bool,
}

impl Tally {
    /// `op`, and `tag` when it opens marked content.
    fn take(&mut self, op: &[u8], tag: Option<&[u8]>) {
        self.sets_state |= self.q.net == 0 && sets_state(op);
        self.optional |= tag.and_then(name).as_deref() == Some(b"OC");
        match op {
            b"q" => self.q.add(1),
            b"Q" => self.q.add(-1),
            b"BT" => {
                self.text.add(1);
                self.text_objects = true;
            }
            b"ET" => {
                self.text.add(-1);
                self.text_objects = true;
            }
            b"BMC" | b"BDC" => self.marked.add(1),
            b"EMC" => self.marked.add(-1),
            _ => {}
        }
        self.moves |= moves_text(op);
        self.shows |= matches!(op, b"Tj" | b"TJ" | b"'" | b"\"");
    }

    /// Whether taking it out leaves what's drawn after it as it was, but
    /// for text shown after it in its text object, and the marked content
    /// it opens or closes.
    fn safe(&self) -> bool {
        // Optional content left open would show or hide what's kept.
        let hides = self.optional && self.marked.net > 0;
        let kept = self.q.balanced() && !self.sets_state && !hides;
        if self.in_text {
            kept && !self.text_objects && !self.moves
        } else {
            kept && self.text.balanced()
        }
    }
}

/// Whether `op` sets graphics state, which lasts until a `Q` restores it:
/// the CTM, a color or color space, line style, rendering intent,
/// flatness, a `gs`, a clip, or text state.
fn sets_state(op: &[u8]) -> bool {
    matches!(
        op,
        b"cm"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"SCN"
            | b"sc"
            | b"scn"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"W"
            | b"W*"
            | b"Tc"
            | b"Tw"
            | b"Tz"
            | b"TL"
            | b"Tf"
            | b"Tr"
            | b"Ts"
            | b"\""
    )
}

/// Whether `op` moves to a line: sets the text and line matrices, which
/// a text object starts afresh.
fn moves_text(op: &[u8]) -> bool {
    matches!(op, b"Td" | b"TD" | b"Tm" | b"T*" | b"'" | b"\"")
}

/// Whether `op` starts or adds to a path, which the next painting
/// operator paints; and whether it ends one.
fn makes_path(op: &[u8]) -> bool {
    matches!(op, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")
}

fn ends_path(op: &[u8]) -> bool {
    matches!(
        op,
        b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n"
    )
}

/// Whether `cuts` may be taken out of `content`: each starts and ends
/// between steps, outside a comment and a path, and is safe ([`splice`]'s
/// rules).
fn check(content: &Concat, res: &ResCx, limits: &Limits, cuts: &[Cut]) -> Result<(), SpliceError> {
    let s = content.bytes();
    let mut in_text = false;
    let mut in_path = false;
    // The cut being read, and what it holds.
    let mut open: Option<(usize, Tally)> = None;
    // The next cut to start.
    let mut next = 0;
    // A cut that showed text in a text object that hasn't moved a line
    // since.
    let mut shown: Option<SpanAcross> = None;
    // Marked content cuts opened that no cut has closed yet, and the first
    // cut that left some open.
    let mut pending = 0;
    let mut opener = 0;
    // Where the last token ended, and whether it was an operand: a step
    // its operator hasn't ended; and the last two operands, where a tag
    // is.
    let mut after = 0;
    let mut operand = false;
    let mut operands: [Option<&[u8]>; 2] = [None, None];
    let unsafe_cut = |i: usize| SpliceError::Unsafe(cuts[i].span);
    let mut tokens = content.tokens(res, limits);
    loop {
        let token = tokens.next().transpose()?;
        let start = token.as_ref().map_or(s.len(), |(r, _)| r.start);
        // The cuts that end or start before this token, in the white-space
        // and comments since the last.
        let between = |at: usize| !in_comment(&s[after..at]);
        if let Some((i, tally)) = open.take_if(|(i, _)| cuts[*i].range.end <= start) {
            let closes_kept = pending + tally.marked.min < 0;
            if operand || in_path || closes_kept || !between(cuts[i].range.end) || !tally.safe() {
                return Err(unsafe_cut(i));
            }
            if pending == 0 && tally.marked.net > 0 {
                opener = i;
            }
            pending += tally.marked.net;
            if tally.in_text && tally.shows {
                shown = Some(cuts[i].span);
            }
        }
        while open.is_none() && next < cuts.len() && cuts[next].range.start <= start {
            let cut = &cuts[next];
            if operand || in_path || !between(cut.range.start) {
                return Err(unsafe_cut(next));
            }
            if cut.range.end <= start && !between(cut.range.end) {
                return Err(unsafe_cut(next));
            }
            if cut.range.end > start {
                open = Some((
                    next,
                    Tally {
                        in_text,
                        ..Tally::default()
                    },
                ));
            }
            next += 1;
        }
        let Some((r, tok)) = token else {
            return match pending {
                0 => Ok(()),
                _ => Err(unsafe_cut(opener)),
            };
        };
        // A token across either end of a cut.
        if let Some(i) = (open.as_ref().map(|(i, _)| *i)).filter(|&i| r.end > cuts[i].range.end) {
            return Err(unsafe_cut(i));
        }
        if next < cuts.len() && cuts[next].range.start < r.end {
            return Err(unsafe_cut(next));
        }
        after = r.end;
        operand = matches!(tok, Tok::Operand(_));
        let op = match tok {
            Tok::Op(op) => op,
            Tok::Operand(o) => {
                operands = [operands[1], Some(o)];
                continue;
            }
            Tok::InlineImage(_) => {
                operands = [None, None];
                continue;
            }
        };
        let tag = match op {
            b"BMC" => operands[1],
            b"BDC" => operands[0],
            _ => None,
        };
        operands = [None, None];
        match &mut open {
            Some((_, tally)) => tally.take(op, tag),
            None => {
                if pending > 0 && matches!(op, b"BMC" | b"BDC" | b"EMC") {
                    return Err(unsafe_cut(opener));
                }
                if let Some(span) = shown.filter(|_| matches!(op, b"Tj" | b"TJ")) {
                    return Err(SpliceError::Unsafe(span));
                }
                if moves_text(op) || matches!(op, b"BT" | b"ET") {
                    shown = None;
                }
            }
        }
        match op {
            b"BT" => in_text = true,
            b"ET" => in_text = false,
            op if makes_path(op) => in_path = true,
            op if ends_path(op) => in_path = false,
            _ => {}
        }
    }
}

/// Whether white-space and comments `gap` end inside a comment.
fn in_comment(gap: &[u8]) -> bool {
    gap.iter().fold(false, |comment, &c| match c {
        b'%' => true,
        b'\r' | b'\n' => false,
        _ => comment,
    })
}

/// `content`'s streams with `cuts` taken out and `rewrites` made, none of
/// them in a cut: a stream neither touches is borrowed as it is.
fn written<'a>(content: &'a Concat, cuts: &[Cut], rewrites: &[&Rewrite]) -> Vec<Cow<'a, [u8]>> {
    (0..content.stream_count())
        .map(|i| cut(content, i, cuts, rewrites))
        .collect()
}

/// Stream `i` of `content` with `cuts` taken out and `rewrites` made.
/// Whether a cut needs a space in its place is decided by the bytes kept
/// either side of it, as a neighbouring cut may take the bytes next to it
/// too, a space where neither is white-space by ISO 32000-2's rules; a
/// list written again starts and ends with a delimiter, so needs none.
fn cut<'a>(content: &'a Concat, i: usize, cuts: &[Cut], rewrites: &[&Rewrite]) -> Cow<'a, [u8]> {
    let s = content.bytes();
    let stream = content.range(i);
    let mut edits: Vec<(&Range<usize>, &[u8])> = (cuts.iter())
        .map(|c| (&c.range, &[][..]))
        .chain(rewrites.iter().map(|r| (&r.range, &r.with[..])))
        .filter(|(r, _)| r.start < stream.end && r.end > stream.start)
        .collect();
    edits.sort_by_key(|(r, _)| r.start);
    let mut out: Option<Vec<u8>> = None;
    let mut at = stream.start;
    let mut edits = edits.into_iter().peekable();
    while let Some((range, with)) = edits.next() {
        let out = out.get_or_insert_with(|| Vec::with_capacity(stream.len()));
        out.extend_from_slice(&s[at..range.start.max(stream.start)]);
        out.extend_from_slice(with);
        at = range.end.min(stream.end);
        // A list written again needs no space; when the next byte is taken
        // too, the cut after it decides.
        if !with.is_empty() || edits.peek().is_some_and(|(r, _)| r.start == at) {
            continue;
        }
        if at < stream.end
            && out.last().is_some_and(|&c| !is_white_space(c))
            && !is_white_space(s[at])
        {
            out.push(b' ');
        }
    }
    match out {
        None => Cow::Borrowed(&s[stream]),
        Some(mut out) => {
            out.extend_from_slice(&s[at..stream.end]);
            Cow::Owned(out)
        }
    }
}

#[cfg(test)]
mod tests;
