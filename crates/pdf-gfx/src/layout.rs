//! Spatial text assembly, reading-order reconstruction, selection geometry, and
//! search (`feature = "text-layout"`), modeled after PDFium's `CPDF_TextPage`
//! and `CPDF_TextPageFind` (`as of chromium/8152`).
//!
//! [`TextPage`] runs [`interpret_with`](crate::interpret_with) with
//! `Wants { text: true, ..Wants::NONE }` and assembles placed glyphs into
//! reading-order lines and characters:
//! - coalesces `/ActualText` marked-content spans into replacement text;
//! - suppresses faux-bold / shadow duplicate glyphs (`< 0.07 * size` within the
//!   preceding 7 characters or adjacent on a sorted baseline);
//! - sorts runs visually along each baseline and synthesizes inter-word spaces
//!   (`' '`) and inter-line breaks (`'\n'`);
//! - marks soft-wrapped line-end hyphens (`'-'` and `U+00AD` between letters);
//! - reorders visual RTL runs into logical reading order via `unicode-bidi`;
//! - provides hit-testing ([`TextPage::char_index_at`]), rectangular text
//!   queries ([`TextPage::text_in_rect`]), merged per-line selection quads
//!   ([`TextPage::selection_quads`]), and Unicode search ([`TextPage::search`]).

use std::borrow::Cow;
use std::fmt;
use std::ops::{ControlFlow, Range};

use pdf_content_streams::Concat;
use pdf_fonts::{Font, FontKind, Fonts};
use pdf_objects::fault::Sink;
use pdf_objects::{Resolve, Streams};
use unicode_bidi::{BidiClass, BidiInfo};

use crate::error::Error;
use crate::fault::Fault;
use crate::geom::{Matrix, Point, Quad, Rect};
use crate::interp::{Forms, Kind, Opts, Summary, Wants, interpret_with};
use crate::limits::Limits;
use crate::marked::Marks;
use crate::phases::phase;
use crate::text::TextShow;
use crate::xobject::UseId;

/// Lookback window for faux-bold / shadow duplicate glyph suppression
/// (`CPDF_TextPage::ProcessTextObjectItems`).
const SAME_WITHIN: usize = 7;

/// Maximum origin distance (as a fraction of effective font size) treated as an
/// overlapping duplicate of the same character.
const SAME_NEAR_FACTOR: f32 = 0.07;

/// Maximum perpendicular baseline distance (as a fraction of the smaller line's
/// effective font size) for two runs to belong to the same line.
const SAME_LINE_PERP_FACTOR: f32 = 0.25;

/// Intra-operator (`TJ`) gap threshold as a fraction of the active font's space
/// advance (`0.5 * space_width`, matching `chromium/8152`).
const INTRA_OP_SPACE_FACTOR: f32 = 0.5;

/// Inter-operator (`Tj` / `Tm`) gap threshold as a fraction of the active
/// font's space advance.
const INTER_OP_SPACE_FACTOR: f32 = 0.6;

/// Fallback space width (as a fraction of font size) when a font's space glyph
/// has zero width.
const FALLBACK_SPACE_EM: f32 = 0.30;

/// Options controlling [`TextPage::extract`] and [`TextPage::extract_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPageOpts {
    /// Whether to descend into Form XObjects (`Forms::Descend` by default).
    pub forms: Forms,
    /// Whether to retain invisible (`3 Tr`) and clip-only (`7 Tr`) text runs
    /// such as searchable OCR text layers (`true` by default, matching PDFium).
    pub include_invisible: bool,
    /// Whether to suppress overlapping faux-bold / shadow duplicate glyphs
    /// (`true` by default, matching PDFium).
    pub dedup_overlaps: bool,
    /// Whether to substitute marked-content `/ActualText` strings in place of
    /// enclosed glyphs (`true` by default, matching PDFium).
    pub actual_text: bool,
    /// Whether to reorder visual right-to-left runs on each line into logical
    /// reading order via the Unicode Bidirectional Algorithm (`true` by
    /// default, matching PDFium).
    pub bidi: bool,
}

impl Default for TextPageOpts {
    fn default() -> Self {
        Self {
            forms: Forms::Descend,
            include_invisible: true,
            dedup_overlaps: true,
            actual_text: true,
            bidi: true,
        }
    }
}

/// One logical character in a [`TextPage`].
///
/// Its `Debug` implementation reports byte offsets and geometry without
/// printing raw document text.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct PageChar {
    /// Unicode scalar value (`' '` for generated spaces, `'\n'` for generated
    /// line breaks, and `'-'` when normalized from a soft hyphen `U+00AD`).
    pub ch: char,
    /// UTF-8 byte offset of `ch` inside [`TextPage::text`].
    pub byte_offset: usize,
    /// Baseline origin in PageSpace.
    pub origin: Point,
    /// Oriented loose bounding quadrilateral in PageSpace.
    pub quad: Quad,
    /// Axis-aligned tight glyph bounding box in PageSpace (matching
    /// `FPDFText_GetCharBox`).
    pub tight_box: Rect,
    /// Effective font size in PageSpace units.
    pub font_size: f32,
    /// `true` when synthesized by layout (`' '` or `'\n'`) rather than decoded
    /// from content-stream glyphs (`FPDFText_IsGenerated`).
    pub generated: bool,
    /// `true` when this character is a line-ending hyphen (`'-'` or `U+00AD`)
    /// wrapping an alphabetic word onto the following line (`FPDFText_IsHyphen`).
    pub soft_hyphen: bool,
    /// `true` when drawn in `3 Tr` (`Invisible`) or `7 Tr` (`Clip`).
    pub invisible: bool,
    /// `true` when enclosed in an `/Artifact` marked-content sequence.
    pub in_artifact: bool,
    /// Innermost `/MCID` enclosing this character, if any.
    pub mcid: Option<u32>,
    line: usize,
    op_id: u32,
    proj_start: f32,
    proj_end: f32,
    space_page: f32,
}

impl PageChar {
    /// UTF-8 byte range of this character inside [`TextPage::text`].
    #[must_use]
    pub const fn byte_range(&self) -> Range<usize> {
        self.byte_offset..(self.byte_offset + self.ch.len_utf8())
    }

    /// Zero-based index of the [`TextLine`] containing this character (for a
    /// generated `'\n'`, the line it terminates).
    #[must_use]
    pub const fn line_index(&self) -> usize {
        self.line
    }
}

impl fmt::Debug for PageChar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageChar")
            .field("byte_offset", &self.byte_offset)
            .field("utf8_len", &self.ch.len_utf8())
            .field("origin", &self.origin)
            .field("quad", &self.quad)
            .field("tight_box", &self.tight_box)
            .field("font_size", &self.font_size)
            .field("generated", &self.generated)
            .field("soft_hyphen", &self.soft_hyphen)
            .field("invisible", &self.invisible)
            .field("in_artifact", &self.in_artifact)
            .field("mcid", &self.mcid)
            .finish()
    }
}

/// One assembled line of text in a [`TextPage`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TextLine {
    /// Range of [`PageChar`] indices on this line (excluding any trailing
    /// generated `'\n'`).
    pub chars: Range<usize>,
    /// UTF-8 byte range of this line inside [`TextPage::text`] (excluding any
    /// trailing `'\n'`).
    pub bytes: Range<usize>,
    /// Axis-aligned bounding box uniting all non-generated characters on this
    /// line in PageSpace.
    pub bounds: Rect,
    /// Baseline origin of the first character on this line in PageSpace.
    pub origin: Point,
    /// Unit baseline progression vector in PageSpace (`(1, 0)` for upright
    /// horizontal text).
    pub direction: Point,
}

/// Options controlling [`TextPage::search`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOpts {
    /// Require exact case matching (`false` by default; mirrors
    /// `FPDF_MATCHCASE`).
    pub match_case: bool,
    /// Require word boundaries before and after each match (`false` by default;
    /// mirrors `FPDF_MATCHWHOLEWORD`).
    pub whole_word: bool,
    /// Advance by a single character after a match so overlapping occurrences
    /// are returned (`false` by default; mirrors `FPDF_CONSECUTIVE`).
    pub consecutive: bool,
    /// Match seamlessly across soft-wrapped line-end hyphens such as
    /// `"inter-\nnational"` (`true` by default; set `false` via
    /// [`SearchOpts::PDFIUM`] for strict `FPDFText_FindStart` behavior).
    pub ignore_soft_hyphens: bool,
}

impl SearchOpts {
    /// Strict PDFium `FPDFText_FindStart` defaults (`match_case: false`,
    /// `whole_word: false`, `consecutive: false`, `ignore_soft_hyphens: false`).
    pub const PDFIUM: Self = Self {
        match_case: false,
        whole_word: false,
        consecutive: false,
        ignore_soft_hyphens: false,
    };
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            match_case: false,
            whole_word: false,
            consecutive: false,
            ignore_soft_hyphens: true,
        }
    }
}

/// One match returned by [`TextPage::search`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TextMatch {
    /// Character index range in [`TextPage::chars`].
    pub chars: Range<usize>,
    /// UTF-8 byte range in [`TextPage::text`].
    pub bytes: Range<usize>,
    /// Merged per-line highlight quadrilaterals in PageSpace.
    pub quads: Vec<Quad>,
}

/// Positioned reading-order text, line geometry, and search index for a page.
///
/// Its `Debug` implementation reports character/byte/line counts and
/// [`Summary`] without exposing document plaintext.
#[derive(Clone, PartialEq)]
pub struct TextPage {
    text: String,
    chars: Vec<PageChar>,
    lines: Vec<TextLine>,
    summary: Summary,
}

impl fmt::Debug for TextPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextPage")
            .field("chars_len", &self.chars.len())
            .field("text_len", &self.text.len())
            .field("lines_len", &self.lines.len())
            .field("summary", &self.summary)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ActualKey {
    stream: Option<UseId>,
    at: Range<usize>,
}

#[derive(Clone, PartialEq, Eq)]
struct OpKey {
    stream: Option<UseId>,
    op_start: usize,
}

#[derive(Clone)]
struct RawCluster {
    text: String,
    code: Option<u32>,
    origin: Point,
    quad: Quad,
    tight_box: Rect,
    dir: Point,
    proj_start: f32,
    proj_end: f32,
    eff_size: f32,
    space_page: f32,
    invisible: bool,
    in_artifact: bool,
    mcid: Option<u32>,
    op: OpKey,
}

struct ActualPending {
    key: ActualKey,
    replacement: String,
    first: RawCluster,
    bounds: Rect,
    tight: Rect,
    proj_end: f32,
}

struct LineWork {
    dir: Point,
    norm: Point,
    ref_origin: Point,
    eff_size: f32,
    clusters: Vec<RawCluster>,
}

impl TextPage {
    /// Extracts reading-order text and geometry from `content` without
    /// collecting recoverable [`Fault`]s.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] returned by [`interpret_with`].
    pub fn extract<'a, S>(
        streams: S,
        content: &Concat<'_>,
        resources: Option<&'a pdf_objects::Object>,
        limits: &Limits,
        fonts: &Fonts,
        opts: TextPageOpts,
    ) -> Result<Self, Error>
    where
        S: Resolve<'a> + Streams + Copy,
    {
        Self::extract_with(streams, content, resources, limits, fonts, opts, &mut ())
    }

    /// Extracts reading-order text and geometry from `content`, reporting
    /// recoverable [`Fault`]s to `sink`.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] returned by [`interpret_with`].
    pub fn extract_with<'a, S, F>(
        streams: S,
        content: &Concat<'_>,
        resources: Option<&'a pdf_objects::Object>,
        limits: &Limits,
        fonts: &Fonts,
        opts: TextPageOpts,
        sink: &mut F,
    ) -> Result<Self, Error>
    where
        S: Resolve<'a> + Streams + Copy,
        F: Sink<Fault> + ?Sized,
    {
        phase!(DEBUG, "gfx.text_page", content_len = content.bytes().len());
        let mut raw: Vec<RawCluster> = Vec::new();
        let mut pending_actual: Option<ActualPending> = None;

        let interp_opts = Opts {
            forms: opts.forms,
            wants: Wants {
                text: true,
                ..Wants::NONE
            },
        };

        let summary = interpret_with(
            streams,
            content,
            resources,
            limits,
            fonts,
            interp_opts,
            sink,
            |ev| {
                let active_actual = if opts.actual_text {
                    outermost_actual(ev.marks)
                } else {
                    None
                };

                if let Some(ref pend) = pending_actual
                    && active_actual.as_ref().map(|(k, _)| k) != Some(&pend.key)
                {
                    flush_actual(pending_actual.take().unwrap(), &mut raw);
                }

                if let Kind::Text(show) = ev.kind {
                    collect_show(
                        &show,
                        ev.marks,
                        OpKey {
                            stream: ev.at.stream.cloned(),
                            op_start: ev.at.op_range().start,
                        },
                        active_actual,
                        opts,
                        &mut pending_actual,
                        &mut raw,
                    );
                }
                ControlFlow::Continue(())
            },
        )?;

        if let Some(pend) = pending_actual.take() {
            flush_actual(pend, &mut raw);
        }

        Ok(assemble_page(raw, opts, summary))
    }

    /// Interpreter summary recorded while reading the page.
    #[must_use]
    pub const fn summary(&self) -> Summary {
        self.summary
    }

    /// Number of [`PageChar`]s on the page (including generated `' '` and
    /// `'\n'` characters).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.chars.len()
    }

    /// Returns `true` if the page contains zero characters.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Assembled UTF-8 text of the page (`'\n'`-separated lines).
    ///
    /// Every [`PageChar`] maps to `&self.text()[c.byte_range()]`.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns [`TextPage::text`] with soft-wrapped line-end hyphens and their
    /// immediately following generated `'\n'` removed so hyphenated words read
    /// continuously.
    #[must_use]
    pub fn text_dehyphenated(&self) -> Cow<'_, str> {
        if !self.chars.iter().any(|c| c.soft_hyphen) {
            return Cow::Borrowed(&self.text);
        }
        let mut out = String::with_capacity(self.text.len());
        let mut skip_newline = false;
        for c in &self.chars {
            if c.soft_hyphen {
                skip_newline = true;
                continue;
            }
            if skip_newline && c.generated && c.ch == '\n' {
                skip_newline = false;
                continue;
            }
            skip_newline = false;
            out.push(c.ch);
        }
        Cow::Owned(out)
    }

    /// Slice of reading-order [`PageChar`]s on the page.
    #[must_use]
    pub fn chars(&self) -> &[PageChar] {
        &self.chars
    }

    /// Slice of reading-order [`TextLine`]s on the page.
    #[must_use]
    pub fn lines(&self) -> &[TextLine] {
        &self.lines
    }

    /// Finds the index of the non-generated character at or nearest `point`
    /// within `(tolerance_x, tolerance_y)` in PageSpace (matching
    /// `FPDFText_GetCharIndexAtPos`).
    #[must_use]
    pub fn char_index_at(&self, point: Point, tolerance_x: f32, tolerance_y: f32) -> Option<usize> {
        let tx = tolerance_x.max(0.0);
        let ty = tolerance_y.max(0.0);
        let mut best: Option<(usize, f32)> = None;
        for (idx, c) in self.chars.iter().enumerate() {
            if c.generated {
                continue;
            }
            let b = c.tight_box;
            if point.x >= b.x0 && point.x <= b.x1 && point.y >= b.y0 && point.y <= b.y1 {
                return Some(idx);
            }
            let dx = if point.x < b.x0 {
                b.x0 - point.x
            } else if point.x > b.x1 {
                point.x - b.x1
            } else {
                0.0
            };
            let dy = if point.y < b.y0 {
                b.y0 - point.y
            } else if point.y > b.y1 {
                point.y - b.y1
            } else {
                0.0
            };
            if dx <= tx && dy <= ty {
                let dist2 = dx * dx + dy * dy;
                if best.is_none_or(|(_, best_d)| dist2 < best_d) {
                    best = Some((idx, dist2));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Extracts the UTF-8 text of characters whose tight boxes intersect `rect`
    /// (preserving inter-word spaces and line breaks between matched lines,
    /// analogous to `FPDFText_GetBoundedText`).
    #[must_use]
    pub fn text_in_rect(&self, rect: Rect) -> String {
        let query = Rect::new(rect.x0, rect.y0, rect.x1, rect.y1);
        let mut out = String::new();
        let mut last_line: Option<usize> = None;
        let mut pending_space = false;

        for c in &self.chars {
            if c.generated {
                if c.ch == ' ' && last_line == Some(c.line) {
                    pending_space = true;
                }
                continue;
            }
            let b = c.tight_box;
            let inside =
                b.x1 >= query.x0 && b.x0 <= query.x1 && b.y1 >= query.y0 && b.y0 <= query.y1;
            if !inside {
                continue;
            }
            if let Some(prev_line) = last_line
                && prev_line != c.line
            {
                out.push('\n');
                pending_space = false;
            } else if pending_space && !out.ends_with(' ') && !out.ends_with('\n') {
                out.push(' ');
                pending_space = false;
            }
            last_line = Some(c.line);
            out.push(c.ch);
        }
        out
    }

    /// Computes merged per-line selection [`Quad`]s covering `chars` in
    /// PageSpace (suitable for viewer selection highlights and PDF annotation
    /// `/QuadPoints`).
    #[must_use]
    pub fn selection_quads(&self, chars: Range<usize>) -> Vec<Quad> {
        let start = chars.start.min(self.chars.len());
        let end = chars.end.min(self.chars.len());
        if start >= end {
            return Vec::new();
        }
        merge_selection(&self.chars[start..end], &self.lines)
            .into_iter()
            .map(|(quad, _)| quad)
            .collect()
    }

    /// Computes merged per-line axis-aligned tight [`Rect`]s covering `chars`
    /// in PageSpace (analogous to `FPDFText_CountRects` / `FPDFText_GetRect`).
    #[must_use]
    pub fn selection_rects(&self, chars: Range<usize>) -> Vec<Rect> {
        let start = chars.start.min(self.chars.len());
        let end = chars.end.min(self.chars.len());
        if start >= end {
            return Vec::new();
        }
        merge_selection(&self.chars[start..end], &self.lines)
            .into_iter()
            .map(|(_, tight)| tight)
            .collect()
    }

    /// Searches the page for `query` under `opts`, returning every match with
    /// its [`PageChar`] range, UTF-8 byte range in [`TextPage::text`], and
    /// merged highlight [`Quad`]s.
    ///
    /// Whitespace runs in `query` match any whitespace (`' '` or `'\n'`) on the
    /// page (matching PDFium's cross-line search), and empty queries return an
    /// empty vector.
    #[must_use]
    pub fn search(&self, query: &str, opts: SearchOpts) -> Vec<TextMatch> {
        if query.is_empty() || self.chars.is_empty() {
            return Vec::new();
        }

        let needle = normalize_needle(query, opts.match_case);
        if needle.is_empty() {
            return Vec::new();
        }

        // Build normalized haystack units mapped back to `self.chars` indices.
        let mut hay: Vec<(char, usize)> = Vec::with_capacity(self.chars.len());
        let mut skip_wrap_nl = false;
        for (idx, pc) in self.chars.iter().enumerate() {
            if opts.ignore_soft_hyphens {
                if pc.soft_hyphen {
                    skip_wrap_nl = true;
                    continue;
                }
                if skip_wrap_nl && pc.generated && pc.ch == '\n' {
                    skip_wrap_nl = false;
                    continue;
                }
                skip_wrap_nl = false;
            } else if pc.soft_hyphen {
                // In strict PDFium mode, a soft-hyphenated wrap inserts a
                // non-character separator (`0xFFFE`) that neither `' '` nor
                // contiguous letters match across.
                hay.push(('\u{FFFE}', idx));
                continue;
            }

            if pc.ch.is_whitespace() {
                if hay.last().is_none_or(|&(prev, _)| prev != ' ') {
                    hay.push((' ', idx));
                }
            } else if opts.match_case {
                hay.push((pc.ch, idx));
            } else {
                for lc in pc.ch.to_lowercase() {
                    hay.push((lc, idx));
                }
            }
        }

        if hay.len() < needle.len() {
            return Vec::new();
        }

        let mut matches = Vec::new();
        let mut pos = 0usize;
        while pos + needle.len() <= hay.len() {
            if hay[pos..pos + needle.len()]
                .iter()
                .zip(&needle)
                .all(|((hc, _), nc)| hc == nc)
            {
                let start_char = hay[pos].1;
                let last_hay_char = hay[pos + needle.len() - 1].1;
                // Expand `end_char` past any trailing whitespace chars collapsed
                // into the final `' '` of the match, or include `last_hay_char + 1`.
                let mut end_char = last_hay_char + 1;
                if needle.last() == Some(&' ') {
                    while end_char < self.chars.len() && self.chars[end_char].ch.is_whitespace() {
                        end_char += 1;
                    }
                }

                let word_ok = !opts.whole_word
                    || (is_word_boundary_before(&self.chars, start_char)
                        && is_word_boundary_after(&self.chars, end_char));

                if word_ok {
                    let byte_start = self.chars[start_char].byte_offset;
                    let byte_end = self.chars[end_char - 1].byte_range().end;
                    let quads = self.selection_quads(start_char..end_char);
                    matches.push(TextMatch {
                        chars: start_char..end_char,
                        bytes: byte_start..byte_end,
                        quads,
                    });
                    if opts.consecutive {
                        pos += 1;
                    } else {
                        pos += needle.len();
                    }
                    continue;
                }
            }
            pos += 1;
        }

        matches
    }
}

fn outermost_actual(marks: &Marks) -> Option<(ActualKey, String)> {
    let mut found = None;
    for m in marks.iter() {
        if let Some(ref txt) = m.actual_text {
            found = Some((
                ActualKey {
                    stream: m.stream.clone(),
                    at: m.at.clone(),
                },
                txt.clone(),
            ));
        }
    }
    found
}

fn flush_actual(pend: ActualPending, raw: &mut Vec<RawCluster>) {
    let text = normalize_pdfium_chars(&pend.replacement);
    if text.is_empty() {
        return;
    }
    let mut cluster = pend.first;
    cluster.text = text;
    cluster.code = None;
    cluster.quad = Quad::from_rect(pend.bounds);
    cluster.tight_box = pend.tight;
    cluster.proj_end = pend.proj_end;
    raw.push(cluster);
}

fn normalize_pdfium_chars(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{00A0}' => ' ',
            '\u{00AD}' => '\u{00AD}',
            other => other,
        })
        .collect()
}

fn takes_no_room(font: &Font, shown: &[u8]) -> bool {
    if font.kind() != FontKind::Type3 && !font.outlined() {
        return false;
    }
    let (mut low, mut high, mut at) = (f32::INFINITY, f32::NEG_INFINITY, 0.0f32);
    for (_, code) in font.codes(shown) {
        let b = if font.outlined() {
            font.char_box(code)
        } else {
            font.glyph_box(code)
        }
        .unwrap_or_default();
        match font.vertical(code) {
            Some(v) => {
                low = low.min(b.x0 - v.vx);
                high = high.max(b.x1 - v.vx);
            }
            None => {
                low = low.min(at + b.x0);
                high = high.max(at + b.x1);
                at += font.width(code);
            }
        }
    }
    (high - low).abs() < 1e-4
}

fn tight_glyph_box(
    font: &Font,
    code: u32,
    start: Matrix,
    size: f32,
    origin_local: Point,
    width_local: f32,
) -> Option<Rect> {
    let raw = if font.outlined() {
        font.char_box(code)
    } else {
        font.glyph_box(code)
    }?;
    let scale = size / 1000.0;
    let x0 = raw.x0 * scale + origin_local.x;
    let y0 = raw.y0 * scale + origin_local.y;
    let mut x1 = raw.x1 * scale + origin_local.x;
    let mut y1 = raw.y1 * scale + origin_local.y;
    if (y1 - y0).abs() < 0.01 {
        y1 = y0 + size.abs().max(0.01);
    }
    if (x1 - x0).abs() < 0.01 {
        x1 = x0 + width_local;
    }
    Some(start.apply_rect(Rect::new(x0, y0, x1, y1)))
}

fn collect_show(
    show: &TextShow<'_>,
    marks: &Marks,
    op: OpKey,
    active_actual: Option<(ActualKey, String)>,
    opts: TextPageOpts,
    pending_actual: &mut Option<ActualPending>,
    raw: &mut Vec<RawCluster>,
) {
    let Some(font) = show.font else {
        return;
    };
    if takes_no_room(font, show.string) {
        return;
    }

    let vertical = font.is_vertical();
    let p0 = show.start.apply(Point::ZERO);
    let along_pt = if vertical {
        show.start.apply(Point::new(0.0, -1.0))
    } else {
        show.start.apply(Point::new(1.0, 0.0))
    };
    let perp_pt = if vertical {
        show.start.apply(Point::new(1.0, 0.0))
    } else {
        show.start.apply(Point::new(0.0, 1.0))
    };

    let dx = along_pt.x - p0.x;
    let dy = along_pt.y - p0.y;
    let scale_along = dx.hypot(dy);
    let dir = if scale_along > 1e-6 {
        Point::new(dx / scale_along, dy / scale_along)
    } else {
        Point::new(1.0, 0.0)
    };
    let scale_perp = (perp_pt.x - p0.x).hypot(perp_pt.y - p0.y);
    let eff_size = (show.size.abs() * scale_perp.max(scale_along)).max(0.001);

    let space_units = font.width(32);
    let space_page = if space_units > 0.0 {
        (space_units * show.size.abs() / 1000.0) * scale_along
    } else {
        eff_size * FALLBACK_SPACE_EM
    };

    let in_artifact = marks.in_artifact();
    let mcid = marks.innermost_mcid();

    let mut local_at = show.offset;
    for glyph in show.glyphs() {
        let units = if vertical {
            font.vertical(glyph.code).map_or(0.0, |v| v.w1)
        } else {
            font.width(glyph.code)
        };
        let width_local = units * show.size / 1000.0;
        let origin_local = if vertical {
            let v = font.vertical(glyph.code);
            let (vx, vy) = v.map_or((0.0, 0.0), |v| (v.vx, v.vy));
            Point::new(-show.size * vx / 1000.0, local_at - show.size * vy / 1000.0)
        } else {
            Point::new(local_at, 0.0)
        };
        let mut next_local = local_at + width_local;
        if font.word_spaced(glyph.code, glyph.bytes.len()) {
            next_local += show.text.word_space;
        }
        next_local += show.text.char_space;
        let step_along = if vertical {
            -(next_local - local_at) * scale_along
        } else {
            (next_local - local_at) * scale_along
        };
        local_at = next_local;

        if glyph.invisible && !opts.include_invisible {
            continue;
        }

        let proj_start = glyph.origin.x * dir.x + glyph.origin.y * dir.y;
        let proj_end = proj_start + step_along.max(0.0);
        let tight = tight_glyph_box(
            font,
            glyph.code,
            show.start,
            show.size,
            origin_local,
            width_local,
        )
        .unwrap_or_else(|| glyph.quad.bounds());

        if let Some((ref key, ref rep)) = active_actual {
            let quad_bounds = glyph.quad.bounds();
            match pending_actual {
                Some(pend) if pend.key == *key => {
                    pend.bounds = pend.bounds.union(quad_bounds);
                    pend.tight = pend.tight.union(tight);
                    pend.proj_end = pend.proj_end.max(proj_end);
                }
                _ => {
                    if let Some(old) = pending_actual.take() {
                        flush_actual(old, raw);
                    }
                    let first = RawCluster {
                        text: String::new(),
                        code: None,
                        origin: glyph.origin,
                        quad: glyph.quad,
                        tight_box: tight,
                        dir,
                        proj_start,
                        proj_end,
                        eff_size,
                        space_page,
                        invisible: glyph.invisible,
                        in_artifact,
                        mcid,
                        op: op.clone(),
                    };
                    *pending_actual = Some(ActualPending {
                        key: key.clone(),
                        replacement: rep.clone(),
                        first,
                        bounds: quad_bounds,
                        tight,
                        proj_end,
                    });
                }
            }
            continue;
        }

        if opts.dedup_overlaps {
            let tol = SAME_NEAR_FACTOR * eff_size;
            let dup = raw.iter().rev().take(SAME_WITHIN).any(|prev| {
                prev.code == Some(glyph.code)
                    && (prev.origin.x - glyph.origin.x).abs() < tol
                    && (prev.origin.y - glyph.origin.y).abs() < tol
            });
            if dup {
                continue;
            }
        }

        let text = match glyph.text() {
            Some(s) => normalize_pdfium_chars(&s),
            None => char::from_u32(glyph.code).unwrap_or('\u{FFFD}').to_string(),
        };
        if text.is_empty() {
            continue;
        }

        raw.push(RawCluster {
            text,
            code: Some(glyph.code),
            origin: glyph.origin,
            quad: glyph.quad,
            tight_box: tight,
            dir,
            proj_start,
            proj_end,
            eff_size,
            space_page,
            invisible: glyph.invisible,
            in_artifact,
            mcid,
            op: op.clone(),
        });
    }
}

fn assemble_page(raw: Vec<RawCluster>, opts: TextPageOpts, summary: Summary) -> TextPage {
    if raw.is_empty() {
        return TextPage {
            text: String::new(),
            chars: Vec::new(),
            lines: Vec::new(),
            summary,
        };
    }

    // 1. Partition clusters into sequential lines (preserving stream block order
    //    while merging runs that share the same baseline).
    let mut line_works: Vec<LineWork> = Vec::new();
    for cluster in raw {
        let join_current = line_works.last().is_some_and(|line| {
            let dot = line.dir.x * cluster.dir.x + line.dir.y * cluster.dir.y;
            if dot < 0.98 {
                return false;
            }
            let rel_x = cluster.origin.x - line.ref_origin.x;
            let rel_y = cluster.origin.y - line.ref_origin.y;
            let perp = (rel_x * line.norm.x + rel_y * line.norm.y).abs();
            let min_size = line.eff_size.min(cluster.eff_size);
            perp <= SAME_LINE_PERP_FACTOR * min_size
        });

        if join_current {
            line_works.last_mut().unwrap().clusters.push(cluster);
        } else {
            let dir = cluster.dir;
            let norm = Point::new(-dir.y, dir.x);
            line_works.push(LineWork {
                dir,
                norm,
                ref_origin: cluster.origin,
                eff_size: cluster.eff_size,
                clusters: vec![cluster],
            });
        }
    }

    // 2. Expand each line: visual sort along `dir`, adjacent overlap dedup,
    //    synthetic space insertion, and Unicode BiDi visual-to-logical reorder.
    let mut expanded_lines: Vec<(Point, Point, Vec<PageChar>)> =
        Vec::with_capacity(line_works.len());

    for (line_idx, mut lw) in line_works.into_iter().enumerate() {
        lw.clusters
            .sort_by(|a, b| a.proj_start.total_cmp(&b.proj_start));

        let mut line_chars: Vec<PageChar> = Vec::with_capacity(lw.clusters.len() + 4);
        let mut prev_end: Option<(f32, f32, OpKey, u32)> = None;
        let mut next_op_id = 1u32;

        for cluster in lw.clusters {
            let cur_op_id = match prev_end {
                Some((_, _, ref last_op, last_id)) if *last_op == cluster.op => last_id,
                _ => {
                    let id = next_op_id;
                    next_op_id = next_op_id.saturating_add(1);
                    id
                }
            };

            if opts.dedup_overlaps
                && let Some(last) = line_chars.last()
                && !last.generated
                && cluster.text.chars().count() == 1
                && cluster.text.starts_with(last.ch)
                && (cluster.origin.x - last.origin.x).abs() < SAME_NEAR_FACTOR * cluster.eff_size
                && (cluster.origin.y - last.origin.y).abs() < SAME_NEAR_FACTOR * cluster.eff_size
            {
                continue;
            }

            if let Some((last_proj_end, last_space, ref last_op, _)) = prev_end {
                let gap = cluster.proj_start - last_proj_end;
                let space_ref = last_space.min(cluster.space_page);
                let factor = if *last_op == cluster.op {
                    INTRA_OP_SPACE_FACTOR
                } else {
                    INTER_OP_SPACE_FACTOR
                };
                let threshold = factor * space_ref;
                let starts_space = cluster.text.starts_with(' ');
                let ends_space = line_chars.last().is_some_and(|c| c.ch == ' ');
                if !starts_space && !ends_space && gap + 1e-4 >= threshold {
                    let sp_origin = Point::new(
                        cluster.origin.x - lw.dir.x * gap,
                        cluster.origin.y - lw.dir.y * gap,
                    );
                    let sp_rect =
                        Rect::new(sp_origin.x, sp_origin.y, cluster.origin.x, cluster.origin.y);
                    line_chars.push(PageChar {
                        ch: ' ',
                        byte_offset: 0,
                        origin: sp_origin,
                        quad: Quad::from_rect(sp_rect),
                        tight_box: sp_rect,
                        font_size: cluster.eff_size,
                        generated: true,
                        soft_hyphen: false,
                        invisible: cluster.invisible,
                        in_artifact: cluster.in_artifact,
                        mcid: cluster.mcid,
                        line: line_idx,
                        op_id: 0,
                        proj_start: last_proj_end,
                        proj_end: cluster.proj_start,
                        space_page: space_ref,
                    });
                }
            }

            let count = cluster.text.chars().count().max(1);
            let span_proj = (cluster.proj_end - cluster.proj_start).max(0.0);
            for (ci, raw_ch) in cluster.text.chars().enumerate() {
                let is_space = raw_ch == ' ';
                if is_space && line_chars.last().is_some_and(|c| c.ch == ' ') {
                    continue;
                }
                let (sub_quad, sub_tight, sub_p0, sub_p1) = if count == 1 {
                    (
                        cluster.quad,
                        cluster.tight_box,
                        cluster.proj_start,
                        cluster.proj_end,
                    )
                } else {
                    let t0 = ci as f32 / count as f32;
                    let t1 = (ci + 1) as f32 / count as f32;
                    let b = cluster.quad.bounds();
                    let w = b.width();
                    let sub_b = Rect::new(b.x0 + w * t0, b.y0, b.x0 + w * t1, b.y1);
                    let tw = cluster.tight_box.width();
                    let sub_t = Rect::new(
                        cluster.tight_box.x0 + tw * t0,
                        cluster.tight_box.y0,
                        cluster.tight_box.x0 + tw * t1,
                        cluster.tight_box.y1,
                    );
                    (
                        Quad::from_rect(sub_b),
                        sub_t,
                        cluster.proj_start + span_proj * t0,
                        cluster.proj_start + span_proj * t1,
                    )
                };

                line_chars.push(PageChar {
                    ch: raw_ch,
                    byte_offset: 0,
                    origin: cluster.origin,
                    quad: sub_quad,
                    tight_box: sub_tight,
                    font_size: cluster.eff_size,
                    generated: false,
                    soft_hyphen: false,
                    invisible: cluster.invisible,
                    in_artifact: cluster.in_artifact,
                    mcid: cluster.mcid,
                    line: line_idx,
                    op_id: cur_op_id,
                    proj_start: sub_p0,
                    proj_end: sub_p1,
                    space_page: cluster.space_page,
                });
            }

            prev_end = Some((cluster.proj_end, cluster.space_page, cluster.op, cur_op_id));
        }

        // Trim trailing generated spaces on the line.
        while line_chars
            .last()
            .is_some_and(|c| c.generated && c.ch == ' ')
        {
            line_chars.pop();
        }

        if opts.bidi && line_chars.len() > 1 {
            reorder_visual_to_logical(&mut line_chars);
        }

        if !line_chars.is_empty() {
            let line_origin = line_chars[0].origin;
            expanded_lines.push((line_origin, lw.dir, line_chars));
        }
    }

    // 3. Detect soft-wrapped line-end hyphens (`'-'` or `U+00AD` preceded by a
    //    letter and followed by a letter on the next line) and flatten into
    //    `text`, `chars`, and `lines`.
    for i in 0..expanded_lines.len() {
        let is_soft = if i + 1 < expanded_lines.len() {
            let cur = &expanded_lines[i].2;
            let next = &expanded_lines[i + 1].2;
            let ends_hyphen = cur
                .last()
                .is_some_and(|c| !c.generated && (c.ch == '-' || c.ch == '\u{00AD}'));
            let prev_alpha = cur
                .len()
                .checked_sub(2)
                .and_then(|k| cur.get(k))
                .is_some_and(|c| c.ch.is_alphabetic());
            let next_alpha = next
                .iter()
                .find(|c| !c.generated || c.ch != ' ')
                .is_some_and(|c| c.ch.is_alphabetic());
            ends_hyphen && prev_alpha && next_alpha
        } else {
            false
        };

        if let Some(last) = expanded_lines[i].2.last_mut() {
            if last.ch == '\u{00AD}' {
                last.ch = '-';
            }
            if is_soft {
                last.soft_hyphen = true;
            }
        }
    }

    let total_chars: usize = expanded_lines
        .iter()
        .map(|(_, _, v)| v.len() + 1)
        .sum::<usize>();
    let mut text = String::with_capacity(total_chars);
    let mut chars = Vec::with_capacity(total_chars);
    let mut lines = Vec::with_capacity(expanded_lines.len());

    let line_count = expanded_lines.len();
    for (final_line_idx, (origin, direction, line_vec)) in expanded_lines.into_iter().enumerate() {
        let char_start = chars.len();
        let byte_start = text.len();
        let mut bounds: Option<Rect> = None;

        for mut pc in line_vec {
            pc.line = final_line_idx;
            pc.byte_offset = text.len();
            if !pc.generated {
                let qb = pc.quad.bounds();
                bounds = Some(bounds.map_or(qb, |acc| acc.union(qb)));
            }
            text.push(pc.ch);
            chars.push(pc);
        }

        let char_end = chars.len();
        let byte_end = text.len();
        let line_bounds =
            bounds.unwrap_or_else(|| Rect::new(origin.x, origin.y, origin.x, origin.y));
        lines.push(TextLine {
            chars: char_start..char_end,
            bytes: byte_start..byte_end,
            bounds: line_bounds,
            origin,
            direction,
        });

        if final_line_idx + 1 < line_count {
            let last_ref = &chars[char_end - 1];
            let nl_pt = Point::new(last_ref.tight_box.x1, last_ref.origin.y);
            let nl_rect = Rect::new(nl_pt.x, nl_pt.y, nl_pt.x, nl_pt.y);
            let nl_offset = text.len();
            text.push('\n');
            chars.push(PageChar {
                ch: '\n',
                byte_offset: nl_offset,
                origin: nl_pt,
                quad: Quad::from_rect(nl_rect),
                tight_box: nl_rect,
                font_size: last_ref.font_size,
                generated: true,
                soft_hyphen: false,
                invisible: last_ref.invisible,
                in_artifact: last_ref.in_artifact,
                mcid: last_ref.mcid,
                line: final_line_idx,
                op_id: 0,
                proj_start: last_ref.proj_end,
                proj_end: last_ref.proj_end,
                space_page: last_ref.space_page,
            });
        }
    }

    TextPage {
        text,
        chars,
        lines,
        summary,
    }
}

fn has_rtl_char(chars: &[PageChar]) -> bool {
    chars.iter().any(|c| {
        matches!(
            unicode_bidi::bidi_class(c.ch),
            BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI
        )
    })
}

/// Converts a visually ordered left-to-right slice of [`PageChar`]s into
/// logical reading order using UAX #9 level reversal (`unicode-bidi`).
fn reorder_visual_to_logical(chars: &mut [PageChar]) {
    if !has_rtl_char(chars) {
        return;
    }
    let mut line_str = String::with_capacity(chars.len() * 2);
    let mut byte_starts = Vec::with_capacity(chars.len());
    for c in chars.iter() {
        byte_starts.push(line_str.len());
        line_str.push(c.ch);
    }

    let bidi = BidiInfo::new(&line_str, None);
    if bidi.paragraphs.is_empty() {
        return;
    }
    let levels: Vec<u8> = byte_starts
        .iter()
        .map(|&b| bidi.levels[b].number())
        .collect();

    let max_level = levels.iter().copied().max().unwrap_or(0);
    let min_odd = levels
        .iter()
        .copied()
        .filter(|&lv| lv % 2 == 1)
        .min()
        .unwrap_or(0);
    if max_level == 0 || min_odd == 0 {
        return;
    }

    // UAX #9 Rule L2: reverse every contiguous sequence at `level >= lv` from
    // `max_level` down to `min_odd`. Applying L2 to visual order recovers
    // logical order for both embedded RTL runs in LTR paragraphs and LTR
    // numbers/words inside RTL paragraphs.
    for lv in (min_odd..=max_level).rev() {
        let mut i = 0;
        while i < chars.len() {
            if levels[i] >= lv {
                let mut j = i + 1;
                while j < chars.len() && levels[j] >= lv {
                    j += 1;
                }
                chars[i..j].reverse();
                i = j;
            } else {
                i += 1;
            }
        }
    }
}

fn merge_selection(slice: &[PageChar], lines: &[TextLine]) -> Vec<(Quad, Rect)> {
    let mut out = Vec::new();
    let mut current: Option<(usize, u32, Rect, Rect, f32, f32, bool)> = None;

    for c in slice {
        if c.generated {
            if let Some((_, _, _, _, _, _, ref mut saw_gen_space)) = current
                && c.ch == ' '
            {
                *saw_gen_space = true;
            }
            continue;
        }
        let qb = c.quad.bounds();
        let p_min = c.proj_start.min(c.proj_end);
        let p_max = c.proj_start.max(c.proj_end);

        match current {
            Some((
                line_idx,
                ref mut cur_op,
                ref mut loose_acc,
                ref mut tight_acc,
                ref mut cur_min,
                ref mut cur_max,
                ref mut saw_gen_space,
            )) if line_idx == c.line => {
                let gap = if p_min > *cur_max {
                    p_min - *cur_max
                } else if *cur_min > p_max {
                    *cur_min - p_max
                } else {
                    0.0
                };
                let merge = (*cur_op != 0 && *cur_op == c.op_id)
                    || (!*saw_gen_space && gap <= c.space_page * INTER_OP_SPACE_FACTOR);
                if merge {
                    *loose_acc = loose_acc.union(qb);
                    *tight_acc = tight_acc.union(c.tight_box);
                    *cur_min = cur_min.min(p_min);
                    *cur_max = cur_max.max(p_max);
                    *cur_op = c.op_id;
                    *saw_gen_space = false;
                } else {
                    let prev_quad = oriented_line_quad(lines.get(line_idx), *loose_acc);
                    out.push((prev_quad, *tight_acc));
                    current = Some((c.line, c.op_id, qb, c.tight_box, p_min, p_max, false));
                }
            }
            Some((prev_line, _, loose_acc, tight_acc, _, _, _)) => {
                let prev_quad = oriented_line_quad(lines.get(prev_line), loose_acc);
                out.push((prev_quad, tight_acc));
                current = Some((c.line, c.op_id, qb, c.tight_box, p_min, p_max, false));
            }
            None => {
                current = Some((c.line, c.op_id, qb, c.tight_box, p_min, p_max, false));
            }
        }
    }

    if let Some((line_idx, _, loose_acc, tight_acc, _, _, _)) = current {
        let quad = oriented_line_quad(lines.get(line_idx), loose_acc);
        out.push((quad, tight_acc));
    }
    out
}

fn oriented_line_quad(line: Option<&TextLine>, bounds: Rect) -> Quad {
    let _ = line;
    Quad::from_rect(bounds)
}

fn normalize_needle(query: &str, match_case: bool) -> Vec<char> {
    let mut out = Vec::with_capacity(query.len());
    for ch in query.chars() {
        if ch.is_whitespace() {
            if out.last() != Some(&' ') {
                out.push(' ');
            }
        } else if match_case {
            out.push(ch);
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn is_word_boundary_before(chars: &[PageChar], start: usize) -> bool {
    if start == 0 || !is_word_char(chars[start].ch) {
        return true;
    }
    !is_word_char(chars[start - 1].ch)
}

fn is_word_boundary_after(chars: &[PageChar], end: usize) -> bool {
    if end == 0 || end >= chars.len() || !is_word_char(chars[end - 1].ch) {
        return true;
    }
    !is_word_char(chars[end].ch)
}
