//! Resource caps for bounded annotation inspection and reply-thread
//! resolution.

/// Per-call resource caps enforced by `pdf-annots` readers and walkers.
///
/// Every traversal over `/Annots` arrays, `/IRT` reply chains, `/QuadPoints`,
/// `/Vertices`, `/InkList` strokes, `/DA` content fragments, and delegated
/// `pdf-semantics` action/destination walks terminates deterministically within
/// fixed memory and CPU bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum indirect reference hops followed when resolving a value (`32` by
    /// default).
    pub hops: usize,
    /// Maximum entries inspected in a single page's `/Annots` array (`16_384`
    /// by default).
    pub annots_per_page: usize,
    /// Maximum `/IRT` reply hops followed when resolving reply roots and review
    /// states (`256` by default).
    pub reply_depth: usize,
    /// Maximum quadrilaterals read from a `/QuadPoints` array (`16_384` by
    /// default).
    pub quads: usize,
    /// Maximum points read across a `/Vertices` array or `/InkList` stroke set
    /// (`65_536` by default).
    pub points: usize,
    /// Content-stream lexing caps used when parsing `/DA` default appearance
    /// strings.
    pub content: pdf_content_streams::Limits,
    /// Action-chain, name-tree, and page-tree caps forwarded to
    /// `pdf-semantics`.
    pub semantics: pdf_semantics::Limits,
}

impl Limits {
    /// Default caps suitable for untrusted real-world PDFs.
    pub const DEFAULT: Limits = Limits {
        hops: 32,
        annots_per_page: 16_384,
        reply_depth: 256,
        quads: 16_384,
        points: 65_536,
        content: pdf_content_streams::Limits::DEFAULT,
        semantics: pdf_semantics::Limits::DEFAULT,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// Which [`Limits`] cap ended an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Cap {
    /// Entries in a page's `/Annots` array exceeded [`Limits::annots_per_page`].
    AnnotsPerPage,
    /// Actions in an action chain exceeded `Limits::semantics.action_chain`.
    ActionChain,
    /// Quadrilaterals in `/QuadPoints` exceeded [`Limits::quads`].
    Quads,
    /// Points in `/Vertices` or `/InkList` exceeded [`Limits::points`].
    Points,
    /// Entries visited in `/Names/Dests` exceeded `Limits::semantics` tree caps.
    DestTree,
}

impl std::fmt::Display for Cap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cap::AnnotsPerPage => f.write_str("annotations per page"),
            Cap::ActionChain => f.write_str("chained actions"),
            Cap::Quads => f.write_str("quadrilaterals in /QuadPoints"),
            Cap::Points => f.write_str("points in /Vertices or /InkList"),
            Cap::DestTree => f.write_str("destination name-tree nodes"),
        }
    }
}
