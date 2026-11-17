//! Zero-I/O page annotation flattening (`plan_flatten`, ISO 32000-2 §12.5.5).

use std::fmt;

use pdf_content_streams::{Res, ResBody, ResKind, Reserved};
use pdf_gfx::Canvas;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve};

use crate::annot::{Annot, page_annots};
use crate::ap::{AppearanceMode, stream_placement_matrix};
use crate::error::Error;
use crate::limits::Limits;
use crate::phases::phase;
use crate::subtypes::{AnnotSubtype, TypedAnnot};

/// Target medium when flattening annotations onto page content (matching
/// PDFium's `FLAT_NORMALDISPLAY` vs. `FLAT_PRINT`).
///
/// Exhaustive on purpose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FlattenTarget {
    /// Normal on-screen display (`Annot::is_visible_on_screen`).
    #[default]
    Display,
    /// Printed output (`Annot::is_printable`).
    Print,
}

/// Options controlling [`plan_flatten`] and [`plan_flatten_filtered`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlattenOpts {
    /// Whether to flatten for screen display or print (`Display` by default).
    pub target: FlattenTarget,
    /// When `true` (`true` by default), stateful `/AP /N` subdictionaries
    /// require an explicit `/AS` match rather than picking an arbitrary state.
    pub strict_state: bool,
    /// Whether `/Subtype /Widget` form annotations are included (`true` by
    /// default; set `false` to flatten markup/non-form annotations while
    /// keeping interactive form widgets live).
    pub include_widgets: bool,
    /// Whether `/Subtype /Redact` marks are flattened (`false` by default).
    ///
    /// A redaction mark only says what is to be removed (ISO 32000-2
    /// §12.5.6.23): until it is applied, the glyphs, paths and pixels under
    /// it are still in the file. Flattening paints the mark's appearance
    /// over them and takes the mark off the page, which leaves a page that
    /// looks redacted and isn't. So a mark is passed over by default: it is
    /// neither drawn nor removed, it stays in [`FlattenPlan::kept_annots`],
    /// and a `select` closure is never asked about it. Its `/Popup` (the
    /// pop-up it names, or one whose `/Parent` is the mark) is kept with it
    /// in the same way.
    ///
    /// Set `true` only to draw the marks anyway. That is what PDFium's
    /// `FPDFPage_Flatten` does (as of chromium/8152): it draws and removes a
    /// `/Redact` like any other annotation, so the default here departs
    /// from it on purpose. Flattening is never redaction: under either
    /// setting the content beneath a mark is left in the file.
    pub include_redactions: bool,
}

impl FlattenOpts {
    /// Default flattening options (`Display`, `strict_state: true`,
    /// `include_widgets: true`, `include_redactions: false`).
    pub const DEFAULT: Self = Self {
        target: FlattenTarget::Display,
        strict_state: true,
        include_widgets: true,
        include_redactions: false,
    };

    /// Creates flattening options for `target`.
    #[must_use]
    pub const fn for_target(target: FlattenTarget) -> Self {
        Self {
            target,
            ..Self::DEFAULT
        }
    }

    /// Sets `strict_state`.
    #[must_use]
    pub const fn with_strict_state(mut self, strict_state: bool) -> Self {
        self.strict_state = strict_state;
        self
    }

    /// Sets `include_widgets`.
    #[must_use]
    pub const fn with_widgets(mut self, include_widgets: bool) -> Self {
        self.include_widgets = include_widgets;
        self
    }

    /// Sets `include_redactions`: `true` draws `/Redact` marks onto the page
    /// and removes them like any other annotation, which hides what they
    /// cover without removing it (see [`Self::include_redactions`]).
    #[must_use]
    pub const fn with_redactions(mut self, include_redactions: bool) -> Self {
        self.include_redactions = include_redactions;
        self
    }
}

impl Default for FlattenOpts {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Result of planning annotation flattening for a single page.
///
/// Its `Debug` implementation reports byte lengths and counts rather than raw
/// content bytes or resource names.
#[derive(Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct FlattenPlan {
    /// Content-stream bytes placing every flattened annotation's `/AP /N` Form
    /// XObject in `/Annots` order (`q <matrix> cm /<Name> Do Q` per drawn
    /// appearance).
    ///
    /// To preserve graphics-state isolation when appending after an existing
    /// page stream, wrap the original page stream in `q\n...\nQ\n` (or use
    /// [`Self::combine_with_page`]).
    pub overlay_content: Vec<u8>,
    /// Collision-free `(xobject_name, form_stream_id)` bindings to add to the
    /// page's `/Resources/XObject` dictionary.
    pub xobject_bindings: Vec<(Vec<u8>, ObjectId)>,
    /// `/Annots` array elements kept on the page (empty when every annotation
    /// was selected for flattening).
    pub kept_annots: Vec<Object>,
    /// Indirect `ObjectId`s of annotations removed from `/Annots` (useful for
    /// pruning structure-tree `/OBJR` or `/AcroForm/Fields` entries).
    pub removed_ids: Vec<ObjectId>,
    /// Number of annotations whose `/AP /N` stream was drawn into
    /// `overlay_content`.
    pub drawn_count: usize,
    /// Number of `/Annots` slots removed from the page.
    pub removed_count: usize,
}

impl fmt::Debug for FlattenPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FlattenPlan")
            .field("overlay_len", &self.overlay_content.len())
            .field("xobject_bindings", &self.xobject_bindings.len())
            .field("kept_annots", &self.kept_annots.len())
            .field("removed_ids", &self.removed_ids)
            .field("drawn_count", &self.drawn_count)
            .field("removed_count", &self.removed_count)
            .finish()
    }
}

impl FlattenPlan {
    /// Returns `true` if any annotation was drawn or removed from `/Annots`.
    #[must_use]
    pub const fn changed(&self) -> bool {
        self.drawn_count > 0 || self.removed_count > 0
    }

    /// Combines `existing_page_content` with `self.overlay_content`, isolating
    /// `existing_page_content` inside `q\n...\nQ\n` so an unclosed CTM or color
    /// on the page cannot distort flattened annotations.
    #[must_use]
    pub fn combine_with_page(&self, existing_page_content: &[u8]) -> Vec<u8> {
        if self.overlay_content.is_empty() {
            return existing_page_content.to_vec();
        }
        if existing_page_content.is_empty() {
            return self.overlay_content.clone();
        }
        let mut out =
            Vec::with_capacity(existing_page_content.len() + self.overlay_content.len() + 6);
        out.extend_from_slice(b"q\n");
        out.extend_from_slice(existing_page_content);
        if !existing_page_content.ends_with(b"\n") {
            out.push(b'\n');
        }
        out.extend_from_slice(b"Q\n");
        out.extend_from_slice(&self.overlay_content);
        out
    }
}

/// Plans flattening of all eligible annotations on `page_dict` according to
/// `opts`.
///
/// `/Redact` marks are kept on the page, undrawn, unless
/// [`FlattenOpts::include_redactions`] is set: painting a mark over content
/// that is still in the file would only make the page look redacted.
/// Flattening never removes the content under a mark.
///
/// # Errors
///
/// Returns [`Error`] if reading `/Annots` or an appearance Form XObject stream
/// fails.
pub fn plan_flatten<'o, R: Resolve<'o> + Copy>(
    objects: R,
    page_dict: &'o Dict,
    page_id: Option<ObjectId>,
    page_resources: Option<&'o Object>,
    opts: FlattenOpts,
    limits: &Limits,
) -> Result<FlattenPlan, Error> {
    plan_flatten_filtered(
        objects,
        page_dict,
        page_id,
        page_resources,
        opts,
        limits,
        |_| true,
    )
}

/// Like [`plan_flatten`], flattening only annotations for which `select`
/// returns `true` (keeping unselected annotations in `FlattenPlan::kept_annots`).
///
/// `select` is not asked about a widget that [`FlattenOpts::include_widgets`]
/// leaves out, nor about a `/Redact` mark or its pop-up unless
/// [`FlattenOpts::include_redactions`] is set: those are kept whatever it
/// would answer.
///
/// # Errors
///
/// Same error conditions as [`plan_flatten`].
pub fn plan_flatten_filtered<'o, R, F>(
    objects: R,
    page_dict: &'o Dict,
    page_id: Option<ObjectId>,
    page_resources: Option<&'o Object>,
    opts: FlattenOpts,
    limits: &Limits,
    mut select: F,
) -> Result<FlattenPlan, Error>
where
    R: Resolve<'o> + Copy,
    F: FnMut(&Annot<'o>) -> bool,
{
    let annots = page_annots(objects, page_dict, page_id, Rules::Pdfium, limits)?;
    phase!(DEBUG, "plan_flatten", annots = annots.len());
    if annots.is_empty() {
        return Ok(FlattenPlan::default());
    }

    let raw_items: Option<&[Object]> = page_dict.get(b"Annots").and_then(|raw| {
        let obj = match raw.as_reference() {
            Some(id) => objects.resolve(id)?,
            None => raw,
        };
        obj.as_array()
    });

    let mut canvas = Canvas::new();
    let mut kept_slots = vec![true; raw_items.map_or(0, <[Object]>::len)];
    let mut removed_ids = Vec::new();
    let mut drawn_count = 0usize;
    let mut removed_count = 0usize;

    // The pop-ups of the redaction marks that are passed over: the ones a
    // mark's `/Popup` names, and the ones whose `/Parent` is a mark.
    let mut mark_ids = Vec::new();
    let mut mark_popups = Vec::new();
    if !opts.include_redactions {
        for annot in &annots {
            if *annot.subtype() == AnnotSubtype::Redact {
                mark_ids.extend(annot.id());
                mark_popups.extend(annot.markup().and_then(|m| m.popup));
            }
        }
    }

    for annot in &annots {
        let is_widget = *annot.subtype() == AnnotSubtype::Widget;
        if is_widget && !opts.include_widgets {
            continue;
        }
        // An unapplied redaction mark stays a mark: drawn onto the page it
        // would hide content that is still in the file.
        let is_redaction = *annot.subtype() == AnnotSubtype::Redact;
        if is_redaction && !opts.include_redactions {
            continue;
        }
        // A kept mark keeps its pop-up, so its `/Popup` still names an
        // annotation of the page.
        if let TypedAnnot::Popup(popup) = annot.typed()
            && (annot.id().is_some_and(|id| mark_popups.contains(&id))
                || popup.parent.is_some_and(|id| mark_ids.contains(&id)))
        {
            continue;
        }
        if !select(annot) {
            continue;
        }

        if let Some(slot) = annot.index()
            && let Some(flag) = kept_slots.get_mut(slot)
        {
            *flag = false;
        }
        removed_count += 1;
        if let Some(id) = annot.id() {
            removed_ids.push(id);
        }

        // `/Popup` UI windows never flatten visually onto page content.
        if *annot.subtype() == AnnotSubtype::Popup {
            continue;
        }
        let visible = match opts.target {
            FlattenTarget::Display => annot.is_visible_on_screen(),
            FlattenTarget::Print => annot.is_printable(),
        };
        if !visible || !annot.has_valid_rect() {
            continue;
        }
        let Some(ap) = annot.appearances() else {
            continue;
        };
        let Some(form_id) = ap.stream(AppearanceMode::Normal, opts.strict_state, false) else {
            continue;
        };
        let Some((_, _, place_mat)) =
            stream_placement_matrix(objects, form_id, annot.rect(), limits)?
        else {
            continue;
        };
        canvas.draw_xobject(Res::written(form_id), place_mat);
        drawn_count += 1;
    }

    let reserved = Reserved::read(objects, page_resources, &limits.content);
    let (overlay_content, needs) = canvas.finish(&reserved);
    let mut xobject_bindings = Vec::with_capacity(needs.0.len());
    for need in needs.0 {
        if need.kind == ResKind::XObject
            && let ResBody::Written(id) = need.res.body()
        {
            xobject_bindings.push((need.name, *id));
        }
    }

    let mut kept_annots = Vec::new();
    if let Some(items) = raw_items {
        for (idx, item) in items.iter().enumerate() {
            // Drop dangling/non-dict entries alongside flattened slots, keeping
            // only slots that resolved to an unselected annotation.
            if kept_slots.get(idx).copied().unwrap_or(false)
                && annots.iter().any(|a| a.index() == Some(idx))
            {
                kept_annots.push(item.clone());
            }
        }
    }

    Ok(FlattenPlan {
        overlay_content,
        xobject_bindings,
        kept_annots,
        removed_ids,
        drawn_count,
        removed_count,
    })
}
