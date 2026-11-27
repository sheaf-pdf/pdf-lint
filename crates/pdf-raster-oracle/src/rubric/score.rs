//! Category-selectable `0–100` visual and ISO 32000-2 conformance rubric.
//!
//! Pure safe Rust (`#![forbid(unsafe_code)]`) implementing:
//! - Perceptual **Oklab `(L, a, b)`** pixel comparison (dual-backdrop when translucent);
//! - `3x3` gradient classification into **Interior Zone** vs. **Edge Band**, with
//!   bidirectional `1 px` Chamfer shift relaxation plus `3x3` local-mean ink-density
//!   matching to separate harmless anti-aliasing phase shifts from missing, displaced,
//!   or wrong small-text glyphs;
//! - `16x16` worst-tile + percentile pooling into three orthogonal sub-penalties
//!   (`struct_pen`, `color_pen`, `edge_pen`) and five [`Grade`] bands;
//! - Toggleable [`RubricCategories`] and [`RubricProfile`] presets (`ConformingCore`,
//!   `PdfiumWild`, `IsoNormative`, `WriteAndSanitize`);
//! - TSV baseline serialization, drift detection, and 4-up diagnostic diff strip rendering.

use std::fmt;
use std::ops::{BitAnd, BitOr, BitOrAssign};
use std::str::FromStr;
use std::sync::LazyLock;

use super::Error;
use super::image::{RgbaImage, RgbaImageExt};

/// Errors returned when parsing [`Grade`], [`RubricProfile`], or TSV baselines
/// ([`parse_baseline_tsv`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// Unrecognized [`Grade`] identifier.
    #[error("unknown rubric grade: {0:?}")]
    UnknownGrade(String),
    /// Unrecognized [`RubricProfile`] identifier.
    #[error("unknown rubric profile: {0:?}")]
    UnknownProfile(String),
    /// Non-comment TSV row did not contain 8 tab-separated columns.
    #[error("line {line}: expected 8 tab-separated columns, found {found}")]
    ColumnCount {
        /// 1-based line number.
        line: usize,
        /// Observed column count.
        found: usize,
    },
    /// Invalid field value on a TSV row.
    #[error("line {line}: bad {field} {value:?}: {reason}")]
    InvalidField {
        /// 1-based line number.
        line: usize,
        /// Column name.
        field: &'static str,
        /// Raw column text.
        value: String,
        /// Parse failure detail.
        reason: String,
    },
}

/// Qualitative grade band mapped from a `0.0..=100.0` [`VisualScore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grade {
    /// `< 75.0`: Missing/displaced glyphs or paths, inverted mask, wrong color space, or clip bug.
    Divergent,
    /// `75.0..92.0`: Visible stroke-weight, soft-mask falloff, or color-profile shift.
    Noticeable,
    /// `92.0..98.0`: Minor anti-aliasing coverage, hinting/stem-darkening, or bilinear seam drift.
    Subtle,
    /// `98.0..100.0`: Visually indistinguishable (`1–2` LSB LUT, IDCT, or rounding jitter).
    Imperceptible,
    /// `100.0`: Byte-for-byte identical (`max_channel_diff == 0`).
    Exact,
}

impl Grade {
    /// Classifies `score` (`0.0..=100.0`) and `max_channel_diff` into a [`Grade`].
    #[must_use]
    pub fn from_score(score: f32, max_channel_diff: u8) -> Grade {
        if max_channel_diff == 0 && score >= 100.0 {
            Grade::Exact
        } else if score >= 98.0 {
            Grade::Imperceptible
        } else if score >= 92.0 {
            Grade::Subtle
        } else if score >= 75.0 {
            Grade::Noticeable
        } else {
            Grade::Divergent
        }
    }

    /// Short stable identifier used in TSV baseline files.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Grade::Exact => "Exact",
            Grade::Imperceptible => "Imperceptible",
            Grade::Subtle => "Subtle",
            Grade::Noticeable => "Noticeable",
            Grade::Divergent => "Divergent",
        }
    }
}

impl fmt::Display for Grade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Grade {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Exact" => Ok(Grade::Exact),
            "Imperceptible" => Ok(Grade::Imperceptible),
            "Subtle" => Ok(Grade::Subtle),
            "Noticeable" => Ok(Grade::Noticeable),
            "Divergent" => Ok(Grade::Divergent),
            other => Err(ParseError::UnknownGrade(other.to_string())),
        }
    }
}

/// Decomposed `0.0..=100.0` visual comparison score between a candidate and reference image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisualScore {
    /// Composite visual score in `0.0..=100.0` (`100.0` iff `max_channel_diff == 0`).
    pub score: f32,
    /// Qualitative grade band.
    pub grade: Grade,
    /// Worst-tile localized structural penalty (`0.0..=60.0`), insensitive to `<= 1 px`
    /// edge shifts and `+/- 2` LSB rounding but highly sensitive to missing, extra, or
    /// wrong glyph/path marks.
    pub struct_pen: f32,
    /// Interior-zone color fidelity penalty (`0.0..=25.0`) measured in Oklab `Delta E_OK`.
    pub color_pen: f32,
    /// Edge-band anti-aliasing / subpixel coverage penalty (`0.0..=15.0`).
    pub edge_pen: f32,
    /// Maximum per-pixel Oklab Euclidean distance `Delta E_OK` across the image.
    pub max_delta_e: f32,
    /// Mean Oklab `Delta E_OK` across interior pixels.
    pub interior_mean_delta_e: f32,
    /// Maximum absolute single-channel `u8` difference (`0..=255`) in raw `RGBA8`.
    pub max_channel_diff: u8,
    /// `(tile_x, tile_y)` coordinates (`16x16` grid) of the highest-penalty structural tile.
    pub worst_tile: (u32, u32),
}

/// Like [`score_images`], but rejects uniform/blank `reference` images with
/// [`Error::UniformReference`] so differential oracle tests cannot pass vacuously
/// when both renderers emit a blank canvas.
///
/// # Errors
///
/// Returns [`Error::DimensionMismatch`] on mismatched dimensions or
/// [`Error::UniformReference`] if every pixel of `reference` is identical.
pub fn score_non_uniform_images(
    candidate: &RgbaImage,
    reference: &RgbaImage,
) -> Result<VisualScore, Error> {
    if reference.is_uniform() {
        return Err(Error::UniformReference);
    }
    score_images(candidate, reference)
}

/// Evaluates the `0.0..=100.0` perceptual [`VisualScore`] between `candidate` and `reference`.
///
/// When either image contains translucent pixels (`alpha < 255`), scoring runs against both an
/// opaque white backdrop and a contrasting dark checkerboard, returning the lower (worse) score so
/// alpha or premultiplication errors are never masked by white-on-white compositing.
///
/// # Errors
///
/// Returns [`Error::DimensionMismatch`] if `candidate` and `reference` have different dimensions.
pub fn score_images(candidate: &RgbaImage, reference: &RgbaImage) -> Result<VisualScore, Error> {
    let (w, h) = (candidate.width(), reference.height());
    if candidate.height() != h || reference.width() != w {
        return Err(Error::DimensionMismatch {
            cand_w: candidate.width(),
            cand_h: candidate.height(),
            ref_w: reference.width(),
            ref_h: reference.height(),
        });
    }

    let max_channel_diff = candidate
        .as_bytes()
        .iter()
        .zip(reference.as_bytes())
        .map(|(&a, &b)| a.abs_diff(b))
        .max()
        .unwrap_or(0);

    if max_channel_diff == 0 {
        return Ok(VisualScore {
            score: 100.0,
            grade: Grade::Exact,
            struct_pen: 0.0,
            color_pen: 0.0,
            edge_pen: 0.0,
            max_delta_e: 0.0,
            interior_mean_delta_e: 0.0,
            max_channel_diff: 0,
            worst_tile: (0, 0),
        });
    }

    let white_cand = candidate.composite_solid([255, 255, 255]);
    let white_ref = reference.composite_solid([255, 255, 255]);
    let white_score = score_composited(w, h, &white_cand, &white_ref, max_channel_diff);

    if candidate.is_opaque() && reference.is_opaque() {
        return Ok(white_score);
    }

    let check_cand = candidate.composite_checkerboard(8, [56, 56, 56], [112, 112, 112]);
    let check_ref = reference.composite_checkerboard(8, [56, 56, 56], [112, 112, 112]);
    let check_score = score_composited(w, h, &check_cand, &check_ref, max_channel_diff);

    if check_score.score < white_score.score {
        Ok(check_score)
    } else {
        Ok(white_score)
    }
}

fn score_composited(
    width: u32,
    height: u32,
    cand_rgb: &[[u8; 3]],
    ref_rgb: &[[u8; 3]],
    max_channel_diff: u8,
) -> VisualScore {
    let w = width as usize;
    let h = height as usize;
    let total = w * h;

    let cand_lab: Vec<[f32; 3]> = cand_rgb.iter().copied().map(srgb8_to_oklab).collect();
    let ref_lab: Vec<[f32; 3]> = ref_rgb.iter().copied().map(srgb8_to_oklab).collect();

    // 1. Raw edge detection where either image has > 0.05 Oklab neighbor contrast.
    let mut raw_edge = vec![false; total];
    for y in 0..h {
        let y0 = y.saturating_sub(1);
        let y1 = (y + 1).min(h - 1);
        for x in 0..w {
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let idx = y * w + x;
            let rc = ref_lab[idx];
            let cc = cand_lab[idx];
            let mut max_grad = 0.0_f32;
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    if nx == x && ny == y {
                        continue;
                    }
                    let nidx = ny * w + nx;
                    max_grad = max_grad
                        .max(delta_e_ok(rc, ref_lab[nidx]))
                        .max(delta_e_ok(cc, cand_lab[nidx]));
                }
            }
            raw_edge[idx] = max_grad > 0.05;
        }
    }

    // Dilate edge mask by 1 pixel so 1-px shifted edges sit inside the Edge Band.
    let mut is_edge = vec![false; total];
    for y in 0..h {
        let y0 = y.saturating_sub(1);
        let y1 = (y + 1).min(h - 1);
        for x in 0..w {
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let mut any = false;
            'nbr: for ny in y0..=y1 {
                for nx in x0..=x1 {
                    if raw_edge[ny * w + nx] {
                        any = true;
                        break 'nbr;
                    }
                }
            }
            is_edge[y * w + x] = any;
        }
    }

    // 2. Direct DeltaE and bidirectional 3x3 shift-relaxed Chamfer DeltaE.
    let mut direct_de = vec![0.0_f32; total];
    let mut shift_de = vec![0.0_f32; total];
    let mut max_delta_e = 0.0_f32;

    let mut interior_vals = Vec::new();
    let mut edge_vals = Vec::new();

    for y in 0..h {
        let y0 = y.saturating_sub(1);
        let y1 = (y + 1).min(h - 1);
        for x in 0..w {
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let idx = y * w + x;
            let c_px = cand_lab[idx];
            let r_px = ref_lab[idx];
            let d = delta_e_ok(c_px, r_px);
            direct_de[idx] = d;
            max_delta_e = max_delta_e.max(d);

            if d == 0.0 {
                shift_de[idx] = 0.0;
            } else {
                // Birchfield-Tomasi subpixel-interpolated 3x3 Oklab interval distance: an
                // anti-aliased edge coverage blend lies inside the 3x3 Oklab min..max hull of the
                // opposing image (zero structural penalty, scored purely by `edge_pen`), whereas a
                // missing mark or wrong color lies outside the opposing 3x3 hull.
                let mut ref_lo = r_px;
                let mut ref_hi = r_px;
                let mut cand_lo = c_px;
                let mut cand_hi = c_px;
                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let nidx = ny * w + nx;
                        let rn = ref_lab[nidx];
                        let cn = cand_lab[nidx];
                        for k in 0..3 {
                            ref_lo[k] = ref_lo[k].min(rn[k]);
                            ref_hi[k] = ref_hi[k].max(rn[k]);
                            cand_lo[k] = cand_lo[k].min(cn[k]);
                            cand_hi[k] = cand_hi[k].max(cn[k]);
                        }
                    }
                }
                let c2r = box_delta_e(c_px, ref_lo, ref_hi);
                let r2c = box_delta_e(r_px, cand_lo, cand_hi);
                shift_de[idx] = c2r.max(r2c);
            }

            if is_edge[idx] {
                edge_vals.push(d);
            } else {
                interior_vals.push(d);
            }
        }
    }

    // If a tiny swatch image has sharp color borders everywhere and 0 interior pixels, evaluate
    // interior color over pixels whose shift-relaxed error exceeds 0 (or all pixels).
    if interior_vals.is_empty() {
        interior_vals.extend_from_slice(&shift_de);
    }

    // 2b. 3x3 local-mean Oklab image + 1-px Birchfield-Tomasi hull comparison (`blur_shift_de`).
    // In edge-dense small text (e.g. 6x8 glyphs), every pixel's 3x3 raw hull spans `[black..white]`,
    // so point `shift_de` alone collapses near zero even when wrong glyphs are drawn. Comparing
    // 3x3 local Oklab means with 3x3 shift relaxation stays identically zero for any <= 1 px
    // translation or subpixel AA shift, while catching wrong glyph counters/stems!
    let blur_cand = box_blur_3x3(w, h, &cand_lab);
    let blur_ref = box_blur_3x3(w, h, &ref_lab);
    let mut blur_shift_de = vec![0.0_f32; total];
    for y in 0..h {
        let y0 = y.saturating_sub(1);
        let y1 = (y + 1).min(h - 1);
        for x in 0..w {
            let idx = y * w + x;
            let bc = blur_cand[idx];
            let br = blur_ref[idx];
            if delta_e_ok(bc, br) <= 0.08 {
                continue;
            }
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let mut r_lo = br;
            let mut r_hi = br;
            let mut c_lo = bc;
            let mut c_hi = bc;
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let nidx = ny * w + nx;
                    let rn = blur_ref[nidx];
                    let cn = blur_cand[nidx];
                    for k in 0..3 {
                        r_lo[k] = r_lo[k].min(rn[k]);
                        r_hi[k] = r_hi[k].max(rn[k]);
                        c_lo[k] = c_lo[k].min(cn[k]);
                        c_hi[k] = c_hi[k].max(cn[k]);
                    }
                }
            }
            blur_shift_de[idx] = box_delta_e(bc, r_lo, r_hi).max(box_delta_e(br, c_lo, c_hi));
        }
    }

    // 3. 16x16 tile structural pooling.
    let tiles_x = w.div_ceil(16);
    let tiles_y = h.div_ceil(16);
    let mut worst_struct = 0.0_f32;
    let mut worst_tile = (0_u32, 0_u32);
    let mut tile_buf = Vec::with_capacity(256);

    for ty in 0..tiles_y {
        let py0 = ty * 16;
        let py1 = (py0 + 16).min(h);
        for tx in 0..tiles_x {
            let px0 = tx * 16;
            let px1 = (px0 + 16).min(w);
            tile_buf.clear();
            let mut sum_sq = 0.0_f32;
            for py in py0..py1 {
                for px in px0..px1 {
                    let idx = py * w + px;
                    // Subtract 2-LSB quantization noise floor (0.012 in Oklab) on point shift_de,
                    // and 0.08 stem-darkening/hinting deadzone on 3x3 local-mean shift_de.
                    let point_excess = (shift_de[idx] - 0.012).max(0.0);
                    let density_excess = (blur_shift_de[idx] - 0.08).max(0.0) * 2.0;
                    let excess = point_excess.max(density_excess);
                    sum_sq += excess * excess;
                    tile_buf.push(excess);
                }
            }
            if tile_buf.is_empty() {
                continue;
            }
            tile_buf.sort_unstable_by(f32::total_cmp);
            let k = tile_buf.len().min(4);
            let top4_mean: f32 = tile_buf[tile_buf.len() - k..].iter().sum::<f32>() / (k as f32);
            let rms = (sum_sq / (tile_buf.len() as f32)).sqrt();
            let tile_val = 0.55 * top4_mean + 0.45 * rms;
            if tile_val > worst_struct {
                worst_struct = tile_val;
                worst_tile = (tx as u32, ty as u32);
            }
        }
    }

    let struct_pen = (worst_struct * 60.0).min(60.0);

    let int_mean = mean(&interior_vals);
    let int_p99 = percentile(&mut interior_vals, 0.99);
    let color_pen = (int_mean * 90.0 + int_p99 * 45.0).min(25.0);

    let (edge_mean, edge_p95) = if edge_vals.is_empty() {
        (0.0, 0.0)
    } else {
        let m = mean(&edge_vals);
        let p = percentile(&mut edge_vals, 0.95);
        (m, p)
    };
    let edge_pen = (edge_mean * 25.0 + edge_p95 * 12.0).min(15.0);

    let raw_score = (100.0 - struct_pen - color_pen - edge_pen).clamp(0.0, 99.99);
    let grade = Grade::from_score(raw_score, max_channel_diff);

    VisualScore {
        score: raw_score,
        grade,
        struct_pen,
        color_pen,
        edge_pen,
        max_delta_e,
        interior_mean_delta_e: int_mean,
        max_channel_diff,
        worst_tile,
    }
}

fn mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / (values.len() as f32)
    }
}

fn percentile(values: &mut [f32], q: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_unstable_by(f32::total_cmp);
    let idx = (((values.len() - 1) as f32) * q.clamp(0.0, 1.0)).round() as usize;
    values[idx.min(values.len() - 1)]
}

fn box_blur_3x3(w: usize, h: usize, lab: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0_f32; 3]; w * h];
    for y in 0..h {
        let y0 = y.saturating_sub(1);
        let y1 = (y + 1).min(h - 1);
        for x in 0..w {
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let mut sum = [0.0_f32; 3];
            let mut count = 0_u32;
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let px = lab[ny * w + nx];
                    sum[0] += px[0];
                    sum[1] += px[1];
                    sum[2] += px[2];
                    count += 1;
                }
            }
            let inv = 1.0 / (count as f32);
            out[y * w + x] = [sum[0] * inv, sum[1] * inv, sum[2] * inv];
        }
    }
    out
}

static SRGB_TO_LINEAR: LazyLock<[f32; 256]> = LazyLock::new(|| {
    std::array::from_fn(|c| {
        let s = (c as f32) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    })
});

#[inline]
fn srgb_u8_to_linear(c: u8) -> f32 {
    SRGB_TO_LINEAR[c as usize]
}

/// Converts an `sRGB8` pixel `[R, G, B]` into Björn Ottosson's perceptual **Oklab** `[L, a, b]`.
#[inline]
#[must_use]
pub fn srgb8_to_oklab(rgb: [u8; 3]) -> [f32; 3] {
    let r = srgb_u8_to_linear(rgb[0]);
    let g = srgb_u8_to_linear(rgb[1]);
    let b = srgb_u8_to_linear(rgb[2]);

    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();

    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

#[inline]
fn delta_e_ok(u: [f32; 3], v: [f32; 3]) -> f32 {
    let dl = u[0] - v[0];
    let da = u[1] - v[1];
    let db = u[2] - v[2];
    (dl * dl + da * da + db * db).sqrt()
}

#[inline]
fn box_delta_e(p: [f32; 3], lo: [f32; 3], hi: [f32; 3]) -> f32 {
    let mut sq = 0.0_f32;
    for k in 0..3 {
        let d = (lo[k] - p[k]).max(p[k] - hi[k]).max(0.0);
        sq += d * d;
    }
    sq.sqrt()
}

/// Renders a 4-panel horizontal diagnostic strip (`Candidate | Reference | 10x Oklab Heatmap | Edge Mask`)
/// to aid debugging visual score regressions.
///
/// # Errors
///
/// Returns [`Error::DimensionMismatch`] or [`Error::InvalidDimensions`] if image sizes mismatch
/// or the combined strip exceeds [`super::MAX_PIXELS`].
pub fn render_diff_strip(candidate: &RgbaImage, reference: &RgbaImage) -> Result<RgbaImage, Error> {
    let (w, h) = (candidate.width(), reference.height());
    if candidate.height() != h || reference.width() != w {
        return Err(Error::DimensionMismatch {
            cand_w: candidate.width(),
            cand_h: candidate.height(),
            ref_w: reference.width(),
            ref_h: reference.height(),
        });
    }
    let strip_w = w.checked_mul(4).ok_or(Error::InvalidDimensions {
        width: w,
        height: h,
    })?;
    let mut out = RgbaImage::from_solid(strip_w, h, [0, 0, 0, 255])?;
    let c_rgb = candidate.composite_solid([255, 255, 255]);
    let r_rgb = reference.composite_solid([255, 255, 255]);

    for y in 0..h {
        for x in 0..w {
            let idx = (y as usize) * (w as usize) + (x as usize);
            let cp = c_rgb[idx];
            let rp = r_rgb[idx];
            out.set_pixel(x, y, [cp[0], cp[1], cp[2], 255]);
            out.set_pixel(w + x, y, [rp[0], rp[1], rp[2], 255]);

            let de = delta_e_ok(srgb8_to_oklab(cp), srgb8_to_oklab(rp));
            let heat = (de * 10.0).clamp(0.0, 1.0);
            let heat_px = [
                (heat * 255.0).round() as u8,
                ((1.0 - (heat - 0.5).abs() * 2.0).max(0.0) * 180.0).round() as u8,
                ((1.0 - heat) * 80.0).round() as u8,
                255,
            ];
            out.set_pixel(w * 2 + x, y, heat_px);

            let diff = cp[0]
                .abs_diff(rp[0])
                .max(cp[1].abs_diff(rp[1]))
                .max(cp[2].abs_diff(rp[2]));
            let mask_v = if diff == 0 { 24 } else { 255 };
            out.set_pixel(w * 3 + x, y, [mask_v, mask_v, mask_v, 255]);
        }
    }
    Ok(out)
}

/// Toggleable evaluation categories for reconciling PDFium compatibility with ISO 32000-2 rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RubricCategories(u32);

impl RubricCategories {
    /// Empty category mask.
    pub const NONE: RubricCategories = RubricCategories(0);
    /// Compare `Rules::Pdfium` output against `libpdfium` (`chromium/8152`).
    pub const VISUAL_PDFIUM: RubricCategories = RubricCategories(1 << 0);
    /// Compare `Rules::Iso` output against an analytic/normative ISO 32000-2 reference image.
    pub const VISUAL_ISO_REF: RubricCategories = RubricCategories(1 << 1);
    /// Compare `Rules::Iso` output directly against `Rules::Pdfium` output (`Iso == Pdfium` on
    /// conforming inputs where the two policies should not diverge).
    pub const SELF_EQUIVALENCE: RubricCategories = RubricCategories(1 << 2);
    /// Assert exact expected `Fault` counts/set when reading under `Rules::Iso`.
    pub const ISO_READ_FAULTS: RubricCategories = RubricCategories(1 << 3);
    /// Assert zero `pdf-lint` / `pdf-arlington` errors on written/mutated PDF output.
    pub const ISO_WRITE_LINT: RubricCategories = RubricCategories(1 << 4);
    /// Assert that `Rules::Iso` and `Rules::Pdfium` intentionally diverge on a known viewer quirk.
    pub const PDFIUM_DIVERGENCE: RubricCategories = RubricCategories(1 << 5);

    /// Raw bitmask.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Combines two category masks (`const`-friendly).
    #[must_use]
    pub const fn union(self, other: RubricCategories) -> RubricCategories {
        RubricCategories(self.0 | other.0)
    }

    /// Removes `other` from `self` (`const`-friendly).
    #[must_use]
    pub const fn without(self, other: RubricCategories) -> RubricCategories {
        RubricCategories(self.0 & !other.0)
    }

    /// Returns `true` if all bits in `other` are enabled in `self`.
    #[must_use]
    pub const fn contains(self, other: RubricCategories) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl BitOr for RubricCategories {
    type Output = RubricCategories;

    fn bitor(self, rhs: RubricCategories) -> RubricCategories {
        self.union(rhs)
    }
}

impl BitOrAssign for RubricCategories {
    fn bitor_assign(&mut self, rhs: RubricCategories) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for RubricCategories {
    type Output = RubricCategories;

    fn bitand(self, rhs: RubricCategories) -> RubricCategories {
        RubricCategories(self.0 & rhs.0)
    }
}

/// Standard named category presets covering the workspace's four conformance regimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RubricProfile {
    /// Clean ISO 32000-2 input where ISO and PDFium agree:
    /// `VISUAL_PDFIUM | SELF_EQUIVALENCE | ISO_READ_FAULTS`.
    ConformingCore,
    /// Malformed or viewer-quirk input where PDFium deliberately departs from ISO 32000-2:
    /// `VISUAL_PDFIUM | PDFIUM_DIVERGENCE`.
    PdfiumWild,
    /// Pure normative ISO 32000-2 feature where PDFium differs or lacks support (e.g. `/Default*`
    /// color spaces, `/All` spot tint, or unprofiled ISO CMYK):
    /// `VISUAL_ISO_REF | ISO_READ_FAULTS`.
    IsoNormative,
    /// PDF writing, appearance synthesis, flattening, or image sanitization:
    /// `VISUAL_PDFIUM | ISO_READ_FAULTS | ISO_WRITE_LINT`.
    WriteAndSanitize,
}

impl RubricProfile {
    /// Default [`RubricCategories`] enabled by this preset.
    #[must_use]
    pub const fn categories(self) -> RubricCategories {
        match self {
            RubricProfile::ConformingCore => RubricCategories::VISUAL_PDFIUM
                .union(RubricCategories::SELF_EQUIVALENCE)
                .union(RubricCategories::ISO_READ_FAULTS),
            RubricProfile::PdfiumWild => {
                RubricCategories::VISUAL_PDFIUM.union(RubricCategories::PDFIUM_DIVERGENCE)
            }
            RubricProfile::IsoNormative => {
                RubricCategories::VISUAL_ISO_REF.union(RubricCategories::ISO_READ_FAULTS)
            }
            RubricProfile::WriteAndSanitize => RubricCategories::VISUAL_PDFIUM
                .union(RubricCategories::ISO_READ_FAULTS)
                .union(RubricCategories::ISO_WRITE_LINT),
        }
    }

    /// Default [`RubricConfig`] thresholds for this preset.
    #[must_use]
    pub const fn config(self) -> RubricConfig {
        let visual_floor = match self {
            RubricProfile::PdfiumWild => 92.0,
            RubricProfile::IsoNormative => 96.0,
            RubricProfile::ConformingCore | RubricProfile::WriteAndSanitize => 95.0,
        };
        RubricConfig {
            categories: self.categories(),
            visual_pdfium_floor: visual_floor,
            visual_iso_floor: 96.0,
            self_equivalence_floor: 99.5,
            expected_iso_faults: 0,
            expected_lint_errors: 0,
        }
    }

    /// Stable name for TSV baseline rows.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RubricProfile::ConformingCore => "ConformingCore",
            RubricProfile::PdfiumWild => "PdfiumWild",
            RubricProfile::IsoNormative => "IsoNormative",
            RubricProfile::WriteAndSanitize => "WriteAndSanitize",
        }
    }
}

impl fmt::Display for RubricProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RubricProfile {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ConformingCore" => Ok(RubricProfile::ConformingCore),
            "PdfiumWild" => Ok(RubricProfile::PdfiumWild),
            "IsoNormative" => Ok(RubricProfile::IsoNormative),
            "WriteAndSanitize" => Ok(RubricProfile::WriteAndSanitize),
            other => Err(ParseError::UnknownProfile(other.to_string())),
        }
    }
}

/// Configurable category mask and pass/fail thresholds for [`evaluate_case`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RubricConfig {
    /// Enabled rubric categories.
    pub categories: RubricCategories,
    /// Minimum required visual score (`0..=100`) when [`RubricCategories::VISUAL_PDFIUM`] is set.
    pub visual_pdfium_floor: f32,
    /// Minimum required visual score (`0..=100`) when [`RubricCategories::VISUAL_ISO_REF`] is set.
    pub visual_iso_floor: f32,
    /// Minimum required visual score between `Iso` and `Pdfium` outputs when
    /// [`RubricCategories::SELF_EQUIVALENCE`] is set.
    pub self_equivalence_floor: f32,
    /// Exact expected `Fault` count under `Rules::Iso` when [`RubricCategories::ISO_READ_FAULTS`]
    /// is set.
    pub expected_iso_faults: usize,
    /// Exact expected lint violation count when [`RubricCategories::ISO_WRITE_LINT`] is set.
    pub expected_lint_errors: usize,
}

/// Rendered outputs and diagnostic counts supplied to [`evaluate_case`].
#[derive(Debug, Clone)]
pub struct CaseEvaluation<'a> {
    /// Candidate image produced under `Rules::Pdfium` (used by `VISUAL_PDFIUM`,
    /// `SELF_EQUIVALENCE`, and `PDFIUM_DIVERGENCE`).
    pub candidate_pdfium: Option<&'a RgbaImage>,
    /// Candidate image produced under `Rules::Iso` (used by `VISUAL_ISO_REF`,
    /// `SELF_EQUIVALENCE`, and `PDFIUM_DIVERGENCE`).
    pub candidate_iso: Option<&'a RgbaImage>,
    /// Ground-truth image rendered by `libpdfium` (`chromium/8152`).
    pub pdfium_reference: Option<&'a RgbaImage>,
    /// Analytic / normative ISO 32000-2 reference image.
    pub iso_reference: Option<&'a RgbaImage>,
    /// Number of `Fault`s emitted while reading under `Rules::Iso`.
    pub iso_fault_count: usize,
    /// Number of `pdf-lint` / `pdf-arlington` violations on written PDF output.
    pub write_lint_errors: usize,
}

/// Result of running [`evaluate_case`] against a [`RubricConfig`].
#[derive(Debug, Clone, PartialEq)]
pub struct CaseOutcome {
    /// `true` iff every enabled category satisfied its threshold/invariant.
    pub passed: bool,
    /// Score of `candidate_pdfium` vs `pdfium_reference` (if `VISUAL_PDFIUM` enabled).
    pub visual_pdfium: Option<VisualScore>,
    /// Score of `candidate_iso` vs `iso_reference` (if `VISUAL_ISO_REF` enabled).
    pub visual_iso: Option<VisualScore>,
    /// Score of `candidate_iso` vs `candidate_pdfium` (if `SELF_EQUIVALENCE` or
    /// `PDFIUM_DIVERGENCE` enabled).
    pub iso_vs_pdfium: Option<VisualScore>,
    /// Human-readable failure reasons for any category that did not pass.
    pub failures: Vec<String>,
}

/// Evaluates `case` under the categories and floors configured in `config`.
///
/// # Errors
///
/// Returns [`Error::DimensionMismatch`] if any compared image pair has mismatched dimensions.
pub fn evaluate_case(
    config: &RubricConfig,
    case: &CaseEvaluation<'_>,
) -> Result<CaseOutcome, Error> {
    let cats = config.categories;
    let mut failures = Vec::new();
    let mut visual_pdfium = None;
    let mut visual_iso = None;
    let mut iso_vs_pdfium = None;

    if cats.contains(RubricCategories::VISUAL_PDFIUM) {
        match (case.candidate_pdfium, case.pdfium_reference) {
            (Some(cand), Some(reference)) => {
                let s = score_images(cand, reference)?;
                if s.score < config.visual_pdfium_floor {
                    failures.push(format!(
                        "VISUAL_PDFIUM score {:.2} ({}) < floor {:.2} (struct={:.2}, color={:.2}, edge={:.2})",
                        s.score, s.grade, config.visual_pdfium_floor, s.struct_pen, s.color_pen, s.edge_pen
                    ));
                }
                visual_pdfium = Some(s);
            }
            _ => failures.push(
                "VISUAL_PDFIUM enabled but candidate_pdfium or pdfium_reference missing"
                    .to_string(),
            ),
        }
    }

    if cats.contains(RubricCategories::VISUAL_ISO_REF) {
        match (case.candidate_iso, case.iso_reference) {
            (Some(cand), Some(reference)) => {
                let s = score_images(cand, reference)?;
                if s.score < config.visual_iso_floor {
                    failures.push(format!(
                        "VISUAL_ISO_REF score {:.2} ({}) < floor {:.2} (struct={:.2}, color={:.2}, edge={:.2})",
                        s.score, s.grade, config.visual_iso_floor, s.struct_pen, s.color_pen, s.edge_pen
                    ));
                }
                visual_iso = Some(s);
            }
            _ => failures.push(
                "VISUAL_ISO_REF enabled but candidate_iso or iso_reference missing".to_string(),
            ),
        }
    }

    if cats.contains(RubricCategories::SELF_EQUIVALENCE)
        || cats.contains(RubricCategories::PDFIUM_DIVERGENCE)
    {
        match (case.candidate_iso, case.candidate_pdfium) {
            (Some(iso_img), Some(pdfium_img)) => {
                let s = score_images(iso_img, pdfium_img)?;
                if cats.contains(RubricCategories::SELF_EQUIVALENCE)
                    && s.score < config.self_equivalence_floor
                {
                    failures.push(format!(
                        "SELF_EQUIVALENCE score {:.2} < floor {:.2} (Rules::Iso unexpectedly diverged from Rules::Pdfium)",
                        s.score, config.self_equivalence_floor
                    ));
                }
                if cats.contains(RubricCategories::PDFIUM_DIVERGENCE)
                    && s.max_channel_diff == 0
                    && case.iso_fault_count == 0
                {
                    failures.push(
                        "PDFIUM_DIVERGENCE enabled, but Rules::Iso produced identical output with 0 faults (promote to ConformingCore)"
                            .to_string(),
                    );
                }
                iso_vs_pdfium = Some(s);
            }
            _ if cats.contains(RubricCategories::SELF_EQUIVALENCE) => {
                failures.push(
                    "SELF_EQUIVALENCE enabled but candidate_iso or candidate_pdfium missing"
                        .to_string(),
                );
            }
            _ => {
                // PDFIUM_DIVERGENCE can also be satisfied via non-zero ISO faults alone when no
                // separate ISO render is supplied.
                if case.iso_fault_count == 0 {
                    failures.push(
                        "PDFIUM_DIVERGENCE enabled with neither ISO image divergence nor ISO read faults"
                            .to_string(),
                    );
                }
            }
        }
    }

    if cats.contains(RubricCategories::ISO_READ_FAULTS)
        && case.iso_fault_count != config.expected_iso_faults
    {
        failures.push(format!(
            "ISO_READ_FAULTS count {} != expected {}",
            case.iso_fault_count, config.expected_iso_faults
        ));
    }

    if cats.contains(RubricCategories::ISO_WRITE_LINT)
        && case.write_lint_errors != config.expected_lint_errors
    {
        failures.push(format!(
            "ISO_WRITE_LINT errors {} != expected {}",
            case.write_lint_errors, config.expected_lint_errors
        ));
    }

    Ok(CaseOutcome {
        passed: failures.is_empty(),
        visual_pdfium,
        visual_iso,
        iso_vs_pdfium,
        failures,
    })
}

/// One recorded row in a committed `chromium-8152-scores.tsv` baseline file.
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineRow {
    /// Stable test-case identifier.
    pub case: String,
    /// Rubric preset under which the score was recorded.
    pub profile: RubricProfile,
    /// Recorded composite visual score (`0.0..=100.0`).
    pub score: f32,
    /// Recorded qualitative grade.
    pub grade: Grade,
    /// Recorded structural sub-penalty.
    pub struct_pen: f32,
    /// Recorded interior color sub-penalty.
    pub color_pen: f32,
    /// Recorded edge anti-aliasing sub-penalty.
    pub edge_pen: f32,
    /// Recorded maximum Oklab `Delta E_OK`.
    pub max_delta_e: f32,
}

/// Drift comparison between a newly measured [`BaselineRow`] and a committed baseline row.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftReport {
    /// Case name.
    pub case: String,
    /// Signed score delta (`measured.score - recorded.score`).
    pub delta_score: f32,
    /// Whether `delta_score.abs() <= tolerance`.
    pub within_tolerance: bool,
}

impl BaselineRow {
    /// Builds a [`BaselineRow`] from a measured [`VisualScore`].
    #[must_use]
    pub fn from_visual(
        case: impl Into<String>,
        profile: RubricProfile,
        score: &VisualScore,
    ) -> BaselineRow {
        BaselineRow {
            case: case.into(),
            profile,
            score: score.score,
            grade: score.grade,
            struct_pen: score.struct_pen,
            color_pen: score.color_pen,
            edge_pen: score.edge_pen,
            max_delta_e: score.max_delta_e,
        }
    }

    /// Compares `self` (newly measured) against `recorded` under `tolerance` score points.
    #[must_use]
    pub fn compare_drift(&self, recorded: &BaselineRow, tolerance: f32) -> DriftReport {
        let delta_score = self.score - recorded.score;
        DriftReport {
            case: self.case.clone(),
            delta_score,
            within_tolerance: delta_score.abs() <= tolerance,
        }
    }
}

/// Formats [`BaselineRow`] entries as deterministic TSV lines suitable for git diffing.
#[must_use]
pub fn format_baseline_tsv(rows: &[BaselineRow]) -> String {
    let mut out = String::from(
        "# case\tprofile\tscore\tgrade\tstruct_pen\tcolor_pen\tedge_pen\tmax_delta_e\n",
    );
    for r in rows {
        out.push_str(&format!(
            "{}\t{}\t{:.2}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.4}\n",
            r.case,
            r.profile,
            r.score,
            r.grade,
            r.struct_pen,
            r.color_pen,
            r.edge_pen,
            r.max_delta_e,
        ));
    }
    out
}

/// Parses TSV content produced by [`format_baseline_tsv`].
///
/// # Errors
///
/// Returns [`ParseError`] if any non-comment row is malformed.
pub fn parse_baseline_tsv(tsv: &str) -> Result<Vec<BaselineRow>, ParseError> {
    let mut rows = Vec::new();
    for (idx, line) in tsv.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 8 {
            return Err(ParseError::ColumnCount {
                line: line_no,
                found: cols.len(),
            });
        }
        let parse_f32 = |col: &str, field: &'static str| -> Result<f32, ParseError> {
            col.parse::<f32>().map_err(|e| ParseError::InvalidField {
                line: line_no,
                field,
                value: col.to_string(),
                reason: e.to_string(),
            })
        };
        rows.push(BaselineRow {
            case: cols[0].to_string(),
            profile: cols[1]
                .parse()
                .map_err(|e: ParseError| ParseError::InvalidField {
                    line: line_no,
                    field: "profile",
                    value: cols[1].to_string(),
                    reason: e.to_string(),
                })?,
            score: parse_f32(cols[2], "score")?,
            grade: cols[3]
                .parse()
                .map_err(|e: ParseError| ParseError::InvalidField {
                    line: line_no,
                    field: "grade",
                    value: cols[3].to_string(),
                    reason: e.to_string(),
                })?,
            struct_pen: parse_f32(cols[4], "struct_pen")?,
            color_pen: parse_f32(cols[5], "color_pen")?,
            edge_pen: parse_f32(cols[6], "edge_pen")?,
            max_delta_e: parse_f32(cols[7], "max_delta_e")?,
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_scores_100_and_1_lsb_never_scores_exact() {
        let a = RgbaImage::from_solid(32, 32, [120, 160, 200, 255]).unwrap();
        let mut b = a.clone();
        let exact = score_images(&a, &b).unwrap();
        assert_eq!(exact.score, 100.0);
        assert_eq!(exact.grade, Grade::Exact);
        assert_eq!(exact.max_channel_diff, 0);

        // Perturb 1 pixel by 1 LSB: must be Imperceptible (>= 98.0) and strictly < 100.0.
        b.set_pixel(10, 10, [121, 160, 200, 255]);
        let one_lsb = score_images(&b, &a).unwrap();
        assert_eq!(one_lsb.grade, Grade::Imperceptible, "{one_lsb:?}");
        assert!(
            one_lsb.score >= 99.0 && one_lsb.score < 100.0,
            "{one_lsb:?}"
        );
    }

    #[test]
    fn uniform_2_lsb_lut_jitter_scores_imperceptible() {
        let mut reference = RgbaImage::from_solid(64, 64, [100, 140, 180, 255]).unwrap();
        let mut candidate = reference.clone();
        for y in 0..64 {
            for x in 0..64 {
                let base = [
                    (40 + x * 2) as u8,
                    (60 + y * 2) as u8,
                    (120 + ((x + y) / 2)) as u8,
                    255,
                ];
                reference.set_pixel(x, y, base);
                let jitter = if (x + y) % 2 == 0 { 1_i16 } else { -2_i16 };
                candidate.set_pixel(
                    x,
                    y,
                    [
                        (i16::from(base[0]) + jitter) as u8,
                        (i16::from(base[1]) - jitter) as u8,
                        base[2],
                        255,
                    ],
                );
            }
        }
        let s = score_images(&candidate, &reference).unwrap();
        assert_eq!(s.grade, Grade::Imperceptible, "{s:?}");
        assert_eq!(s.struct_pen, 0.0, "{s:?}");
    }

    #[test]
    fn subpixel_edge_aa_shift_scores_subtle_without_structural_penalty() {
        // Vertical dark bar on white background with slightly different AA edge coverage (~0.3 px).
        let mut reference = RgbaImage::from_solid(64, 64, [255, 255, 255, 255]).unwrap();
        reference.fill_rect(20, 8, 24, 48, [20, 20, 20, 255]);
        for y in 8..56 {
            reference.set_pixel(19, y, [180, 180, 180, 255]);
            reference.set_pixel(44, y, [180, 180, 180, 255]);
        }

        let mut candidate = reference.clone();
        for y in 8..56 {
            // Shift coverage by ~0.3 px on the anti-aliased boundary only.
            candidate.set_pixel(19, y, [115, 115, 115, 255]);
            candidate.set_pixel(44, y, [225, 225, 225, 255]);
        }

        let s = score_images(&candidate, &reference).unwrap();
        assert!(
            matches!(s.grade, Grade::Subtle | Grade::Imperceptible),
            "{s:?}"
        );
        assert!(s.score >= 92.0, "{s:?}");
        assert!(
            s.struct_pen < 1.0,
            "1px AA shift must not trigger struct_pen: {s:?}"
        );
        assert_eq!(s.color_pen, 0.0, "interior is untouched: {s:?}");
    }

    #[test]
    fn missing_four_pixel_minus_sign_on_white_page_scores_divergent() {
        // 99.998% identical 400x500 white page, except candidate dropped a 4-pixel minus sign!
        // Naive whole-page PSNR / pixel-match rate would score this 99.998% ("pass"); our
        // worst-tile structural rubric must flag it as `Divergent` (< 75.0).
        let candidate = RgbaImage::from_solid(400, 500, [255, 255, 255, 255]).unwrap();
        let mut reference = candidate.clone();
        reference.fill_rect(198, 244, 4, 1, [0, 0, 0, 255]);

        let s = score_images(&candidate, &reference).unwrap();
        assert_eq!(s.grade, Grade::Divergent, "{s:?}");
        assert!(s.score < 75.0, "{s:?}");
        assert!(s.struct_pen > 30.0, "{s:?}");
        assert_eq!(s.worst_tile, (198 / 16, 244 / 16));
    }

    #[test]
    fn wrong_small_text_glyphs_in_edge_dense_tile_score_divergent() {
        // Edge-dense 6x8 small-text glyphs ("OOOO" hollow counters vs "XXXX" crossed strokes):
        // every pixel has both black ink and white paper in its 3x3 neighborhood, yet 3x3
        // local-mean hull scoring detects the wrong glyph topology as `Divergent` (< 75.0).
        let mut reference = RgbaImage::from_solid(48, 16, [255, 255, 255, 255]).unwrap();
        let mut candidate = reference.clone();
        for i in 0..4_u32 {
            let gx = 4 + i * 10;
            let gy = 4;
            // Hollow 6x8 'O' in reference:
            reference.fill_rect(gx, gy, 6, 1, [0, 0, 0, 255]);
            reference.fill_rect(gx, gy + 7, 6, 1, [0, 0, 0, 255]);
            reference.fill_rect(gx, gy, 1, 8, [0, 0, 0, 255]);
            reference.fill_rect(gx + 5, gy, 1, 8, [0, 0, 0, 255]);
            // Crossed/filled 'X'/box in candidate:
            for row in 0..8_u32 {
                let c0 = (row * 5) / 7;
                let c1 = 5 - c0;
                candidate.fill_rect(gx + c0.min(4), gy + row, 2, 1, [0, 0, 0, 255]);
                candidate.fill_rect(gx + c1.min(4), gy + row, 2, 1, [0, 0, 0, 255]);
            }
        }
        let s = score_images(&candidate, &reference).unwrap();
        assert_eq!(
            s.grade,
            Grade::Divergent,
            "wrong small glyphs must be Divergent: {s:?}"
        );
        assert!(s.score < 75.0, "{s:?}");
        assert!(s.struct_pen > 20.0, "{s:?}");
    }

    #[test]
    fn uniform_reference_guard_and_typed_parse_errors() {
        let blank_a = RgbaImage::from_solid(16, 16, [255, 255, 255, 255]).unwrap();
        let blank_b = blank_a.clone();
        assert_eq!(
            score_non_uniform_images(&blank_a, &blank_b),
            Err(Error::UniformReference)
        );
        assert!(matches!(
            "Bogus".parse::<Grade>(),
            Err(ParseError::UnknownGrade(_))
        ));
        assert!(matches!(
            "Bogus".parse::<RubricProfile>(),
            Err(ParseError::UnknownProfile(_))
        ));
        assert!(matches!(
            parse_baseline_tsv("only\ttwo\n"),
            Err(ParseError::ColumnCount { line: 1, found: 2 })
        ));
    }

    #[test]
    fn wrong_cmyk_swatch_triggers_color_penalty() {
        // Adobe CMYK cyan (0, 174, 239) vs naive subtractive cyan (0, 255, 255).
        let reference = RgbaImage::from_solid(32, 32, [0, 174, 239, 255]).unwrap();
        let candidate = RgbaImage::from_solid(32, 32, [0, 255, 255, 255]).unwrap();
        let s = score_images(&candidate, &reference).unwrap();
        assert!(s.color_pen > 15.0, "{s:?}");
        assert!(s.score < 80.0, "{s:?}");
    }

    #[test]
    fn dual_backdrop_catches_white_on_white_alpha_bug() {
        // Opaque white vs completely transparent white: identical over white, completely different
        // over dark checkerboard!
        let opaque_white = RgbaImage::from_solid(16, 16, [255, 255, 255, 255]).unwrap();
        let transparent = RgbaImage::from_solid(16, 16, [255, 255, 255, 0]).unwrap();
        let s = score_images(&transparent, &opaque_white).unwrap();
        assert_eq!(s.grade, Grade::Divergent, "{s:?}");
        assert!(s.score < 50.0, "{s:?}");
    }

    #[test]
    fn rubric_profiles_and_tsv_roundtrip() {
        let img_a = RgbaImage::from_solid(16, 16, [40, 120, 200, 255]).unwrap();
        let img_b = RgbaImage::from_solid(16, 16, [41, 120, 200, 255]).unwrap();

        // ConformingCore passes when Iso == Pdfium, Pdfium matches ref, and 0 faults.
        let core_cfg = RubricProfile::ConformingCore.config();
        let out = evaluate_case(
            &core_cfg,
            &CaseEvaluation {
                candidate_pdfium: Some(&img_b),
                candidate_iso: Some(&img_b),
                pdfium_reference: Some(&img_a),
                iso_reference: None,
                iso_fault_count: 0,
                write_lint_errors: 0,
            },
        )
        .unwrap();
        assert!(out.passed, "{out:?}");

        // PdfiumWild requires divergence (either visual difference between Iso and Pdfium or faults).
        let wild_cfg = RubricProfile::PdfiumWild.config();
        let wild_fail = evaluate_case(
            &wild_cfg,
            &CaseEvaluation {
                candidate_pdfium: Some(&img_a),
                candidate_iso: Some(&img_a),
                pdfium_reference: Some(&img_a),
                iso_reference: None,
                iso_fault_count: 0,
                write_lint_errors: 0,
            },
        )
        .unwrap();
        assert!(!wild_fail.passed);

        let wild_ok = evaluate_case(
            &wild_cfg,
            &CaseEvaluation {
                candidate_pdfium: Some(&img_a),
                candidate_iso: Some(&RgbaImage::from_solid(16, 16, [0, 255, 255, 255]).unwrap()),
                pdfium_reference: Some(&img_a),
                iso_reference: None,
                iso_fault_count: 1,
                write_lint_errors: 0,
            },
        )
        .unwrap();
        assert!(wild_ok.passed, "{wild_ok:?}");

        // TSV round-trip + drift check + diff strip.
        let row = BaselineRow::from_visual(
            "cmyk_cyan",
            RubricProfile::ConformingCore,
            out.visual_pdfium.as_ref().unwrap(),
        );
        let tsv = format_baseline_tsv(std::slice::from_ref(&row));
        let parsed = parse_baseline_tsv(&tsv).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(row.compare_drift(&parsed[0], 0.05).within_tolerance);

        let strip = render_diff_strip(&img_b, &img_a).unwrap();
        assert_eq!((strip.width(), strip.height()), (64, 16));
    }
}
