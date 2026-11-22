//! Placed text strings (`Tj`, `TJ`, `'`, `"`) and lazy glyph layout
//! (`CPDF_TextObject::CalcPositionDataInternal`, `CPDF_TextPage::GetLooseBounds`).
//!
//! Each non-empty string shown emits a [`TextShow`] event carrying its active
//! [`Font`], size, effective [`TextMode`], text-space-to-PageSpace `start`
//! matrix, and offset from the operator's starting pen. Calling
//! [`TextShow::glyphs`] lazily decodes character codes, computes horizontal or
//! vertical origins and advances, and unites metric cells with embedded glyph
//! outline boxes when font programs are loaded.

use std::borrow::Cow;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::Range;

use pdf_content_streams::TextMode;
use pdf_fonts::{Codes, Font, FontApprox, FontKind, GlyphBox};

use crate::geom::{Matrix, Point, Quad, Rect};
use crate::state::{State, TextState};

/// Upper bound (`3e37`, ~`0.1 * f32::MAX`) checked in `f64` before laying out
/// glyphs in `f32` so intermediate coordinate sums never overflow `f32`.
const PLACED: f64 = 3e37;

/// PDFium's minimum outline side threshold (`kSizeEpsilon` in
/// `cpdf_textpage.cpp`), in text space at the active font size.
const SIZE_EPSILON: f32 = 0.01;

/// One non-empty string shown by `Tj`, `'`, `"`, or an element of `TJ`.
///
/// Empty strings (`()`) are skipped without emitting a `TextShow`, matching
/// PDFium. Within `TJ`, all non-empty strings share one `start` matrix (at the
/// pen after any leading numeric adjustments) and advance via `offset`.
///
/// Its `Debug` implementation prints `string_len` rather than raw text bytes.
#[derive(Clone)]
#[non_exhaustive]
pub struct TextShow<'e> {
    /// Decoded string bytes (literal escapes or hex digits already decoded).
    pub string: &'e [u8],
    /// Active font (`Some` once `Tf` or `/ExtGState` `/Font` has run, using
    /// PDFium's stock Helvetica when the resource name is missing; `None` only
    /// before any font selection, where PDFium draws nothing).
    pub font: Option<&'e Font>,
    /// `true` when `font` is PDFium's stock Helvetica fallback.
    pub stock: bool,
    /// Font size in text space units.
    pub size: f32,
    /// Effective rendering mode (`Tr`, forced to [`TextMode::Fill`] for Type 3
    /// fonts, matching PDFium).
    pub mode: TextMode,
    /// Transform from unscaled-size text space (at the operator's starting pen,
    /// after `Tz` and `Ts`) onto PageSpace.
    pub start: Matrix,
    /// Distance from `start` along the baseline (or down the column in vertical
    /// writing) where this string begins (`0.0` for `Tj` / `'` / `"` and the
    /// first string of `TJ`).
    pub offset: f32,
    pub(crate) text: &'e TextState,
}

impl fmt::Debug for TextShow<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextShow")
            .field("string_len", &self.string.len())
            .field("font", &self.font.map(Font::id))
            .field("stock", &self.stock)
            .field("size", &self.size)
            .field("mode", &self.mode)
            .field("start", &self.start)
            .field("offset", &self.offset)
            .finish()
    }
}

impl<'e> TextShow<'e> {
    pub(crate) fn new(
        string: &'e [u8],
        state: &'e State,
        font: Option<&'e Font>,
        offset: f32,
    ) -> Self {
        let text = &state.text;
        let type3 = font.is_some_and(|f| f.kind() == FontKind::Type3);
        let at = Matrix {
            a: text.scale,
            e: text.pen.x,
            f: text.pen.y + text.rise,
            ..Matrix::IDENTITY
        };
        Self {
            string,
            font,
            stock: text.stock,
            size: text.size,
            mode: if type3 { TextMode::Fill } else { text.mode },
            start: at.then(text.matrix).then(state.ctm),
            offset,
            text,
        }
    }

    pub(crate) fn bounded(&self, font: &Font, n: usize, end: f32) -> bool {
        let text = self.text;
        let size = f64::from(text.size.abs());
        let em = f64::from(font.extent()) * size / 1000.0;
        let step = em + f64::from(text.word_space.abs()) + f64::from(text.char_space.abs());
        let reach = f64::from(self.offset.abs()) + n as f64 * step + em + size / 2.0;
        let s = self.start;
        let spread = [s.a, s.b, s.c, s.d]
            .iter()
            .map(|v| f64::from(v.abs()))
            .sum::<f64>();
        let corner = f64::from(s.e.abs()) + f64::from(s.f.abs()) + spread * reach;
        let pen = f64::from(text.pen.x.abs()) + f64::from(text.pen.y.abs());
        let along = pen + 2.0 * reach * (1.0 + f64::from(text.scale.abs()));
        end.is_finite() && corner < PLACED && along < PLACED
    }

    /// Text rendering matrix `Trm` where the operator's text begins
    /// (ISO 32000-2, §9.4.4): maps glyph space scaled to unit font size (`1.0`)
    /// onto PageSpace.
    #[must_use]
    pub fn trm(&self) -> Matrix {
        Matrix {
            a: self.size,
            d: self.size,
            ..Matrix::IDENTITY
        }
        .then(self.start)
    }

    /// Lazily lays out each character code in `self.string` as a [`Glyph`].
    /// Empty when `self.font` is `None`.
    pub fn glyphs(&self) -> Glyphs<'e> {
        let invisible = matches!(self.mode, TextMode::Invisible | TextMode::Clip);
        let inner = self.font.map(|font| GlyphsInner {
            codes: font.codes(self.string),
            font,
            text: self.text,
            start: self.start,
            at: self.offset,
            moved: self.text.guessed,
            invisible,
        });
        Glyphs { inner }
    }
}

/// Lazy iterator over the [`Glyph`]s of a [`TextShow`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Clone)]
pub struct Glyphs<'e> {
    inner: Option<GlyphsInner<'e>>,
}

#[derive(Clone)]
struct GlyphsInner<'e> {
    codes: Codes<'e>,
    font: &'e Font,
    text: &'e TextState,
    start: Matrix,
    at: f32,
    moved: bool,
    invisible: bool,
}

impl fmt::Debug for Glyphs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Glyphs")
            .field("has_font", &self.inner.is_some())
            .finish()
    }
}

impl<'e> Iterator for Glyphs<'e> {
    type Item = Glyph<'e>;

    fn next(&mut self) -> Option<Self::Item> {
        let g = self.inner.as_mut()?;
        let (bytes, code) = g.codes.next()?;
        let font = g.font;
        let text = g.text;
        let vertical = font.is_vertical();
        let laid = step_code(font, text, vertical, &mut g.at, bytes, code);

        let outlined = font.outlined();
        let size = text.size / 1000.0;
        let metrics = font.metrics();
        let guessed = font.width_guessed(laid.code);
        let outline = outlined.then(|| font.char_box(laid.code)).flatten();
        let mut approx = font.approx();
        approx.widths |= g.moved || guessed;
        approx.boxes |= outlined && outline.is_none();
        g.moved |= guessed;

        let (origin, cell, pen) = if vertical {
            let v = font.vertical(laid.code);
            let (vx, vy) = v.map_or((0.0, 0.0), |v| (v.vx, v.vy));
            let origin = Point::new(-text.size * vx / 1000.0, laid.at - text.size * vy / 1000.0);
            let half = text.size / 2.0;
            let cell = Rect::new(-half, laid.at + laid.width, half, laid.at);
            (origin, cell, Point::new(0.0, laid.at))
        } else {
            let origin = Point::new(laid.at, 0.0);
            let cell = Rect::new(
                laid.at,
                metrics.descent * size,
                laid.at + laid.width,
                metrics.ascent * size,
            );
            let pen = Point::new(laid.at * text.scale, 0.0);
            (origin, cell, pen)
        };

        let cell = match outline {
            Some(r) => cell.union(outline_box(r, origin, size, laid.units * size)),
            None => cell,
        };

        Some(Glyph {
            bytes: laid.bytes,
            code: laid.code,
            cid: font.cid(laid.code),
            origin: g.start.apply(origin),
            advance: if vertical {
                laid.end - laid.at
            } else {
                (laid.end - laid.at) * text.scale
            },
            quad: g.start.apply_quad(cell),
            approx,
            invisible: g.invisible,
            pen: Point::new(text.pen.x + pen.x, text.pen.y + pen.y),
            font,
            text,
        })
    }
}

impl FusedIterator for Glyphs<'_> {}

fn outline_box(r: GlyphBox, origin: Point, size: f32, width: f32) -> Rect {
    let (x0, y0) = (r.x0 * size + origin.x, r.y0 * size + origin.y);
    let (mut x1, mut y1) = (r.x1 * size + origin.x, r.y1 * size + origin.y);
    if (y1 - y0).abs() < SIZE_EPSILON {
        y1 = y0 + size;
    }
    if (x1 - x0).abs() < SIZE_EPSILON {
        x1 = x0 + width;
    }
    Rect::new(x0, y0, x1, y1)
}

/// One placed glyph decoded from a [`TextShow`].
#[derive(Clone)]
#[non_exhaustive]
pub struct Glyph<'e> {
    /// Byte range of this character code inside [`TextShow::string`].
    pub bytes: Range<usize>,
    /// Raw character code decoded by the font's encoding / CMap.
    pub code: u32,
    /// CID selected by `code` in a Type 0 font, if known.
    pub cid: Option<u32>,
    /// Glyph origin in PageSpace (in vertical writing, the horizontal glyph
    /// origin `origin - (vx, vy)` matching PDFium).
    pub origin: Point,
    /// Advance in the text matrix's space along the line (scaled by `Tz`) or
    /// down the vertical column (including glyph width, `Tc`, and single-byte
    /// space `Tw`, excluding `TJ` numeric adjustments).
    pub advance: f32,
    /// Oriented loose bounding quadrilateral in PageSpace: metric ascent/descent
    /// cell (or em-wide vertical column) united in text space with the
    /// embedded glyph outline box when available.
    pub quad: Quad,
    /// Approximation flags indicating whether codes, widths, heights, or
    /// outline boxes were synthesized or affected by an earlier guessed width
    /// on the same line.
    pub approx: FontApprox,
    /// `true` when shown in `3 Tr` (`Invisible`) or `7 Tr` (`Clip`).
    pub invisible: bool,
    /// Starting pen position of this glyph in the text matrix's space.
    pub pen: Point,
    pub(crate) font: &'e Font,
    pub(crate) text: &'e TextState,
}

impl fmt::Debug for Glyph<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Glyph")
            .field("bytes", &self.bytes)
            .field("code", &self.code)
            .field("cid", &self.cid)
            .field("origin", &self.origin)
            .field("advance", &self.advance)
            .field("quad", &self.quad)
            .field("approx", &self.approx)
            .field("invisible", &self.invisible)
            .field("pen", &self.pen)
            .finish()
    }
}

impl Glyph<'_> {
    /// Unicode text mapped from `self.code` by the font (`/ToUnicode`, simple
    /// encoding, or CJK UCS-2 collection), if known.
    #[must_use]
    pub fn text(&self) -> Option<Cow<'_, str>> {
        self.font.code_text(self.code)
    }

    /// Standalone `Tm` text matrix that places a new string starting directly
    /// at this glyph's position (preserving the active text matrix's rotation,
    /// scale, and skew).
    #[must_use]
    pub fn tm(&self) -> Matrix {
        Matrix::translate(self.pen.x, self.pen.y).then(self.text.matrix)
    }
}

struct Laid {
    bytes: Range<usize>,
    code: u32,
    at: f32,
    units: f32,
    width: f32,
    end: f32,
}

fn step_code(
    font: &Font,
    text: &TextState,
    vertical: bool,
    at: &mut f32,
    bytes: Range<usize>,
    code: u32,
) -> Laid {
    let start = *at;
    let units = if vertical {
        font.vertical(code).map_or(0.0, |v| v.w1)
    } else {
        font.width(code)
    };
    let width = units * text.size / 1000.0;
    let mut next = start + width;
    if font.word_spaced(code, bytes.len()) {
        next += text.word_space;
    }
    next += text.char_space;
    *at = next;
    Laid {
        bytes,
        code,
        at: start,
        units,
        width,
        end: next,
    }
}

pub(crate) fn advance(font: &Font, text: &TextState, s: &[u8], from: f32) -> (f32, usize) {
    match font.byte_widths() {
        Some(widths) => (bytes_end(widths, text, s, from), s.len()),
        None => laid_end(font, text, s, from),
    }
}

pub(crate) fn laid_end(font: &Font, text: &TextState, s: &[u8], from: f32) -> (f32, usize) {
    let vertical = font.is_vertical();
    let mut at = from;
    let mut glyphs = 0;
    for (bytes, code) in font.codes(s) {
        let _ = step_code(font, text, vertical, &mut at, bytes, code);
        glyphs += 1;
    }
    (at, glyphs)
}

pub(crate) fn bytes_end(widths: &[f32; 256], text: &TextState, s: &[u8], from: f32) -> f32 {
    let mut at = from;
    if text.word_space == 0.0 && text.char_space == 0.0 {
        for &b in s {
            at += widths[usize::from(b)] * text.size / 1000.0;
        }
        return at;
    }
    for &b in s {
        let mut next = at + widths[usize::from(b)] * text.size / 1000.0;
        if b == 32 {
            next += text.word_space;
        }
        next += text.char_space;
        at = next;
    }
    at
}
