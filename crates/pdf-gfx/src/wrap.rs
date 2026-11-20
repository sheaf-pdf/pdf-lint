//! Multiline paragraph wrapping, alignment, auto-sizing, and comb-cell layout
//! (`feature = "text-layout"`).
//!
//! [`layout_text`] lays out UTF-8 text inside a target [`Rect`] over any
//! [`LayoutFont`] ([`Std14`], [`Font`], or [`FontEncoder`]), supporting word or
//! character wrapping, left/center/right/justified alignment, top/middle/bottom
//! vertical alignment, automatic font sizing (`size: None`), and fixed-width
//! comb cells (`comb: Some(n)`). [`Canvas::draw_layout`](crate::Canvas::draw_layout)
//! and [`Canvas::text_box`](crate::Canvas::text_box) emit the resulting lines as
//! position-independent `Tm` + `Tj` operations.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use pdf_content_streams::TextMode;
use pdf_fonts::{Encoding, Font, FontMetrics, Std14};

use crate::geom::{Point, Rect};

/// Font abstraction providing metrics, advance widths (per 1000 em), and
/// character-code encoding for [`layout_text`].
///
/// Implemented for [`Std14`], [`Font`], and [`FontEncoder`].
pub trait LayoutFont {
    /// Ascent, descent, and font bounding box per 1000 em units.
    fn font_metrics(&self) -> FontMetrics;

    /// Advance width of `ch` in 1/1000 em units, or `None` if `ch` has no
    /// character code in this font.
    fn char_advance(&self, ch: char) -> Option<f32>;

    /// Appends the encoded character-code bytes for `ch` to `out`, returning
    /// `false` without modifying `out` if `ch` cannot be encoded.
    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool;
}

impl<T: LayoutFont + ?Sized> LayoutFont for &T {
    fn font_metrics(&self) -> FontMetrics {
        (**self).font_metrics()
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        (**self).char_advance(ch)
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        (**self).encode_char(ch, out)
    }
}

fn std14_encoding(font: Std14) -> Encoding {
    match font {
        Std14::Symbol => Encoding::Symbol,
        Std14::ZapfDingbats => Encoding::ZapfDingbats,
        _ => Encoding::WinAnsi,
    }
}

impl LayoutFont for Std14 {
    fn font_metrics(&self) -> FontMetrics {
        self.metrics()
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        let enc = std14_encoding(*self);
        let code = enc.code(ch)?;
        let glyph = enc.glyph(code)?;
        self.glyph_width(glyph)
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        let Some(code) = std14_encoding(*self).code(ch) else {
            return false;
        };
        out.push(code);
        true
    }
}

impl LayoutFont for Font {
    fn font_metrics(&self) -> FontMetrics {
        self.metrics()
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        let code = find_single_byte_code(self, ch)?;
        Some(self.width(u32::from(code)))
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        let Some(code) = find_single_byte_code(self, ch) else {
            return false;
        };
        out.push(code);
        true
    }
}

fn find_single_byte_code(font: &Font, target: char) -> Option<u8> {
    if target == ' '
        && font.code_text(32).as_deref() == Some(" ")
        && font
            .codes(&[32])
            .next()
            .is_some_and(|(r, c)| r.len() == 1 && c == 32)
    {
        return Some(32);
    }
    for byte in 0u8..=255u8 {
        let buf = [byte];
        let Some((range, code)) = font.codes(&buf).next() else {
            continue;
        };
        if range.len() == 1
            && let Some(txt) = font.code_text(code)
            && txt.chars().count() == 1
            && txt.starts_with(target)
        {
            return Some(byte);
        }
    }
    None
}

/// Pre-indexed reverse character encoder over a parsed [`Font`] (supporting
/// both single-byte encodings with `/Differences` and 2-byte Type 0 CMaps with
/// `/ToUnicode`).
#[derive(Clone)]
pub struct FontEncoder<'a> {
    font: &'a Font,
    map: HashMap<char, ([u8; 4], u8, f32)>,
}

impl fmt::Debug for FontEncoder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontEncoder")
            .field("mapped_chars", &self.map.len())
            .finish()
    }
}

impl<'a> FontEncoder<'a> {
    /// Indexes encodable characters and widths from `font` (scanning single-byte
    /// codes first and 2-byte codes when `font` uses a multi-byte CMap).
    #[must_use]
    pub fn new(font: &'a Font) -> Self {
        let mut map: HashMap<char, ([u8; 4], u8, f32)> = HashMap::new();
        let single_byte = font.byte_widths().is_some();
        if single_byte {
            for b in 0u8..=255u8 {
                let code = u32::from(b);
                if let Some(txt) = font.code_text(code)
                    && txt.chars().count() == 1
                {
                    let ch = txt.chars().next().unwrap();
                    map.entry(ch).or_insert(([b, 0, 0, 0], 1, font.width(code)));
                }
            }
        } else {
            for hi in 0u8..=255u8 {
                let first = [hi, 0];
                let Some((r1, c1)) = font.codes(&first).next() else {
                    continue;
                };
                if r1.len() == 1 {
                    if let Some(txt) = font.code_text(c1)
                        && txt.chars().count() == 1
                    {
                        let ch = txt.chars().next().unwrap();
                        map.entry(ch).or_insert(([hi, 0, 0, 0], 1, font.width(c1)));
                    }
                } else if r1.len() == 2 {
                    for lo in 0u8..=255u8 {
                        let pair = [hi, lo];
                        if let Some((r2, c2)) = font.codes(&pair).next()
                            && r2.len() == 2
                            && let Some(txt) = font.code_text(c2)
                            && txt.chars().count() == 1
                        {
                            let ch = txt.chars().next().unwrap();
                            map.entry(ch).or_insert(([hi, lo, 0, 0], 2, font.width(c2)));
                        }
                    }
                }
            }
        }
        Self { font, map }
    }
}

impl LayoutFont for FontEncoder<'_> {
    fn font_metrics(&self) -> FontMetrics {
        self.font.metrics()
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        self.map.get(&ch).map(|&(_, _, w)| w)
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        let Some(&(bytes, len, _)) = self.map.get(&ch) else {
            return false;
        };
        out.extend_from_slice(&bytes[..usize::from(len)]);
        true
    }
}

/// Horizontal text alignment within a layout box.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    /// Flush with the left edge (`x0`).
    #[default]
    Left,
    /// Centered horizontally within the box width.
    Center,
    /// Flush with the right edge (`x1`).
    Right,
    /// Distributes extra space across inter-word spaces (`Tw`) on wrapped
    /// non-final lines; the final line of each paragraph stays left-aligned.
    Justify,
}

/// Vertical block alignment within a layout box.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum VerticalAlign {
    /// Anchored below the top edge (`y1`).
    #[default]
    Top,
    /// Vertically centered between `y0` and `y1`.
    Middle,
    /// Anchored above the bottom edge (`y0`).
    Bottom,
}

/// Line-wrapping policy for [`layout_text`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum WrapMode {
    /// Wraps at whitespace and after ASCII hyphens, breaking an individual
    /// overlong word across characters only if the word alone exceeds the box
    /// width.
    #[default]
    Word,
    /// Wraps strictly at character boundaries whenever the next character would
    /// exceed the box width.
    Char,
    /// Disables automatic width wrapping (explicit `'\n'` line breaks still
    /// start a new line).
    NoWrap,
}

/// Layout parameters for [`layout_text`] and [`Canvas::text_box`](crate::Canvas::text_box).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextBoxStyle {
    /// Fixed font size in points, or `None` (or `Some(0.0)`) to auto-size
    /// within `[min_auto_size, max_auto_size]` so the text fits inside the
    /// box: `max_auto_size` exactly where the text fits at it.
    pub size: Option<f32>,
    /// Minimum font size chosen when `size` is auto-sized (`4.0` by default).
    pub min_auto_size: f32,
    /// Maximum font size chosen when `size` is auto-sized (`12.0` by default).
    pub max_auto_size: f32,
    /// Explicit baseline-to-baseline line step in points (`None` uses
    /// `(ascent - descent) / 1000 * font_size * leading_factor`, the heights
    /// as [`font_heights_as_given`](Self::font_heights_as_given) says).
    pub line_height: Option<f32>,
    /// Multiplier applied to `(ascent - descent)` when `line_height` is `None`
    /// (`1.2` by default).
    pub leading_factor: f32,
    /// Horizontal alignment (`TextAlign::Left` by default).
    pub align: TextAlign,
    /// Vertical alignment (`VerticalAlign::Top` by default).
    pub vertical_align: VerticalAlign,
    /// Line-wrapping mode (`WrapMode::Word` by default).
    pub wrap: WrapMode,
    /// Uniform inner padding inset from all four sides of `rect` (`0.0` by
    /// default).
    pub padding: f32,
    /// Optional fixed number of equal-width comb cells across the box width
    /// (for AcroForm comb text fields).
    pub comb: Option<u32>,
    /// Whether [`Canvas::text_box`](crate::Canvas::text_box) clips drawing to
    /// `rect` inside a `q`..`Q` scope (`true` by default).
    pub clip: bool,
    /// Text rendering mode (`TextMode::Fill` by default).
    pub mode: TextMode,
    /// Whether a wrapped line keeps the spaces it's broken at (`false` by
    /// default).
    ///
    /// Off, the spaces a line ends with are trimmed from it, and a wrapped
    /// line's first spaces are passed over. On, no space is dropped, and
    /// words are found as PDFium's line breaking finds them as of
    /// chromium/8152: under [`WrapMode::Word`] a line that would pass the
    /// box's width at its next character, a space among them, is broken
    /// before the last word that begins on it after its first character, so
    /// the spaces before that word end the line, in its `codes`, its
    /// `text_range`, and the `width` it's aligned by, and the next line
    /// begins at the word. With no such word it's broken before that next
    /// character, which begins the next line even where it's a space. A
    /// paragraph's last line keeps the spaces it ends with too. Justified
    /// text is spread by the spaces between its words, not by those that end
    /// its line.
    ///
    /// A word begins at a character that isn't a space or punctuation after
    /// a space or punctuation, but not after punctuation that joins what's
    /// about it (an apostrophe, comma, hyphen, full stop or solidus: `e.g.`
    /// and `half-baked` are each one word); at an opening bracket; at a
    /// currency sign, but not after one; and at an ideograph or after one;
    /// never between two letters or digits, and not inside a bracket just
    /// opened (the spaces after it, and the first character after those,
    /// carry on its word).
    pub line_end_spaces: bool,
    /// Whether the font's ascent and descent are taken as its
    /// [`LayoutFont::font_metrics`] gives them (`false` by default).
    ///
    /// Off, layout takes the ascent as at least `0.70` em, the descent as at
    /// most `-0.15` em, and the difference between them as at least `0.85`
    /// em, so a font that gives small heights, or none, still has room. On,
    /// they're taken as given wherever they're finite and the ascent is
    /// above the descent (per em, as layout divides them by `1000`); where
    /// they aren't, the same floors apply, so no line has a step of `0`.
    pub font_heights_as_given: bool,
}

impl Default for TextBoxStyle {
    fn default() -> Self {
        Self {
            size: Some(12.0),
            min_auto_size: 4.0,
            max_auto_size: 12.0,
            line_height: None,
            leading_factor: 1.2,
            align: TextAlign::Left,
            vertical_align: VerticalAlign::Top,
            wrap: WrapMode::Word,
            padding: 0.0,
            comb: None,
            clip: true,
            mode: TextMode::Fill,
            line_end_spaces: false,
            font_heights_as_given: false,
        }
    }
}

/// Error returned when [`layout_text`] encounters a character with no code in
/// the target [`LayoutFont`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("character {character:?} at byte {at} cannot be encoded in this font")]
pub struct UnencodableChar {
    /// UTF-8 byte offset of the unencodable character in the input `&str`.
    pub at: usize,
    /// The character that could not be encoded.
    pub character: char,
}

/// One positioned line (or comb cell row) produced by [`layout_text`].
///
/// Its `Debug` implementation prints byte lengths and geometry rather than raw
/// encoded string bytes.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct LaidOutLine {
    /// UTF-8 byte range in the source text covered by this line.
    pub text_range: Range<usize>,
    /// Baseline origin in PageSpace.
    pub origin: Point,
    /// Advance width of this line in PageSpace points (including justified word
    /// spacing when active).
    pub width: f32,
    /// Encoded character-code bytes for `Tj` (empty when `cells` is used for
    /// comb layout).
    pub codes: Vec<u8>,
    /// Extra word spacing (`Tw`) in text-space points (`0.0` unless justified).
    pub word_space: f32,
    /// Individual `(origin, codes)` placements when laid out into `comb` cells.
    pub cells: Vec<(Point, Vec<u8>)>,
}

impl fmt::Debug for LaidOutLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaidOutLine")
            .field("text_range", &self.text_range)
            .field("origin", &self.origin)
            .field("width", &self.width)
            .field("codes_len", &self.codes.len())
            .field("word_space", &self.word_space)
            .field("cells_len", &self.cells.len())
            .finish()
    }
}

/// Positioned multiline or comb text layout computed by [`layout_text`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LaidOutText {
    /// Chosen font size in points.
    pub font_size: f32,
    /// Baseline-to-baseline step in points.
    pub line_height: f32,
    /// Inner content rectangle after applying `padding`.
    pub bounds: Rect,
    /// `true` if the laid-out lines exceeded `bounds` horizontally or
    /// vertically (or exceeded `comb` cell count).
    pub overflowed: bool,
    /// Positioned lines from top to bottom.
    pub lines: Vec<LaidOutLine>,
    pub(crate) mode: TextMode,
}

#[derive(Clone)]
struct EncodedGlyph {
    byte_start: usize,
    byte_end: usize,
    ch: char,
    units: f32,
    codes: [u8; 4],
    codes_len: u8,
}

/// Lays out `text` inside `rect` using `font` and `style`.
///
/// Supports `\n` and `\r\n` paragraph breaks, `Word` / `Char` / `NoWrap`
/// wrapping, `Left` / `Center` / `Right` / `Justify` horizontal alignment,
/// `Top` / `Middle` / `Bottom` vertical alignment, automatic font sizing when
/// `style.size` is `None` or `<= 0.0`, and fixed-cell `comb` layout.
///
/// # Errors
///
/// Returns [`UnencodableChar`] for the first character (other than `\r` and
/// `\n`, or `\t` which is normalized to `' '`) that `font` cannot encode.
pub fn layout_text<F: LayoutFont + ?Sized>(
    font: &F,
    text: &str,
    rect: Rect,
    style: &TextBoxStyle,
) -> Result<LaidOutText, UnencodableChar> {
    let norm_rect = Rect::new(rect.x0, rect.y0, rect.x1, rect.y1);
    let pad = style.padding.max(0.0);
    let inner = norm_rect.inset(pad, pad).unwrap_or_else(|| {
        let cx = 0.5 * (norm_rect.x0 + norm_rect.x1);
        let cy = 0.5 * (norm_rect.y0 + norm_rect.y1);
        Rect::new(cx, cy, cx, cy)
    });

    // Pre-encode every scalar (validating encodability upfront even if `rect` is
    // tiny or `text` wraps).
    let mut paragraphs: Vec<(Range<usize>, Vec<EncodedGlyph>)> = Vec::new();
    let mut para_start = 0usize;
    let mut cur_glyphs: Vec<EncodedGlyph> = Vec::new();
    let mut chars_iter = text.char_indices().peekable();
    let mut tmp_codes = Vec::with_capacity(4);

    while let Some((idx, ch)) = chars_iter.next() {
        if ch == '\r' {
            let next_nl = chars_iter.peek().is_some_and(|&(_, c)| c == '\n');
            paragraphs.push((para_start..idx, std::mem::take(&mut cur_glyphs)));
            if next_nl {
                let (nl_idx, _) = chars_iter.next().unwrap();
                para_start = nl_idx + 1;
            } else {
                para_start = idx + 1;
            }
            continue;
        }
        if ch == '\n' {
            paragraphs.push((para_start..idx, std::mem::take(&mut cur_glyphs)));
            para_start = idx + 1;
            continue;
        }
        let mapped = if ch == '\t' { ' ' } else { ch };
        let Some(units) = font.char_advance(mapped) else {
            return Err(UnencodableChar {
                at: idx,
                character: ch,
            });
        };
        tmp_codes.clear();
        if !font.encode_char(mapped, &mut tmp_codes) || tmp_codes.len() > 4 {
            return Err(UnencodableChar {
                at: idx,
                character: ch,
            });
        }
        let mut arr = [0u8; 4];
        arr[..tmp_codes.len()].copy_from_slice(&tmp_codes);
        cur_glyphs.push(EncodedGlyph {
            byte_start: idx,
            byte_end: idx + ch.len_utf8(),
            ch: mapped,
            units,
            codes: arr,
            codes_len: tmp_codes.len() as u8,
        });
    }
    paragraphs.push((para_start..text.len(), cur_glyphs));

    let (ascent_em, descent_em, height_em) =
        line_heights(font.font_metrics(), style.font_heights_as_given);

    if let Some(comb_n) = style.comb.filter(|&n| n > 0) {
        return Ok(layout_comb(
            &paragraphs,
            text.len(),
            inner,
            style,
            comb_n,
            ascent_em,
            descent_em,
            height_em,
        ));
    }

    let explicit_size = style.size.filter(|&s| s > 0.0 && s.is_finite());
    let chosen_size = match explicit_size {
        Some(s) => s,
        None => choose_auto_size(&paragraphs, inner, style, ascent_em, descent_em, height_em),
    };

    Ok(build_lines_at_size(
        &paragraphs,
        inner,
        style,
        chosen_size,
        ascent_em,
        descent_em,
        height_em,
    ))
}

/// The ascent, the descent, and the height between them, per em, that layout
/// measures lines by: `metrics`' own where `as_given` and they give a height
/// (both finite, the ascent above the descent), else floored to at least
/// `0.70`, at most `-0.15`, and at least `0.85`
/// ([`TextBoxStyle::font_heights_as_given`]).
fn line_heights(metrics: FontMetrics, as_given: bool) -> (f32, f32, f32) {
    let ascent = metrics.ascent / 1000.0;
    let descent = metrics.descent / 1000.0;
    // Distinct finite numbers differ by more than nothing, so the height
    // is above 0.
    if as_given && ascent.is_finite() && descent.is_finite() && ascent > descent {
        return (ascent, descent, ascent - descent);
    }
    let ascent_em = ascent.max(0.70);
    let descent_em = descent.min(-0.15);
    (ascent_em, descent_em, (ascent_em - descent_em).max(0.85))
}

#[allow(clippy::too_many_arguments)]
fn layout_comb(
    paragraphs: &[(Range<usize>, Vec<EncodedGlyph>)],
    total_bytes: usize,
    inner: Rect,
    style: &TextBoxStyle,
    comb_n: u32,
    ascent_em: f32,
    descent_em: f32,
    height_em: f32,
) -> LaidOutText {
    let glyphs: Vec<&EncodedGlyph> = paragraphs.iter().flat_map(|(_, g)| g.iter()).collect();
    let cell_w = inner.width() / (comb_n as f32);
    let font_size = match style.size.filter(|&s| s > 0.0 && s.is_finite()) {
        Some(s) => s,
        None => {
            let min_s = style.min_auto_size.max(1.0);
            let max_s = style.max_auto_size.max(min_s);
            let fit_h = inner.height() / height_em;
            let fit_w = cell_w / 0.75;
            fit_h.min(fit_w).clamp(min_s, max_s)
        }
    };
    let line_h = style
        .line_height
        .filter(|&lh| lh > 0.0)
        .unwrap_or(height_em * font_size * style.leading_factor.max(0.5));

    let count = glyphs.len().min(comb_n as usize);
    let overflowed =
        glyphs.len() > comb_n as usize || height_em * font_size > inner.height() + 1e-3;
    let start_cell = match style.align {
        TextAlign::Right => (comb_n as usize).saturating_sub(count),
        TextAlign::Center => ((comb_n as usize).saturating_sub(count)) / 2,
        TextAlign::Left | TextAlign::Justify => 0,
    };

    let baseline_y = single_line_baseline(
        inner,
        style.vertical_align,
        font_size,
        ascent_em,
        descent_em,
    );
    let mut cells = Vec::with_capacity(count);
    for (i, g) in glyphs.iter().take(count).enumerate() {
        let cell_idx = start_cell + i;
        let glyph_w = g.units * font_size / 1000.0;
        let cx = inner.x0 + (cell_idx as f32) * cell_w + 0.5 * (cell_w - glyph_w);
        cells.push((
            Point::new(cx, baseline_y),
            g.codes[..usize::from(g.codes_len)].to_vec(),
        ));
    }

    let lines = if cells.is_empty() {
        Vec::new()
    } else {
        let origin = cells[0].0;
        vec![LaidOutLine {
            text_range: 0..total_bytes,
            origin,
            width: (count as f32) * cell_w,
            codes: Vec::new(),
            word_space: 0.0,
            cells,
        }]
    };

    LaidOutText {
        font_size,
        line_height: line_h,
        bounds: inner,
        overflowed,
        lines,
        mode: style.mode,
    }
}

fn single_line_baseline(
    inner: Rect,
    valign: VerticalAlign,
    font_size: f32,
    ascent_em: f32,
    descent_em: f32,
) -> f32 {
    let asc = ascent_em * font_size;
    let desc = descent_em * font_size;
    match valign {
        VerticalAlign::Top => inner.y1 - asc,
        VerticalAlign::Middle => 0.5 * (inner.y0 + inner.y1) - 0.5 * (asc + desc),
        VerticalAlign::Bottom => inner.y0 - desc,
    }
}

fn choose_auto_size(
    paragraphs: &[(Range<usize>, Vec<EncodedGlyph>)],
    inner: Rect,
    style: &TextBoxStyle,
    ascent_em: f32,
    descent_em: f32,
    height_em: f32,
) -> f32 {
    let min_s = style.min_auto_size.max(0.5);
    let max_s = style.max_auto_size.max(min_s);
    if inner.width() <= 0.0 || inner.height() <= 0.0 {
        return min_s;
    }

    // The ceiling itself where the text fits at it: the search below only
    // closes on it from under.
    let fits = |size: f32| {
        !build_lines_at_size(
            paragraphs, inner, style, size, ascent_em, descent_em, height_em,
        )
        .overflowed
    };
    if fits(max_s) {
        return max_s;
    }

    let mut lo = min_s;
    let mut hi = max_s;
    for _ in 0..16 {
        let mid = 0.5 * (lo + hi);
        let trial = build_lines_at_size(
            paragraphs, inner, style, mid, ascent_em, descent_em, height_em,
        );
        if !trial.overflowed {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

struct WrappedSlice<'a> {
    range: Range<usize>,
    glyphs: &'a [EncodedGlyph],
    is_para_last: bool,
}

/// `glyphs`, a paragraph's, cut into lines no wider than `max_width` by
/// `wrap`, the spaces kept where `keep_spaces`
/// ([`TextBoxStyle::line_end_spaces`]).
fn wrap_paragraph<'a>(
    para_range: &Range<usize>,
    glyphs: &'a [EncodedGlyph],
    max_width: f32,
    font_size: f32,
    wrap: WrapMode,
    keep_spaces: bool,
) -> Vec<WrappedSlice<'a>> {
    if glyphs.is_empty() {
        return vec![WrappedSlice {
            range: para_range.clone(),
            glyphs: &[],
            is_para_last: true,
        }];
    }
    if wrap == WrapMode::NoWrap || max_width <= 0.0 {
        return vec![WrappedSlice {
            range: glyphs[0].byte_start..glyphs.last().unwrap().byte_end,
            glyphs,
            is_para_last: true,
        }];
    }

    let scale = font_size / 1000.0;
    let mut out = Vec::new();
    let mut start = 0usize;
    let n = glyphs.len();

    while start < n {
        // Skip leading spaces on wrapped continuation lines (not on index 0 of
        // the paragraph so intentional leading indentation is preserved).
        if start > 0 && !keep_spaces {
            while start < n && glyphs[start].ch == ' ' {
                start += 1;
            }
            if start >= n {
                break;
            }
        }

        let mut width = 0.0f32;
        let mut end = start;
        let mut last_break: Option<usize> = None;

        while end < n {
            let gw = glyphs[end].units * scale;
            // Keeping spaces, a line breaks before a word, which a character
            // that would pass the width may itself begin.
            if keep_spaces && wrap == WrapMode::Word && end > start && begins_word(glyphs, end) {
                last_break = Some(end);
            }
            if end > start && width + gw > max_width + 1e-4 {
                // If this glyph is a space, break right here and consume trailing spaces.
                if !keep_spaces && glyphs[end].ch == ' ' {
                    last_break = Some(end);
                }
                break;
            }
            width += gw;
            if wrap == WrapMode::Word && !keep_spaces {
                if glyphs[end].ch == ' ' {
                    last_break = Some(end);
                } else if glyphs[end].ch == '-' && end + 1 < n {
                    last_break = Some(end + 1);
                }
            }
            end += 1;
        }

        if end == n {
            let kept = &glyphs[start..end];
            let trimmed = if keep_spaces {
                kept
            } else {
                trim_trailing_spaces(kept)
            };
            let range =
                slice_byte_range(trimmed, glyphs[start].byte_start..glyphs[end - 1].byte_end);
            out.push(WrappedSlice {
                range,
                glyphs: trimmed,
                is_para_last: true,
            });
            break;
        }

        let split_at = match wrap {
            WrapMode::Word => last_break
                .filter(|&b| b > start)
                .unwrap_or(end.max(start + 1)),
            WrapMode::Char => end.max(start + 1),
            WrapMode::NoWrap => n,
        };

        let raw_slice = &glyphs[start..split_at];
        let trimmed = if keep_spaces {
            raw_slice
        } else {
            trim_trailing_spaces(raw_slice)
        };
        let fallback_start = raw_slice[0].byte_start;
        let fallback_end = raw_slice.last().unwrap().byte_end;
        let range = slice_byte_range(trimmed, fallback_start..fallback_end);
        out.push(WrappedSlice {
            range,
            glyphs: trimmed,
            is_para_last: false,
        });
        start = split_at;
    }

    if let Some(last) = out.last_mut() {
        last.is_para_last = true;
    }
    out
}

/// Whether a word begins at `glyphs[i]`, for a line that keeps its spaces,
/// as PDFium's line breaking finds one as of chromium/8152
/// (`core/fpdfdoc/cpvt_section.cpp`, `SplitLines` and `NeedDivision`): at an
/// opening bracket, or where the character before it and it divide
/// ([`divides`]); but not inside a bracket just opened, where the spaces
/// after it and the first character after those carry on its word.
fn begins_word(glyphs: &[EncodedGlyph], i: usize) -> bool {
    let Some(prev) = i.checked_sub(1).map(|p| glyphs[p].ch) else {
        return false;
    };
    let ch = glyphs[i].ch;
    if !(opens(ch) || divides(prev, ch)) {
        return false;
    }
    // A space is never a word's start, so only a word's first character
    // looks back, and only over the spaces just before it.
    let opened = glyphs[..i]
        .iter()
        .rev()
        .map(|g| g.ch)
        .find(|&c| !is_space(c))
        .is_some_and(opens);
    !opened
}

/// Whether a line may break between `prev` and `ch`, `ch` beginning a word:
/// never between two letters or digits, before a space or punctuation, or
/// next to punctuation that joins what's about it (an apostrophe, comma,
/// hyphen, full stop or solidus); always after a space or other
/// punctuation; never after a currency sign; always before one or an
/// ideograph, and after an ideograph.
pub(crate) fn divides(prev: char, ch: char) -> bool {
    // Two letters or digits never divide: every rule below that does needs
    // a space, punctuation, a currency sign, or an ideograph.
    if is_space(ch) || is_punctuation(ch) || joins(prev) || joins(ch) {
        return false;
    }
    if is_space(prev) || is_punctuation(prev) {
        return true;
    }
    if is_prefix(prev) {
        return false;
    }
    is_prefix(ch) || is_ideograph(ch) || is_ideograph(prev)
}

/// A space, for line breaking: U+0020 and the ideographic space.
fn is_space(ch: char) -> bool {
    matches!(ch, ' ' | '\u{3000}')
}

/// Punctuation, for line breaking: a line may break after it, and before
/// it never.
fn is_punctuation(ch: char) -> bool {
    matches!(ch,
        '\u{1}'..='\u{4}'
        | '!' | '"' | '\'' | '(' | ')' | ',' | '-' | '.' | '/' | ':' | ';' | '?'
        | '[' | ']' | '{' | '}'
        | '\u{82}' | '\u{84}' | '\u{85}' | '\u{91}'..='\u{94}' | '\u{96}' | '\u{B4}' | '\u{B8}'
        | '\u{2010}'..='\u{2013}'
        | '\u{2018}'..='\u{201F}'
        | '\u{2032}'..='\u{2037}'
        | '\u{203C}'..='\u{203E}'
        | '\u{2044}'
        | '\u{3001}'..='\u{3003}'
        | '\u{3005}'
        | '\u{3009}'..='\u{3011}'
        | '\u{3014}'..='\u{301B}'
        | '\u{301D}'..='\u{301F}'
        | '\u{FE50}'..='\u{FE5E}'
        | '\u{FE63}'
        | '\u{FF01}' | '\u{FF02}' | '\u{FF07}'..='\u{FF09}' | '\u{FF0C}' | '\u{FF0E}' | '\u{FF0F}'
        | '\u{FF1A}' | '\u{FF1B}' | '\u{FF1F}' | '\u{FF3B}' | '\u{FF3D}' | '\u{FF40}'
        | '\u{FF5B}'..='\u{FF5D}'
        | '\u{FF61}'..='\u{FF65}'
        | '\u{FF9E}' | '\u{FF9F}')
}

/// Punctuation that joins the characters about it, so no line breaks next
/// to it: the apostrophe, comma, hyphen, full stop and solidus (and
/// U+0006).
fn joins(ch: char) -> bool {
    matches!(ch, '\u{6}' | '\'' | ',' | '-' | '.' | '/')
}

/// An opening bracket, which begins a word wherever it isn't inside one
/// just opened.
fn opens(ch: char) -> bool {
    matches!(
        ch,
        '\u{1}'
            | '\u{3}'
            | '('
            | '['
            | '{'
            | '\u{300A}'
            | '\u{300C}'
            | '\u{300E}'
            | '\u{3010}'
            | '\u{3014}'
            | '\u{3016}'
            | '\u{3018}'
            | '\u{301A}'
            | '\u{FF08}'
            | '\u{FF3B}'
            | '\u{FF5B}'
            | '\u{FF62}'
    )
}

/// A sign written before a number (a currency sign, or the numero sign),
/// which no line breaks after.
fn is_prefix(ch: char) -> bool {
    matches!(ch,
        '$' | '\u{80}' | '\u{A2}'..='\u{A5}' | '\u{20A0}'..='\u{20CF}' | '\u{2116}' | '\u{FE69}'
        | '\u{FF04}' | '\u{FFE0}' | '\u{FFE1}' | '\u{FFE5}' | '\u{FFE6}')
}

/// An ideograph or syllable of Chinese, Japanese or Korean, each a word of
/// its own for line breaking.
fn is_ideograph(ch: char) -> bool {
    matches!(ch,
        '\u{1100}'..='\u{11FF}'
        | '\u{2E80}'..='\u{2FFF}'
        | '\u{3005}' | '\u{3006}' | '\u{3021}'..='\u{3029}' | '\u{3031}'..='\u{3035}'
        | '\u{3040}'..='\u{9FBF}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE30}'..='\u{FE4F}'
        | '\u{FF66}'..='\u{FF9D}'
        | '\u{20000}'..='\u{2A6DF}'
        | '\u{2F800}'..='\u{2FA1F}')
}

/// How many of `glyphs` are spaces that `Tw` widens: a space drawn as the
/// single byte 32 (ISO 32000-2, 9.3.3).
fn word_spaces(glyphs: &[EncodedGlyph]) -> usize {
    glyphs
        .iter()
        .filter(|g| g.ch == ' ' && g.codes_len == 1 && g.codes[0] == b' ')
        .count()
}

fn trim_trailing_spaces(slice: &[EncodedGlyph]) -> &[EncodedGlyph] {
    let mut end = slice.len();
    while end > 0 && slice[end - 1].ch == ' ' {
        end -= 1;
    }
    &slice[..end]
}

fn slice_byte_range(trimmed: &[EncodedGlyph], fallback: Range<usize>) -> Range<usize> {
    match (trimmed.first(), trimmed.last()) {
        (Some(f), Some(l)) => f.byte_start..l.byte_end,
        _ => fallback.start..fallback.start,
    }
}

fn build_lines_at_size(
    paragraphs: &[(Range<usize>, Vec<EncodedGlyph>)],
    inner: Rect,
    style: &TextBoxStyle,
    font_size: f32,
    ascent_em: f32,
    descent_em: f32,
    height_em: f32,
) -> LaidOutText {
    let max_w = inner.width();
    let max_h = inner.height();
    let line_h = style
        .line_height
        .filter(|&lh| lh > 0.0 && lh.is_finite())
        .unwrap_or(height_em * font_size * style.leading_factor.max(0.5));

    let mut wrapped: Vec<WrappedSlice<'_>> = Vec::new();
    for (p_range, p_glyphs) in paragraphs {
        wrapped.extend(wrap_paragraph(
            p_range,
            p_glyphs,
            max_w,
            font_size,
            style.wrap,
            style.line_end_spaces,
        ));
    }

    let line_count = wrapped.len();
    let asc = ascent_em * font_size;
    let desc = descent_em * font_size;
    let total_block_h = if line_count == 0 {
        0.0
    } else {
        asc - desc + (line_count.saturating_sub(1) as f32) * line_h
    };

    let first_baseline_y = if line_count <= 1 {
        single_line_baseline(
            inner,
            style.vertical_align,
            font_size,
            ascent_em,
            descent_em,
        )
    } else {
        match style.vertical_align {
            VerticalAlign::Top => inner.y1 - asc,
            VerticalAlign::Middle => 0.5 * (inner.y0 + inner.y1) + 0.5 * total_block_h - asc,
            VerticalAlign::Bottom => {
                inner.y0 - desc + (line_count.saturating_sub(1) as f32) * line_h
            }
        }
    };

    let mut overflowed = total_block_h > max_h + 1e-3;
    let scale = font_size / 1000.0;
    let mut lines = Vec::with_capacity(line_count);

    for (i, ws) in wrapped.into_iter().enumerate() {
        let raw_w: f32 = ws.glyphs.iter().map(|g| g.units * scale).sum();
        if raw_w > max_w + 1e-3 {
            overflowed = true;
        }
        let spaces = word_spaces(ws.glyphs);

        let (x, word_space, final_w) = match style.align {
            TextAlign::Left => (inner.x0, 0.0, raw_w),
            TextAlign::Center => (inner.x0 + 0.5 * (max_w - raw_w), 0.0, raw_w),
            TextAlign::Right => (inner.x1 - raw_w, 0.0, raw_w),
            // The spaces a line ends with show nothing: the line's words are
            // spread to the box's edge by the spaces between them, and the
            // `Tw` that widens those widens the ones at its end too.
            TextAlign::Justify if style.line_end_spaces && !ws.is_para_last => {
                let words = trim_trailing_spaces(ws.glyphs);
                let words_w: f32 = words.iter().map(|g| g.units * scale).sum();
                let between = word_spaces(words);
                if between > 0 && max_w > words_w {
                    let extra = (max_w - words_w) / (between as f32);
                    (inner.x0, extra, raw_w + extra * (spaces as f32))
                } else {
                    (inner.x0, 0.0, raw_w)
                }
            }
            TextAlign::Justify if !ws.is_para_last && spaces > 0 && max_w > raw_w => {
                let extra = (max_w - raw_w) / (spaces as f32);
                (inner.x0, extra, max_w)
            }
            TextAlign::Justify => (inner.x0, 0.0, raw_w),
        };

        let y = first_baseline_y - (i as f32) * line_h;
        let mut codes = Vec::with_capacity(ws.glyphs.len());
        for g in ws.glyphs {
            codes.extend_from_slice(&g.codes[..usize::from(g.codes_len)]);
        }

        lines.push(LaidOutLine {
            text_range: ws.range,
            origin: Point::new(x, y),
            width: final_w,
            codes,
            word_space,
            cells: Vec::new(),
        });
    }

    LaidOutText {
        font_size,
        line_height: line_h,
        bounds: inner,
        overflowed,
        lines,
        mode: style.mode,
    }
}
