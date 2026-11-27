//! The interactive form dictionary and the walk over its field tree.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use pdf_annots::Quadding;
use pdf_objects::fault::Sink;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Str, Value, text};

#[cfg(feature = "text-layout")]
use crate::appearance::FieldFont;
use crate::error::Error;
use crate::fault::Fault;
use crate::field::{Field, FieldFlags, FieldType, FieldValue, Found, Widget, kind_of};
use crate::limits::{Cap, Limits};
use crate::phases::phase;
use crate::read::{DictAt, EntryAt, Reader, locked_string};

/// A document's interactive form (`/AcroForm`, ISO 32000-2 §12.7.3) with its
/// field tree read.
///
/// Fields are held in one list, each before its kids ([`fields`](Self::fields));
/// a field names its parent and kids by their indexes there.
///
/// `Debug` prints counts and flags, never a field's name or value.
#[derive(Clone)]
pub struct Form<'o> {
    dict: &'o Dict,
    id: Option<ObjectId>,
    fields: Vec<Field<'o>>,
    roots: Vec<usize>,
    da: Option<&'o [u8]>,
    quadding: Option<Quadding>,
    need_appearances: bool,
    sig_flags: u32,
    resources: Option<DictAt<'o>>,
    fonts: Option<DictAt<'o>>,
}

impl<'o> Form<'o> {
    /// Reads the form a document's catalog names, without collecting
    /// non-fatal [`Fault`]s. `Ok(None)` when the catalog has no `/AcroForm`
    /// dictionary.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unreadable`] on locked, I/O, or reader-cap failures, or
    /// [`Error::Limit`] when the tree holds more than [`Limits::fields`] fields
    /// and widgets or a value more than [`Limits::items`] items.
    pub fn read<R: Resolve<'o>>(
        objects: R,
        catalog: &'o Dict,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Option<Self>, Error> {
        Self::read_with(objects, catalog, rules, limits, &mut ())
    }

    /// Reads the form a document's catalog names, reporting recoverable ISO
    /// 32000-2 departures to `sink`.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::read`].
    pub fn read_with<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        objects: R,
        catalog: &'o Dict,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Option<Self>, Error> {
        let mut reader = Reader {
            objects,
            rules,
            limits,
            sink,
        };
        match reader.dict_entry(catalog, None, "AcroForm")? {
            Some((dict, id)) => read_form(&mut reader, dict, id).map(Some),
            None => Ok(None),
        }
    }

    /// Reads a form from its `/AcroForm` dictionary, `id` its object number
    /// when it is indirect, reporting recoverable departures to `sink` (`&mut
    /// ()` for none).
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::read`].
    pub fn from_dict<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
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
        read_form(&mut reader, dict, id)
    }

    /// The `/AcroForm` dictionary.
    #[must_use]
    pub const fn dict(&self) -> &'o Dict {
        self.dict
    }

    /// The `/AcroForm` dictionary's object number, or `None` when the catalog
    /// holds it directly.
    #[must_use]
    pub const fn id(&self) -> Option<ObjectId> {
        self.id
    }

    /// Every field of the tree, each before its kids.
    #[must_use]
    pub fn fields(&self) -> &[Field<'o>] {
        &self.fields
    }

    /// The indexes in [`fields`](Self::fields) of the fields `/Fields` lists,
    /// in its order.
    #[must_use]
    pub fn roots(&self) -> &[usize] {
        &self.roots
    }

    /// The terminal fields with their indexes in [`fields`](Self::fields).
    pub fn terminals(&self) -> impl Iterator<Item = (usize, &Field<'o>)> {
        self.fields
            .iter()
            .enumerate()
            .filter(|(_, field)| field.is_terminal())
    }

    /// The fully qualified name of the field at `index`: the partial names from
    /// the top of the tree down to it, joined by periods, a field with no
    /// partial name adding nothing. `None` when `index` is out of range.
    #[must_use]
    pub fn full_name(&self, index: usize) -> Option<String> {
        let mut parts = Vec::new();
        let mut at = Some(index);
        while let Some(i) = at {
            let field = self.fields.get(i)?;
            if let Some(name) = field.partial_name() {
                parts.push(name);
            }
            at = field.parent;
        }
        parts.reverse();
        Some(parts.join("."))
    }

    /// The index of the first field whose fully qualified name
    /// ([`full_name`](Self::full_name)) is `name`.
    ///
    /// It looks at each field in turn, matching `name` against the field's
    /// partial names from its own up, and builds no name: a caller with many
    /// names to find in a large form keeps a map of its own from
    /// [`full_name`](Self::full_name).
    #[must_use]
    pub fn find(&self, name: &str) -> Option<usize> {
        (0..self.fields.len()).find(|&i| self.is_named(i, name))
    }

    /// Whether the fully qualified name of the field at `index` is `name`:
    /// `name` is eaten from its end, a partial name and the period before it
    /// at a time, a field with no partial name eating nothing.
    fn is_named(&self, index: usize, name: &str) -> bool {
        let mut rest = name;
        // Whether a partial name has been eaten, so the next is followed by
        // a period.
        let mut joined = false;
        let mut at = Some(index);
        while let Some(field) = at.and_then(|i| self.fields.get(i)) {
            if let Some(part) = field.partial_name() {
                if joined {
                    match rest.strip_suffix('.') {
                        Some(before) => rest = before,
                        None => return false,
                    }
                }
                match rest.strip_suffix(part) {
                    Some(before) => rest = before,
                    None => return false,
                }
                joined = true;
            }
            at = field.parent;
        }
        rest.is_empty()
    }

    /// The bytes of the form's default appearance string (`/DA`), which a field
    /// with none of its own is drawn with.
    #[must_use]
    pub const fn default_appearance(&self) -> Option<&'o [u8]> {
        self.da
    }

    /// The form's quadding (`/Q`), for a field with none of its own; left when
    /// none is written.
    #[must_use]
    pub fn quadding(&self) -> Quadding {
        self.quadding.unwrap_or_default()
    }

    /// Whether the form asks its reader to make the fields' appearances
    /// (`/NeedAppearances`).
    #[must_use]
    pub const fn need_appearances(&self) -> bool {
        self.need_appearances
    }

    /// The form's `/SigFlags`: bit 1 says signatures exist, bit 2 that the file
    /// is to be saved by appending only.
    #[must_use]
    pub const fn sig_flags(&self) -> u32 {
        self.sig_flags
    }

    /// Whether the form holds an XFA form too: an `/XFA` entry is written,
    /// whatever it holds, as PDFium counts it. [`FormKind::read`] reads it by
    /// the rules it's given, and says whether the XFA is the whole form.
    ///
    /// [`FormKind::read`]: crate::FormKind::read
    #[must_use]
    pub fn has_xfa(&self) -> bool {
        self.dict.get("XFA").is_some()
    }

    /// The form's default resources (`/DR`), with the dictionary's object
    /// number when it is indirect.
    #[must_use]
    pub const fn resources(&self) -> Option<(&'o Dict, Option<ObjectId>)> {
        self.resources
    }

    /// The names of the fonts in the default resources (`/DR /Font`), which a
    /// `/DA` string's font operator may name.
    pub fn font_names(&self) -> impl Iterator<Item = &'o [u8]> {
        self.fonts
            .into_iter()
            .flat_map(|(fonts, _)| fonts.iter().map(|(name, _)| name))
    }

    /// The font the default resources hold under `name`, with its object
    /// number when it is indirect; `None` when there is none or it isn't a
    /// dictionary.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unreadable`] on locked, I/O, or reader-cap failures.
    pub fn font<R: Resolve<'o>>(
        &self,
        objects: R,
        name: &[u8],
        limits: &Limits,
    ) -> Result<Option<(&'o Dict, Option<ObjectId>)>, Error> {
        let Some((fonts, within)) = self.fonts else {
            return Ok(None);
        };
        let Some(raw) = fonts.get(name) else {
            return Ok(None);
        };
        let mut reader = Reader {
            objects,
            rules: Rules::Pdfium,
            limits,
            sink: &mut (),
        };
        let Some((object, id)) = reader.reached(raw, within)? else {
            return Ok(None);
        };
        let own = raw.as_reference().is_some().then_some(id).flatten();
        Ok(object.as_dict().map(|dict| (dict, own)))
    }

    /// The font a `/DA` string's font name `name` draws a field's text in: the
    /// default resources' font of that name, read, or the standard 14 font the
    /// name falls back to where they have none ([`FieldFont::find`] over the
    /// form's `/DR /Font`), departures told to `sink`.
    ///
    /// # Errors
    ///
    /// As [`FieldFont::find`]: [`Error::Limit`] past
    /// [`Limits::font_budget`], [`Error::Unreadable`] on locked, I/O, or
    /// reader-cap failures.
    #[cfg(feature = "text-layout")]
    pub fn field_font<R, S>(
        &self,
        objects: R,
        name: &[u8],
        limits: &Limits,
        sink: &mut S,
    ) -> Result<FieldFont, Error>
    where
        R: Resolve<'o> + pdf_objects::Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        FieldFont::find(objects, self.fonts, name, limits, sink)
    }
}

impl fmt::Debug for Form<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Form")
            .field("id", &self.id)
            .field("fields", &self.fields.len())
            .field("roots", &self.roots.len())
            .field("need_appearances", &self.need_appearances)
            .field("sig_flags", &self.sig_flags)
            .finish_non_exhaustive()
    }
}

/// What a field hands down to its kids.
#[derive(Clone, Default)]
struct Inherited<'o> {
    field_type: Option<FieldType>,
    flags: Option<FieldFlags>,
    value: Option<Arc<FieldValue<'o>>>,
    default_value: Option<Arc<FieldValue<'o>>>,
    /// `/Opt` and `/I` as a field hands them down ([`Found::entry`]),
    /// undecoded, and `/TI`.
    options: Option<EntryAt<'o>>,
    selected: Option<EntryAt<'o>>,
    top_index: Option<u32>,
    da: Option<&'o [u8]>,
    quadding: Option<Quadding>,
    max_len: Option<u32>,
    /// Whether a `/MaxLen` was held on the way down, a number or not, `null`
    /// included: a field below one takes none of its widgets' under PDFium's
    /// rules.
    max_len_held: bool,
}

/// What an entry of `/Fields` or `/Kids` turned out to be.
enum Node<'o> {
    Field(usize),
    Widget(Widget<'o>),
    Skipped,
}

struct Walk<'o> {
    fields: Vec<Field<'o>>,
    handed: Vec<Inherited<'o>>,
    depths: Vec<usize>,
    by_id: HashMap<ObjectId, usize>,
    values: HashMap<(ObjectId, &'static str), Option<Arc<FieldValue<'o>>>>,
    seen: HashSet<ObjectId>,
    count: usize,
}

/// The default resources (`/DR`) of the form `dict`, object `id`, and their
/// font dictionary (`/Font`), each with its own number when it is indirect.
fn default_resources<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    id: Option<ObjectId>,
) -> Result<(Option<DictAt<'o>>, Option<DictAt<'o>>), Error> {
    let resources = reader.dict_entry(dict, id, "DR")?;
    let fonts = match resources {
        Some((dr, dr_id)) => reader.dict_entry(dr, dr_id.or(id), "Font")?,
        None => None,
    };
    Ok((resources, fonts))
}

/// The font dictionary of the default resources of the form `catalog` names
/// (`/AcroForm /DR /Font`), as [`Form`] holds it, nothing else of the form
/// read.
#[cfg(feature = "text-layout")]
pub(crate) fn catalog_fonts<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    catalog: &'o Dict,
) -> Result<Option<DictAt<'o>>, Error> {
    match reader.dict_entry(catalog, None, "AcroForm")? {
        Some((dict, id)) => Ok(default_resources(reader, dict, id)?.1),
        None => Ok(None),
    }
}

fn read_form<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    id: Option<ObjectId>,
) -> Result<Form<'o>, Error> {
    let da = reader.bytes_entry(dict, id, "DA")?;
    let quadding = reader.int_entry(dict, id, "Q")?.map(Quadding::from_pdf_int);
    let need_appearances = reader
        .bool_entry(dict, id, "NeedAppearances")?
        .unwrap_or(false);
    let sig_flags = reader
        .int_entry(dict, id, "SigFlags")?
        .map_or(0, |n| n as u32);
    let (resources, fonts) = default_resources(reader, dict, id)?;

    let mut walk = Walk {
        fields: Vec::new(),
        handed: Vec::new(),
        depths: Vec::new(),
        by_id: HashMap::new(),
        values: HashMap::new(),
        seen: HashSet::new(),
        count: 0,
    };
    let mut roots = Vec::new();
    if let Some((entries, within)) = reader.array_entry(dict, id, "Fields")? {
        let inherited = Inherited {
            da,
            quadding,
            ..Inherited::default()
        };
        let mut deferred = Vec::new();
        for raw in entries {
            if has_parent_dict(reader, raw, within)? {
                deferred.push(raw);
            } else if let Node::Field(index) = node(
                reader, &mut walk, raw, within, None, &inherited, 0, false, false,
            )? {
                roots.push(index);
            }
        }
        if !deferred.is_empty() {
            attach_deferred(reader, &mut walk, &mut roots, &deferred, within, &inherited)?;
        }
    }
    phase!(
        DEBUG,
        "form",
        fields = walk.fields.len(),
        entries = walk.count
    );

    Ok(Form {
        dict,
        id,
        fields: walk.fields,
        roots,
        da,
        quadding,
        need_appearances,
        sig_flags,
        resources,
        fonts,
    })
}

/// Whether `raw` in `/Fields` resolves to a dictionary whose `/Parent` also
/// resolves to a dictionary, so top-level roots in `/Fields` should walk first.
fn has_parent_dict<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &Reader<'_, R, S>,
    raw: &'o Object,
    within: Option<ObjectId>,
) -> Result<bool, Error> {
    let objects: &dyn Resolve<'o> = &reader.objects;
    let mut quiet = Reader {
        objects,
        rules: reader.rules,
        limits: reader.limits,
        sink: &mut (),
    };
    let Some((obj, reached_id)) = quiet.reached(raw, within)? else {
        return Ok(false);
    };
    let Some(dict) = obj.as_dict() else {
        return Ok(false);
    };
    let entry_within = raw.as_reference().and(reached_id).or(within);
    Ok(quiet.dict_entry(dict, entry_within, "Parent")?.is_some())
}

/// Whether `raw` resolves to an unseen field kid dictionary (having `/T` or an
/// array `/Kids`).
fn is_field_kid<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &Reader<'_, R, S>,
    seen: &HashSet<ObjectId>,
    raw: &'o Object,
    within: Option<ObjectId>,
) -> Result<bool, Error> {
    let objects: &dyn Resolve<'o> = &reader.objects;
    let mut quiet = Reader {
        objects,
        rules: reader.rules,
        limits: reader.limits,
        sink: &mut (),
    };
    let Some((obj, reached_id)) = quiet.reached(raw, within)? else {
        return Ok(false);
    };
    let id = raw.as_reference().and(reached_id);
    if id.is_some_and(|oid| seen.contains(&oid)) {
        return Ok(false);
    }
    let Some(dict) = obj.as_dict() else {
        return Ok(false);
    };
    Ok(dict.get("T").is_some() || quiet.array_entry(dict, id.or(within), "Kids")?.is_some())
}

/// Pass 2 over `/Fields` entries whose `/Parent` resolves to a dictionary: skips
/// entries already reached via Pass 1 `/Kids`, and otherwise climbs `/Parent`
/// (up to `Limits::depth`, cycle-safe) so ancestors are loaded and inherited
/// from before attaching unreferenced children or widgets.
fn attach_deferred<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    walk: &mut Walk<'o>,
    roots: &mut Vec<usize>,
    deferred: &[&'o Object],
    within: Option<ObjectId>,
    form_inherited: &Inherited<'o>,
) -> Result<(), Error> {
    // Pre-scan which parent `ObjectId`s have at least one field kid among
    // `deferred`, so mixed nameless widgets + field kids under a `/Kids`-less
    // parent become terminal fields regardless of their order in `/Fields`.
    let mut mixed_parents = HashSet::new();
    {
        let objects: &dyn Resolve<'o> = &reader.objects;
        let mut quiet = Reader {
            objects,
            rules: reader.rules,
            limits: reader.limits,
            sink: &mut (),
        };
        for &raw in deferred {
            if let Some((obj, reached_id)) = quiet.reached(raw, within)?
                && let Some(dict) = obj.as_dict()
            {
                let entry_within = raw.as_reference().and(reached_id).or(within);
                if (dict.get("T").is_some()
                    || quiet.array_entry(dict, entry_within, "Kids")?.is_some())
                    && let Some(p_raw) = dict.get("Parent")
                    && p_raw.as_reference().is_some()
                    && let Some((p_obj, Some(p_id))) = quiet.reached(p_raw, entry_within)?
                    && p_obj.as_dict().is_some()
                {
                    mixed_parents.insert(p_id);
                }
            }
        }
    }

    for &raw in deferred {
        let entry_info = {
            let objects: &dyn Resolve<'o> = &reader.objects;
            let mut quiet = Reader {
                objects,
                rules: reader.rules,
                limits: reader.limits,
                sink: &mut (),
            };
            quiet.reached(raw, within)?
        };
        let Some((entry_obj, reached_id)) = entry_info else {
            continue;
        };
        let entry_id = raw.as_reference().and(reached_id);
        if entry_id.is_some_and(|oid| walk.seen.contains(&oid)) {
            continue;
        }
        let Some(entry_dict) = entry_obj.as_dict() else {
            continue;
        };

        // Climb `/Parent` references up to `Limits::depth`.
        let mut ancestors: Vec<(&'o Object, ObjectId, Option<ObjectId>)> = Vec::new();
        let mut anchor: Option<(usize, ObjectId)> = None;
        {
            let objects: &dyn Resolve<'o> = &reader.objects;
            let mut quiet = Reader {
                objects,
                rules: reader.rules,
                limits: reader.limits,
                sink: &mut (),
            };
            let mut climbed = HashSet::new();
            if let Some(oid) = entry_id {
                climbed.insert(oid);
            }
            let mut cur_dict = entry_dict;
            let mut cur_within = entry_id.or(within);
            while ancestors.len() + 1 < reader.limits.depth {
                let Some(p_raw) = cur_dict.get("Parent") else {
                    break;
                };
                if p_raw.as_reference().is_none() {
                    break;
                }
                let Some((p_obj, Some(p_id))) = quiet.reached(p_raw, cur_within)? else {
                    break;
                };
                let Some(p_dict) = p_obj.as_dict() else {
                    break;
                };
                if let Some(&existing_idx) = walk.by_id.get(&p_id) {
                    anchor = Some((existing_idx, p_id));
                    break;
                }
                if walk.seen.contains(&p_id) || !climbed.insert(p_id) {
                    break;
                }
                ancestors.push((p_raw, p_id, cur_within));
                cur_dict = p_dict;
                cur_within = Some(p_id);
            }
        }

        let mut cur_parent = anchor.map(|(idx, pid)| (idx, Some(pid)));
        for (p_raw, p_id, raw_within) in ancestors.into_iter().rev() {
            if let Some(&idx) = walk.by_id.get(&p_id) {
                cur_parent = Some((idx, Some(p_id)));
                continue;
            }
            if walk.seen.contains(&p_id) {
                cur_parent = None;
                break;
            }
            let (inh, depth) = match cur_parent {
                Some((p_idx, _)) => (walk.handed[p_idx].clone(), walk.depths[p_idx] + 1),
                None => (form_inherited.clone(), 0),
            };
            match node(
                reader, &mut *walk, p_raw, raw_within, cur_parent, &inh, depth, false, true,
            )? {
                Node::Field(idx) => {
                    if let Some((p_idx, _)) = cur_parent {
                        walk.fields[p_idx].kids.push(idx);
                    } else {
                        roots.push(idx);
                    }
                    cur_parent = Some((idx, Some(p_id)));
                }
                _ => {
                    cur_parent = None;
                    break;
                }
            }
        }

        if entry_id.is_some_and(|oid| walk.seen.contains(&oid)) {
            if let Some((p_idx, p_id)) = cur_parent
                && walk.fields[p_idx].is_terminal()
                && walk.fields[p_idx].field_type.is_none()
            {
                reader.say(p_id, None, Fault::MissingFieldType);
            }
            continue;
        }

        match cur_parent {
            Some((p_idx, p_id)) => {
                let inh = walk.handed[p_idx].clone();
                let depth = walk.depths[p_idx] + 1;
                let as_field = !walk.fields[p_idx].kids.is_empty()
                    || p_id.is_some_and(|pid| mixed_parents.contains(&pid));
                match node(
                    reader,
                    &mut *walk,
                    raw,
                    within,
                    Some((p_idx, p_id)),
                    &inh,
                    depth,
                    as_field,
                    false,
                )? {
                    Node::Field(kid) => walk.fields[p_idx].kids.push(kid),
                    Node::Widget(mut widget) => {
                        let was_unheld = reader.rules == Rules::Pdfium
                            && !walk.handed[p_idx].max_len_held
                            && walk.fields[p_idx].max_len.is_none()
                            && !walk.fields[p_idx]
                                .widgets
                                .iter()
                                .any(|w| !w.merged && w.dict.get("MaxLen").is_some());
                        let w_dict = widget.dict;
                        let w_at = widget.id.or(p_id);
                        widget.index = walk.fields[p_idx].widgets.len();
                        walk.fields[p_idx].widgets.push(widget);
                        if was_unheld && w_dict.get("MaxLen").is_some() {
                            walk.fields[p_idx].max_len = reader
                                .int_entry(w_dict, w_at, "MaxLen")?
                                .and_then(|n| u32::try_from(n).ok());
                        }
                    }
                    Node::Skipped => {}
                }
                if walk.fields[p_idx].is_terminal() && walk.fields[p_idx].field_type.is_none() {
                    reader.say(p_id, None, Fault::MissingFieldType);
                }
            }
            None => {
                if let Node::Field(idx) = node(
                    reader,
                    &mut *walk,
                    raw,
                    within,
                    None,
                    form_inherited,
                    0,
                    false,
                    false,
                )? {
                    roots.push(idx);
                }
            }
        }
    }
    Ok(())
}

/// Reads one entry of `/Fields` or of a field's `/Kids`, and the tree under
/// it. `parent` is the field whose `/Kids` holds the entry, with its number.
#[expect(clippy::too_many_arguments)]
fn node<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    walk: &mut Walk<'o>,
    raw: &'o Object,
    within: Option<ObjectId>,
    parent: Option<(usize, Option<ObjectId>)>,
    inherited: &Inherited<'o>,
    depth: usize,
    as_field: bool,
    pending_kid: bool,
) -> Result<Node<'o>, Error> {
    let Some((object, reached_id)) = reader.reached(raw, within)? else {
        return Ok(Node::Skipped);
    };
    let id = if raw.as_reference().is_some() {
        reached_id
    } else {
        reader.say(within, raw.at(), Fault::DirectField);
        None
    };
    let Some(dict) = object.as_dict() else {
        reader.say(id.or(within), object.at(), Fault::FieldNotDict);
        return Ok(Node::Skipped);
    };
    if let Some(id) = id
        && !walk.seen.insert(id)
    {
        reader.say(within, raw.at(), Fault::FieldLoop(id));
        return Ok(Node::Skipped);
    }
    walk.count += 1;
    if walk.count > reader.limits.fields {
        return Err(Error::Limit(Cap::Fields));
    }
    if let Some((_, Some(parent_id))) = parent
        && reader.sink.wants()
        && dict.get("Parent").and_then(Object::as_reference) != Some(parent_id)
    {
        reader.say(id.or(within), None, Fault::ParentMismatch);
    }

    let kids = reader.array_entry(dict, id.or(within), "Kids")?;
    // A kid with no name and no kids of its own is one of its parent's
    // widgets, not a field, unless its sibling in `/Kids` is a field (`as_field`).
    if !as_field && parent.is_some() && kids.is_none() && dict.get("T").is_none() {
        return Ok(Node::Widget(Widget {
            dict,
            id,
            merged: false,
            // Its place among its field's widgets, known where it's kept.
            index: 0,
        }));
    }
    if depth >= reader.limits.depth {
        reader.say(id.or(within), None, Fault::TooDeep);
        return Ok(Node::Skipped);
    }

    let at = id.or(within);
    let name = reader.text_entry(dict, at, "T")?;
    let field_type = match reader.name_entry(dict, at, "FT")? {
        Some(ft) => {
            let field_type = FieldType::from_name(ft);
            if field_type.is_none() {
                reader.say(at, None, Fault::WrongType { key: "FT" });
            }
            field_type
        }
        None => inherited.field_type,
    };
    let flags = reader
        .int_entry(dict, at, "Ff")?
        .map(|n| FieldFlags::from_bits(n as u32))
        .or(inherited.flags);
    let value = own_value(reader, &mut walk.values, dict, at, "V", &inherited.value)?;
    let default_value = own_value(
        reader,
        &mut walk.values,
        dict,
        at,
        "DV",
        &inherited.default_value,
    )?;
    // A choice field's options and selected indices, or a button field's
    // export values, found here and decoded when they're asked for
    // (`Field::options`, `Field::selected`); and the top index, a number.
    let options = own_entry(reader, dict, at, "Opt", inherited.options);
    let selected = own_entry(reader, dict, at, "I", inherited.selected);
    let top_index = reader
        .int_entry(dict, at, "TI")?
        .map(|n| u32::try_from(n).unwrap_or(0))
        .or(inherited.top_index);
    let da = reader.bytes_entry(dict, at, "DA")?.or(inherited.da);
    let quadding = reader
        .int_entry(dict, at, "Q")?
        .map(Quadding::from_pdf_int)
        .or(inherited.quadding);
    let held_max_len = reader.held_int_entry(dict, at, "MaxLen")?;
    let max_len_held = held_max_len.is_some() || inherited.max_len_held;
    let max_len = held_max_len
        .flatten()
        .and_then(|n| u32::try_from(n).ok())
        .or(inherited.max_len);

    let index = walk.fields.len();
    if let Some(oid) = id {
        walk.by_id.insert(oid, index);
    }
    let handed = Inherited {
        field_type,
        flags,
        value: value.clone(),
        default_value: default_value.clone(),
        options: options.entry,
        selected: selected.entry,
        top_index,
        da,
        quadding,
        max_len,
        max_len_held,
    };
    walk.handed.push(handed.clone());
    walk.depths.push(depth);
    walk.fields.push(Field {
        dict,
        id,
        parent: parent.map(|(index, _)| index),
        kids: Vec::new(),
        widgets: Vec::new(),
        name,
        field_type,
        kind: field_type.map(|ft| kind_of(ft, flags.unwrap_or_default(), reader.rules)),
        flags: flags.unwrap_or_default(),
        value,
        default_value,
        da,
        quadding,
        max_len,
        options,
        selected,
        top_index,
    });

    match kids {
        Some((entries, kids_within)) => {
            let mut mixed = false;
            if depth + 1 < reader.limits.depth {
                for raw in entries {
                    if is_field_kid(reader, &walk.seen, raw, kids_within)? {
                        mixed = true;
                        break;
                    }
                }
            }
            for raw in entries {
                match node(
                    reader,
                    walk,
                    raw,
                    kids_within,
                    Some((index, id)),
                    &handed,
                    depth + 1,
                    mixed,
                    false,
                )? {
                    Node::Field(kid) => walk.fields[index].kids.push(kid),
                    Node::Widget(mut widget) => {
                        widget.index = walk.fields[index].widgets.len();
                        walk.fields[index].widgets.push(widget);
                    }
                    Node::Skipped => {}
                }
            }
        }
        None => {
            if parent.is_none() && walk.fields[index].name.is_none() {
                reader.say(at, None, Fault::MissingName);
            }
            // A field that is its own widget: one dictionary holds both (or a
            // nameless widget kid promoted to a terminal field alongside field
            // siblings).
            if !pending_kid
                && (as_field
                    || dict.get("Subtype").and_then(Object::as_name) == Some(b"Widget")
                    || dict.get("Rect").is_some())
            {
                walk.fields[index].widgets.push(Widget {
                    dict,
                    id,
                    merged: true,
                    index: 0,
                });
            }
        }
    }
    // Under PDFium's rules a field with no `/MaxLen` of its own or an
    // ancestor's takes the first of its widgets' own, as PDFium as of
    // chromium/8152 reads a field's (`core/fpdfdoc/cpdf_formfield.cpp`,
    // `GetMaxLen`): the first widget whose dictionary has the key, whatever
    // it holds, and none where that's no number of 0 or more. An entry that
    // holds `null` is one, as PDFium's dictionaries keep it, so the widgets
    // aren't read below it; one that reaches no object isn't. ISO 32000-2
    // gives the entry to fields alone (§12.7.5.3). A field that is its own
    // widget has one dictionary, whose entry was read above: it isn't read
    // again, which would tell what is wrong with it a second time.
    if reader.rules == Rules::Pdfium && !max_len_held {
        let first = (walk.fields[index].widgets.iter())
            .find(|widget| !widget.merged && widget.dict.get("MaxLen").is_some())
            .map(|widget| (widget.dict, widget.id.or(at)));
        if let Some((widget, within)) = first {
            walk.fields[index].max_len = reader
                .int_entry(widget, within, "MaxLen")?
                .and_then(|n| u32::try_from(n).ok());
        }
    }
    if !pending_kid && walk.fields[index].is_terminal() && field_type.is_none() {
        reader.say(at, None, Fault::MissingFieldType);
    }
    Ok(Node::Field(index))
}

/// A field's `/V` or `/DV`: its own where the entry reaches a value, typed
/// (cached by `(ObjectId, key)` when indirect so shared array values across
/// many fields decode and report faults once);
/// `inherited` where it has none or the entry reaches nothing (`null`, a
/// reference to no object or to `null`, a damaged object, a chain too long),
/// as an entry that is null is one that's absent (ISO 32000-2, §7.3.9) and
/// as every other inherited entry is read; none for a value of no kind a
/// field holds, which is the field's own, wrong, and told.
fn own_value<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    values: &mut HashMap<(ObjectId, &'static str), Option<Arc<FieldValue<'o>>>>,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
    inherited: &Option<Arc<FieldValue<'o>>>,
) -> Result<Option<Arc<FieldValue<'o>>>, Error> {
    let Some(raw) = dict.get(key) else {
        return Ok(inherited.clone());
    };
    let Some((object, id)) = reader.reached(raw, within)? else {
        return Ok(inherited.clone());
    };
    if let Some(oid) = raw.as_reference().and(id) {
        if let Some(cached) = values.get(&(oid, key)) {
            return Ok(cached.clone());
        }
        let decoded = field_value(reader, raw, object, id, key)?.map(Arc::new);
        values.insert((oid, key), decoded.clone());
        return Ok(decoded);
    }
    Ok(field_value(reader, raw, object, id, key)?.map(Arc::new))
}

/// Types a `/V` or `/DV` that `raw` reaches as `object`, read within `id`:
/// `None` for a value of no kind a field holds.
fn field_value<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    raw: &'o Object,
    object: &'o Object,
    id: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<FieldValue<'o>>, Error> {
    Ok(Some(match object.value() {
        Value::String(Str::Bytes(bytes), _) => {
            FieldValue::Text(text::decode_str(bytes, reader.rules))
        }
        Value::String(Str::Encrypted(_), _) => return Err(locked_string(id, key)),
        Value::Name(_) => FieldValue::State(object.as_name().unwrap_or_default()),
        Value::Array(items) => {
            if items.len() > reader.limits.items {
                return Err(Error::Limit(Cap::Items));
            }
            let mut texts = Vec::with_capacity(items.len());
            for item in items {
                let Some((item, item_id)) = reader.reached(item, id)? else {
                    continue;
                };
                match item.value() {
                    Value::String(Str::Bytes(bytes), _) => {
                        texts.push(text::decode_str(bytes, reader.rules));
                    }
                    Value::String(Str::Encrypted(_), _) => {
                        return Err(locked_string(item_id, key));
                    }
                    _ => reader.say(item_id, item.at(), Fault::WrongType { key }),
                }
            }
            FieldValue::Items(texts)
        }
        Value::Dict(dict) => FieldValue::Dict(dict),
        Value::Stream(_) => {
            FieldValue::Stream(raw.as_reference().is_some().then_some(id).flatten())
        }
        _ => {
            reader.say(id, object.at(), Fault::WrongType { key });
            return Ok(None);
        }
    }))
}

/// A field's `/Opt` or `/I`, kept undecoded ([`Found`]): the object its own
/// entry reaches, with the object it was read within; `inherited` where it
/// has none or the entry reaches nothing, as [`own_value`] reads `/V`. ISO
/// 32000-2 marks a button field's `/Opt` inheritable (Table 230) and says
/// nothing of the kind for a choice field's `/Opt` and `/I` (Table 234);
/// both are read inherited, under either rules, as PDFium as of
/// chromium/8152 reads them.
///
/// The entry is followed telling nothing and failing for nothing, so a form
/// reads as it would without it: an entry that reaches no object is kept
/// to be followed again when it's asked for, which tells a damaged object
/// or a loop, and one that can't be read is kept, and handed down, to fail
/// then.
fn own_entry<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &Reader<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
    inherited: Option<EntryAt<'o>>,
) -> Found<'o> {
    let Some(raw) = dict.get(key) else {
        return Found {
            own: None,
            entry: inherited,
        };
    };
    let objects: &dyn Resolve<'o> = &reader.objects;
    let mut quiet = Reader {
        objects,
        rules: reader.rules,
        limits: reader.limits,
        sink: &mut (),
    };
    match quiet.reached(raw, within) {
        Ok(Some(reached)) => Found {
            own: None,
            entry: Some(reached),
        },
        Ok(None) => Found {
            own: Some((raw, within)),
            entry: inherited,
        },
        Err(_) => Found {
            own: None,
            entry: Some((raw, within)),
        },
    }
}
