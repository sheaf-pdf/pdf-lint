//! Standard filter-chain decoding for inline images (`BI..ID..EI`) and direct streams.
//!
//! Indirect Image XObjects obtain leading standard-filter output via
//! [`Streams::decoded_standard`](pdf_objects::Streams::decoded_standard). Inline images and direct
//! in-memory streams parse their `/Filter` and `/DecodeParms` pipeline through the shared
//! [`pdf_objects::streams::parse_filter_chain`] reader and run [`pdf_filters::decode_as`] up to the
//! first image codec filter, producing an equivalent [`DecodedStream`](pdf_objects::DecodedStream).

use std::borrow::Cow;

use pdf_filters::{Backend, Filter, Layout, Predictor, Reading, decode_as};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::streams::{
    DecodedStream, FilterChainError, FilterPredictor, PredictorLayout, StandardFilter, Unfiltered,
    parse_filter_chain,
};
use pdf_objects::{Dict, Resolve};

use crate::error::Error;
use crate::fault::Fault;
use crate::limits::LimitKind;

/// Selects `zlib-rs` when the `zlib-rs` Cargo feature is enabled, or `miniz_oxide` otherwise.
#[inline]
pub(crate) const fn flate_backend() -> Backend {
    #[cfg(feature = "zlib-rs")]
    {
        Backend::ZlibRs
    }
    #[cfg(not(feature = "zlib-rs"))]
    {
        Backend::MinizOxide
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

fn map_unfiltered(why: Unfiltered) -> Error {
    let filter = match why {
        Unfiltered::Unsupported { filter }
        | Unfiltered::Unwanted { filter }
        | Unfiltered::Parameters { filter, .. } => filter,
        Unfiltered::Filters { .. } => b"<too-many-filters>".to_vec(),
        Unfiltered::Entries { .. } | Unfiltered::NotADictionary | Unfiltered::Key { .. } => {
            b"DecodeParms".to_vec()
        }
        Unfiltered::Integer { key } | Unfiltered::Count { key, .. } => key.as_bytes().to_vec(),
        Unfiltered::Predictor { .. } | Unfiltered::Zero | Unfiltered::Bits { .. } => {
            b"Predictor".to_vec()
        }
        Unfiltered::EarlyChange { .. } => b"EarlyChange".to_vec(),
        Unfiltered::Filter | Unfiltered::Names | Unfiltered::Reference => b"<invalid>".to_vec(),
        _ => b"<invalid>".to_vec(),
    };
    Error::UnsupportedFilter { filter }
}

/// Decodes `raw_bytes` through any leading standard filters declared in `dict`, stopping at the
/// first non-standard filter (`DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode`, `JPXDecode`, etc.).
pub(crate) fn decode_inline_standard<'a, 'o, R, S>(
    objects: R,
    dict: &'o Dict,
    raw_bytes: &'a [u8],
    limit: usize,
    reading: Reading,
    place: Option<Place>,
    sink: &mut S,
) -> Result<DecodedStream<'a>, Error>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let (parsed, _) = match parse_filter_chain(objects, dict) {
        Ok(res) => res,
        Err(FilterChainError::Resolve(u)) => return Err(Error::Unreadable(u)),
        Err(FilterChainError::Unfiltered(why)) => return Err(map_unfiltered(why)),
    };

    if parsed.standard.is_empty() {
        if raw_bytes.len() > limit {
            return Err(Error::Limit {
                kind: LimitKind::EncodedBytes,
                value: raw_bytes.len() as u64,
                max: limit as u64,
            });
        }
        return Ok(DecodedStream::new(Cow::Borrowed(raw_bytes), parsed.tail));
    }

    let standard: Vec<Filter> = parsed.standard.into_iter().map(to_pdf_filter).collect();
    let fault_place = place.unwrap_or_else(|| Place::new(None, None));
    let decoded = match decode_as(raw_bytes, &standard, limit, flate_backend(), reading) {
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
                sink.fault(fault_place, Fault::Filter(f));
            }
            err.data
        }
    };

    Ok(DecodedStream::new(Cow::Owned(decoded), parsed.tail))
}
