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

use std::collections::HashSet;
use std::fmt;
use std::ops::ControlFlow;

use pdf_content_streams::{Concat, ContentWriter, ImageStream, Operator, Operators, Reserved};
use pdf_fonts::Fonts;
use pdf_objects::fault::Sink;
use pdf_objects::{Object, ObjectId, Resolve, Streams, serialize};

use crate::error::{Error, Unwritable};
use crate::fault::Fault;
use crate::geom::{Matrix, Point};
use crate::interp::{Event, Forms, Kind, Opts, Wants, interpret_with};
use crate::limits::Limits;
use crate::phases::phase;

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

    let mut page = ContentWriter::new();
    let mut forms: Vec<(ObjectId, ContentWriter)> = Vec::new();
    let mut written = HashSet::new();
    let mut uses: Vec<(ObjectId, Option<usize>)> = Vec::new();
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
                    let first = written.insert(form.form).then(|| {
                        forms.push((form.form, ContentWriter::new()));
                        forms.len() - 1
                    });
                    uses.push((form.form, first));
                }
                Kind::FormEnd(_) => {
                    uses.pop();
                }
                Kind::Op => {
                    let (form, out) = match uses.last() {
                        None => (None, &mut page),
                        Some(&(form, Some(i))) => (Some(form), &mut forms[i].1),
                        Some((_, None)) => return ControlFlow::Continue(()),
                    };
                    if let Err(err) = write_op(out, form, &event, limits) {
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
    let finish = |w: ContentWriter| w.finish(&reserved).0;
    Ok(Encoded {
        content: finish(page),
        forms: forms.into_iter().map(|(id, w)| (id, finish(w))).collect(),
    })
}

fn write_op(
    out: &mut ContentWriter,
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
        if image.capped() {
            return Err(Error::CappedInlineImage { form, at: start });
        }
        let raw_inline = &at.content()[image.at()];
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
        out.operation(step.operands().map(|o| o.bytes()), raw_inline);
        return Ok(());
    }

    let op = step.operator();
    if !matches!(
        op,
        Operator::MoveLine
            | Operator::MoveLineLeading
            | Operator::NextLine
            | Operator::NextLineShow
            | Operator::NextLineShowSpaced
    ) {
        out.operation(step.operands().map(|o| o.bytes()), step.name());
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
        out.operation(step.operands().map(|o| o.bytes()), step.name());
        return Ok(());
    }

    let real_at = |x: f32| {
        exact_real(x).map_err(|why| Error::Unwritable {
            form,
            at: start,
            why,
        })
    };

    if op == Operator::NextLineShowSpaced {
        out.operation([real_at(after.word_space)?.as_slice()], b"Tw");
        out.operation([real_at(after.char_space)?.as_slice()], b"Tc");
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
    out.operation(tm.iter().map(Vec::as_slice), b"Tm");

    if after.line != Point::ZERO {
        let td = [real_at(after.line.x)?, real_at(after.line.y)?];
        out.operation(td.iter().map(Vec::as_slice), b"Td");
    }

    if matches!(op, Operator::NextLineShow | Operator::NextLineShowSpaced)
        && let Some(string) = step.arg(0)
    {
        out.operation([string.bytes()], b"Tj");
    }
    Ok(())
}

fn exact_real(x: f32) -> Result<Vec<u8>, Unwritable> {
    let mut out = Vec::new();
    serialize::real(&mut out, f64::from(x)).map_err(|_| Unwritable::Line)?;
    Ok(out)
}
