//! Standard filter-chain decoding for inline images (`BI..ID..EI`).
//!
//! Indirect Image XObjects obtain leading standard-filter output via
//! [`Streams::decoded_standard`](pdf_objects::Streams::decoded_standard). Inline images live inside
//! content-stream slices rather than indirect streams, so this module parses their expanded
//! `/Filter` and `/DecodeParms` entries and runs [`pdf_filters::decode_as`] up to the first image
//! codec filter, producing an equivalent [`DecodedStream`](pdf_objects::DecodedStream).

use std::borrow::Cow;

use pdf_filters::{Backend, Filter, Layout, Predictor, Reading, decode_as};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::streams::{DecodedStream, FilterStep, FilterTail};
use pdf_objects::{Dict, Resolve};

use crate::error::Error;
use crate::fault::Fault;
use crate::header::resolve_obj;
use crate::limits::LimitKind;

const MAX_FILTERS: usize = 25;

fn expand_filter_name(name: &[u8]) -> &[u8] {
    match name {
        b"AHx" => b"ASCIIHexDecode",
        b"A85" => b"ASCII85Decode",
        b"LZW" => b"LZWDecode",
        b"Fl" => b"FlateDecode",
        b"RL" => b"RunLengthDecode",
        b"CCF" => b"CCITTFaxDecode",
        b"DCT" => b"DCTDecode",
        other => other,
    }
}

/// Decodes `raw_bytes` through any leading standard filters declared in `dict`, stopping at the
/// first non-standard filter (`DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode`, `JPXDecode`, etc.).
pub(crate) fn decode_inline_standard<'a, 'o, R, S>(
    objects: R,
    dict: &'o Dict,
    raw_bytes: &'a [u8],
    limit: usize,
    reading: Reading,
    sink: &mut S,
) -> Result<DecodedStream<'a>, Error>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let names: Vec<&'o [u8]> = match dict
        .get(b"Filter")
        .map(|o| resolve_obj(objects, o))
        .transpose()?
        .flatten()
    {
        None => Vec::new(),
        Some(obj) if obj.is_null() => Vec::new(),
        Some(obj) => {
            if let Some(name) = obj.as_name() {
                vec![name]
            } else if let Some(arr) = obj.as_array() {
                let mut out = Vec::with_capacity(arr.len());
                for item in arr {
                    let Some(resolved) = resolve_obj(objects, item)? else {
                        return Err(Error::UnsupportedFilter {
                            filter: b"<null>".to_vec(),
                        });
                    };
                    let Some(name) = resolved.as_name() else {
                        return Err(Error::UnsupportedFilter {
                            filter: b"<non-name>".to_vec(),
                        });
                    };
                    out.push(name);
                }
                out
            } else {
                return Err(Error::UnsupportedFilter {
                    filter: b"<invalid>".to_vec(),
                });
            }
        }
    };

    if names.len() > MAX_FILTERS {
        return Err(Error::UnsupportedFilter {
            filter: b"<too-many-filters>".to_vec(),
        });
    }

    let parms_list = read_filter_parms(objects, dict, names.len())?;
    let mut standard = Vec::new();
    let mut tail = None;
    let mut iter = names.into_iter().zip(parms_list);
    while let Some((raw_name, parms)) = iter.next() {
        let canonical = expand_filter_name(raw_name);
        match canonical {
            b"FlateDecode" => standard.push(Filter::Flate {
                predictor: parse_predictor(objects, parms)?,
            }),
            b"LZWDecode" => standard.push(Filter::Lzw {
                early_change: parse_early_change(objects, parms)?,
                predictor: parse_predictor(objects, parms)?,
            }),
            b"ASCIIHexDecode" => {
                if parms.is_some() {
                    return Err(Error::UnsupportedFilter {
                        filter: canonical.to_vec(),
                    });
                }
                standard.push(Filter::AsciiHex);
            }
            b"ASCII85Decode" => {
                if parms.is_some() {
                    return Err(Error::UnsupportedFilter {
                        filter: canonical.to_vec(),
                    });
                }
                standard.push(Filter::Ascii85);
            }
            b"RunLengthDecode" => {
                if parms.is_some() {
                    return Err(Error::UnsupportedFilter {
                        filter: canonical.to_vec(),
                    });
                }
                standard.push(Filter::RunLength);
            }
            b"Crypt" => continue,
            _ => {
                let first = FilterStep::new(canonical, parms.cloned());
                let after = iter
                    .map(|(n, p)| FilterStep::new(expand_filter_name(n), p.cloned()))
                    .collect();
                tail = Some(FilterTail::new(first, after));
                break;
            }
        }
    }

    if standard.is_empty() {
        if raw_bytes.len() > limit {
            return Err(Error::Limit {
                kind: LimitKind::EncodedBytes,
                value: raw_bytes.len() as u64,
                max: limit as u64,
            });
        }
        return Ok(DecodedStream::new(Cow::Borrowed(raw_bytes), tail));
    }

    let decoded = match decode_as(raw_bytes, &standard, limit, Backend::MinizOxide, reading) {
        Ok(bytes) => bytes,
        Err(err) => {
            for &(_, f) in &err.faults {
                if f == pdf_filters::Fault::Limit {
                    return Err(Error::Limit {
                        kind: LimitKind::EncodedBytes,
                        value: (limit as u64).saturating_add(1),
                        max: limit as u64,
                    });
                }
            }
            if err.data.is_empty() && !raw_bytes.is_empty() {
                return Err(Error::NotAnImage);
            }
            for (_, f) in err.faults {
                sink.fault(Place::new(None, None), Fault::Filter(f));
            }
            err.data
        }
    };

    Ok(DecodedStream::new(Cow::Owned(decoded), tail))
}

fn read_filter_parms<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    count: usize,
) -> Result<Vec<Option<&'o Dict>>, Error> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let Some(raw) = dict.get(b"DecodeParms") else {
        return Ok(vec![None; count]);
    };
    let Some(obj) = resolve_obj(objects, raw)? else {
        return Ok(vec![None; count]);
    };
    if obj.is_null() {
        return Ok(vec![None; count]);
    }
    if let Some(d) = obj.as_dict() {
        return Ok(vec![Some(d); count]);
    }
    if let Some(arr) = obj.as_array() {
        if arr.is_empty() {
            return Ok(vec![None; count]);
        }
        if arr.len() != count {
            return Err(Error::UnsupportedFilter {
                filter: b"DecodeParms".to_vec(),
            });
        }
        let mut out = Vec::with_capacity(count);
        for item in arr {
            let resolved = resolve_obj(objects, item)?;
            match resolved {
                None => out.push(None),
                Some(o) if o.is_null() => out.push(None),
                Some(o) => match o.as_dict() {
                    Some(d) => out.push(Some(d)),
                    None => {
                        return Err(Error::UnsupportedFilter {
                            filter: b"DecodeParms".to_vec(),
                        });
                    }
                },
            }
        }
        return Ok(out);
    }
    Err(Error::UnsupportedFilter {
        filter: b"DecodeParms".to_vec(),
    })
}

fn dict_int<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
    default: i64,
) -> Result<i64, Error> {
    let Some(raw) = dict.get(key) else {
        return Ok(default);
    };
    let Some(obj) = resolve_obj(objects, raw)? else {
        return Ok(default);
    };
    if obj.is_null() {
        return Ok(default);
    }
    obj.as_integer().ok_or_else(|| Error::UnsupportedFilter {
        filter: key.to_vec(),
    })
}

fn parse_early_change<'o, R: Resolve<'o> + Copy>(
    objects: R,
    parms: Option<&'o Dict>,
) -> Result<bool, Error> {
    let Some(d) = parms else {
        return Ok(true);
    };
    match dict_int(objects, d, b"EarlyChange", 1)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::UnsupportedFilter {
            filter: b"EarlyChange".to_vec(),
        }),
    }
}

fn parse_predictor<'o, R: Resolve<'o> + Copy>(
    objects: R,
    parms: Option<&'o Dict>,
) -> Result<Predictor, Error> {
    let Some(d) = parms else {
        return Ok(Predictor::None);
    };
    let pred = dict_int(objects, d, b"Predictor", 1)?;
    if pred == 1 {
        return Ok(Predictor::None);
    }
    let colors = dict_int(objects, d, b"Colors", 1)?;
    let bpc = dict_int(objects, d, b"BitsPerComponent", 8)?;
    let columns = dict_int(objects, d, b"Columns", 1)?;
    if colors <= 0
        || columns <= 0
        || !matches!(bpc, 1 | 2 | 4 | 8 | 16)
        || u32::try_from(colors).is_err()
        || u32::try_from(columns).is_err()
    {
        return Err(Error::UnsupportedFilter {
            filter: b"Predictor".to_vec(),
        });
    }
    let layout = Layout {
        colors: colors as u32,
        bits_per_component: bpc as u32,
        columns: columns as u32,
    };
    match pred {
        2 => Ok(Predictor::Tiff(layout)),
        10..=15 => Ok(Predictor::Png(layout)),
        _ => Err(Error::UnsupportedFilter {
            filter: b"Predictor".to_vec(),
        }),
    }
}
