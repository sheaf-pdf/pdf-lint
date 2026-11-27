//! Optional content groups (`/OCProperties`, `/OCG`, `/OCMD`, ISO 32000-2
//! §8.11, Tables 96–100).
//!
//! [`read`] parses a catalog's `/OCProperties` dictionary into [`OcProperties`]
//! (groups, default configuration `/D`, and alternate `/Configs`), and
//! [`OcState::is_visible`] evaluates both `/Type /OCG` groups and `/Type /OCMD`
//! membership dictionaries (`/P` policies and `/VE` boolean visibility
//! expressions) under [`Limits::oc_depth`].

use std::collections::{HashMap, HashSet};
use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Str, Unreadable, Value, Why, text};

use crate::info::{Len, len};
use crate::phases::phase;
use crate::{Fault, HOPS, Limits, too_long};

/// Base visibility state of optional content groups in a configuration
/// (`/BaseState`, ISO 32000-2 §8.11.4.3 Table 99).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum BaseState {
    /// `/ON` (default): initialize all groups to visible before applying `/OFF`.
    #[default]
    On,
    /// `/OFF`: initialize all groups to hidden before applying `/ON`.
    Off,
    /// `/Unchanged`: leave existing states untouched before applying `/ON` and `/OFF`.
    Unchanged,
}

impl BaseState {
    /// Parses a `/BaseState` name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"ON" => Some(Self::On),
            b"OFF" => Some(Self::Off),
            b"Unchanged" => Some(Self::Unchanged),
            _ => None,
        }
    }
}

/// Membership visibility policy on an `/OCMD` dictionary (`/P`, ISO 32000-2
/// §8.11.2.2 Table 97).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum VisibilityPolicy {
    /// `/AllOn`: visible iff every listed `/OCG` is ON.
    AllOn,
    /// `/AnyOn` (default): visible iff at least one listed `/OCG` is ON.
    #[default]
    AnyOn,
    /// `/AnyOff`: visible iff at least one listed `/OCG` is OFF.
    AnyOff,
    /// `/AllOff`: visible iff every listed `/OCG` is OFF.
    AllOff,
}

impl VisibilityPolicy {
    /// Parses a `/P` name token.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"AllOn" => Some(Self::AllOn),
            b"AnyOn" => Some(Self::AnyOn),
            b"AnyOff" => Some(Self::AnyOff),
            b"AllOff" => Some(Self::AllOff),
            _ => None,
        }
    }
}

/// Boolean visibility expression (`/VE` on `/OCMD`, ISO 32000-2 §8.11.2.2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibilityExpr {
    /// Leaf reference to an `/OCG` dictionary.
    Ocg(ObjectId),
    /// `[/And expr1 expr2 ...]`.
    And(Vec<VisibilityExpr>),
    /// `[/Or expr1 expr2 ...]`.
    Or(Vec<VisibilityExpr>),
    /// `[/Not expr]`.
    Not(Box<VisibilityExpr>),
}

impl VisibilityExpr {
    /// Evaluates this expression against `state`.
    #[must_use]
    pub fn eval(&self, state: &OcState) -> bool {
        match self {
            Self::Ocg(id) => state.is_ocg_on(*id),
            Self::And(items) => items.iter().all(|e| e.eval(state)),
            Self::Or(items) => items.iter().any(|e| e.eval(state)),
            Self::Not(inner) => !inner.eval(state),
        }
    }
}

/// Usage event category (`/Event` on `/AS` dictionaries and `/Usage` keys,
/// ISO 32000-2 §8.11.4.4 Tables 100–101).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OcEvent {
    /// Interactive screen viewing (`/View` / `/ViewState`).
    View,
    /// Printing (`/Print` / `/PrintState`).
    Print,
    /// Exporting (`/Export` / `/ExportState`).
    Export,
}

impl OcEvent {
    /// Parses `/View`, `/Print`, or `/Export`.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"View" => Some(Self::View),
            b"Print" => Some(Self::Print),
            b"Export" => Some(Self::Export),
            _ => None,
        }
    }

    /// Canonical PDF name bytes for this event (`b"View"`, `b"Print"`, `b"Export"`).
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::View => b"View",
            Self::Print => b"Print",
            Self::Export => b"Export",
        }
    }
}

/// Optional content `/Usage` state overrides on an `/OCG` dictionary
/// (ISO 32000-2 §8.11.4.4 Table 100).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct OcUsage {
    /// `/Usage /View /ViewState` (`Some(true)` for `/ON`, `Some(false)` for `/OFF`).
    pub view: Option<bool>,
    /// `/Usage /Print /PrintState` (`Some(true)` for `/ON`, `Some(false)` for `/OFF`).
    pub print: Option<bool>,
    /// `/Usage /Export /ExportState` (`Some(true)` for `/ON`, `Some(false)` for `/OFF`).
    pub export: Option<bool>,
}

impl OcUsage {
    /// Returns the `/ON` (`true`) or `/OFF` (`false`) override for `event`, if present.
    #[must_use]
    pub const fn state_for(self, event: OcEvent) -> Option<bool> {
        match event {
            OcEvent::View => self.view,
            OcEvent::Print => self.print,
            OcEvent::Export => self.export,
        }
    }
}

/// Usage application dictionary from a configuration's `/AS` array
/// (ISO 32000-2 §8.11.4.4 Table 101).
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OcUsageApp {
    /// Triggering event (`/View`, `/Print`, or `/Export`).
    pub event: OcEvent,
    /// Optional content groups governed by this rule (`/OCGs`).
    pub ocgs: Vec<ObjectId>,
    /// Usage categories consulted (`/Category`, e.g. `View`, `Print`, `Export`).
    pub categories: Vec<Box<[u8]>>,
}

impl fmt::Debug for OcUsageApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OcUsageApp")
            .field("event", &self.event)
            .field("ocgs", &self.ocgs.len())
            .field("categories", &self.categories.len())
            .finish()
    }
}

/// One optional content group (`/Type /OCG`, ISO 32000-2 §8.11.2.1 Table 96).
///
/// Its `Debug` implementation reports `name`'s byte length and intent count.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ocg {
    /// Indirect object ID of the `/OCG` dictionary.
    pub id: ObjectId,
    /// Decoded group `/Name`.
    pub name: String,
    /// `/Intent` name(s) (`[b"View"]` when `/Intent` is absent, per Table 96).
    pub intents: Vec<Box<[u8]>>,
    /// `/Usage` state overrides (`/ViewState`, `/PrintState`, `/ExportState`).
    pub usage: OcUsage,
}

impl fmt::Debug for Ocg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ocg")
            .field("id", &self.id)
            .field("name", &Len(self.name.len()))
            .field("intents", &self.intents.len())
            .field("usage", &self.usage)
            .finish()
    }
}

/// Node in a configuration's `/Order` layer-panel presentation tree
/// (ISO 32000-2 §8.11.4.3 Table 99).
#[derive(Clone, PartialEq, Eq)]
pub enum OcOrderItem {
    /// Indirect reference to an `/OCG`.
    Group(ObjectId),
    /// Nested group array, optionally headed by a non-selectable text label.
    Section {
        /// Optional header label when the nested array's first element is a text string.
        label: Option<String>,
        /// Child order items.
        items: Vec<OcOrderItem>,
    },
}

impl fmt::Debug for OcOrderItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Group(id) => f.debug_tuple("Group").field(id).finish(),
            Self::Section { label, items } => f
                .debug_struct("Section")
                .field("label", &len(label))
                .field("items", &items.len())
                .finish(),
        }
    }
}

/// Optional content configuration dictionary (`/D` or an element of `/Configs`,
/// ISO 32000-2 §8.11.4.3 Table 99).
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OcConfig {
    /// Optional configuration `/Name`.
    pub name: Option<String>,
    /// Optional `/Creator`.
    pub creator: Option<String>,
    /// `/BaseState` (`BaseState::On` when absent).
    pub base_state: BaseState,
    /// `/ON` group IDs.
    pub on: Vec<ObjectId>,
    /// `/OFF` group IDs.
    pub off: Vec<ObjectId>,
    /// `/Locked` group IDs.
    pub locked: Vec<ObjectId>,
    /// `/Order` presentation hierarchy.
    pub order: Vec<OcOrderItem>,
    /// `/AS` usage application dictionaries (ISO 32000-2 §8.11.4.4 Table 101).
    pub usage_apps: Vec<OcUsageApp>,
}

impl fmt::Debug for OcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OcConfig")
            .field("name", &len(&self.name))
            .field("creator", &len(&self.creator))
            .field("base_state", &self.base_state)
            .field("on", &self.on.len())
            .field("off", &self.off.len())
            .field("locked", &self.locked.len())
            .field("order", &self.order.len())
            .field("usage_apps", &self.usage_apps.len())
            .finish()
    }
}

/// Decoded catalog `/OCProperties` dictionary (ISO 32000-2 §8.11.4.2 Table 98).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OcProperties {
    /// Registered optional content groups from `/OCGs`.
    pub groups: Vec<Ocg>,
    /// Default viewing configuration (`/D`).
    pub default_config: OcConfig,
    /// Alternate configurations (`/Configs`).
    pub configs: Vec<OcConfig>,
}

impl OcProperties {
    /// Builds the active [`OcState`] from [`Self::default_config`].
    #[must_use]
    pub fn default_state(&self) -> OcState {
        self.state_for(&self.default_config)
    }

    /// Builds the active [`OcState`] from [`Self::default_config`] for `event`
    /// (`View`, `Print`, or `Export`) under `rules`.
    ///
    /// - Under [`Rules::Iso`], `/Usage` state overrides are applied through matching
    ///   `/AS` usage application dictionaries on `default_config` (ISO 32000-2 §8.11.4.4).
    /// - Under [`Rules::Pdfium`], `/Usage` (`ViewState`, `PrintState`, `ExportState`)
    ///   is applied directly for each group that declares it (`CPDF_OCContext`).
    #[must_use]
    pub fn default_state_for_event(&self, event: OcEvent, rules: Rules) -> OcState {
        self.state_for_event(&self.default_config, event, rules)
    }

    /// Builds an [`OcState`] by applying `config` and `event` usage rules to [`Self::groups`].
    #[must_use]
    pub fn state_for_event(&self, config: &OcConfig, event: OcEvent, rules: Rules) -> OcState {
        let mut state = self.state_for(config);
        let usage_by_id: HashMap<ObjectId, OcUsage> =
            self.groups.iter().map(|g| (g.id, g.usage)).collect();
        match rules {
            Rules::Iso => {
                let cat_name = event.as_bytes();
                for app in &config.usage_apps {
                    if app.event == event && app.categories.iter().any(|c| c.as_ref() == cat_name) {
                        for &gid in &app.ocgs {
                            if let Some(on) = usage_by_id.get(&gid).and_then(|u| u.state_for(event))
                            {
                                state.set_ocg(gid, on);
                            }
                        }
                    }
                }
            }
            _ => {
                for g in &self.groups {
                    if let Some(on) = g.usage.state_for(event) {
                        state.set_ocg(g.id, on);
                    }
                }
            }
        }
        state
    }

    /// Builds an [`OcState`] by applying `config` to [`Self::groups`].
    #[must_use]
    pub fn state_for(&self, config: &OcConfig) -> OcState {
        let mut states = HashMap::with_capacity(self.groups.len());
        let initial = match config.base_state {
            BaseState::On | BaseState::Unchanged => true,
            BaseState::Off => false,
        };
        for g in &self.groups {
            states.insert(g.id, initial);
        }
        if config.base_state != BaseState::On {
            for &id in &config.on {
                states.insert(id, true);
            }
        }
        if config.base_state != BaseState::Off {
            for &id in &config.off {
                states.insert(id, false);
            }
        }
        OcState {
            states,
            max_depth: Limits::DEFAULT.oc_depth,
        }
    }
}

/// Resolved per-group ON/OFF state capable of evaluating `/OCG` and `/OCMD`
/// visibility on content marked-content (`BDC`), XObjects, and annotations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcState {
    states: HashMap<ObjectId, bool>,
    max_depth: usize,
}

impl OcState {
    /// Returns whether `/OCG` `id` is ON (unregistered groups default to `true`
    /// per ISO 32000-2 §8.11.4.2).
    #[must_use]
    pub fn is_ocg_on(&self, id: ObjectId) -> bool {
        self.states.get(&id).copied().unwrap_or(true)
    }

    /// Overrides the visibility of `/OCG` `id`.
    pub fn set_ocg(&mut self, id: ObjectId, visible: bool) {
        self.states.insert(id, visible);
    }

    /// Evaluates whether `oc` (an indirect or direct `/OCG` or `/OCMD` object,
    /// such as `/OC` on an XObject/annotation or a `/Properties` resource entry)
    /// is visible.
    ///
    /// Unknown or malformed dictionaries return `true` (fail-open visibility,
    /// matching ISO 32000-2 §8.11.2.2).
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] on reader caps, locked objects, or reference loops.
    pub fn is_visible<'o, R: Resolve<'o>>(
        &self,
        objects: R,
        oc: &'o Object,
    ) -> Result<bool, Unreadable> {
        let mut sink = ();
        let Some((obj, id)) = reached(&objects, oc, None, &mut sink)? else {
            return Ok(true);
        };
        let Some(dict) = obj.as_dict() else {
            return Ok(true);
        };
        match dict.get(b"Type").and_then(Object::as_name) {
            Some(b"OCMD") => self.eval_ocmd(&objects, dict, id),
            Some(b"OCG") => Ok(id.is_none_or(|oid| self.is_ocg_on(oid))),
            _ => {
                // Distinguish by keys when `/Type` is omitted.
                if dict.get(b"VE").is_some()
                    || dict.get(b"OCGs").is_some()
                    || dict.get(b"P").is_some()
                {
                    self.eval_ocmd(&objects, dict, id)
                } else {
                    Ok(id.is_none_or(|oid| self.is_ocg_on(oid)))
                }
            }
        }
    }

    fn eval_ocmd<'o, R: Resolve<'o>>(
        &self,
        objects: &R,
        dict: &'o Dict,
        within: Option<ObjectId>,
    ) -> Result<bool, Unreadable> {
        // Per ISO 32000-2 §8.11.2.2 Table 97, `/VE` takes precedence over `/OCGs` + `/P`.
        if let Some(raw_ve) = dict.get(b"VE")
            && let Some(expr) = parse_ve(objects, raw_ve, within, 0, self.max_depth, &mut ())?
        {
            return Ok(expr.eval(self));
        }

        let policy = dict
            .get(b"P")
            .and_then(Object::as_name)
            .and_then(VisibilityPolicy::from_name)
            .unwrap_or_default();

        let Some(raw_ocgs) = dict.get(b"OCGs") else {
            return Ok(true);
        };
        let mut group_ids = Vec::new();
        if let Some(id) = raw_ocgs.as_reference()
            && let Some((resolved, _)) = reached(objects, raw_ocgs, within, &mut ())?
        {
            if let Some(arr) = resolved.as_array() {
                for item in arr {
                    if let Some(gid) = valid_ocg_ref(objects, item, Some(id))? {
                        group_ids.push(gid);
                    }
                }
            } else if resolved.as_dict().is_some() {
                group_ids.push(id);
            }
        } else if let Some(arr) = raw_ocgs.as_array() {
            for item in arr {
                if let Some(gid) = valid_ocg_ref(objects, item, within)? {
                    group_ids.push(gid);
                }
            }
        }

        // Empty or all-null `/OCGs`: visible per §8.11.2.2.
        if group_ids.is_empty() {
            return Ok(true);
        }
        Ok(match policy {
            VisibilityPolicy::AllOn => group_ids.iter().all(|&g| self.is_ocg_on(g)),
            VisibilityPolicy::AnyOn => group_ids.iter().any(|&g| self.is_ocg_on(g)),
            VisibilityPolicy::AnyOff => group_ids.iter().any(|&g| !self.is_ocg_on(g)),
            VisibilityPolicy::AllOff => group_ids.iter().all(|&g| !self.is_ocg_on(g)),
        })
    }
}

fn valid_ocg_ref<'o, R: Resolve<'o>>(
    objects: &R,
    item: &'o Object,
    within: Option<ObjectId>,
) -> Result<Option<ObjectId>, Unreadable> {
    let Some(gid) = item.as_reference() else {
        return Ok(None);
    };
    let Some((obj, _)) = reached(objects, item, within, &mut ())? else {
        return Ok(None);
    };
    Ok(obj.as_dict().is_some().then_some(gid))
}

fn parse_ve<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    raw: &'o Object,
    within: Option<ObjectId>,
    depth: usize,
    max_depth: usize,
    sink: &mut S,
) -> Result<Option<VisibilityExpr>, Unreadable> {
    if depth >= max_depth {
        say(sink, within, raw.at(), Fault::OcTooDeep);
        return Ok(None);
    }
    let Some((obj, id)) = reached(objects, raw, within, sink)? else {
        return Ok(None);
    };
    if obj.as_dict().is_some() {
        return Ok(id.map(VisibilityExpr::Ocg));
    }
    let Some(arr) = obj.as_array() else {
        say(sink, id, obj.at(), Fault::OcWrongType { key: "VE" });
        return Ok(None);
    };
    let Some((op_obj, rest)) = arr.split_first() else {
        say(sink, id, obj.at(), Fault::OcWrongType { key: "VE" });
        return Ok(None);
    };
    let Some(op) = op_obj.as_name() else {
        say(sink, id, op_obj.at(), Fault::OcWrongType { key: "VE" });
        return Ok(None);
    };
    match op {
        b"Not" => {
            if rest.len() != 1 {
                say(sink, id, obj.at(), Fault::OcWrongType { key: "VE" });
                return Ok(None);
            }
            let Some(sub) = parse_ve(objects, &rest[0], id, depth + 1, max_depth, sink)? else {
                return Ok(None);
            };
            Ok(Some(VisibilityExpr::Not(Box::new(sub))))
        }
        b"And" | b"Or" => {
            if rest.is_empty() {
                say(sink, id, obj.at(), Fault::OcWrongType { key: "VE" });
                return Ok(None);
            }
            let mut subs = Vec::with_capacity(rest.len());
            for item in rest {
                let Some(sub) = parse_ve(objects, item, id, depth + 1, max_depth, sink)? else {
                    return Ok(None);
                };
                subs.push(sub);
            }
            Ok(Some(if op == b"And" {
                VisibilityExpr::And(subs)
            } else {
                VisibilityExpr::Or(subs)
            }))
        }
        _ => {
            say(sink, id, op_obj.at(), Fault::OcWrongType { key: "VE" });
            Ok(None)
        }
    }
}

/// Reads catalog `/OCProperties` (ISO 32000-2 §8.11.4.2).
///
/// Returns `Ok(None)` when `/OCProperties` is absent, `null`, a dangling
/// reference, or not a dictionary.
///
/// # Errors
///
/// Returns [`Unreadable`] on reader caps, locked objects/strings, or reference
/// loops.
pub fn read<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Option<OcProperties>, Unreadable> {
    let Some(raw_ocp) = catalog.get(b"OCProperties") else {
        return Ok(None);
    };
    let Some((ocp_obj, ocp_id)) = reached(&objects, raw_ocp, None, sink)? else {
        return Ok(None);
    };
    let Some(ocp_dict) = ocp_obj.as_dict() else {
        say(sink, ocp_id, ocp_obj.at(), Fault::OcInvalid);
        return Ok(None);
    };

    let mut groups = Vec::new();
    let mut seen_groups = HashSet::new();
    match ocp_dict.get(b"OCGs") {
        Some(raw_ocgs) => match reached(&objects, raw_ocgs, ocp_id, sink)? {
            Some((arr_obj, arr_id)) => match arr_obj.as_array() {
                Some(arr) => {
                    for item in arr {
                        let Some(gid) = item.as_reference() else {
                            if !item.is_null() {
                                say(sink, arr_id, item.at(), Fault::OcWrongType { key: "OCGs" });
                            }
                            continue;
                        };
                        if !seen_groups.insert(gid) {
                            continue;
                        }
                        let Some((g_obj, _)) = reached(&objects, item, arr_id, sink)? else {
                            continue;
                        };
                        let Some(g_dict) = g_obj.as_dict() else {
                            say(sink, Some(gid), g_obj.at(), Fault::OcInvalid);
                            continue;
                        };
                        groups.push(read_ocg(&objects, g_dict, gid, rules, sink)?);
                    }
                }
                None => say(
                    sink,
                    arr_id,
                    arr_obj.at(),
                    Fault::OcWrongType { key: "OCGs" },
                ),
            },
            None => say(sink, ocp_id, raw_ocgs.at(), Fault::OcInvalid),
        },
        None => say(sink, ocp_id, None, Fault::OcInvalid),
    }

    let default_config = match ocp_dict.get(b"D") {
        Some(raw_d) => match reached(&objects, raw_d, ocp_id, sink)? {
            Some((d_obj, d_id)) => match d_obj.as_dict() {
                Some(d_dict) => read_config(&objects, d_dict, d_id, rules, limits, sink)?,
                None => {
                    say(sink, d_id, d_obj.at(), Fault::OcWrongType { key: "D" });
                    OcConfig::default()
                }
            },
            None => {
                say(sink, ocp_id, raw_d.at(), Fault::OcInvalid);
                OcConfig::default()
            }
        },
        None => {
            say(sink, ocp_id, None, Fault::OcInvalid);
            OcConfig::default()
        }
    };

    let mut configs = Vec::new();
    if let Some(raw_cfgs) = ocp_dict.get(b"Configs")
        && let Some((cfgs_obj, cfgs_id)) = reached(&objects, raw_cfgs, ocp_id, sink)?
    {
        if let Some(arr) = cfgs_obj.as_array() {
            for item in arr {
                let Some((c_obj, c_id)) = reached(&objects, item, cfgs_id, sink)? else {
                    continue;
                };
                if let Some(c_dict) = c_obj.as_dict() {
                    configs.push(read_config(&objects, c_dict, c_id, rules, limits, sink)?);
                } else {
                    say(
                        sink,
                        c_id,
                        c_obj.at(),
                        Fault::OcWrongType { key: "Configs" },
                    );
                }
            }
        } else {
            say(
                sink,
                cfgs_id,
                cfgs_obj.at(),
                Fault::OcWrongType { key: "Configs" },
            );
        }
    }

    phase!(
        DEBUG,
        "ocg::read",
        groups = groups.len(),
        configs = configs.len()
    );
    Ok(Some(OcProperties {
        groups,
        default_config,
        configs,
    }))
}

fn read_ocg<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    id: ObjectId,
    rules: Rules,
    sink: &mut S,
) -> Result<Ocg, Unreadable> {
    let name = match text_entry(objects, dict, Some(id), "Name", rules, sink)? {
        Some(s) => s,
        None => {
            say(sink, Some(id), None, Fault::OcInvalid);
            String::new()
        }
    };
    let intents = match dict.get(b"Intent") {
        Some(raw) => match reached(objects, raw, Some(id), sink)? {
            Some((obj, iid)) => match obj.value() {
                Value::Name(n) => vec![Box::from(&n[..])],
                Value::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        if let Some((io, _)) = reached(objects, item, iid, sink)?
                            && let Some(n) = io.as_name()
                        {
                            out.push(Box::from(n));
                        } else {
                            say(sink, iid, item.at(), Fault::OcWrongType { key: "Intent" });
                        }
                    }
                    out
                }
                _ => {
                    say(sink, iid, obj.at(), Fault::OcWrongType { key: "Intent" });
                    vec![Box::from(&b"View"[..])]
                }
            },
            None => vec![Box::from(&b"View"[..])],
        },
        None => vec![Box::from(&b"View"[..])],
    };
    let usage = read_ocg_usage(objects, dict, Some(id), sink)?;
    Ok(Ocg {
        id,
        name,
        intents,
        usage,
    })
}

fn read_ocg_usage<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    sink: &mut S,
) -> Result<OcUsage, Unreadable> {
    let Some(raw) = dict.get(b"Usage") else {
        return Ok(OcUsage::default());
    };
    let Some((obj, uid)) = reached(objects, raw, within, sink)? else {
        return Ok(OcUsage::default());
    };
    let Some(u_dict) = obj.as_dict() else {
        say(sink, uid, obj.at(), Fault::OcWrongType { key: "Usage" });
        return Ok(OcUsage::default());
    };
    let mut state_key = |sub_key: &[u8], st_key: &[u8]| -> Result<Option<bool>, Unreadable> {
        let Some(sub_raw) = u_dict.get(sub_key) else {
            return Ok(None);
        };
        let Some((sub_obj, sid)) = reached(objects, sub_raw, uid, sink)? else {
            return Ok(None);
        };
        let Some(sub_dict) = sub_obj.as_dict() else {
            return Ok(None);
        };
        let Some(st_raw) = sub_dict.get(st_key) else {
            return Ok(None);
        };
        let Some((st_obj, _)) = reached(objects, st_raw, sid, sink)? else {
            return Ok(None);
        };
        Ok(match st_obj.as_name() {
            Some(b"ON") => Some(true),
            Some(b"OFF") => Some(false),
            _ => None,
        })
    };
    Ok(OcUsage {
        view: state_key(b"View", b"ViewState")?,
        print: state_key(b"Print", b"PrintState")?,
        export: state_key(b"Export", b"ExportState")?,
    })
}

fn read_config<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<OcConfig, Unreadable> {
    let name = text_entry(objects, dict, within, "Name", rules, sink)?;
    let creator = text_entry(objects, dict, within, "Creator", rules, sink)?;
    let base_state = match dict.get(b"BaseState") {
        Some(raw) => match reached(objects, raw, within, sink)? {
            Some((o, bid)) => match o.as_name().and_then(BaseState::from_name) {
                Some(bs) => bs,
                None => {
                    say(sink, bid, o.at(), Fault::OcWrongType { key: "BaseState" });
                    BaseState::On
                }
            },
            None => BaseState::On,
        },
        None => BaseState::On,
    };
    let on = ref_array(objects, dict, within, "ON", sink)?;
    let off = ref_array(objects, dict, within, "OFF", sink)?;
    let locked = ref_array(objects, dict, within, "Locked", sink)?;
    let order = match dict.get(b"Order") {
        Some(raw) => match reached(objects, raw, within, sink)? {
            Some((o, oid)) => match o.as_array() {
                Some(arr) => read_order_array(objects, arr, oid, 0, rules, limits, sink)?,
                None => {
                    say(sink, oid, o.at(), Fault::OcWrongType { key: "Order" });
                    Vec::new()
                }
            },
            None => Vec::new(),
        },
        None => Vec::new(),
    };
    let usage_apps = read_usage_apps(objects, dict, within, sink)?;

    Ok(OcConfig {
        name,
        creator,
        base_state,
        on,
        off,
        locked,
        order,
        usage_apps,
    })
}

fn read_usage_apps<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    sink: &mut S,
) -> Result<Vec<OcUsageApp>, Unreadable> {
    let Some(raw) = dict.get(b"AS") else {
        return Ok(Vec::new());
    };
    let Some((obj, aid)) = reached(objects, raw, within, sink)? else {
        return Ok(Vec::new());
    };
    let Some(arr) = obj.as_array() else {
        say(sink, aid, obj.at(), Fault::OcWrongType { key: "AS" });
        return Ok(Vec::new());
    };
    let mut apps = Vec::new();
    for item in arr {
        let Some((app_obj, app_id)) = reached(objects, item, aid, sink)? else {
            continue;
        };
        let Some(app_dict) = app_obj.as_dict() else {
            say(sink, app_id, app_obj.at(), Fault::OcWrongType { key: "AS" });
            continue;
        };
        let Some(ev_raw) = app_dict.get(b"Event") else {
            continue;
        };
        let Some((ev_obj, _)) = reached(objects, ev_raw, app_id, sink)? else {
            continue;
        };
        let Some(event) = ev_obj.as_name().and_then(OcEvent::from_name) else {
            continue;
        };
        let ocgs = ref_array(objects, app_dict, app_id, "OCGs", sink)?;
        let mut categories = Vec::new();
        if let Some(cat_raw) = app_dict.get(b"Category")
            && let Some((cat_obj, cid)) = reached(objects, cat_raw, app_id, sink)?
            && let Some(cat_arr) = cat_obj.as_array()
        {
            for c_item in cat_arr {
                if let Some((co, _)) = reached(objects, c_item, cid, sink)?
                    && let Some(n) = co.as_name()
                {
                    categories.push(Box::from(n));
                }
            }
        }
        apps.push(OcUsageApp {
            event,
            ocgs,
            categories,
        });
    }
    Ok(apps)
}

fn read_order_array<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    arr: &'o [Object],
    within: Option<ObjectId>,
    depth: usize,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Vec<OcOrderItem>, Unreadable> {
    if depth >= limits.oc_depth {
        say(sink, within, None, Fault::OcTooDeep);
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for item in arr {
        if let Some(gid) = item.as_reference() {
            let Some((resolved, _)) = reached(objects, item, within, sink)? else {
                continue;
            };
            if let Some(sub_arr) = resolved.as_array() {
                out.push(read_order_section(
                    objects,
                    sub_arr,
                    Some(gid),
                    depth + 1,
                    rules,
                    limits,
                    sink,
                )?);
            } else if resolved.as_dict().is_some() {
                out.push(OcOrderItem::Group(gid));
            } else {
                say(
                    sink,
                    Some(gid),
                    resolved.at(),
                    Fault::OcWrongType { key: "Order" },
                );
            }
        } else if let Some(sub_arr) = item.as_array() {
            out.push(read_order_section(
                objects,
                sub_arr,
                within,
                depth + 1,
                rules,
                limits,
                sink,
            )?);
        } else if !item.is_null() {
            say(sink, within, item.at(), Fault::OcWrongType { key: "Order" });
        }
    }
    Ok(out)
}

fn read_order_section<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    arr: &'o [Object],
    within: Option<ObjectId>,
    depth: usize,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<OcOrderItem, Unreadable> {
    if depth >= limits.oc_depth {
        say(sink, within, None, Fault::OcTooDeep);
        return Ok(OcOrderItem::Section {
            label: None,
            items: Vec::new(),
        });
    }
    if let Some((first, rest)) = arr.split_first() {
        match first.value() {
            Value::String(Str::Bytes(bytes), _) => {
                let label = Some(text::decode_str(bytes, rules));
                let items = read_order_array(objects, rest, within, depth, rules, limits, sink)?;
                return Ok(OcOrderItem::Section { label, items });
            }
            Value::String(Str::Encrypted(_), _) => {
                let oid = within.unwrap_or(ObjectId::new(0, 0));
                return Err(Unreadable::new(
                    oid,
                    Why::Locked,
                    format!("{oid} R holds /Order label as ciphertext: it takes a password"),
                ));
            }
            _ => {}
        }
    }
    let items = read_order_array(objects, arr, within, depth, rules, limits, sink)?;
    Ok(OcOrderItem::Section { label: None, items })
}

fn ref_array<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
    sink: &mut S,
) -> Result<Vec<ObjectId>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(Vec::new());
    };
    let Some((obj, id)) = reached(objects, raw, within, sink)? else {
        return Ok(Vec::new());
    };
    let Some(arr) = obj.as_array() else {
        say(sink, id, obj.at(), Fault::OcWrongType { key });
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        match item.as_reference() {
            Some(gid) => out.push(gid),
            None if !item.is_null() => say(sink, id, item.at(), Fault::OcWrongType { key }),
            None => {}
        }
    }
    Ok(out)
}

fn text_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<String>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = reached(objects, raw, within, sink)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::String(Str::Bytes(bytes), _) => Ok(Some(text::decode_str(bytes, rules))),
        Value::String(Str::Encrypted(_), _) => {
            let oid = id.unwrap_or(ObjectId::new(0, 0));
            Err(Unreadable::new(
                oid,
                Why::Locked,
                format!("{oid} R holds /{key} as ciphertext: it takes a password"),
            ))
        }
        _ => {
            say(sink, id, obj.at(), Fault::OcWrongType { key });
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId, dict};

    use super::{BaseState, read};
    use crate::Limits;

    #[test]
    fn ocg_and_ocmd_visibility_policies_and_ve_expressions() {
        let g1 = ObjectId::new(10, 0);
        let g2 = ObjectId::new(11, 0);
        let ocmd_all = ObjectId::new(12, 0);
        let ocmd_ve = ObjectId::new(13, 0);

        let objects = BTreeMap::from([
            (
                g1,
                Object::from(dict! {
                    "Type" => Object::name("OCG"),
                    "Name" => Object::text("Watermark Layer"),
                }),
            ),
            (
                g2,
                Object::from(dict! {
                    "Type" => Object::name("OCG"),
                    "Name" => Object::text("Hidden Audit Layer"),
                }),
            ),
            (
                ocmd_all,
                Object::from(dict! {
                    "Type" => Object::name("OCMD"),
                    "OCGs" => vec![Object::from(g1), Object::from(g2)],
                    "P" => Object::name("AllOn"),
                }),
            ),
            (
                ocmd_ve,
                Object::from(dict! {
                    "Type" => Object::name("OCMD"),
                    "VE" => vec![
                        Object::name("And"),
                        Object::from(g1),
                        Object::from(vec![Object::name("Not"), Object::from(g2)]),
                    ],
                }),
            ),
        ]);

        let catalog = dict! {
            "OCProperties" => dict! {
                "OCGs" => vec![Object::from(g1), Object::from(g2)],
                "D" => dict! {
                    "Name" => Object::text("Secret Default View"),
                    "BaseState" => Object::name("ON"),
                    "OFF" => vec![Object::from(g2)],
                    "Order" => vec![
                        Object::from(vec![Object::text("Secret Section"), Object::from(g1), Object::from(g2)]),
                    ],
                },
            },
        };

        let mut faults = Vec::new();
        let ocp = read(
            &objects,
            &catalog,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap()
        .unwrap();
        assert!(faults.is_empty(), "{faults:?}");
        assert_eq!(ocp.groups.len(), 2);
        assert_eq!(ocp.default_config.base_state, BaseState::On);

        let mut state = ocp.default_state();
        assert!(state.is_ocg_on(g1));
        assert!(!state.is_ocg_on(g2));
        assert!(!state.is_visible(&objects, &Object::from(ocmd_all)).unwrap());
        assert!(state.is_visible(&objects, &Object::from(ocmd_ve)).unwrap());

        state.set_ocg(g2, true);
        assert!(state.is_visible(&objects, &Object::from(ocmd_all)).unwrap());
        assert!(!state.is_visible(&objects, &Object::from(ocmd_ve)).unwrap());

        let dbg = format!("{ocp:?}");
        assert!(!dbg.contains("Secret"), "{dbg}");
        assert!(!dbg.contains("Watermark"), "{dbg}");
    }

    #[test]
    fn ocg_usage_respects_as_under_iso_and_applies_directly_under_pdfium() {
        use super::OcEvent;

        let g_print_as = ObjectId::new(20, 0);
        let g_print_no_as = ObjectId::new(21, 0);
        let objects = BTreeMap::from([
            (
                g_print_as,
                Object::from(dict! {
                    "Type" => Object::name("OCG"),
                    "Name" => Object::text("Print Stamp"),
                    "Usage" => dict! {
                        "Print" => dict! { "PrintState" => Object::name("ON") },
                    },
                }),
            ),
            (
                g_print_no_as,
                Object::from(dict! {
                    "Type" => Object::name("OCG"),
                    "Name" => Object::text("Unlisted Print Layer"),
                    "Usage" => dict! {
                        "Print" => dict! { "PrintState" => Object::name("ON") },
                    },
                }),
            ),
        ]);
        let catalog = dict! {
            "OCProperties" => dict! {
                "OCGs" => vec![Object::from(g_print_as), Object::from(g_print_no_as)],
                "D" => dict! {
                    "BaseState" => Object::name("OFF"),
                    "AS" => vec![Object::from(dict! {
                        "Event" => Object::name("Print"),
                        "OCGs" => vec![Object::from(g_print_as)],
                        "Category" => vec![Object::name("Print")],
                    })],
                },
            },
        };
        let ocp = read(&objects, &catalog, Rules::Iso, &Limits::DEFAULT, &mut ())
            .unwrap()
            .unwrap();

        // View: both OFF under both rules.
        let iso_view = ocp.default_state_for_event(OcEvent::View, Rules::Iso);
        assert!(!iso_view.is_ocg_on(g_print_as));
        assert!(!iso_view.is_ocg_on(g_print_no_as));

        // ISO Print: only `g_print_as` (listed in `/AS`) turns ON; `g_print_no_as` stays OFF!
        let iso_print = ocp.default_state_for_event(OcEvent::Print, Rules::Iso);
        assert!(iso_print.is_ocg_on(g_print_as));
        assert!(!iso_print.is_ocg_on(g_print_no_as));

        // PDFium Print: `/Usage /Print /PrintState /ON` turns BOTH groups ON directly!
        let pdfium_print = ocp.default_state_for_event(OcEvent::Print, Rules::Pdfium);
        assert!(pdfium_print.is_ocg_on(g_print_as));
        assert!(pdfium_print.is_ocg_on(g_print_no_as));
    }
}
