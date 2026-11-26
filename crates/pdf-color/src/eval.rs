//! Color space preparation and evaluation to normalized sRGB (`[f32; 3]` / `[u8; 3]`),
//! supporting both strict ISO 32000-2 (`Rules::Iso`) and PDFium (`Rules::Pdfium`, as of
//! chromium/8152) policies.

use std::fmt;
#[cfg(feature = "icc")]
use std::sync::Arc;

#[cfg(feature = "icc")]
use pdf_objects::Stream;
use pdf_objects::fault::Place;
use pdf_objects::fault::Sink;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Rules, Span, Streams, Unreadable, Why};

use crate::cmyk_lut::pdfium_cmyk_to_srgb;
use crate::fault::Fault;
use crate::function::{Function, clamp_pair};
use crate::limits::Limits;
use crate::resources::Spaces;
use crate::space::{
    Cie, Colorant, DeviceN, Family, Icc, Indexed, Separation, Space, read_number_vec,
    resolve_bounded,
};

const D65_WHITE: [f32; 3] = [0.950_47, 1.0, 1.088_83];

/// Options controlling how [`PreparedSpace`] resolves defaults and evaluates colors.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct EvalOptions<'o> {
    /// Reading/evaluation policy:
    /// - [`Rules::Iso`]: applies `/DefaultGray`, `/DefaultRGB`, `/DefaultCMYK` (§8.6.5.6),
    ///   optional destination `/OutputIntents` ICC profile (§14.11.5), `CalGray` `/Gamma` +
    ///   `/WhitePoint`, ISO §10.3.5 unprofiled CMYK (`1 - min(1, C + K)`), and `Separation /All`
    ///   neutral tint `1 - t`.
    /// - [`Rules::Pdfium`]: reproduces PDFium as of chromium/8152 (`AdobeCMYK_to_sRGB`, direct
    ///   `CalGray` passthrough, ignoring `/Default*` unless explicitly requested, and running
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

/// A compiled, thread-safe (`Send + Sync + Clone`) color space evaluator.
///
/// Decodes lookup tables, tint-transform functions, and (with `feature = "icc"`) ICC profiles once
/// at preparation time so per-pixel / per-scanline evaluation performs zero object lookups.
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
        target: Box<PreparedSpace>,
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
    SeparationAllIso,
    Tint {
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

    /// Pre-resolved default `/Decode` intervals `(min, max)` for an image of
    /// `bits_per_component` (`1..=32`) in this space (ISO 32000-2, Table 90).
    #[must_use]
    pub fn default_decode(&self, bits_per_component: u8) -> Option<Vec<(f64, f64)>> {
        if !(1..=32).contains(&bits_per_component) {
            return None;
        }
        if self.family == Family::Indexed {
            let hi = ((1_u64 << bits_per_component) - 1) as f64;
            return Some(vec![(0.0, hi)]);
        }
        self.decode_ranges.clone()
    }

    /// Evaluates `components` (`len >= self.components()`) to [`Paint`].
    #[must_use]
    pub fn eval(&self, components: &[f32], limits: &Limits) -> Option<Paint> {
        self.eval_inner(components, None, limits)
    }

    /// Evaluates `components` (`len >= self.components()`) to [`Paint`], overriding the active
    /// [`Rules`] (`Rules::Iso` vs `Rules::Pdfium`) on device, CIE, and base/alternate spaces.
    #[must_use]
    pub fn eval_with_rules(
        &self,
        components: &[f32],
        rules: Rules,
        limits: &Limits,
    ) -> Option<Paint> {
        self.eval_inner(components, Some(rules), limits)
    }

    fn eval_inner(
        &self,
        components: &[f32],
        rules_override: Option<Rules>,
        limits: &Limits,
    ) -> Option<Paint> {
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
            PreparedKind::Defaulted { target } => target.eval_inner(c, rules_override, limits),
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
                    _ => [
                        D65_WHITE[0] * inv_f(l),
                        D65_WHITE[1] * inv_f(m),
                        D65_WHITE[2] * inv_f(n),
                    ],
                };
                Some(Paint::Srgb(xyz_d65_to_srgb(d65)))
            }
            #[cfg(feature = "icc")]
            PreparedKind::Icc {
                n,
                is_lab_pcs_input,
                range,
                transform,
                fallback,
            } => {
                let mut src = [0.0_f32; 4];
                if *is_lab_pcs_input && *n == 3 {
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
                    fallback.eval_inner(c, rules_override, limits)
                }
            }
            PreparedKind::IccFallback { fallback } => {
                fallback.eval_inner(c, rules_override, limits)
            }
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
                base.eval_inner(dst, rules_override, limits)
            }
            PreparedKind::SeparationNone => Some(Paint::None),
            PreparedKind::SeparationAllIso => {
                let g = 1.0 - clamp01(c[0]);
                Some(Paint::Srgb([g, g, g]))
            }
            PreparedKind::Tint {
                all_none,
                tint,
                alternate,
            } => {
                if *all_none && rules_override.unwrap_or(Rules::Iso) == Rules::Iso {
                    return Some(Paint::None);
                }
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
                if !tint.eval(c, func_out, limits) {
                    return None;
                }
                alternate.eval_inner(&func_out[..alt_n], rules_override, limits)
            }
        }
    }

    /// Evaluates `components` directly to normalized sRGB (`[f32; 3]`), returning `None` if
    /// evaluation fails or produces [`Paint::None`].
    #[must_use]
    pub fn eval_srgb(&self, components: &[f32], limits: &Limits) -> Option<[f32; 3]> {
        self.eval(components, limits)?.srgb()
    }

    /// Evaluates `components` directly to quantized `sRGB8` (`[u8; 3]`), returning `None` if
    /// evaluation fails or produces [`Paint::None`].
    #[must_use]
    pub fn eval_srgb8(&self, components: &[f32], limits: &Limits) -> Option<[u8; 3]> {
        self.eval(components, limits)?.srgb8()
    }

    /// Batch-converts `out.len()` packed pixel component tuples from `samples`
    /// (`samples.len() >= out.len() * self.components()`) into `sRGB8`, filling `backdrop` for
    /// [`Paint::None`] pixels.
    ///
    /// Returns `false` if `samples` is too short or any pixel fails to evaluate.
    pub fn eval_pixels_srgb8(
        &self,
        samples: &[f32],
        out: &mut [[u8; 3]],
        backdrop: [u8; 3],
        limits: &Limits,
    ) -> bool {
        let stride = self.components as usize;
        let Some(needed) = out.len().checked_mul(stride) else {
            return false;
        };
        if samples.len() < needed {
            return false;
        }
        for (i, dst) in out.iter_mut().enumerate() {
            let start = i * stride;
            let Some(paint) = self.eval(&samples[start..start + stride], limits) else {
                return false;
            };
            *dst = paint.srgb8_over(backdrop);
        }
        true
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
            return Ok(Some(PreparedSpace {
                family,
                components,
                decode_ranges,
                kind: PreparedKind::Defaulted {
                    target: Box::new(target),
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

    fn unprofiled_device_fallback(&self, n: u32) -> PreparedSpace {
        let (family, components, kind) = match n {
            1 => (Family::DeviceGray, 1, PreparedKind::DeviceGray),
            3 => (Family::DeviceRGB, 3, PreparedKind::DeviceRGB),
            _ => (
                Family::DeviceCMYK,
                4,
                PreparedKind::DeviceCMYK {
                    rules: self.options.rules,
                },
            ),
        };
        PreparedSpace {
            family,
            components,
            decode_ranges: Some(vec![(0.0, 1.0); components as usize]),
            kind,
        }
    }

    fn prepare_cal_gray(&self, cie: &Cie<'o>) -> Result<PreparedSpace, Unreadable> {
        let white = valid_white(cie.white_point);
        let gamma = read_cal_gray_gamma(&self.objects, cie.dict, self.limits)?.unwrap_or(1.0);
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
        let gamma = read_fixed_f32::<3, _>(&self.objects, cie.dict, b"Gamma", self.limits)?
            .filter(|g| g.iter().all(|&v| v > 0.0))
            .unwrap_or([1.0, 1.0, 1.0]);
        let matrix = read_fixed_f32::<9, _>(&self.objects, cie.dict, b"Matrix", self.limits)?
            .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
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
            self.unprofiled_device_fallback(n)
        };

        #[cfg(feature = "icc")]
        if let Some(stream) = icc.stream {
            match decode_icc_stream(&self.objects, stream, icc.id, self.limits.stream_bytes)? {
                Some(bytes) => match open_icc_transform(&bytes, n, self.limits) {
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
                                range: range_f32,
                                transform,
                                fallback: Box::new(fallback),
                            },
                        }));
                    }
                    None => self.say(icc.id, None, Fault::IccProfile),
                },
                None => self.say(icc.id, None, Fault::IccProfile),
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
            Ok(None) => return Ok(None),
            Err(e) if matches!(e.why, Why::Damaged | Why::Filter | Why::Limit) => {
                let lookup_id = match &idx.lookup {
                    Some(crate::space::Lookup::Stream { id, .. }) => *id,
                    _ => None,
                };
                self.say(lookup_id, None, Fault::LookupLength);
                return Ok(None);
            }
            Err(e) => return Err(e),
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
            return Ok(Some(PreparedSpace {
                family: Family::Separation,
                components: 1,
                decode_ranges: Some(vec![(0.0, 1.0)]),
                kind: PreparedKind::SeparationAllIso,
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
        Ok(Some(PreparedSpace {
            family: Family::Separation,
            components: 1,
            decode_ranges: Some(vec![(0.0, 1.0)]),
            kind: PreparedKind::Tint {
                all_none: false,
                tint,
                alternate: Box::new(alternate),
            },
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
        let all_none = self.options.rules == Rules::Iso
            && dn.names.len() == m as usize
            && dn
                .names
                .iter()
                .all(|n| n.map(Colorant::of) == Some(Colorant::None));
        Ok(Some(PreparedSpace {
            family: Family::DeviceN,
            components: m,
            decode_ranges: Some(vec![(0.0, 1.0); m as usize]),
            kind: PreparedKind::Tint {
                all_none,
                tint,
                alternate: Box::new(alternate),
            },
        }))
    }
}

#[cfg(feature = "icc")]
fn decode_icc_stream<'o, S: Streams>(
    streams: &S,
    stream: &'o Stream,
    id: Option<ObjectId>,
    limit: usize,
) -> Result<Option<std::borrow::Cow<'o, [u8]>>, Unreadable> {
    if let Some(id) = id {
        match streams.decoded(id, limit) {
            Ok(b) => Ok(Some(std::borrow::Cow::Owned(b.into_owned()))),
            Err(e) if matches!(e.why, Why::Damaged | Why::Filter | Why::Limit) => Ok(None),
            Err(e) => Err(e),
        }
    } else if !stream.dict.has(b"Filter") {
        Ok(stream
            .data
            .bytes()
            .filter(|b| b.len() <= limit)
            .map(std::borrow::Cow::Borrowed))
    } else {
        Ok(None)
    }
}

#[cfg(feature = "icc")]
fn open_icc_transform(
    bytes: &[u8],
    n: u32,
    limits: &Limits,
) -> Option<(Arc<moxcms::TransformF32Executor>, bool)> {
    use moxcms::{ColorProfile, DataColorSpace, Layout, ParsingOptions, TransformOptions};

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
    Some((transform, is_lab))
}

fn valid_white(wp: Option<[f64; 3]>) -> [f32; 3] {
    match wp {
        Some([x, y, z]) if x > 0.0 && y > 0.0 && z > 0.0 => [x as f32, y as f32, z as f32],
        _ => D65_WHITE,
    }
}

fn read_cal_gray_gamma<'o, R: Resolve<'o>>(
    objects: &R,
    dict: Option<&'o Dict>,
    limits: &Limits,
) -> Result<Option<f32>, Unreadable> {
    let Some(entry) = dict.and_then(|d| d.get(b"Gamma")).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let mut visits = 0;
    let Some((obj, _)) = resolve_bounded(objects, entry, &mut visits, limits)? else {
        return Ok(None);
    };
    Ok(obj
        .as_number()
        .filter(|&g| g.is_finite() && g > 0.0)
        .map(|g| g as f32))
}

fn read_fixed_f32<'o, const N: usize, R: Resolve<'o>>(
    objects: &R,
    dict: Option<&'o Dict>,
    key: &[u8],
    limits: &Limits,
) -> Result<Option<[f32; N]>, Unreadable> {
    let Some(entry) = dict.and_then(|d| d.get(key)).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let Some(nums) = read_number_vec(objects, entry, N, limits)? else {
        return Ok(None);
    };
    let mut out = [0.0_f32; N];
    for (dst, src) in out.iter_mut().zip(nums) {
        *dst = src as f32;
    }
    Ok(Some(out))
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
