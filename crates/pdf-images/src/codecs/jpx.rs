//! `JPXDecode` (JPEG 2000, ISO/IEC 15444) adapter over `hayro-jpeg2000`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro_jpeg2000::{ColorSpace as JpxColorSpace, DecodeSettings, DecoderContext, Image};
use pdf_color::{EvalOptions, Family, PreparedSpace, Space};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Unreadable, Why, WithStreams, resolve_fn};

use crate::error::Error;
use crate::fault::Fault;
use crate::header::{ImageCodec, SMaskInData, check_dimensions, check_sample_budget};
use crate::limits::Limits;
use crate::samples::SampleData;

pub(crate) struct JpxDecoded {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) raw_depth: u8,
    pub(crate) bpc: u8,
    pub(crate) components: u8,
    pub(crate) samples: SampleData,
    pub(crate) embedded_alpha: Option<Vec<u8>>,
    pub(crate) embedded_space: Option<PreparedSpace>,
}

/// Decodes a `JPXDecode` codestream or JP2 container into native-depth [`SampleData`] (`1, 2, 4,
/// 8, 16` bpc), optionally extracting embedded alpha (`/SMaskInData`) and JP2 color specifications
/// when the PDF Image dictionary omits `/ColorSpace` (ISO 32000-2, §7.4.9).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_jpx<S>(
    data: &[u8],
    dict_width: u32,
    dict_height: u32,
    pdf_space: Option<&PreparedSpace>,
    is_stencil: bool,
    smask_in_data: SMaskInData,
    eval_opts: &EvalOptions<'_>,
    limits: &Limits,
    place: Place,
    sink: &mut S,
) -> Result<JpxDecoded, Error>
where
    S: Sink<Fault> + ?Sized,
{
    let pdf_is_indexed = pdf_space.is_some_and(|s| s.family() == Family::Indexed);
    let settings = DecodeSettings {
        resolve_palette_indices: !pdf_is_indexed,
        strict: false,
        target_resolution: None,
    };

    let raw_out = catch_unwind(AssertUnwindSafe(|| -> Result<RawJpxPlanes, Error> {
        let image = Image::new(data, &settings).map_err(|e| Error::Codec {
            codec: ImageCodec::Jpx,
            detail: format!("{e:?}"),
        })?;
        let w = image.width();
        let h = image.height();
        let pixels = check_dimensions(w, h, limits)?;

        // Bound f32 working planes before invoking wavelet synthesis (`4` bytes/sample/component).
        let est_channels =
            u64::from(image.color_space().num_channels()) + u64::from(u8::from(image.has_alpha()));
        let _ = check_sample_budget(
            pixels.saturating_mul(est_channels.max(1)).saturating_mul(4),
            limits,
        )?;

        let jpx_cs = image.color_space().clone();
        let has_alpha = image.has_alpha();
        let mut ctx = DecoderContext::default();
        let decoded = image.decode(&mut ctx).map_err(|e| Error::Codec {
            codec: ImageCodec::Jpx,
            detail: format!("{e:?}"),
        })?;
        let comps = decoded.components();
        if comps.is_empty() {
            return Err(Error::Codec {
                codec: ImageCodec::Jpx,
                detail: "JPX image decoded zero components".to_string(),
            });
        }

        let pixel_count = pixels as usize;
        for c in comps {
            if c.samples().len() < pixel_count {
                return Err(Error::Codec {
                    codec: ImageCodec::Jpx,
                    detail: "JPX component plane shorter than width * height".to_string(),
                });
            }
        }

        let (color_planes, alpha_plane) = if has_alpha && comps.len() >= 2 {
            (&comps[..comps.len() - 1], Some(&comps[comps.len() - 1]))
        } else {
            (comps, None)
        };

        let raw_depth = color_planes[0].bit_depth().clamp(1, 16);
        let target_bpc = normalize_pdf_bpc(raw_depth);
        let num_color = u8::try_from(color_planes.len()).map_err(|_| Error::Codec {
            codec: ImageCodec::Jpx,
            detail: "too many JPX color channels".to_string(),
        })?;

        let embedded_alpha = match (smask_in_data, alpha_plane) {
            (SMaskInData::Separate | SMaskInData::Premultiplied, Some(a_comp)) if !is_stencil => {
                let a_max = ((1_u32 << a_comp.bit_depth().clamp(1, 24)) - 1) as f32;
                let a_scale = if a_max > 0.0 { 255.0 / a_max } else { 0.0 };
                let mut alpha = Vec::with_capacity(pixel_count);
                for &v in &a_comp.samples()[..pixel_count] {
                    alpha.push((v * a_scale).round().clamp(0.0, 255.0) as u8);
                }
                Some(alpha)
            }
            _ => None,
        };

        let unpremultiply = !pdf_is_indexed
            && smask_in_data == SMaskInData::Premultiplied
            && embedded_alpha.is_some();
        let samples = pack_jpx_planes(
            color_planes,
            w,
            h,
            num_color,
            target_bpc,
            !pdf_is_indexed,
            if unpremultiply {
                embedded_alpha.as_deref()
            } else {
                None
            },
            limits,
        )?;

        Ok(RawJpxPlanes {
            width: w,
            height: h,
            raw_depth,
            bpc: target_bpc,
            components: num_color,
            jpx_cs,
            samples,
            embedded_alpha,
        })
    }));

    let planes = match raw_out {
        Ok(res) => res?,
        Err(_) => {
            return Err(Error::Codec {
                codec: ImageCodec::Jpx,
                detail: "internal JPEG 2000 decoder panic caught".to_string(),
            });
        }
    };

    if planes.width != dict_width || planes.height != dict_height {
        sink.fault(place, Fault::DimensionMismatch);
    }

    let embedded_space = if is_stencil {
        None
    } else if let Some(space) = pdf_space {
        if u32::from(planes.components) != space.components() {
            sink.fault(place, Fault::ComponentMismatch);
        }
        None
    } else {
        Some(prepare_jpx_space(
            &planes.jpx_cs,
            planes.components,
            eval_opts,
            limits,
            sink,
        )?)
    };

    Ok(JpxDecoded {
        width: planes.width,
        height: planes.height,
        raw_depth: planes.raw_depth,
        bpc: planes.bpc,
        components: planes.components,
        samples: planes.samples,
        embedded_alpha: planes.embedded_alpha,
        embedded_space,
    })
}

struct RawJpxPlanes {
    width: u32,
    height: u32,
    raw_depth: u8,
    bpc: u8,
    components: u8,
    jpx_cs: JpxColorSpace,
    samples: SampleData,
    embedded_alpha: Option<Vec<u8>>,
}

const fn normalize_pdf_bpc(depth: u8) -> u8 {
    match depth {
        0 | 1 => 1,
        2 => 2,
        3 | 4 => 4,
        5..=8 => 8,
        _ => 16,
    }
}

#[allow(clippy::too_many_arguments)]
fn pack_jpx_planes(
    planes: &[hayro_jpeg2000::ComponentData],
    width: u32,
    height: u32,
    components: u8,
    target_bpc: u8,
    rescale_to_target: bool,
    unpremul_alpha: Option<&[u8]>,
    limits: &Limits,
) -> Result<SampleData, Error> {
    let pixels = (width as usize) * (height as usize);
    let comps = usize::from(components);
    let target_max = ((1_u32 << target_bpc) - 1) as f32;

    let scales: Vec<f32> = planes
        .iter()
        .map(|p| {
            if !rescale_to_target {
                return 1.0;
            }
            let src_max = ((1_u32 << p.bit_depth().clamp(1, 24)) - 1) as f32;
            if src_max > 0.0 {
                target_max / src_max
            } else {
                0.0
            }
        })
        .collect();

    let quantize = |plane_idx: usize, px_idx: usize| -> u16 {
        let raw = planes[plane_idx].samples()[px_idx] * scales[plane_idx];
        let adjusted = match unpremul_alpha {
            Some(alpha) => {
                let a = alpha[px_idx];
                if a == 0 {
                    0.0
                } else if a < 255 {
                    raw * (255.0 / f32::from(a))
                } else {
                    raw
                }
            }
            None => raw,
        };
        adjusted.round().clamp(0.0, target_max) as u16
    };

    match target_bpc {
        1 | 2 | 4 => {
            let stride = ((u64::from(width) * u64::from(components) * u64::from(target_bpc))
                .div_ceil(8)) as usize;
            let needed = check_sample_budget((stride as u64) * u64::from(height), limits)?;
            let mut buf = vec![0_u8; needed];
            let bpc_usize = usize::from(target_bpc);
            let mask = (1_u8 << bpc_usize) - 1;
            let w_usize = width as usize;
            for y in 0..(height as usize) {
                let row = &mut buf[y * stride..(y + 1) * stride];
                for x in 0..w_usize {
                    let px_idx = y * w_usize + x;
                    for c in 0..comps {
                        let v = (quantize(c, px_idx) as u8) & mask;
                        let bit_idx = (x * comps + c) * bpc_usize;
                        let shift = 8 - bpc_usize - (bit_idx & 7);
                        row[bit_idx >> 3] |= v << shift;
                    }
                }
            }
            Ok(SampleData::Packed {
                bpc: target_bpc,
                stride,
                data: buf,
            })
        }
        8 => {
            let needed = check_sample_budget((pixels as u64) * (comps as u64), limits)?;
            let mut buf = Vec::with_capacity(needed);
            for px_idx in 0..pixels {
                for c in 0..comps {
                    buf.push(quantize(c, px_idx) as u8);
                }
            }
            Ok(SampleData::U8(buf))
        }
        16 => {
            let needed_bytes = check_sample_budget((pixels as u64) * (comps as u64) * 2, limits)?;
            let mut buf = Vec::with_capacity(needed_bytes / 2);
            for px_idx in 0..pixels {
                for c in 0..comps {
                    buf.push(quantize(c, px_idx));
                }
            }
            Ok(SampleData::U16(buf))
        }
        _ => unreachable!(),
    }
}

fn prepare_jpx_space<S>(
    jpx_cs: &JpxColorSpace,
    components: u8,
    eval_opts: &EvalOptions<'_>,
    limits: &Limits,
    sink: &mut S,
) -> Result<PreparedSpace, Error>
where
    S: Sink<Fault> + ?Sized,
{
    let mut color_sink = ColorFaultForwarder { inner: sink };
    let mut local_opts = EvalOptions::from_spaces(eval_opts.rules, None);
    local_opts.output_intent_profile = eval_opts.output_intent_profile;

    if let JpxColorSpace::Icc {
        profile,
        num_channels,
    } = jpx_cs
        && let Some(space) = PreparedSpace::from_icc_bytes(
            profile,
            u32::from(*num_channels),
            &local_opts,
            &limits.color,
            &mut color_sink,
        )
    {
        return Ok(space);
    }

    let fallback_space = match (jpx_cs, components) {
        (JpxColorSpace::Gray, _) | (_, 1) => Space::DeviceGray,
        (JpxColorSpace::RGB, _) | (_, 3) => Space::DeviceRGB,
        (JpxColorSpace::CMYK, _) | (_, 4) => Space::DeviceCMYK,
        _ => return Err(Error::MissingColorSpace),
    };
    let empty = WithStreams::new(resolve_fn(|_| None), |id, _| {
        Err(Unreadable::new(id, Why::Damaged, "none"))
    });
    PreparedSpace::from_space(
        empty,
        &fallback_space,
        &local_opts,
        &limits.color,
        &mut color_sink,
    )?
    .ok_or(Error::MissingColorSpace)
}

pub(crate) struct ColorFaultForwarder<'a, S: ?Sized> {
    pub(crate) inner: &'a mut S,
}

impl<S: Sink<Fault> + ?Sized> Sink<pdf_color::Fault> for ColorFaultForwarder<'_, S> {
    fn fault(&mut self, at: Place, fault: pdf_color::Fault) {
        self.inner.fault(at, Fault::Color(fault));
    }

    fn wants(&self) -> bool {
        self.inner.wants()
    }
}
