//! The group's rules (`crate::group`), each held alone over plain data.

use std::borrow::Cow;

use crate::group::{FieldGroup, Group, GroupKind, GroupWidget, on_state_name};

fn group_of<'a>(names: &[Option<&'a [u8]>]) -> Vec<GroupWidget<'a>> {
    names.iter().map(|&name| GroupWidget::new(name)).collect()
}

/// Widgets of `names`, each with the export value `exports` gives it.
fn exporting<'a>(names: &[Option<&'a [u8]>], exports: &[&'a str]) -> Vec<GroupWidget<'a>> {
    let mut widgets = group_of(names);
    for (widget, export) in widgets.iter_mut().zip(exports) {
        widget.export = Some(Cow::Borrowed(*export));
    }
    widgets
}

fn radios<'a>(widgets: &'a [GroupWidget<'a>]) -> Group<'a> {
    Group::new(GroupKind::Radio, widgets)
}

fn names(group: &Group<'_>, states: &[bool]) -> Vec<Vec<u8>> {
    (0..group.widgets.len())
        .map(|i| group.state_name(i, states[i]).into_owned())
        .collect()
}

#[test]
fn a_radio_group_turns_the_others_off() {
    let widgets = group_of(&[Some(b"1"), Some(b"2"), Some(b"3")]);
    let group = radios(&widgets);
    let states = group.press(&[true, false, false], 1);
    assert_eq!(states, [false, true, false]);
    assert_eq!(&*group.value(&states), b"2");
    assert_eq!(names(&group, &states), [&b"Off"[..], b"2", b"Off"]);
    // From all off, and with two on before (a file that says so): the one
    // pressed alone is on after.
    assert_eq!(group.press(&[false; 3], 2), [false, false, true]);
    assert_eq!(group.press(&[true, false, true], 1), [false, true, false]);
}

#[test]
fn pressing_the_on_radio_turns_it_off_but_with_no_toggle_to_off() {
    let widgets = group_of(&[Some(b"1"), Some(b"2"), Some(b"3")]);
    let mut group = radios(&widgets);
    let on = [true, false, false];
    let off = group.press(&on, 0);
    assert_eq!(off, [false; 3]);
    assert_eq!(&*group.value(&off), b"Off");
    assert_eq!(group.export(&off), None);

    group.no_toggle_to_off = true;
    assert_eq!(group.press(&on, 0), on);
    // Pressing another still moves the selection.
    assert_eq!(group.press(&on, 2), [false, false, true]);
    // An on radio of a group in unison stays on too, and so do those with it.
    let same = group_of(&[Some(b"1"), Some(b"2"), Some(b"1")]);
    let mut together = radios(&same);
    together.no_toggle_to_off = true;
    together.in_unison = true;
    assert_eq!(together.press(&[true, false, true], 0), [true, false, true]);
}

#[test]
fn radios_in_unison_turn_on_together_and_out_of_it_alone() {
    let widgets = group_of(&[Some(b"1"), Some(b"2"), Some(b"1")]);
    let mut group = radios(&widgets);
    assert!(!group.is_in_unison());
    assert_eq!(group.press(&[false; 3], 2), [false, false, true]);
    assert_eq!(group.states_for(b"1"), [true, false, false]);

    group.in_unison = true;
    assert!(group.is_in_unison());
    let states = group.press(&[false; 3], 2);
    assert_eq!(states, [true, false, true]);
    assert_eq!(&*group.value(&states), b"1");
    assert_eq!(group.states_for(b"1"), [true, false, true]);
    // Turned off together, without NoToggleToOff.
    assert_eq!(group.press(&states, 0), [false; 3]);
    // The second pressed turns the two off.
    assert_eq!(group.press(&states, 1), [false, true, false]);

    // One on state but another export value doesn't go with it.
    let apart = exporting(&[Some(b"1"), Some(b"2"), Some(b"1")], &["a", "b", "c"]);
    let mut group = radios(&apart);
    group.in_unison = true;
    assert_eq!(group.press(&[false; 3], 0), [true, false, false]);
    let same = exporting(&[Some(b"1"), Some(b"2"), Some(b"1")], &["a", "b", "a"]);
    let mut group = radios(&same);
    group.in_unison = true;
    assert_eq!(group.press(&[false; 3], 0), [true, false, true]);
}

#[test]
fn check_boxes_of_one_on_state_go_together() {
    let widgets = group_of(&[Some(b"Yes"), Some(b"Yes"), Some(b"No")]);
    let mut group = Group::new(GroupKind::CheckBox, &widgets);
    // A check box field is in unison whatever its flags say.
    assert!(group.is_in_unison());
    let on = group.press(&[false; 3], 0);
    assert_eq!(on, [true, true, false]);
    assert_eq!(&*group.value(&on), b"Yes");
    assert_eq!(group.export(&on).as_deref(), Some("Yes"));
    let off = group.press(&on, 1);
    assert_eq!(off, [false; 3]);
    assert_eq!(&*group.value(&off), b"Off");
    // NoToggleToOff is a radio button field's: a check box turns off.
    group.no_toggle_to_off = true;
    assert_eq!(group.press(&on, 1), [false; 3]);
    assert_eq!(group.states_for(b"Yes"), [true, true, false]);
}

#[test]
fn check_boxes_of_distinct_on_states_act_as_radios_and_may_all_be_off() {
    let widgets = group_of(&[Some(b"a"), Some(b"b"), Some(b"c")]);
    let group = Group::new(GroupKind::CheckBox, &widgets);
    let states = group.press(&[true, false, false], 1);
    assert_eq!(states, [false, true, false]);
    assert_eq!(&*group.value(&states), b"b");
    let off = group.press(&states, 1);
    assert_eq!(off, [false; 3]);
    assert_eq!(&*group.value(&off), b"Off");
    assert_eq!(names(&group, &off), [&b"Off"[..], b"Off", b"Off"]);
}

#[test]
fn with_opt_widgets_of_one_export_value_are_told_apart_by_index() {
    // `/Opt [(apple) (pear) (apple)]`, the widgets' states named by index.
    let widgets = exporting(&[None, None, None], &["apple", "pear", "apple"]);
    let group = Group::new(GroupKind::CheckBox, &widgets);
    assert_eq!(
        (0..3)
            .map(|i| group.on_state(i).unwrap().into_owned())
            .collect::<Vec<_>>(),
        [&b"0"[..], b"1", b"2"]
    );
    let states = group.press(&[false; 3], 0);
    assert_eq!(states, [true, false, false]);
    assert_eq!(&*group.value(&states), b"0");
    assert_eq!(group.export(&states).as_deref(), Some("apple"));
    // By export value, the first that has it.
    assert_eq!(
        group.states_for_export("apple"),
        Some(vec![true, false, false])
    );
    assert_eq!(
        group.states_for_export("pear"),
        Some(vec![false, true, false])
    );
    assert_eq!(group.states_for_export("plum"), None);
    // The third, of the first's export value, by its own name.
    assert_eq!(group.states_for(b"2"), [false, false, true]);

    // Widgets with names of their own keep them, the export value the entry.
    let own = exporting(&[Some(b"x"), Some(b"y")], &["a", "b"]);
    let group = Group::new(GroupKind::CheckBox, &own);
    let states = group.press(&[false, false], 1);
    assert_eq!(&*group.value(&states), b"y");
    assert_eq!(group.export(&states).as_deref(), Some("b"));
    assert_eq!(&*group.state_name(1, true), b"y");

    // A radio group in unison of one export value: told apart by index too.
    let widgets = exporting(&[None, None, None], &["apple", "pear", "apple"]);
    let mut group = radios(&widgets);
    group.in_unison = true;
    assert_eq!(group.press(&[false; 3], 0), [true, false, false]);
}

#[test]
fn export_values_are_names_read_as_text_without_opt() {
    let widgets = group_of(&[Some(b"Yes"), Some(b"caf\xc3\xa9"), Some(b"\xff"), None]);
    let group = Group::new(GroupKind::CheckBox, &widgets);
    assert_eq!(group.export_value(0).as_deref(), Some("Yes"));
    assert_eq!(group.export_value(1).as_deref(), Some("caf\u{e9}"));
    assert_eq!(group.export_value(2).as_deref(), Some("\u{fffd}"));
    assert_eq!(group.export_value(3).as_deref(), Some("Yes"));
    assert_eq!(group.export_value(4), None);
    assert_eq!(
        group.states_for_export("caf\u{e9}"),
        Some(vec![false, true, false, false])
    );
    // The widget with no appearance is named `Yes`, as the first is: they go
    // together.
    assert_eq!(
        group.states_for_export("Yes"),
        Some(vec![true, false, false, true])
    );
}

#[test]
fn on_state_names_are_the_widgets_own_else_by_index_else_yes() {
    for name in [&b"1"[..], b"Yes", b"On", b"spouse", b"Choice1"] {
        assert_eq!(&*on_state_name(Some(name), false, 3), name);
        assert_eq!(&*on_state_name(Some(name), true, 3), name);
    }
    assert_eq!(&*on_state_name(None, false, 3), b"Yes");
    assert_eq!(&*on_state_name(None, true, 3), b"3");
    assert_eq!(&*on_state_name(None, true, 0), b"0");
    assert_eq!(&*on_state_name(None, true, 1234), b"1234");
    assert_eq!(
        &*on_state_name(None, true, usize::MAX),
        usize::MAX.to_string().as_bytes()
    );
    // Neither `Off` nor an empty name names an on state.
    assert_eq!(&*on_state_name(Some(b"Off"), false, 2), b"Yes");
    assert_eq!(&*on_state_name(Some(b""), true, 2), b"2");

    // A group names its widgets the same way.
    let mut widgets = group_of(&[Some(b"spouse"), None, Some(b"Off"), Some(b"")]);
    widgets[3].export = Some(Cow::Borrowed("x"));
    let group = Group::new(GroupKind::Radio, &widgets);
    let named: Vec<_> = (0..4).map(|i| group.on_state(i).unwrap()).collect();
    assert_eq!(named, [&b"spouse"[..], b"Yes", b"Yes", b"3"]);
    assert_eq!(group.on_state(4), None);
}

#[test]
fn states_for_off_or_an_unknown_name_is_all_off() {
    let widgets = group_of(&[Some(b"1"), Some(b"Off"), None]);
    let group = Group::new(GroupKind::CheckBox, &widgets);
    assert_eq!(group.states_for(b"Off"), [false; 3]);
    assert_eq!(group.states_for(b"nothing"), [false; 3]);
    assert_eq!(group.states_for(b""), [false; 3]);
    assert_eq!(group.states_for(b"1"), [true, false, false]);
    // The two named `Yes`, the second's `Off` read as none.
    assert_eq!(group.states_for(b"Yes"), [false, true, true]);
}

#[test]
fn short_states_and_an_index_past_the_widgets_dont_panic() {
    let widgets = group_of(&[Some(b"1"), Some(b"2"), Some(b"3")]);
    let group = radios(&widgets);
    // Read as off past their end, and cut to the widgets.
    assert_eq!(group.press(&[], 1), [false, true, false]);
    assert_eq!(group.press(&[true], 2), [false, false, true]);
    assert_eq!(
        group.press(&[true, false, false, true, true], 1),
        [false, true, false]
    );
    // An index past them changes nothing.
    assert_eq!(group.press(&[true], 3), [true, false, false]);
    assert_eq!(group.press(&[], usize::MAX), [false; 3]);
    assert_eq!(&*group.state_name(3, true), b"Off");
    assert_eq!(group.export_value(3), None);
    assert_eq!(&*group.value(&[false, false, false, true]), b"Off");
    assert_eq!(group.export(&[false, false, false, true]), None);
    assert_eq!(&*group.value(&[]), b"Off");

    // A group of none.
    let none = Group::new(GroupKind::CheckBox, &[]);
    assert_eq!(none.press(&[true], 0), Vec::<bool>::new());
    assert_eq!(none.states_for(b"Yes"), Vec::<bool>::new());
    assert_eq!(none.states_for_export("Yes"), None);
    assert_eq!(&*none.value(&[true]), b"Off");
}

#[test]
fn debug_prints_no_name_or_export_value() {
    let shown = |names: &[Option<&[u8]>], exports: &[&str]| {
        let widgets = exporting(names, exports);
        let mut group = Group::new(GroupKind::Radio, &widgets);
        group.in_unison = true;
        let owned = (widgets.iter())
            .map(|w| GroupWidget {
                on_state: w.on_state.as_deref().map(|n| Cow::Owned(n.to_vec())),
                export: w.export.as_deref().map(|e| Cow::Owned(e.to_owned())),
            })
            .collect();
        let field = FieldGroup::new(GroupKind::Radio, true, false, owned, vec![true]);
        format!("{group:?} {widgets:?} {field:?}")
    };
    let one = shown(&[Some(b"spouse"), None], &["apple", "pear"]);
    for word in ["spouse", "apple", "pear"] {
        assert!(!one.contains(word), "{one}");
    }
    assert!(one.contains("widgets: 2") && one.contains("on: 1"), "{one}");
    // Nor their bytes or lengths in any other form: a group of other names
    // and export values prints alike.
    let other = shown(&[Some(b"x"), None], &["a much longer export", "b"]);
    assert_eq!(one, other);
}

#[test]
fn public_types_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<GroupKind>();
    check::<GroupWidget<'static>>();
    check::<Group<'static>>();
    check::<FieldGroup>();
}
