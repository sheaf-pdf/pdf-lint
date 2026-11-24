//! Fields, their flags, kinds, values, and widgets.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use pdf_annots::{Annot, AppearanceEntry, Appearances, Quadding};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Str, Value, text};

use crate::error::Error;
use crate::fault::Fault;
use crate::group::{FieldGroup, Group, GroupKind, GroupWidget};
use crate::limits::{Cap, Limits};
use crate::read::{EntryAt, Reader, locked_string};

/// What a field of `field_type` with `flags` is, under `rules`.
pub(crate) fn kind_of(field_type: FieldType, flags: FieldFlags, rules: Rules) -> FieldKind {
    let (push, radio) = (
        flags.contains(FieldFlags::PUSHBUTTON),
        flags.contains(FieldFlags::RADIO),
    );
    match field_type {
        FieldType::Button if radio && (rules == Rules::Pdfium || !push) => FieldKind::Radio,
        FieldType::Button if push => FieldKind::PushButton,
        FieldType::Button => FieldKind::CheckBox,
        FieldType::Text => FieldKind::Text,
        FieldType::Choice if flags.contains(FieldFlags::COMBO) => FieldKind::Combo,
        FieldType::Choice => FieldKind::List,
        FieldType::Signature => FieldKind::Signature,
    }
}

/// A field's type, its `/FT`, the field's own or inherited (ISO 32000-2
/// §12.7.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FieldType {
    /// `/Btn`: a push button, check box, or radio button.
    Button,
    /// `/Tx`: a text field.
    Text,
    /// `/Ch`: a list box or combo box.
    Choice,
    /// `/Sig`: a signature field.
    Signature,
}

impl FieldType {
    /// The type an `/FT` name gives, or `None` for any other name.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"Btn" => Some(Self::Button),
            b"Tx" => Some(Self::Text),
            b"Ch" => Some(Self::Choice),
            b"Sig" => Some(Self::Signature),
            _ => None,
        }
    }

    /// The `/FT` name of this type.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Button => "Btn",
            Self::Text => "Tx",
            Self::Choice => "Ch",
            Self::Signature => "Sig",
        }
    }
}

/// What a field is to its user: its [`FieldType`] told apart by its
/// [`FieldFlags`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FieldKind {
    /// A button that keeps no value ([`FieldFlags::PUSHBUTTON`]).
    PushButton,
    /// A button that is on or off.
    CheckBox,
    /// One of a set of buttons of which at most one is on
    /// ([`FieldFlags::RADIO`]).
    Radio,
    /// A text field.
    Text,
    /// A choice field drawn as a drop-down list ([`FieldFlags::COMBO`]).
    Combo,
    /// A choice field drawn as a scrolling list.
    List,
    /// A signature field.
    Signature,
}

/// A field's `/Ff` flags, the field's own or inherited (ISO 32000-2 §12.7.4).
///
/// Which flags mean anything depends on the field's [`FieldType`]: bit 26 is
/// [`RADIOS_IN_UNISON`](Self::RADIOS_IN_UNISON) on a button and
/// [`RICH_TEXT`](Self::RICH_TEXT) on a text field.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FieldFlags(u32);

impl FieldFlags {
    /// Bit 1: the user may not change the field's value.
    pub const READ_ONLY: Self = Self(1);
    /// Bit 2: the field must have a value when the form is submitted.
    pub const REQUIRED: Self = Self(1 << 1);
    /// Bit 3: the field is not exported when the form is submitted.
    pub const NO_EXPORT: Self = Self(1 << 2);
    /// Bit 13, text: the field holds more than one line.
    pub const MULTILINE: Self = Self(1 << 12);
    /// Bit 14, text: the value is a password, not to be shown or kept.
    pub const PASSWORD: Self = Self(1 << 13);
    /// Bit 15, radio: one button is on at all times.
    pub const NO_TOGGLE_TO_OFF: Self = Self(1 << 14);
    /// Bit 16, button: the field is a set of radio buttons.
    pub const RADIO: Self = Self(1 << 15);
    /// Bit 17, button: the field is a push button.
    pub const PUSHBUTTON: Self = Self(1 << 16);
    /// Bit 18, choice: the field is a combo box.
    pub const COMBO: Self = Self(1 << 17);
    /// Bit 19, combo box: the user may type a value.
    pub const EDIT: Self = Self(1 << 18);
    /// Bit 20, choice: the options are sorted.
    pub const SORT: Self = Self(1 << 19);
    /// Bit 21, text: the value is the path of a file to submit.
    pub const FILE_SELECT: Self = Self(1 << 20);
    /// Bit 22, choice: more than one option may be selected.
    pub const MULTI_SELECT: Self = Self(1 << 21);
    /// Bit 23, text and choice: the text is not spell-checked.
    pub const DO_NOT_SPELL_CHECK: Self = Self(1 << 22);
    /// Bit 24, text: the field doesn't scroll past its rectangle.
    pub const DO_NOT_SCROLL: Self = Self(1 << 23);
    /// Bit 25, text: the field is `/MaxLen` equal cells, one character each.
    pub const COMB: Self = Self(1 << 24);
    /// Bit 26, radio: buttons with the same on state turn on and off together.
    pub const RADIOS_IN_UNISON: Self = Self(1 << 25);
    /// Bit 26, text: the value is rich text.
    pub const RICH_TEXT: Self = Self(1 << 25);
    /// Bit 27, choice: a selection is committed as soon as it is made.
    pub const COMMIT_ON_SEL_CHANGE: Self = Self(1 << 26);

    /// The flags `/Ff` holds; the bits above the 32nd are dropped.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The flags as `/Ff` holds them.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every flag of `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for FieldFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl fmt::Debug for FieldFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FieldFlags({:#x})", self.0)
    }
}

/// A field's value (`/V`) or default value (`/DV`), as typed.
///
/// `Debug` prints the variant and a length, never the text.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum FieldValue<'o> {
    /// A text string: a text field's text, or a choice field's one selected
    /// option.
    Text(String),
    /// A name: the on state of a check box or of a radio button set, or `Off`.
    State(&'o [u8]),
    /// An array of text strings: a list box's selected options.
    Items(Vec<String>),
    /// A dictionary: a signature field's signature.
    Dict(&'o Dict),
    /// A stream, whose data is a text field's text; `None` when it wasn't
    /// reached by reference.
    Stream(Option<ObjectId>),
}

impl fmt::Debug for FieldValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => write!(f, "Text({} bytes)", text.len()),
            Self::State(name) => write!(f, "State({} bytes)", name.len()),
            Self::Items(items) => write!(f, "Items({})", items.len()),
            Self::Dict(dict) => write!(f, "Dict({} entries)", dict.len()),
            Self::Stream(id) => f.debug_tuple("Stream").field(id).finish(),
        }
    }
}

/// One node of the field tree: a terminal field, or a field whose `/Kids` are
/// fields.
///
/// The type, flags, value, default value, default appearance, quadding, and
/// maximum length are the effective ones: the field's own, else the nearest
/// ancestor's, and for the default appearance and quadding the form's after
/// that; under PDFium's rules, a maximum length neither gives is the first
/// of the field's widgets' to hold one ([`max_len`](Self::max_len)).
/// [`dict`](Self::dict) has what the field itself holds.
///
/// `Debug` prints numbers and counts, never the field's name or value.
#[derive(Clone)]
pub struct Field<'o> {
    pub(crate) dict: &'o Dict,
    pub(crate) id: Option<ObjectId>,
    pub(crate) parent: Option<usize>,
    pub(crate) kids: Vec<usize>,
    pub(crate) widgets: Vec<Widget<'o>>,
    pub(crate) name: Option<String>,
    pub(crate) field_type: Option<FieldType>,
    pub(crate) kind: Option<FieldKind>,
    pub(crate) flags: FieldFlags,
    pub(crate) value: Option<Arc<FieldValue<'o>>>,
    pub(crate) default_value: Option<Arc<FieldValue<'o>>>,
    pub(crate) da: Option<&'o [u8]>,
    pub(crate) quadding: Option<Quadding>,
    pub(crate) max_len: Option<u32>,
    pub(crate) options: Found<'o>,
    pub(crate) selected: Found<'o>,
    pub(crate) top_index: Option<u32>,
}

impl<'o> Field<'o> {
    /// The field's dictionary.
    #[must_use]
    pub const fn dict(&self) -> &'o Dict {
        self.dict
    }

    /// The field's object number, or `None` for a field written as a direct
    /// dictionary.
    #[must_use]
    pub const fn id(&self) -> Option<ObjectId> {
        self.id
    }

    /// The index in [`Form::fields`](crate::Form::fields) of the field this one
    /// is a kid of, or `None` at the top of the tree.
    #[must_use]
    pub const fn parent(&self) -> Option<usize> {
        self.parent
    }

    /// The indexes in [`Form::fields`](crate::Form::fields) of the kids that are
    /// fields, in `/Kids` order.
    #[must_use]
    pub fn kids(&self) -> &[usize] {
        &self.kids
    }

    /// Whether the field is terminal: none of its kids is a field.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.kids.is_empty()
    }

    /// The field's widgets, in `/Kids` order: the kids that are widgets, or the
    /// field's own dictionary when it is a widget too.
    #[must_use]
    pub fn widgets(&self) -> &[Widget<'o>] {
        &self.widgets
    }

    /// The field's partial name (`/T`), its own.
    #[must_use]
    pub fn partial_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The field's type (`/FT`).
    #[must_use]
    pub const fn field_type(&self) -> Option<FieldType> {
        self.field_type
    }

    /// The field's flags (`/Ff`); none when no `/Ff` is written.
    #[must_use]
    pub const fn flags(&self) -> FieldFlags {
        self.flags
    }

    /// What the field is, from its type and flags. A button with both the
    /// push button and the radio flags is a push button under ISO 32000-2's
    /// rules, where the radio flag means something only when the push button
    /// flag is clear, and a radio button under PDFium's as of chromium/8152.
    #[must_use]
    pub const fn kind(&self) -> Option<FieldKind> {
        self.kind
    }

    /// The field's value (`/V`).
    #[must_use]
    pub fn value(&self) -> Option<&FieldValue<'o>> {
        self.value.as_deref()
    }

    /// The field's default value (`/DV`).
    #[must_use]
    pub fn default_value(&self) -> Option<&FieldValue<'o>> {
        self.default_value.as_deref()
    }

    /// The bytes of the default appearance string (`/DA`) the field's text is
    /// drawn with. A widget's own `/DA` comes before it.
    #[must_use]
    pub const fn default_appearance(&self) -> Option<&'o [u8]> {
        self.da
    }

    /// How the field's text is justified (`/Q`); left when none is written.
    #[must_use]
    pub fn quadding(&self) -> Quadding {
        self.quadding.unwrap_or_default()
    }

    /// The most characters a text field takes (`/MaxLen`): the field's own,
    /// else the nearest ancestor's.
    ///
    /// Under [`Rules::Iso`] that's all: the entry is a field's (ISO 32000-2,
    /// §12.7.5.3), and a widget's own is no field's. Under [`Rules::Pdfium`]
    /// a field with no `/MaxLen` of its own or an ancestor's takes the first
    /// of its widgets' own, as PDFium as of chromium/8152 reads it: the first
    /// widget, in `/Kids` order, whose dictionary has the entry, whatever it
    /// holds, so a `0` there, or no number, gives no limit though a later
    /// widget has one. A `/MaxLen` the field or an ancestor holds keeps the
    /// widgets unread though it's no number or `null`, as PDFium finds it;
    /// one that's a reference to no object is none.
    #[must_use]
    pub const fn max_len(&self) -> Option<u32> {
        self.max_len
    }
}

/// A choice field's options and selection, and a button field's export
/// values and group, decoded when they're asked for: `Form::read` finds the
/// entries and decodes none of them, so a caller that reads a form for its
/// names and values pays nothing for options it doesn't ask for. What's
/// wrong in `/Opt` or `/I` is the call's too: an entry that can't be read
/// (locked, past a reader's cap, or not given by its source) fails it, and
/// one that reaches a damaged object or a reference loop is told by it, so
/// `Form::read` reads such a form as it would one without the entry.
///
/// `/TI`, a number, is read with the field, as `/Q` and `/MaxLen` are, so
/// [`top_index`](Self::top_index) takes no objects, and one the reader
/// can't read fails `Form::read` as theirs does.
///
/// `/Opt`, `/I`, and `/TI` are the field's own, else the nearest
/// ancestor's, under either rules: ISO 32000-2 marks a button field's `/Opt`
/// inheritable (Table 230) and says nothing of the kind for a choice field's
/// three (Table 234), and PDFium as of chromium/8152 inherits all of them.
/// [`dict`](Self::dict) has what the field itself holds.
impl<'o> Field<'o> {
    /// The field's options (`/Opt`): a choice field's, each an export value
    /// and the label shown for it, or a check box or radio button field's
    /// export values, one for each widget by its index (ISO 32000-2, Tables
    /// 230 and 234). None where there's no `/Opt`.
    ///
    /// A text string is an export value that is its own label, and an array
    /// of two text strings an export value and its label. An item that is
    /// neither keeps its place as an option of no text, since a button's
    /// entries go by its widgets' indexes, and is told as
    /// [`Fault::WrongType`] by [`options_with`](Self::options_with); an
    /// `/Opt` that is no array is no options, told the same way.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Limit`] ([`Cap::Items`]) when `/Opt` has more items
    /// than [`Limits::items`], or [`Error::Unreadable`] on locked, I/O, or
    /// reader-cap failures.
    pub fn options<R: Resolve<'o>>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Vec<FieldOption>, Error> {
        self.options_with(objects, rules, limits, &mut ())
    }

    /// The field's options, as [`options`](Self::options) reads them,
    /// reporting recoverable departures to `sink`.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::options`].
    pub fn options_with<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Vec<FieldOption>, Error> {
        let mut reader = Reader {
            objects,
            rules,
            limits,
            sink,
        };
        let entry = reach(&mut reader, self.options)?;
        read_options(&mut reader, entry)
    }

    /// How many items the field's `/Opt` array has, each decoded or not;
    /// none where there's no `/Opt`, or it's no array. Nothing is decoded,
    /// counted against [`Limits::items`], or told: what's wrong in `/Opt` is
    /// [`options_with`](Self::options_with)'s to tell. A check box or radio
    /// button field with an `/Opt` array, even an empty one, names the on
    /// states it has no name for by its widgets' indexes
    /// ([`Group::has_opt`](crate::group::Group::has_opt)), which this tells
    /// apart from a field with no `/Opt`, as an empty
    /// [`options`](Self::options) doesn't.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unreadable`] on locked, I/O, or reader-cap failures.
    pub fn option_count<R: Resolve<'o>>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Option<usize>, Error> {
        let mut reader = Reader {
            objects,
            rules,
            limits,
            sink: &mut (),
        };
        let entry = reach(&mut reader, self.options)?;
        Ok(entry.and_then(|(object, _)| object.as_array().map(<[Object]>::len)))
    }

    /// The index in [`options`](Self::options) of the first option a list
    /// box shows at its top (`/TI`); 0 where there is none, or it's less
    /// than 0. Unlike `/Opt` and `/I`, the entry is read with the field, as
    /// `/Q` and `/MaxLen` are.
    #[must_use]
    pub fn top_index(&self) -> usize {
        usize::try_from(self.top_index.unwrap_or(0)).unwrap_or(usize::MAX)
    }

    /// The indexes in [`options`](Self::options) of the selected options as
    /// `/I` gives them, in its order; none where there's no `/I`. An item
    /// that is no index of 0 or more is left out, and a number in place of
    /// the array is taken as its one item, each told as
    /// [`Fault::WrongType`] by [`selected_with`](Self::selected_with); under
    /// [`Rules::Pdfium`] a real is taken too, cut to an integer, as PDFium
    /// reads it. What a field selects is [`selection`](Self::selection).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Limit`] ([`Cap::Items`]) when `/I` has more items
    /// than [`Limits::items`], or [`Error::Unreadable`] on locked, I/O, or
    /// reader-cap failures.
    pub fn selected<R: Resolve<'o>>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Vec<usize>, Error> {
        self.selected_with(objects, rules, limits, &mut ())
    }

    /// The selected indexes, as [`selected`](Self::selected) reads them,
    /// reporting recoverable departures to `sink`.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::selected`].
    pub fn selected_with<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Vec<usize>, Error> {
        let mut reader = Reader {
            objects,
            rules,
            limits,
            sink,
        };
        let entry = reach(&mut reader, self.selected)?;
        read_selected(&mut reader, entry)
    }

    /// The indexes in [`options`](Self::options) of the options the field's
    /// value selects, by [`selected_options`] over its options, `/I`, and
    /// `/V`'s text strings (a value that is no text string, or array of
    /// them, is none, and selects nothing).
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::options`] and [`Self::selected`].
    pub fn selection<R: Resolve<'o>>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Vec<usize>, Error> {
        self.selection_with(objects, rules, limits, &mut ())
    }

    /// The options the field's value selects, as
    /// [`selection`](Self::selection) finds them, reporting recoverable
    /// departures to `sink`.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::selection`].
    pub fn selection_with<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Vec<usize>, Error> {
        let mut reader = Reader {
            objects,
            rules,
            limits,
            sink,
        };
        let entry = reach(&mut reader, self.options)?;
        let options = read_options(&mut reader, entry)?;
        let entry = reach(&mut reader, self.selected)?;
        let indices = read_selected(&mut reader, entry)?;
        let values: Vec<&str> = match self.value() {
            Some(FieldValue::Text(text)) => vec![text.as_str()],
            Some(FieldValue::Items(items)) => items.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        };
        Ok(selected_options(&options, &indices, &values))
    }

    /// The field's widgets as a check box or radio button group, for
    /// [`crate::group`]'s rules: its kind, its NoToggleToOff and
    /// RadiosInUnison flags, each widget's on state ([`Widget::states`])
    /// and export value (its entry in [`options`](Self::options)), and each
    /// widget's state as its `/AS` has it. None for a field that is neither
    /// a check box nor a radio button field: a text or choice field, a push
    /// button, or a field with no type.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::options`] and [`Widget::states`].
    pub fn group<R: Resolve<'o>>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Option<FieldGroup>, Error> {
        self.group_with(objects, rules, limits, &mut ())
    }

    /// The field's widgets as a group, as [`group`](Self::group) reads them,
    /// reporting recoverable departures to `sink`.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::group`].
    pub fn group_with<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Option<FieldGroup>, Error> {
        let kind = match self.kind {
            Some(FieldKind::CheckBox) => GroupKind::CheckBox,
            Some(FieldKind::Radio) => GroupKind::Radio,
            _ => return Ok(None),
        };
        let objects: &dyn Resolve<'o> = &objects;
        let (has_opt, options) = {
            let mut reader = Reader {
                objects,
                rules,
                limits,
                sink: &mut *sink,
            };
            let entry = reach(&mut reader, self.options)?;
            let has_opt = entry.is_some_and(|(object, _)| object.as_array().is_some());
            (has_opt, read_options(&mut reader, entry)?)
        };
        let mut widgets = Vec::with_capacity(self.widgets.len());
        let mut shown = Vec::with_capacity(self.widgets.len());
        for (index, widget) in self.widgets.iter().enumerate() {
            let states = widget.states_with(objects, rules, limits, &mut *sink)?;
            let mut member = GroupWidget::new(None);
            member.on_state = states.on_state.map(|name| Cow::Owned(name.into_vec()));
            member.export = (options.get(index)).map(|option| Cow::Owned(option.export.clone()));
            widgets.push(member);
            shown.push(states.state);
        }
        let mut group = Group::new(kind, &widgets);
        group.no_toggle_to_off = self.flags.contains(FieldFlags::NO_TOGGLE_TO_OFF);
        group.in_unison = self.flags.contains(FieldFlags::RADIOS_IN_UNISON);
        group.has_opt = has_opt;
        let states = (shown.iter().enumerate())
            .map(|(index, state)| state.as_deref() == group.on_state(index).as_deref())
            .collect();
        let (no_toggle_to_off, in_unison) = (group.no_toggle_to_off, group.in_unison);
        Ok(Some(FieldGroup::new(
            kind,
            no_toggle_to_off,
            in_unison,
            has_opt,
            widgets,
            states,
        )))
    }
}

impl fmt::Debug for Field<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Field")
            .field("id", &self.id)
            .field("parent", &self.parent)
            .field("kids", &self.kids.len())
            .field("widgets", &self.widgets.len())
            .field("named", &self.name.is_some())
            .field("field_type", &self.field_type)
            .field("flags", &self.flags)
            .field("value", &self.value)
            .field("default_value", &self.default_value)
            .field("max_len", &self.max_len)
            .finish_non_exhaustive()
    }
}

/// A widget annotation of a field: one place on a page where the field is
/// shown.
#[derive(Clone, Copy)]
pub struct Widget<'o> {
    pub(crate) dict: &'o Dict,
    pub(crate) id: Option<ObjectId>,
    pub(crate) merged: bool,
    pub(crate) index: usize,
}

impl<'o> Widget<'o> {
    /// The widget's dictionary: the field's own when
    /// [`is_merged`](Self::is_merged).
    #[must_use]
    pub const fn dict(&self) -> &'o Dict {
        self.dict
    }

    /// The widget's object number, or `None` for one written as a direct
    /// dictionary.
    #[must_use]
    pub const fn id(&self) -> Option<ObjectId> {
        self.id
    }

    /// Whether the widget and its field are one dictionary.
    #[must_use]
    pub const fn is_merged(&self) -> bool {
        self.merged
    }

    /// The widget's index among its field's widgets
    /// ([`Field::widgets`]): the one a check box's or radio button's export
    /// value in `/Opt`, and its state in [`crate::group`]'s rules, go by.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Reads the widget as an annotation: its rectangle, flags, border,
    /// appearance characteristics (`/MK`), and appearances.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unreadable`] on locked, I/O, or reader-cap failures, or
    /// [`Error::Limit`] when a cap of [`Limits::annots`] is exceeded.
    pub fn annot<R: Resolve<'o>>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Annot<'o>, Error> {
        self.annot_with(objects, rules, limits, &mut ())
    }

    /// Reads the widget as an annotation, reporting recoverable departures to
    /// `sink` as [`Fault::Annot`].
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::annot`].
    pub fn annot_with<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Annot<'o>, Error> {
        let mut lifted = Lift(sink);
        Ok(Annot::from_dict_with(
            objects,
            self.dict,
            self.id,
            rules,
            &limits.annots,
            &mut lifted,
        )?)
    }

    /// Reads the widget's appearance states: the name of its normal
    /// appearance's on state, its `/AS`, and whether it has an `Off`
    /// appearance. Only `/AP` and `/AS` are read
    /// ([`pdf_annots::Appearances::read`]), so a cap on the widget's actions
    /// or destination can't cost a caller its states.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unreadable`] on locked, I/O, or reader-cap failures.
    pub fn states<R: Resolve<'o>>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
    ) -> Result<WidgetStates, Error> {
        self.states_with(objects, rules, limits, &mut ())
    }

    /// Reads the widget's appearance states, reporting recoverable
    /// departures in `/AP` and `/AS` to `sink` as [`Fault::Annot`].
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Self::states`].
    pub fn states_with<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        &self,
        objects: R,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<WidgetStates, Error> {
        let mut lifted = Lift(sink);
        let appearances = Appearances::read_with(
            objects,
            self.dict,
            self.id,
            rules,
            &limits.annots,
            &mut lifted,
        )?;
        Ok(WidgetStates::of(appearances, rules))
    }
}

impl fmt::Debug for Widget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Widget")
            .field("id", &self.id)
            .field("merged", &self.merged)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

/// Passes `pdf-annots`' faults on as this crate's.
struct Lift<'s, S: ?Sized>(&'s mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_annots::Fault> for Lift<'_, S> {
    fn fault(&mut self, at: Place, fault: pdf_annots::Fault) {
        self.0.fault(at, Fault::Annot(fault));
    }

    fn wants(&self) -> bool {
        self.0.wants()
    }
}

/// One of a field's options (`/Opt`): the export value, which a choice
/// field's value holds when the option is selected, and the label shown for
/// it where that is another text (ISO 32000-2, Table 234). A check box's or
/// radio button's export value has no label (Table 230).
///
/// Built from outside the crate with [`FieldOption::new`]. `Debug` prints
/// lengths, never the text.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldOption {
    /// The export value.
    pub export: String,
    /// The label shown, where the option gives one apart from its export
    /// value.
    pub label: Option<String>,
}

impl FieldOption {
    /// An option whose export value is its label.
    #[must_use]
    pub fn new(export: impl Into<String>) -> Self {
        Self {
            export: export.into(),
            label: None,
        }
    }

    /// The text shown for the option: its label, else its export value.
    #[must_use]
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.export)
    }
}

impl fmt::Debug for FieldOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldOption")
            .field("export", &format_args!("{} bytes", self.export.len()))
            .field("label", &self.label.as_ref().map(String::len))
            .finish()
    }
}

/// A widget's appearance states ([`Widget::states`]): what names a check
/// box's or radio button's on state, and the state it's in.
///
/// `Debug` prints whether each is there, never a name.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WidgetStates {
    /// The name of the on state of the widget's normal appearance (`/AP
    /// /N`): of its state dictionary's names with a stream, the first that
    /// isn't `Off` or empty, or under [`Rules::Pdfium`] the least by its
    /// bytes, as PDFium as of chromium/8152 keeps a dictionary's names in
    /// order. None where `/N` is a single stream or there is none.
    pub on_state: Option<Box<[u8]>>,
    /// The widget's appearance state (`/AS`).
    pub state: Option<Box<[u8]>>,
    /// Whether the normal appearance has an `Off` state. ISO 32000-2,
    /// §12.7.5.2.3 makes it optional.
    pub has_off: bool,
}

impl WidgetStates {
    pub(crate) fn of(appearances: Option<Appearances>, rules: Rules) -> Self {
        let Some(appearances) = appearances else {
            return Self::default();
        };
        let names: &[(Box<[u8]>, ObjectId)] = match &appearances.normal {
            Some(AppearanceEntry::States(states)) => states,
            _ => &[],
        };
        let mut on = (names.iter())
            .map(|(name, _)| name)
            .filter(|name| !name.is_empty() && ***name != *b"Off");
        let on_state = match rules {
            Rules::Pdfium => on.min(),
            _ => on.next(),
        };
        Self {
            on_state: on_state.cloned(),
            state: appearances.state,
            has_off: names.iter().any(|(name, _)| **name == *b"Off"),
        }
    }

    /// Whether the widget is on: its `/AS` is the name of its on state.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.on_state.is_some() && self.state == self.on_state
    }
}

impl fmt::Debug for WidgetStates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetStates")
            .field("has_on_state", &self.on_state.is_some())
            .field("has_state", &self.state.is_some())
            .field("is_on", &self.is_on())
            .field("has_off", &self.has_off)
            .finish()
    }
}

/// The indexes in `options` of the options a choice field's value selects,
/// as PDFium as of chromium/8152 finds them, sorted and each once: `indices`
/// (`/I`) where they agree with `values` (`/V`'s text strings), else every
/// option whose export value is one of `values`. They agree where there are
/// as many of each, each index is of an option, and those options' export
/// values are `values`, each as many times as it's given; ISO 32000-2, Table
/// 234 has `/V` used where the two differ.
///
/// With no value (`values` empty) nothing is selected, whatever `indices`
/// has: `/V` is the field's value, and `/I` only tells apart the options
/// it selects (Table 234). The appearance PDFium makes for a list box with
/// an `/I` array and no `/V` highlights no option of a text of its own
/// either. (It reads the indexes in the value's place there, so a number
/// written in place of the array selects the option at it, and an array's
/// numbers, read as empty text, an option whose export value is empty:
/// neither is followed here.)
#[must_use]
pub fn selected_options(options: &[FieldOption], indices: &[usize], values: &[&str]) -> Vec<usize> {
    if values.is_empty() {
        return Vec::new();
    }
    let of_options = |&index: &usize| Some(index).filter(|&i| i < options.len());
    let agree = || {
        if indices.len() != values.len() || indices.iter().any(|i| of_options(i).is_none()) {
            return false;
        }
        let mut given: Vec<&str> = values.to_vec();
        let mut found: Vec<&str> = (indices.iter().filter_map(of_options))
            .map(|i| options[i].export.as_str())
            .collect();
        given.sort_unstable();
        found.sort_unstable();
        given == found
    };
    let mut selected: Vec<usize> = if agree() {
        indices.iter().filter_map(of_options).collect()
    } else {
        let mut given: Vec<&str> = values.to_vec();
        given.sort_unstable();
        (0..options.len())
            .filter(|&i| given.binary_search(&options[i].export.as_str()).is_ok())
            .collect()
    };
    selected.sort_unstable();
    selected.dedup();
    selected
}

/// A field's `/Opt` or `/I` as the walk finds it, followed again and
/// decoded when it's asked for.
#[derive(Clone, Copy, Default)]
pub(crate) struct Found<'o> {
    /// The field's own entry, with the object it's in, where the walk's
    /// following it reached no object (`null`, a reference to none, a
    /// damaged object, a loop): followed again when it's asked for, which
    /// tells what stopped it.
    pub(crate) own: Option<EntryAt<'o>>,
    /// The entry the field has otherwise, as its kids inherit it: the object
    /// its own entry reaches, else the nearest ancestor's, with the object
    /// it was read within; or an entry the walk couldn't follow (locked,
    /// past a reader's cap, or not given by its source), its own or an
    /// ancestor's, which fails when it's asked for.
    pub(crate) entry: Option<EntryAt<'o>>,
}

/// The object a field's `/Opt` or `/I` reaches, `found` as the walk found
/// it, with the object it was read within: what its own entry reaches,
/// telling a damaged object or a loop where that's none, else the entry it
/// has otherwise.
fn reach<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    found: Found<'o>,
) -> Result<Option<EntryAt<'o>>, Error> {
    if let Some((raw, within)) = found.own
        && let Some(reached) = reader.reached(raw, within)?
    {
        return Ok(Some(reached));
    }
    match found.entry {
        Some((entry, within)) => reader.reached(entry, within),
        None => Ok(None),
    }
}

/// Decodes a field's `/Opt`, `entry` the object it reaches.
fn read_options<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    entry: Option<EntryAt<'o>>,
) -> Result<Vec<FieldOption>, Error> {
    let Some((object, within)) = entry else {
        return Ok(Vec::new());
    };
    let Some(items) = object.as_array() else {
        reader.say(within, object.at(), Fault::WrongType { key: "Opt" });
        return Ok(Vec::new());
    };
    if items.len() > reader.limits.items {
        return Err(Error::Limit(Cap::Items));
    }
    let mut options = Vec::with_capacity(items.len());
    for raw in items {
        let (option, at) = match reader.reached(raw, within)? {
            Some((item, id)) => {
                let option = match item.value() {
                    Value::String(..) => (opt_text(item, id)?)
                        .map(|bytes| FieldOption::new(text::decode_str(bytes, reader.rules))),
                    Value::Array(pair) => opt_pair(reader, pair, id)?,
                    _ => None,
                };
                (option, (id, item.at()))
            }
            None => (None, (within, raw.at())),
        };
        options.push(option.unwrap_or_else(|| {
            reader.say(at.0, at.1, Fault::WrongType { key: "Opt" });
            FieldOption::default()
        }));
    }
    Ok(options)
}

/// An `/Opt` item that is an array of two text strings, an export value and
/// its label; none for any other array.
fn opt_pair<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    pair: &'o [Object],
    within: Option<ObjectId>,
) -> Result<Option<FieldOption>, Error> {
    let [export, label] = pair else {
        return Ok(None);
    };
    let mut text = |raw: &'o Object| match reader.reached(raw, within)? {
        Some((object, id)) => opt_text(object, id),
        None => Ok(None),
    };
    let (Some(export), Some(label)) = (text(export)?, text(label)?) else {
        return Ok(None);
    };
    Ok(Some(FieldOption {
        export: text::decode_str(export, reader.rules),
        label: Some(text::decode_str(label, reader.rules)),
    }))
}

/// The bytes of an `/Opt` text string, `id` the object it was read within;
/// none for an object of another type.
fn opt_text(object: &Object, id: Option<ObjectId>) -> Result<Option<&[u8]>, Error> {
    match object.value() {
        Value::String(Str::Bytes(bytes), _) => Ok(Some(bytes.as_slice())),
        Value::String(Str::Encrypted(_), _) => Err(locked_string(id, "Opt")),
        _ => Ok(None),
    }
}

/// Decodes a field's `/I`, `entry` the object it reaches.
fn read_selected<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    entry: Option<EntryAt<'o>>,
) -> Result<Vec<usize>, Error> {
    let Some((object, within)) = entry else {
        return Ok(Vec::new());
    };
    let Some(items) = object.as_array() else {
        reader.say(within, object.at(), Fault::WrongType { key: "I" });
        return Ok(index_of(reader, object).into_iter().collect());
    };
    if items.len() > reader.limits.items {
        return Err(Error::Limit(Cap::Items));
    }
    let mut indices = Vec::with_capacity(items.len());
    for raw in items {
        let index = match reader.reached(raw, within)? {
            Some((item, id)) => {
                let index = index_of(reader, item);
                if index.is_none_or(|_| item.as_integer().is_none()) {
                    reader.say(id, item.at(), Fault::WrongType { key: "I" });
                }
                index
            }
            None => {
                reader.say(within, raw.at(), Fault::WrongType { key: "I" });
                None
            }
        };
        indices.extend(index);
    }
    Ok(indices)
}

/// An index of 0 or more that `object` gives: an integer, or under
/// [`Rules::Pdfium`] a finite real cut to an integer, as PDFium reads one.
fn index_of<R, S: ?Sized>(reader: &Reader<'_, R, S>, object: &Object) -> Option<usize> {
    let n = match object.as_integer() {
        Some(n) => n,
        None => match object.as_number() {
            Some(v) if reader.rules == Rules::Pdfium && v.is_finite() => v as i64,
            _ => return None,
        },
    };
    usize::try_from(n).ok()
}
