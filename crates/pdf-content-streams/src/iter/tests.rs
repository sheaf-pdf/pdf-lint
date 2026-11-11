//! Each named iterator walked to its end by `next` and by `fold`, and
//! from the back by `next_back` and `rfold` where it walks from either
//! end, alike, its hints holding what it gives; and walked by `next` past
//! its end, none more. Those whose walk knows how many are left give it
//! exactly in their hints.

use std::fmt::Debug;
use std::iter::FusedIterator;

use pdf_objects::{Object, ObjectId, dict};

use crate::*;

/// What `iter` gives, walked by `next` and by `fold` alike: its hints hold
/// the count at every step, it gives none past its end, and `count`
/// counts the same.
fn walked<I>(iter: I) -> Vec<I::Item>
where
    I: Iterator + FusedIterator + Clone + Debug,
    I::Item: PartialEq + Debug,
{
    let mut stepped = iter.clone();
    let mut by_next = Vec::new();
    loop {
        let (low, high) = stepped.size_hint();
        let Some(item) = stepped.next() else {
            assert_eq!(low, 0, "{iter:?}");
            break;
        };
        let left = 1 + stepped.clone().count();
        assert!(low <= left && high.is_none_or(|h| h >= left), "{iter:?}");
        by_next.push(item);
    }
    assert!(stepped.next().is_none() && stepped.next().is_none());
    let by_fold = (iter.clone()).fold(Vec::new(), |mut v, item| {
        v.push(item);
        v
    });
    assert_eq!(by_next, by_fold, "{iter:?}");
    assert_eq!(iter.count(), by_next.len());
    by_next
}

/// `iter`'s hints exact at every step, as the walk it forwards knows how
/// many are left: a hint of its own would be valid, and not exact.
fn hinted_exactly<I>(iter: I)
where
    I: Iterator + Clone + Debug,
{
    let mut stepped = iter.clone();
    loop {
        let left = stepped.clone().count();
        assert_eq!(stepped.size_hint(), (left, Some(left)), "{iter:?}");
        if stepped.next().is_none() {
            break;
        }
    }
}

/// What `iter` gives from the back, by `next_back` and by `rfold`, alike,
/// and the same as from the front, reversed.
fn walked_back<I>(iter: I) -> Vec<I::Item>
where
    I: DoubleEndedIterator + FusedIterator + Clone + Debug,
    I::Item: PartialEq + Debug,
{
    let mut stepped = iter.clone();
    let mut by_next_back = Vec::new();
    while let Some(item) = stepped.next_back() {
        by_next_back.push(item);
    }
    assert!(stepped.next_back().is_none() && stepped.next().is_none());
    let by_rfold = (iter.clone()).rfold(Vec::new(), |mut v, item| {
        v.push(item);
        v
    });
    assert_eq!(by_next_back, by_rfold, "{iter:?}");
    let mut forward = walked(iter);
    forward.reverse();
    assert_eq!(by_next_back, forward);
    by_next_back
}

#[test]
fn an_operations_operands_walk_alike() {
    let limits = Limits::DEFAULT;
    // Three operands held in the item, and twenty spilled to the heap.
    let many: String = (0..20).map(|i| format!("{i} ")).collect();
    let content = format!("1 (a) /N Tj {many}l");
    let read: Vec<_> = operations(content.as_bytes(), &ResCx::none(), &limits)
        .map(Result::unwrap)
        .collect();
    for (op, count) in read.iter().zip([3, 20]) {
        let operands = op.operands();
        assert_eq!(operands.len(), count);
        let bytes: Vec<_> = walked(operands.clone()).iter().map(|o| o.range()).collect();
        let back: Vec<_> = walked_back(operands).iter().map(|o| o.range()).collect();
        assert_eq!(bytes.len(), count);
        assert!(bytes.iter().rev().eq(&back));
        assert_eq!(
            op.operands().next().map(|o| o.range().start),
            Some(op.range().start)
        );
    }
    // From the middle, from either end, a step at a time.
    let mut operands = read[1].operands();
    operands.next();
    operands.next_back();
    assert_eq!(operands.len(), 18);
    assert_eq!(walked_back(operands).len(), 18);
    // None: an operator alone.
    let q = operations(b"q", &ResCx::none(), &limits)
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(walked(q.operands()), []);
}

#[test]
fn a_sets_operators_walk_alike_in_order() {
    let all = walked(Operators::ALL.iter());
    let want: Vec<_> = (Operator::ALL.iter().copied())
        .chain([Operator::Other])
        .collect();
    assert_eq!(all, want);
    assert_eq!(Operators::ALL.iter().len(), Operator::ALL.len() + 1);
    let some = Operators::of(&[b"EMC", b"q", b"Fl", b"cm"]);
    let given = walked(some.iter());
    let want = [
        Operator::Save,
        Operator::Transform,
        Operator::EndMarked,
        Operator::Other,
    ];
    assert_eq!(given, want);
    assert_eq!(some.iter().len(), some.len());
    assert_eq!(walked(Operators::NONE.iter()), []);
    // Each category's, in its own order.
    for &op in Operator::ALL {
        let Some(category) = op.category() else {
            continue;
        };
        let theirs = walked(category.operators().iter());
        assert!(theirs.contains(&op));
        assert!(theirs.is_sorted_by_key(|&o| o as usize));
    }
}

#[test]
fn capped_names_walk_alike() {
    let nested = || Object::from(vec![Object::from(vec![Object::from("DeviceGray")])]);
    let spaces = dict! {
        "CS2" => nested(),
        "CS0" => nested(),
        "CS1" => Object::from("DeviceRGB"),
        "CS3" => nested(),
    };
    let resources = Object::from(dict! { "ColorSpace" => spaces });
    let nothing = |_: ObjectId| None;
    let mut limits = Limits::DEFAULT;
    limits.color.space_depth = 0;
    let res = ResCx::read(nothing, Some(&resources), &limits).unwrap();
    let names = walked(res.capped());
    assert_eq!(names, [&b"CS0"[..], b"CS2", b"CS3"]);
    hinted_exactly(res.capped());
    assert_eq!(walked(ResCx::none().capped()), Vec::<&[u8]>::new());
}

#[test]
fn names_walk_alike_by_category() {
    assert!(
        ResKind::ALL.is_sorted(),
        "`NameSet::of` reads `ALL` as in order"
    );
    let content = b"/F2 9 Tf /F1 9 Tf /GS0 gs /Im0 Do /X /P0 BDC EMC /Sh0 sh /CS0 cs /P1 scn";
    let page = Concat::new(&[content]);
    let names = names_used(&page, &ResCx::none(), &Limits::DEFAULT, &[]).unwrap();
    let every = walked(names.iter());
    assert_eq!(every.len(), names.len());
    hinted_exactly(names.iter());
    let mut kinds = 0;
    for &kind in ResKind::ALL {
        let of = walked(names.of(kind));
        let want: Vec<_> = (every.iter())
            .filter(|(k, _)| *k == kind)
            .map(|&(_, name)| name)
            .collect();
        assert_eq!(of, want, "{kind:?}");
        kinds += usize::from(!of.is_empty());
    }
    assert_eq!(kinds, ResKind::ALL.len());
    assert_eq!(walked(names.of(ResKind::Font)), [&b"F1"[..], b"F2"]);
    assert_eq!(walked(NameSet::default().iter()), []);
}

#[test]
fn property_lists_walk_alike() {
    let lists = (0..5).map(|i| (format!("P{i}").into_bytes(), dict! { "MCID" => i }));
    let props = Properties::new(lists);
    hinted_exactly(props.lists());
    let mut given: Vec<_> = (walked(props.lists()).into_iter())
        .map(|d| d.get(b"MCID").and_then(Object::as_integer))
        .collect();
    given.sort();
    assert_eq!(given, (0..5).map(Some).collect::<Vec<_>>());
    assert_eq!(
        walked(Properties::default().lists()),
        Vec::<&pdf_objects::Dict>::new()
    );
}

#[test]
fn ranges_still_open_walk_alike() {
    let props = Properties::default();
    let page = Concat::new(&[b"/A BMC /B BMC EMC /C BMC /D BMC"]);
    let mut nest = MarkedNest::new(&props, &Limits::DEFAULT);
    let mut seen = Vec::new();
    for op in page.operations(&ResCx::none(), &Limits::DEFAULT) {
        nest.step(&page, &op.unwrap()).unwrap();
        seen.push(walked(nest.open()));
        assert_eq!(nest.open().len(), seen.last().unwrap().len());
    }
    assert_eq!(seen.last().unwrap(), &[0, 2, 3]);
}

#[test]
fn reserved_names_walk_alike() {
    let resources = Object::from(dict! {
        "Font" => dict! { "F2" => Object::reference(1), "F1" => Object::reference(2) },
        "ExtGState" => dict! { "GS0" => dict! { "ca" => 0.5 } },
    });
    let nothing = |_: ObjectId| None;
    let reserved = Reserved::read(nothing, Some(&resources), &Limits::DEFAULT);
    let names = walked(reserved.names());
    hinted_exactly(reserved.names());
    let want = [
        (ResKind::ExtGState, &b"GS0"[..]),
        (ResKind::Font, b"F1"),
        (ResKind::Font, b"F2"),
    ];
    assert_eq!(names, want);
    assert_eq!(walked(Reserved::default().names()), []);
}
