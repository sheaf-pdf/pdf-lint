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
    /// `(ascent - descent) / 1000 * font_size * leading_factor`).
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

    let metrics = font.font_metrics();
    let ascent_em = (metrics.ascent / 1000.0).max(0.70);
    let descent_em = (metrics.descent / 1000.0).min(-0.15);
    let height_em = (ascent_em - descent_em).max(0.85);

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

fn wrap_paragraph<'a>(
    para_range: &Range<usize>,
    glyphs: &'a [EncodedGlyph],
    max_width: f32,
    font_size: f32,
    wrap: WrapMode,
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
        if start > 0 {
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
            if end > start && width + gw > max_width + 1e-4 {
                // If this glyph is a space, break right here and consume trailing spaces.
                if glyphs[end].ch == ' ' {
                    last_break = Some(end);
                }
                break;
            }
            width += gw;
            if wrap == WrapMode::Word {
                if glyphs[end].ch == ' ' {
                    last_break = Some(end);
                } else if glyphs[end].ch == '-' && end + 1 < n {
                    last_break = Some(end + 1);
                }
            }
            end += 1;
        }

        if end == n {
            let trimmed = trim_trailing_spaces(&glyphs[start..end]);
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
        let trimmed = trim_trailing_spaces(raw_slice);
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
            p_range, p_glyphs, max_w, font_size, style.wrap,
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
        let spaces = ws
            .glyphs
            .iter()
            .filter(|g| g.ch == ' ' && g.codes_len == 1 && g.codes[0] == b' ')
            .count();

        let (x, word_space, final_w) = match style.align {
            TextAlign::Left => (inner.x0, 0.0, raw_w),
            TextAlign::Center => (inner.x0 + 0.5 * (max_w - raw_w), 0.0, raw_w),
            TextAlign::Right => (inner.x1 - raw_w, 0.0, raw_w),
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
