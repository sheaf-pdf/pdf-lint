//! Fields, their flags, kinds, values, and widgets.

use std::fmt;
use std::sync::Arc;

use pdf_annots::{Annot, Quadding};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, ObjectId, Resolve};

use crate::error::Error;
use crate::fault::Fault;
use crate::limits::Limits;

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
/// that. [`dict`](Self::dict) has what the field itself holds.
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
    pub(crate) flags: FieldFlags,
    pub(crate) value: Option<Arc<FieldValue<'o>>>,
    pub(crate) default_value: Option<Arc<FieldValue<'o>>>,
    pub(crate) da: Option<&'o [u8]>,
    pub(crate) quadding: Option<Quadding>,
    pub(crate) max_len: Option<u32>,
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

    /// What the field is, from its type and flags. The push button flag wins
    /// over the radio flag, which means something only when it is clear.
    #[must_use]
    pub fn kind(&self) -> Option<FieldKind> {
        let flags = self.flags;
        Some(match self.field_type? {
            FieldType::Button if flags.contains(FieldFlags::PUSHBUTTON) => FieldKind::PushButton,
            FieldType::Button if flags.contains(FieldFlags::RADIO) => FieldKind::Radio,
            FieldType::Button => FieldKind::CheckBox,
            FieldType::Text => FieldKind::Text,
            FieldType::Choice if flags.contains(FieldFlags::COMBO) => FieldKind::Combo,
            FieldType::Choice => FieldKind::List,
            FieldType::Signature => FieldKind::Signature,
        })
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

    /// The most characters a text field takes (`/MaxLen`).
    #[must_use]
    pub const fn max_len(&self) -> Option<u32> {
        self.max_len
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
}

impl fmt::Debug for Widget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Widget")
            .field("id", &self.id)
            .field("merged", &self.merged)
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
