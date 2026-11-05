//! What a resources dictionary names (ISO 32000-2, 7.8.3): its `/ColorSpace`
//! dictionary, reached as PDFium reaches it.

use pdf_objects::{Dict, Object, Resolve, Span, Unreadable, Value};

use crate::count::look;

/// A resources dictionary's named color spaces: its `/ColorSpace` dictionary.
/// A key a reader of the file gave no bytes is no name: PDFium drops an empty
/// key as it parses it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spaces<'o> {
    dict: &'o Dict,
}

/// A name in a [`Spaces`]: a dictionary's entry, each part as it was read.
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
    /// of chromium/8125 there's no dictionary, and that's no error. An object
    /// that's [`Why::Damaged`](crate::Why::Damaged) isn't there, and any other
    /// failure to read one is the `Err`, whole.
    ///
    /// ```
    /// use pdf_color::{Limits, Object, ObjectId, Spaces, components, dict};
    ///
    /// let resources = Object::from(dict! { "ColorSpace" => dict! { "A" => "DeviceCMYK" } });
    /// let nothing = |_: ObjectId| None;
    /// let spaces = Spaces::read(nothing, Some(&resources)).unwrap().unwrap();
    /// let named = spaces.get(b"A").unwrap();
    /// assert_eq!(components(nothing, named.value, &Limits::DEFAULT), Ok(Some(4)));
    /// assert_eq!(Spaces::read(nothing, None), Ok(None));
    /// ```
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
    pub fn iter(&self) -> impl Iterator<Item = Named<'o>> + use<'o> {
        (self.dict.last_entries())
            .filter(|entry| !entry.key.is_empty())
            .map(|entry| Named {
                key: &entry.key,
                key_at: entry.key_at,
                value: &entry.value,
            })
    }

    /// The name `key`, its last entry if it's written twice: a color space a
    /// reader of content looks up, or `DefaultGray` and the like (8.6.5.6).
    pub fn get(&self, key: &[u8]) -> Option<Named<'o>> {
        if key.is_empty() {
            return None;
        }
        self.dict.entry(key).map(|entry| Named {
            key: &entry.key,
            key_at: entry.key_at,
            value: &entry.value,
        })
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
                ((n, 0), stream.unwrap_or(object))
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
                Object::string("abcdef"),
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
            ((1, 0), icc),
            ((2, 0), gray),
            ((3, 0), spaces),
            ((4, 0), function),
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
            for why in [Why::Limit, Why::Locked, Why::Source] {
                let failing = Failing { why, asked: &asked };
                let error = Spaces::read(&failing, Some(&resources)).expect_err(text);
                assert_eq!((error.id, error.why), ((5, 0), why), "{text}");
            }
        }
    }
}
