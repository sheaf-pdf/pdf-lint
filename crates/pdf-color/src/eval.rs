//! Color space preparation and evaluation to normalized sRGB (`[f32; 3]` / `[u8; 3]`),
//! supporting both strict ISO 32000-2 (`Rules::Iso`) and PDFium (`Rules::Pdfium`, as of
//! chromium/8152) policies.

use std::fmt;
#[cfg(feature = "icc")]
use std::sync::{Arc, Mutex, OnceLock};

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Object, ObjectId, Resolve, Rules, Span, Streams, Unreadable, Why};

use crate::cmyk_lut::pdfium_cmyk_to_srgb;
use crate::fault::Fault;
use crate::function::{Function, clamp_pair};
use crate::limits::Limits;
use crate::resources::Spaces;
use crate::space::{
    Cie, Colorant, DeviceN, Family, Icc, Indexed, LookupError, Separation, Space, table_90_decode,
};
#[cfg(feature = "icc")]
use crate::walk::{StreamDecodeErr, decode_stream_or_inline};

const D65_WHITE: [f32; 3] = [0.950_47, 1.0, 1.088_83];

/// Options controlling how [`PreparedSpace`] resolves defaults and evaluates colors.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct EvalOptions<'o> {
    /// Reading/evaluation policy:
    /// - [`Rules::Iso`]: applies `/DefaultGray`, `/DefaultRGB`, `/DefaultCMYK` (§8.6.5.6),
    ///   optional destination `/OutputIntents` ICC profile (§14.11.5), `CalGray` `/Gamma` +
    ///   `/WhitePoint`, ISO §10.4.2.5 unprofiled CMYK (`1 - min(1, C + K)`), and `Separation /All`
    ///   neutral tint `1 - t`.
    /// - [`Rules::Pdfium`]: reproduces PDFium as of chromium/8152 (`AdobeCMYK_to_sRGB`, direct
    ///   `CalGray` passthrough, ignoring `/Default*` and `/OutputIntents`, and running
    ///   `Separation /All`'s tint transform).
    pub rules: Rules,
    /// Optional `/DefaultGray` space from the active `/Resources` `/ColorSpace` dictionary.
    pub default_gray: Option<&'o Object>,
    /// Optional `/DefaultRGB` space from the active `/Resources` `/ColorSpace` dictionary.
    pub default_rgb: Option<&'o Object>,
    /// Optional `/DefaultCMYK` space from the active `/Resources` `/ColorSpace` dictionary.
    pub default_cmyk: Option<&'o Object>,
    /// Optional decoded ICC profile bytes from the document's `/OutputIntents`
    /// `/DestOutputProfile` (used under `Rules::Iso` when the `icc` feature is enabled).
    pub output_intent_profile: Option<&'o [u8]>,
}

impl Default for EvalOptions<'_> {
    fn default() -> Self {
        EvalOptions::PDFIUM
    }
}

impl<'o> EvalOptions<'o> {
    /// Strict ISO 32000-2 evaluation options without resource-level default overrides.
    pub const ISO: EvalOptions<'static> = EvalOptions {
        rules: Rules::Iso,
        default_gray: None,
        default_rgb: None,
        default_cmyk: None,
        output_intent_profile: None,
    };

    /// Viewer-compatible PDFium (`chromium/8152`) evaluation options.
    pub const PDFIUM: EvalOptions<'static> = EvalOptions {
        rules: Rules::Pdfium,
        default_gray: None,
        default_rgb: None,
        default_cmyk: None,
        output_intent_profile: None,
    };

    /// Creates options for `rules`, populating `/DefaultGray`, `/DefaultRGB`, and `/DefaultCMYK`
    /// from `spaces` when `rules == Rules::Iso`.
    #[must_use]
    pub fn from_spaces(rules: Rules, spaces: Option<Spaces<'o>>) -> EvalOptions<'o> {
        let lookup = |name: &[u8]| spaces.and_then(|s| s.get(name)).map(|n| n.value);
        EvalOptions {
            rules,
            default_gray: (rules == Rules::Iso)
                .then(|| lookup(b"DefaultGray"))
                .flatten(),
            default_rgb: (rules == Rules::Iso)
                .then(|| lookup(b"DefaultRGB"))
                .flatten(),
            default_cmyk: (rules == Rules::Iso)
                .then(|| lookup(b"DefaultCMYK"))
                .flatten(),
            output_intent_profile: None,
        }
    }
}

/// Output produced by evaluating a color in a [`PreparedSpace`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Paint {
    /// Visible color in normalized sRGB (`0.0..=1.0` per channel).
    Srgb([f32; 3]),
    /// Special `Separation /None` (or all-`/None` `DeviceN`): marks nothing, leaving the backdrop
    /// unchanged (ISO 32000-2, §8.6.6.4).
    None,
}

impl Paint {
    /// Returns `[R, G, B]` in `0.0..=1.0`, or `Option::None` for [`Paint::None`].
    #[must_use]
    pub const fn srgb(self) -> Option<[f32; 3]> {
        match self {
            Paint::Srgb(rgb) => Some(rgb),
            Paint::None => Option::None,
        }
    }

    /// Quantizes [`Paint::Srgb`] to `sRGB8` (`0..=255`), returning `Option::None` for
    /// [`Paint::None`].
    #[must_use]
    pub fn srgb8(self) -> Option<[u8; 3]> {
        self.srgb().map(quantize_srgb8)
    }

    /// Quantizes [`Paint::Srgb`] to `sRGB8`, returning `backdrop` for [`Paint::None`].
    #[must_use]
    pub fn srgb8_over(self, backdrop: [u8; 3]) -> [u8; 3] {
        self.srgb8().unwrap_or(backdrop)
    }
}

#[inline]
fn quantize_srgb8(rgb: [f32; 3]) -> [u8; 3] {
    let ch = |v: f32| -> u8 {
        if v.is_finite() {
            (v.clamp(0.0, 1.0) * 255.0).round() as u8
        } else {
            0
        }
    };
    [ch(rgb[0]), ch(rgb[1]), ch(rgb[2])]
}

#[inline]
fn quantize_u8(v: f32) -> u8 {
    if v.is_finite() {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

/// A compiled, thread-safe (`Send + Sync + Clone`) color space evaluator.
///
/// Decodes lookup tables, tint-transform functions, and (with `feature = "icc"`) ICC profiles once
/// at preparation time so per-pixel / per-scanline evaluation performs zero object lookups and
/// requires no [`Limits`] parameter. When the `icc` feature is disabled, `ICCBased` spaces
/// silently evaluate through their `/Alternate` (or `/N` device space) without emitting
/// [`Fault::IccProfile`].
#[derive(Clone)]
pub struct PreparedSpace {
    family: Family,
    components: u32,
    decode_ranges: Option<Vec<(f64, f64)>>,
    kind: PreparedKind,
}

#[derive(Clone)]
enum PreparedKind {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK {
        rules: Rules,
    },
    Defaulted {
        rules: Rules,
        target: Box<PreparedSpace>,
        unprofiled: Box<PreparedSpace>,
    },
    CalGray {
        rules: Rules,
        white: [f32; 3],
        gamma: f32,
    },
    CalRGB {
        white: [f32; 3],
        gamma: [f32; 3],
        matrix: [f32; 9],
    },
    Lab {
        rules: Rules,
        white: [f32; 3],
        range: [f32; 4],
    },
    #[cfg(feature = "icc")]
    Icc {
        n: u32,
        is_lab_pcs_input: bool,
        only_iso: bool,
        rules: Rules,
        range: Vec<(f32, f32)>,
        transform: Arc<moxcms::TransformF32Executor>,
        fallback: Box<PreparedSpace>,
    },
    IccFallback {
        fallback: Box<PreparedSpace>,
    },
    Indexed {
        hival: u8,
        base_ranges: Vec<(f32, f32)>,
        table: Vec<u8>,
        base: Box<PreparedSpace>,
    },
    SeparationNone,
    SeparationAll {
        rules: Rules,
        tint_fallback: Option<(Function, Box<PreparedSpace>)>,
    },
    Tint {
        rules: Rules,
        all_none: bool,
        tint: Function,
        alternate: Box<PreparedSpace>,
    },
}

impl fmt::Debug for PreparedSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedSpace")
            .field("family", &self.family)
            .field("components", &self.components)
            .finish_non_exhaustive()
    }
}

impl PreparedSpace {
    /// Reads `value` as a [`Space`] and prepares it for color evaluation under `options` and
    /// `limits`.
    ///
    /// Returns `Ok(None)` for `Pattern` spaces (which carry no standalone color conversion) or
    /// spaces that cannot be loaded. Passing `&mut ()` produces the exact same result as passing
    /// an active fault sink.
    ///
    /// # Errors
    ///
    /// Propagates [`Unreadable`] for non-recoverable reader failures (`Why::Capped`,
    /// `Why::Locked`, `Why::Source`, `Why::Detached`).
    pub fn prepare<'o, O, S>(
        objects: O,
        value: &'o Object,
        options: &EvalOptions<'o>,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Option<PreparedSpace>, Unreadable>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        let Some(space) = Space::read(objects, value, limits, sink)? else {
            return Ok(None);
        };
        Self::from_space(objects, &space, options, limits, sink)
    }

    /// Prepares an already-parsed [`Space`] for color evaluation.
    ///
    /// # Errors
    ///
    /// Propagates [`Unreadable`] for non-recoverable reader failures.
    pub fn from_space<'o, O, S>(
        objects: O,
        space: &Space<'o>,
        options: &EvalOptions<'o>,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Option<PreparedSpace>, Unreadable>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        PrepCtx {
            objects,
            options,
            limits,
            sink,
        }
        .build(space, 0, true)
    }

    /// Prepares an `ICCBased` space directly from raw ICC profile bytes and channel count `n`
    /// (`1`, `3`, or `4`), without requiring an indirect PDF stream [`ObjectId`].
    ///
    /// Used for ICC profiles embedded inside image containers (such as JP2 `colr` boxes). When
    /// `profile` is invalid or the `icc` feature is disabled, falls back to the unprofiled device
    /// space for `n` (emitting [`Fault::IccProfile`] when `icc` is enabled and `profile` fails to
    /// parse). Returns `None` if `n` is not `1`, `3`, or `4`.
    pub fn from_icc_bytes<S: Sink<Fault> + ?Sized>(
        profile: &[u8],
        n: u32,
        options: &EvalOptions<'_>,
        limits: &Limits,
        sink: &mut S,
    ) -> Option<PreparedSpace> {
        if !matches!(n, 1 | 3 | 4) {
            return None;
        }
        let decode_ranges = Some(vec![(0.0, 1.0); n as usize]);
        let fallback = unprofiled_device_space(n, options.rules);

        #[cfg(feature = "icc")]
        {
            if let Some((transform, is_lab)) = open_icc_transform(profile, n, limits) {
                return Some(PreparedSpace {
                    family: Family::ICCBased,
                    components: n,
                    decode_ranges,
                    kind: PreparedKind::Icc {
                        n,
                        is_lab_pcs_input: is_lab,
                        only_iso: false,
                        rules: options.rules,
                        range: vec![(0.0, 1.0); n as usize],
                        transform,
                        fallback: Box::new(fallback),
                    },
                });
            }
            if sink.wants() {
                sink.fault(Place::new(None, None), Fault::IccProfile);
            }
        }
        #[cfg(not(feature = "icc"))]
        let _ = (profile, limits, sink);

        Some(PreparedSpace {
            family: Family::ICCBased,
            components: n,
            decode_ranges,
            kind: PreparedKind::IccFallback {
                fallback: Box::new(fallback),
            },
        })
    }

    /// The prepared space's [`Family`].
    #[must_use]
    pub const fn family(&self) -> Family {
        self.family
    }

    /// Number of input components expected by [`Self::eval`].
    #[must_use]
    pub const fn components(&self) -> u32 {
        self.components
    }

    /// Underlying base [`PreparedSpace`] when `self.family()` is [`Family::Indexed`], or `None`
    /// otherwise.
    #[must_use]
    pub fn base_space(&self) -> Option<&PreparedSpace> {
        match &self.kind {
            PreparedKind::Indexed { base, .. } => Some(base),
            _ => None,
        }
    }

    /// Component `(min, max)` decode ranges for non-`Indexed` spaces, or `None` for `Indexed`.
    #[must_use]
    pub fn decode_ranges(&self) -> Option<&[(f64, f64)]> {
        self.decode_ranges.as_deref()
    }

    /// Pre-resolved default `/Decode` intervals `(min, max)` for an image of
    /// `bits_per_component` (`1..=32`) in this space (ISO 32000-2, Table 90).
    #[must_use]
    pub fn default_decode(&self, bits_per_component: u8) -> Option<Vec<(f64, f64)>> {
        table_90_decode(
            self.family,
            Some(self.components),
            bits_per_component,
            self.decode_ranges.as_deref(),
        )
    }

    /// Evaluates `components` (`len >= self.components()`) to [`Paint`].
    #[must_use]
    pub fn eval(&self, components: &[f32]) -> Option<Paint> {
        self.eval_inner(components, None)
    }

    /// Evaluates `components` (`len >= self.components()`) to [`Paint`], overriding the active
    /// [`Rules`] (`Rules::Iso` vs `Rules::Pdfium`) across device defaults, `/OutputIntents`, CIE
    /// spaces, `Separation /All`, `DeviceN /None`, and `Type 4` PostScript tint transforms.
    #[must_use]
    pub fn eval_with_rules(&self, components: &[f32], rules: Rules) -> Option<Paint> {
        self.eval_inner(components, Some(rules))
    }

    fn eval_inner(&self, components: &[f32], rules_override: Option<Rules>) -> Option<Paint> {
        let need = self.components as usize;
        if components.len() < need {
            return None;
        }
        let c = &components[..need];
        match &self.kind {
            PreparedKind::DeviceGray => {
                let g = clamp01(c[0]);
                Some(Paint::Srgb([g, g, g]))
            }
            PreparedKind::DeviceRGB => {
                Some(Paint::Srgb([clamp01(c[0]), clamp01(c[1]), clamp01(c[2])]))
            }
            PreparedKind::DeviceCMYK { rules } => {
                let active = rules_override.unwrap_or(*rules);
                let (cy, mg, yl, bk) = (clamp01(c[0]), clamp01(c[1]), clamp01(c[2]), clamp01(c[3]));
                let rgb = match active {
                    Rules::Iso => [
                        1.0 - (cy + bk).min(1.0),
                        1.0 - (mg + bk).min(1.0),
                        1.0 - (yl + bk).min(1.0),
                    ],
                    _ => pdfium_cmyk_to_srgb(cy, mg, yl, bk),
                };
                Some(Paint::Srgb(rgb))
            }
            PreparedKind::Defaulted {
                rules,
                target,
                unprofiled,
            } => {
                let active = rules_override.unwrap_or(*rules);
                if active == Rules::Iso {
                    target.eval_inner(c, rules_override)
                } else {
                    unprofiled.eval_inner(c, rules_override)
                }
            }
            PreparedKind::CalGray {
                rules,
                white,
                gamma,
            } => {
                let active = rules_override.unwrap_or(*rules);
                let a = clamp01(c[0]);
                let rgb = match active {
                    Rules::Iso => {
                        let ag = a.powf(*gamma);
                        let xyz = [white[0] * ag, white[1] * ag, white[2] * ag];
                        let d65 = adapt_bradford(xyz, *white);
                        xyz_d65_to_srgb(d65)
                    }
                    _ => [a, a, a],
                };
                Some(Paint::Srgb(rgb))
            }
            PreparedKind::CalRGB {
                white,
                gamma,
                matrix,
            } => {
                let ar = clamp01(c[0]).powf(gamma[0]);
                let bg = clamp01(c[1]).powf(gamma[1]);
                let cb = clamp01(c[2]).powf(gamma[2]);
                let xyz = [
                    matrix[0] * ar + matrix[3] * bg + matrix[6] * cb,
                    matrix[1] * ar + matrix[4] * bg + matrix[7] * cb,
                    matrix[2] * ar + matrix[5] * bg + matrix[8] * cb,
                ];
                let d65 = adapt_bradford(xyz, *white);
                Some(Paint::Srgb(xyz_d65_to_srgb(d65)))
            }
            PreparedKind::Lab {
                rules,
                white,
                range,
            } => {
                let active = rules_override.unwrap_or(*rules);
                let l_star = clamp_pair(c[0], 0.0, 100.0);
                let a_star = clamp_pair(c[1], range[0], range[1]);
                let b_star = clamp_pair(c[2], range[2], range[3]);
                let m = (l_star + 16.0) / 116.0;
                let l = m + a_star / 500.0;
                let n = m - b_star / 200.0;
                let inv_f = |t: f32| -> f32 {
                    const DELTA: f32 = 6.0 / 29.0;
                    if t >= DELTA {
                        t * t * t
                    } else {
                        (108.0 / 841.0) * (t - 4.0 / 29.0)
                    }
                };
                let d65 = match active {
                    Rules::Iso => {
                        let xyz = [
                            white[0] * inv_f(l),
                            white[1] * inv_f(m),
                            white[2] * inv_f(n),
                        ];
                        adapt_bradford(xyz, *white)
                    }
                    _ => {
                        // PDFium (`CPDF_LabCS::GetRGB`, as of chromium/8152) ignores `/WhitePoint`
                        // and evaluates `Lab -> XYZ` with fixed constants `(0.957, 1.0, 1.0889)`.
                        const PDFIUM_LAB_WHITE: [f32; 3] = [0.957, 1.0, 1.0889];
                        [
                            PDFIUM_LAB_WHITE[0] * inv_f(l),
                            PDFIUM_LAB_WHITE[1] * inv_f(m),
                            PDFIUM_LAB_WHITE[2] * inv_f(n),
                        ]
                    }
                };
                Some(Paint::Srgb(xyz_d65_to_srgb(d65)))
            }
            #[cfg(feature = "icc")]
            PreparedKind::Icc {
                n,
                is_lab_pcs_input,
                only_iso,
                rules,
                range,
                transform,
                fallback,
            } => {
                let active = rules_override.unwrap_or(*rules);
                if *only_iso && active != Rules::Iso {
                    return fallback.eval_inner(c, rules_override);
                }
                let mut src = [0.0_f32; 4];
                normalize_icc_input(c, *n, *is_lab_pcs_input, range, &mut src);
                let mut dst = [0.0_f32; 3];
                if moxcms::TransformExecutor::transform(
                    transform.as_ref(),
                    &src[..*n as usize],
                    &mut dst,
                )
                .is_ok()
                {
                    Some(Paint::Srgb([
                        clamp01(dst[0]),
                        clamp01(dst[1]),
                        clamp01(dst[2]),
                    ]))
                } else {
                    fallback.eval_inner(c, rules_override)
                }
            }
            PreparedKind::IccFallback { fallback } => fallback.eval_inner(c, rules_override),
            PreparedKind::Indexed {
                hival,
                base_ranges,
                table,
                base,
            } => {
                let raw_idx = if c[0].is_finite() { c[0].round() } else { 0.0 };
                let idx = (raw_idx.clamp(0.0, f32::from(*hival)) as usize).min(*hival as usize);
                let m = base.components() as usize;
                let offset = idx * m;
                let mut mapped = [0.0_f32; 32];
                let mut mapped_vec = Vec::new();
                let dst: &mut [f32] = if m <= mapped.len() {
                    &mut mapped[..m]
                } else {
                    mapped_vec.resize(m, 0.0);
                    &mut mapped_vec
                };
                for (j, slot) in dst.iter_mut().enumerate() {
                    let byte = table.get(offset + j).copied().unwrap_or(0);
                    let t = f32::from(byte) / 255.0;
                    let (lo, hi) = base_ranges.get(j).copied().unwrap_or((0.0, 1.0));
                    *slot = lo + t * (hi - lo);
                }
                base.eval_inner(dst, rules_override)
            }
            PreparedKind::SeparationNone => Some(Paint::None),
            PreparedKind::SeparationAll {
                rules,
                tint_fallback,
            } => {
                let active = rules_override.unwrap_or(*rules);
                if active == Rules::Iso {
                    let g = 1.0 - clamp01(c[0]);
                    Some(Paint::Srgb([g, g, g]))
                } else if let Some((tint, alternate)) = tint_fallback {
                    eval_tint_transform(c, tint, alternate, rules_override)
                } else {
                    let g = 1.0 - clamp01(c[0]);
                    Some(Paint::Srgb([g, g, g]))
                }
            }
            PreparedKind::Tint {
                rules,
                all_none,
                tint,
                alternate,
            } => {
                let active = rules_override.unwrap_or(*rules);
                if *all_none && active == Rules::Iso {
                    return Some(Paint::None);
                }
                eval_tint_transform(c, tint, alternate, rules_override)
            }
        }
    }

    /// Evaluates `components` directly to normalized sRGB (`[f32; 3]`), returning `None` if
    /// evaluation fails or produces [`Paint::None`].
    #[must_use]
    pub fn eval_srgb(&self, components: &[f32]) -> Option<[f32; 3]> {
        self.eval(components)?.srgb()
    }

    /// Evaluates `components` directly to quantized `sRGB8` (`[u8; 3]`), returning `None` if
    /// evaluation fails or produces [`Paint::None`].
    #[must_use]
    pub fn eval_srgb8(&self, components: &[f32]) -> Option<[u8; 3]> {
        self.eval(components)?.srgb8()
    }

    /// Batch-converts `out.len()` packed pixel component tuples from `samples`
    /// (`samples.len() >= out.len() * self.components()`) into `sRGB8`, filling `backdrop` for
    /// [`Paint::None`] pixels.
    ///
    /// Returns `false` if `samples` is too short or any pixel fails to evaluate.
    #[must_use]
    pub fn eval_pixels_srgb8(
        &self,
        samples: &[f32],
        out: &mut [[u8; 3]],
        backdrop: [u8; 3],
    ) -> bool {
        self.eval_pixels_srgb8_inner(samples, out, backdrop, None)
    }

    /// Like [`Self::eval_pixels_srgb8`], overriding the active [`Rules`] across all nested/default
    /// spaces.
    #[must_use]
    pub fn eval_pixels_srgb8_with_rules(
        &self,
        samples: &[f32],
        out: &mut [[u8; 3]],
        backdrop: [u8; 3],
        rules: Rules,
    ) -> bool {
        self.eval_pixels_srgb8_inner(samples, out, backdrop, Some(rules))
    }

    fn eval_pixels_srgb8_inner(
        &self,
        samples: &[f32],
        out: &mut [[u8; 3]],
        backdrop: [u8; 3],
        rules_override: Option<Rules>,
    ) -> bool {
        let stride = self.components as usize;
        let Some(needed) = out.len().checked_mul(stride) else {
            return false;
        };
        if samples.len() < needed {
            return false;
        }
        if out.is_empty() {
            return true;
        }

        match &self.kind {
            PreparedKind::DeviceGray => {
                for (dst, &s) in out.iter_mut().zip(&samples[..needed]) {
                    let g = quantize_u8(s);
                    *dst = [g, g, g];
                }
                return true;
            }
            PreparedKind::DeviceRGB => {
                let triples = samples[..needed].as_chunks::<3>().0;
                for (dst, &[r, g, b]) in out.iter_mut().zip(triples) {
                    *dst = [quantize_u8(r), quantize_u8(g), quantize_u8(b)];
                }
                return true;
            }
            PreparedKind::DeviceCMYK { rules } => {
                let quads = samples[..needed].as_chunks::<4>().0;
                if rules_override.unwrap_or(*rules) == Rules::Iso {
                    for (dst, &[c, m, y, k]) in out.iter_mut().zip(quads) {
                        let (cy, mg, yl, bk) = (clamp01(c), clamp01(m), clamp01(y), clamp01(k));
                        *dst = quantize_srgb8([
                            1.0 - (cy + bk).min(1.0),
                            1.0 - (mg + bk).min(1.0),
                            1.0 - (yl + bk).min(1.0),
                        ]);
                    }
                } else {
                    for (dst, &[c, m, y, k]) in out.iter_mut().zip(quads) {
                        *dst = quantize_srgb8(pdfium_cmyk_to_srgb(
                            clamp01(c),
                            clamp01(m),
                            clamp01(y),
                            clamp01(k),
                        ));
                    }
                }
                return true;
            }
            PreparedKind::Defaulted {
                rules,
                target,
                unprofiled,
            } => {
                if rules_override.unwrap_or(*rules) == Rules::Iso {
                    return target.eval_pixels_srgb8_inner(samples, out, backdrop, rules_override);
                }
                return unprofiled.eval_pixels_srgb8_inner(samples, out, backdrop, rules_override);
            }
            PreparedKind::IccFallback { fallback } => {
                return fallback.eval_pixels_srgb8_inner(samples, out, backdrop, rules_override);
            }
            PreparedKind::SeparationNone => {
                out.fill(backdrop);
                return true;
            }
            #[cfg(feature = "icc")]
            PreparedKind::Icc {
                n,
                is_lab_pcs_input,
                only_iso,
                rules,
                range,
                transform,
                fallback,
            } => {
                if *only_iso && rules_override.unwrap_or(*rules) != Rules::Iso {
                    return fallback.eval_pixels_srgb8_inner(
                        samples,
                        out,
                        backdrop,
                        rules_override,
                    );
                }
                let ch = *n as usize;
                let mut src_chunk = [0.0_f32; 64 * 4];
                let mut dst_chunk = [0.0_f32; 64 * 3];
                let mut pixel_idx = 0_usize;
                while pixel_idx < out.len() {
                    let count = (out.len() - pixel_idx).min(64);
                    for k in 0..count {
                        let c_off = (pixel_idx + k) * ch;
                        normalize_icc_input(
                            &samples[c_off..c_off + ch],
                            *n,
                            *is_lab_pcs_input,
                            range,
                            &mut src_chunk[k * ch..(k + 1) * ch],
                        );
                    }
                    if moxcms::TransformExecutor::transform(
                        transform.as_ref(),
                        &src_chunk[..count * ch],
                        &mut dst_chunk[..count * 3],
                    )
                    .is_ok()
                    {
                        for k in 0..count {
                            let d = &dst_chunk[k * 3..k * 3 + 3];
                            out[pixel_idx + k] =
                                [quantize_u8(d[0]), quantize_u8(d[1]), quantize_u8(d[2])];
                        }
                    } else if !fallback.eval_pixels_srgb8_inner(
                        &samples[pixel_idx * ch..(pixel_idx + count) * ch],
                        &mut out[pixel_idx..pixel_idx + count],
                        backdrop,
                        rules_override,
                    ) {
                        return false;
                    }
                    pixel_idx += count;
                }
                return true;
            }
            PreparedKind::Indexed { hival, .. } if out.len() >= 16 => {
                let mut lut: [Option<[u8; 3]>; 256] = [None; 256];
                let hi = *hival;
                for (dst, &raw) in out.iter_mut().zip(&samples[..needed]) {
                    let r = if raw.is_finite() { raw.round() } else { 0.0 };
                    let idx = (r.clamp(0.0, f32::from(hi)) as usize).min(hi as usize);
                    if let Some(rgb) = lut[idx] {
                        *dst = rgb;
                    } else {
                        let Some(paint) = self.eval_inner(&[idx as f32], rules_override) else {
                            return false;
                        };
                        let rgb = paint.srgb8_over(backdrop);
                        lut[idx] = Some(rgb);
                        *dst = rgb;
                    }
                }
                return true;
            }
            _ => {}
        }

        for (i, dst) in out.iter_mut().enumerate() {
            let start = i * stride;
            let Some(paint) = self.eval_inner(&samples[start..start + stride], rules_override)
            else {
                return false;
            };
            *dst = paint.srgb8_over(backdrop);
        }
        true
    }
}

fn eval_tint_transform(
    c: &[f32],
    tint: &Function,
    alternate: &PreparedSpace,
    rules_override: Option<Rules>,
) -> Option<Paint> {
    let alt_n = alternate.components() as usize;
    let func_n = (tint.outputs() as usize).max(alt_n);
    let mut buf = [0.0_f32; 16];
    let mut heap = Vec::new();
    let func_out: &mut [f32] = if func_n <= buf.len() {
        &mut buf[..func_n]
    } else {
        heap.resize(func_n, 0.0);
        &mut heap
    };
    if !tint.eval_with_rules(c, func_out, rules_override) {
        return None;
    }
    alternate.eval_inner(&func_out[..alt_n], rules_override)
}

#[cfg(feature = "icc")]
#[inline]
fn normalize_icc_input(
    c: &[f32],
    n: u32,
    is_lab_pcs_input: bool,
    range: &[(f32, f32)],
    src: &mut [f32],
) {
    if is_lab_pcs_input && n == 3 {
        let l = clamp_pair(c[0], range[0].0, range[0].1);
        let a = clamp_pair(c[1], range[1].0, range[1].1);
        let b = clamp_pair(c[2], range[2].0, range[2].1);
        src[0] = (l / 100.0).clamp(0.0, 1.0);
        src[1] = ((a + 128.0) / 255.0).clamp(0.0, 1.0);
        src[2] = ((b + 128.0) / 255.0).clamp(0.0, 1.0);
    } else {
        for (i, &(r0, r1)) in range.iter().enumerate() {
            let clamped = clamp_pair(c[i], r0, r1);
            let span = r1 - r0;
            src[i] = if span == 0.0 {
                0.0
            } else {
                let v = (clamped - r0) / span;
                if v.is_finite() {
                    v.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            };
        }
    }
}

fn unprofiled_device_space(n: u32, rules: Rules) -> PreparedSpace {
    let (family, components, kind) = match n {
        1 => (Family::DeviceGray, 1, PreparedKind::DeviceGray),
        3 => (Family::DeviceRGB, 3, PreparedKind::DeviceRGB),
        _ => (Family::DeviceCMYK, 4, PreparedKind::DeviceCMYK { rules }),
    };
    PreparedSpace {
        family,
        components,
        decode_ranges: Some(vec![(0.0, 1.0); components as usize]),
        kind,
    }
}

struct PrepCtx<'l, 'o, O, S: ?Sized> {
    objects: O,
    options: &'l EvalOptions<'o>,
    limits: &'l Limits,
    sink: &'l mut S,
}

impl<'o, O, S> PrepCtx<'_, 'o, O, S>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    fn say(&mut self, id: Option<ObjectId>, at: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            self.sink.fault(Place::new(id, at), fault);
        }
    }

    fn build(
        &mut self,
        space: &Space<'o>,
        depth: usize,
        allow_defaults: bool,
    ) -> Result<Option<PreparedSpace>, Unreadable> {
        if depth > self.limits.space_depth {
            return Ok(None);
        }
        match space {
            Space::DeviceGray => self.prepare_device(
                Family::DeviceGray,
                1,
                allow_defaults
                    .then_some(self.options.default_gray)
                    .flatten(),
                PreparedKind::DeviceGray,
                depth,
            ),
            Space::DeviceRGB => self.prepare_device(
                Family::DeviceRGB,
                3,
                allow_defaults.then_some(self.options.default_rgb).flatten(),
                PreparedKind::DeviceRGB,
                depth,
            ),
            Space::DeviceCMYK => self.prepare_device(
                Family::DeviceCMYK,
                4,
                allow_defaults
                    .then_some(self.options.default_cmyk)
                    .flatten(),
                PreparedKind::DeviceCMYK {
                    rules: self.options.rules,
                },
                depth,
            ),
            Space::CalGray(cie) => Ok(Some(self.prepare_cal_gray(cie)?)),
            Space::CalRGB(cie) => Ok(Some(self.prepare_cal_rgb(cie)?)),
            Space::Lab(cie) => Ok(Some(self.prepare_lab(cie)?)),
            Space::ICCBased(icc) => self.prepare_icc(icc, depth),
            Space::Indexed(idx) => self.prepare_indexed(idx, depth),
            Space::Separation(sep) => self.prepare_separation(sep, depth),
            Space::DeviceN(dn) => self.prepare_device_n(dn, depth),
            Space::Pattern(_) => Ok(None),
        }
    }

    fn prepare_device(
        &mut self,
        family: Family,
        components: u32,
        default_override: Option<&'o Object>,
        native_kind: PreparedKind,
        depth: usize,
    ) -> Result<Option<PreparedSpace>, Unreadable> {
        let decode_ranges = Some(vec![(0.0, 1.0); components as usize]);

        // 1. ISO 32000-2 §8.6.5.6 `/DefaultGray`, `/DefaultRGB`, `/DefaultCMYK` substitution:
        //    any color space with matching component count other than `Lab`, `Indexed`, or
        //    `Pattern` may substitute for an unprofiled device space.
        if self.options.rules == Rules::Iso
            && let Some(def_obj) = default_override
            && let Some(def_space) = Space::read(self.objects, def_obj, self.limits, self.sink)?
            && !matches!(
                def_space.family(),
                Family::Lab | Family::Indexed | Family::Pattern
            )
            && def_space.components() == Some(components)
            && let Some(target) = self.build(&def_space, depth + 1, false)?
        {
            let unprofiled = PreparedSpace {
                family,
                components,
                decode_ranges: decode_ranges.clone(),
                kind: native_kind,
            };
            return Ok(Some(PreparedSpace {
                family,
                components,
                decode_ranges,
                kind: PreparedKind::Defaulted {
                    rules: self.options.rules,
                    target: Box::new(target),
                    unprofiled: Box::new(unprofiled),
                },
            }));
        }

        // 2. ISO 32000-2 §14.11.5 `/OutputIntents` destination profile fallback when unprofiled.
        #[cfg(feature = "icc")]
        if self.options.rules == Rules::Iso
            && let Some(profile_bytes) = self.options.output_intent_profile
            && let Some((transform, is_lab)) =
                open_icc_transform(profile_bytes, components, self.limits)
        {
            let fallback = PreparedSpace {
                family,
                components,
                decode_ranges: decode_ranges.clone(),
                kind: native_kind,
            };
            return Ok(Some(PreparedSpace {
                family,
                components,
                decode_ranges,
                kind: PreparedKind::Icc {
                    n: components,
                    is_lab_pcs_input: is_lab,
                    only_iso: true,
                    rules: self.options.rules,
                    range: vec![(0.0, 1.0); components as usize],
                    transform,
                    fallback: Box::new(fallback),
                },
            }));
        }

        Ok(Some(PreparedSpace {
            family,
            components,
            decode_ranges,
            kind: native_kind,
        }))
    }

    fn prepare_cal_gray(&self, cie: &Cie<'o>) -> Result<PreparedSpace, Unreadable> {
        let white = valid_white(cie.white_point);
        let gamma = cie
            .cal_gray_gamma(&self.objects, self.limits)?
            .unwrap_or(1.0);
        Ok(PreparedSpace {
            family: Family::CalGray,
            components: 1,
            decode_ranges: Some(vec![(0.0, 1.0)]),
            kind: PreparedKind::CalGray {
                rules: self.options.rules,
                white,
                gamma,
            },
        })
    }

    fn prepare_cal_rgb(&self, cie: &Cie<'o>) -> Result<PreparedSpace, Unreadable> {
        let white = valid_white(cie.white_point);
        let (gamma, matrix) = cie.cal_rgb_params(&self.objects, self.limits)?;
        Ok(PreparedSpace {
            family: Family::CalRGB,
            components: 3,
            decode_ranges: Some(vec![(0.0, 1.0); 3]),
            kind: PreparedKind::CalRGB {
                white,
                gamma,
                matrix,
            },
        })
    }

    fn prepare_lab(&self, cie: &Cie<'o>) -> Result<PreparedSpace, Unreadable> {
        let white = valid_white(cie.white_point);
        let r64 = cie.lab_range(self.objects, self.limits)?;
        let range = [r64[0] as f32, r64[1] as f32, r64[2] as f32, r64[3] as f32];
        Ok(PreparedSpace {
            family: Family::Lab,
            components: 3,
            decode_ranges: Some(vec![(0.0, 100.0), (r64[0], r64[1]), (r64[2], r64[3])]),
            kind: PreparedKind::Lab {
                rules: self.options.rules,
                white,
                range,
            },
        })
    }

    fn prepare_icc(
        &mut self,
        icc: &Icc<'o>,
        depth: usize,
    ) -> Result<Option<PreparedSpace>, Unreadable> {
        let Some(n) = icc.n else {
            return Ok(None);
        };
        let range64 = icc
            .range(self.objects, self.limits)?
            .unwrap_or_else(|| vec![(0.0, 1.0); n as usize]);

        // Build `/Alternate` or device fallback for `n`.
        let fallback = if let Some(alt) = &icc.alternate
            && alt.components() == Some(n)
            && let Some(prep) = self.build(alt, depth + 1, false)?
        {
            prep
        } else {
            unprofiled_device_space(n, self.options.rules)
        };

        #[cfg(feature = "icc")]
        if let Some(stream) = icc.stream {
            match decode_stream_or_inline(&self.objects, stream, icc.id, self.limits.stream_bytes) {
                Ok(bytes) => match open_icc_transform(&bytes, n, self.limits) {
                    Some((transform, is_lab)) => {
                        let range_f32 =
                            range64.iter().map(|&(a, b)| (a as f32, b as f32)).collect();
                        return Ok(Some(PreparedSpace {
                            family: Family::ICCBased,
                            components: n,
                            decode_ranges: Some(range64),
                            kind: PreparedKind::Icc {
                                n,
                                is_lab_pcs_input: is_lab,
                                only_iso: false,
                                rules: self.options.rules,
                                range: range_f32,
                                transform,
                                fallback: Box::new(fallback),
                            },
                        }));
                    }
                    None => self.say(icc.id, None, Fault::IccProfile),
                },
                Err(StreamDecodeErr::Limit | StreamDecodeErr::Corrupt) => {
                    self.say(icc.id, None, Fault::IccProfile);
                }
                Err(StreamDecodeErr::Unreadable(e)) => return Err(e),
            }
        }

        Ok(Some(PreparedSpace {
            family: Family::ICCBased,
            components: n,
            decode_ranges: Some(range64),
            kind: PreparedKind::IccFallback {
                fallback: Box::new(fallback),
            },
        }))
    }

    fn prepare_indexed(
        &mut self,
        idx: &Indexed<'o>,
        depth: usize,
    ) -> Result<Option<PreparedSpace>, Unreadable> {
        let (Some(base_space), Some(hival)) = (idx.base.as_deref(), idx.hival) else {
            return Ok(None);
        };
        if matches!(base_space.family(), Family::Indexed | Family::Pattern) {
            return Ok(None);
        }
        let Some(base) = self.build(base_space, depth + 1, true)? else {
            return Ok(None);
        };
        let bytes = match idx.lookup_bytes(&self.objects, self.limits.stream_bytes) {
            Ok(Some(b)) => b,
            Ok(None) | Err(LookupError::Locked) => return Ok(None),
            Err(LookupError::Limit) => {
                let lookup_id = match &idx.lookup {
                    Some(crate::space::Lookup::Stream { id, .. }) => *id,
                    _ => None,
                };
                self.say(lookup_id, None, Fault::LookupLength);
                return Ok(None);
            }
            Err(LookupError::Unreadable(e)) if matches!(e.why, Why::Damaged | Why::Filter) => {
                let lookup_id = match &idx.lookup {
                    Some(crate::space::Lookup::Stream { id, .. }) => *id,
                    _ => None,
                };
                self.say(lookup_id, None, Fault::LookupLength);
                return Ok(None);
            }
            Err(LookupError::Unreadable(e)) => return Err(e),
        };
        let base_ranges = base
            .decode_ranges
            .as_ref()
            .map(|pairs| pairs.iter().map(|&(a, b)| (a as f32, b as f32)).collect())
            .unwrap_or_else(|| vec![(0.0, 1.0); base.components() as usize]);

        Ok(Some(PreparedSpace {
            family: Family::Indexed,
            components: 1,
            decode_ranges: None,
            kind: PreparedKind::Indexed {
                hival,
                base_ranges,
                table: bytes.into_owned(),
                base: Box::new(base),
            },
        }))
    }

    fn prepare_separation(
        &mut self,
        sep: &Separation<'o>,
        depth: usize,
    ) -> Result<Option<PreparedSpace>, Unreadable> {
        let colorant = sep.name.map(Colorant::of);
        if colorant == Some(Colorant::None) {
            return Ok(Some(PreparedSpace {
                family: Family::Separation,
                components: 1,
                decode_ranges: Some(vec![(0.0, 1.0)]),
                kind: PreparedKind::SeparationNone,
            }));
        }
        if colorant == Some(Colorant::All) && self.options.rules == Rules::Iso {
            // Prepare the optional tint transform quietly so `eval_with_rules(..., Rules::Pdfium)`
            // reproduces PDFium without requiring a second `PreparedSpace::prepare` pass.
            let tint_fallback = if let (Some(alt_space), Some(tint_obj)) =
                (sep.alternate.as_deref(), sep.tint)
                && let Some(alternate) = (PrepCtx {
                    objects: self.objects,
                    options: self.options,
                    limits: self.limits,
                    sink: &mut (),
                })
                .build(alt_space, depth + 1, true)?
                && let Some(tint) =
                    Function::read(self.objects, tint_obj, Rules::Pdfium, self.limits, &mut ())?
                && tint.inputs() == 1
                && tint.outputs() >= alternate.components()
            {
                Some((tint, Box::new(alternate)))
            } else {
                None
            };
            return Ok(Some(PreparedSpace {
                family: Family::Separation,
                components: 1,
                decode_ranges: Some(vec![(0.0, 1.0)]),
                kind: PreparedKind::SeparationAll {
                    rules: self.options.rules,
                    tint_fallback,
                },
            }));
        }
        let (Some(alt_space), Some(tint_obj)) = (sep.alternate.as_deref(), sep.tint) else {
            return Ok(None);
        };
        let Some(alternate) = self.build(alt_space, depth + 1, true)? else {
            return Ok(None);
        };
        let Some(tint) = Function::read(
            self.objects,
            tint_obj,
            self.options.rules,
            self.limits,
            self.sink,
        )?
        else {
            return Ok(None);
        };
        if tint.inputs() != 1 || tint.outputs() < alternate.components() {
            return Ok(None);
        }
        let kind = if colorant == Some(Colorant::All) {
            PreparedKind::SeparationAll {
                rules: self.options.rules,
                tint_fallback: Some((tint, Box::new(alternate))),
            }
        } else {
            PreparedKind::Tint {
                rules: self.options.rules,
                all_none: false,
                tint,
                alternate: Box::new(alternate),
            }
        };
        Ok(Some(PreparedSpace {
            family: Family::Separation,
            components: 1,
            decode_ranges: Some(vec![(0.0, 1.0)]),
            kind,
        }))
    }

    fn prepare_device_n(
        &mut self,
        dn: &DeviceN<'o>,
        depth: usize,
    ) -> Result<Option<PreparedSpace>, Unreadable> {
        let (Some(m), Some(alt_space), Some(tint_obj)) =
            (dn.components, dn.alternate.as_deref(), dn.tint)
        else {
            return Ok(None);
        };
        let Some(alternate) = self.build(alt_space, depth + 1, true)? else {
            return Ok(None);
        };
        let Some(tint) = Function::read(
            self.objects,
            tint_obj,
            self.options.rules,
            self.limits,
            self.sink,
        )?
        else {
            return Ok(None);
        };
        if tint.inputs() != m || tint.outputs() < alternate.components() {
            return Ok(None);
        }
        let all_none = dn.names.len() == m as usize
            && dn
                .names
                .iter()
                .all(|n| n.map(Colorant::of) == Some(Colorant::None));
        Ok(Some(PreparedSpace {
            family: Family::DeviceN,
            components: m,
            decode_ranges: Some(vec![(0.0, 1.0); m as usize]),
            kind: PreparedKind::Tint {
                rules: self.options.rules,
                all_none,
                tint,
                alternate: Box::new(alternate),
            },
        }))
    }
}

#[cfg(feature = "icc")]
const ICC_MEMO_MAX_BYTES: usize = 64 * 1024;
#[cfg(feature = "icc")]
const ICC_MEMO_SLOTS: usize = 4;

#[cfg(feature = "icc")]
struct IccMemoEntry {
    n: u32,
    bytes: Vec<u8>,
    transform: Arc<moxcms::TransformF32Executor>,
    is_lab: bool,
}

#[cfg(feature = "icc")]
#[derive(Default)]
struct IccMemo {
    slots: [Option<IccMemoEntry>; ICC_MEMO_SLOTS],
    next: usize,
}

#[cfg(feature = "icc")]
fn icc_memo() -> &'static Mutex<IccMemo> {
    static MEMO: OnceLock<Mutex<IccMemo>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(IccMemo::default()))
}

#[cfg(feature = "icc")]
fn open_icc_transform(
    bytes: &[u8],
    n: u32,
    limits: &Limits,
) -> Option<(Arc<moxcms::TransformF32Executor>, bool)> {
    use moxcms::{ColorProfile, DataColorSpace, Layout, ParsingOptions, TransformOptions};

    if bytes.len() > limits.stream_bytes {
        return None;
    }

    if bytes.len() <= ICC_MEMO_MAX_BYTES
        && let Ok(guard) = icc_memo().lock()
    {
        for entry in guard.slots.iter().flatten() {
            if entry.n == n && entry.bytes.as_slice() == bytes {
                return Some((Arc::clone(&entry.transform), entry.is_lab));
            }
        }
    }

    let parse_opts = ParsingOptions {
        max_profile_size: limits.stream_bytes.saturating_add(1),
        ..ParsingOptions::default()
    };
    let profile = ColorProfile::new_from_slice_with_options(bytes, parse_opts).ok()?;
    let (src_layout, is_lab) = match (n, profile.color_space) {
        (1, DataColorSpace::Gray) => (Layout::Gray, false),
        (3, DataColorSpace::Lab) => (Layout::Rgb, true),
        (
            3,
            DataColorSpace::Rgb
            | DataColorSpace::Xyz
            | DataColorSpace::Yxy
            | DataColorSpace::YCbr
            | DataColorSpace::Luv
            | DataColorSpace::Hsv
            | DataColorSpace::Hls
            | DataColorSpace::Cmy
            | DataColorSpace::Color3,
        ) => (Layout::Rgb, false),
        (4, DataColorSpace::Cmyk | DataColorSpace::Color4) => (Layout::Rgba, false),
        _ => return None,
    };
    let dst = ColorProfile::new_srgb();
    let transform = profile
        .create_transform_f32(src_layout, &dst, Layout::Rgb, TransformOptions::default())
        .ok()?;

    if bytes.len() <= ICC_MEMO_MAX_BYTES
        && let Ok(mut guard) = icc_memo().lock()
    {
        let idx = guard.next % ICC_MEMO_SLOTS;
        guard.next = guard.next.wrapping_add(1);
        guard.slots[idx] = Some(IccMemoEntry {
            n,
            bytes: bytes.to_vec(),
            transform: Arc::clone(&transform),
            is_lab,
        });
    }

    Some((transform, is_lab))
}

fn valid_white(wp: Option<[f64; 3]>) -> [f32; 3] {
    match wp {
        Some([x, y, z]) if x > 0.0 && y > 0.0 && z > 0.0 => [x as f32, y as f32, z as f32],
        _ => D65_WHITE,
    }
}

#[inline]
fn clamp01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[inline]
fn adapt_bradford(xyz: [f32; 3], src_w: [f32; 3]) -> [f32; 3] {
    if (src_w[0] - D65_WHITE[0]).abs() < 1e-3
        && (src_w[1] - D65_WHITE[1]).abs() < 1e-3
        && (src_w[2] - D65_WHITE[2]).abs() < 1e-3
    {
        return xyz;
    }
    let ma = |v: [f32; 3]| -> [f32; 3] {
        [
            0.8951 * v[0] + 0.2664 * v[1] - 0.1614 * v[2],
            -0.7502 * v[0] + 1.7135 * v[1] + 0.0367 * v[2],
            0.0389 * v[0] - 0.0685 * v[1] + 1.0296 * v[2],
        ]
    };
    let rs = ma(src_w);
    let rd = ma(D65_WHITE);
    if rs[0].abs() < 1e-6 || rs[1].abs() < 1e-6 || rs[2].abs() < 1e-6 {
        return xyz;
    }
    let c = ma(xyz);
    let adapted = [
        c[0] * (rd[0] / rs[0]),
        c[1] * (rd[1] / rs[1]),
        c[2] * (rd[2] / rs[2]),
    ];
    [
        0.986_992_9 * adapted[0] - 0.147_054_3 * adapted[1] + 0.159_962_7 * adapted[2],
        0.432_305_3 * adapted[0] + 0.518_360_3 * adapted[1] + 0.049_291_2 * adapted[2],
        -0.008_528_7 * adapted[0] + 0.040_042_8 * adapted[1] + 0.968_486_7 * adapted[2],
    ]
}

#[inline]
fn linear_to_srgb(c: f32) -> f32 {
    let c = clamp01(c);
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
fn xyz_d65_to_srgb(xyz: [f32; 3]) -> [f32; 3] {
    let r = 3.240_454_2 * xyz[0] - 1.537_138_5 * xyz[1] - 0.498_531_4 * xyz[2];
    let g = -0.969_266 * xyz[0] + 1.876_010_8 * xyz[1] + 0.041_556 * xyz[2];
    let b = 0.055_643_4 * xyz[0] - 0.204_025_9 * xyz[1] + 1.057_225_2 * xyz[2];
    [linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b)]
}
