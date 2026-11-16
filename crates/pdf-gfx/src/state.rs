//! Graphics, color, transparency, and text state (`CPDF_AllStates`).
//!
//! Tracks everything saved by `q` and restored by `Q` across content streams
//! and Form XObjects. Heap-backed fields use `Arc` or inline small-buffer
//! storage so `State::clone()` on `q` / `Q` is `O(1)` without heap allocation,
//! and every `Debug` implementation omits raw resource names and file strings.

use std::fmt;
use std::sync::Arc;

use pdf_content_streams::{
    Blend, FillRule, Limits as ContentLimits, LineCap, LineJoin, Operand, Operation, Operator,
    TextMode,
};
use pdf_fonts::FontId;
use pdf_objects::ObjectId;

use crate::clip::Clip;
use crate::geom::{Matrix, Point, Rect};
use crate::path::Seg;

/// Active color space family in the graphics state (`CS` / `cs` or implicit via
/// `G` / `g` / `RG` / `rg` / `K` / `k`).
#[derive(Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ActiveColorSpace {
    /// `/DeviceGray` (initial default, ISO 32000-2 §8.4.1 Table 51).
    #[default]
    DeviceGray,
    /// `/DeviceRGB`.
    DeviceRgb,
    /// `/DeviceCMYK`.
    DeviceCmyk,
    /// `/Pattern` (uncolored or colored tiling/shading patterns).
    Pattern,
    /// A resource-named color space in `/Resources/ColorSpace` (decoded name
    /// bytes).
    Named(Arc<[u8]>),
}

impl ActiveColorSpace {
    /// Parses a color space operand name (handling inline-style `/G`, `/RGB`,
    /// `/CMYK` only if explicitly spelled `/DeviceGray`, `/DeviceRGB`,
    /// `/DeviceCMYK`, or `/Pattern`, matching `CS`/`cs` in ISO 32000-2 §8.6.8).
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"DeviceGray" => Self::DeviceGray,
            b"DeviceRGB" => Self::DeviceRgb,
            b"DeviceCMYK" => Self::DeviceCmyk,
            b"Pattern" => Self::Pattern,
            other => Self::Named(Arc::from(other)),
        }
    }

    /// Resource name bytes when [`ActiveColorSpace::Named`].
    #[must_use]
    pub fn resource_name(&self) -> Option<&[u8]> {
        match self {
            Self::Named(name) => Some(name),
            _ => None,
        }
    }
}

impl fmt::Debug for ActiveColorSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceGray => f.write_str("DeviceGray"),
            Self::DeviceRgb => f.write_str("DeviceRgb"),
            Self::DeviceCmyk => f.write_str("DeviceCmyk"),
            Self::Pattern => f.write_str("Pattern"),
            Self::Named(name) => f
                .debug_struct("Named")
                .field("name_len", &name.len())
                .finish(),
        }
    }
}

/// Active color component values, stored inline for up to 4 components (`Gray`,
/// `RGB`, `CMYK`, `Lab`, `CalRGB`, `Separation`, `Indexed`) and `Arc`-shared
/// when `DeviceN` has more than 4 colorants.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorValues {
    inline: [f32; 4],
    len: u8,
    extra: Option<Arc<[f32]>>,
}

impl Default for ColorValues {
    /// Initial color `0.0` (black in `/DeviceGray`, ISO 32000-2 §8.4.1).
    fn default() -> Self {
        Self::from_slice(&[0.0])
    }
}

impl ColorValues {
    /// Constructs color component values from `values`.
    #[must_use]
    pub fn from_slice(values: &[f32]) -> Self {
        if values.len() <= 4 {
            let mut inline = [0.0; 4];
            inline[..values.len()].copy_from_slice(values);
            Self {
                inline,
                len: values.len() as u8,
                extra: None,
            }
        } else {
            Self {
                inline: [0.0; 4],
                len: 0,
                extra: Some(Arc::from(values)),
            }
        }
    }

    /// Component values in order.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        match &self.extra {
            Some(extra) => extra,
            None => &self.inline[..usize::from(self.len)],
        }
    }

    fn initial_for(space: &ActiveColorSpace) -> Self {
        match space {
            ActiveColorSpace::DeviceGray => Self::from_slice(&[0.0]),
            ActiveColorSpace::DeviceRgb => Self::from_slice(&[0.0, 0.0, 0.0]),
            ActiveColorSpace::DeviceCmyk => Self::from_slice(&[0.0, 0.0, 0.0, 1.0]),
            ActiveColorSpace::Pattern | ActiveColorSpace::Named(_) => Self::from_slice(&[]),
        }
    }
}

/// Stroke dash pattern (`d` or `/ExtGState` `/D`, ISO 32000-2 §8.4.3.6).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DashPattern {
    /// Alternating dash and gap lengths in user units (empty for a solid line).
    pub array: Arc<[f32]>,
    /// Distance into the dash pattern at which the stroke starts.
    pub phase: f32,
}

impl DashPattern {
    /// Returns `true` when `array` is empty (a solid line).
    #[must_use]
    pub fn is_solid(&self) -> bool {
        self.array.is_empty()
    }
}

/// Colorimetric rendering intent (`ri` or `/ExtGState` `/RI`, ISO 32000-2
/// §8.6.5.8).
#[derive(Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum RenderingIntent {
    /// `/AbsoluteColorimetric`.
    AbsoluteColorimetric,
    /// `/RelativeColorimetric` (initial default, ISO 32000-2 §8.4.1).
    #[default]
    RelativeColorimetric,
    /// `/Saturation`.
    Saturation,
    /// `/Perceptual`.
    Perceptual,
    /// Non-standard rendering intent name.
    Other(Arc<[u8]>),
}

impl RenderingIntent {
    /// Parses a rendering intent from its decoded PDF name bytes.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"AbsoluteColorimetric" => Self::AbsoluteColorimetric,
            b"RelativeColorimetric" => Self::RelativeColorimetric,
            b"Saturation" => Self::Saturation,
            b"Perceptual" => Self::Perceptual,
            other => Self::Other(Arc::from(other)),
        }
    }
}

impl fmt::Debug for RenderingIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AbsoluteColorimetric => f.write_str("AbsoluteColorimetric"),
            Self::RelativeColorimetric => f.write_str("RelativeColorimetric"),
            Self::Saturation => f.write_str("Saturation"),
            Self::Perceptual => f.write_str("Perceptual"),
            Self::Other(name) => f
                .debug_struct("Other")
                .field("name_len", &name.len())
                .finish(),
        }
    }
}

/// Transparency blend mode in the graphics state (`/ExtGState` `/BM`,
/// ISO 32000-2 §11.3.5).
#[derive(Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BlendState {
    /// One of ISO 32000-2's sixteen standard blend modes (`Normal` by default).
    Mode(Blend),
    /// Legacy `/Compatible` (equivalent to `Normal` in transparency).
    Compatible,
    /// Unrecognized blend mode name.
    Other(Arc<[u8]>),
}

impl Default for BlendState {
    fn default() -> Self {
        Self::Mode(Blend::Normal)
    }
}

impl BlendState {
    /// Parses a single `/BM` name.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        let mode = match name {
            b"Normal" => Blend::Normal,
            b"Multiply" => Blend::Multiply,
            b"Screen" => Blend::Screen,
            b"Overlay" => Blend::Overlay,
            b"Darken" => Blend::Darken,
            b"Lighten" => Blend::Lighten,
            b"ColorDodge" => Blend::ColorDodge,
            b"ColorBurn" => Blend::ColorBurn,
            b"HardLight" => Blend::HardLight,
            b"SoftLight" => Blend::SoftLight,
            b"Difference" => Blend::Difference,
            b"Exclusion" => Blend::Exclusion,
            b"Hue" => Blend::Hue,
            b"Saturation" => Blend::Saturation,
            b"Color" => Blend::Color,
            b"Luminosity" => Blend::Luminosity,
            b"Compatible" => return Self::Compatible,
            other => return Self::Other(Arc::from(other)),
        };
        Self::Mode(mode)
    }

    /// Returns `true` if the effective blend mode is `Normal` or `Compatible`.
    #[must_use]
    pub const fn is_normal(&self) -> bool {
        matches!(self, Self::Mode(Blend::Normal) | Self::Compatible)
    }
}

impl fmt::Debug for BlendState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mode(b) => f.debug_tuple("Mode").field(b).finish(),
            Self::Compatible => f.write_str("Compatible"),
            Self::Other(name) => f
                .debug_struct("Other")
                .field("name_len", &name.len())
                .finish(),
        }
    }
}

/// Subtype of a soft-mask dictionary (`/SMask` `/S`, ISO 32000-2 §11.6.5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SoftMaskKind {
    /// `/Alpha`: group alpha derives mask values.
    Alpha,
    /// `/Luminosity`: group luminosity derives mask values.
    Luminosity,
}

/// Active soft mask in the graphics state (`/ExtGState` `/SMask`, ISO 32000-2
/// §11.6.5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum SoftMask {
    /// No soft mask (`/None`).
    #[default]
    None,
    /// A soft-mask dictionary referencing transparency group XObject `group`
    /// (`/G`).
    Mask {
        /// Mask derivation subtype (`/Alpha` or `/Luminosity`), if valid.
        kind: Option<SoftMaskKind>,
        /// Indirect object ID of the transparency group XObject (`/G`), if a
        /// stream reference.
        group: Option<ObjectId>,
    },
}

/// Complete graphics and text state at an [`Event`](crate::Event), saved by `q`
/// and restored by `Q`.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct State {
    /// Current transformation matrix from user space onto PageSpace (`cm` and
    /// enclosing Form XObject `/Matrix` entries).
    pub ctm: Matrix,
    /// Line width in user units (`w`, `/LW`; default `1.0`).
    pub line_width: f32,
    /// Line cap style (`J`, `/LC`; default [`LineCap::Butt`]).
    pub line_cap: LineCap,
    /// Line join style (`j`, `/LJ`; default [`LineJoin::Miter`]).
    pub line_join: LineJoin,
    /// Miter limit (`M`, `/ML`; default `10.0`).
    pub miter_limit: f32,
    /// Dash pattern (`d`, `/D`; default solid).
    pub dash: DashPattern,
    /// Rendering intent (`ri`, `/RI`; default
    /// [`RenderingIntent::RelativeColorimetric`]).
    pub rendering_intent: RenderingIntent,
    /// Flatness tolerance (`i`, `/FL`; default `0.0`).
    pub flatness: f32,
    /// Active stroking color space (`CS`, `G`, `RG`, `K`).
    pub stroke_color_space: ActiveColorSpace,
    /// Active nonstroking (fill) color space (`cs`, `g`, `rg`, `k`).
    pub fill_color_space: ActiveColorSpace,
    /// Active stroking color components (`SC`, `SCN`, `G`, `RG`, `K`).
    pub stroke_color: ColorValues,
    /// Active nonstroking (fill) color components (`sc`, `scn`, `g`, `rg`, `k`).
    pub fill_color: ColorValues,
    /// Active stroking pattern resource name set by `SCN`, if any.
    pub stroke_pattern: Option<Arc<[u8]>>,
    /// Active nonstroking (fill) pattern resource name set by `scn`, if any.
    pub fill_pattern: Option<Arc<[u8]>>,
    /// Stroking constant alpha (`/ExtGState` `/CA`; default `1.0`).
    pub stroke_alpha: f32,
    /// Nonstroking (fill) constant alpha (`/ExtGState` `/ca`; default `1.0`).
    pub fill_alpha: f32,
    /// Alpha-is-shape flag (`/ExtGState` `/AIS`; default `false`).
    pub alpha_is_shape: bool,
    /// Text knockout flag (`/ExtGState` `/TK`; default `true`).
    pub text_knockout: bool,
    /// Transparency blend mode (`/ExtGState` `/BM`; default `Normal`).
    pub blend_mode: BlendState,
    /// Active soft mask (`/ExtGState` `/SMask`; default [`SoftMask::None`]).
    pub soft_mask: SoftMask,
    /// Stroking overprint flag (`/ExtGState` `/OP`; default `false`).
    pub stroke_overprint: bool,
    /// Nonstroking (fill) overprint flag (`/ExtGState` `/op`, falling back to
    /// `/OP` when `/op` is absent from that `ExtGState`; default `false`).
    pub fill_overprint: bool,
    /// Overprint mode (`/ExtGState` `/OPM`; default `0`).
    pub overprint_mode: i32,
    /// Active clipping path and text clips.
    pub clip: Clip,
    /// Active text state and line/pen position.
    pub text: TextState,
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("ctm", &self.ctm)
            .field("line_width", &self.line_width)
            .field("line_cap", &self.line_cap)
            .field("line_join", &self.line_join)
            .field("miter_limit", &self.miter_limit)
            .field("dash", &self.dash)
            .field("rendering_intent", &self.rendering_intent)
            .field("flatness", &self.flatness)
            .field("stroke_color_space", &self.stroke_color_space)
            .field("fill_color_space", &self.fill_color_space)
            .field("stroke_color", &self.stroke_color)
            .field("fill_color", &self.fill_color)
            .field(
                "stroke_pattern_len",
                &self.stroke_pattern.as_ref().map(|n| n.len()),
            )
            .field(
                "fill_pattern_len",
                &self.fill_pattern.as_ref().map(|n| n.len()),
            )
            .field("stroke_alpha", &self.stroke_alpha)
            .field("fill_alpha", &self.fill_alpha)
            .field("alpha_is_shape", &self.alpha_is_shape)
            .field("text_knockout", &self.text_knockout)
            .field("blend_mode", &self.blend_mode)
            .field("soft_mask", &self.soft_mask)
            .field("stroke_overprint", &self.stroke_overprint)
            .field("fill_overprint", &self.fill_overprint)
            .field("overprint_mode", &self.overprint_mode)
            .field("clip", &self.clip)
            .field("text", &self.text)
            .finish()
    }
}

/// Active text state and positioning parameters (`CPDF_TextState` plus text
/// matrix/line/pen on `CPDF_AllStates`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TextState {
    /// Active [`FontId`] selected by `Tf` or `/ExtGState` `/Font` (`None`
    /// before any font has been selected).
    pub font: Option<FontId>,
    /// `true` when the active font fell back to PDFium's stock Helvetica
    /// because `/Resources/Font` had no font under the requested name.
    pub stock: bool,
    /// Font size (`Tf` or `/ExtGState` `/Font`, default `1.0` in PDFium before
    /// `Tf`).
    pub size: f32,
    /// Character spacing `Tc` in unscaled text space units (`0.0` by default).
    pub char_space: f32,
    /// Word spacing `Tw` in unscaled text space units (`0.0` by default;
    /// applied to single-byte character code `32`).
    pub word_space: f32,
    /// Horizontal scaling `Tz` as a fraction (`1.0` = `100%`).
    pub scale: f32,
    /// Text leading `TL` in unscaled text space units (`0.0` by default).
    pub leading: f32,
    /// Text rise `Ts` in unscaled text space units (`0.0` by default).
    pub rise: f32,
    /// Text rendering mode `Tr` ([`TextMode::Fill`] by default).
    pub mode: TextMode,
    /// Text matrix set by `Tm` or reset to identity by `BT`.
    pub matrix: Matrix,
    /// Start of the current line in `matrix`'s space (`Td`, `TD`, `T*` offset
    /// this; `Tm` and `BT` reset it to `(0, 0)`).
    pub line: Point,
    /// Current pen position in `matrix`'s space where the next text-showing
    /// operation begins.
    pub pen: Point,
    /// `true` if any glyph shown since `line` was last set used approximated
    /// character codes or widths, making subsequent glyph positions on this
    /// line approximate.
    pub guessed: bool,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            font: None,
            stock: false,
            size: 1.0,
            char_space: 0.0,
            word_space: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            mode: TextMode::Fill,
            matrix: Matrix::IDENTITY,
            line: Point::ZERO,
            pen: Point::ZERO,
            guessed: false,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            ctm: Matrix::IDENTITY,
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: DashPattern::default(),
            rendering_intent: RenderingIntent::RelativeColorimetric,
            flatness: 0.0,
            stroke_color_space: ActiveColorSpace::DeviceGray,
            fill_color_space: ActiveColorSpace::DeviceGray,
            stroke_color: ColorValues::default(),
            fill_color: ColorValues::default(),
            stroke_pattern: None,
            fill_pattern: None,
            stroke_alpha: 1.0,
            fill_alpha: 1.0,
            alpha_is_shape: false,
            text_knockout: true,
            blend_mode: BlendState::default(),
            soft_mask: SoftMask::None,
            stroke_overprint: false,
            fill_overprint: false,
            overprint_mode: 0,
            clip: Clip::default(),
            text: TextState::default(),
        }
    }
}

impl State {
    /// Conservative axis-aligned bounding box of the active clip in PageSpace,
    /// or `None` when unclipped.
    #[must_use]
    pub fn clip_bounds(&self) -> Option<Rect> {
        self.clip.bounds()
    }

    pub(crate) fn is_finite(&self) -> bool {
        self.ctm.is_finite() && self.text.line.is_finite() && self.text.pen.is_finite()
    }

    /// Initial state inside a Form XObject (`CPDF_StreamContentParser::AddForm`),
    /// carrying general, graph, color, and `CPDF_TextState` fields from `self`
    /// while resetting `Tz`, `TL`, `Ts`, `matrix`, `line`, and `pen`, setting
    /// `ctm` to `matrix`, and intersecting `clip` with `bbox` (if any).
    pub(crate) fn form(&self, matrix: Matrix, bbox: Option<Rect>) -> State {
        let clip = match bbox {
            // Order of corners matches PDFium's `CFX_Path::AppendRect`.
            Some(r) => {
                let corners = [
                    Point::new(r.x0, r.y0),
                    Point::new(r.x0, r.y1),
                    Point::new(r.x1, r.y1),
                    Point::new(r.x1, r.y0),
                ]
                .map(|p| matrix.apply(p));
                let mut segs = Vec::with_capacity(6);
                segs.push(Seg::Move(corners[0]));
                segs.extend(
                    corners[1..]
                        .iter()
                        .chain(&corners[..1])
                        .map(|&p| Seg::Line(p)),
                );
                segs.push(Seg::Close);
                let bounds = crate::path::bounds(&segs);
                self.clip.with_path(&segs, FillRule::NonZero, bounds)
            }
            None => self.clip.clone(),
        };
        let mut out = self.clone();
        out.ctm = matrix;
        out.clip = clip;
        out.text = TextState {
            font: self.text.font,
            stock: self.text.stock,
            size: self.text.size,
            char_space: self.text.char_space,
            word_space: self.text.word_space,
            mode: self.text.mode,
            ..TextState::default()
        };
        out
    }

    /// Applies an operator that updates non-resource graphics/color/text state.
    pub(crate) fn set(&mut self, op: Operator, step: &Operation<'_>, content_lim: &ContentLimits) {
        match op {
            Operator::Transform => {
                if let Some(m) = matrix(step) {
                    self.ctm = m.then(self.ctm);
                }
            }
            Operator::LineWidth => set_num(&mut self.line_width, step),
            Operator::LineCap => self.line_cap = cap(integer(step)),
            Operator::LineJoin => self.line_join = join(integer(step)),
            Operator::MiterLimit => set_num(&mut self.miter_limit, step),
            Operator::Dash => {
                if let Some(array) = step.arg(1).filter(Operand::is_array)
                    && let Some([phase]) = nums(step)
                {
                    let dashes: Vec<f32> = array
                        .elements()
                        .filter(|e| e.object(content_lim).is_some())
                        .map(|e| e.number())
                        .filter(|v| v.is_finite())
                        .collect();
                    self.dash = DashPattern {
                        array: dashes.into(),
                        phase,
                    };
                }
            }
            Operator::RenderingIntent => {
                if let Some(name) = step.arg(0).and_then(|o| o.name()) {
                    self.rendering_intent = RenderingIntent::from_name(&name);
                }
            }
            Operator::Flatness => set_num(&mut self.flatness, step),
            Operator::StrokeColorSpace | Operator::FillColorSpace => {
                let stroke = op == Operator::StrokeColorSpace;
                if let Some(name) = step.arg(0).and_then(|o| o.name()) {
                    let space = ActiveColorSpace::from_name(&name);
                    let init = ColorValues::initial_for(&space);
                    if stroke {
                        self.stroke_color_space = space;
                        self.stroke_color = init;
                        self.stroke_pattern = None;
                    } else {
                        self.fill_color_space = space;
                        self.fill_color = init;
                        self.fill_pattern = None;
                    }
                }
            }
            Operator::StrokeGray | Operator::FillGray => {
                if let Some([g]) = nums(step) {
                    let vals = ColorValues::from_slice(&[g]);
                    if op == Operator::StrokeGray {
                        self.stroke_color_space = ActiveColorSpace::DeviceGray;
                        self.stroke_color = vals;
                        self.stroke_pattern = None;
                    } else {
                        self.fill_color_space = ActiveColorSpace::DeviceGray;
                        self.fill_color = vals;
                        self.fill_pattern = None;
                    }
                }
            }
            Operator::StrokeRgb | Operator::FillRgb => {
                if let Some(rgb) = nums::<3>(step) {
                    let vals = ColorValues::from_slice(&rgb);
                    if op == Operator::StrokeRgb {
                        self.stroke_color_space = ActiveColorSpace::DeviceRgb;
                        self.stroke_color = vals;
                        self.stroke_pattern = None;
                    } else {
                        self.fill_color_space = ActiveColorSpace::DeviceRgb;
                        self.fill_color = vals;
                        self.fill_pattern = None;
                    }
                }
            }
            Operator::StrokeCmyk | Operator::FillCmyk => {
                if let Some(cmyk) = nums::<4>(step) {
                    let vals = ColorValues::from_slice(&cmyk);
                    if op == Operator::StrokeCmyk {
                        self.stroke_color_space = ActiveColorSpace::DeviceCmyk;
                        self.stroke_color = vals;
                        self.stroke_pattern = None;
                    } else {
                        self.fill_color_space = ActiveColorSpace::DeviceCmyk;
                        self.fill_color = vals;
                        self.fill_pattern = None;
                    }
                }
            }
            Operator::StrokeColor
            | Operator::FillColor
            | Operator::StrokeColorN
            | Operator::FillColorN => {
                let stroke = matches!(op, Operator::StrokeColor | Operator::StrokeColorN);
                let allows_pattern = matches!(op, Operator::StrokeColorN | Operator::FillColorN);
                let pat = if allows_pattern {
                    step.arg(0)
                        .and_then(|o| o.name())
                        .map(|n| Arc::<[u8]>::from(&*n))
                } else {
                    None
                };
                let count = step
                    .operands()
                    .len()
                    .saturating_sub(usize::from(pat.is_some()));
                let mut components = Vec::with_capacity(count.min(16));
                let mut all_finite = true;
                for operand in step.operands().take(count) {
                    let v = operand.number();
                    if !v.is_finite() {
                        all_finite = false;
                        break;
                    }
                    components.push(v);
                }
                if all_finite {
                    let vals = ColorValues::from_slice(&components);
                    if stroke {
                        self.stroke_color = vals;
                        if allows_pattern {
                            self.stroke_pattern = pat;
                        }
                    } else {
                        self.fill_color = vals;
                        if allows_pattern {
                            self.fill_pattern = pat;
                        }
                    }
                }
            }
            Operator::BeginText => {
                self.text.matrix = Matrix::IDENTITY;
                self.text.line = Point::ZERO;
                self.text.pen = Point::ZERO;
                self.text.guessed = false;
            }
            Operator::TextMatrix => {
                if let Some(m) = matrix(step) {
                    self.text.matrix = m;
                    self.text.line = Point::ZERO;
                    self.text.pen = Point::ZERO;
                    self.text.guessed = false;
                }
            }
            Operator::MoveLine | Operator::MoveLineLeading => {
                if let Some([x, y]) = nums(step) {
                    self.text.line.x += x;
                    self.text.line.y += y;
                    self.text.pen = self.text.line;
                    self.text.guessed = false;
                    if op == Operator::MoveLineLeading {
                        self.text.leading = -y;
                    }
                }
            }
            Operator::NextLine => self.text.next_line(),
            Operator::CharSpace => set_num(&mut self.text.char_space, step),
            Operator::WordSpace => set_num(&mut self.text.word_space, step),
            // PDFium's `Handle_SetHorzScale` accepts `Tz` with exactly 1 operand.
            Operator::Scale if step.operands().len() == 1 => {
                if let Some([percent]) = nums(step) {
                    self.text.scale = percent / 100.0;
                }
            }
            Operator::Leading => set_num(&mut self.text.leading, step),
            Operator::Rise => set_num(&mut self.text.rise, step),
            Operator::RenderMode => {
                if let Some(mode) = nums(step).and_then(|[n]| TextMode::of(int(n))) {
                    self.text.mode = mode;
                }
            }
            _ => {}
        }
    }

    /// Applies the line-moving prefix of `'` or `"` before its string is shown.
    pub(crate) fn next_line_of(&mut self, op: Operator, step: &Operation<'_>) {
        if op == Operator::NextLineShowSpaced
            && let Some([tw, tc, _]) = nums(step)
        {
            self.text.word_space = tw;
            self.text.char_space = tc;
        }
        self.text.next_line();
    }

    /// Computes the state after a line-moving text operator (`Td`, `TD`, `T*`,
    /// or the line-advance portion of `'` / `"`).
    pub(crate) fn moved(&self, op: Operator, step: &Operation<'_>, lim: &ContentLimits) -> State {
        let mut moved = self.clone();
        match op {
            Operator::NextLineShow | Operator::NextLineShowSpaced => moved.next_line_of(op, step),
            _ => moved.set(op, step, lim),
        }
        moved
    }
}

impl TextState {
    pub(crate) fn next_line(&mut self) {
        self.line.y -= self.leading;
        self.pen = self.line;
        self.guessed = false;
    }
}

/// Casts `n` to `i32` the way PDFium casts floats (`0` when out of `i32` range).
pub(crate) fn int(n: f32) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&n) {
        n as i32
    } else {
        0
    }
}

fn integer(step: &Operation<'_>) -> i32 {
    nums(step).map_or(0, |[n]| int(n))
}

pub(crate) fn cap(n: i32) -> LineCap {
    match n {
        1 => LineCap::Round,
        2 => LineCap::Square,
        _ => LineCap::Butt,
    }
}

pub(crate) fn join(n: i32) -> LineJoin {
    match n {
        1 => LineJoin::Round,
        2 => LineJoin::Bevel,
        _ => LineJoin::Miter,
    }
}

fn set_num(slot: &mut f32, step: &Operation<'_>) {
    if let Some([n]) = nums(step) {
        *slot = n;
    }
}

fn matrix(step: &Operation<'_>) -> Option<Matrix> {
    let [a, b, c, d, e, f] = nums(step)?;
    Some(Matrix { a, b, c, d, e, f })
}

/// Reads the last `N` operands in forward order using PDFium's `GetNumber`
/// rules (`0.0` when absent or non-numeric), returning `None` if any is
/// non-finite.
pub(crate) fn nums<const N: usize>(step: &Operation<'_>) -> Option<[f32; N]> {
    let n: [f32; N] = std::array::from_fn(|k| step.arg(N - 1 - k).map_or(0.0, |o| o.number()));
    n.iter().all(|v| v.is_finite()).then_some(n)
}
