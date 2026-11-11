//! Splicing (ISO 32000-2, 7.8.2): a page's streams with ranges taken out,
//! every other byte as it was, never read into operations and written
//! again; and a marked range's property list written again without some
//! of its keys (14.6.2). [`splice`] has the rules.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use pdf_tokenizer::is_white_space;

use crate::error::Error;
use crate::inline::{Found, ImageStream, InlineImage, ResCx};
use crate::limits::Limits;
use crate::operand::{Operand, name, object};
use crate::page::{Concat, SpanAcross};
use crate::phases::phase;
use crate::scan::{Event, Raw, TOP, Visit, next_raw, walk};
use crate::tokens::{Tok, Tokens};
use crate::unprinted::Len;

/// Why a splice, or a property list written again, was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum SpliceError {
    /// The content can't be read under the caps given: its error, at an
    /// offset into the joined bytes.
    #[error(transparent)]
    Read(#[from] Error),
    /// A range that isn't in the content: an end past its stream's end or
    /// past the last stream, or before its start; or what's taken out, or
    /// a list written again, found in a content of another length
    /// ([`Spliced::streams`]).
    #[error("{0} is outside the content")]
    OutOfRange(SpanAcross),
    /// A range that overlaps another taken out with it; or a marked range
    /// whose property list written again another written again with it
    /// overlaps ([`Spliced::streams`]).
    #[error("{0} overlaps another range taken out, or another list written again")]
    Overlap(SpanAcross),
    /// A range whose taking out could change what's drawn after it, by
    /// [`splice`]'s rules, or which leaves what's kept read otherwise.
    #[error("taking out {0} would change what's drawn after it")]
    Unsafe(SpanAcross),
    /// A marked range whose property list can't be written again without
    /// the keys asked, as [`without_keys`] says, or whose writing again
    /// leaves the content read otherwise, alone or with what's made with it
    /// ([`Spliced::streams`]).
    #[error("the property list of the range at {0} can't be written again without the keys asked")]
    List(SpanAcross),
    /// Content that holds an inline image whose size was counted past a
    /// cap ([`InlineImage::capped`](crate::InlineImage::capped)), so that
    /// where it ends, and so what's read after it, may not be where PDFium
    /// has it: the image, from its `BI` to its `EI`. A caller raises the
    /// caps that sized it and tries again.
    #[error("the inline image at {0} is sized by a color space counted past a cap")]
    Capped(SpanAcross),
    /// [`splice_each`]'s checks would read more than
    /// [`Limits::splice_reads`] bytes.
    #[error("checking what to take out would read more than {cap} bytes")]
    Reads {
        /// The cap: the most the checks may read.
        cap: u64,
    },
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
///   the text matrix on (9.4.3), which outlasts its text object, so text
///   shown after it, before a line is moved or a text object begins, would
///   move back: that refuses it too;
/// - PDFium shows text and moves lines outside a text object too, where
///   ISO 32000-2 has neither (9.4.1), the text matrices as the last text
///   object left them: so a range outside one that shows text, moves a
///   line or holds a text object, again even inside a `q`…`Q` of its own,
///   is refused where text is shown after it outside a text object, or,
///   where it moves a line or holds a text object, a line is moved there,
///   before a text object begins or `Tm` sets the matrices;
/// - text shown while the text rendering mode adds it to the clip (modes
///   4 to 7) goes into the clip at the end of its text object (9.3.6),
///   which lasts past it: so it shows no text while the mode may, but in a
///   text object of its own that ends inside a `q`…`Q` of its own. The
///   mode is followed through the content as `Tr` sets it, and `q` and `Q`
///   save and restore it; one whose operand PDFium reads as no mode, or
///   has none, may;
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
/// The tokens are read as [`tokens`](fn@crate::tokens) reads them, by
/// PDFium's rules, so a range is judged by where those end each token, an
/// inline image's among them, by the reading `limits` names: its DCT and
/// CCITTFax data's end by the stand-in, or decoded
/// ([`Limits::coded_ends`]). A splice judged by one reading holds for that
/// one: a caller that wants what PDFium draws asks for the decoders'
/// (`CodedEnds::Decoded`, with the `image-ends` feature). An image's size
/// is counted from its color space, which, counted past a cap, a name
/// [`ResCx::capped`](crate::ResCx::capped) names, or an array in its
/// dictionary past [`Limits::space_values`] or a cap of [`Limits::color`],
/// counts as one that doesn't load, with no error to a reader, where
/// PDFium's count may be another: content that holds such an image is
/// refused ([`SpliceError::Capped`]), as where it ends can't be sure; a
/// space capped that no image is drawn in refuses nothing. Content that
/// can't be read under `limits` is refused with its error
/// ([`SpliceError::Read`]).
///
/// What's kept must read as it did, as how a token is read may depend on
/// what's after it: a `BI` with a keyword where its dictionary's key
/// should be begins no image, and its would-be dictionary is read as
/// content, up to that keyword; and an inline image's data's end may be
/// found by what's past it, or past the image (an ASCIIHex image's `>`,
/// with no `EI` after it, makes PDFium keep no stream of it and read it on
/// to its first `EI`; Flate data whose header asks for a preset
/// dictionary is ended by the bytes naming it; a decoder of DCT or
/// CCITTFax data may read on past `EI`). So what a splice gives is read
/// again under `limits`, and it's refused ([`SpliceError::Unsafe`], naming
/// the first range at or after the first token kept that reads otherwise,
/// or the last) unless every token kept reads as it did: its kind and
/// bytes, and an inline image's data and what PDFium keeps of it in the
/// same places in it. It costs two readings, however many ranges it's
/// given: of `content`, and of what it gives. A caller that splices again
/// and again bounds how often; [`splice_each`] bounds its own checks
/// ([`Limits::splice_reads`]).
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
///
/// # Errors
///
/// [`SpliceError::Unsafe`] for a range whose taking out could change what's
/// drawn after it, or leaves what's kept read otherwise;
/// [`SpliceError::Overlap`] and [`SpliceError::OutOfRange`] for ranges that
/// overlap or aren't in the content; [`SpliceError::Capped`] for content that
/// holds an image a cap sized; and [`SpliceError::Read`] for content that can't
/// be read under `limits`.
pub fn splice<'a>(
    content: &'a Concat,
    res: &ResCx,
    limits: &Limits,
    drop: &[SpanAcross],
) -> Result<Vec<Cow<'a, [u8]>>, SpliceError> {
    phase!(
        DEBUG,
        "splice",
        len = content.bytes().len(),
        ranges = drop.len(),
    );
    Ok(checked(content, res, limits, drop)?.1)
}

/// What [`splice_each`] takes out of a page's content: of that content, as
/// what it holds are places in its joined bytes. [`Spliced::default`]
/// takes nothing out, for lists written again alone.
#[derive(Debug, Clone, Default)]
pub struct Spliced {
    /// Whether each group was taken out.
    pub taken: Vec<bool>,
    /// The cuts of those taken, in order.
    cuts: Vec<Cut>,
    /// The length of the content's joined bytes.
    of: usize,
}

impl Spliced {
    /// Whether where `span` starts in `content` is taken out.
    #[must_use]
    pub fn takes(&self, content: &Concat, span: SpanAcross) -> bool {
        let at = content.offset(span.start);
        (self.cuts.iter()).any(|c| at.is_some_and(|at| c.range.contains(&at)))
    }

    /// `content`'s streams with what's taken out taken out, and `rewrites`
    /// made in what's kept, `content` the one [`splice_each`] and
    /// [`without_keys`] were given (another of another length is refused,
    /// [`SpliceError::OutOfRange`]): a rewrite in what's taken out goes
    /// with it, and one given twice is made once. None when no stream
    /// changes.
    ///
    /// What's taken out was read again as [`splice_each`] took it, and
    /// each rewrite as [`without_keys`] made it, alone. Together they may
    /// read otherwise: an ASCII85 image's data ended by a `~` in one list
    /// and a comment in another may run on, both written again, over what
    /// was drawn after them. So with rewrites, what this gives is read
    /// again, its tokens read with `res` under `limits`, as [`splice`]
    /// reads what it gives, and it's refused unless every token kept reads
    /// as it did, a list written again as it's now written: refused
    /// ([`SpliceError::Unsafe`] for a range taken out,
    /// [`SpliceError::List`] for a list) naming the first at or after the
    /// first token kept that reads otherwise, or the last. Two rewrites of
    /// one list are refused ([`SpliceError::Overlap`]): its keys are taken
    /// out in one [`without_keys`]. A caller refused may make what's taken
    /// out alone, or each rewrite alone. Content that can't be read under
    /// `limits` is refused with its error ([`SpliceError::Read`]), and, with
    /// rewrites, content that holds an image sized by a count past a cap
    /// ([`SpliceError::Capped`]). With rewrites, it costs two readings: of
    /// `content`, and of what it gives.
    ///
    /// # Errors
    ///
    /// [`SpliceError::OutOfRange`] for another content than the one given
    /// before; [`SpliceError::Overlap`] for two rewrites of one list; and, with
    /// rewrites, [`SpliceError::Unsafe`] and [`SpliceError::List`] where what
    /// it gives reads otherwise, [`SpliceError::Capped`] and
    /// [`SpliceError::Read`], as above.
    pub fn streams<'a>(
        &self,
        content: &'a Concat,
        res: &ResCx,
        limits: &Limits,
        rewrites: &[Rewrite],
    ) -> Result<Option<Vec<Cow<'a, [u8]>>>, SpliceError> {
        phase!(
            DEBUG,
            "streams",
            len = content.bytes().len(),
            cuts = self.cuts.len(),
            rewrites = rewrites.len(),
        );
        let of = content.bytes().len();
        if let Some(cut) = self.cuts.first().filter(|_| self.of != of) {
            return Err(SpliceError::OutOfRange(cut.span));
        }
        if let Some(list) = rewrites.iter().find(|r| r.of != of) {
            return Err(SpliceError::OutOfRange(list.span));
        }
        let mut lists: Vec<&Rewrite> = (rewrites.iter())
            .filter(|r| !(self.cuts.iter()).any(|c| overlap(&c.range, &r.range)))
            .collect();
        lists.sort_by_key(|r| r.range.start);
        lists.dedup_by(|a, b| a.range == b.range && a.with == b.with);
        if let Some(pair) = (lists.windows(2)).find(|pair| overlap(&pair[0].range, &pair[1].range))
        {
            return Err(SpliceError::Overlap(pair[1].span));
        }
        if lists.is_empty() {
            return Ok((!self.cuts.is_empty()).then(|| written(content, &self.cuts, &[])));
        }
        make(content, res, limits, &self.cuts, &lists).map(Some)
    }
}

/// `content`'s streams with as many of `groups` taken out as can be, its
/// tokens read with `res` under `limits`: each group the ranges that take
/// one thing out, all of them where [`splice`] takes them together; else
/// those it takes alone, together, or none where it refuses those
/// together. A group it refuses stays as it is ([`Spliced::taken`]).
/// Content that can't be read under `limits` is refused with its error
/// ([`SpliceError::Read`]), and content that holds an image sized by a
/// count past a cap likewise ([`SpliceError::Capped`]), where a check reads
/// as far as it, as every check reads the content alike, never taken for
/// groups refused.
///
/// It costs a check of all of them together, and where that's refused,
/// one for each group and one for those taken, each two readings
/// ([`splice`]): of the content, and of what it keeps. Each is counted, by
/// those readings' bytes, before it's made, and past
/// [`Limits::splice_reads`] in all it's refused ([`SpliceError::Reads`]),
/// never a group left in unchecked.
///
/// # Errors
///
/// [`SpliceError::Read`], [`SpliceError::Capped`] and [`SpliceError::Reads`],
/// each the call's: a group refused otherwise stays, and is no error.
pub fn splice_each(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    groups: &[Vec<SpanAcross>],
) -> Result<Spliced, SpliceError> {
    phase!(
        DEBUG,
        "splice_each",
        len = content.bytes().len(),
        groups = groups.len(),
    );
    each(content, res, limits, groups, &mut 0)
}

/// [`splice_each`], what its checks read added to `spent`, which the tests
/// and the fuzz body hold its budget to.
pub(crate) fn each(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    groups: &[Vec<SpanAcross>],
    spent: &mut u64,
) -> Result<Spliced, SpliceError> {
    // The cuts of `drop` where it's taken out, none where it's refused: a
    // range outside the content, ranges that overlap, or ones it may not
    // take out. What a check reads is counted before it's made, and the
    // content's error where it can't be read, an image a cap sized and the
    // budget passed are the call's, never a group's.
    let mut check = |drop: &[SpanAcross]| -> Result<Option<Vec<Cut>>, SpliceError> {
        phase!(TRACE, "check", ranges = drop.len());
        let cuts = match cuts(content, drop) {
            Ok(cuts) => cuts,
            Err(SpliceError::OutOfRange(_) | SpliceError::Overlap(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let kept = written(content, &cuts, &[]);
        *spent = spent.saturating_add(reads(content, &cuts, &kept));
        if *spent > limits.splice_reads {
            return Err(SpliceError::Reads {
                cap: limits.splice_reads,
            });
        }
        match safe(content, res, limits, &cuts, &[], &kept) {
            Ok(()) => Ok(Some(cuts)),
            Err(
                SpliceError::Unsafe(_)
                | SpliceError::OutOfRange(_)
                | SpliceError::Overlap(_)
                | SpliceError::List(_),
            ) => Ok(None),
            Err(
                e @ (SpliceError::Read(_) | SpliceError::Capped(_) | SpliceError::Reads { .. }),
            ) => Err(e),
        }
    };
    let of = content.bytes().len();
    if groups.is_empty() {
        return Ok(Spliced::default());
    }
    if let Some(cuts) = check(&groups.concat())? {
        let taken = vec![true; groups.len()];
        return Ok(Spliced { taken, cuts, of });
    }
    let taken = (groups.iter())
        .map(|cuts| Ok(check(cuts)?.is_some()))
        .collect::<Result<Vec<bool>, SpliceError>>()?;
    let cuts: Vec<SpanAcross> = (groups.iter().zip(&taken))
        .filter(|(_, taken)| **taken)
        .flat_map(|(cuts, _)| cuts.iter().copied())
        .collect();
    if cuts.is_empty() {
        return Ok(Spliced {
            taken,
            cuts: Vec::new(),
            of,
        });
    }
    Ok(match check(&cuts)? {
        Some(cuts) => Spliced { taken, cuts, of },
        None => Spliced {
            taken: vec![false; groups.len()],
            cuts: Vec::new(),
            of,
        },
    })
}

/// A marked range's property list written again ([`without_keys`]):
/// the range, where its list is in the joined bytes of the content it was
/// read from, and what it's now. [`Spliced::streams`] makes it in that
/// content's streams.
#[derive(Clone, PartialEq, Eq)]
pub struct Rewrite {
    span: SpanAcross,
    pub(crate) range: Range<usize>,
    with: Vec<u8>,
    /// The length of the content's joined bytes.
    of: usize,
}

impl fmt::Debug for Rewrite {
    /// Where its list is, and what it's now by its length; not the list,
    /// whose strings may be a page's text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rewrite")
            .field("span", &self.span)
            .field("range", &self.range)
            .field("with", &Len(self.with.len()))
            .field("of", &self.of)
            .finish()
    }
}

/// The property list of the `BDC` that opens `span` in `content` (14.6.2),
/// written again without the entries whose keys `keys` names: each entry
/// goes with the spaces and tabs before it on its line, and every other
/// byte stays as written, comments, line ends and what the dictionaries
/// it holds hold among them. None when it has none of them.
///
/// The `BDC`'s operands are the content's tokens, read from its start as
/// [`tokens`](fn@crate::tokens) reads them, with `res` under `limits`, from
/// the first at `span`'s start to the `BDC`, its list the last of them, so
/// the list is a token of the page's, never bytes inside an image's data
/// or a string; the list's entries are read as PDFium reads a dictionary,
/// so arrays nest in it, and its keys decoded as [`name`] decodes them.
/// Refused ([`SpliceError::List`]) when the range doesn't
/// open with a `BDC` whose list is written in the content, in one stream,
/// and reads as a dictionary ([`object`]); when what's left
/// of it isn't one operand that reads as the list without those entries
/// (every entry of a key written twice goes); or when the content with it
/// written again doesn't read as it did but for the list, read as one
/// operand, the list written again, as [`splice`] reads what it gives.
/// Content that can't be read to the `BDC` is refused with its error
/// ([`SpliceError::Read`]). Where it writes the list again, it reads the
/// content whole, and refuses content past the `BDC` that can't be read
/// alike, and content that holds an image sized by a count past a cap
/// ([`SpliceError::Capped`]). A list that holds none of the keys is
/// `None`, whatever follows it, as nothing is written. It costs a reading
/// of the content to the `BDC`, and, where it writes the list again, two
/// more: of the content and of what it gives, three at most, whatever keys
/// it's given; a caller writing many lists again bounds how many. What it
/// gives is made in `content`'s streams by [`Spliced::streams`], with
/// what's taken out or with nothing.
///
/// ```
/// use pdf_content_streams::{Concat, Limits, Properties, ResCx, Spliced, marked, without_keys};
///
/// let (res, limits, props) = (ResCx::none(), Limits::DEFAULT, Properties::default());
/// let page = Concat::new(&[b"/Artifact <</Subtype /Watermark /Tool_Gen 1>> BDC q 0 g Q EMC"]);
/// let span = marked(&page, &props, &res, &limits).unwrap()[0].span;
/// let keys: [&[u8]; 1] = [b"Tool_Gen"];
/// let rewrite = without_keys(&page, &res, &limits, span, &keys).unwrap().unwrap();
/// // Made with nothing taken out.
/// let nothing = Spliced::default();
/// let made = nothing.streams(&page, &res, &limits, &[rewrite]).unwrap().unwrap();
/// assert_eq!(&*made[0], b"/Artifact <</Subtype /Watermark>> BDC q 0 g Q EMC");
/// ```
///
/// # Errors
///
/// [`SpliceError::List`] where the list can't be written again without the
/// keys, as above; [`SpliceError::OutOfRange`] where `span` isn't in the
/// content; [`SpliceError::Read`] where the content can't be read to the
/// `BDC`; and, where it writes the list again, [`SpliceError::Capped`] and
/// [`SpliceError::Read`] as [`splice`] gives them.
pub fn without_keys(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    span: SpanAcross,
    keys: &[&[u8]],
) -> Result<Option<Rewrite>, SpliceError> {
    phase!(
        DEBUG,
        "without_keys",
        len = content.bytes().len(),
        keys = keys.len(),
    );
    let range = content.offsets(span).ok_or(SpliceError::OutOfRange(span))?;
    let refused = || SpliceError::List(span);
    let s = content.bytes();
    // Its operands, the list last, to its `BDC`: the content's tokens from
    // its start, the first at the range's start or after it, inside it.
    let mut list = None;
    reading(s.len());
    let mut read = content.tokens(res, limits);
    loop {
        match read.next().transpose()? {
            Some((r, _)) if r.end <= range.start => {}
            Some((r, _)) if r.start < range.start || r.end > range.end => return Err(refused()),
            Some((r, Tok::Operand(_))) => list = Some(r),
            Some((_, Tok::Op(b"BDC"))) => break,
            _ => return Err(refused()),
        }
    }
    let list = list
        .filter(|r| s[r.clone()].starts_with(b"<<"))
        .ok_or_else(refused)?;
    let at = content.span(list.clone());
    if at.start.stream != at.end.stream {
        return Err(refused());
    }
    let with = without_entries(&s[list.clone()], keys, limits).ok_or_else(refused)?;
    if with == s[list.clone()] {
        return Ok(None);
    }
    let rewrite = Rewrite {
        span,
        range: list,
        with,
        of: s.len(),
    };
    // The content with it written again, read again beside the content:
    // each token as it was, the list as it's written.
    make(content, res, limits, &[], &[&rewrite])?;
    Ok(Some(rewrite))
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

/// A page's streams, each borrowed where nothing changes it.
type Streams<'a> = Vec<Cow<'a, [u8]>>;

/// A range to take out, in the joined bytes.
#[derive(Debug, Clone)]
struct Cut {
    range: Range<usize>,
    span: SpanAcross,
}

/// The cuts that take `drop` out of `content`, its tokens read with `res`
/// under `limits`, in order, once they're found safe to take out together;
/// and the streams they give, read again as the tokens they keep.
fn checked<'a>(
    content: &'a Concat,
    res: &ResCx,
    limits: &Limits,
    drop: &[SpanAcross],
) -> Result<(Vec<Cut>, Streams<'a>), SpliceError> {
    let cuts = cuts(content, drop)?;
    let kept = make(content, res, limits, &cuts, &[])?;
    Ok((cuts, kept))
}

/// The cuts that take `drop` out of `content`, in order, but none that's
/// empty: refused where one isn't in it, or two overlap. Nothing is read.
fn cuts(content: &Concat, drop: &[SpanAcross]) -> Result<Vec<Cut>, SpliceError> {
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
    Ok(cuts)
}

/// `content`'s streams with `cuts` taken out and `lists` written again,
/// apart and in order, its tokens read with `res` under `limits`, once
/// they're found safe to make together ([`safe`]).
fn make<'a>(
    content: &'a Concat,
    res: &ResCx,
    limits: &Limits,
    cuts: &[Cut],
    lists: &[&Rewrite],
) -> Result<Streams<'a>, SpliceError> {
    let kept = written(content, cuts, lists);
    safe(content, res, limits, cuts, lists, &kept)?;
    Ok(kept)
}

/// Whether `kept`, `content`'s streams with `cuts` taken out and `lists`
/// written again, its tokens read with `res` under `limits`, may be made:
/// each cut by [`splice`]'s rules, and what they give read again as the
/// tokens they keep, each list as it's now written. Two readings, or, with
/// neither, one, as [`reads`] counts them.
fn safe(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    cuts: &[Cut],
    lists: &[&Rewrite],
    kept: &[Cow<[u8]>],
) -> Result<(), SpliceError> {
    if cuts.is_empty() && lists.is_empty() {
        return check(content, res, limits, cuts, lists, None);
    }
    let streams: Vec<&[u8]> = kept.iter().map(|s| &**s).collect();
    let again = Concat::new(&streams);
    reading(again.bytes().len());
    let again = Some(Again(again.tokens(res, limits)));
    check(content, res, limits, cuts, lists, again)
}

/// What checking `cuts`, which give `kept`, reads of `content`, in bytes:
/// the content, and, where there are cuts, what they keep, joined as
/// [`Concat`] joins streams.
fn reads(content: &Concat, cuts: &[Cut], kept: &[Cow<[u8]>]) -> u64 {
    let read = content.bytes().len() as u64;
    if cuts.is_empty() {
        return read;
    }
    let again = (kept.iter()).fold(kept.len().saturating_sub(1) as u64, |sum, stream| {
        sum.saturating_add(stream.len() as u64)
    });
    read.saturating_add(again)
}

/// A reading of `bytes` of content begun, by a splice or a list written
/// again: told under test, which holds what's read to its caps.
#[inline]
fn reading(bytes: usize) {
    #[cfg(test)]
    READINGS.with(|readings| readings.borrow_mut().push(bytes));
    #[cfg(not(test))]
    let _ = bytes;
}

#[cfg(test)]
thread_local! {
    /// The readings of content a splice or a list written again has begun
    /// on this thread, by their bytes, in order ([`reading`]).
    pub(crate) static READINGS: std::cell::RefCell<Vec<usize>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// What a splice or a list written again gives, its tokens read again as
/// the content it's made from was, to be held to what it keeps of them.
struct Again<'s, 'r>(Tokens<'s, 'r>);

impl Again<'_, '_> {
    /// Whether the next token read again is `tok`, at `at` in `s`, read as
    /// it was there: its kind and bytes, and an inline image's data and
    /// what PDFium keeps of it in the same places in it, its data's end
    /// found as `found` says.
    fn next_is(&mut self, s: &[u8], at: Range<usize>, tok: &Tok, found: Found) -> bool {
        let Some(Ok((again, read))) = self.0.next() else {
            return false;
        };
        let content = self.0.content();
        match (tok, read) {
            (Tok::Op(a), Tok::Op(b)) | (Tok::Operand(a), Tok::Operand(b)) => *a == b,
            (Tok::InlineImage(was), Tok::InlineImage(data)) => {
                s[at.clone()] == content[again.clone()]
                    && placed(content, again, data, self.0.found())
                        == placed(s, at, was.clone(), found)
            }
            _ => false,
        }
    }

    /// Whether it's read to its end: no token is left, and no error.
    fn done(&mut self) -> bool {
        self.0.next().is_none()
    }
}

/// An inline image at `at` in `s`, its data at `data`, found as `found`
/// says: its data and the stream PDFium keeps of it, each from its start.
fn placed(
    s: &[u8],
    at: Range<usize>,
    data: Range<usize>,
    found: Found,
) -> (Range<usize>, ImageStream) {
    let inside = |r: Range<usize>| r.start - at.start..r.end - at.start;
    let stream = match InlineImage::new(s, at.clone(), data.clone(), found, false).stream() {
        ImageStream::Data(kept) => ImageStream::Data(inside(kept)),
        other => other,
    };
    (inside(data), stream)
}

fn overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The cut or list written again to name where what's kept reads
/// otherwise from `at` on: the first at or after it, as a token's reading
/// looks on, or the last.
fn blame(cuts: &[Cut], lists: &[&Rewrite], at: usize) -> Option<SpliceError> {
    let edits = || {
        let cuts = cuts
            .iter()
            .map(|c| (c.range.start, SpliceError::Unsafe(c.span)));
        cuts.chain((lists.iter()).map(|l| (l.range.start, SpliceError::List(l.span))))
    };
    let first = edits()
        .filter(|(start, _)| *start >= at)
        .min_by_key(|(start, _)| *start);
    let last = || edits().max_by_key(|(start, _)| *start);
    first.or_else(last).map(|(_, refused)| refused)
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
    /// Whether text it shows may go into a clip that lasts past it.
    clips: bool,
    /// Whether text it shows in a text object of its own may go into the
    /// clip at that one's end, still to come.
    clip_text: bool,
}

impl Tally {
    /// `op`, `tag` when it opens marked content, and whether text shown
    /// now may go into the clip.
    fn take(&mut self, op: &[u8], tag: Option<&[u8]>, clipping: bool) {
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
                // The clip it sets lasts past the range but inside a
                // `q`…`Q` of its own.
                self.clips |= self.clip_text && self.q.net <= 0;
                self.clip_text = false;
            }
            b"BMC" | b"BDC" => self.marked.add(1),
            b"EMC" => self.marked.add(-1),
            _ => {}
        }
        self.moves |= moves_text(op);
        let shows = matches!(op, b"Tj" | b"TJ" | b"'" | b"\"");
        self.shows |= shows;
        if shows && clipping {
            // The text object it starts in ends after it.
            match self.in_text {
                true => self.clips = true,
                false => self.clip_text = true,
            }
        }
    }

    /// Whether taking it out leaves what's drawn after it as it was, but
    /// for text shown or lines moved after it, and the marked content it
    /// opens or closes.
    fn safe(&self) -> bool {
        // Optional content left open would show or hide what's kept.
        let hides = self.optional && self.marked.net > 0;
        let clips = self.clips || self.clip_text;
        let kept = self.q.balanced() && !self.sets_state && !hides && !clips;
        if self.in_text {
            kept && !self.text_objects && !self.moves
        } else {
            kept && self.text.balanced()
        }
    }
}

/// Whether text shown may go into the clip, by the text rendering mode
/// (9.3.6), as `Tr` sets it and `q` and `Q` save and restore it: each
/// setting, and how many `q`s deep it was made.
#[derive(Default)]
struct Modes {
    depth: usize,
    set: Vec<(usize, bool)>,
}

impl Modes {
    fn clipping(&self) -> bool {
        self.set.last().is_some_and(|&(_, clipping)| clipping)
    }

    fn save(&mut self) {
        self.depth = self.depth.saturating_add(1);
    }

    /// A `Q` with nothing saved restores nothing.
    fn restore(&mut self) {
        if self.depth == 0 {
            return;
        }
        self.depth -= 1;
        while self.set.last().is_some_and(|&(at, _)| at > self.depth) {
            self.set.pop();
        }
    }

    fn set(&mut self, clipping: bool) {
        match self.set.last_mut() {
            Some((at, was)) if *at == self.depth => *was = clipping,
            _ => self.set.push((self.depth, clipping)),
        }
    }
}

/// Whether `Tr` with `operand`, its last, may set a mode that puts text
/// shown into the clip (4 to 7): any but a number whose whole part is 0 to
/// 3, as PDFium reads an integer operand ([`Operand::number`]); none, or a
/// number PDFium reads as no mode, may.
fn clipping(operand: Option<&[u8]>) -> bool {
    let n = operand.map_or(f32::NAN, |o| Operand::new(o, 0..o.len()).number());
    !(n > -1.0 && n < 4.0)
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
/// rules); and, where `again` is what they give with `lists` written
/// again, each token kept reads there as it did, a list as it's now
/// written.
fn check(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    cuts: &[Cut],
    lists: &[&Rewrite],
    mut again: Option<Again>,
) -> Result<(), SpliceError> {
    let s = content.bytes();
    let mut in_text = false;
    let mut in_path = false;
    // The cut being read, and what it holds.
    let mut open: Option<(usize, Tally)> = None;
    // The next cut to start, and the next list written again to read.
    let mut next = 0;
    let mut list = 0;
    // A cut that showed text, so moved the text matrix, which nothing kept
    // has set since; and one outside a text object that moved a line or
    // began a text object, so set the text line matrix too.
    let mut shown: Option<SpanAcross> = None;
    let mut moved: Option<SpanAcross> = None;
    // Whether text shown may go into the clip.
    let mut modes = Modes::default();
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
    reading(s.len());
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
            if tally.shows {
                shown = Some(cuts[i].span);
            }
            if !tally.in_text && (tally.moves || tally.text_objects) {
                moved = Some(cuts[i].span);
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
            if pending != 0 {
                return Err(unsafe_cut(opener));
            }
            if again.as_mut().is_some_and(|again| !again.done()) {
                let last = content.span(s.len()..s.len());
                return Err(blame(cuts, lists, s.len()).unwrap_or(SpliceError::Unsafe(last)));
            }
            return Ok(());
        };
        // An image whose size was counted past a cap, wherever it is: what
        // it ends, and so where what's after it is read from, can't be sure.
        if matches!(tok, Tok::InlineImage(_)) && tokens.capped() {
            return Err(SpliceError::Capped(content.span(r)));
        }
        // A token across either end of a cut.
        if let Some(i) = (open.as_ref().map(|(i, _)| *i)).filter(|&i| r.end > cuts[i].range.end) {
            return Err(unsafe_cut(i));
        }
        if next < cuts.len() && cuts[next].range.start < r.end {
            return Err(unsafe_cut(next));
        }
        // A token kept, read again, a list as it's now written: the first
        // cut or list at or after it, or the last, is named where it reads
        // otherwise, as a token's reading looks on.
        if let Some(again) = again.as_mut().filter(|_| open.is_none()) {
            while lists.get(list).is_some_and(|l| l.range.start < r.start) {
                list += 1;
            }
            let same = match lists.get(list).filter(|l| l.range == r) {
                Some(l) => {
                    let now = Tok::Operand(&l.with);
                    again.next_is(&l.with, 0..l.with.len(), &now, Found::Walk)
                }
                None => again.next_is(s, r.clone(), &tok, tokens.found()),
            };
            if !same {
                let token = SpliceError::Unsafe(content.span(r.clone()));
                return Err(blame(cuts, lists, r.start).unwrap_or(token));
            }
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
        let last = operands[1];
        operands = [None, None];
        match &mut open {
            Some((_, tally)) => tally.take(op, tag, modes.clipping()),
            None => {
                if pending > 0 && matches!(op, b"BMC" | b"BDC" | b"EMC") {
                    return Err(unsafe_cut(opener));
                }
                // Text shown from the text matrix a cut left, or a line
                // moved from the line matrix one left (9.4.2, 9.4.3); a
                // line moved sets the text matrix, a text object begun or
                // `Tm` both.
                let shows = matches!(op, b"Tj" | b"TJ");
                let from_line = matches!(op, b"Td" | b"TD" | b"T*" | b"'" | b"\"");
                if let Some(span) = moved.filter(|_| shows || from_line) {
                    return Err(SpliceError::Unsafe(span));
                }
                if let Some(span) = shown.filter(|_| shows) {
                    return Err(SpliceError::Unsafe(span));
                }
                if from_line {
                    shown = None;
                }
                if matches!(op, b"BT" | b"Tm") {
                    (shown, moved) = (None, None);
                }
            }
        }
        match op {
            b"BT" => in_text = true,
            b"ET" => in_text = false,
            b"q" => modes.save(),
            b"Q" => modes.restore(),
            b"Tr" => modes.set(clipping(last)),
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
fn written<'a>(content: &'a Concat, cuts: &[Cut], rewrites: &[&Rewrite]) -> Streams<'a> {
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
