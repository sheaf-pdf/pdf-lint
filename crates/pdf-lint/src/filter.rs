//! A stream's `/Filter` and `/DecodeParms`, read as [`pdf_filters`]
//! takes them. They're read as qpdf reads them (ISO 32000-2, 7.4, leaves
//! much open): `/Fl`, `/LZW`, `/AHx`, `/A85` and `/RL`, the inline image
//! abbreviations, name filters on any stream; `/Crypt` is no filter, as
//! the streams the reader decodes aren't encrypted; `/DecodeParms` that
//! isn't an array is every filter's, and an empty array none; a filter
//! that takes no parameters given some, or parameters that aren't a
//! filter's, isn't decoded; a reference to no object is null (7.3.10).
//! Where qpdf departs from the spec, the spec: TIFF's predictor takes 1,
//! 2, 4, 8 or 16 bits a sample.

use pdf_filters::{Filter, Layout, Predictor};

use crate::object::{Dict, Object, ObjectId, Value};

/// The most filters a stream may list, as qpdf allows.
pub const FILTERS: usize = 25;

/// What follows a reference to the object it refers to, none for no
/// object; or none where references aren't followed, as in a
/// cross-reference stream's dictionary, whose values are direct (ISO
/// 32000-2, 7.5.8.2).
pub(crate) type Resolve<'r, 'a> = Option<&'r dyn Fn(ObjectId) -> Option<&'a Object>>;

/// The filters `dict`'s `/Filter` and `/DecodeParms` give, in order,
/// references followed by `resolve`: none for no `/Filter`, a null, or an
/// empty array; or why the reader doesn't decode them.
pub(crate) fn filters<'a>(dict: &'a Dict, resolve: Resolve<'_, 'a>) -> Result<Vec<Filter>, String> {
    let names: Vec<&[u8]> = match dict
        .get(b"Filter")
        .map(|o| resolved(o, resolve))
        .transpose()?
    {
        None | Some(None) => Vec::new(),
        Some(Some(filter)) => match &filter.value {
            Value::Name(name) => vec![&name[..]],
            Value::Array(array) => (array.iter())
                .map(|o| Ok(resolved(o, resolve)?.and_then(Object::as_name)))
                .collect::<Result<Option<_>, String>>()?
                .ok_or("its /Filter's array isn't names")?,
            _ => return Err("its /Filter is neither a name nor an array".to_string()),
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
        let named = |why: String| format!("/{shown}'s {why}");
        filters.push(match name {
            b"FlateDecode" | b"Fl" => Filter::Flate {
                predictor: predictor(parms, resolve).map_err(named)?,
            },
            b"LZWDecode" | b"LZW" => Filter::Lzw {
                early_change: early_change(parms, resolve).map_err(named)?,
                predictor: predictor(parms, resolve).map_err(named)?,
            },
            b"ASCIIHexDecode" | b"AHx" => none(Filter::AsciiHex)?,
            b"ASCII85Decode" | b"A85" => none(Filter::Ascii85)?,
            b"RunLengthDecode" | b"RL" => none(Filter::RunLength)?,
            b"Crypt" => {
                crypt(parms, resolve).map_err(named)?;
                continue;
            }
            _ => return Err(format!("its filter /{shown} isn't one the reader decodes")),
        });
    }
    Ok(filters)
}

/// The name of the crypt filter a stream's own `/Crypt` filter names, when
/// its `/Filter` has one: its parameters' `/Name`, `/Identity` by default
/// (ISO 32000-2, Table 14).
pub(crate) fn crypt_filter<'a>(dict: &'a Dict, resolve: Resolve<'_, 'a>) -> Option<Vec<u8>> {
    let names: Vec<Option<&[u8]>> = match resolved(dict.get(b"Filter")?, resolve).ok()??.value {
        Value::Name(ref name) => vec![Some(&name[..])],
        Value::Array(ref array) => (array.iter())
            .map(|o| {
                resolved(o, resolve)
                    .ok()
                    .flatten()
                    .and_then(Object::as_name)
            })
            .collect(),
        _ => return None,
    };
    let at = names.iter().position(|n| *n == Some(&b"Crypt"[..]))?;
    let parms = parameters(dict, names.len(), resolve).ok()?;
    let name = (parms.get(at).copied().flatten()).and_then(|p| match &p.value {
        Value::Dict(d) => resolved(d.get(b"Name")?, resolve)
            .ok()?
            .and_then(Object::as_name),
        _ => None,
    });
    Some(name.unwrap_or(b"Identity").to_vec())
}

/// `o`, or what it refers to: references followed, as many as a chain of
/// them may take; none for a null, or a reference to no object.
fn resolved<'a>(mut o: &'a Object, resolve: Resolve<'_, 'a>) -> Result<Option<&'a Object>, String> {
    for _ in 0..8 {
        match (&o.value, resolve) {
            (Value::Ref(_), None) => {
                return Err("its /Filter or /DecodeParms holds a reference, which a cross-reference stream's may not".to_string());
            }
            (Value::Ref(id), Some(resolve)) => match resolve(*id) {
                Some(to) => o = to,
                None => return Ok(None),
            },
            (Value::Null, _) => return Ok(None),
            _ => return Ok(Some(o)),
        }
    }
    Ok(None)
}

/// Each of `count` filters' parameters, none for a null: `/DecodeParms`'
/// array, each its own, or what isn't an array, every filter's.
fn parameters<'a>(
    dict: &'a Dict,
    count: usize,
    resolve: Resolve<'_, 'a>,
) -> Result<Vec<Option<&'a Object>>, String> {
    let Some(parms) = dict.get(b"DecodeParms") else {
        return Ok(vec![None; count]);
    };
    let Some(parms) = resolved(parms, resolve)? else {
        return Ok(vec![None; count]);
    };
    Ok(match &parms.value {
        Value::Array(array) if array.is_empty() || count == 0 => vec![None; count],
        Value::Array(array) if array.len() != count => {
            return Err(format!(
                "its /DecodeParms has {} entries for {count} filters",
                array.len()
            ));
        }
        Value::Array(array) => (array.iter())
            .map(|o| resolved(o, resolve))
            .collect::<Result<_, _>>()?,
        _ => vec![Some(parms); count],
    })
}

/// A parameter of `parms` that's an integer, resolved; none when it's
/// absent or null, or `parms` isn't a dictionary.
fn integer<'a>(
    parms: Option<&'a Object>,
    key: &str,
    resolve: Resolve<'_, 'a>,
) -> Result<Option<i64>, String> {
    let Some(Value::Dict(dict)) = parms.map(|o| &o.value) else {
        return Ok(None);
    };
    let Some(value) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    match resolved(value, resolve)?.map(|o| &o.value) {
        None => Ok(None),
        Some(Value::Integer(n)) => Ok(Some(*n)),
        Some(_) => Err(format!("/{key} isn't an integer")),
    }
}

/// What an LZW or Flate filter's `/Predictor`, `/Colors`,
/// `/BitsPerComponent` and `/Columns` say, with the spec's defaults; a
/// parameters value that isn't a dictionary, the defaults all, as in
/// qpdf. The layout's values are integers, and with a predictor, a
/// layout.
fn predictor<'a>(parms: Option<&'a Object>, resolve: Resolve<'_, 'a>) -> Result<Predictor, String> {
    let colors = integer(parms, "Colors", resolve)?;
    let bits = integer(parms, "BitsPerComponent", resolve)?;
    let columns = integer(parms, "Columns", resolve)?;
    let png = match integer(parms, "Predictor", resolve)? {
        None | Some(1) => return Ok(Predictor::None),
        Some(2) => false,
        Some(10..=15) => true,
        Some(n) => return Err(format!("/Predictor is {n}, not 1, 2 or 10 to 15")),
    };
    let count = |key: &str, value: Option<i64>, default: u32| -> Result<u32, String> {
        match value {
            None => Ok(default),
            Some(n) => u32::try_from(n).map_err(|_| format!("/{key} is {n}, not a count")),
        }
    };
    let layout = Layout {
        colors: count("Colors", colors, 1)?,
        bits_per_component: count("BitsPerComponent", bits, 8)?,
        columns: count("Columns", columns, 1)?,
    };
    if layout.colors == 0 || layout.columns == 0 {
        return Err("/Colors or /Columns is 0".to_string());
    }
    if !matches!(layout.bits_per_component, 1 | 2 | 4 | 8 | 16) {
        let bits = layout.bits_per_component;
        return Err(format!("/BitsPerComponent is {bits}, not 1, 2, 4, 8 or 16"));
    }
    Ok(match png {
        true => Predictor::Png(layout),
        false => Predictor::Tiff(layout),
    })
}

/// An LZW filter's `/EarlyChange`: 1 by default, and 0 or 1.
fn early_change<'a>(parms: Option<&'a Object>, resolve: Resolve<'_, 'a>) -> Result<bool, String> {
    match integer(parms, "EarlyChange", resolve)? {
        None | Some(1) => Ok(true),
        Some(0) => Ok(false),
        Some(n) => Err(format!("/EarlyChange is {n}, not 0 or 1")),
    }
}

/// Whether a `/Crypt` filter's parameters are ones qpdf takes: none, or
/// a dictionary of `/Name` and `/Type /CryptFilterDecodeParms`, others
/// null.
fn crypt<'a>(parms: Option<&'a Object>, resolve: Resolve<'_, 'a>) -> Result<(), String> {
    let Some(parms) = parms else {
        return Ok(());
    };
    let Value::Dict(dict) = &parms.value else {
        return Err("parameters aren't a dictionary".to_string());
    };
    for (key, value) in dict.iter() {
        let value = resolved(value, resolve)?;
        let taken = match key {
            b"Name" => true,
            b"Type" => value.is_none_or(|v| v.as_name() == Some(b"CryptFilterDecodeParms")),
            _ => value.is_none(),
        };
        if !taken {
            return Err(format!("parameters have /{}", String::from_utf8_lossy(key)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::dict;

    /// `dict`'s filters, a reference to no object.
    fn read(dict: &Dict) -> Result<Vec<Filter>, String> {
        filters(dict, Some(&|_| None))
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
        // Without a predictor, a layout that isn't one doesn't matter, but
        // its values must be integers, as in qpdf.
        let lzw = Filter::Lzw {
            early_change: true,
            predictor: Predictor::None,
        };
        assert_eq!(with(dict! { "BitsPerComponent" => 3 }), Ok(vec![lzw]));
        assert_eq!(
            with(dict! { "Columns" => -1, "Colors" => 5_000_000_000i64 }),
            Ok(vec![lzw])
        );
        let why = with(dict! { "Columns" => "many" }).unwrap_err();
        assert_eq!(why, "/LZW's /Columns isn't an integer");
    }

    #[test]
    fn crypt_with_the_identity_filter_is_no_filter() {
        let identity = dict! { "Name" => "Identity" };
        let parms: Vec<Object> = vec![identity.into(), Object::null()];
        let dict = dict! { "Filter" => names(&["Crypt", "Fl"]), "DecodeParms" => parms };
        assert_eq!(read(&dict), Ok(vec![FLATE]));
        assert_eq!(read(&dict! { "Filter" => "Crypt" }), Ok(vec![]));
        // Whatever its crypt filter: the streams the reader decodes aren't
        // encrypted. But parameters qpdf doesn't take, it doesn't.
        let other = dict! { "Filter" => "Crypt", "DecodeParms" => dict! { "Name" => "StdCF" } };
        assert_eq!(read(&other), Ok(vec![]));
        let odd =
            dict! { "Filter" => "Crypt", "DecodeParms" => dict! { "Name" => "StdCF", "X" => 1 } };
        assert_eq!(read(&odd).unwrap_err(), "/Crypt's parameters have /X");
        let typed = dict! { "Type" => "CryptFilterDecodeParms", "X" => Object::null() };
        assert_eq!(
            read(&dict! { "Filter" => "Crypt", "DecodeParms" => typed }),
            Ok(vec![])
        );
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
        assert_eq!(filters(&dict, Some(&resolve)), Ok(vec![flate]));
        // A reference to no object is null (ISO 32000-2, 7.3.10).
        let dict = dict! { "Filter" => Object::reference(6) };
        assert_eq!(filters(&dict, Some(&resolve)), Ok(vec![]));
        let dict =
            dict! { "Filter" => Object::reference(7), "DecodeParms" => Object::reference(6) };
        assert_eq!(filters(&dict, Some(&resolve)), Ok(vec![FLATE]));
        // Where references aren't followed, as in a cross-reference
        // stream's dictionary, one isn't read.
        let why = filters(&dict, None).unwrap_err();
        assert_eq!(
            why,
            "its /Filter or /DecodeParms holds a reference, which a cross-reference stream's may not"
        );
    }
}
