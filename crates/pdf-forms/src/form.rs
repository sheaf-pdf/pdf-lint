//! The interactive form dictionary and the walk over its field tree.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use pdf_annots::Quadding;
use pdf_objects::fault::Sink;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Str, Value, text};

use crate::error::Error;
use crate::fault::Fault;
use crate::field::{Field, FieldFlags, FieldType, FieldValue, Widget, kind_of};
use crate::limits::{Cap, Limits};
use crate::phases::phase;
use crate::read::{DictAt, Reader, locked_string};

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

    /// The index of the first field whose fully qualified name is `name`.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<usize> {
        (0..self.fields.len()).find(|&i| self.full_name(i).as_deref() == Some(name))
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

    /// Whether the form holds an XFA form too (`/XFA`).
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
    da: Option<&'o [u8]>,
    quadding: Option<Quadding>,
    max_len: Option<u32>,
}

/// What an entry of `/Fields` or `/Kids` turned out to be.
enum Node<'o> {
    Field(usize),
    Widget(Widget<'o>),
    Skipped,
}

struct Walk<'o> {
    fields: Vec<Field<'o>>,
    seen: HashSet<ObjectId>,
    count: usize,
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
    let resources = reader.dict_entry(dict, id, "DR")?;
    let fonts = match resources {
        Some((dr, dr_id)) => reader.dict_entry(dr, dr_id.or(id), "Font")?,
        None => None,
    };

    let mut walk = Walk {
        fields: Vec::new(),
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
        for raw in entries {
            if let Node::Field(index) = node(reader, &mut walk, raw, within, None, &inherited, 0)? {
                roots.push(index);
            }
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

/// Reads one entry of `/Fields` or of a field's `/Kids`, and the tree under
/// it. `parent` is the field whose `/Kids` holds the entry, with its number.
fn node<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    walk: &mut Walk<'o>,
    raw: &'o Object,
    within: Option<ObjectId>,
    parent: Option<(usize, Option<ObjectId>)>,
    inherited: &Inherited<'o>,
    depth: usize,
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
    // widgets, not a field.
    if parent.is_some() && kids.is_none() && dict.get("T").is_none() {
        return Ok(Node::Widget(Widget {
            dict,
            id,
            merged: false,
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
    let value = match dict.get("V") {
        Some(raw) => field_value(reader, raw, at, "V")?.map(Arc::new),
        None => inherited.value.clone(),
    };
    let default_value = match dict.get("DV") {
        Some(raw) => field_value(reader, raw, at, "DV")?.map(Arc::new),
        None => inherited.default_value.clone(),
    };
    let da = reader.bytes_entry(dict, at, "DA")?.or(inherited.da);
    let quadding = reader
        .int_entry(dict, at, "Q")?
        .map(Quadding::from_pdf_int)
        .or(inherited.quadding);
    let max_len = reader
        .int_entry(dict, at, "MaxLen")?
        .and_then(|n| u32::try_from(n).ok())
        .or(inherited.max_len);

    let index = walk.fields.len();
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
        value: value.clone(),
        default_value: default_value.clone(),
        da,
        quadding,
        max_len,
    });

    match kids {
        Some((entries, kids_within)) => {
            let handed = Inherited {
                field_type,
                flags,
                value,
                default_value,
                da,
                quadding,
                max_len,
            };
            for raw in entries {
                match node(
                    reader,
                    walk,
                    raw,
                    kids_within,
                    Some((index, id)),
                    &handed,
                    depth + 1,
                )? {
                    Node::Field(kid) => walk.fields[index].kids.push(kid),
                    Node::Widget(widget) => walk.fields[index].widgets.push(widget),
                    Node::Skipped => {}
                }
            }
        }
        None => {
            if parent.is_none() && walk.fields[index].name.is_none() {
                reader.say(at, None, Fault::MissingName);
            }
            // A field that is its own widget: one dictionary holds both.
            if dict.get("Subtype").and_then(Object::as_name) == Some(b"Widget")
                || dict.get("Rect").is_some()
            {
                walk.fields[index].widgets.push(Widget {
                    dict,
                    id,
                    merged: true,
                });
            }
        }
    }
    if walk.fields[index].is_terminal() && field_type.is_none() {
        reader.say(at, None, Fault::MissingFieldType);
    }
    Ok(Node::Field(index))
}

/// Types a `/V` or `/DV`: `None` for `null` and for a value of no kind a field
/// holds.
fn field_value<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    raw: &'o Object,
    within: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<FieldValue<'o>>, Error> {
    let Some((object, id)) = reader.reached(raw, within)? else {
        return Ok(None);
    };
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
