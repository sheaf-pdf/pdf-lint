//! Interactive actions (`/A`, `/Next`, `/AA`, and `/OpenAction`; ISO 32000-2,
//! 7.7.2, 12.6).
//!
//! Actions appear on link and widget annotations (`/A`, `/AA`), document
//! outline items (`/A`), form fields (`/AA`), pages (`/AA`), and the document
//! catalog (`/OpenAction` and `/AA`). [`read_chain`] walks an action dictionary
//! and any `/Next` sequence or tree iteratively with cycle detection and
//! [`Limits::action_chain`] enforcement.

use std::collections::HashSet;
use std::fmt;

use pdf_objects::fault::Sink;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Str, Unreadable, Value, text};

use crate::dest::{Dest, Walk, locked_string, parse_dest_value, parse_explicit_array};
use crate::fault::Fault;
use crate::info::{Len, len};
use crate::limits::Limits;

/// Standard named actions defined in ISO 32000-2, 12.6.4.12, Table 215, plus
/// viewer-specific extensions.
#[derive(Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NamedAction {
    /// `/NextPage`: navigate to the next page.
    NextPage,
    /// `/PrevPage`: navigate to the previous page.
    PrevPage,
    /// `/FirstPage`: navigate to the first page.
    FirstPage,
    /// `/LastPage`: navigate to the last page.
    LastPage,
    /// Any other viewer-defined `/N` name (e.g. `Print`, `Find`).
    Other(Box<[u8]>),
}

impl fmt::Debug for NamedAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NextPage => f.write_str("NextPage"),
            Self::PrevPage => f.write_str("PrevPage"),
            Self::FirstPage => f.write_str("FirstPage"),
            Self::LastPage => f.write_str("LastPage"),
            Self::Other(bytes) => f.debug_tuple("Other").field(&Len(bytes.len())).finish(),
        }
    }
}

impl NamedAction {
    /// Parses a `/Named` action `/N` name.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"NextPage" => Self::NextPage,
            b"PrevPage" => Self::PrevPage,
            b"FirstPage" => Self::FirstPage,
            b"LastPage" => Self::LastPage,
            other => Self::Other(Box::from(other)),
        }
    }

    /// Raw name bytes of this named action.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::NextPage => b"NextPage",
            Self::PrevPage => b"PrevPage",
            Self::FirstPage => b"FirstPage",
            Self::LastPage => b"LastPage",
            Self::Other(b) => b,
        }
    }
}

/// Typed PDF action dictionary (ISO 32000-2, 12.6.4).
///
/// Its `Debug` implementation reports byte lengths rather than URIs, file
/// paths, or scripts.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum Action {
    /// `/GoTo`: jump to a destination in the current document (12.6.4.2).
    GoTo {
        /// Target destination (`/D`).
        dest: Option<Dest>,
    },
    /// `/GoToR`: jump to a destination in an external PDF file (12.6.4.3).
    GoToR {
        /// Target file path decoded from `/F` (direct string or `/UF`/`/F` in a
        /// file specification dictionary).
        file: Option<String>,
        /// Target destination (`/D`; explicit arrays use 0-based page indices).
        dest: Option<Dest>,
        /// `/NewWindow` preference, if specified.
        new_window: Option<bool>,
    },
    /// `/GoToE`: jump to a destination in an embedded file (12.6.4.4).
    GoToE {
        /// Target file specification name, if specified.
        file: Option<String>,
        /// Target destination (`/D`).
        dest: Option<Dest>,
    },
    /// `/URI`: resolve a Uniform Resource Identifier (12.6.4.8).
    Uri {
        /// Raw URI bytes from `/URI` (UTF-8 / ASCII in ISO 32000-2; preserved as
        /// bytes matching `FPDFAction_GetURIPath`).
        uri: Vec<u8>,
        /// `/IsMap`: whether to append mouse click coordinates (`false` by
        /// default).
        is_map: bool,
    },
    /// `/Named`: execute a predefined viewer action (12.6.4.12).
    Named(NamedAction),
    /// `/Launch`: launch an application or open a file (12.6.4.5).
    Launch {
        /// Target file/application path from `/F` or `/Win/F`.
        file: Option<String>,
        /// `/NewWindow` preference, if specified.
        new_window: Option<bool>,
    },
    /// `/JavaScript`: execute an ECMAScript script (12.6.4.17).
    JavaScript {
        /// Whether `/JS` holds a string or stream script.
        has_script: bool,
    },
    /// `/Hide`: show or hide one or more annotations (`/H`, 12.6.4.11).
    Hide {
        /// `true` when `/H` hides the target(s) (`true` by default in PDF).
        hide: bool,
    },
    /// `/SubmitForm`: transmit form field values to a URL (12.7.6.2).
    SubmitForm {
        /// Submission destination URL/file specification, if present.
        url: Option<String>,
        /// `/Flags` bitmask (`0` by default).
        flags: u32,
    },
    /// `/ResetForm`: reset form fields to their `/DV` defaults (12.7.6.3).
    ResetForm {
        /// `/Flags` bitmask (`0` by default).
        flags: u32,
    },
    /// Any other action subtype `/S`.
    Other(Box<[u8]>),
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GoTo { dest } => f.debug_struct("GoTo").field("dest", dest).finish(),
            Self::GoToR {
                file,
                dest,
                new_window,
            } => f
                .debug_struct("GoToR")
                .field("file", &len(file))
                .field("dest", dest)
                .field("new_window", new_window)
                .finish(),
            Self::GoToE { file, dest } => f
                .debug_struct("GoToE")
                .field("file", &len(file))
                .field("dest", dest)
                .finish(),
            Self::Uri { uri, is_map } => f
                .debug_struct("Uri")
                .field("uri", &Len(uri.len()))
                .field("is_map", is_map)
                .finish(),
            Self::Named(n) => f.debug_tuple("Named").field(n).finish(),
            Self::Launch { file, new_window } => f
                .debug_struct("Launch")
                .field("file", &len(file))
                .field("new_window", new_window)
                .finish(),
            Self::JavaScript { has_script } => f
                .debug_struct("JavaScript")
                .field("has_script", has_script)
                .finish(),
            Self::Hide { hide } => f.debug_struct("Hide").field("hide", hide).finish(),
            Self::SubmitForm { url, flags } => f
                .debug_struct("SubmitForm")
                .field("url", &len(url))
                .field("flags", flags)
                .finish(),
            Self::ResetForm { flags } => f.debug_struct("ResetForm").field("flags", flags).finish(),
            Self::Other(s) => f.debug_tuple("Other").field(&Len(s.len())).finish(),
        }
    }
}

impl Action {
    /// Constructs a `/URI` action [`Dict`].
    #[must_use]
    pub fn uri_dict(uri: impl Into<Vec<u8>>) -> Dict {
        let mut d = Dict::new();
        d.set("Type", Object::name("Action"));
        d.set("S", Object::name("URI"));
        d.set("URI", Object::bytes(uri.into()));
        d
    }

    /// Constructs a `/GoTo` action [`Dict`].
    #[must_use]
    pub fn goto_dict(dest: &Dest) -> Dict {
        let mut d = Dict::new();
        d.set("Type", Object::name("Action"));
        d.set("S", Object::name("GoTo"));
        d.set("D", dest.to_object());
        d
    }
}

/// Additional-actions trigger dictionary (`/AA`, ISO 32000-2, 12.6.3, Tables
/// 197–200), covering annotation, page, form-field, and document catalog
/// triggers.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct AdditionalActions {
    /// `/E`: cursor enters active area (annotation).
    pub enter: Vec<Action>,
    /// `/X`: cursor exits active area (annotation).
    pub exit: Vec<Action>,
    /// `/D`: mouse button pressed inside active area (annotation).
    pub mouse_down: Vec<Action>,
    /// `/U`: mouse button released inside active area (annotation).
    pub mouse_up: Vec<Action>,
    /// `/Fo`: widget receives input focus (annotation).
    pub focus: Vec<Action>,
    /// `/Bl`: widget loses input focus (annotation).
    pub blur: Vec<Action>,
    /// `/PO`: page is opened (annotation or page `/AA` `/O`).
    pub page_open: Vec<Action>,
    /// `/PC`: page is closed (annotation or page `/AA` `/C`).
    pub page_close: Vec<Action>,
    /// `/PV`: page becomes visible (annotation).
    pub page_visible: Vec<Action>,
    /// `/PI`: page becomes invisible (annotation).
    pub page_invisible: Vec<Action>,
    /// `/K`: keystroke / selection modification (form field).
    pub keystroke: Vec<Action>,
    /// `/F`: field formatting before display (form field).
    pub format: Vec<Action>,
    /// `/V`: field value validation on change (form field).
    pub validate: Vec<Action>,
    /// `/C`: recalculation when another field changes (form field; note page
    /// `/AA` uses `/O` and `/C`, whereas annotation/field `/AA` uses `/C` for
    /// calculate).
    pub calculate: Vec<Action>,
    /// `/WC`: before closing the document (catalog `/AA`).
    pub will_close: Vec<Action>,
    /// `/WS`: before saving the document (catalog `/AA`).
    pub will_save: Vec<Action>,
    /// `/DS`: after saving the document (catalog `/AA`).
    pub did_save: Vec<Action>,
    /// `/WP`: before printing the document (catalog `/AA`).
    pub will_print: Vec<Action>,
    /// `/DP`: after printing the document (catalog `/AA`).
    pub did_print: Vec<Action>,
}

/// Catalog `/OpenAction` entry (ISO 32000-2, 7.7.2, Table 29): either an
/// explicit destination array or an action chain.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OpenAction {
    /// Direct destination (`[page /Fit ...]` or named under `Rules::Pdfium`).
    Dest(Dest),
    /// Action chain starting at `/OpenAction`.
    Actions(Vec<Action>),
}

/// Why reading an action chain, `/AA`, or `/OpenAction` failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An action `/Next` chain or tree exceeded [`Limits::action_chain`].
    #[error("an action chain exceeds Limits::action_chain")]
    TooLong,
    /// An object or string could not be read due to reader caps, a missing
    /// password, or an I/O failure.
    #[error("{0}")]
    Unreadable(#[from] Unreadable),
}

/// Reads the document catalog's `/OpenAction` entry (ISO 32000-2, 7.7.2, Table
/// 29).
///
/// # Errors
///
/// Returns [`Error::Unreadable`] on locked/capped/I/O failures or
/// [`Error::TooLong`] if an action chain exceeds [`Limits::action_chain`].
pub fn open_action<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Option<OpenAction>, Error> {
    let Some(raw) = catalog.get(b"OpenAction") else {
        return Ok(None);
    };
    let mut w = Walk {
        objects,
        rules,
        sink,
    };
    let Some((obj, id)) = w.reached(raw, None)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::Array(arr) => Ok(parse_explicit_array(&mut w, arr, id, obj.at(), true)?
            .map(|exp| OpenAction::Dest(Dest::Explicit(exp)))),
        Value::Dict(_) => {
            let chain = read_chain_inner(&mut w, raw, None, limits)?;
            if chain.is_empty() {
                Ok(None)
            } else {
                Ok(Some(OpenAction::Actions(chain)))
            }
        }
        Value::String(_, _) | Value::Name(_) if matches!(rules, Rules::Pdfium) => {
            w.say(id, obj.at(), Fault::DestInvalid);
            Ok(parse_dest_value(&mut w, raw, None, true)?.map(OpenAction::Dest))
        }
        _ => {
            w.say(id, obj.at(), Fault::ActionInvalid);
            Ok(None)
        }
    }
}

/// Reads an action dictionary (and any `/Next` sequence or tree) rooted at
/// `root`.
///
/// # Errors
///
/// Returns [`Error::TooLong`] if the chain exceeds [`Limits::action_chain`], or
/// [`Error::Unreadable`] on locked/capped/I/O failures.
pub fn read_chain<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    root: &'o Object,
    within: Option<ObjectId>,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Vec<Action>, Error> {
    let mut w = Walk {
        objects,
        rules,
        sink,
    };
    read_chain_inner(&mut w, root, within, limits)
}

/// Reads the `/AA` additional-actions dictionary on `dict`, if present.
///
/// # Errors
///
/// Same error conditions as [`read_chain`].
pub fn read_additional<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Option<AdditionalActions>, Error> {
    let mut w = Walk {
        objects,
        rules,
        sink,
    };
    let Some((aa, aa_id)) = w.dict_entry(dict, within, "AA")? else {
        return Ok(None);
    };
    let mut slot = |key: &[u8]| -> Result<Vec<Action>, Error> {
        match aa.get(key) {
            Some(raw) => read_chain_inner(&mut w, raw, aa_id, limits),
            None => Ok(Vec::new()),
        }
    };
    Ok(Some(AdditionalActions {
        enter: slot(b"E")?,
        exit: slot(b"X")?,
        mouse_down: slot(b"D")?,
        mouse_up: slot(b"U")?,
        focus: slot(b"Fo")?,
        blur: slot(b"Bl")?,
        page_open: slot(b"PO")?,
        page_close: slot(b"PC")?,
        page_visible: slot(b"PV")?,
        page_invisible: slot(b"PI")?,
        keystroke: slot(b"K")?,
        format: slot(b"F")?,
        validate: slot(b"V")?,
        calculate: slot(b"C")?,
        will_close: slot(b"WC")?,
        will_save: slot(b"WS")?,
        did_save: slot(b"DS")?,
        will_print: slot(b"WP")?,
        did_print: slot(b"DP")?,
    }))
}

/// Reads a file specification (`/F` or `/FS`; ISO 32000-2, 7.11.3) into a
/// display/path string, reading `/UF` first, then `/F`, `/DOS`, `/Unix`, or
/// `/Mac` (matching PDFium's `CPDF_FileSpec::GetFileName`).
///
/// # Errors
///
/// Returns [`Unreadable`] if an indirect reference or encrypted string cannot
/// be read.
pub fn read_file_spec<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    raw: &'o Object,
    within: Option<ObjectId>,
    key: &'static str,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<String>, Unreadable> {
    let mut w = Walk {
        objects,
        rules,
        sink,
    };
    read_file_spec_inner(&mut w, raw, within, key)
}

fn read_file_spec_inner<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    raw: &'o Object,
    within: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<String>, Unreadable> {
    let Some((obj, id)) = w.reached(raw, within)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::String(Str::Bytes(bytes), _) => Ok(Some(text::decode_str(bytes, w.rules))),
        Value::String(Str::Encrypted(_), _) => Err(locked_string(id, key)),
        Value::Dict(fs) => {
            for sub in ["UF", "F", "DOS", "Unix", "Mac"] {
                if let Some(val) = text_entry(w, fs, id, sub)? {
                    return Ok(Some(val));
                }
            }
            Ok(None)
        }
        _ => {
            w.say(id, obj.at(), Fault::ActionWrongType { key });
            Ok(None)
        }
    }
}

fn read_chain_inner<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    root: &'o Object,
    within: Option<ObjectId>,
    limits: &Limits,
) -> Result<Vec<Action>, Error> {
    let mut out = Vec::new();
    let mut visited: HashSet<ObjectId> = HashSet::new();
    let mut stack: Vec<(&'o Object, Option<ObjectId>)> = vec![(root, within)];

    while let Some((raw, parent_id)) = stack.pop() {
        let Some((obj, id)) = w.reached(raw, parent_id)? else {
            continue;
        };
        if let Some(oid) = id
            && !visited.insert(oid)
        {
            w.say(parent_id, raw.at(), Fault::ActionInvalid);
            continue;
        }
        match obj.value() {
            Value::Dict(d) => {
                if out.len() >= limits.action_chain {
                    return Err(Error::TooLong);
                }
                if let Some(act) = parse_single_action(w, d, id)? {
                    out.push(act);
                }
                if let Some(next) = d.get(b"Next") {
                    stack.push((next, id));
                }
            }
            Value::Array(arr) => {
                for item in arr.iter().rev() {
                    stack.push((item, id));
                }
            }
            _ => {
                w.say(id, obj.at(), Fault::ActionInvalid);
            }
        }
    }
    Ok(out)
}

fn parse_single_action<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    d: &'o Dict,
    id: Option<ObjectId>,
) -> Result<Option<Action>, Unreadable> {
    let Some(s) = name_entry(w, d, id, "S")? else {
        w.say(id, None, Fault::ActionInvalid);
        return Ok(None);
    };
    let action = match s {
        b"GoTo" => {
            let dest = match d.get(b"D") {
                Some(raw) => parse_dest_value(w, raw, id, true)?,
                None => {
                    w.say(id, None, Fault::ActionInvalid);
                    None
                }
            };
            Action::GoTo { dest }
        }
        b"GoToR" => {
            let file = match d.get(b"F") {
                Some(f) => read_file_spec_inner(w, f, id, "F")?,
                None => {
                    w.say(id, None, Fault::ActionInvalid);
                    None
                }
            };
            let dest = match d.get(b"D") {
                Some(raw) => parse_dest_value(w, raw, id, false)?,
                None => None,
            };
            let new_window = bool_entry(w, d, id, "NewWindow")?;
            Action::GoToR {
                file,
                dest,
                new_window,
            }
        }
        b"GoToE" => {
            let file = match d.get(b"F") {
                Some(f) => read_file_spec_inner(w, f, id, "F")?,
                None => None,
            };
            let dest = match d.get(b"D") {
                Some(raw) => parse_dest_value(w, raw, id, false)?,
                None => None,
            };
            Action::GoToE { file, dest }
        }
        b"URI" => {
            let Some(uri_bytes) = byte_string_entry(w, d, id, "URI")? else {
                w.say(id, None, Fault::ActionInvalid);
                return Ok(None);
            };
            let is_map = bool_entry(w, d, id, "IsMap")?.unwrap_or(false);
            Action::Uri {
                uri: uri_bytes.to_vec(),
                is_map,
            }
        }
        b"Named" => {
            let Some(n) = name_entry(w, d, id, "N")? else {
                w.say(id, None, Fault::ActionInvalid);
                return Ok(None);
            };
            Action::Named(NamedAction::from_name(n))
        }
        b"Launch" => {
            let mut file = match d.get(b"F") {
                Some(f) => read_file_spec_inner(w, f, id, "F")?,
                None => None,
            };
            if file.is_none()
                && let Some((win, win_id)) = w.dict_entry(d, id, "Win")?
                && let Some(wf) = win.get(b"F")
            {
                file = read_file_spec_inner(w, wf, win_id, "F")?;
            }
            let new_window = bool_entry(w, d, id, "NewWindow")?;
            Action::Launch { file, new_window }
        }
        b"JavaScript" => {
            let has_script = match d.get(b"JS") {
                Some(js_raw) => match w.reached(js_raw, id)? {
                    Some((obj, js_id)) => match obj.value() {
                        Value::String(Str::Bytes(_), _) | Value::Stream(_) => true,
                        Value::String(Str::Encrypted(_), _) => {
                            return Err(locked_string(js_id, "JS"));
                        }
                        _ => {
                            w.say(js_id, obj.at(), Fault::ActionWrongType { key: "JS" });
                            false
                        }
                    },
                    None => false,
                },
                None => {
                    w.say(id, None, Fault::ActionInvalid);
                    false
                }
            };
            Action::JavaScript { has_script }
        }
        b"Hide" => {
            let hide = bool_entry(w, d, id, "H")?.unwrap_or(true);
            Action::Hide { hide }
        }
        b"SubmitForm" => {
            let url = match d.get(b"F") {
                Some(f) => read_file_spec_inner(w, f, id, "F")?,
                None => None,
            };
            let flags = int_entry(w, d, id, "Flags")?.unwrap_or(0) as i32 as u32;
            Action::SubmitForm { url, flags }
        }
        b"ResetForm" => {
            let flags = int_entry(w, d, id, "Flags")?.unwrap_or(0) as i32 as u32;
            Action::ResetForm { flags }
        }
        other => Action::Other(Box::from(other)),
    };
    Ok(Some(action))
}

fn name_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<&'o [u8]>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = w.reached(raw, within)? else {
        return Ok(None);
    };
    match obj.as_name() {
        Some(n) => Ok(Some(n)),
        None => {
            w.say(id, obj.at(), Fault::ActionWrongType { key });
            Ok(None)
        }
    }
}

fn bool_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<bool>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = w.reached(raw, within)? else {
        return Ok(None);
    };
    match obj.as_bool() {
        Some(b) => Ok(Some(b)),
        None => {
            w.say(id, obj.at(), Fault::ActionWrongType { key });
            Ok(None)
        }
    }
}

fn int_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<i64>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = w.reached(raw, within)? else {
        return Ok(None);
    };
    match obj.as_integer() {
        Some(n) => Ok(Some(n)),
        None => {
            w.say(id, obj.at(), Fault::ActionWrongType { key });
            Ok(None)
        }
    }
}

fn text_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<String>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = w.reached(raw, within)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::String(Str::Bytes(bytes), _) => Ok(Some(text::decode_str(bytes, w.rules))),
        Value::String(Str::Encrypted(_), _) => Err(locked_string(id, key)),
        _ => {
            w.say(id, obj.at(), Fault::ActionWrongType { key });
            Ok(None)
        }
    }
}

fn byte_string_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    w: &mut Walk<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<&'o [u8]>, Unreadable> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = w.reached(raw, within)? else {
        return Ok(None);
    };
    match obj.value() {
        Value::String(Str::Bytes(bytes), _) => Ok(Some(&bytes[..])),
        Value::String(Str::Encrypted(_), _) => Err(locked_string(id, key)),
        Value::Name(bytes) if matches!(w.rules, Rules::Pdfium) => {
            w.say(id, obj.at(), Fault::ActionWrongType { key });
            Ok(Some(&bytes[..]))
        }
        _ => {
            w.say(id, obj.at(), Fault::ActionWrongType { key });
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId, dict};

    use super::{
        Action, AdditionalActions, Error, NamedAction, OpenAction, open_action, read_additional,
        read_chain,
    };
    use crate::dest::Dest;
    use crate::{Fault, Limits};

    #[test]
    fn action_types_are_send_sync_and_debug_hygienic() {
        fn check<T: Send + Sync>() {}
        check::<NamedAction>();
        check::<Action>();
        check::<AdditionalActions>();
        check::<OpenAction>();
        check::<Error>();

        let a = Action::Uri {
            uri: b"https://secret.internal.example/key=999".to_vec(),
            is_map: false,
        };
        let b = Action::Launch {
            file: Some("C:\\secret\\payload.exe".into()),
            new_window: Some(true),
        };
        let dbg = format!("{a:?} {b:?}");
        assert!(!dbg.contains("secret"), "{dbg}");
        assert!(!dbg.contains("payload"), "{dbg}");
    }

    #[test]
    fn cyclic_next_chain_cap_and_open_action() {
        let a1 = ObjectId::new(1, 0);
        let a2 = ObjectId::new(2, 0);
        let mut objects = BTreeMap::new();
        objects.insert(
            a1,
            Object::from(dict! {
                "S" => Object::name("URI"),
                "URI" => Object::text("https://a.example"),
                "Next" => a2,
            }),
        );
        objects.insert(
            a2,
            Object::from(dict! {
                "S" => Object::name("Named"),
                "N" => Object::name("LastPage"),
                "Next" => a1,
            }),
        );
        let mut faults = Vec::new();
        let chain = read_chain(
            &objects,
            &Object::from(a1),
            None,
            Rules::Pdfium,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap();
        assert_eq!(chain.len(), 2);
        assert!(faults.iter().any(|(_, f)| *f == Fault::ActionInvalid));

        let mut small = Limits::DEFAULT;
        small.action_chain = 1;
        assert_eq!(
            read_chain(
                &objects,
                &Object::from(a1),
                None,
                Rules::Pdfium,
                &small,
                &mut ()
            ),
            Err(Error::TooLong)
        );

        let cat_dest = dict! {
            "OpenAction" => vec![Object::from(ObjectId::new(5, 0)), Object::name("Fit")],
        };
        assert_eq!(
            open_action(&objects, &cat_dest, Rules::Iso, &Limits::DEFAULT, &mut ()).unwrap(),
            Some(OpenAction::Dest(Dest::fit(ObjectId::new(5, 0))))
        );

        let with_aa = dict! {
            "AA" => dict! {
                "WC" => a1,
            },
        };
        let aa = read_additional(
            &objects,
            &with_aa,
            None,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap()
        .unwrap();
        assert_eq!(aa.will_close.len(), 2);
    }
}
