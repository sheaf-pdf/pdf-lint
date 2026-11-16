//! Per-page resource caps for bounded content interpretation and re-encoding.

/// Per-page caps enforced by [`interpret`](crate::interpret) and
/// [`encode`](crate::encode).
///
/// Every counter spans the page's content stream and all Form XObjects drawn on
/// it so fan-out bombs (`Do` trees) and font decompression bombs terminate
/// deterministically within fixed memory and CPU bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Lexing, operand-nesting, inline-image, and color-space caps passed to
    /// `pdf-content-streams`.
    pub content: pdf_content_streams::Limits,
    /// Font dictionary, CMap, Type 3 CharProc, and outline-fuel caps passed to
    /// `pdf-fonts`.
    pub fonts: pdf_fonts::Limits,
    /// Deepest Form XObject nesting (`Do` inside `Do`) followed; deeper uses
    /// emit [`Kind::FormRefused`](crate::Kind::FormRefused) with
    /// [`Refused::Depth`](crate::Refused::Depth) (`40` by default, matching
    /// PDFium's `kMaxFormLevel`).
    pub form_depth: usize,
    /// Maximum `q` graphics-state saves active simultaneously within one
    /// content stream (`256` by default).
    pub states: usize,
    /// Maximum operations interpreted across the page and every Form XObject
    /// use (`2_000_000` by default).
    pub operators: usize,
    /// Maximum glyphs placed across the page and every Form XObject use
    /// (`2_000_000` by default).
    pub glyphs: usize,
    /// Maximum path segments held across the active clip path and the path
    /// currently being built (`2_000_000` by default).
    pub segments: usize,
    /// Maximum nested marked-content sequences (`BMC`/`BDC`) open at once
    /// across the page and active Form XObjects (`256` by default).
    pub marked: usize,
    /// Maximum total content bytes interpreted across the page and every Form
    /// XObject use, charging a form's byte length each time it is drawn so
    /// whitespace-heavy `Do` fan-outs are bounded (`256 MiB` by default).
    pub interpreted: usize,
    /// Shared decompression and glyph-outline budget in bytes across the
    /// page's content, every distinct Form XObject stream, every font's
    /// streams, and CID glyph outline evaluation (`128 MiB` by default).
    pub decompressed: usize,
}

impl Limits {
    /// Default per-page caps suitable for untrusted real-world PDFs.
    pub const DEFAULT: Limits = Limits {
        content: pdf_content_streams::Limits::DEFAULT,
        fonts: pdf_fonts::Limits::DEFAULT,
        form_depth: 40,
        states: 256,
        operators: 2_000_000,
        glyphs: 2_000_000,
        segments: 2_000_000,
        marked: 256,
        interpreted: 256 << 20,
        decompressed: 128 << 20,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// Which [`Limits`] cap ended an interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Cap {
    /// `q` saves active at once exceeded [`Limits::states`].
    States,
    /// Operations interpreted exceeded [`Limits::operators`].
    Operators,
    /// Glyphs placed exceeded [`Limits::glyphs`].
    Glyphs,
    /// Path segments in the clip and active path exceeded [`Limits::segments`].
    Segments,
    /// Open `BMC`/`BDC` sequences exceeded [`Limits::marked`].
    Marked,
    /// Total content bytes interpreted across form uses exceeded
    /// [`Limits::interpreted`].
    Interpreted,
    /// Total bytes decompressed across streams, fonts, and glyph outlines
    /// exceeded [`Limits::decompressed`].
    Decompressed,
}

impl std::fmt::Display for Cap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cap::States => f.write_str("saved graphics states"),
            Cap::Operators => f.write_str("operators interpreted"),
            Cap::Glyphs => f.write_str("glyphs placed"),
            Cap::Segments => f.write_str("path segments"),
            Cap::Marked => f.write_str("nested marked-content sequences"),
            Cap::Interpreted => f.write_str("content bytes interpreted"),
            Cap::Decompressed => f.write_str("bytes decompressed"),
        }
    }
}
