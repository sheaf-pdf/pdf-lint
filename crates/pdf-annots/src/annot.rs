//! Core annotation wrapper (`Annot`), markup metadata (`MarkupInfo`), and page
//! `/Annots` + `/Tabs` traversal (ISO 32000-2 §12.5.2 & §12.5.6.2).

use std::collections::HashSet;
use std::fmt;

use pdf_gfx::Rect;
use pdf_objects::fault::Sink;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, ObjectId, Resolve, Value};
use pdf_semantics::action::AdditionalActions;
use pdf_semantics::info::Dated;

use crate::ap::{Appearances, read_appearances};
use crate::error::Error;
use crate::fault::Fault;
use crate::flags::AnnotFlags;
use crate::limits::{Cap, Limits};
use crate::phases::phase;
use crate::replies::ReplyKind;
use crate::resolve::Reader;
use crate::style::{AnnotColor, Border, read_border, read_color, read_legacy_border};
use crate::subtypes::{AnnotSubtype, TypedAnnot, read_typed};

/// Page tabbing order (`/Tabs` on a `/Page` dictionary, ISO 32000-2 §7.7.3.3
/// Table 31; required to be `/S` on annotated pages in PDF/UA-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TabOrder {
    /// `/R`: row order.
    Row,
    /// `/C`: column order.
    Column,
    /// `/S`: structure order.
    Structure,
    /// `/A`: `/Annots` array order (PDF 2.0).
    Annotations,
    /// `/W`: widget order (PDF 2.0).
    Widget,
}

impl TabOrder {
    /// Parses a `/Tabs` name byte slice.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"R" => Self::Row,
            b"C" => Self::Column,
            b"S" => Self::Structure,
            b"A" => Self::Annotations,
            b"W" => Self::Widget,
            _ => return None,
        })
    }
}

/// Reads the `/Tabs` entry on `page_dict`, if present.
///
/// # Errors
///
/// Returns [`Error::Unreadable`] if an indirect `/Tabs` object cannot be read.
pub fn page_tabs<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    page_dict: &'o Dict,
    page_id: Option<ObjectId>,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Option<TabOrder>, Error> {
    let mut reader = Reader {
        objects,
        rules,
        limits,
        sink,
    };
    let Some(name) = reader.name_entry(page_dict, page_id, "Tabs")? else {
        return Ok(None);
    };
    match TabOrder::from_name(name) {
        Some(tab) => Ok(Some(tab)),
        None => {
            reader.say(page_id, None, Fault::WrongType { key: "Tabs" });
            Ok(None)
        }
    }
}

/// Common entries shared by markup annotations (ISO 32000-2 §12.5.6.2 Table
/// 172).
///
/// Its `Debug` implementation reports string lengths rather than author names,
/// subjects, or rich text.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct MarkupInfo {
    /// `/T`: title / author label displayed in the pop-up header.
    pub author: Option<String>,
    /// `/Subj`: short description of the subject being addressed.
    pub subject: Option<String>,
    /// `/CA`: constant stroking/overall opacity (`1.0` by default).
    pub opacity: f32,
    /// `/ca`: nonstroking opacity (PDF 2.0; falls back to `opacity` when
    /// absent).
    pub fill_opacity: Option<f32>,
    /// `/RC`: rich text string, if present (streams are omitted here and can be
    /// inspected via [`Annot::dict`]).
    pub rich_text: Option<String>,
    /// `/CreationDate`: when the annotation was created.
    pub created: Option<Dated>,
    /// `/Popup`: indirect `ObjectId` of the associated `/Popup` annotation.
    pub popup: Option<ObjectId>,
    /// `/IRT`: indirect `ObjectId` of the annotation this one is in reply to.
    pub in_reply_to: Option<ObjectId>,
    /// `/RT`: relationship to `in_reply_to` (`Reply` vs. `Group`).
    pub reply_kind: ReplyKind,
}

impl fmt::Debug for MarkupInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MarkupInfo")
            .field("author_len", &self.author.as_ref().map(String::len))
            .field("subject_len", &self.subject.as_ref().map(String::len))
            .field("opacity", &self.opacity)
            .field("fill_opacity", &self.fill_opacity)
            .field("rich_text_len", &self.rich_text.as_ref().map(String::len))
            .field("created", &self.created)
            .field("popup", &self.popup)
            .field("in_reply_to", &self.in_reply_to)
            .field("reply_kind", &self.reply_kind)
            .finish()
    }
}

/// Decoded PDF annotation backed by a borrowed `&'o Dict` (standalone
/// annotation object, direct dictionary, or merged Field+Widget dictionary).
///
/// Its `Debug` implementation reports counts, geometry, flags, and byte/char
/// lengths without printing `/Contents`, `/NM`, or author text.
#[derive(Clone, PartialEq)]
pub struct Annot<'o> {
    dict: &'o Dict,
    id: Option<ObjectId>,
    index: Option<usize>,
    subtype: AnnotSubtype,
    raw_rect: [f32; 4],
    rect: Rect,
    has_valid_rect: bool,
    contents: Option<String>,
    name: Option<String>,
    modified: Option<Dated>,
    flags: AnnotFlags,
    color: Option<AnnotColor>,
    border: Border,
    legacy_border: Option<[f32; 3]>,
    appearances: Option<Appearances>,
    struct_parent: Option<i32>,
    oc: Option<ObjectId>,
    additional_actions: Option<AdditionalActions>,
    markup: Option<MarkupInfo>,
    typed: TypedAnnot,
}

impl fmt::Debug for Annot<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Annot")
            .field("id", &self.id)
            .field("index", &self.index)
            .field("subtype", &self.subtype)
            .field("rect", &self.rect)
            .field("has_valid_rect", &self.has_valid_rect)
            .field("contents_len", &self.contents.as_ref().map(String::len))
            .field("name_len", &self.name.as_ref().map(String::len))
            .field("modified", &self.modified)
            .field("flags", &self.flags)
            .field("color", &self.color)
            .field("border", &self.border)
            .field("appearances", &self.appearances)
            .field("struct_parent", &self.struct_parent)
            .field("oc", &self.oc)
            .field("markup", &self.markup)
            .field("typed", &self.typed)
            .finish()
    }
}

impl<'o> Annot<'o> {
    /// Parses a single annotation dictionary `dict` (including standalone
    /// annotations and merged Field+Widget dictionaries) without collecting
    /// non-fatal [`Fault`]s.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unreadable`] on locked/I/O/reader-cap failures or
    /// [`Error::Limit`] if a coordinate/action cap is exceeded.
    pub fn from_dict<R: Resolve<'o>>(
        objects: R,
        dict: &'o Dict,
        id: Option<ObjectId>,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Self, Error> {
        Self::from_dict_with(objects, dict, id, rules, limits, &mut ())
    }

    /// Parses a single annotation dictionary `dict`, reporting recoverable ISO
    /// 32000-2 departures to `sink`.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::from_dict`].
    pub fn from_dict_with<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        objects: R,
        dict: &'o Dict,
        id: Option<ObjectId>,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Self, Error> {
        let mut reader = Reader {
            objects,
            rules,
            limits,
            sink,
        };
        read_one(&mut reader, dict, id, None)
    }

    /// Underlying annotation [`Dict`].
    #[must_use]
    pub const fn dict(&self) -> &'o Dict {
        self.dict
    }

    /// Indirect `ObjectId` of this annotation, or `None` if direct.
    #[must_use]
    pub const fn id(&self) -> Option<ObjectId> {
        self.id
    }

    /// Zero-based index in the enclosing page's `/Annots` array, when read via
    /// [`page_annots`] / [`page_annots_with`].
    #[must_use]
    pub const fn index(&self) -> Option<usize> {
        self.index
    }

    /// Annotation `/Subtype`.
    #[must_use]
    pub const fn subtype(&self) -> &AnnotSubtype {
        &self.subtype
    }

    /// Normalized annotation bounding rectangle in PageSpace (`Rect::ZERO` when
    /// `/Rect` was missing or invalid).
    #[must_use]
    pub const fn rect(&self) -> Rect {
        self.rect
    }

    /// Raw 4-element `/Rect` `[a, b, c, d]` in written order (`[0.0; 4]` when
    /// missing or invalid), matching PDFium's `FPDFAnnot_GetRect`.
    #[must_use]
    pub const fn raw_rect(&self) -> [f32; 4] {
        self.raw_rect
    }

    /// Returns `true` if `/Rect` was present and parsed into four numbers.
    #[must_use]
    pub const fn has_valid_rect(&self) -> bool {
        self.has_valid_rect
    }

    /// `/Contents`: text description or comment body, if present.
    #[must_use]
    pub fn contents(&self) -> Option<&str> {
        self.contents.as_deref()
    }

    /// `/NM`: unique annotation name on the page, if present.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// `/M`: last-modified date or raw text string, if present.
    #[must_use]
    pub const fn modified(&self) -> Option<&Dated> {
        self.modified.as_ref()
    }

    /// `/F`: annotation flags bitmask.
    #[must_use]
    pub const fn flags(&self) -> AnnotFlags {
        self.flags
    }

    /// Returns `true` if this annotation is visible on screen according to its
    /// [`AnnotFlags`] and subtype.
    #[must_use]
    pub const fn is_visible_on_screen(&self) -> bool {
        self.flags.displays_on_screen(self.subtype.is_standard())
    }

    /// Returns `true` if this annotation should print according to its
    /// [`AnnotFlags`] and subtype.
    #[must_use]
    pub const fn is_printable(&self) -> bool {
        self.flags.prints(self.subtype.is_standard())
    }

    /// `/C`: annotation color (`None` when absent).
    #[must_use]
    pub const fn color(&self) -> Option<AnnotColor> {
        self.color
    }

    /// Effective [`Border`] resolved from `/BS` or `/Border`.
    #[must_use]
    pub const fn border(&self) -> &Border {
        &self.border
    }

    /// Raw 3-number `[horizontal_radius, vertical_radius, border_width]` from a
    /// `/Border` array when `/Border` has at least 3 elements (matching
    /// PDFium's `FPDFAnnot_GetBorder`).
    #[must_use]
    pub const fn legacy_border(&self) -> Option<[f32; 3]> {
        self.legacy_border
    }

    /// `/AP` appearance streams and `/AS` active state, if present.
    #[must_use]
    pub const fn appearances(&self) -> Option<&Appearances> {
        self.appearances.as_ref()
    }

    /// `/StructParent`: key in the structure tree's `/ParentTree`, if present.
    #[must_use]
    pub const fn struct_parent(&self) -> Option<i32> {
        self.struct_parent
    }

    /// `/OC`: indirect optional-content group or membership dictionary ID, if
    /// present.
    #[must_use]
    pub const fn oc(&self) -> Option<ObjectId> {
        self.oc
    }

    /// `/AA`: additional actions triggered by mouse/focus/page events, if
    /// present.
    #[must_use]
    pub const fn additional_actions(&self) -> Option<&AdditionalActions> {
        self.additional_actions.as_ref()
    }

    /// Common markup metadata (`/T`, `/CA`, `/IRT`, `/Popup`, etc.) when
    /// `self.subtype().is_markup()`.
    #[must_use]
    pub const fn markup(&self) -> Option<&MarkupInfo> {
        self.markup.as_ref()
    }

    /// Subtype-specific decoded view.
    #[must_use]
    pub const fn typed(&self) -> &TypedAnnot {
        &self.typed
    }
}

/// Reads every dictionary annotation in `page_dict`'s `/Annots` array without
/// collecting non-fatal [`Fault`]s.
///
/// # Errors
///
/// Returns [`Error::Unreadable`] on locked/I/O/reader-cap failures or
/// [`Error::Limit`] if [`Limits::annots_per_page`] or another cap is exceeded.
pub fn page_annots<'o, R: Resolve<'o>>(
    objects: R,
    page_dict: &'o Dict,
    page_id: Option<ObjectId>,
    rules: Rules,
    limits: &Limits,
) -> Result<Vec<Annot<'o>>, Error> {
    page_annots_with(objects, page_dict, page_id, rules, limits, &mut ())
}

/// Reads every dictionary annotation in `page_dict`'s `/Annots` array,
/// reporting recoverable ISO 32000-2 departures to `sink`.
///
/// Entries in `/Annots` that resolve to `null` (dangling references) or
/// non-dictionaries emit a [`Fault`] and are skipped, while `Annot::index()`
/// preserves each annotation's original 0-based slot in `/Annots` (matching
/// `FPDFPage_GetAnnot(page, index)`).
///
/// # Errors
///
/// Same error conditions as [`page_annots`].
pub fn page_annots_with<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    page_dict: &'o Dict,
    page_id: Option<ObjectId>,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Vec<Annot<'o>>, Error> {
    let mut reader = Reader {
        objects,
        rules,
        limits,
        sink,
    };
    let Some(raw_annots) = page_dict.get(b"Annots") else {
        return Ok(Vec::new());
    };
    let Some((annots_obj, annots_id)) = reader.reached(raw_annots, page_id)? else {
        return Ok(Vec::new());
    };
    let Some(items) = annots_obj.as_array() else {
        reader.say(annots_id, annots_obj.at(), Fault::AnnotsNotArray);
        return Ok(Vec::new());
    };
    phase!(DEBUG, "page_annots", count = items.len());
    if items.len() > limits.annots_per_page {
        return Err(Error::Limit(Cap::AnnotsPerPage));
    }

    let mut seen_ids = HashSet::new();
    let mut out = Vec::with_capacity(items.len().min(64));
    for (index, item) in items.iter().enumerate() {
        let direct_ref = item.as_reference();
        let Some((entry_obj, entry_id)) = reader.reached(item, annots_id)? else {
            if let Some(dangling) = direct_ref {
                reader.say(annots_id, item.at(), Fault::DanglingAnnot(dangling));
            } else {
                reader.say(annots_id, item.at(), Fault::AnnotNotDict);
            }
            continue;
        };
        let Value::Dict(dict) = entry_obj.value() else {
            reader.say(entry_id, entry_obj.at(), Fault::AnnotNotDict);
            continue;
        };
        let own_id = if direct_ref.is_some() { entry_id } else { None };
        if let Some(oid) = own_id {
            if !seen_ids.insert(oid) {
                reader.say(annots_id, item.at(), Fault::DuplicateAnnot(oid));
                if rules == Rules::Iso {
                    continue;
                }
            }
        } else {
            reader.say(annots_id, item.at(), Fault::DirectAnnot);
        }
        out.push(read_one(&mut reader, dict, own_id, Some(index))?);
    }
    Ok(out)
}

fn read_one<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    id: Option<ObjectId>,
    index: Option<usize>,
) -> Result<Annot<'o>, Error> {
    let subtype = match reader.name_entry(dict, id, "Subtype")? {
        Some(name) => AnnotSubtype::from_name(name),
        None => {
            reader.say(id, None, Fault::MissingSubtype);
            AnnotSubtype::Other(Box::default())
        }
    };

    let (raw_rect, rect, has_valid_rect) = match reader.rect4(dict, id, "Rect")? {
        Some((raw, norm)) => (raw, norm, true),
        None => {
            if dict.get(b"Rect").is_none() {
                reader.say(id, None, Fault::InvalidRect);
            }
            ([0.0; 4], Rect::new(0.0, 0.0, 0.0, 0.0), false)
        }
    };

    let contents = reader.text_or_name_entry(dict, id, "Contents")?;
    let name = reader.text_or_name_entry(dict, id, "NM")?;
    let modified = match dict.get(b"M") {
        Some(_) => reader.dated_entry(dict, id, "M")?,
        None => None,
    };
    let flags = reader
        .int_entry(dict, id, "F")?
        .map_or(AnnotFlags::NONE, AnnotFlags::from_pdf_int);
    let color = read_color(reader, dict, id, "C")?;
    let legacy_border = read_legacy_border(reader, dict, id)?.map(|lb| [lb.hr, lb.vr, lb.bw]);
    let border = read_border(reader, dict, id)?;
    let appearances = read_appearances(reader, dict, id)?;
    let struct_parent = reader
        .int_entry(dict, id, "StructParent")?
        .and_then(|n| i32::try_from(n).ok());
    let oc = match dict.get(b"OC") {
        Some(raw) => match reader.reached(raw, id)? {
            Some((obj, oc_id)) if obj.as_dict().is_some() => oc_id,
            Some((obj, oc_id)) => {
                reader.say(oc_id, obj.at(), Fault::WrongType { key: "OC" });
                None
            }
            None => None,
        },
        None => None,
    };
    let additional_actions = reader.additional_actions(dict, id)?;

    let markup = if subtype.is_markup() {
        let author = reader.text_or_name_entry(dict, id, "T")?;
        let subject = reader.text_or_name_entry(dict, id, "Subj")?;
        let opacity = reader
            .float_entry(dict, id, "CA")?
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let fill_opacity = reader
            .float_entry(dict, id, "ca")?
            .map(|v| v.clamp(0.0, 1.0));
        let rich_text = match dict.get(b"RC") {
            Some(rc) => match reader.reached(rc, id)? {
                None => None,
                Some((rc_obj, _)) if rc_obj.as_stream().is_some() => None,
                Some(_) => reader.text_entry(dict, id, "RC")?,
            },
            None => None,
        };
        let created = reader.dated_entry(dict, id, "CreationDate")?;
        let popup = dict.get(b"Popup").and_then(|o| o.as_reference());
        let in_reply_to = dict.get(b"IRT").and_then(|o| o.as_reference());
        let reply_kind = reader
            .name_entry(dict, id, "RT")?
            .map_or(ReplyKind::Reply, ReplyKind::from_name);
        Some(MarkupInfo {
            author,
            subject,
            opacity,
            fill_opacity,
            rich_text,
            created,
            popup,
            in_reply_to,
            reply_kind,
        })
    } else {
        None
    };

    let typed = read_typed(reader, &subtype, dict, id)?;

    Ok(Annot {
        dict,
        id,
        index,
        subtype,
        raw_rect,
        rect,
        has_valid_rect,
        contents,
        name,
        modified,
        flags,
        color,
        border,
        legacy_border,
        appearances,
        struct_parent,
        oc,
        additional_actions,
        markup,
        typed,
    })
}
