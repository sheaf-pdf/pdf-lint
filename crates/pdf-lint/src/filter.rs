//! A stream's `/Filter` and `/DecodeParms`, read as [`pdf_filters`]
//! takes them. They're read as qpdf reads them (ISO 32000-2, 7.4, leaves
//! much open): `/Fl`, `/LZW`, `/AHx`, `/A85` and `/RL`, the inline image
//! abbreviations, name filters on any stream; `/Crypt` with the identity
//! filter is no filter; `/DecodeParms` that isn't an array is every
//! filter's, and an empty array none; a filter that takes no parameters
//! given some, or parameters that aren't a filter's, isn't decoded. Where
//! qpdf departs from the spec, the spec: TIFF's predictor takes 1, 2, 4, 8
//! or 16 bits a sample.

use pdf_filters::{Filter, Layout, Predictor};

use crate::object::{Dict, Object, ObjectId, Value};

/// The most filters a stream may list, as qpdf allows.
pub const FILTERS: usize = 25;

/// The filters `dict`'s `/Filter` and `/DecodeParms` give, in order, with
/// `resolve` giving the object a reference refers to (none when it
/// can't): none for no `/Filter`, a null, or an empty array; or why the
/// reader doesn't decode them.
pub(crate) fn filters<'a>(
    dict: &'a Dict,
    resolve: &dyn Fn(ObjectId) -> Option<&'a Object>,
) -> Result<Vec<Filter>, String> {
    let names: Vec<&[u8]> = match dict.get(b"Filter") {
        None => Vec::new(),
        Some(filter) => match resolved(filter, resolve).map(|o| &o.value) {
            None => return Err("its /Filter refers to no object it can read".to_string()),
            Some(Value::Null) => Vec::new(),
            Some(Value::Name(name)) => vec![&name[..]],
            Some(Value::Array(array)) => (array.iter())
                .map(|o| resolved(o, resolve).and_then(Object::as_name))
                .collect::<Option<_>>()
                .ok_or("its /Filter's array isn't names")?,
            Some(_) => return Err("its /Filter is neither a name nor an array".to_string()),
        },
    };
    if names.len() > FILTERS {
        return Err(format!(
            "its /Filter lists {} filters, more than {FILTERS}",
            names.len()
        ));
    }
    let parms = parameters(dict, names.len(), resolve)?;
    let mut filters = Vec::new();
    for (name, parms) in names.into_iter().zip(parms) {
        let shown = String::from_utf8_lossy(name);
        let none = |filter: Filter| match parms {
            None => Ok(filter),
            Some(_) => Err(format!(
                "its /DecodeParms gives /{shown} parameters, which it takes none of"
            )),
        };
        filters.push(match name {
            b"FlateDecode" | b"Fl" => Filter::Flate {
                predictor: predictor(parms, resolve).map_err(|why| format!("/{shown}'s {why}"))?,
            },
            b"LZWDecode" | b"LZW" => Filter::Lzw {
                early_change: early_change(parms, resolve)
                    .map_err(|why| format!("/{shown}'s {why}"))?,
                predictor: predictor(parms, resolve).map_err(|why| format!("/{shown}'s {why}"))?,
            },
            b"ASCIIHexDecode" | b"AHx" => none(Filter::AsciiHex)?,
            b"ASCII85Decode" | b"A85" => none(Filter::Ascii85)?,
            b"RunLengthDecode" | b"RL" => none(Filter::RunLength)?,
            b"Crypt" => match crypt_filter(parms, resolve) {
                Some(b"Identity") | None => continue,
                Some(other) => {
                    let other = String::from_utf8_lossy(other);
                    return Err(format!(
                        "its /Crypt filter is /{other}, which needs decrypting"
                    ));
                }
            },
            _ => return Err(format!("its filter /{shown} isn't one the reader decodes")),
        });
    }
    Ok(filters)
}

/// `o`, or what it refers to: references followed, as many as a chain of
/// them may take, unless one can't be.
fn resolved<'a>(
    mut o: &'a Object,
    resolve: &dyn Fn(ObjectId) -> Option<&'a Object>,
) -> Option<&'a Object> {
    for _ in 0..8 {
        match o.value {
            Value::Ref(id) => o = resolve(id)?,
            _ => return Some(o),
        }
    }
    None
}

/// Each of `count` filters' parameters, none for a null: `/DecodeParms`'
/// array, each its own, or what isn't an array, every filter's.
fn parameters<'a>(
    dict: &'a Dict,
    count: usize,
    resolve: &dyn Fn(ObjectId) -> Option<&'a Object>,
) -> Result<Vec<Option<&'a Object>>, String> {
    let Some(parms) = dict.get(b"DecodeParms") else {
        return Ok(vec![None; count]);
    };
    let not_null = |o: Option<&'a Object>| o.filter(|o| !matches!(o.value, Value::Null));
    let parms =
        resolved(parms, resolve).ok_or("its /DecodeParms refers to no object it can read")?;
    Ok(match &parms.value {
        Value::Array(array) if array.is_empty() || count == 0 => vec![None; count],
        Value::Array(array) if array.len() != count => {
            return Err(format!(
                "its /DecodeParms has {} entries for {count} filters",
                array.len()
            ));
        }
        Value::Array(array) => array
            .iter()
            .map(|o| not_null(resolved(o, resolve)))
            .collect(),
        _ => vec![not_null(Some(parms)); count],
    })
}

/// A parameter of `parms` that's an integer, resolved; none when it's
/// absent or `parms` isn't a dictionary.
fn integer<'a>(
    parms: Option<&'a Object>,
    key: &str,
    resolve: &dyn Fn(ObjectId) -> Option<&'a Object>,
) -> Result<Option<i64>, String> {
    let Some(Value::Dict(dict)) = parms.map(|o| &o.value) else {
        return Ok(None);
    };
    match dict.get(key.as_bytes()).map(|o| resolved(o, resolve)) {
        None
        | Some(Some(Object {
            value: Value::Null, ..
        })) => Ok(None),
        Some(Some(Object {
            value: Value::Integer(n),
            ..
        })) => Ok(Some(*n)),
        Some(_) => Err(format!("/{key} isn't an integer")),
    }
}

/// What an LZW or Flate filter's `/Predictor`, `/Colors`,
/// `/BitsPerComponent` and `/Columns` say, with the spec's defaults; a
/// parameters value that isn't a dictionary, the defaults all, as in
/// qpdf.
fn predictor<'a>(
    parms: Option<&'a Object>,
    resolve: &dyn Fn(ObjectId) -> Option<&'a Object>,
) -> Result<Predictor, String> {
    let count = |key: &str, default: u32| -> Result<u32, String> {
        match integer(parms, key, resolve)? {
            None => Ok(default),
            Some(n) => u32::try_from(n).map_err(|_| format!("/{key} is {n}, not a count")),
        }
    };
    let layout = Layout {
        colors: count("Colors", 1)?,
        bits_per_component: count("BitsPerComponent", 8)?,
        columns: count("Columns", 1)?,
    };
    let predictor = match integer(parms, "Predictor", resolve)? {
        None | Some(1) => return Ok(Predictor::None),
        Some(2) => Predictor::Tiff(layout),
        Some(10..=15) => Predictor::Png(layout),
        Some(n) => return Err(format!("/Predictor is {n}, not 1, 2 or 10 to 15")),
    };
    if layout.colors == 0 || layout.columns == 0 {
        return Err("/Colors or /Columns is 0".to_string());
    }
    if !matches!(layout.bits_per_component, 1 | 2 | 4 | 8 | 16) {
        let bits = layout.bits_per_component;
        return Err(format!("/BitsPerComponent is {bits}, not 1, 2, 4, 8 or 16"));
    }
    Ok(predictor)
}

/// An LZW filter's `/EarlyChange`: 1 by default, and 0 or 1.
fn early_change<'a>(
    parms: Option<&'a Object>,
    resolve: &dyn Fn(ObjectId) -> Option<&'a Object>,
) -> Result<bool, String> {
    match integer(parms, "EarlyChange", resolve)? {
        None | Some(1) => Ok(true),
        Some(0) => Ok(false),
        Some(n) => Err(format!("/EarlyChange is {n}, not 0 or 1")),
    }
}

/// A `/Crypt` filter's `/Name`, when its parameters give one.
fn crypt_filter<'a>(
    parms: Option<&'a Object>,
    resolve: &dyn Fn(ObjectId) -> Option<&'a Object>,
) -> Option<&'a [u8]> {
    let Some(Value::Dict(dict)) = parms.map(|o| &o.value) else {
        return None;
    };
    resolved(dict.get(b"Name")?, resolve).and_then(Object::as_name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::dict;

    fn read(dict: &Dict) -> Result<Vec<Filter>, String> {
        filters(dict, &|_| None)
    }

    fn names(names: &[&str]) -> Object {
        names
            .iter()
            .map(|&n| Object::from(n))
            .collect::<Vec<_>>()
            .into()
    }

    fn png(colors: u32, bits_per_component: u32, columns: u32) -> Predictor {
        Predictor::Png(Layout {
            colors,
            bits_per_component,
            columns,
        })
    }

    const FLATE: Filter = Filter::Flate {
        predictor: Predictor::None,
    };

    #[test]
    fn no_filter_is_none_a_null_or_an_empty_array() {
        assert_eq!(read(&dict! {}), Ok(vec![]));
        assert_eq!(read(&dict! { "Filter" => Object::null() }), Ok(vec![]));
        assert_eq!(read(&dict! { "Filter" => names(&[]) }), Ok(vec![]));
        // Parameters for no filters are none.
        let parms: Vec<Object> = vec![dict! {}.into()];
        assert_eq!(
            read(&dict! { "Filter" => names(&[]), "DecodeParms" => parms }),
            Ok(vec![])
        );
    }

    #[test]
    fn names_and_their_abbreviations_name_filters() {
        let lzw = Filter::Lzw {
            early_change: true,
            predictor: Predictor::None,
        };
        let all = [
            ("FlateDecode", FLATE),
            ("Fl", FLATE),
            ("LZWDecode", lzw),
            ("LZW", lzw),
            ("ASCIIHexDecode", Filter::AsciiHex),
            ("AHx", Filter::AsciiHex),
            ("ASCII85Decode", Filter::Ascii85),
            ("A85", Filter::Ascii85),
            ("RunLengthDecode", Filter::RunLength),
            ("RL", Filter::RunLength),
        ];
        for (name, filter) in all {
            assert_eq!(
                read(&dict! { "Filter" => name }),
                Ok(vec![filter]),
                "{name}"
            );
        }
        let why = read(&dict! { "Filter" => "DCTDecode" }).unwrap_err();
        assert_eq!(why, "its filter /DCTDecode isn't one the reader decodes");
        let why = read(&dict! { "Filter" => 1 }).unwrap_err();
        assert_eq!(why, "its /Filter is neither a name nor an array");
        let why = read(&dict! { "Filter" => vec![Object::from(1)] }).unwrap_err();
        assert_eq!(why, "its /Filter's array isn't names");
        let many = vec!["Fl"; FILTERS + 1];
        let why = read(&dict! { "Filter" => names(&many) }).unwrap_err();
        assert!(why.contains("more than 25"), "{why}");
    }

    #[test]
    fn parameters_are_an_arrays_each_or_one_for_every_filter() {
        let predicted = dict! { "Predictor" => 12, "Columns" => 4 };
        let parms: Vec<Object> = vec![Object::null(), predicted.clone().into()];
        let dict = dict! { "Filter" => names(&["AHx", "Fl"]), "DecodeParms" => parms };
        let flate = Filter::Flate {
            predictor: png(1, 8, 4),
        };
        assert_eq!(read(&dict), Ok(vec![Filter::AsciiHex, flate]));
        // One dictionary, every filter's.
        let dict = dict! { "Filter" => names(&["Fl", "Fl"]), "DecodeParms" => predicted };
        assert_eq!(read(&dict), Ok(vec![flate, flate]));
        // An empty array, none; another length, not decoded.
        let dict = dict! { "Filter" => "Fl", "DecodeParms" => names(&[]) };
        assert_eq!(read(&dict), Ok(vec![FLATE]));
        let two: Vec<Object> = vec![Object::null(), Object::null()];
        let dict = dict! { "Filter" => "Fl", "DecodeParms" => two };
        assert_eq!(
            read(&dict).unwrap_err(),
            "its /DecodeParms has 2 entries for 1 filters"
        );
        // A filter that takes none given some, even none at all.
        let dict = dict! { "Filter" => "AHx", "DecodeParms" => dict! {} };
        let why = read(&dict).unwrap_err();
        assert_eq!(
            why,
            "its /DecodeParms gives /AHx parameters, which it takes none of"
        );
        // What isn't a dictionary, LZW and Flate take as the defaults.
        assert_eq!(
            read(&dict! { "Filter" => "Fl", "DecodeParms" => 5 }),
            Ok(vec![FLATE])
        );
    }

    #[test]
    fn predictors_and_early_change_are_read_and_checked() {
        let with = |parms: Dict| read(&dict! { "Filter" => "LZW", "DecodeParms" => parms });
        let tiff = Predictor::Tiff(Layout {
            colors: 3,
            bits_per_component: 16,
            columns: 7,
        });
        let parms = dict! {
            "Predictor" => 2, "Colors" => 3, "BitsPerComponent" => 16, "Columns" => 7, "EarlyChange" => 0,
        };
        let lzw = Filter::Lzw {
            early_change: false,
            predictor: tiff,
        };
        assert_eq!(with(parms), Ok(vec![lzw]));
        let bad = [
            (
                dict! { "EarlyChange" => 2 },
                "/LZW's /EarlyChange is 2, not 0 or 1",
            ),
            (
                dict! { "Predictor" => 3 },
                "/LZW's /Predictor is 3, not 1, 2 or 10 to 15",
            ),
            (
                dict! { "Predictor" => "PNG" },
                "/LZW's /Predictor isn't an integer",
            ),
            (
                dict! { "Predictor" => 12, "Columns" => -1 },
                "/LZW's /Columns is -1, not a count",
            ),
            (
                dict! { "Predictor" => 12, "Columns" => 0 },
                "/LZW's /Colors or /Columns is 0",
            ),
            (
                dict! { "Predictor" => 2, "BitsPerComponent" => 3 },
                "/LZW's /BitsPerComponent is 3, not 1, 2, 4, 8 or 16",
            ),
        ];
        for (parms, why) in bad {
            assert_eq!(with(parms).unwrap_err(), why);
        }
        // Without a predictor, a layout that isn't one doesn't matter.
        let lzw = Filter::Lzw {
            early_change: true,
            predictor: Predictor::None,
        };
        assert_eq!(with(dict! { "BitsPerComponent" => 3 }), Ok(vec![lzw]));
    }

    #[test]
    fn crypt_with_the_identity_filter_is_no_filter() {
        let identity = dict! { "Name" => "Identity" };
        let parms: Vec<Object> = vec![identity.into(), Object::null()];
        let dict = dict! { "Filter" => names(&["Crypt", "Fl"]), "DecodeParms" => parms };
        assert_eq!(read(&dict), Ok(vec![FLATE]));
        assert_eq!(read(&dict! { "Filter" => "Crypt" }), Ok(vec![]));
        let other = dict! { "Filter" => "Crypt", "DecodeParms" => dict! { "Name" => "StdCF" } };
        let why = read(&other).unwrap_err();
        assert_eq!(why, "its /Crypt filter is /StdCF, which needs decrypting");
    }

    #[test]
    fn references_are_followed_as_far_as_they_resolve() {
        let predicted = dict! { "Predictor" => Object::reference(9), "Columns" => 4 };
        let objects = BTreeMap::from([
            ((7, 0), Object::from("FlateDecode")),
            ((8, 0), Object::from(predicted)),
            ((9, 0), Object::from(12)),
        ]);
        let resolve = |id| objects.get(&id);
        let filter: Vec<Object> = vec![Object::reference(7)];
        let dict = dict! { "Filter" => filter, "DecodeParms" => Object::reference(8) };
        let flate = Filter::Flate {
            predictor: png(1, 8, 4),
        };
        assert_eq!(filters(&dict, &resolve), Ok(vec![flate]));
        let dict = dict! { "Filter" => Object::reference(6) };
        let why = filters(&dict, &resolve).unwrap_err();
        assert_eq!(why, "its /Filter refers to no object it can read");
    }
}
