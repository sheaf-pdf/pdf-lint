//! Explicit and named destinations (ISO 32000-2, 12.3.2).
//!
//! A destination names a page in a document and how to fit or position it in a
//! viewer window (Table 149). It can be written directly as an explicit array
//! (`[page /Fit ...]`) or indirectly by name—either as a byte string looked up
//! in the catalog's `/Names/Dests` name tree (PDF 1.2+) or as a `/` name looked
//! up in the legacy `/Dests` dictionary (PDF 1.1).
//!
//! Under [`Rules::Iso`], string names resolve only in `/Names/Dests`, `/` names
//! resolve only in `/Dests`, and local destinations must reference their target
//! page by an indirect `ObjectId`. Under [`Rules::Pdfium`] (as of
//! chromium/8152), named destinations fall back across `/Names/Dests` and
//! `/Dests` regardless of whether they were written as a string or a name
//! ([`Fault::DestCrossResolved`]), and 0-based integer page indices in local
//! destinations resolve to that page slot ([`Fault::DestIntegerPage`]).

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Str, Unreadable, Value, Why};

use crate::fault::Fault;
use crate::info::Len;
use crate::limits::Limits;
use crate::pages::Pages;
use crate::phases::phase;
use crate::{HOPS, too_long, tree};

/// Target page of an explicit destination array (ISO 32000-2, 12.3.2.2, Table
/// 149).
///
/// Exhaustive on purpose: a destination array's first element is either an
/// indirect page object reference (local destinations) or a 0-based page index
/// (remote `/GoToR` destinations, and tolerated for local destinations under
/// `Rules::Pdfium`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageTarget {
    /// Indirect object ID of the target `/Page` dictionary.
    PageRef(ObjectId),
    /// Zero-based page index (`0..page_count`).
    PageIndex(usize),
}

/// View magnification and scroll fit mode of an explicit destination (ISO
/// 32000-2, 12.3.2.2, Table 149).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[non_exhaustive]
pub enum DestFit {
    /// `[page /XYZ left top zoom]`: position `(left, top)` at the upper-left
    /// corner with magnification `zoom` (`None` or `0` retains the current
    /// value).
    XYZ {
        /// Horizontal coordinate `left` (`None` when `null`).
        left: Option<f32>,
        /// Vertical coordinate `top` (`None` when `null`).
        top: Option<f32>,
        /// Magnification factor `zoom` (`None` when `null` or `0`).
        zoom: Option<f32>,
    },
    /// `[page /Fit]`: fit the entire page within the window.
    #[default]
    Fit,
    /// `[page /FitH top]`: fit the page width, positioning `top` at the top
    /// edge.
    FitH {
        /// Vertical coordinate `top` (`None` when `null`).
        top: Option<f32>,
    },
    /// `[page /FitV left]`: fit the page height, positioning `left` at the left
    /// edge.
    FitV {
        /// Horizontal coordinate `left` (`None` when `null`).
        left: Option<f32>,
    },
    /// `[page /FitR left bottom right top]`: fit the normalized rectangle
    /// `[left, bottom, right, top]`.
    FitR {
        /// Left coordinate (`min(x0, x1)`).
        left: f32,
        /// Bottom coordinate (`min(y0, y1)`).
        bottom: f32,
        /// Right coordinate (`max(x0, x1)`).
        right: f32,
        /// Top coordinate (`max(y0, y1)`).
        top: f32,
    },
    /// `[page /FitB]`: fit the page's bounding box within the window.
    FitB,
    /// `[page /FitBH top]`: fit the bounding-box width with `top` at the top
    /// edge.
    FitBH {
        /// Vertical coordinate `top` (`None` when `null`).
        top: Option<f32>,
    },
    /// `[page /FitBV left]`: fit the bounding-box height with `left` at the
    /// left edge.
    FitBV {
        /// Horizontal coordinate `left` (`None` when `null`).
        left: Option<f32>,
    },
    /// Destination array with no recognized fit keyword (tolerated under
    /// `Rules::Pdfium`, matching `PDFDEST_VIEW_UNKNOWN_MODE`).
    Unknown,
}

/// Parsed explicit destination `[page /Fit ...]` before resolving `PageTarget`
/// to a concrete 0-based page index.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ExplicitDest {
    /// Target page reference or 0-based page index.
    pub page: PageTarget,
    /// Magnification and scroll fit mode.
    pub fit: DestFit,
}

/// How a named destination was written (`(string)` vs. `/Name`).
///
/// Exhaustive on purpose: ISO 32000-2 §12.3.2.3 defines string names (PDF 1.2+)
/// and `/` names (PDF 1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DestNameKind {
    /// Byte string `(name)` (PDF 1.2+, `/Names/Dests`).
    ByteString,
    /// Name object `/name` (PDF 1.1 legacy `/Dests`).
    Name,
}

/// Destination specified by `/Dest`, `/OpenAction`, or an action's `/D` entry
/// (ISO 32000-2, 12.3.2).
///
/// Its `Debug` implementation reports byte lengths rather than destination
/// names.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum Dest {
    /// Direct destination array `[page /Fit ...]`.
    Explicit(ExplicitDest),
    /// Named destination to be looked up in Catalog `/Names/Dests` or `/Dests`.
    Named {
        /// Raw destination name bytes.
        name: Vec<u8>,
        /// Whether the destination was written as a string or a `/` name.
        kind: DestNameKind,
    },
}

impl fmt::Debug for Dest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Explicit(exp) => f.debug_tuple("Explicit").field(exp).finish(),
            Self::Named { name, kind } => f
                .debug_struct("Named")
                .field("name", &Len(name.len()))
                .field("kind", kind)
                .finish(),
        }
    }
}

impl Dest {
    /// Constructs an explicit `[page_ref /Fit]` destination.
    #[must_use]
    pub const fn fit(page: ObjectId) -> Self {
        Self::Explicit(ExplicitDest {
            page: PageTarget::PageRef(page),
            fit: DestFit::Fit,
        })
    }

    /// Constructs an explicit `[page_ref /XYZ left top zoom]` destination.
    #[must_use]
    pub const fn xyz(
        page: ObjectId,
        left: Option<f32>,
        top: Option<f32>,
        zoom: Option<f32>,
    ) -> Self {
        Self::Explicit(ExplicitDest {
            page: PageTarget::PageRef(page),
            fit: DestFit::XYZ { left, top, zoom },
        })
    }

    /// Serializes this destination as a PDF [`Object`].
    #[must_use]
    pub fn to_object(&self) -> Object {
        match self {
            Self::Named { name, kind } => match kind {
                DestNameKind::ByteString => Object::bytes(name.clone()),
                DestNameKind::Name => Object::name(name.as_slice()),
            },
            Self::Explicit(exp) => explicit_to_object(*exp),
        }
    }
}

fn explicit_to_object(exp: ExplicitDest) -> Object {
    let page_obj = match exp.page {
        PageTarget::PageRef(id) => Object::from(id),
        PageTarget::PageIndex(idx) => Object::from(idx as i64),
    };
    let opt_num = |v: Option<f32>| match v {
        Some(n) => Object::from(f64::from(n)),
        None => Object::null(),
    };
    let mut arr = vec![page_obj];
    match exp.fit {
        DestFit::XYZ { left, top, zoom } => {
            arr.push(Object::name("XYZ"));
            arr.push(opt_num(left));
            arr.push(opt_num(top));
            arr.push(opt_num(zoom));
        }
        DestFit::Fit | DestFit::Unknown => arr.push(Object::name("Fit")),
        DestFit::FitH { top } => {
            arr.push(Object::name("FitH"));
            arr.push(opt_num(top));
        }
        DestFit::FitV { left } => {
            arr.push(Object::name("FitV"));
            arr.push(opt_num(left));
        }
        DestFit::FitR {
            left,
            bottom,
            right,
            top,
        } => {
            arr.push(Object::name("FitR"));
            arr.push(Object::from(f64::from(left)));
            arr.push(Object::from(f64::from(bottom)));
            arr.push(Object::from(f64::from(right)));
            arr.push(Object::from(f64::from(top)));
        }
        DestFit::FitB => arr.push(Object::name("FitB")),
        DestFit::FitBH { top } => {
            arr.push(Object::name("FitBH"));
            arr.push(opt_num(top));
        }
        DestFit::FitBV { left } => {
            arr.push(Object::name("FitBV"));
            arr.push(opt_num(left));
        }
    }
    Object::from(arr)
}

/// Destination resolved to a concrete 0-based page index in a document.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ResolvedDest {
    /// Zero-based index into [`Pages`].
    pub page_index: usize,
    /// Indirect `ObjectId` of that page, if numbered.
    pub page_id: Option<ObjectId>,
    /// Magnification and scroll fit mode.
    pub fit: DestFit,
}

/// Why building or resolving a [`DestTable`] failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `/Names/Dests` exceeded [`Limits::tree_nodes`] or [`Limits::tree_depth`].
    #[error("the /Names/Dests tree exceeds the configured node or depth limit")]
    TooManyDests,
    /// An object on the way could not be read due to reader caps, a missing
    /// password, or an I/O failure.
    #[error("{0}")]
    Unreadable(#[from] Unreadable),
}

/// Pre-indexed destination lookup table for `O(1)` resolution of named and
/// page-reference destinations across an entire document.
///
/// Its `Debug` implementation reports entry counts rather than destination
/// names.
#[derive(Clone, Default, PartialEq)]
pub struct DestTable {
    page_by_id: HashMap<ObjectId, usize>,
    page_ids: Vec<Option<ObjectId>>,
    tree_dests: BTreeMap<Vec<u8>, ExplicitDest>,
    dict_dests: BTreeMap<Vec<u8>, ExplicitDest>,
}

impl fmt::Debug for DestTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DestTable")
            .field("pages", &self.page_ids.len())
            .field("tree_dests", &self.tree_dests.len())
            .field("dict_dests", &self.dict_dests.len())
            .finish()
    }
}

impl DestTable {
    /// Builds a [`DestTable`] from `catalog` (`/Names/Dests` and `/Dests`) and
    /// `pages`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unreadable`] on locked/I/O/reader-cap failures or
    /// [`Error::TooManyDests`] if `/Names/Dests` hits [`Limits::tree_nodes`].
    pub fn read<'o, R: Resolve<'o> + Copy, S: Sink<Fault> + ?Sized>(
        objects: R,
        catalog: Option<&'o Dict>,
        pages: &Pages<'o>,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Self, Error> {
        phase!(DEBUG, "dest_table", pages = pages.len());
        let mut page_by_id = HashMap::with_capacity(pages.len());
        let mut page_ids = Vec::with_capacity(pages.len());
        for (idx, p) in pages.iter().enumerate() {
            page_ids.push(p.id);
            if let Some(id) = p.id {
                page_by_id.entry(id).or_insert(idx);
            }
        }

        let mut tree_dests = BTreeMap::new();
        let mut dict_dests = BTreeMap::new();

        let Some(cat) = catalog else {
            return Ok(Self {
                page_by_id,
                page_ids,
                tree_dests,
                dict_dests,
            });
        };

        let mut walk_read = Walk {
            objects,
            rules,
            sink,
        };

        // 1. Catalog `/Names` -> `/Dests` name tree (PDF 1.2+).
        if let Some((names_dict, names_id)) = walk_read.dict_entry(cat, None, "Names")?
            && let Some(dests_root) = names_dict.get(b"Dests")
        {
            let mut pairs = Vec::new();
            let mut hit_cap = false;
            {
                let mut sink_proxy = TreeSink {
                    inner: walk_read.sink,
                    hit_cap: &mut hit_cap,
                };
                let mut walk = tree::names(objects, dests_root, limits, &mut sink_proxy);
                for item in &mut walk {
                    let (key, val) = item?;
                    pairs.push((key.to_vec(), val));
                }
                if !walk.whole() && pairs.len() >= limits.tree_nodes {
                    hit_cap = true;
                }
            }
            if hit_cap {
                return Err(Error::TooManyDests);
            }
            for (key, val) in pairs {
                if let Some(exp) = parse_dest_target(&mut walk_read, val, names_id, true)? {
                    tree_dests.insert(key, exp);
                }
            }
        }

        // 2. Legacy Catalog `/Dests` dictionary (PDF 1.1).
        if let Some((dests_dict, dests_id)) = walk_read.dict_entry(cat, None, "Dests")? {
            for entry in dests_dict.entries() {
                if let Some(exp) = parse_dest_target(&mut walk_read, entry.value(), dests_id, true)?
                {
                    dict_dests.insert(entry.key().to_vec(), exp);
                }
            }
        }

        Ok(Self {
            page_by_id,
            page_ids,
            tree_dests,
            dict_dests,
        })
    }

    /// Number of named destinations indexed across `/Names/Dests` and `/Dests`.
    #[must_use]
    pub fn named_len(&self) -> usize {
        self.tree_dests.len() + self.dict_dests.len()
    }

    /// Resolves `dest` to a [`ResolvedDest`] under `rules`, reporting any ISO
    /// departures to `sink`.
    #[must_use]
    pub fn resolve<S: Sink<Fault> + ?Sized>(
        &self,
        dest: &Dest,
        rules: Rules,
        sink: &mut S,
    ) -> Option<ResolvedDest> {
        let pdfium = matches!(rules, Rules::Pdfium);
        let explicit = match dest {
            Dest::Explicit(exp) => *exp,
            Dest::Named { name, kind } => match (pdfium, kind) {
                (false, DestNameKind::ByteString) => *self.tree_dests.get(name)?,
                (false, DestNameKind::Name) => *self.dict_dests.get(name)?,
                (true, DestNameKind::ByteString) => {
                    if let Some(exp) = self.tree_dests.get(name) {
                        *exp
                    } else {
                        let exp = self.dict_dests.get(name)?;
                        if sink.wants() {
                            sink.fault(Place::new(None, None), Fault::DestCrossResolved);
                        }
                        *exp
                    }
                }
                (true, DestNameKind::Name) => {
                    if let Some(exp) = self.tree_dests.get(name) {
                        if sink.wants() && !self.dict_dests.contains_key(name) {
                            sink.fault(Place::new(None, None), Fault::DestCrossResolved);
                        }
                        *exp
                    } else {
                        *self.dict_dests.get(name)?
                    }
                }
            },
        };

        let page_index = match explicit.page {
            PageTarget::PageRef(id) => *self.page_by_id.get(&id)?,
            PageTarget::PageIndex(idx) => {
                if sink.wants() {
                    sink.fault(Place::new(None, None), Fault::DestIntegerPage);
                }
                if rules == Rules::Iso || idx >= self.page_ids.len() {
                    return None;
                }
                idx
            }
        };

        Some(ResolvedDest {
            page_index,
            page_id: self.page_ids.get(page_index).copied().flatten(),
            fit: explicit.fit,
        })
    }
}

struct TreeSink<'a, S: ?Sized> {
    inner: &'a mut S,
    hit_cap: &'a mut bool,
}

impl<S: Sink<Fault> + ?Sized> Sink<Fault> for TreeSink<'_, S> {
    fn wants(&self) -> bool {
        true
    }

    fn fault(&mut self, place: Place, fault: Fault) {
        if matches!(fault, Fault::TreeTooDeep | Fault::TreeTooManyNodes) {
            *self.hit_cap = true;
        }
        if self.inner.wants() {
            self.inner.fault(place, fault);
        }
    }
}

/// Convenience wrapper resolving a single [`Dest`] without holding a
/// [`DestTable`].
///
/// # Errors
///
/// Same error conditions as [`DestTable::read`].
pub fn resolve_dest<'o, R: Resolve<'o> + Copy, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: Option<&'o Dict>,
    pages: &Pages<'o>,
    dest: &Dest,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Option<ResolvedDest>, Error> {
    let table = DestTable::read(objects, catalog, pages, rules, limits, sink)?;
    Ok(table.resolve(dest, rules, sink))
}

/// Parses a `/Dest` or `/D` object value (`local = true` for in-document
/// destinations such as `/Link` `/Dest`, `/GoTo` `/D`, or `/OpenAction`;
/// `local = false` for `/GoToR` and `/GoToE` remote destinations where integer
/// page numbers are standard).
///
/// # Errors
///
/// Returns [`Unreadable`] if an indirect reference or encrypted string cannot
/// be read.
pub fn read<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    value: &'o Object,
    within: Option<ObjectId>,
    local: bool,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<Dest>, Unreadable> {
    let mut w = Walk {
        objects,
        rules,
        sink,
    };
    parse_dest_value(&mut w, value, within, local)
}

pub(crate) struct Walk<'s, R, S: ?Sized> {
    pub(crate) objects: R,
    pub(crate) rules: Rules,
    pub(crate) sink: &'s mut S,
}

impl<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized> Walk<'_, R, S> {
    pub(crate) fn reached(
        &mut self,
        value: &'o Object,
        within: Option<ObjectId>,
    ) -> Result<Option<(&'o Object, Option<ObjectId>)>, Unreadable> {
        let (mut at, mut last) = (value, None);
        for _ in 0..HOPS {
            match at.value() {
                Value::Ref(id) => {
                    last = Some(*id);
                    match self.objects.try_resolve(*id) {
                        Ok(Some(object)) => at = object,
                        Ok(None) => return Ok(None),
                        Err(unreadable) if unreadable.why == Why::Damaged => {
                            self.say(within, value.at(), Fault::Damaged { id: *id });
                            return Ok(None);
                        }
                        Err(unreadable) => return Err(unreadable),
                    }
                }
                Value::Null => return Ok(None),
                _ => return Ok(Some((at, last.or(within)))),
            }
        }
        Err(too_long(last.unwrap_or(ObjectId::new(0, 0))))
    }

    pub(crate) fn dict_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<(&'o Dict, Option<ObjectId>)>, Unreadable> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.as_dict() {
            Some(d) => Ok(Some((d, id))),
            None => {
                self.say(id, obj.at(), Fault::ActionWrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn say(&mut self, object: Option<ObjectId>, span: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            self.sink.fault(Place::new(object, span), fault);
        }
    }
}

pub(crate) fn locked_string(within: Option<ObjectId>, key: &'static str) -> Unreadable {
    let id = within.unwrap_or(ObjectId::new(0, 0));
    Unreadable::new(
        id,
        Why::Locked,
        format!("{id} R holds /{key} as ciphertext: it takes a password"),
    )
}

pub(crate) fn parse_dest_value<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    raw: &'o Object,
    within: Option<ObjectId>,
    local: bool,
) -> Result<Option<Dest>, Unreadable> {
    let Some((obj, id)) = w.reached(raw, within)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::String(Str::Bytes(bytes), _) => Ok(Some(Dest::Named {
            name: bytes.to_vec(),
            kind: DestNameKind::ByteString,
        })),
        Value::String(Str::Encrypted(_), _) => Err(locked_string(id, "Dest")),
        Value::Name(bytes) => Ok(Some(Dest::Named {
            name: bytes.to_vec(),
            kind: DestNameKind::Name,
        })),
        Value::Array(arr) => {
            Ok(parse_explicit_array(w, arr, id, obj.at(), local)?.map(Dest::Explicit))
        }
        Value::Dict(d) if matches!(w.rules, Rules::Pdfium) => {
            w.say(id, obj.at(), Fault::DestInvalid);
            if let Some(d_val) = d.get(b"D") {
                Ok(parse_dest_target(w, d_val, id, local)?.map(Dest::Explicit))
            } else {
                Ok(None)
            }
        }
        _ => {
            w.say(id, obj.at(), Fault::DestInvalid);
            Ok(None)
        }
    }
}

fn parse_dest_target<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    raw: &'o Object,
    within: Option<ObjectId>,
    local: bool,
) -> Result<Option<ExplicitDest>, Unreadable> {
    let Some((obj, id)) = w.reached(raw, within)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::Array(arr) => parse_explicit_array(w, arr, id, obj.at(), local),
        Value::Dict(d) => match d.get(b"D") {
            Some(inner) => {
                let Some((inner_obj, inner_id)) = w.reached(inner, id)? else {
                    return Ok(None);
                };
                match inner_obj.as_array() {
                    Some(arr) => parse_explicit_array(w, arr, inner_id, inner_obj.at(), local),
                    None => {
                        w.say(inner_id, inner_obj.at(), Fault::DestInvalid);
                        Ok(None)
                    }
                }
            }
            None => {
                w.say(id, obj.at(), Fault::DestInvalid);
                Ok(None)
            }
        },
        _ => {
            w.say(id, obj.at(), Fault::DestInvalid);
            Ok(None)
        }
    }
}

pub(crate) fn parse_explicit_array<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    arr: &'o [Object],
    within: Option<ObjectId>,
    span: Option<Span>,
    local: bool,
) -> Result<Option<ExplicitDest>, Unreadable> {
    let Some(first) = arr.first() else {
        w.say(within, span, Fault::DestInvalid);
        return Ok(None);
    };
    let page = match first.value() {
        Value::Ref(page_id) => PageTarget::PageRef(*page_id),
        _ => {
            let Some((p_obj, p_id)) = w.reached(first, within)? else {
                w.say(within, first.at(), Fault::DestInvalid);
                return Ok(None);
            };
            if let Some(n) = p_obj.as_integer() {
                let Ok(idx) = usize::try_from(n) else {
                    w.say(p_id, p_obj.at(), Fault::DestInvalid);
                    return Ok(None);
                };
                if local && w.rules == Rules::Iso {
                    w.say(p_id, p_obj.at(), Fault::DestIntegerPage);
                    return Ok(None);
                }
                PageTarget::PageIndex(idx)
            } else if p_obj.as_dict().is_some()
                && let Some(pid) = p_id
            {
                PageTarget::PageRef(pid)
            } else {
                w.say(p_id, p_obj.at(), Fault::DestInvalid);
                return Ok(None);
            }
        }
    };

    let opt_param = |w: &mut Walk<'_, R, S>, idx: usize| -> Result<Option<f32>, Unreadable> {
        let Some(raw) = arr.get(idx) else {
            return Ok(None);
        };
        let Some((o, oid)) = w.reached(raw, within)? else {
            return Ok(None);
        };
        match o.as_number() {
            Some(v) if (v as f32).is_finite() => Ok(Some(v as f32)),
            _ => {
                w.say(oid, o.at(), Fault::DestInvalid);
                Ok(None)
            }
        }
    };

    let Some(mode_raw) = arr.get(1) else {
        w.say(within, span, Fault::DestInvalid);
        if matches!(w.rules, Rules::Pdfium) {
            return Ok(Some(ExplicitDest {
                page,
                fit: DestFit::Unknown,
            }));
        }
        return Ok(None);
    };
    let Some((mode_obj, mode_id)) = w.reached(mode_raw, within)? else {
        w.say(within, mode_raw.at(), Fault::DestInvalid);
        return Ok(None);
    };
    let Some(mode_name) = mode_obj.as_name() else {
        w.say(mode_id, mode_obj.at(), Fault::DestInvalid);
        return Ok(None);
    };

    let fit = match mode_name {
        b"XYZ" => DestFit::XYZ {
            left: opt_param(w, 2)?,
            top: opt_param(w, 3)?,
            // Per ISO 32000-2 §12.3.2.2 Table 149 and PDFium's
            // `FPDFDest_GetLocationInPage`, a `zoom` of `0` has the same
            // meaning as `null` (retain current magnification).
            zoom: opt_param(w, 4)?.filter(|&z| z != 0.0),
        },
        b"Fit" => DestFit::Fit,
        b"FitH" => DestFit::FitH {
            top: opt_param(w, 2)?,
        },
        b"FitV" => DestFit::FitV {
            left: opt_param(w, 2)?,
        },
        b"FitR" => {
            let l = opt_param(w, 2)?.unwrap_or(0.0);
            let b = opt_param(w, 3)?.unwrap_or(0.0);
            let r = opt_param(w, 4)?.unwrap_or(0.0);
            let t = opt_param(w, 5)?.unwrap_or(0.0);
            if arr.len() < 6 {
                w.say(within, span, Fault::DestInvalid);
                if w.rules == Rules::Iso {
                    return Ok(None);
                }
            }
            DestFit::FitR {
                left: l.min(r),
                bottom: b.min(t),
                right: l.max(r),
                top: b.max(t),
            }
        }
        b"FitB" => DestFit::FitB,
        b"FitBH" => DestFit::FitBH {
            top: opt_param(w, 2)?,
        },
        b"FitBV" => DestFit::FitBV {
            left: opt_param(w, 2)?,
        },
        _ => {
            w.say(mode_id, mode_obj.at(), Fault::DestInvalid);
            if matches!(w.rules, Rules::Pdfium) {
                DestFit::Unknown
            } else {
                return Ok(None);
            }
        }
    };

    Ok(Some(ExplicitDest { page, fit }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId, dict};

    use super::{
        Dest, DestFit, DestNameKind, DestTable, Error, ExplicitDest, PageTarget, read, resolve_dest,
    };
    use crate::pages::pages;
    use crate::{Fault, Limits};

    #[test]
    fn dest_types_are_send_sync_and_debug_hygienic() {
        fn check<T: Send + Sync>() {}
        check::<PageTarget>();
        check::<DestFit>();
        check::<ExplicitDest>();
        check::<DestNameKind>();
        check::<Dest>();
        check::<DestTable>();
        check::<Error>();

        let d = Dest::Named {
            name: b"SECRET_DEST_TARGET".to_vec(),
            kind: DestNameKind::ByteString,
        };
        let dbg = format!("{d:?}");
        assert!(!dbg.contains("SECRET_DEST_TARGET"), "{dbg}");
    }

    #[test]
    fn cross_table_and_integer_local_dest_iso_vs_pdfium() {
        let p1 = ObjectId::new(10, 0);
        let p2 = ObjectId::new(11, 0);
        let pages_id = ObjectId::new(9, 0);
        let mut objects = BTreeMap::new();
        objects.insert(
            pages_id,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::from(p1), Object::from(p2)],
                "Count" => 2,
            }),
        );
        objects.insert(
            p1,
            Object::from(dict! { "Type" => Object::name("Page"), "Parent" => pages_id }),
        );
        objects.insert(
            p2,
            Object::from(dict! { "Type" => Object::name("Page"), "Parent" => pages_id }),
        );
        let catalog = dict! {
            "Pages" => pages_id,
            "Names" => dict! {
                "Dests" => dict! {
                    "Names" => vec![
                        Object::text("chap2"),
                        Object::from(dict! { "D" => vec![Object::from(p2), Object::name("XYZ"), Object::from(0), Object::from(700), Object::from(0)] }),
                    ],
                },
            },
        };
        let table = pages(&objects, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();

        let cross = Dest::Named {
            name: b"chap2".to_vec(),
            kind: DestNameKind::Name,
        };
        assert_eq!(
            resolve_dest(
                &objects,
                Some(&catalog),
                &table,
                &cross,
                Rules::Iso,
                &Limits::DEFAULT,
                &mut ()
            )
            .unwrap(),
            None
        );
        let mut faults = Vec::new();
        let resolved = resolve_dest(
            &objects,
            Some(&catalog),
            &table,
            &cross,
            Rules::Pdfium,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.page_index, 1);
        assert_eq!(
            resolved.fit,
            DestFit::XYZ {
                left: Some(0.0),
                top: Some(700.0),
                zoom: None,
            }
        );
        assert!(faults.iter().any(|(_, f)| *f == Fault::DestCrossResolved));

        let int_local = Object::from(vec![Object::from(1), Object::name("Fit")]);
        assert_eq!(
            read(&objects, &int_local, None, true, Rules::Iso, &mut ()).unwrap(),
            None
        );
        let parsed_pdfium = read(&objects, &int_local, None, true, Rules::Pdfium, &mut ())
            .unwrap()
            .unwrap();
        let res_int = resolve_dest(
            &objects,
            Some(&catalog),
            &table,
            &parsed_pdfium,
            Rules::Pdfium,
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap()
        .unwrap();
        assert_eq!(res_int.page_index, 1);
    }
}
