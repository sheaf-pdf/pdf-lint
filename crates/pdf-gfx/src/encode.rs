//! Position-independent content-stream and Form XObject re-encoding.
//!
//! [`encode`] walks a page's [`Concat`] content and every Form XObject it draws
//! via [`interpret`], writing each stream out through [`ContentWriter`] while
//! rewriting relative line-moving text operators (`Td`, `TD`, `T*`, `'`, `"`)
//! into explicit `Tm` + `Td` pairs computed from the interpreted [`State`]:
//!
//! - `Td`, `TD`, and `T*` become `<matrix> Tm` followed (when `line != (0, 0)`)
//!   by `<line.x> <line.y> Td`;
//! - `'` becomes `<matrix> Tm` (+ `<line> Td`) + `<string> Tj`;
//! - `"` becomes `<tw> Tw` + `<tc> Tc` + `<matrix> Tm` (+ `<line> Td`) +
//!   `<string> Tj`.
//!
//! Every line's placement is then self-contained—independent of `TL` or earlier
//! lines—while preserving the exact `f32` matrix + offset sums PDFium performs
//! when placing glyphs, so re-interpreting the encoded output reproduces every
//! glyph, path, image, shading, clip, and mark bit-for-bit.

use std::collections::HashMap;
use std::fmt;
use std::ops::{ControlFlow, Range};

use pdf_content_streams::{
    Concat, ContentWriter, ImageStream, InlineImage, Operator, Operators, ResCx, Reserved,
    operations,
};
use pdf_fonts::Fonts;
use pdf_objects::fault::Sink;
use pdf_objects::{Object, ObjectId, Resolve, Streams, serialize};

use crate::error::{Error, Unwritable};
use crate::fault::Fault;
use crate::geom::{Matrix, Point};
use crate::interp::{At, Event, Forms, Kind, Opts, Wants, interpret_with};
use crate::limits::{Cap, Limits};
use crate::phases::phase;
use crate::state::nums;

/// Re-encoded page content stream and Form XObject streams returned by
/// [`encode`] and [`encode_with`].
///
/// Its `Debug` implementation reports byte lengths rather than raw content
/// stream bytes.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Encoded {
    /// Re-encoded page content stream (all page `/Contents` streams joined as
    /// one).
    pub content: Vec<u8>,
    /// Re-encoded content bytes of each distinct Form XObject entered on the
    /// page, in the order first drawn. Form XObjects that were only ever
    /// refused are omitted.
    pub forms: Vec<(ObjectId, Vec<u8>)>,
}

impl fmt::Debug for Encoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let form_lens: Vec<(ObjectId, usize)> =
            self.forms.iter().map(|(id, b)| (*id, b.len())).collect();
        f.debug_struct("Encoded")
            .field("content_len", &self.content.len())
            .field("forms", &form_lens)
            .finish()
    }
}

/// Re-encodes `content` and every Form XObject drawn on it without collecting
/// non-fatal [`Fault`]s.
///
/// # Errors
///
/// Returns [`Error::PageStream`] if `content.undecoded()` is non-empty,
/// [`Error::CappedInlineImage`] if an inline image was sized under a
/// color-space cap, [`Error::Unwritable`] if a line move or inline image cannot
/// be written back safely, or any [`Error`] from [`interpret`](crate::interpret).
pub fn encode<'o, O>(
    objects: O,
    content: &Concat<'_>,
    resources: Option<&'o Object>,
    limits: &Limits,
    fonts: &Fonts,
) -> Result<Encoded, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    encode_with(objects, content, resources, limits, fonts, &mut ())
}

/// Re-encodes `content` and every Form XObject drawn on it, reporting
/// recoverable ISO 32000-2 departures to `sink`.
///
/// # Errors
///
/// Same error conditions as [`encode`].
pub fn encode_with<'o, O, S>(
    objects: O,
    content: &Concat<'_>,
    resources: Option<&'o Object>,
    limits: &Limits,
    fonts: &Fonts,
    sink: &mut S,
) -> Result<Encoded, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    phase!(DEBUG, "encode", bytes = content.bytes().len());
    if let Some(unreadable) = content.undecoded().first() {
        return Err(Error::PageStream(unreadable.clone()));
    }

    let mut page = StreamOut::default();
    let mut form_index: HashMap<ObjectId, usize> = HashMap::new();
    let mut forms: Vec<(ObjectId, StreamOut)> = Vec::new();
    let mut uses: Vec<UseFrame> = Vec::new();
    let mut stop_err: Option<Error> = None;

    let opts = Opts {
        forms: Forms::Descend,
        wants: Wants {
            ops: Operators::ALL,
            ..Wants::NONE
        },
    };

    interpret_with(
        objects,
        content,
        resources,
        limits,
        fonts,
        opts,
        sink,
        |event| {
            match event.kind {
                Kind::FormBegin(form) => {
                    let (idx, first) = match form_index.get(&form.form) {
                        Some(&idx) => (idx, false),
                        None => {
                            let idx = forms.len();
                            form_index.insert(form.form, idx);
                            forms.push((form.form, StreamOut::default()));
                            (idx, true)
                        }
                    };
                    uses.push(UseFrame {
                        form: form.form,
                        idx,
                        first,
                        inline_cursor: 0,
                    });
                }
                Kind::FormEnd(_) => {
                    if let Some(ended) = uses.pop()
                        && !ended.first
                    {
                        let expected = &forms[ended.idx].1.inlines;
                        if ended.inline_cursor != expected.len() {
                            let at = expected.get(ended.inline_cursor).map_or(0, |s| s.orig_at);
                            stop_err = Some(Error::Unwritable {
                                form: Some(ended.form),
                                at,
                                why: Unwritable::InlineStream,
                            });
                            return ControlFlow::Break(());
                        }
                    }
                }
                Kind::Op => {
                    let res = match uses.last_mut() {
                        None => write_op(&mut page, None, &event, limits),
                        Some(u) if u.first => {
                            write_op(&mut forms[u.idx].1, Some(u.form), &event, limits)
                        }
                        Some(u) => check_repeat_op(
                            &mut forms[u.idx].1,
                            u.form,
                            &mut u.inline_cursor,
                            &event.at,
                        ),
                    };
                    if let Err(err) = res {
                        stop_err = Some(err);
                        return ControlFlow::Break(());
                    }
                }
                _ => {}
            }
            ControlFlow::Continue(())
        },
    )?;

    if let Some(err) = stop_err {
        return Err(err);
    }

    let reserved = Reserved::default();
    let mut verified_bytes = 0usize;
    let content = finish_stream(page, None, &reserved, limits, &mut verified_bytes)?;
    let mut out_forms = Vec::with_capacity(forms.len());
    for (id, form_out) in forms {
        let bytes = finish_stream(form_out, Some(id), &reserved, limits, &mut verified_bytes)?;
        out_forms.push((id, bytes));
    }
    Ok(Encoded {
        content,
        forms: out_forms,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InlineSnap {
    orig_at: usize,
    raw_len: usize,
    rel_data: Range<usize>,
    rel_kept: Option<Range<usize>>,
}

impl InlineSnap {
    fn of(image: &InlineImage<'_>) -> Self {
        let base = image.at().start;
        let rel = |r: Range<usize>| (r.start - base)..(r.end - base);
        let rel_kept = match image.stream() {
            ImageStream::Data(kept) => Some(rel(kept)),
            _ => None,
        };
        Self {
            orig_at: base,
            raw_len: image.at().len(),
            rel_data: rel(image.data()),
            rel_kept,
        }
    }
}

#[derive(Default)]
struct StreamOut {
    writer: ContentWriter,
    inlines: Vec<InlineSnap>,
    res_list: Vec<ResCx>,
}

impl StreamOut {
    fn record_res(&mut self, res: &ResCx) {
        if !self.res_list.contains(res) {
            self.res_list.push(res.clone());
        }
    }
}

struct UseFrame {
    form: ObjectId,
    idx: usize,
    first: bool,
    inline_cursor: usize,
}

fn validate_inline<'a>(
    form: Option<ObjectId>,
    start: usize,
    content: &'a [u8],
    image: &InlineImage<'a>,
) -> Result<&'a [u8], Error> {
    if image.capped() {
        return Err(Error::CappedInlineImage { form, at: start });
    }
    let raw_inline = &content[image.at()];
    if image.stream() == ImageStream::Missing || !raw_inline.ends_with(b"EI") {
        return Err(Error::Unwritable {
            form,
            at: start,
            why: Unwritable::InlineStream,
        });
    }
    if let Some(given) = image.length()
        && given != image.data().len() as u64
    {
        return Err(Error::Unwritable {
            form,
            at: start,
            why: Unwritable::InlineLength,
        });
    }
    Ok(raw_inline)
}

fn check_repeat_op(
    out: &mut StreamOut,
    form: ObjectId,
    cursor: &mut usize,
    at: &At<'_>,
) -> Result<(), Error> {
    let Some(step) = at.operation() else {
        return Ok(());
    };
    let start = step.at().start;
    if let Some(image) = step.inline_image() {
        validate_inline(Some(form), start, at.content(), &image)?;
        let snap = InlineSnap::of(&image);
        if out.inlines.get(*cursor) != Some(&snap) {
            return Err(Error::Unwritable {
                form: Some(form),
                at: start,
                why: Unwritable::InlineStream,
            });
        }
        *cursor += 1;
        out.record_res(at.res);
    } else if step.operator() == Operator::BeginInlineImage {
        return Err(Error::Unwritable {
            form: Some(form),
            at: start,
            why: Unwritable::LoneBi,
        });
    }
    Ok(())
}

fn finish_stream(
    out: StreamOut,
    form: Option<ObjectId>,
    reserved: &Reserved,
    limits: &Limits,
    verified_bytes: &mut usize,
) -> Result<Vec<u8>, Error> {
    let bytes = out.writer.finish(reserved).0;
    if !out.inlines.is_empty() {
        for res in &out.res_list {
            *verified_bytes = verified_bytes.saturating_add(bytes.len());
            if *verified_bytes > limits.interpreted {
                return Err(Error::Limit(Cap::Interpreted));
            }
            verify_encoded_inlines(&bytes, &out.inlines, res, &limits.content, form)?;
        }
    }
    Ok(bytes)
}

fn verify_encoded_inlines(
    bytes: &[u8],
    expected: &[InlineSnap],
    res: &ResCx,
    content_limits: &pdf_content_streams::Limits,
    form: Option<ObjectId>,
) -> Result<(), Error> {
    let fallback_at = expected.first().map_or(0, |s| s.orig_at);
    let mut idx = 0usize;
    for step in operations(bytes, res, content_limits) {
        let Ok(step) = step else {
            let at = expected.get(idx).map_or(fallback_at, |s| s.orig_at);
            return Err(Error::Unwritable {
                form,
                at,
                why: Unwritable::InlineStream,
            });
        };
        if let Some(image) = step.inline_image() {
            let Some(exp) = expected.get(idx) else {
                return Err(Error::Unwritable {
                    form,
                    at: fallback_at,
                    why: Unwritable::InlineStream,
                });
            };
            if image.capped() {
                return Err(Error::CappedInlineImage {
                    form,
                    at: exp.orig_at,
                });
            }
            let raw = &bytes[image.at()];
            let snap = InlineSnap::of(&image);
            if image.stream() == ImageStream::Missing
                || !raw.ends_with(b"EI")
                || snap.raw_len != exp.raw_len
                || snap.rel_data != exp.rel_data
                || snap.rel_kept != exp.rel_kept
            {
                return Err(Error::Unwritable {
                    form,
                    at: exp.orig_at,
                    why: Unwritable::InlineStream,
                });
            }
            idx += 1;
        } else if step.operator() == Operator::BeginInlineImage {
            let at = expected.get(idx).map_or(fallback_at, |s| s.orig_at);
            return Err(Error::Unwritable {
                form,
                at,
                why: Unwritable::LoneBi,
            });
        }
    }
    if idx != expected.len() {
        let at = expected.get(idx).map_or(fallback_at, |s| s.orig_at);
        return Err(Error::Unwritable {
            form,
            at,
            why: Unwritable::InlineStream,
        });
    }
    Ok(())
}

fn write_op(
    out: &mut StreamOut,
    form: Option<ObjectId>,
    event: &Event<'_>,
    limits: &Limits,
) -> Result<(), Error> {
    let at = &event.at;
    let Some(step) = at.operation() else {
        return Ok(());
    };
    let start = step.at().start;

    if let Some(image) = step.inline_image() {
        let raw_inline = validate_inline(form, start, at.content(), &image)?;
        out.inlines.push(InlineSnap::of(&image));
        out.record_res(at.res);
        out.writer
            .operation(step.operands().map(|o| o.bytes()), raw_inline);
        return Ok(());
    }

    let op = step.operator();
    if op == Operator::BeginInlineImage {
        return Err(Error::Unwritable {
            form,
            at: start,
            why: Unwritable::LoneBi,
        });
    }

    if !matches!(
        op,
        Operator::MoveLine
            | Operator::MoveLineLeading
            | Operator::NextLine
            | Operator::NextLineShow
            | Operator::NextLineShowSpaced
    ) {
        out.writer
            .operation(step.operands().map(|o| o.bytes()), step.name());
        return Ok(());
    }

    let before = &event.state.text;
    let moved = event.state.moved(op, step, &limits.content);
    let after = &moved.text;

    // Preserve a no-op `Td`/`TD` verbatim (e.g. one with out-of-range operands
    // that leaves `line` and `pen` untouched while still interrupting PDFium's
    // fast path builder).
    if matches!(op, Operator::MoveLine | Operator::MoveLineLeading)
        && after.line == before.line
        && after.pen == before.pen
    {
        out.writer
            .operation(step.operands().map(|o| o.bytes()), step.name());
        return Ok(());
    }

    let real_at = |x: f32| {
        exact_real(x).map_err(|why| Error::Unwritable {
            form,
            at: start,
            why,
        })
    };

    if op == Operator::NextLineShowSpaced && nums::<3>(step).is_some() {
        out.writer
            .operation([real_at(after.word_space)?.as_slice()], b"Tw");
        out.writer
            .operation([real_at(after.char_space)?.as_slice()], b"Tc");
    }

    let Matrix { a, b, c, d, e, f } = after.matrix;
    let tm = [
        real_at(a)?,
        real_at(b)?,
        real_at(c)?,
        real_at(d)?,
        real_at(e)?,
        real_at(f)?,
    ];
    out.writer.operation(tm.iter().map(Vec::as_slice), b"Tm");

    if after.line != Point::ZERO {
        let td = [real_at(after.line.x)?, real_at(after.line.y)?];
        out.writer.operation(td.iter().map(Vec::as_slice), b"Td");
    }

    if matches!(op, Operator::NextLineShow | Operator::NextLineShowSpaced)
        && let Some(string) = step.arg(0)
    {
        out.writer.operation([string.bytes()], b"Tj");
    }
    Ok(())
}

fn exact_real(x: f32) -> Result<Vec<u8>, Unwritable> {
    let mut out = Vec::new();
    serialize::real(&mut out, f64::from(x)).map_err(|_| Unwritable::Line)?;
    Ok(out)
}
