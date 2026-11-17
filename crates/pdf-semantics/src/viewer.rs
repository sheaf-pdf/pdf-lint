//! Catalog display properties and `/ViewerPreferences`
//! (ISO 32000-2, §7.7.2 Table 29 and §12.2 Table 147).
//!
//! [`read`] inspects catalog `/PageLayout`, `/PageMode`, `/Lang`, and
//! `/ViewerPreferences`, reporting malformed entries via [`Fault`] while
//! preserving PDFium's `FPDF_VIEWERREF_*` and `FPDFCatalog_GetLanguage`
//! defaults. [`write_prefs`] and [`apply_catalog`] serialize them back onto a
//! catalog dictionary.

use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Str, Unreadable, Value, Why, dict, text};

use crate::info::len;
use crate::phases::phase;
use crate::{Fault, HOPS, too_long};

/// Catalog `/PageLayout` page arrangement (ISO 32000-2 §7.7.2 Table 29).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PageLayout {
    /// `/SinglePage` (default): display one page at a time.
    #[default]
    SinglePage,
    /// `/OneColumn`: display pages in one continuous column.
    OneColumn,
    /// `/TwoColumnLeft`: two columns, odd-numbered pages on the left.
    TwoColumnLeft,
    /// `/TwoColumnRight`: two columns, odd-numbered pages on the right.
    TwoColumnRight,
    /// `/TwoPageLeft`: two pages at a time, odd-numbered pages on the left.
    TwoPageLeft,
    /// `/TwoPageRight`: two pages at a time, odd-numbered pages on the right.
    TwoPageRight,
}

impl PageLayout {
    /// Parses a `/PageLayout` name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"SinglePage" => Some(Self::SinglePage),
            b"OneColumn" => Some(Self::OneColumn),
            b"TwoColumnLeft" => Some(Self::TwoColumnLeft),
            b"TwoColumnRight" => Some(Self::TwoColumnRight),
            b"TwoPageLeft" => Some(Self::TwoPageLeft),
            b"TwoPageRight" => Some(Self::TwoPageRight),
            _ => None,
        }
    }

    /// PDF name token.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::SinglePage => "SinglePage",
            Self::OneColumn => "OneColumn",
            Self::TwoColumnLeft => "TwoColumnLeft",
            Self::TwoColumnRight => "TwoColumnRight",
            Self::TwoPageLeft => "TwoPageLeft",
            Self::TwoPageRight => "TwoPageRight",
        }
    }
}

/// Catalog `/PageMode` initial panel state (ISO 32000-2 §7.7.2 Table 29).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PageMode {
    /// `/UseNone` (default): neither document outline nor thumbnail images visible.
    #[default]
    UseNone,
    /// `/UseOutlines`: document outline visible.
    UseOutlines,
    /// `/UseThumbs`: thumbnail images visible.
    UseThumbs,
    /// `/FullScreen`: full-screen mode with no menu bar or window controls.
    FullScreen,
    /// `/UseOC`: optional content group panel visible.
    UseOc,
    /// `/UseAttachments`: attachments panel visible.
    UseAttachments,
}

impl PageMode {
    /// Parses a `/PageMode` name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"UseNone" => Some(Self::UseNone),
            b"UseOutlines" => Some(Self::UseOutlines),
            b"UseThumbs" => Some(Self::UseThumbs),
            b"FullScreen" => Some(Self::FullScreen),
            b"UseOC" => Some(Self::UseOc),
            b"UseAttachments" => Some(Self::UseAttachments),
            _ => None,
        }
    }

    /// PDF name token.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::UseNone => "UseNone",
            Self::UseOutlines => "UseOutlines",
            Self::UseThumbs => "UseThumbs",
            Self::FullScreen => "FullScreen",
            Self::UseOc => "UseOC",
            Self::UseAttachments => "UseAttachments",
        }
    }
}

/// `/NonFullScreenPageMode` in `/ViewerPreferences` (ISO 32000-2 §12.2 Table 147).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum NonFullScreenPageMode {
    /// `/UseNone` (default).
    #[default]
    UseNone,
    /// `/UseOutlines`.
    UseOutlines,
    /// `/UseThumbs`.
    UseThumbs,
    /// `/UseOC`.
    UseOc,
}

impl NonFullScreenPageMode {
    /// Parses a `/NonFullScreenPageMode` name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"UseNone" => Some(Self::UseNone),
            b"UseOutlines" => Some(Self::UseOutlines),
            b"UseThumbs" => Some(Self::UseThumbs),
            b"UseOC" => Some(Self::UseOc),
            _ => None,
        }
    }

    /// PDF name token.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::UseNone => "UseNone",
            Self::UseOutlines => "UseOutlines",
            Self::UseThumbs => "UseThumbs",
            Self::UseOc => "UseOC",
        }
    }
}

/// Predominant reading order (`/Direction`, ISO 32000-2 §12.2 Table 147).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    /// `/L2R` (default): left-to-right.
    #[default]
    L2R,
    /// `/R2L`: right-to-left (including vertical writing systems).
    R2L,
}

impl Direction {
    /// Parses a `/Direction` name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"L2R" => Some(Self::L2R),
            b"R2L" => Some(Self::R2L),
            _ => None,
        }
    }

    /// PDF name token.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::L2R => "L2R",
            Self::R2L => "R2L",
        }
    }
}

/// Page boundary box name (`/ViewArea`, `/ViewClip`, `/PrintArea`, `/PrintClip`,
/// ISO 32000-2 §12.2 Table 147).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PageBoundary {
    /// `/MediaBox`.
    MediaBox,
    /// `/CropBox` (default).
    #[default]
    CropBox,
    /// `/BleedBox`.
    BleedBox,
    /// `/TrimBox`.
    TrimBox,
    /// `/ArtBox`.
    ArtBox,
}

impl PageBoundary {
    /// Parses a boundary box name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"MediaBox" => Some(Self::MediaBox),
            b"CropBox" => Some(Self::CropBox),
            b"BleedBox" => Some(Self::BleedBox),
            b"TrimBox" => Some(Self::TrimBox),
            b"ArtBox" => Some(Self::ArtBox),
            _ => None,
        }
    }

    /// PDF name token.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::MediaBox => "MediaBox",
            Self::CropBox => "CropBox",
            Self::BleedBox => "BleedBox",
            Self::TrimBox => "TrimBox",
            Self::ArtBox => "ArtBox",
        }
    }
}

/// Print scaling preference (`/PrintScaling`, ISO 32000-2 §12.2 Table 147).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PrintScaling {
    /// `/AppDefault` (default; `FPDF_VIEWERREF_GetPrintScaling` returns `true`).
    #[default]
    AppDefault,
    /// `/None`: print without page scaling (`FPDF_VIEWERREF_GetPrintScaling` returns `false`).
    None,
}

impl PrintScaling {
    /// Parses a `/PrintScaling` name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"AppDefault" => Some(Self::AppDefault),
            b"None" => Some(Self::None),
            _ => None,
        }
    }

    /// PDF name token.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::AppDefault => "AppDefault",
            Self::None => "None",
        }
    }
}

/// Duplex paper handling (`/Duplex`, ISO 32000-2 §12.2 Table 147).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Duplex {
    /// `/Simplex` (`FPDF_DUPLEXTYPE::Simplex = 1`): print single-sided.
    Simplex,
    /// `/DuplexFlipShortEdge` (`FPDF_DUPLEXTYPE::DuplexFlipShortEdge = 2`).
    DuplexFlipShortEdge,
    /// `/DuplexFlipLongEdge` (`FPDF_DUPLEXTYPE::DuplexFlipLongEdge = 3`).
    DuplexFlipLongEdge,
}

impl Duplex {
    /// Parses a `/Duplex` name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"Simplex" => Some(Self::Simplex),
            b"DuplexFlipShortEdge" => Some(Self::DuplexFlipShortEdge),
            b"DuplexFlipLongEdge" => Some(Self::DuplexFlipLongEdge),
            _ => None,
        }
    }

    /// PDF name token.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::Simplex => "Simplex",
            Self::DuplexFlipShortEdge => "DuplexFlipShortEdge",
            Self::DuplexFlipLongEdge => "DuplexFlipLongEdge",
        }
    }

    /// PDFium `FPDF_DUPLEXTYPE` integer (`1`..=`3`).
    #[must_use]
    pub const fn pdfium_code(self) -> i32 {
        match self {
            Self::Simplex => 1,
            Self::DuplexFlipShortEdge => 2,
            Self::DuplexFlipLongEdge => 3,
        }
    }
}

/// Decoded `/ViewerPreferences` dictionary (ISO 32000-2 §12.2 Table 147).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ViewerPrefs {
    /// `/HideToolbar` (`false` by default).
    pub hide_toolbar: bool,
    /// `/HideMenubar` (`false` by default).
    pub hide_menubar: bool,
    /// `/HideWindowUI` (`false` by default).
    pub hide_window_ui: bool,
    /// `/FitWindow` (`false` by default).
    pub fit_window: bool,
    /// `/CenterWindow` (`false` by default).
    pub center_window: bool,
    /// `/DisplayDocTitle` (`false` by default; required `true` by PDF/UA-1).
    pub display_doc_title: bool,
    /// `/NonFullScreenPageMode`.
    pub non_full_screen_page_mode: Option<NonFullScreenPageMode>,
    /// `/Direction`.
    pub direction: Option<Direction>,
    /// `/ViewArea`.
    pub view_area: Option<PageBoundary>,
    /// `/ViewClip`.
    pub view_clip: Option<PageBoundary>,
    /// `/PrintArea`.
    pub print_area: Option<PageBoundary>,
    /// `/PrintClip`.
    pub print_clip: Option<PageBoundary>,
    /// `/PrintScaling`.
    pub print_scaling: Option<PrintScaling>,
    /// `/Duplex`.
    pub duplex: Option<Duplex>,
    /// `/PickTrayByPDFSize`.
    pub pick_tray_by_pdf_size: Option<bool>,
    /// `/PrintPageRange` raw 1-based integers (ISO 32000-2 §12.2: pairs of
    /// 1-based `[first, last]` page numbers, capped to 4,096 elements).
    pub print_page_range: Vec<i32>,
    /// `/NumCopies` (`None` when absent; PDFium defaults `FPDF_VIEWERREF_GetNumCopies` to `1`).
    pub num_copies: Option<i32>,
}

impl ViewerPrefs {
    /// Returns whether print scaling is enabled (`true` unless
    /// `self.print_scaling == Some(PrintScaling::None)`), matching
    /// `FPDF_VIEWERREF_GetPrintScaling`.
    #[must_use]
    pub fn print_scaling_enabled(&self) -> bool {
        self.print_scaling != Some(PrintScaling::None)
    }

    /// Effective copy count (`1` when `/NumCopies` is absent), matching
    /// `FPDF_VIEWERREF_GetNumCopies`.
    #[must_use]
    pub fn effective_num_copies(&self) -> i32 {
        self.num_copies.unwrap_or(1)
    }
}

/// Combined catalog display settings (`/PageLayout`, `/PageMode`, `/Lang`, and
/// `/ViewerPreferences`).
///
/// Its `Debug` implementation reports `lang`'s byte length rather than the raw
/// language string.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CatalogView {
    /// Catalog `/PageLayout`, if present and valid.
    pub page_layout: Option<PageLayout>,
    /// Catalog `/PageMode`, if present and valid.
    pub page_mode: Option<PageMode>,
    /// Catalog `/Lang` BCP 47 language tag, if present.
    pub lang: Option<String>,
    /// Catalog `/ViewerPreferences`, if present and a dictionary.
    pub viewer_prefs: Option<ViewerPrefs>,
}

impl fmt::Debug for CatalogView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CatalogView")
            .field("page_layout", &self.page_layout)
            .field("page_mode", &self.page_mode)
            .field("lang", &len(&self.lang))
            .field("viewer_prefs", &self.viewer_prefs)
            .finish()
    }
}

const MAX_PRINT_RANGE_ELEMENTS: usize = 4096;

/// Reads catalog display entries (`/PageLayout`, `/PageMode`, `/Lang`, and
/// `/ViewerPreferences`) from `catalog`.
///
/// # Errors
///
/// Returns [`Unreadable`] on reader caps, locked objects/strings, or reference
/// loops.
pub fn read<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    sink: &mut S,
) -> Result<CatalogView, Unreadable> {
    let page_layout = enum_entry(
        &objects,
        catalog,
        None,
        "PageLayout",
        PageLayout::from_name,
        sink,
    )?;
    let page_mode = enum_entry(
        &objects,
        catalog,
        None,
        "PageMode",
        PageMode::from_name,
        sink,
    )?;
    let lang = match catalog.get(b"Lang") {
        Some(raw) => match reached(&objects, raw, None, sink)? {
            Some((obj, lid)) => match obj.value() {
                Value::String(Str::Bytes(bytes), _) => Some(text::decode_str(bytes, rules)),
                Value::String(Str::Encrypted(_), _) => {
                    let oid = lid.unwrap_or(ObjectId::new(0, 0));
                    return Err(Unreadable::new(
                        oid,
                        Why::Locked,
                        format!("{oid} R holds /Lang as ciphertext: it takes a password"),
                    ));
                }
                _ => {
                    say(
                        sink,
                        lid,
                        obj.at(),
                        Fault::ViewerPrefWrongType { key: "Lang" },
                    );
                    None
                }
            },
            None => None,
        },
        None => None,
    };

    let viewer_prefs = match catalog.get(b"ViewerPreferences") {
        Some(raw_vp) => match reached(&objects, raw_vp, None, sink)? {
            Some((vp_obj, vp_id)) => match vp_obj.as_dict() {
                Some(vp_dict) => Some(read_prefs(&objects, vp_dict, vp_id, sink)?),
                None => {
                    say(sink, vp_id, vp_obj.at(), Fault::ViewerPrefInvalid);
                    None
                }
            },
            None => None,
        },
        None => None,
    };

    phase!(DEBUG, "viewer::read", has_prefs = viewer_prefs.is_some());
    Ok(CatalogView {
        page_layout,
        page_mode,
        lang,
        viewer_prefs,
    })
}

fn read_prefs<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    sink: &mut S,
) -> Result<ViewerPrefs, Unreadable> {
    let hide_toolbar = bool_entry(objects, dict, within, "HideToolbar", sink)?.unwrap_or(false);
    let hide_menubar = bool_entry(objects, dict, within, "HideMenubar", sink)?.unwrap_or(false);
    let hide_window_ui = bool_entry(objects, dict, within, "HideWindowUI", sink)?.unwrap_or(false);
    let fit_window = bool_entry(objects, dict, within, "FitWindow", sink)?.unwrap_or(false);
    let center_window = bool_entry(objects, dict, within, "CenterWindow", sink)?.unwrap_or(false);
    let display_doc_title =
        bool_entry(objects, dict, within, "DisplayDocTitle", sink)?.unwrap_or(false);
    let non_full_screen_page_mode = enum_entry(
        objects,
        dict,
        within,
        "NonFullScreenPageMode",
        NonFullScreenPageMode::from_name,
        sink,
    )?;
    let direction = enum_entry(
        objects,
        dict,
        within,
        "Direction",
        Direction::from_name,
        sink,
    )?;
    let view_area = enum_entry(
        objects,
        dict,
        within,
        "ViewArea",
        PageBoundary::from_name,
        sink,
    )?;
    let view_clip = enum_entry(
        objects,
        dict,
        within,
        "ViewClip",
        PageBoundary::from_name,
        sink,
    )?;
    let print_area = enum_entry(
        objects,
        dict,
        within,
        "PrintArea",
        PageBoundary::from_name,
        sink,
    )?;
    let print_clip = enum_entry(
        objects,
        dict,
        within,
        "PrintClip",
        PageBoundary::from_name,
        sink,
    )?;
    let print_scaling = enum_entry(
        objects,
        dict,
        within,
        "PrintScaling",
        PrintScaling::from_name,
        sink,
    )?;
    let duplex = enum_entry(objects, dict, within, "Duplex", Duplex::from_name, sink)?;
    let pick_tray_by_pdf_size = bool_entry(objects, dict, within, "PickTrayByPDFSize", sink)?;

    let mut print_page_range = Vec::new();
    if let Some(raw_ppr) = dict.get(b"PrintPageRange")
        && let Some((ppr_obj, ppr_id)) = reached(objects, raw_ppr, within, sink)?
    {
        if let Some(arr) = ppr_obj.as_array() {
            if arr.is_empty() || arr.len() % 2 != 0 {
                say(sink, ppr_id, ppr_obj.at(), Fault::ViewerPrefInvalid);
            }
            for item in arr.iter().take(MAX_PRINT_RANGE_ELEMENTS) {
                let Some((io, iid)) = reached(objects, item, ppr_id, sink)? else {
                    say(
                        sink,
                        ppr_id,
                        item.at(),
                        Fault::ViewerPrefWrongType {
                            key: "PrintPageRange",
                        },
                    );
                    continue;
                };
                match io.as_integer().and_then(|n| i32::try_from(n).ok()) {
                    Some(n) => {
                        if n < 1 {
                            say(sink, iid, io.at(), Fault::ViewerPrefInvalid);
                        }
                        print_page_range.push(n);
                    }
                    None => say(
                        sink,
                        iid,
                        io.at(),
                        Fault::ViewerPrefWrongType {
                            key: "PrintPageRange",
                        },
                    ),
                }
            }
        } else {
            say(
                sink,
                ppr_id,
                ppr_obj.at(),
                Fault::ViewerPrefWrongType {
                    key: "PrintPageRange",
                },
            );
        }
    }

    let num_copies = match dict.get(b"NumCopies") {
        Some(raw) => match reached(objects, raw, within, sink)? {
            Some((o, nid)) => match o.as_integer().and_then(|n| i32::try_from(n).ok()) {
                Some(n) => {
                    if n < 1 {
                        say(
                            sink,
                            nid,
                            o.at(),
                            Fault::ViewerPrefWrongType { key: "NumCopies" },
                        );
                    }
                    Some(n)
                }
                None => {
                    say(
                        sink,
                        nid,
                        o.at(),
                        Fault::ViewerPrefWrongType { key: "NumCopies" },
                    );
                    None
                }
            },
            None => None,
        },
        None => None,
    };

    Ok(ViewerPrefs {
        hide_toolbar,
        hide_menubar,
        hide_window_ui,
        fit_window,
        center_window,
        display_doc_title,
        non_full_screen_page_mode,
        direction,
        view_area,
        view_clip,
        print_area,
        print_clip,
        print_scaling,
        duplex,
        pick_tray_by_pdf_size,
        print_page_range,
        num_copies,
    })
}

fn bool_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
    sink: &mut S,
) -> Result<Option<bool>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = reached(objects, raw, within, sink)? else {
        return Ok(None);
    };
    match obj.as_bool() {
        Some(b) => Ok(Some(b)),
        None => {
            say(sink, id, obj.at(), Fault::ViewerPrefWrongType { key });
            Ok(None)
        }
    }
}

fn enum_entry<'o, T, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
    parse: fn(&[u8]) -> Option<T>,
    sink: &mut S,
) -> Result<Option<T>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = reached(objects, raw, within, sink)? else {
        return Ok(None);
    };
    match obj.as_name().and_then(parse) {
        Some(v) => Ok(Some(v)),
        None => {
            say(sink, id, obj.at(), Fault::ViewerPrefWrongType { key });
            Ok(None)
        }
    }
}

fn say<S: Sink<Fault> + ?Sized>(
    sink: &mut S,
    id: Option<ObjectId>,
    at: Option<Span>,
    fault: Fault,
) {
    if sink.wants() {
        sink.fault(Place::new(id, at), fault);
    }
}

fn reached<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    value: &'o Object,
    within: Option<ObjectId>,
    sink: &mut S,
) -> Result<Option<(&'o Object, Option<ObjectId>)>, Unreadable> {
    let (mut at, mut last) = (value, None);
    for _ in 0..HOPS {
        match at.value() {
            Value::Ref(id) => {
                last = Some(*id);
                match objects.try_resolve(*id) {
                    Ok(Some(obj)) => at = obj,
                    Ok(None) => return Ok(None),
                    Err(u) if u.why == Why::Damaged => {
                        say(sink, within, value.at(), Fault::Damaged { id: *id });
                        return Ok(None);
                    }
                    Err(u) => return Err(u),
                }
            }
            Value::Null => return Ok(None),
            _ => return Ok(Some((at, last.or(within)))),
        }
    }
    Err(too_long(last.unwrap_or(ObjectId::new(0, 0))))
}

/// Serializes `prefs` into a `/ViewerPreferences` dictionary (omitting default
/// `false`/`None` entries).
#[must_use]
pub fn write_prefs(prefs: &ViewerPrefs) -> Dict {
    let mut d = dict! {};
    if prefs.hide_toolbar {
        d.set("HideToolbar", true);
    }
    if prefs.hide_menubar {
        d.set("HideMenubar", true);
    }
    if prefs.hide_window_ui {
        d.set("HideWindowUI", true);
    }
    if prefs.fit_window {
        d.set("FitWindow", true);
    }
    if prefs.center_window {
        d.set("CenterWindow", true);
    }
    if prefs.display_doc_title {
        d.set("DisplayDocTitle", true);
    }
    if let Some(m) = prefs.non_full_screen_page_mode {
        d.set("NonFullScreenPageMode", Object::name(m.as_name()));
    }
    if let Some(dir) = prefs.direction {
        d.set("Direction", Object::name(dir.as_name()));
    }
    if let Some(b) = prefs.view_area {
        d.set("ViewArea", Object::name(b.as_name()));
    }
    if let Some(b) = prefs.view_clip {
        d.set("ViewClip", Object::name(b.as_name()));
    }
    if let Some(b) = prefs.print_area {
        d.set("PrintArea", Object::name(b.as_name()));
    }
    if let Some(b) = prefs.print_clip {
        d.set("PrintClip", Object::name(b.as_name()));
    }
    if let Some(ps) = prefs.print_scaling {
        d.set("PrintScaling", Object::name(ps.as_name()));
    }
    if let Some(dup) = prefs.duplex {
        d.set("Duplex", Object::name(dup.as_name()));
    }
    if let Some(pt) = prefs.pick_tray_by_pdf_size {
        d.set("PickTrayByPDFSize", pt);
    }
    if !prefs.print_page_range.is_empty() {
        let arr: Vec<Object> = prefs
            .print_page_range
            .iter()
            .map(|&n| Object::from(i64::from(n)))
            .collect();
        d.set("PrintPageRange", arr);
    }
    if let Some(nc) = prefs.num_copies {
        d.set("NumCopies", i64::from(nc));
    }
    d
}

/// Applies `view`'s `/PageLayout`, `/PageMode`, `/Lang`, and `/ViewerPreferences`
/// onto `catalog`.
pub fn apply_catalog(catalog: &mut Dict, view: &CatalogView) {
    if let Some(layout) = view.page_layout {
        catalog.set("PageLayout", Object::name(layout.as_name()));
    } else {
        catalog.remove(b"PageLayout");
    }
    if let Some(mode) = view.page_mode {
        catalog.set("PageMode", Object::name(mode.as_name()));
    } else {
        catalog.remove(b"PageMode");
    }
    if let Some(lang) = &view.lang {
        catalog.set("Lang", Object::text(lang));
    } else {
        catalog.remove(b"Lang");
    }
    if let Some(prefs) = &view.viewer_prefs {
        catalog.set("ViewerPreferences", write_prefs(prefs));
    } else {
        catalog.remove(b"ViewerPreferences");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId, dict};

    use super::{
        CatalogView, Direction, Duplex, PageLayout, PageMode, PrintScaling, ViewerPrefs,
        apply_catalog, read,
    };

    #[test]
    fn round_trip_catalog_view_and_debug_hygiene() {
        let prefs = ViewerPrefs {
            display_doc_title: true,
            direction: Some(Direction::R2L),
            print_scaling: Some(PrintScaling::None),
            duplex: Some(Duplex::DuplexFlipLongEdge),
            print_page_range: vec![1, 3, 5, 8],
            num_copies: Some(2),
            ..ViewerPrefs::default()
        };

        let view = CatalogView {
            page_layout: Some(PageLayout::TwoPageLeft),
            page_mode: Some(PageMode::UseOutlines),
            lang: Some("ja-JP".into()),
            viewer_prefs: Some(prefs),
        };

        let mut catalog = dict! { "Type" => Object::name("Catalog") };
        apply_catalog(&mut catalog, &view);

        let objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
        let mut faults = Vec::new();
        let decoded = read(&objects, &catalog, Rules::Iso, &mut faults).unwrap();
        assert!(faults.is_empty(), "{faults:?}");
        assert_eq!(decoded, view);
        assert!(
            !decoded
                .viewer_prefs
                .as_ref()
                .unwrap()
                .print_scaling_enabled()
        );
        assert_eq!(
            decoded
                .viewer_prefs
                .as_ref()
                .unwrap()
                .effective_num_copies(),
            2
        );

        let dbg = format!("{decoded:?}");
        assert!(!dbg.contains("ja-JP"), "{dbg}");
    }
}
