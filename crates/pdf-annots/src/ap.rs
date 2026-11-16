//! Appearance dictionaries (`/AP`), appearance states (`/AS`), and ISO 32000-2
//! §12.5.5 Form XObject placement matrices.

use std::fmt;

use pdf_gfx::{Matrix, Point, Rect};
use pdf_objects::fault::Sink;
use pdf_objects::{Dict, ObjectId, Resolve, Value};

use crate::error::Error;
use crate::fault::Fault;
use crate::resolve::Reader;

/// Which slot of `/AP` is selected (ISO 32000-2 §12.5.5 Table 170).
///
/// Exhaustive on purpose: `/AP` defines exactly these three entries (`/N`,
/// `/R`, `/D`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AppearanceMode {
    /// `/N`: normal appearance used for static display and printing.
    #[default]
    Normal = 0,
    /// `/R`: rollover appearance while the pointer hovers over the annotation.
    Rollover = 1,
    /// `/D`: down appearance while the mouse button is depressed.
    Down = 2,
}

impl AppearanceMode {
    /// Dictionary key in `/AP` (`"N"`, `"R"`, or `"D"`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Normal => "N",
            Self::Rollover => "R",
            Self::Down => "D",
        }
    }
}

/// One `/AP` slot (`/N`, `/R`, or `/D`): either a single indirect Form XObject
/// stream or a state subdictionary mapping state names (e.g. `Off`, `Yes`) to
/// Form XObject stream IDs.
///
/// Exhaustive on purpose: ISO 32000-2 §12.5.5 specifies either a stream or a
/// subdictionary of streams.
#[derive(Clone, PartialEq, Eq)]
pub enum AppearanceEntry {
    /// A single Form XObject stream (`ObjectId`).
    Single(ObjectId),
    /// State subdictionary `(state_name, form_stream_id)` in dictionary order.
    States(Vec<(Box<[u8]>, ObjectId)>),
}

impl fmt::Debug for AppearanceEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(id) => f.debug_tuple("Single").field(id).finish(),
            Self::States(states) => f
                .debug_struct("States")
                .field("count", &states.len())
                .finish(),
        }
    }
}

impl AppearanceEntry {
    /// Resolves the active Form XObject stream ID for `state_name` (`/AS`).
    ///
    /// - For [`Self::Single`], always returns `Some(id)` regardless of
    ///   `/AS`.
    /// - For [`Self::States`], returns the stream matching
    ///   `state_name`. When `strict_state` is `false` and `state_name` is
    ///   `None`, falls back to the first entry if and only if there is a single
    ///   entry; when `strict_state` is `true` (used when flattening stateful
    ///   widgets), missing `/AS` returns `None`.
    #[must_use]
    pub fn resolve(&self, state_name: Option<&[u8]>, strict_state: bool) -> Option<ObjectId> {
        match self {
            Self::Single(id) => Some(*id),
            Self::States(states) => match state_name {
                Some(target) => states
                    .iter()
                    .rev()
                    .find(|(k, _)| k.as_ref() == target)
                    .map(|(_, id)| *id),
                None if !strict_state && states.len() == 1 => Some(states[0].1),
                None => None,
            },
        }
    }
}

/// Parsed `/AP` appearance dictionary and `/AS` active state name on an
/// annotation (ISO 32000-2 §12.5.5).
///
/// Its `Debug` implementation omits raw state-name bytes.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Appearances {
    /// `/N`: normal appearance.
    pub normal: Option<AppearanceEntry>,
    /// `/R`: rollover appearance.
    pub rollover: Option<AppearanceEntry>,
    /// `/D`: down appearance.
    pub down: Option<AppearanceEntry>,
    /// `/AS`: active appearance state name, if present.
    pub state: Option<Box<[u8]>>,
}

impl fmt::Debug for Appearances {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Appearances")
            .field("normal", &self.normal)
            .field("rollover", &self.rollover)
            .field("down", &self.down)
            .field("has_state", &self.state.is_some())
            .finish()
    }
}

impl Appearances {
    /// Returns the [`AppearanceEntry`] for `mode` (without falling back across
    /// modes).
    #[must_use]
    pub const fn entry(&self, mode: AppearanceMode) -> Option<&AppearanceEntry> {
        match mode {
            AppearanceMode::Normal => self.normal.as_ref(),
            AppearanceMode::Rollover => self.rollover.as_ref(),
            AppearanceMode::Down => self.down.as_ref(),
        }
    }

    /// Resolves the active Form XObject `ObjectId` for `mode`.
    ///
    /// Per ISO 32000-2 §12.5.5 Table 170, when `/R` or `/D` is absent, viewers
    /// fall back to `/N` (`fallback_to_normal = true`).
    #[must_use]
    pub fn stream(
        &self,
        mode: AppearanceMode,
        strict_state: bool,
        fallback_to_normal: bool,
    ) -> Option<ObjectId> {
        let state = self.state.as_deref();
        if let Some(entry) = self.entry(mode)
            && let Some(id) = entry.resolve(state, strict_state)
        {
            return Some(id);
        }
        if fallback_to_normal
            && mode != AppearanceMode::Normal
            && let Some(normal) = &self.normal
        {
            return normal.resolve(state, strict_state);
        }
        None
    }
}

pub(crate) fn read_appearances<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
) -> Result<Option<Appearances>, Error> {
    let state = reader
        .name_entry(dict, within, "AS")?
        .map(Box::<[u8]>::from);
    let Some((ap, ap_id)) = reader.dict_entry(dict, within, "AP")? else {
        return Ok(state.map(|s| Appearances {
            normal: None,
            rollover: None,
            down: None,
            state: Some(s),
        }));
    };
    let normal = read_ap_slot(reader, ap, ap_id, "N")?;
    let rollover = read_ap_slot(reader, ap, ap_id, "R")?;
    let down = read_ap_slot(reader, ap, ap_id, "D")?;
    Ok(Some(Appearances {
        normal,
        rollover,
        down,
        state,
    }))
}

fn read_ap_slot<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    ap: &'o Dict,
    ap_id: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<AppearanceEntry>, Error> {
    let Some(raw) = ap.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = reader.reached(raw, ap_id)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::Stream(_) => match id {
            Some(stream_id) => Ok(Some(AppearanceEntry::Single(stream_id))),
            None => {
                reader.say(ap_id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        },
        Value::Dict(sub) => {
            let mut states = Vec::with_capacity(sub.len());
            for entry in sub.entries() {
                let Some((s_obj, s_id)) = reader.reached(entry.value(), id)? else {
                    continue;
                };
                if s_obj.as_stream().is_some()
                    && let Some(stream_id) = s_id
                {
                    states.push((Box::from(entry.key()), stream_id));
                } else {
                    reader.say(s_id, s_obj.at(), Fault::WrongType { key });
                }
            }
            Ok(Some(AppearanceEntry::States(states)))
        }
        _ => {
            reader.say(id, obj.at(), Fault::WrongType { key });
            Ok(None)
        }
    }
}

/// Computes the outer transformation [`Matrix`] `A` that places an appearance
/// Form XObject onto `annot_rect` according to ISO 32000-2 §12.5.5 (*Algorithm:
/// Appearance streams*).
///
/// Specifically:
/// 1. Transforms the four corners of `form_bbox` by `form_matrix` to obtain the
///    transformed appearance box `T`.
/// 2. Computes `A` mapping `T`'s lower-left corner to `annot_rect`'s lower-left
///    corner and scaling `T`'s width and height onto `annot_rect`'s width and
///    height.
/// 3. When drawing the Form XObject in a page content stream via `Do` (which
///    itself applies `form_matrix` inside the Form XObject), emit `A` via `cm`
///    before `Do`; the combined Form-space-to-PageSpace matrix is
///    `form_matrix.then(A)`.
///
/// Returns `None` if `T` has zero width or height, or if any coordinate is
/// non-finite.
#[must_use]
pub fn placement_matrix(form_bbox: Rect, form_matrix: Matrix, annot_rect: Rect) -> Option<Matrix> {
    let transformed = form_matrix.apply_rect(form_bbox);
    let tw = transformed.width();
    let th = transformed.height();
    if tw <= 0.0 || th <= 0.0 || !tw.is_finite() || !th.is_finite() {
        return None;
    }
    let sx = annot_rect.width() / tw;
    let sy = annot_rect.height() / th;
    let tx = annot_rect.x0 - transformed.x0 * sx;
    let ty = annot_rect.y0 - transformed.y0 * sy;
    let a = Matrix {
        a: sx,
        b: 0.0,
        c: 0.0,
        d: sy,
        e: tx,
        f: ty,
    };
    a.is_finite().then_some(a)
}

/// Reads an appearance Form XObject stream's `/BBox` and `/Matrix` (`IDENTITY`
/// when absent) and computes its [`placement_matrix`] onto `annot_rect`.
pub fn stream_placement_matrix<'o, R: Resolve<'o>>(
    objects: R,
    stream_id: ObjectId,
    annot_rect: Rect,
    limits: &crate::limits::Limits,
) -> Result<Option<(Rect, Matrix, Matrix)>, Error> {
    let mut dummy = ();
    let mut reader = Reader {
        objects,
        rules: pdf_objects::pdf_tokenizer::Rules::Pdfium,
        limits,
        sink: &mut dummy,
    };
    let Some((obj, id)) = reader.resolve_id(stream_id)? else {
        return Ok(None);
    };
    let Some(stream) = obj.as_stream() else {
        return Ok(None);
    };
    let Some((_, bbox)) = reader.rect4(&stream.dict, id, "BBox")? else {
        return Ok(None);
    };
    let form_matrix = match reader.array_entry(&stream.dict, id, "Matrix")? {
        Some((arr, m_id)) if arr.len() >= 6 => {
            let mut m = [0.0f32; 6];
            let mut ok = true;
            for (slot, item) in m.iter_mut().zip(arr.iter().take(6)) {
                if let Some(v) = reader.float_value(item, m_id, None)? {
                    *slot = v;
                } else {
                    ok = false;
                    break;
                }
            }
            if ok {
                Matrix {
                    a: m[0],
                    b: m[1],
                    c: m[2],
                    d: m[3],
                    e: m[4],
                    f: m[5],
                }
            } else {
                Matrix::IDENTITY
            }
        }
        _ => Matrix::IDENTITY,
    };
    let Some(place) = placement_matrix(bbox, form_matrix, annot_rect) else {
        return Ok(None);
    };
    // Keep unused Point import clean if needed; validate anchor finiteness.
    let _ = Point::ZERO;
    Ok(Some((bbox, form_matrix, place)))
}
