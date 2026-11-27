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
use pdf_objects::object::{Dict, Object, ObjectId, Value};
pub use pdf_objects::streams::{FILTERS, Unfiltered};
use pdf_objects::streams::{
    FilterChainError, FilterPredictor, FilterTail, PredictorLayout, StandardFilter,
    parse_filter_chain_with,
};

/// What follows a reference to the object it refers to, none for no
/// object; or none where references aren't followed, as in a
/// cross-reference stream's dictionary, whose values are direct (ISO
/// 32000-2, 7.5.8.2).
pub(crate) type Resolve<'r, 'a> = Option<&'r dyn Fn(ObjectId) -> Option<&'a Object>>;

/// Parsed `/Filter` and `/DecodeParms` pipeline for a stream dictionary
/// ([`read::filter_chain`](crate::read::filter_chain)).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FilterChain {
    /// Leading standard filters (`FlateDecode`, `LZWDecode`, `ASCIIHexDecode`, `ASCII85Decode`,
    /// `RunLengthDecode`), ready for [`pdf_filters::decode`].
    ///
    /// `/Crypt` filters are validated and elided here because stream decryption runs before filter
    /// decoding.
    pub standard: Vec<Filter>,
    /// First non-standard filter (such as `DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode`, or
    /// `JPXDecode`) and any filters listed after it, or `None` when every filter in the pipeline
    /// is standard.
    pub tail: Option<FilterTail>,
}

impl FilterChain {
    /// Creates a [`FilterChain`] from leading `standard` filters and optional `tail`.
    #[must_use]
    pub const fn new(standard: Vec<Filter>, tail: Option<FilterTail>) -> FilterChain {
        FilterChain { standard, tail }
    }
}

fn to_pdf_predictor(pred: FilterPredictor) -> Predictor {
    let to_layout = |l: PredictorLayout| Layout {
        colors: l.colors,
        bits_per_component: l.bits_per_component,
        columns: l.columns,
    };
    match pred {
        FilterPredictor::None => Predictor::None,
        FilterPredictor::Tiff(l) => Predictor::Tiff(to_layout(l)),
        FilterPredictor::Png(l) => Predictor::Png(to_layout(l)),
    }
}

fn to_pdf_filter(step: StandardFilter) -> Filter {
    match step {
        StandardFilter::Flate { predictor } => Filter::Flate {
            predictor: to_pdf_predictor(predictor),
        },
        StandardFilter::Lzw {
            early_change,
            predictor,
        } => Filter::Lzw {
            early_change,
            predictor: to_pdf_predictor(predictor),
        },
        StandardFilter::AsciiHex => Filter::AsciiHex,
        StandardFilter::Ascii85 => Filter::Ascii85,
        StandardFilter::RunLength => Filter::RunLength,
    }
}

/// Reads `dict`'s `/Filter` and `/DecodeParms` pipeline, stopping at the first non-standard
/// filter.
pub(crate) fn filter_chain<'a>(
    dict: &'a Dict,
    resolve: Resolve<'_, 'a>,
) -> Result<FilterChain, Unfiltered> {
    chain_inner(dict, resolve).map(|(chain, _)| chain)
}

/// The filters `dict`'s `/Filter` and `/DecodeParms` give, in order,
/// references followed by `resolve`: none for no `/Filter`, a null, or an
/// empty array; or why the reader doesn't decode them.
pub(crate) fn filters<'a>(
    dict: &'a Dict,
    resolve: Resolve<'_, 'a>,
) -> Result<Vec<Filter>, Unfiltered> {
    let (chain, unsupported) = chain_inner(dict, resolve)?;
    if let Some(raw) = unsupported {
        return Err(Unfiltered::Unsupported {
            filter: raw.to_vec(),
        });
    }
    Ok(chain.standard)
}

/// Parses `dict`'s filter chain and returns `(FilterChain, raw_first_unsupported_name)`.
pub(crate) fn chain_inner<'a>(
    dict: &'a Dict,
    resolve: Resolve<'_, 'a>,
) -> Result<(FilterChain, Option<&'a [u8]>), Unfiltered> {
    let wrapped = resolve.map(|f| move |id| Ok::<_, core::convert::Infallible>(f(id)));
    let resolver_ref = wrapped
        .as_ref()
        .map(|f| f as &dyn Fn(ObjectId) -> Result<Option<&'a Object>, core::convert::Infallible>);
    let (parsed, raw_tail) = match parse_filter_chain_with(dict, resolver_ref) {
        Ok(res) => res,
        Err(FilterChainError::Resolve(never)) => match never {},
        Err(FilterChainError::Unfiltered(u)) => return Err(u),
    };
    Ok((
        FilterChain {
            standard: parsed.standard.into_iter().map(to_pdf_filter).collect(),
            tail: parsed.tail,
        },
        raw_tail,
    ))
}

/// The name of the crypt filter a stream's own `/Crypt` filter names, when
/// its `/Filter` has one: its parameters' `/Name`, `/Identity` by default
/// (ISO 32000-2, Table 14).
pub(crate) fn crypt_filter<'a>(dict: &'a Dict, resolve: Resolve<'_, 'a>) -> Option<Vec<u8>> {
    let names: Vec<Option<&[u8]>> = match *resolved(dict.get(b"Filter")?, resolve).ok()??.value() {
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
    let name = (parms.get(at).copied().flatten()).and_then(|p| match p.value() {
        Value::Dict(d) => resolved(d.get(b"Name")?, resolve)
            .ok()?
            .and_then(Object::as_name),
        _ => None,
    });
    Some(name.unwrap_or(b"Identity").to_vec())
}

/// `dict`, a stream's, without its own `/Crypt` filter where that leaves its
/// data in the clear (its parameters' `/Name` `/Identity`, or none, ISO
/// 32000-2, Table 14): the filter taken out of `/Filter`, and its parameters
/// out of `/DecodeParms` where that's an array of one for each filter, each
/// of the two written direct, and both left out where no filter is left.
/// None where it has no such filter.
pub(crate) fn without_identity_crypt<'a>(dict: &'a Dict, resolve: Resolve<'_, 'a>) -> Option<Dict> {
    if crypt_filter(dict, resolve)? != b"Identity" {
        return None;
    }
    let mut out = dict.clone();
    let filters: &'a [Object] = match resolved(dict.get(b"Filter")?, resolve).ok()??.value() {
        Value::Array(array) => array,
        _ => &[],
    };
    let named = |o: &'a Object| {
        resolved(o, resolve)
            .ok()
            .flatten()
            .and_then(Object::as_name)
    };
    let at = filters.iter().position(|o| named(o) == Some(&b"Crypt"[..]));
    let parameters = (dict.get(b"DecodeParms"))
        .and_then(|parms| resolved(parms, resolve).ok().flatten())
        .map(Object::value);
    match at {
        Some(at) if filters.len() > 1 => {
            let mut filters = filters.to_vec();
            filters.remove(at);
            if let Some(Value::Array(parms)) = parameters
                && parms.len() == filters.len() + 1
            {
                let mut parms = parms.clone();
                parms.remove(at);
                out.set("DecodeParms", parms);
            }
            out.set("Filter", filters);
        }
        // The one filter, as a name or an array of one.
        _ => {
            out.remove(b"Filter");
            out.remove(b"DecodeParms");
        }
    }
    Some(out)
}

/// `o`, or what it refers to: references followed, as many as a chain of
/// them may take; none for a null, or a reference to no object.
fn resolved<'a>(
    mut o: &'a Object,
    resolve: Resolve<'_, 'a>,
) -> Result<Option<&'a Object>, Unfiltered> {
    for _ in 0..8 {
        match (o.value(), resolve) {
            (Value::Ref(_), None) => return Err(Unfiltered::Reference),
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
) -> Result<Vec<Option<&'a Object>>, Unfiltered> {
    let Some(parms) = dict.get(b"DecodeParms") else {
        return Ok(vec![None; count]);
    };
    let Some(parms) = resolved(parms, resolve)? else {
        return Ok(vec![None; count]);
    };
    Ok(match parms.value() {
        Value::Array(array) if array.is_empty() || count == 0 => vec![None; count],
        Value::Array(array) if array.len() != count => {
            return Err(Unfiltered::Entries {
                entries: array.len(),
                filters: count,
            });
        }
        Value::Array(array) => (array.iter())
            .map(|o| resolved(o, resolve))
            .collect::<Result<_, _>>()?,
        _ => vec![Some(parms); count],
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::dict;
    use pdf_objects::streams::FilterStep;

    use super::*;

    /// `dict`'s filters, a reference to no object, or what the reader says
    /// of them.
    fn read(dict: &Dict) -> Result<Vec<Filter>, String> {
        filters(dict, Some(&|_| None)).map_err(|why| why.to_string())
    }

    fn names(names: &[&str]) -> Object {
        names
            .iter()
            .map(|&n| Object::name(n))
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

    /// Each reason the reader doesn't take a stream's filters says it as
    /// its text did, by the same format string.
    #[test]
    fn each_reason_filters_arent_taken_is_said_as_they_were() {
        let name = b"DCTDecode";
        let shown = String::from_utf8_lossy(name);
        let key = "Columns";
        let (count, entries, n, bits) = (26, 2, -1, 3);
        let inner = Unfiltered::Integer { key };
        let cases = [
            (
                Unfiltered::Filter,
                "its /Filter is neither a name nor an array".to_string(),
            ),
            (
                Unfiltered::Names,
                "its /Filter's array isn't names".to_string(),
            ),
            (
                Unfiltered::Filters { count },
                format!("its /Filter lists {} filters, more than {FILTERS}", count),
            ),
            (
                Unfiltered::Unsupported {
                    filter: name.to_vec(),
                },
                format!("its filter /{shown} isn't one the reader decodes"),
            ),
            (
                Unfiltered::Unwanted {
                    filter: name.to_vec(),
                },
                format!("its /DecodeParms gives /{shown} parameters, which it takes none of"),
            ),
            (
                Unfiltered::Parameters {
                    filter: name.to_vec(),
                    why: Box::new(inner.clone()),
                },
                format!("/{shown}'s {inner}"),
            ),
            (
                Unfiltered::Entries {
                    entries,
                    filters: count,
                },
                format!("its /DecodeParms has {} entries for {count} filters", entries),
            ),
            (
                Unfiltered::Reference,
                "its /Filter or /DecodeParms holds a reference, which a cross-reference stream's may not".to_string(),
            ),
            (inner.clone(), format!("/{key} isn't an integer")),
            (
                Unfiltered::Predictor { value: n },
                format!("/Predictor is {n}, not 1, 2 or 10 to 15"),
            ),
            (
                Unfiltered::Count { key, value: n },
                format!("/{key} is {n}, not a count"),
            ),
            (Unfiltered::Zero, "/Colors or /Columns is 0".to_string()),
            (
                Unfiltered::Bits { bits },
                format!("/BitsPerComponent is {bits}, not 1, 2, 4, 8 or 16"),
            ),
            (
                Unfiltered::EarlyChange { value: n },
                format!("/EarlyChange is {n}, not 0 or 1"),
            ),
            (
                Unfiltered::NotADictionary,
                "parameters aren't a dictionary".to_string(),
            ),
            (
                Unfiltered::Key {
                    key: b"X".to_vec(),
                },
                format!("parameters have /{}", String::from_utf8_lossy(b"X")),
            ),
        ];
        for (unfiltered, said) in cases {
            assert_eq!(unfiltered.to_string(), said, "{unfiltered:?}");
        }
    }

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
                read(&dict! { "Filter" => Object::name(name) }),
                Ok(vec![filter]),
                "{name}"
            );
        }
        let why = read(&dict! { "Filter" => Object::name("DCTDecode") }).unwrap_err();
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
        let dict = dict! { "Filter" => Object::name("Fl"), "DecodeParms" => names(&[]) };
        assert_eq!(read(&dict), Ok(vec![FLATE]));
        let two: Vec<Object> = vec![Object::null(), Object::null()];
        let dict = dict! { "Filter" => Object::name("Fl"), "DecodeParms" => two };
        assert_eq!(
            read(&dict).unwrap_err(),
            "its /DecodeParms has 2 entries for 1 filters"
        );
        // A filter that takes none given some, even none at all.
        let dict = dict! { "Filter" => Object::name("AHx"), "DecodeParms" => dict! {} };
        let why = read(&dict).unwrap_err();
        assert_eq!(
            why,
            "its /DecodeParms gives /AHx parameters, which it takes none of"
        );
        // What isn't a dictionary, LZW and Flate take as the defaults.
        assert_eq!(
            read(&dict! { "Filter" => Object::name("Fl"), "DecodeParms" => 5 }),
            Ok(vec![FLATE])
        );
    }

    #[test]
    fn predictors_and_early_change_are_read_and_checked() {
        let with =
            |parms: Dict| read(&dict! { "Filter" => Object::name("LZW"), "DecodeParms" => parms });
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
                dict! { "Predictor" => Object::name("PNG") },
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
        let why = with(dict! { "Columns" => Object::name("many") }).unwrap_err();
        assert_eq!(why, "/LZW's /Columns isn't an integer");
    }

    #[test]
    fn crypt_with_the_identity_filter_is_no_filter() {
        let identity = dict! { "Name" => Object::name("Identity") };
        let parms: Vec<Object> = vec![identity.into(), Object::null()];
        let dict = dict! { "Filter" => names(&["Crypt", "Fl"]), "DecodeParms" => parms };
        assert_eq!(read(&dict), Ok(vec![FLATE]));
        assert_eq!(
            read(&dict! { "Filter" => Object::name("Crypt") }),
            Ok(vec![])
        );
        // Whatever its crypt filter: the streams the reader decodes aren't
        // encrypted. But parameters qpdf doesn't take, it doesn't.
        let other = dict! {
            "Filter" => Object::name("Crypt"),
            "DecodeParms" => dict! { "Name" => Object::name("StdCF") },
        };
        assert_eq!(read(&other), Ok(vec![]));
        let odd = dict! {
            "Filter" => Object::name("Crypt"),
            "DecodeParms" => dict! { "Name" => Object::name("StdCF"), "X" => 1 },
        };
        assert_eq!(read(&odd).unwrap_err(), "/Crypt's parameters have /X");
        let typed =
            dict! { "Type" => Object::name("CryptFilterDecodeParms"), "X" => Object::null() };
        assert_eq!(
            read(&dict! { "Filter" => Object::name("Crypt"), "DecodeParms" => typed }),
            Ok(vec![])
        );
    }

    #[test]
    fn references_are_followed_as_far_as_they_resolve() {
        let predicted = dict! { "Predictor" => Object::reference(9), "Columns" => 4 };
        let objects = BTreeMap::from([
            (ObjectId::new(7, 0), Object::name("FlateDecode")),
            (ObjectId::new(8, 0), Object::from(predicted)),
            (ObjectId::new(9, 0), Object::from(12)),
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
        assert_eq!(why, Unfiltered::Reference);
        assert_eq!(
            why.to_string(),
            "its /Filter or /DecodeParms holds a reference, which a cross-reference stream's may not"
        );
    }

    #[test]
    fn filter_chain_stops_at_first_non_standard_filter_and_agrees_with_filters() {
        // Pure standard chain has `tail: None` and matches `filters`.
        let predicted = dict! { "Predictor" => 12, "Columns" => 4 };
        let parms: Vec<Object> = vec![Object::null(), predicted.clone().into()];
        let pure = dict! { "Filter" => names(&["AHx", "Fl"]), "DecodeParms" => parms };
        assert_eq!(
            filter_chain(&pure, Some(&|_| None)),
            Ok(FilterChain::new(
                vec![
                    Filter::AsciiHex,
                    Filter::Flate {
                        predictor: png(1, 8, 4)
                    }
                ],
                None,
            ))
        );

        // Standard filter followed by DCTDecode with indirect parameters.
        let dct_parms = dict! { "ColorTransform" => 0 };
        let jbig2_parms = dict! { "JBIG2Globals" => Object::reference(15) };
        let objects = BTreeMap::from([
            (ObjectId::new(10, 0), Object::from(dct_parms.clone())),
            (ObjectId::new(12, 0), Object::from(jbig2_parms.clone())),
        ]);
        let resolve = |id| objects.get(&id);

        let parms: Vec<Object> = vec![predicted.into(), Object::reference(10)];
        let flate_dct =
            dict! { "Filter" => names(&["FlateDecode", "DCTDecode"]), "DecodeParms" => parms };
        assert_eq!(
            filter_chain(&flate_dct, Some(&resolve)),
            Ok(FilterChain::new(
                vec![Filter::Flate {
                    predictor: png(1, 8, 4)
                }],
                Some(FilterTail::new(
                    FilterStep::new("DCTDecode", Some(dct_parms)),
                    vec![],
                )),
            ))
        );

        // JBIG2Decode preserves indirect `/JBIG2Globals` inside resolved `parms`.
        let jbig2 = dict! {
            "Filter" => Object::name("JBIG2Decode"),
            "DecodeParms" => Object::reference(12),
        };
        let chain = filter_chain(&jbig2, Some(&resolve)).unwrap();
        assert!(chain.standard.is_empty());
        let tail = chain.tail.unwrap();
        assert_eq!(tail.first.name.as_bytes(), b"JBIG2Decode");
        assert_eq!(
            tail.first
                .parms
                .as_ref()
                .and_then(|d| d.get(b"JBIG2Globals"))
                .and_then(Object::as_reference),
            Some(ObjectId::new(15, 0))
        );
        assert!(tail.after.is_empty());

        // Inline abbreviations `CCF` and `DCT` expand in `first` and `after`, and `/Crypt` is
        // validated and elided before the codec.
        let inline_chain = dict! {
            "Filter" => names(&["Crypt", "CCF", "DCT", "Fl"]),
            "DecodeParms" => vec![
                Object::from(dict! { "Name" => Object::name("Identity") }),
                Object::from(dict! { "K" => -1 }),
                Object::null(),
                Object::null(),
            ],
        };
        let chain = filter_chain(&inline_chain, None).unwrap();
        assert!(chain.standard.is_empty());
        let tail = chain.tail.unwrap();
        assert_eq!(tail.first.name.as_bytes(), b"CCITTFaxDecode");
        assert_eq!(
            tail.first
                .parms
                .as_ref()
                .and_then(|d| d.get(b"K"))
                .and_then(Object::as_integer),
            Some(-1)
        );
        assert_eq!(
            tail.after,
            vec![
                FilterStep::new("DCTDecode", None),
                FilterStep::new("FlateDecode", None),
            ]
        );

        // Refusals on structural errors or invalid leading standard filter parameters match `filters`.
        let bad_leading = dict! {
            "Filter" => names(&["Fl", "DCTDecode"]),
            "DecodeParms" => vec![Object::from(dict! { "Predictor" => 99 }), Object::null()],
        };
        assert_eq!(
            filter_chain(&bad_leading, None).unwrap_err(),
            filters(&bad_leading, None).unwrap_err()
        );
        assert_eq!(filter_chain(&jbig2, None), Err(Unfiltered::Reference));
    }
}
