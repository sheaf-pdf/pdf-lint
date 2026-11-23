//! Multiline paragraph wrapping, alignment, auto-sizing, and comb-cell layout
//! (`feature = "text-layout"`).
//!
//! [`layout_text`] lays out UTF-8 text inside a target [`Rect`] over any
//! [`LayoutFont`] ([`Std14`], [`Font`], or [`FontEncoder`]), or a [`FontSet`]
//! of several drawn in order of preference ([`WithFallback`]), supporting word
//! or character wrapping, left/center/right/justified alignment,
//! top/middle/bottom vertical alignment, automatic font sizing (`size: None`),
//! and fixed-width comb cells (`comb: Some(n)`), a character or a grapheme
//! cluster at a time ([`TextBoxStyle::clusters`]).
//! [`Canvas::draw_layout`](crate::Canvas::draw_layout),
//! [`Canvas::draw_layout_in`](crate::Canvas::draw_layout_in) and
//! [`Canvas::text_box`](crate::Canvas::text_box) emit the resulting lines as
//! position-independent `Tm` + `Tj` operations.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use pdf_content_streams::{Res, TextMode};
use pdf_fonts::{Encoding, Font, FontMetrics, Std14};
use unicode_segmentation::UnicodeSegmentation;

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

/// The character Symbol's encoding has a code for where text has `ch`
/// ([`Encoding::code`]), or `ch`. Five of its glyphs are written in text as
/// characters the encoding's codes don't read back as:
///
/// - `Delta`, `Omega` and `mu` are Greek letters, as the alphabet around them
///   is, but their codes' text is the signs ∆ (U+2206), Ω (U+2126) and
///   µ (U+00B5), as the Adobe Glyph List reads those names. A keyboard gives
///   the letters Δ (U+0394), Ω (U+03A9) and μ (U+03BC), and composed text has
///   no U+2126 at all: its canonical composition is U+03A9.
/// - `angleleft` and `angleright` read as U+2329 and U+232A, whose canonical
///   compositions are U+3008 and U+3009, so composed text has those alone.
///
/// Text composed before it's laid out (as a combining accent needs:
/// [`Encoding::code`]) would otherwise lose the omega sign and the angle
/// brackets it had before composing.
fn symbol_character(ch: char) -> char {
    match ch {
        '\u{0394}' => '\u{2206}',
        '\u{03A9}' => '\u{2126}',
        '\u{03BC}' => '\u{00B5}',
        '\u{3008}' => '\u{2329}',
        '\u{3009}' => '\u{232A}',
        ch => ch,
    }
}

/// The code that draws `ch` in a standard 14 font, in the encoding its
/// dictionary names ([`Std14::dict`]), with that encoding.
fn std14_code(font: Std14, ch: char) -> Option<(Encoding, u8)> {
    let enc = std14_encoding(font);
    let ch = match enc {
        Encoding::Symbol => symbol_character(ch),
        _ => ch,
    };
    Some((enc, enc.code(ch)?))
}

/// A standard 14 font lays text out in the encoding its dictionary names
/// ([`Std14::dict`]): WinAnsiEncoding for the twelve Latin fonts, their own for
/// Symbol and ZapfDingbats, each character the code [`Encoding::code`] gives it.
/// Symbol also takes the Greek letters Δ (U+0394), Ω (U+03A9) and μ (U+03BC)
/// and the angle brackets U+3008 and U+3009, at the codes of the signs ∆
/// (U+2206), Ω (U+2126) and µ (U+00B5) and of U+2329 and U+232A, which draw
/// them: so the letters a keyboard gives are drawn, and text composed (NFC)
/// keeps its omega sign and angle brackets. Those five codes read back as the
/// signs, not as the characters given.
impl LayoutFont for Std14 {
    fn font_metrics(&self) -> FontMetrics {
        self.metrics()
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        let (enc, code) = std14_code(*self, ch)?;
        let glyph = enc.glyph(code)?;
        self.glyph_width(glyph)
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        let Some((_, code)) = std14_code(*self, ch) else {
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

/// What [`layout_text`] lays text out in: one font, or several in order of
/// preference, each a face, counted from `0`.
///
/// Each unit of the text (a character, or a grapheme cluster with
/// [`TextBoxStyle::clusters`]) is drawn in the first face that has an advance
/// and a code for every character of it ([`face_for`]), so a font that lacks
/// a character falls back to the faces after it. Every [`LayoutFont`] is a
/// set of one face, so [`layout_text`] and
/// [`Canvas::text_box`](crate::Canvas::text_box) take one as they always
/// have; [`WithFallback`] puts two sets one after the other, and nests.
///
/// Each method's `face` counts from `0` below [`faces`](Self::faces): asked
/// of a face past them, a set gives no advance and no code, and metrics of
/// its own choosing. The methods' names begin `face_` so that a font's own
/// (`Std14::metrics`) keep theirs wherever the trait is in scope.
pub trait FontSet {
    /// How many faces the set has.
    fn faces(&self) -> usize;

    /// The ascent, descent, and font bounding box of `face`, per 1000 em
    /// units.
    fn face_metrics(&self, face: usize) -> FontMetrics;

    /// The advance width of `ch` in `face`, in 1/1000 em units, or `None`
    /// where `face` has no code for it.
    fn face_advance(&self, face: usize, ch: char) -> Option<f32>;

    /// Appends the code that draws `ch` in `face` to `out`, returning `false`
    /// without changing `out` where `face` can't encode it.
    fn face_encode(&self, face: usize, ch: char, out: &mut Vec<u8>) -> bool;

    /// The resource `face` is drawn under, where the set knows it (`None` by
    /// default). [`Canvas::text_box`](crate::Canvas::text_box) draws the
    /// first face under the resource it's given and every other under the
    /// set's own answer, so text in several fonts needs no list of resources
    /// beside the set.
    fn face_resource(&self, face: usize) -> Option<Res> {
        let _ = face;
        None
    }
}

/// A font is a set of one face, whose resource the caller names.
impl<T: LayoutFont + ?Sized> FontSet for T {
    fn faces(&self) -> usize {
        1
    }

    fn face_metrics(&self, _face: usize) -> FontMetrics {
        self.font_metrics()
    }

    fn face_advance(&self, face: usize, ch: char) -> Option<f32> {
        match face {
            0 => self.char_advance(ch),
            _ => None,
        }
    }

    fn face_encode(&self, face: usize, ch: char, out: &mut Vec<u8>) -> bool {
        face == 0 && self.encode_char(ch, out)
    }
}

/// Two sets of fonts, one after the other: `first`'s faces, then `second`'s,
/// each drawn where the faces before it can't draw a unit of the text. The
/// second set's first face is drawn under the resource given for it, and
/// every other face under its own set's answer ([`FontSet::face_resource`]).
///
/// Sets nest, so three fonts or more are a `WithFallback` whose first or
/// second set is one: `WithFallback::new(&WithFallback::new(&a, &b, b_res),
/// &c, c_res)` draws in `a`, then `b`, then `c`, as
/// `WithFallback::new(&a, &WithFallback::new(&b, &c, c_res), b_res)` does.
///
/// Its `Debug` gives how many faces it has, never a resource.
///
/// ```
/// use pdf_gfx::pdf_content_streams::Res;
/// use pdf_gfx::pdf_fonts::Std14;
/// use pdf_gfx::{Rect, TextBoxStyle, WithFallback, layout_text};
///
/// let symbol = Res::dict(Std14::Symbol.dict())?;
/// let fonts = WithFallback::new(&Std14::Helvetica, &Std14::Symbol, symbol);
/// let laid = layout_text(
///     &fonts,
///     "Area \u{3C0}r\u{B2}",
///     Rect::from_xywh(0.0, 0.0, 200.0, 20.0),
///     &TextBoxStyle::default(),
/// )?;
///
/// // Helvetica draws all but the pi, which falls back to Symbol.
/// let runs: Vec<usize> = laid.lines[0].runs.iter().map(|run| run.face).collect();
/// assert_eq!(runs, [0, 1, 0]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct WithFallback<'a, A: ?Sized, B: ?Sized> {
    first: &'a A,
    second: &'a B,
    second_res: Res,
}

impl<'a, A: FontSet + ?Sized, B: FontSet + ?Sized> WithFallback<'a, A, B> {
    /// `first`'s faces, then `second`'s, the first of `second`'s drawn under
    /// `second_res`.
    #[must_use]
    pub fn new(first: &'a A, second: &'a B, second_res: Res) -> Self {
        WithFallback {
            first,
            second,
            second_res,
        }
    }

    /// Which of the two sets `face` is in, and its face there.
    fn of(&self, face: usize) -> Result<usize, usize> {
        let firsts = self.first.faces();
        match face.checked_sub(firsts) {
            None => Ok(face),
            Some(face) => Err(face),
        }
    }
}

impl<A: ?Sized, B: ?Sized> Clone for WithFallback<'_, A, B> {
    fn clone(&self) -> Self {
        WithFallback {
            first: self.first,
            second: self.second,
            second_res: self.second_res.clone(),
        }
    }
}

impl<A: FontSet + ?Sized, B: FontSet + ?Sized> fmt::Debug for WithFallback<'_, A, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WithFallback")
            .field("faces", &self.faces())
            .finish()
    }
}

impl<A: FontSet + ?Sized, B: FontSet + ?Sized> FontSet for WithFallback<'_, A, B> {
    fn faces(&self) -> usize {
        self.first.faces().saturating_add(self.second.faces())
    }

    fn face_metrics(&self, face: usize) -> FontMetrics {
        match self.of(face) {
            Ok(face) => self.first.face_metrics(face),
            Err(face) => self.second.face_metrics(face),
        }
    }

    fn face_advance(&self, face: usize, ch: char) -> Option<f32> {
        match self.of(face) {
            Ok(face) => self.first.face_advance(face, ch),
            Err(face) => self.second.face_advance(face, ch),
        }
    }

    fn face_encode(&self, face: usize, ch: char, out: &mut Vec<u8>) -> bool {
        match self.of(face) {
            Ok(face) => self.first.face_encode(face, ch, out),
            Err(face) => self.second.face_encode(face, ch, out),
        }
    }

    fn face_resource(&self, face: usize) -> Option<Res> {
        match self.of(face) {
            Ok(face) => self.first.face_resource(face),
            Err(0) => Some(self.second_res.clone()),
            Err(face) => self.second.face_resource(face),
        }
    }
}

/// The face of `font` that draws `unit`, a character or a grapheme cluster
/// of text, as [`layout_text`] chooses it: the first that has an advance and
/// a code of at most four bytes for every character of it, a `\t` taken as a
/// space. An empty unit is the first face's.
///
/// It encodes as layout does, so a font that records what it encodes (a
/// `ProgramFont`, with `embed`) records the unit's glyphs in the face that
/// draws it, and where an earlier face has an advance for every character
/// of the unit but can't encode one, what that face encoded before it.
///
/// # Errors
///
/// [`UnencodableChar`] where no face draws all of `unit`, at the byte offset
/// in `unit` of the first of its characters the first face has no advance
/// for, else of the first it can't encode: the character a layout in that
/// font alone would refuse.
pub fn face_for<F: FontSet + ?Sized>(font: &F, unit: &str) -> Result<usize, UnencodableChar> {
    encode_unit(font, unit.char_indices(), &mut Vec::new(), &mut Vec::new())
}

/// The characters of a unit, at their byte offsets in the text, encoded
/// into `out` in the face [`face_for`] chooses, which it returns.
fn encode_unit<F, I>(
    font: &F,
    unit: I,
    out: &mut Vec<EncodedGlyph>,
    tmp_codes: &mut Vec<u8>,
) -> Result<usize, UnencodableChar>
where
    F: FontSet + ?Sized,
    I: Iterator<Item = (usize, char)> + Clone,
{
    let start = out.len();
    let mut refused = None;
    for face in 0..font.faces() {
        match encode_in(font, face, unit.clone(), out, tmp_codes) {
            Ok(()) => return Ok(face),
            Err(err) => {
                out.truncate(start);
                refused.get_or_insert(err);
            }
        }
    }
    let refused = refused.or_else(|| {
        let (at, character) = unit.clone().next()?;
        Some(UnencodableChar { at, character })
    });
    match refused {
        Some(err) => Err(err),
        None => Ok(0),
    }
}

/// A unit's characters encoded into `out` in `face`: every advance asked
/// before any code, so a face that has none for one of them encodes
/// nothing; or the character it can't draw.
fn encode_in<F, I>(
    font: &F,
    face: usize,
    unit: I,
    out: &mut Vec<EncodedGlyph>,
    tmp_codes: &mut Vec<u8>,
) -> Result<(), UnencodableChar>
where
    F: FontSet + ?Sized,
    I: Iterator<Item = (usize, char)> + Clone,
{
    let first = out.len();
    let mut joined = false;
    for (at, ch) in unit.clone() {
        let mapped = if ch == '\t' { ' ' } else { ch };
        let Some(units) = font.face_advance(face, mapped) else {
            return Err(UnencodableChar { at, character: ch });
        };
        out.push(EncodedGlyph {
            byte_start: at,
            byte_end: at + ch.len_utf8(),
            ch: mapped,
            units,
            codes: [0; 4],
            codes_len: 0,
            face,
            joined,
        });
        joined = true;
    }
    for ((at, ch), glyph) in unit.zip(&mut out[first..]) {
        tmp_codes.clear();
        if !font.face_encode(face, glyph.ch, tmp_codes) || tmp_codes.len() > 4 {
            return Err(UnencodableChar { at, character: ch });
        }
        glyph.codes[..tmp_codes.len()].copy_from_slice(tmp_codes);
        glyph.codes_len = tmp_codes.len() as u8;
    }
    Ok(())
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
    /// On, a line's width is also found as PDFium finds it, each advance
    /// times the size and then times `0.001`, summed in `f32`, and a line
    /// passes the box's width where that sum is above it at all, so a line
    /// whose glyphs fill the box exactly breaks where PDFium breaks it: in
    /// Courier at 10 points, ten characters of 6 each sum past 60 and the
    /// tenth begins the next line, where at 12 points ten of 7.2 sum to 72
    /// and fit. Off, the advances are scaled by the size over `1000` and a
    /// line passes the width by more than `0.0001`.
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
    /// Whether the text is laid out an extended grapheme cluster at a time
    /// (Unicode Standard Annex #29), not a character at a time (`false` by
    /// default).
    ///
    /// On, a character and the marks that go with it are one unit: drawn in
    /// one face of a [`FontSet`], the first that has all of it
    /// ([`face_for`]); one comb cell, its codes one string centred in the
    /// cell by their advances together, and [`comb`](Self::comb) counting
    /// cells by clusters; and never split across lines, a word longer than
    /// its line broken between clusters alone. `\r` and `\n` end paragraphs
    /// before the text is cut into clusters, and `\t` is a space. Text whose
    /// every cluster is one character is laid out as with it off.
    pub clusters: bool,
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
            clusters: false,
        }
    }
}

/// Error returned when [`layout_text`] encounters a character with no code in
/// the target [`LayoutFont`], or a unit of text no face of a [`FontSet`] draws
/// whole ([`face_for`]).
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
    /// comb layout), in each face as [`runs`](Self::runs) says.
    pub codes: Vec<u8>,
    /// Extra word spacing (`Tw`) in text-space points (`0.0` unless justified).
    pub word_space: f32,
    /// Individual `(origin, codes)` placements when laid out into `comb` cells.
    pub cells: Vec<(Point, Vec<u8>)>,
    /// The runs of `codes` drawn in one face each, in order: one for a line
    /// in one face, none for an empty line or a comb line.
    pub runs: Vec<LaidOutRun>,
    /// The face each of `cells` is drawn in, beside it.
    pub cell_faces: Vec<usize>,
    /// The line's ascent in points: the largest of the faces drawn on it (an
    /// empty line's the first face's), as the heights
    /// [`TextBoxStyle::font_heights_as_given`] says.
    pub ascent: f32,
    /// The line's descent in points, below `0`: the smallest of the faces
    /// drawn on it (an empty line's the first face's).
    pub descent: f32,
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
            .field("runs", &self.runs)
            .field("cell_faces", &self.cell_faces)
            .field("ascent", &self.ascent)
            .field("descent", &self.descent)
            .finish()
    }
}

/// A run of a [`LaidOutLine`]'s codes drawn in one face of the [`FontSet`] it
/// was laid out in.
///
/// [`Canvas::draw_layout_in`](crate::Canvas::draw_layout_in) draws each under
/// its face's resource, placed by a `Tm` of its own.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LaidOutRun {
    /// The face it's drawn in.
    pub face: usize,
    /// The bytes of the line's `codes` it draws.
    pub codes: Range<usize>,
    /// The UTF-8 byte range in the source text it draws.
    pub text: Range<usize>,
    /// Its start from the line's origin, in points: the advances before it on
    /// the line, with the word spacing of the spaces among them.
    pub x: f32,
}

/// Positioned multiline or comb text layout computed by [`layout_text`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LaidOutText {
    /// Chosen font size in points.
    pub font_size: f32,
    /// Baseline-to-baseline step in points: between every two lines where
    /// they're all in one face, or the line height given; where they're in
    /// more than one, the step between two lines as tall as the first, each
    /// line's own heights giving the steps ([`LaidOutLine::ascent`],
    /// [`LaidOutLine::descent`]).
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
    /// The face it's drawn in.
    face: usize,
    /// Whether it carries on the unit of the glyph before it: a character
    /// of a grapheme cluster after its first ([`TextBoxStyle::clusters`]).
    joined: bool,
}

impl EncodedGlyph {
    fn codes(&self) -> &[u8] {
        &self.codes[..usize::from(self.codes_len)]
    }
}

/// Where the unit that begins at `glyphs[i]` ends: past the glyphs it joins.
fn unit_end(glyphs: &[EncodedGlyph], i: usize) -> usize {
    let mut end = i + 1;
    while glyphs.get(end).is_some_and(|g| g.joined) {
        end += 1;
    }
    end
}

/// Whether `glyphs[i]` is a space alone, a unit of its own.
fn is_space_unit(glyphs: &[EncodedGlyph], i: usize) -> bool {
    glyphs[i].ch == ' ' && !glyphs[i].joined && !glyphs.get(i + 1).is_some_and(|g| g.joined)
}

/// The heights, per em, layout measures a line by: its ascent and descent,
/// and the height between them ([`line_heights`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Heights {
    ascent: f32,
    descent: f32,
    height: f32,
}

impl Heights {
    /// Where a line of more than one face starts from, before its faces.
    const ZERO: Heights = Heights {
        ascent: 0.0,
        descent: 0.0,
        height: 0.0,
    };

    fn of(metrics: FontMetrics, as_given: bool) -> Heights {
        let (ascent, descent, height) = line_heights(metrics, as_given);
        Heights {
            ascent,
            descent,
            height,
        }
    }

    /// The heights of a line that holds both: the larger ascent, the
    /// smaller descent, and a height at least either's.
    fn with(self, other: Heights) -> Heights {
        let ascent = self.ascent.max(other.ascent);
        let descent = self.descent.min(other.descent);
        Heights {
            ascent,
            descent,
            height: (ascent - descent).max(self.height).max(other.height),
        }
    }
}

/// The heights of the faces a layout draws in, the first face's first.
struct Faces {
    /// Each face drawn, by number, with its heights; the first face at `0`.
    heights: Vec<(usize, Heights)>,
}

impl Faces {
    fn of<F: FontSet + ?Sized>(
        font: &F,
        paragraphs: &[(Range<usize>, Vec<EncodedGlyph>)],
        as_given: bool,
    ) -> Faces {
        let mut faces: Vec<usize> = (paragraphs.iter())
            .flat_map(|(_, glyphs)| glyphs.iter().map(|g| g.face))
            .filter(|&face| face != 0)
            .collect();
        faces.sort_unstable();
        faces.dedup();
        let heights = std::iter::once(0)
            .chain(faces)
            .map(|face| (face, Heights::of(font.face_metrics(face), as_given)))
            .collect();
        Faces { heights }
    }

    /// Whether every glyph is the first face's.
    fn one(&self) -> bool {
        self.heights.len() == 1
    }

    fn get(&self, face: usize) -> Heights {
        match self.heights.binary_search_by_key(&face, |&(face, _)| face) {
            Ok(i) => self.heights[i].1,
            Err(_) => self.heights[0].1,
        }
    }

    /// The face every one of `glyphs` is in (the first face's for none), or
    /// `None` where they're in more than one.
    fn sole(glyphs: &[EncodedGlyph]) -> Option<usize> {
        let face = glyphs.first().map_or(0, |g| g.face);
        glyphs.iter().all(|g| g.face == face).then_some(face)
    }

    /// The heights of a line of `glyphs`, not a comb line: from `0`, the
    /// largest ascent and smallest descent of the faces drawn on it, or the
    /// first face's for an empty line.
    fn line(&self, glyphs: &[EncodedGlyph]) -> Heights {
        if glyphs.is_empty() {
            return self.get(0);
        }
        let mut heights = Heights::ZERO;
        let mut face = None;
        for g in glyphs {
            if face != Some(g.face) {
                face = Some(g.face);
                heights = heights.with(self.get(g.face));
            }
        }
        heights
    }
}

/// Lays out `text` inside `rect` using `font` and `style`.
///
/// Supports `\n` and `\r\n` paragraph breaks, `Word` / `Char` / `NoWrap`
/// wrapping, `Left` / `Center` / `Right` / `Justify` horizontal alignment,
/// `Top` / `Middle` / `Bottom` vertical alignment, automatic font sizing when
/// `style.size` is `None` or `<= 0.0`, and fixed-cell `comb` layout.
///
/// `font` is one font or a [`FontSet`] of several: each unit of the text (a
/// character, or a grapheme cluster with [`TextBoxStyle::clusters`]) is drawn
/// in the first face that has all of it ([`face_for`]), and each line tells
/// the runs of its codes in each face ([`LaidOutLine::runs`]). A line's
/// ascent and descent are the largest and smallest of the faces drawn on it,
/// from `0`, an empty line's the first face's, as PDFium sets a line of two
/// fonts as of chromium/8152 (`core/fpdfdoc/cpvt_section.cpp`); a comb line
/// starts from the first face's and widens by its cells'. The step from one
/// line to the next is the distance from the one's descent to the other's
/// ascent, times [`TextBoxStyle::leading_factor`], or
/// [`TextBoxStyle::line_height`] where it's given. A set lays text out as its
/// first face alone does where that face draws every unit, those past a
/// comb's cells among them, and lines not in comb cells as one face alone
/// does where every line is in that face.
///
/// # Errors
///
/// Returns [`UnencodableChar`] for the first unit no face draws (other than
/// `\r` and `\n`, or `\t` which is normalized to `' '`), at the character of
/// it the first face can't encode ([`face_for`]): for one font, the first
/// character it can't encode.
pub fn layout_text<F: FontSet + ?Sized>(
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

    // Pre-encode every unit (validating encodability upfront even if `rect` is
    // tiny or `text` wraps).
    let mut paragraphs: Vec<(Range<usize>, Vec<EncodedGlyph>)> = Vec::new();
    let mut tmp_codes = Vec::with_capacity(4);
    let mut other_faces = false;
    for para in paragraph_ranges(text) {
        let mut glyphs: Vec<EncodedGlyph> = Vec::new();
        let body = &text[para.clone()];
        if style.clusters {
            for (offset, cluster) in body.grapheme_indices(true) {
                let at = para.start + offset;
                let unit = cluster.char_indices().map(move |(i, ch)| (at + i, ch));
                other_faces |= encode_unit(font, unit, &mut glyphs, &mut tmp_codes)? != 0;
            }
        } else {
            for (offset, ch) in body.char_indices() {
                let unit = std::iter::once((para.start + offset, ch));
                other_faces |= encode_unit(font, unit, &mut glyphs, &mut tmp_codes)? != 0;
            }
        }
        paragraphs.push((para, glyphs));
    }

    let faces = match other_faces {
        true => Faces::of(font, &paragraphs, style.font_heights_as_given),
        false => Faces {
            heights: vec![(
                0,
                Heights::of(font.face_metrics(0), style.font_heights_as_given),
            )],
        },
    };

    if let Some(comb_n) = style.comb.filter(|&n| n > 0) {
        return Ok(layout_comb(
            &paragraphs,
            text.len(),
            inner,
            style,
            comb_n,
            &faces,
        ));
    }

    let explicit_size = style.size.filter(|&s| s > 0.0 && s.is_finite());
    let chosen_size = match explicit_size {
        Some(s) => s,
        None => choose_auto_size(&paragraphs, inner, style, &faces),
    };

    Ok(build_lines_at_size(
        &paragraphs,
        inner,
        style,
        chosen_size,
        &faces,
    ))
}

/// The byte ranges of `text`'s paragraphs, each ended by `\r\n`, `\r` or
/// `\n`, and the last by the text's end.
fn paragraph_ranges(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut paragraphs = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                paragraphs.push(start..i);
                i += match bytes.get(i + 1) {
                    Some(b'\n') => 2,
                    _ => 1,
                };
                start = i;
            }
            b'\n' => {
                paragraphs.push(start..i);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    paragraphs.push(start..text.len());
    paragraphs
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

fn layout_comb(
    paragraphs: &[(Range<usize>, Vec<EncodedGlyph>)],
    total_bytes: usize,
    inner: Rect,
    style: &TextBoxStyle,
    comb_n: u32,
    faces: &Faces,
) -> LaidOutText {
    let glyphs: Vec<&EncodedGlyph> = paragraphs.iter().flat_map(|(_, g)| g.iter()).collect();
    // Each cell's unit: the glyphs from one not joined to the one before it
    // up to the next.
    let mut units: Vec<Range<usize>> = Vec::with_capacity(glyphs.len());
    for (i, g) in glyphs.iter().enumerate() {
        match units.last_mut() {
            Some(unit) if g.joined => unit.end = i + 1,
            _ => units.push(i..i + 1),
        }
    }
    let count = units.len().min(comb_n as usize);

    // The first face's heights, widened by each face a cell is drawn in.
    let mut heights = faces.get(0);
    for unit in &units[..count] {
        let face = glyphs[unit.start].face;
        if face != 0 {
            heights = heights.with(faces.get(face));
        }
    }
    let Heights {
        ascent: ascent_em,
        descent: descent_em,
        height: height_em,
    } = heights;

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

    let overflowed = units.len() > comb_n as usize || height_em * font_size > inner.height() + 1e-3;
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
    let mut cell_faces = Vec::with_capacity(count);
    for (i, unit) in units.iter().take(count).enumerate() {
        let unit = &glyphs[unit.clone()];
        let cell_idx = start_cell + i;
        let advance = unit[1..].iter().fold(unit[0].units, |sum, g| sum + g.units);
        let glyph_w = advance * font_size / 1000.0;
        let cx = inner.x0 + (cell_idx as f32) * cell_w + 0.5 * (cell_w - glyph_w);
        let codes = match unit {
            [g] => g.codes().to_vec(),
            _ => unit.iter().flat_map(|g| g.codes()).copied().collect(),
        };
        cells.push((Point::new(cx, baseline_y), codes));
        cell_faces.push(unit[0].face);
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
            runs: Vec::new(),
            cell_faces,
            ascent: ascent_em * font_size,
            descent: descent_em * font_size,
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
    faces: &Faces,
) -> f32 {
    let min_s = style.min_auto_size.max(0.5);
    let max_s = style.max_auto_size.max(min_s);
    if inner.width() <= 0.0 || inner.height() <= 0.0 {
        return min_s;
    }

    // The ceiling itself where the text fits at it: the search below only
    // closes on it from under.
    let fits = |size: f32| !build_lines_at_size(paragraphs, inner, style, size, faces).overflowed;
    if fits(max_s) {
        return max_s;
    }

    let mut lo = min_s;
    let mut hi = max_s;
    for _ in 0..16 {
        let mid = 0.5 * (lo + hi);
        let trial = build_lines_at_size(paragraphs, inner, style, mid, faces);
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
    // Keeping spaces, a line's width is summed and compared as PDFium's line
    // breaking does it as of chromium/8152 (`core/fpdfdoc/cpvt_section.cpp`,
    // `SplitLines`, over `cpvt_variabletext.cpp`'s `GetWordWidth`): each
    // advance times the size, then times `0.001`, in `f32`, and the line full
    // where the sum passes the width at all. Otherwise, by the scale, with an
    // allowance of `1e-4` for its rounding.
    let advance = |g: &EncodedGlyph| match keep_spaces {
        true => g.units * font_size * 0.001,
        false => g.units * scale,
    };
    let passes = |after: f32| match keep_spaces {
        true => after > max_width,
        false => after > max_width + 1e-4,
    };
    let mut out = Vec::new();
    let mut start = 0usize;
    let n = glyphs.len();

    while start < n {
        // Skip leading spaces on wrapped continuation lines (not on index 0 of
        // the paragraph so intentional leading indentation is preserved).
        if start > 0 && !keep_spaces {
            while start < n && is_space_unit(glyphs, start) {
                start += 1;
            }
            if start >= n {
                break;
            }
        }

        let mut width = 0.0f32;
        let mut end = start;
        let mut last_break: Option<usize> = None;

        // A unit at a time: a character, or a grapheme cluster, which no
        // line breaks inside.
        while end < n {
            let next = unit_end(glyphs, end);
            let mut after = width + advance(&glyphs[end]);
            for g in &glyphs[end + 1..next] {
                after += advance(g);
            }
            // Keeping spaces, a line breaks before a word, which a character
            // that would pass the width may itself begin.
            if keep_spaces && wrap == WrapMode::Word && end > start && begins_word(glyphs, end) {
                last_break = Some(end);
            }
            if end > start && passes(after) {
                // If this glyph is a space, break right here and consume trailing spaces.
                if !keep_spaces && is_space_unit(glyphs, end) {
                    last_break = Some(end);
                }
                break;
            }
            width = after;
            if wrap == WrapMode::Word && !keep_spaces {
                if is_space_unit(glyphs, end) {
                    last_break = Some(end);
                } else if glyphs[end].ch == '-' && next < n {
                    last_break = Some(next);
                }
            }
            end = next;
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
                .unwrap_or(end.max(unit_end(glyphs, start))),
            WrapMode::Char => end.max(unit_end(glyphs, start)),
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
///
/// Of a unit of several characters (a grapheme cluster), its first stands
/// for it, before it and after it.
fn begins_word(glyphs: &[EncodedGlyph], i: usize) -> bool {
    // The characters that begin each unit before this one, last first.
    let mut before = glyphs[..i].iter().rev().filter(|g| !g.joined).map(|g| g.ch);
    let Some(prev) = before.next() else {
        return false;
    };
    let ch = glyphs[i].ch;
    if !(opens(ch) || divides(prev, ch)) {
        return false;
    }
    // A space is never a word's start, so only a word's first character
    // looks back, and only over the spaces just before it.
    let opened = std::iter::once(prev)
        .chain(before)
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
/// single byte 32 (ISO 32000-2, §9.3.3).
fn word_spaces(glyphs: &[EncodedGlyph]) -> usize {
    glyphs.iter().filter(|g| widened(g)).count()
}

/// Whether `Tw` widens `g`: a space drawn as the single byte 32 (ISO
/// 32000-2, §9.3.3).
fn widened(g: &EncodedGlyph) -> bool {
    g.ch == ' ' && g.codes_len == 1 && g.codes[0] == b' '
}

/// `slice` without the spaces it ends with, each a unit of its own: a slice
/// ends where a unit does, so a space not joined to the glyph before it is
/// one.
fn trim_trailing_spaces(slice: &[EncodedGlyph]) -> &[EncodedGlyph] {
    let mut end = slice.len();
    while end > 0 && slice[end - 1].ch == ' ' && !slice[end - 1].joined {
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

/// Where a layout's lines stand: each at the same step in the heights of
/// one face, or each at its own place by its own heights.
enum Placement {
    /// Every line in one face: the first baseline, the step, the heights.
    Even(f32, f32, Heights),
    /// Lines in more than one face: each line's baseline and heights.
    Uneven(Vec<(f32, Heights)>),
}

/// The face every one of `wrapped` is in, an empty line counting as the
/// first face's, or `None` where they're in more than one.
fn sole_face(wrapped: &[WrappedSlice<'_>]) -> Option<usize> {
    let mut faces = wrapped.iter().map(|ws| Faces::sole(ws.glyphs));
    let face = faces.next()??;
    faces.all(|f| f == Some(face)).then_some(face)
}

fn build_lines_at_size(
    paragraphs: &[(Range<usize>, Vec<EncodedGlyph>)],
    inner: Rect,
    style: &TextBoxStyle,
    font_size: f32,
    faces: &Faces,
) -> LaidOutText {
    let max_w = inner.width();
    let max_h = inner.height();
    let explicit_h = style.line_height.filter(|&lh| lh > 0.0 && lh.is_finite());
    let leading = style.leading_factor.max(0.5);

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
    let sole = match faces.one() {
        true => Some(0),
        false => sole_face(&wrapped),
    };
    let (placement, line_h, total_block_h) = match sole {
        Some(face) => {
            let heights = faces.get(face);
            let Heights {
                ascent: ascent_em,
                descent: descent_em,
                height: height_em,
            } = heights;
            let line_h = explicit_h.unwrap_or(height_em * font_size * leading);
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
                    VerticalAlign::Middle => {
                        0.5 * (inner.y0 + inner.y1) + 0.5 * total_block_h - asc
                    }
                    VerticalAlign::Bottom => {
                        inner.y0 - desc + (line_count.saturating_sub(1) as f32) * line_h
                    }
                }
            };
            (
                Placement::Even(first_baseline_y, line_h, heights),
                line_h,
                total_block_h,
            )
        }
        None => uneven(
            &wrapped, faces, inner, style, font_size, explicit_h, leading,
        ),
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

        let (y, heights) = match &placement {
            Placement::Even(first_baseline_y, line_h, heights) => {
                (first_baseline_y - (i as f32) * line_h, *heights)
            }
            Placement::Uneven(placed) => placed[i],
        };
        let mut codes = Vec::with_capacity(ws.glyphs.len());
        for g in ws.glyphs {
            codes.extend_from_slice(g.codes());
        }
        let runs = match (faces.one(), ws.glyphs) {
            (_, []) => Vec::new(),
            (true, [first, .., last] | [first @ last]) => vec![LaidOutRun {
                face: 0,
                codes: 0..codes.len(),
                text: first.byte_start..last.byte_end,
                x: 0.0,
            }],
            (false, glyphs) => runs_of(glyphs, scale, word_space),
        };

        lines.push(LaidOutLine {
            text_range: ws.range,
            origin: Point::new(x, y),
            width: final_w,
            codes,
            word_space,
            cells: Vec::new(),
            runs,
            cell_faces: Vec::new(),
            ascent: heights.ascent * font_size,
            descent: heights.descent * font_size,
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

/// The places of `wrapped`'s lines, in more than one face: each line's
/// heights its own ([`Faces::line`]), each step from one line's descent to
/// the next one's ascent (or the step given), times the leading; with the
/// step two lines of the first's heights would take, and the block's
/// height, from the first line's ascent to the last one's descent.
fn uneven(
    wrapped: &[WrappedSlice<'_>],
    faces: &Faces,
    inner: Rect,
    style: &TextBoxStyle,
    font_size: f32,
    explicit_h: Option<f32>,
    leading: f32,
) -> (Placement, f32, f32) {
    let heights: Vec<Heights> = wrapped.iter().map(|ws| faces.line(ws.glyphs)).collect();
    let (Some(&first), Some(&last)) = (heights.first(), heights.last()) else {
        let line_h = explicit_h.unwrap_or(faces.get(0).height * font_size * leading);
        return (Placement::Uneven(Vec::new()), line_h, 0.0);
    };
    let step = |above: Heights, below: Heights| {
        explicit_h.unwrap_or((below.ascent - above.descent) * font_size * leading)
    };
    let line_h = explicit_h.unwrap_or(first.height * font_size * leading);
    let steps: f32 = heights.windows(2).map(|w| step(w[0], w[1])).sum();
    let asc = first.ascent * font_size;
    let desc = last.descent * font_size;
    let total_block_h = asc - desc + steps;

    let first_baseline_y = if heights.len() == 1 {
        single_line_baseline(
            inner,
            style.vertical_align,
            font_size,
            first.ascent,
            first.descent,
        )
    } else {
        match style.vertical_align {
            VerticalAlign::Top => inner.y1 - asc,
            VerticalAlign::Middle => 0.5 * (inner.y0 + inner.y1) + 0.5 * total_block_h - asc,
            VerticalAlign::Bottom => inner.y0 - desc + steps,
        }
    };
    let mut placed = Vec::with_capacity(heights.len());
    let mut y = first_baseline_y;
    for (i, &line) in heights.iter().enumerate() {
        if i > 0 {
            y -= step(heights[i - 1], line);
        }
        placed.push((y, line));
    }
    (Placement::Uneven(placed), line_h, total_block_h)
}

/// The runs of a line's `glyphs` in one face each: where each begins in the
/// line's codes, its text, and how far from the line's origin, by the
/// advances before it at `scale` and `word_space` for each space `Tw`
/// widens among them.
fn runs_of(glyphs: &[EncodedGlyph], scale: f32, word_space: f32) -> Vec<LaidOutRun> {
    let mut runs: Vec<LaidOutRun> = Vec::new();
    let (mut at, mut x) = (0, 0.0f32);
    for g in glyphs {
        let end = at + usize::from(g.codes_len);
        match runs.last_mut() {
            Some(run) if run.face == g.face => {
                run.codes.end = end;
                run.text.end = g.byte_end;
            }
            _ => runs.push(LaidOutRun {
                face: g.face,
                codes: at..end,
                text: g.byte_start..g.byte_end,
                x,
            }),
        }
        at = end;
        x += g.units * scale;
        if widened(g) {
            x += word_space;
        }
    }
    runs
}
