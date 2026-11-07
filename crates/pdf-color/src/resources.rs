//! What a resources dictionary names (ISO 32000-2, 7.8.3): its `/ColorSpace`
//! dictionary, reached as PDFium reaches it, and each name counted.

use std::fmt;
use std::iter::FusedIterator;

use pdf_objects::object::LastEntries;
use pdf_objects::{Dict, Entry, Object, Resolve, Span, Unreadable, Value};

use crate::count::{Counted, counted, look};
use crate::limits::Limits;

/// A resources dictionary's named color spaces: its `/ColorSpace` dictionary.
/// A key a reader of the file gave no bytes is no name: PDFium drops an empty
/// key as it parses it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spaces<'o> {
    dict: &'o Dict,
}

/// A name in a [`Spaces`]: a dictionary's entry, each part as it was read.
///
/// `#[non_exhaustive]`, so a part more can join it in a minor release: it's
/// read by its fields, and made only by this crate.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Named<'o> {
    /// The key's bytes, as the reader of the file read them, so that a caller
    /// with its bytes can read a key with a `#` otherwise from where it's
    /// written.
    pub key: &'o [u8],
    /// Where the key was written, when the reader knows.
    pub key_at: Option<Span>,
    /// What the key holds, as written, for
    /// [`components`](crate::components).
    pub value: &'o Object,
}

impl<'o> Named<'o> {
    /// The name `entry` is.
    fn of(entry: &'o Entry) -> Named<'o> {
        Named {
            key: &entry.key,
            key_at: entry.key_at,
            value: &entry.value,
        }
    }
}

/// The dictionary `value` is, a stream's among them, through at most two
/// references, as PDFium's accessor reads one: through more, or in a loop, it's
/// none.
fn dictionary<'o, R: Resolve<'o>>(
    objects: &R,
    value: &'o Object,
) -> Result<Option<&'o Dict>, Unreadable> {
    let mut object = value;
    for _ in 0..2 {
        let Some(id) = object.as_reference() else {
            break;
        };
        let Some(next) = look(objects, id)? else {
            return Ok(None);
        };
        object = next;
    }
    Ok(match object.value() {
        Value::Dict(dict) => Some(dict),
        Value::Stream(stream) => Some(&stream.dict),
        _ => None,
    })
}

impl<'o> Spaces<'o> {
    /// The `/ColorSpace` dictionary of `resources`, a page's or a form's
    /// `/Resources` as written, or none where there's no such thing: for a page
    /// from pdf-semantics that's its `Attribute::written`, not its `value`, which
    /// has been followed, past where PDFium stops. What a form draws in, as PDFium
    /// reads it, is two rules. A form with no `/Resources` takes what holds it, the
    /// parent form's or the page's. A form with `/Resources` and none here takes
    /// the page's, whatever holds it. So none is what its reader asks.
    ///
    /// This is PDFium's reading, and it tells nothing: `/Resources` through three
    /// references, which 7.3.10 allows, a damaged `/ColorSpace` and a stream given
    /// for a dictionary are each just what they are here, with no fault. An ISO
    /// check of a resources dictionary reads the dictionary itself.
    ///
    /// `resources` and its `/ColorSpace` may each be a dictionary or a stream,
    /// each through at most two references: through three, or in a loop, as
    /// of chromium/8125 there's no dictionary, and that's no error.
    ///
    /// ```
    /// use pdf_color::pdf_objects::{Object, ObjectId, dict};
    /// use pdf_color::{Limits, Spaces, components};
    ///
    /// let resources = Object::from(dict! { "ColorSpace" => dict! { "A" => "DeviceCMYK" } });
    /// let nothing = |_: ObjectId| None;
    /// let spaces = Spaces::read(nothing, Some(&resources)).unwrap().unwrap();
    /// let named = spaces.get(b"A").unwrap();
    /// assert_eq!(components(nothing, named.value, &Limits::DEFAULT), Ok(Some(4)));
    /// assert_eq!(Spaces::read(nothing, None), Ok(None));
    /// ```
    ///
    /// # Errors
    ///
    /// [`Unreadable`], whole, for any failure to read `resources`, its
    /// `/ColorSpace` or a reference on the way to either, but one that's
    /// [`Why::Damaged`](pdf_objects::Why::Damaged): an object past a reader's
    /// cap, locked, or a source that didn't give its bytes. An object that's
    /// damaged isn't there.
    pub fn read<R: Resolve<'o>>(
        objects: R,
        resources: Option<&'o Object>,
    ) -> Result<Option<Spaces<'o>>, Unreadable> {
        let Some(resources) = resources else {
            return Ok(None);
        };
        let Some(resources) = dictionary(&objects, resources)? else {
            return Ok(None);
        };
        let Some(spaces) = resources.get(b"ColorSpace") else {
            return Ok(None);
        };
        Ok(dictionary(&objects, spaces)?.map(|dict| Spaces { dict }))
    }

    /// Each name, a key written twice at its last entry, in order.
    pub fn iter(&self) -> Iter<'o> {
        Iter {
            entries: self.dict.last_entries(),
        }
    }

    /// Each name, as [`iter`](Spaces::iter) gives them, with its count as
    /// [`counted`](crate::counted) gives it for the name's value, under
    /// `limits`: PDFium's count, which sizes an unfiltered inline image drawn
    /// in it, and not ISO's ([`Space::components`](crate::Space::components)).
    /// Its [`count`](Counted::count) is none for a name whose value is a
    /// reference to nothing, as PDFium has no space there, and
    /// [`capped`](Counted::capped) says where a cap was met, for a caller that
    /// acts on where an image ends.
    ///
    /// Each name is counted as it's asked for, and nothing is kept between
    /// them, so a caller that stops early pays for what it asked, and the
    /// whole costs what each name's [`counted`](crate::counted) does, with no
    /// cap over all of them ([`Limits::space_visits`] says what that can come
    /// to). Its upper bound is the dictionary's entries, for a caller that
    /// makes room for every name at once.
    ///
    /// ```
    /// use pdf_color::pdf_objects::{Object, ObjectId, dict};
    /// use pdf_color::{Limits, Spaces};
    ///
    /// let spaces = dict! { "A" => "DeviceCMYK", "B" => Object::reference(9), "C" => "Pattern" };
    /// let resources = Object::from(dict! { "ColorSpace" => spaces });
    /// let nothing = |_: ObjectId| None;
    /// let spaces = Spaces::read(nothing, Some(&resources)).unwrap().unwrap();
    /// let counts: Vec<(&[u8], Option<u32>)> = (spaces.components(nothing, &Limits::DEFAULT))
    ///     .map(|(named, counted)| (named.key, counted.unwrap().count))
    ///     .collect();
    /// // A reference to nothing is no space at all.
    /// assert_eq!(counts, [(&b"A"[..], Some(4)), (&b"B"[..], None), (&b"C"[..], Some(1))]);
    /// ```
    ///
    /// # Errors
    ///
    /// Each name's own, as [`counted`](crate::counted) fails for its value. A
    /// name whose space can't be read, an object past a reader's cap, locked,
    /// or whose source failed, has that `Err` for its count, and the names
    /// after it are counted all the same: a caller that wants the whole as one
    /// `Err` stops at the first by `?`, and one that passes it over counts the
    /// rest without it. An object that's
    /// [`Why::Damaged`](pdf_objects::Why::Damaged) isn't there, and is no
    /// error.
    pub fn components<R: Resolve<'o> + Copy>(
        &self,
        objects: R,
        limits: &Limits,
    ) -> Components<'o, R> {
        Components {
            names: self.iter(),
            objects,
            limits: *limits,
        }
    }

    /// The name `key`, its last entry if it's written twice: a color space a
    /// reader of content looks up, or `DefaultGray` and the like (8.6.5.6).
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<Named<'o>> {
        if key.is_empty() {
            return None;
        }
        self.dict.entry(key).map(Named::of)
    }
}

/// A [`Spaces`]' names, each key written twice at its last entry, in order:
/// what [`Spaces::iter`] gives. A key a reader of the file gave no bytes is
/// no name.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Iter<'o> {
    entries: LastEntries<'o>,
}

impl<'o> Iterator for Iter<'o> {
    type Item = Named<'o>;

    #[inline]
    fn next(&mut self) -> Option<Named<'o>> {
        (self.entries.by_ref())
            .find(|entry| !entry.key.is_empty())
            .map(Named::of)
    }

    /// At most one an entry left: as many as the dictionary has, at first.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.entries.size_hint().1)
    }

    /// One pass over the entries, as `count`, `for_each` and `last` fold.
    #[inline]
    fn fold<B, F: FnMut(B, Named<'o>) -> B>(self, init: B, mut f: F) -> B {
        self.entries
            .fold(init, |acc, entry| match entry.key.is_empty() {
                true => acc,
                false => f(acc, Named::of(entry)),
            })
    }
}

impl DoubleEndedIterator for Iter<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        (self.entries.by_ref())
            .rfind(|entry| !entry.key.is_empty())
            .map(Named::of)
    }
}

impl FusedIterator for Iter<'_> {}

/// A [`Spaces`]' names, each with its count as [`counted`](crate::counted)
/// gives it for the name's value, or the `Err` that name's space met, in
/// [`Spaces::iter`]'s order: what [`Spaces::components`] gives.
///
/// # Errors
///
/// A name's count is the `Err` its space met, as [`Spaces::components`]
/// says. A name that fails ends nothing: the names after it are counted all
/// the same.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Clone)]
pub struct Components<'o, R> {
    names: Iter<'o>,
    objects: R,
    limits: Limits,
}

impl<'o, R: Resolve<'o> + Copy> Iterator for Components<'o, R> {
    type Item = (Named<'o>, Result<Counted, Unreadable>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let named = self.names.next()?;
        Some((named, counted(self.objects, named.value, &self.limits)))
    }

    /// [`Iter`]'s: at most one an entry left.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.names.size_hint()
    }
}

impl<'o, R: Resolve<'o> + Copy> FusedIterator for Components<'o, R> {}

/// The names it has left, as [`Iter`] prints them, and its caps, but not its
/// resolver, which a closure can't print: as `core`'s adapters print what
/// they walk and not their closures.
impl<R> fmt::Debug for Components<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (f.debug_struct("Components"))
            .field("names", &self.names)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use pdf_objects::parse::Parser;
    use pdf_objects::{Object, ObjectId, Why, dict};

    use super::*;
    use crate::{Limits, components};

    /// `text` as one value.
    fn value(text: &str) -> Object {
        let mut parser = Parser::at(text.as_bytes(), 0);
        (parser.value(&mut ())).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    /// `extra` read, the first of them object 5, a dictionary with `/Length` a
    /// stream.
    fn objects(extra: &[&str]) -> BTreeMap<ObjectId, Object> {
        (extra.iter().zip(5..))
            .map(|(text, n)| {
                let object = value(text);
                let stream = (object.as_dict())
                    .filter(|dict| dict.has(b"Length"))
                    .map(|dict| Object::stream(dict.clone()));
                (ObjectId::new(n, 0), stream.unwrap_or(object))
            })
            .collect()
    }

    /// Whether `resources`, with `extra` for its objects, has a `/ColorSpace`
    /// dictionary, by what it holds under `/X`.
    fn found(resources: &str, extra: &[&str]) -> Option<usize> {
        let (resources, objects) = (value(resources), objects(extra));
        let resolve = |id: ObjectId| objects.get(&id);
        let spaces = Spaces::read(resolve, Some(&resources)).unwrap();
        spaces.map(|spaces| spaces.iter().count())
    }

    /// A chain of `n` objects from 5 on, `n - 1` references then `end`.
    fn chain<'a>(n: u32, end: &'a str, refs: &'a mut Vec<String>) -> Vec<&'a str> {
        refs.clear();
        refs.extend((0..n - 1).map(|at| format!("{} 0 R", 6 + at)));
        refs.iter().map(String::as_str).chain([end]).collect()
    }

    #[test]
    fn color_spaces_count_their_components() {
        let icc = Object::stream(dict! { "N" => 4 });
        let gray = Object::from("DeviceGray");
        let spaces = Object::from(dict! {
            "Icc" => vec![Object::from("ICCBased"), Object::reference(1)],
            "Cmyk" => "DeviceCMYK",
            "Gray" => Object::reference(2),
            "Idx" => vec![
                Object::from("Indexed"),
                Object::from("DeviceRGB"),
                Object::from(1),
                Object::text("abcdef"),
            ],
            "N2" => vec![
                Object::from("DeviceN"),
                Object::from(vec![Object::from("a"), Object::from("b")]),
                Object::from("DeviceRGB"),
                Object::reference(4),
            ],
            "Lab" => vec![Object::from("Lab"), Object::from(dict! {})],
            "Odd" => 7,
        });
        let function = value("<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0] >>");
        let objects = BTreeMap::from([
            (ObjectId::new(1, 0), icc),
            (ObjectId::new(2, 0), gray),
            (ObjectId::new(3, 0), spaces),
            (ObjectId::new(4, 0), function),
        ]);
        let resources = Object::from(dict! { "ColorSpace" => Object::reference(3) });
        let resolve = |id: ObjectId| objects.get(&id);
        let spaces = Spaces::read(resolve, Some(&resources)).unwrap().unwrap();
        let counted: Vec<(&[u8], Option<u32>)> = (spaces.iter())
            .map(|named| {
                let count = components(resolve, named.value, &Limits::DEFAULT);
                (named.key, count.unwrap())
            })
            .collect();
        let want: [(&[u8], Option<u32>); 7] = [
            (b"Icc", Some(4)),
            (b"Cmyk", Some(4)),
            (b"Gray", Some(1)),
            (b"Idx", Some(1)),
            (b"N2", Some(2)),
            (b"Lab", Some(3)),
            (b"Odd", Some(3)),
        ];
        assert_eq!(counted, want);
        // The same, name by name, from the dictionary's own iterator.
        let named: Vec<(&[u8], Option<u32>)> = (spaces.components(resolve, &Limits::DEFAULT))
            .map(|(named, counted)| (named.key, counted.unwrap().count))
            .collect();
        assert_eq!(named, want);
        assert_eq!(Spaces::read(resolve, None), Ok(None));
    }

    #[test]
    fn resources_and_their_color_spaces_stop_at_two_references() {
        let plain = "<< /ColorSpace << /X /DeviceCMYK >> >>";
        let spaces = "<< /X /DeviceCMYK >>";
        let mut refs = Vec::new();
        for (n, want) in [(1, Some(1)), (2, Some(1)), (3, None), (4, None), (33, None)] {
            assert_eq!(found("5 0 R", &chain(n, plain, &mut refs)), want, "{n}");
            let of = "<< /ColorSpace 5 0 R >>";
            assert_eq!(found(of, &chain(n, spaces, &mut refs)), want, "{n}");
        }
        // Each two, the two together.
        let both = "<< /ColorSpace 7 0 R >>";
        assert_eq!(found("5 0 R", &["6 0 R", both, "8 0 R", spaces]), Some(1));
    }

    #[test]
    fn a_loop_of_resources_is_no_dictionary_and_no_error() {
        assert_eq!(found("5 0 R", &["5 0 R"]), None);
        assert_eq!(found("5 0 R", &["6 0 R", "5 0 R"]), None);
        assert_eq!(found("<< /ColorSpace 5 0 R >>", &["5 0 R"]), None);
        assert_eq!(found("<< /ColorSpace 5 0 R >>", &["6 0 R", "5 0 R"]), None);
    }

    #[test]
    fn resources_may_each_be_a_stream_s_dictionary() {
        let stream = "<< /Length 0 /ColorSpace << /X /G >> >>";
        assert_eq!(found("5 0 R", &[stream]), Some(1));
        assert_eq!(found("5 0 R", &["6 0 R", stream]), Some(1));
        let spaces = "<< /Length 0 /X /G >>";
        assert_eq!(found("<< /ColorSpace 5 0 R >>", &[spaces]), Some(2));
    }

    #[test]
    fn what_is_no_dictionary_is_none() {
        for resources in [
            "null",
            "/Foo",
            "7",
            "[]",
            "99 0 R",
            "<< >>",
            "<< /ColorSpace null >>",
        ] {
            assert_eq!(found(resources, &[]), None, "{resources}");
        }
        assert_eq!(found("<< /ColorSpace [] >>", &[]), None);
        assert_eq!(found("<< /ColorSpace 99 0 R >>", &[]), None);
        assert_eq!(found("<< /ColorSpace 5 0 R >>", &["null"]), None);
        assert_eq!(found("5 0 R", &["null"]), None);
        // Empty, it's there: a form with one doesn't use its page's.
        assert_eq!(found("<< /ColorSpace << >> >>", &[]), Some(0));
    }

    #[test]
    fn a_key_written_twice_is_its_last_and_each_read_is_the_same() {
        let resources = value("<< /ColorSpace << /X /DeviceGray /Y 7 /X /DeviceCMYK / /G >> >>");
        let nothing = |_: ObjectId| None;
        let spaces = Spaces::read(nothing, Some(&resources)).unwrap().unwrap();
        let keys: Vec<&[u8]> = spaces.iter().map(|named| named.key).collect();
        assert_eq!(keys, [&b"Y"[..], &b"X"[..]]);
        for named in spaces.iter() {
            assert_eq!(spaces.get(named.key), Some(named));
            assert!(named.key_at.is_some());
        }
        let x = spaces.get(b"X").unwrap();
        assert_eq!(x.value.as_name(), Some(&b"DeviceCMYK"[..]));
        assert_eq!(spaces.get(b"Z"), None);
        // An empty key PDFium drops as it reads it.
        assert_eq!(spaces.get(b""), None);
    }

    /// What `iter` gave as a filter over the dictionary's last entries: each
    /// name, an empty key none, in order.
    fn filtered<'o>(spaces: &Spaces<'o>) -> Vec<Named<'o>> {
        (spaces.dict.last_entries())
            .filter(|entry| !entry.key.is_empty())
            .map(|entry| Named {
                key: &entry.key,
                key_at: entry.key_at,
                value: &entry.value,
            })
            .collect()
    }

    #[test]
    fn iter_gives_each_name_in_order_and_backwards() {
        let shapes: [(&str, &[&str]); 5] = [
            // A key written twice, and an empty key.
            (
                "<< /ColorSpace << /X /DeviceGray /Y 7 /X /DeviceCMYK / /G >> >>",
                &[],
            ),
            // A stream's dictionary, its `/Length` among its names.
            (
                "<< /ColorSpace 5 0 R >>",
                &["<< /Length 0 /X /G /Y /RGB >>"],
            ),
            // Keys twice, empty keys first, between and last.
            (
                "<< /ColorSpace << / /G /A 1 /B 2 / /RGB /A 3 /C 4 /B 5 / /K >> >>",
                &[],
            ),
            // Empty keys alone, and none at all.
            ("<< /ColorSpace << / /G / /RGB >> >>", &[]),
            ("<< /ColorSpace << >> >>", &[]),
        ];
        for (text, extra) in shapes {
            let (resources, objects) = (value(text), objects(extra));
            let resolve = |id: ObjectId| objects.get(&id);
            let spaces = Spaces::read(resolve, Some(&resources)).unwrap().unwrap();
            let want = filtered(&spaces);
            let iter: Iter<'_> = spaces.iter();
            // At most every entry, so a caller can make room for them all.
            let entries = spaces.dict.entries().len();
            assert_eq!(iter.size_hint(), (0, Some(entries)), "{text}");
            let names: Vec<Named<'_>> = iter.clone().collect();
            assert_eq!(names, want, "{text}");
            let folded = iter.clone().fold(Vec::new(), |mut names, named| {
                names.push(named);
                names
            });
            assert_eq!(folded, want, "{text}");
            let mut back: Vec<Named<'_>> = iter.clone().rev().collect();
            back.reverse();
            assert_eq!(back, want, "{text}");
            // Fused, from either end.
            let mut done = iter.clone();
            done.by_ref().for_each(drop);
            assert_eq!((done.next(), done.next_back()), (None, None), "{text}");
        }
        // From both ends at once, each name once.
        let resources = value(shapes[2].0);
        let nothing = |_: ObjectId| None;
        let spaces = Spaces::read(nothing, Some(&resources)).unwrap().unwrap();
        let mut iter = spaces.iter();
        let key = |named: Option<Named<'_>>| named.map(|named| named.key.to_vec());
        assert_eq!(key(iter.next()), Some(b"A".to_vec()));
        assert_eq!(key(iter.next_back()), Some(b"B".to_vec()));
        // One entry left between them.
        assert_eq!(iter.size_hint(), (0, Some(1)));
        assert_eq!(key(iter.next()), Some(b"C".to_vec()));
        assert_eq!((key(iter.next()), key(iter.next_back())), (None, None));
    }

    /// What [`counted`] gives: `count`, and whether a cap was met for it.
    fn count_of(count: Option<u32>, capped: bool) -> Counted {
        Counted { count, capped }
    }

    /// A resolver over `objects` that fails for object 5, which is locked.
    #[derive(Clone, Copy)]
    struct LockedFive<'m>(&'m BTreeMap<ObjectId, Object>);

    impl<'m> Resolve<'m> for LockedFive<'m> {
        fn resolve(&self, id: ObjectId) -> Option<&'m Object> {
            self.0.get(&id)
        }

        fn try_resolve(&self, id: ObjectId) -> Result<Option<&'m Object>, Unreadable> {
            match id.number {
                5 => Err(Unreadable::new(id, Why::Locked, "it's locked")),
                _ => Ok(self.0.get(&id)),
            }
        }
    }

    #[test]
    fn components_counts_each_name_and_a_failed_one_ends_nothing() {
        let resources = value("<< /ColorSpace << /A [/ICCBased 6 0 R] /B 5 0 R /C /G >> >>");
        let objects = objects(&["/DeviceRGB", "<< /Length 0 /N 4 >>"]);
        let locked = LockedFive(&objects);
        let spaces = Spaces::read(locked, Some(&resources)).unwrap().unwrap();
        let components: Components<'_, LockedFive<'_>> =
            spaces.components(locked, &Limits::DEFAULT);
        assert_eq!(components.size_hint(), (0, Some(3)));
        // Its names left and its caps, and not its resolver.
        let debug = format!("{components:?}");
        let (names, limits) = (spaces.iter(), Limits::DEFAULT);
        let names_and_caps = format!("names: {names:?}, limits: {limits:?}");
        assert_eq!(debug, format!("Components {{ {names_and_caps}, .. }}"));
        let read: Vec<(&[u8], Result<Counted, Unreadable>)> = (components.clone())
            .map(|(named, counted)| (named.key, counted))
            .collect();
        assert_eq!(read.len(), 3);
        let sure = |count| Ok(count_of(Some(count), false));
        assert_eq!(read[0], (&b"A"[..], sure(4)));
        let (key, failed) = &read[1];
        let failed = failed.as_ref().unwrap_err();
        assert_eq!(
            (*key, failed.id, failed.why),
            (&b"B"[..], ObjectId::new(5, 0), Why::Locked)
        );
        assert_eq!(read[2], (&b"C"[..], sure(1)));
        // Each as `counted` gives it, and so are the names after it.
        for (named, counted) in components {
            let alone = crate::counted(locked, named.value, &Limits::DEFAULT);
            assert_eq!(counted, alone);
        }
        let mut components = spaces.components(locked, &Limits::DEFAULT);
        components.next();
        assert_eq!(components.size_hint(), (0, Some(2)));
        components.by_ref().for_each(drop);
        assert!(components.next().is_none());
    }

    #[test]
    fn components_says_which_names_met_a_cap() {
        let resources = value("<< /ColorSpace << /N [[/DeviceGray]] /G /DeviceGray >> >>");
        let nothing = |_: ObjectId| None;
        let spaces = Spaces::read(nothing, Some(&resources)).unwrap().unwrap();
        let mut limits = Limits::DEFAULT;
        limits.space_depth = 0;
        let read: Vec<(&[u8], Result<Counted, Unreadable>)> = (spaces.components(nothing, &limits))
            .map(|(named, counted)| (named.key, counted))
            .collect();
        let n = count_of(Some(3), true);
        let g = count_of(Some(1), false);
        assert_eq!(read, [(&b"N"[..], Ok(n)), (&b"G"[..], Ok(g))]);
        // Under the default caps, neither.
        let whole: Vec<Counted> = (spaces.components(nothing, &Limits::DEFAULT))
            .map(|(_, counted)| counted.unwrap())
            .collect();
        assert_eq!(whole, [count_of(Some(1), false), g]);
    }

    /// A resolver that fails each lookup for `why`, and counts them.
    struct Failing<'c> {
        why: Why,
        asked: &'c Cell<usize>,
    }

    impl<'a> Resolve<'a> for &Failing<'_> {
        fn resolve(&self, _: ObjectId) -> Option<&'a Object> {
            None
        }

        fn try_resolve(&self, id: ObjectId) -> Result<Option<&'a Object>, Unreadable> {
            self.asked.set(self.asked.get() + 1);
            Err(Unreadable::new(id, self.why, "it fails"))
        }
    }

    #[test]
    fn a_failed_read_is_nothing_there_if_damaged_and_otherwise_the_error() {
        for text in ["5 0 R", "<< /ColorSpace 5 0 R >>"] {
            let resources = value(text);
            let asked = Cell::new(0);
            let damaged = Failing {
                why: Why::Damaged,
                asked: &asked,
            };
            assert_eq!(Spaces::read(&damaged, Some(&resources)), Ok(None), "{text}");
            assert_eq!(asked.get(), 1);
            for why in [Why::Capped, Why::Locked, Why::Source] {
                let failing = Failing { why, asked: &asked };
                let error = Spaces::read(&failing, Some(&resources)).expect_err(text);
                assert_eq!((error.id, error.why), (ObjectId::new(5, 0), why), "{text}");
            }
        }
    }
}
