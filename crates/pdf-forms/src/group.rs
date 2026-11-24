//! The rules a check box or radio button field's widgets follow when one of
//! them is pressed or the field's value is set (ISO 32000-2, §12.7.5.2.3 and
//! §12.7.5.2.4): which widgets' `/AS` become their on states and which `Off`,
//! and what the field's `/V` becomes.
//!
//! The rules are pure functions of plain data, a [`Group`] of
//! [`GroupWidget`]s: nothing is read, and they take no `Rules`. A caller with
//! a reader of its own fills them from its own model; one on this crate's
//! read layer takes [`Field::group`](crate::Field::group). A caller that saves
//! a field's value sets each widget's `/AS` from [`Group::state_name`] and the
//! field's `/V` from [`Group::value`], after [`Group::press`] for a widget
//! pressed, [`Group::states_for`] for a value given as a state's name, or
//! [`Group::states_for_export`] for one given as an export value; a caller
//! that makes a button's appearance names its on state by [`on_state_name`].
//!
//! - **A widget's on state** is the name its normal appearance (`/AP /N`) has
//!   for on, where it has one; else, in a field with `/Opt`
//!   ([`Group::has_opt`]), its index among the field's widgets as a decimal
//!   (ISO 32000-2, Table 230), and else `Yes`, the names PDFium as of
//!   chromium/8152 gives a state it makes.
//! - **A widget's export value** is its `/Opt` entry. In a field with
//!   `/Opt`, a widget whose entry is empty, or that `/Opt` ends before, has
//!   `Yes`, as PDFium as of chromium/8152 has it, so a widget past a short
//!   `/Opt` doesn't go with one of its on state that has an entry. In a
//!   field with no `/Opt` it's the on state's name read as text (UTF-8, as
//!   ISO 32000-2, §7.3.5 has a name's bytes read).
//! - **In unison,** pressing an off widget turns on every widget whose on
//!   state and export value are both its own, and turns every other off; out
//!   of unison it alone turns on. A check box field's widgets are always in
//!   unison, so check boxes of one on state go together and check boxes of
//!   distinct on states act as radio buttons; a radio button field's are with
//!   RadiosInUnison. PDFium as of chromium/8152 follows these rules; ISO
//!   32000-2 says only that buttons of one on state go together.
//! - **Pressing an on widget** turns it off, and those in unison with it,
//!   so a set of check boxes may be all off; a radio button stays on where
//!   the field has NoToggleToOff (ISO 32000-2, §12.7.5.2.4). PDFium as of
//!   chromium/8152 never turns a radio button off by a press, the flag set or
//!   not.
//! - **The field's value** is the name of the first on widget's on state,
//!   which is what its `/AS` becomes, `Off` where none is on (ISO 32000-2,
//!   §12.7.5.2.3: `/V` and `/AS` are the same). With `/Opt`, PDFium as of
//!   chromium/8152 writes the pressed widget's index whatever its on state's
//!   name, and leaves `/V` as it was where a widget is turned off.
//!
//! Each function is linear in the widgets and allocates nothing but its
//! answer. `Debug` prints counts and flags, never a state's name or an
//! export value.
//!
//! ```
//! use pdf_forms::group::{Group, GroupKind, GroupWidget};
//!
//! // Three radio buttons, the first on.
//! let widgets = [
//!     GroupWidget::new(Some(b"visa")),
//!     GroupWidget::new(Some(b"amex")),
//!     GroupWidget::new(Some(b"other")),
//! ];
//! let group = Group::new(GroupKind::Radio, &widgets);
//! let states = group.states_for(b"visa");
//! assert_eq!(states, [true, false, false]);
//!
//! // The second pressed: it turns on and the others off.
//! let states = group.press(&states, 1);
//! assert_eq!(states, [false, true, false]);
//! assert_eq!(&*group.value(&states), b"amex");
//! assert_eq!(&*group.state_name(0, states[0]), b"Off");
//! assert_eq!(&*group.state_name(1, states[1]), b"amex");
//! ```

use std::borrow::Cow;
use std::fmt;

/// Whether a [`Group`]'s widgets are check boxes or radio buttons.
///
/// Exhaustive on purpose: a button that keeps a state is one or the other
/// (ISO 32000-2, §12.7.5.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    /// A check box field's widgets, always in unison.
    CheckBox,
    /// A radio button field's widgets.
    Radio,
}

/// One widget of a [`Group`]: what names its on state and its export value.
///
/// Built from outside the crate with [`GroupWidget::new`], its fields set
/// after. `Debug` prints whether each is there, never a name or a value.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GroupWidget<'a> {
    /// The name the widget's normal appearance (`/AP /N`) has for its on
    /// state, as [`WidgetStates::on_state`](crate::WidgetStates::on_state)
    /// reads it. None where it has no appearance yet; then its on state is
    /// named by [`on_state_name`]. An empty name and `Off` are read as none.
    pub on_state: Option<Cow<'a, [u8]>>,
    /// The widget's export value: its entry in the field's `/Opt`, by the
    /// widget's index. None where the field has no `/Opt`, or no entry for
    /// the widget: its export value is then its on state's name read as
    /// text in a field with no `/Opt`, and `Yes` in one with
    /// ([`Group::has_opt`]), as it is for an entry that is empty. A widget
    /// that has an entry is read as one of a field with `/Opt`, whatever
    /// `has_opt` says.
    pub export: Option<Cow<'a, str>>,
}

impl<'a> GroupWidget<'a> {
    /// A widget whose normal appearance names its on state `on_state`, with
    /// no export value of its own.
    #[must_use]
    pub fn new(on_state: Option<&'a [u8]>) -> Self {
        Self {
            on_state: on_state.map(Cow::Borrowed),
            export: None,
        }
    }
}

impl fmt::Debug for GroupWidget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupWidget")
            .field("has_on_state", &self.on_state.is_some())
            .field("has_export", &self.export.is_some())
            .finish()
    }
}

/// A check box or radio button field's widgets, with the flags that rule
/// them: what the module's rules take.
///
/// Built from outside the crate with [`Group::new`], the flags set after.
/// `Debug` prints the kind, the flags, and how many widgets, never a name.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Group<'a> {
    /// Whether the widgets are check boxes or radio buttons.
    pub kind: GroupKind,
    /// NoToggleToOff (`/Ff` bit 15, `FieldFlags::NO_TOGGLE_TO_OFF`): pressing a
    /// radio button that is on leaves it on. A check box field's is read as
    /// clear, as the flag is a radio button field's.
    pub no_toggle_to_off: bool,
    /// RadiosInUnison (`/Ff` bit 26, `FieldFlags::RADIOS_IN_UNISON`): radio
    /// buttons of one on state and export value turn on and off together. A
    /// check box field's widgets always do, whatever this says.
    pub in_unison: bool,
    /// Whether the field has an `/Opt` array, of any length, an empty one
    /// among them ([`Field::option_count`](crate::Field::option_count)): its
    /// widgets with no on state of their own are then named by their
    /// indexes, and those with no entry export `Yes`.
    pub has_opt: bool,
    /// The field's widgets, in its order ([`Field::widgets`](crate::Field::widgets)'s):
    /// a widget's index here is the one its states, presses, and `/Opt`
    /// entry are by.
    pub widgets: &'a [GroupWidget<'a>],
}

impl<'a> Group<'a> {
    /// A group of `kind` of `widgets`, with neither flag, of a field with no
    /// `/Opt`.
    #[must_use]
    pub const fn new(kind: GroupKind, widgets: &'a [GroupWidget<'a>]) -> Self {
        Self {
            kind,
            no_toggle_to_off: false,
            in_unison: false,
            has_opt: false,
            widgets,
        }
    }

    /// Whether the widgets of one on state and export value go together: a
    /// check box field's always, a radio button field's with RadiosInUnison.
    #[must_use]
    pub const fn is_in_unison(&self) -> bool {
        matches!(self.kind, GroupKind::CheckBox) || self.in_unison
    }

    /// The name of widget `index`'s on state: its own, else as
    /// [`on_state_name`] gives it. None past the widgets.
    #[must_use]
    pub fn on_state(&self, index: usize) -> Option<Cow<'a, [u8]>> {
        Some(self.name(index)?.into_cow())
    }

    /// Widget `index`'s export value: its `/Opt` entry; in a field with
    /// `/Opt`, `Yes` where the entry is empty or there is none; in a field
    /// with no `/Opt`, its on state's name read as text (as UTF-8, a byte
    /// that isn't replaced by U+FFFD). None past the widgets.
    #[must_use]
    pub fn export_value(&self, index: usize) -> Option<Cow<'a, str>> {
        let name = self.name(index)?;
        Some(match self.opt_export(index) {
            Some(export) => Cow::Borrowed(export),
            None => match name.into_cow() {
                Cow::Borrowed(name) => String::from_utf8_lossy(name),
                Cow::Owned(name) => Cow::Owned(String::from_utf8_lossy(&name).into_owned()),
            },
        })
    }

    /// Each widget on or off for a field whose `/V` is `value`, a state's
    /// name: on where its on state's name is `value`, in unison every such
    /// widget and out of it the first. `Off`, or a name no widget's on state
    /// has, is every widget off.
    #[must_use]
    pub fn states_for(&self, value: &[u8]) -> Vec<bool> {
        let mut states = vec![false; self.widgets.len()];
        if value == b"Off" {
            return states;
        }
        let unison = self.is_in_unison();
        let mut buf = [0; NAME_BUF];
        for (index, state) in states.iter_mut().enumerate() {
            if self
                .name(index)
                .is_some_and(|name| name.bytes(&mut buf) == value)
            {
                *state = true;
                if !unison {
                    break;
                }
            }
        }
        states
    }

    /// Each widget on or off for a field given the export value `export`: the
    /// first widget whose export value is `export` pressed from every widget
    /// off ([`press`](Self::press)), so in unison those that go with it are
    /// on too. None where no widget's export value is `export`.
    #[must_use]
    pub fn states_for_export(&self, export: &str) -> Option<Vec<bool>> {
        let index = (0..self.widgets.len()).find(|&i| self.exports_as(i, export))?;
        Some(self.press(&[], index))
    }

    /// Each widget on or off after widget `index` is pressed, `states` each
    /// widget's before, by the module's rules. An off widget turns on, and
    /// in unison so does every widget of its on state and export value;
    /// every other turns off. An on widget turns off, and in unison those
    /// that go with it, but for a radio button where the field has
    /// NoToggleToOff, which leaves every widget as it was.
    ///
    /// The answer has a state for each widget. `states` shorter than the
    /// widgets is read as off past its end, and longer is cut to them; an
    /// `index` past the widgets changes nothing.
    #[must_use]
    pub fn press(&self, states: &[bool], index: usize) -> Vec<bool> {
        let mut after: Vec<bool> = (0..self.widgets.len())
            .map(|i| states.get(i).copied().unwrap_or(false))
            .collect();
        let Some(&was_on) = after.get(index) else {
            return after;
        };
        let unison = self.is_in_unison();
        if !was_on {
            for (i, state) in after.iter_mut().enumerate() {
                *state = i == index || (unison && self.together(i, index));
            }
        } else if !(self.kind == GroupKind::Radio && self.no_toggle_to_off) {
            for (i, state) in after.iter_mut().enumerate() {
                if i == index || (unison && self.together(i, index)) {
                    *state = false;
                }
            }
        }
        after
    }

    /// The field's value (`/V`, a name) for `states`: the name of the first
    /// on widget's on state, `Off` where none is on.
    #[must_use]
    pub fn value(&self, states: &[bool]) -> Cow<'a, [u8]> {
        match self.first_on(states).and_then(|index| self.on_state(index)) {
            Some(name) => name,
            None => Cow::Borrowed(b"Off"),
        }
    }

    /// The field's export value for `states`: the first on widget's
    /// ([`export_value`](Self::export_value)), none where none is on.
    #[must_use]
    pub fn export(&self, states: &[bool]) -> Option<Cow<'a, str>> {
        self.export_value(self.first_on(states)?)
    }

    /// What widget `index`'s `/AS` becomes when it is `on`: its on state's
    /// name, or `Off`. `Off` past the widgets.
    #[must_use]
    pub fn state_name(&self, index: usize, on: bool) -> Cow<'a, [u8]> {
        match self.name(index).filter(|_| on) {
            Some(name) => name.into_cow(),
            None => Cow::Borrowed(b"Off"),
        }
    }

    /// The first widget `states` has on.
    fn first_on(&self, states: &[bool]) -> Option<usize> {
        states.iter().take(self.widgets.len()).position(|&on| on)
    }

    /// What names widget `index`'s on state.
    fn name(&self, index: usize) -> Option<Name<'a>> {
        let widgets: &'a [GroupWidget<'a>] = self.widgets;
        let widget = widgets.get(index)?;
        let own = (widget.on_state.as_deref()).filter(|name| is_on_name(name));
        Some(match own {
            Some(name) => Name::Own(name),
            None if self.has_opt || widget.export.is_some() => Name::Index(index),
            None => Name::Yes,
        })
    }

    /// Widget `index`'s export value as `/Opt` gives it: its entry, and
    /// `Yes` for an empty one, or for none in a field with `/Opt`. None in
    /// a field with no `/Opt`, where it's the on state's name, and past the
    /// widgets.
    fn opt_export(&self, index: usize) -> Option<&'a str> {
        let widgets: &'a [GroupWidget<'a>] = self.widgets;
        match widgets.get(index)?.export.as_deref() {
            Some(export) if !export.is_empty() => Some(export),
            Some(_) => Some("Yes"),
            None if self.has_opt => Some("Yes"),
            None => None,
        }
    }

    /// Whether widgets `a` and `b` go together in unison: the same on state
    /// and the same export value.
    fn together(&self, a: usize, b: usize) -> bool {
        let (Some(name_a), Some(name_b)) = (self.name(a), self.name(b)) else {
            return false;
        };
        let (mut buf_a, mut buf_b) = ([0; NAME_BUF], [0; NAME_BUF]);
        if name_a.bytes(&mut buf_a) != name_b.bytes(&mut buf_b) {
            return false;
        }
        match self.opt_export(b) {
            Some(export) => self.exports_as(a, export),
            None => {
                let mut buf = [0; NAME_BUF];
                self.exports_as(a, &String::from_utf8_lossy(name_b.bytes(&mut buf)))
            }
        }
    }

    /// Whether widget `index`'s export value is `export`.
    fn exports_as(&self, index: usize, export: &str) -> bool {
        match (self.opt_export(index), self.name(index)) {
            (Some(own), _) => own == export,
            (None, Some(name)) => {
                let mut buf = [0; NAME_BUF];
                String::from_utf8_lossy(name.bytes(&mut buf)) == export
            }
            (None, None) => false,
        }
    }
}

impl fmt::Debug for Group<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Group")
            .field("kind", &self.kind)
            .field("no_toggle_to_off", &self.no_toggle_to_off)
            .field("in_unison", &self.in_unison)
            .field("has_opt", &self.has_opt)
            .field("widgets", &self.widgets.len())
            .finish()
    }
}

/// The name a check box's or radio button's on state is written under in
/// its widget's appearance dictionary (`/AP`): `existing`, the name its
/// normal appearance already has for on, where it has one; else, in a field
/// that has an `/Opt` array (`has_opt`: one of any length, whether or not
/// it has an entry for the widget, as [`Group::has_opt`] is), `index`, the
/// widget's among the field's, as a decimal (ISO 32000-2, Table 230), which
/// tells widgets of one export value apart; else `Yes`. These are the names
/// PDFium as of chromium/8152 gives a state it makes, but that PDFium puts
/// the index before a name the widget has: here a name the widget has is
/// kept, so its stream is found by it. [`Group::on_state`] gives the same
/// name for a widget of its group.
///
/// An `existing` name that is empty, or `Off`, is none.
#[must_use]
pub fn on_state_name(existing: Option<&[u8]>, has_opt: bool, index: usize) -> Cow<'_, [u8]> {
    match existing.filter(|name| is_on_name(name)) {
        Some(name) => Cow::Borrowed(name),
        None if has_opt => Cow::Owned(index.to_string().into_bytes()),
        None => Cow::Borrowed(b"Yes"),
    }
}

/// Whether `name` can name an on state: it isn't empty and isn't `Off`.
fn is_on_name(name: &[u8]) -> bool {
    !name.is_empty() && name != b"Off"
}

/// The longest decimal a `usize` is written as.
const NAME_BUF: usize = 20;

/// What names a widget's on state, compared without being written out.
#[derive(Clone, Copy)]
enum Name<'a> {
    /// The name its normal appearance has.
    Own(&'a [u8]),
    /// Its index, in a field with `/Opt`.
    Index(usize),
    /// `Yes`.
    Yes,
}

impl<'a> Name<'a> {
    /// The name's bytes, an index written into `buf`.
    fn bytes<'b>(&self, buf: &'b mut [u8; NAME_BUF]) -> &'b [u8]
    where
        'a: 'b,
    {
        match *self {
            Name::Own(name) => name,
            Name::Index(mut index) => {
                let mut at = NAME_BUF;
                loop {
                    at -= 1;
                    buf[at] = b'0' + (index % 10) as u8;
                    index /= 10;
                    if index == 0 {
                        break;
                    }
                }
                &buf[at..]
            }
            Name::Yes => b"Yes",
        }
    }

    fn into_cow(self) -> Cow<'a, [u8]> {
        match self {
            Name::Own(name) => Cow::Borrowed(name),
            Name::Index(index) => Cow::Owned(index.to_string().into_bytes()),
            Name::Yes => Cow::Borrowed(b"Yes"),
        }
    }
}

/// A check box or radio button field's group, read from the field and its
/// widgets by [`Field::group`](crate::Field::group): the kind, the flags,
/// each widget's on state's name and export value, and the widgets' states
/// as their `/AS` has them.
///
/// `Debug` prints the kind, the flags, and counts, never a name or a value.
#[derive(Clone)]
pub struct FieldGroup {
    kind: GroupKind,
    no_toggle_to_off: bool,
    in_unison: bool,
    has_opt: bool,
    widgets: Vec<GroupWidget<'static>>,
    states: Vec<bool>,
}

impl FieldGroup {
    pub(crate) fn new(
        kind: GroupKind,
        no_toggle_to_off: bool,
        in_unison: bool,
        has_opt: bool,
        widgets: Vec<GroupWidget<'static>>,
        states: Vec<bool>,
    ) -> Self {
        Self {
            kind,
            no_toggle_to_off,
            in_unison,
            has_opt,
            widgets,
            states,
        }
    }

    /// The group the module's rules take.
    #[must_use]
    pub fn group(&self) -> Group<'_> {
        Group {
            kind: self.kind,
            no_toggle_to_off: self.no_toggle_to_off,
            in_unison: self.in_unison,
            has_opt: self.has_opt,
            widgets: &self.widgets,
        }
    }

    /// Each widget on or off as its `/AS` has it: on where `/AS` is the name
    /// of its on state as the group names it ([`Group::on_state`]), so a
    /// widget with no appearance yet is on where `/AS` names the state its
    /// appearance would be made under.
    #[must_use]
    pub fn states(&self) -> &[bool] {
        &self.states
    }
}

impl fmt::Debug for FieldGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldGroup")
            .field("kind", &self.kind)
            .field("no_toggle_to_off", &self.no_toggle_to_off)
            .field("in_unison", &self.in_unison)
            .field("has_opt", &self.has_opt)
            .field("widgets", &self.widgets.len())
            .field("on", &self.states.iter().filter(|&&on| on).count())
            .finish()
    }
}
