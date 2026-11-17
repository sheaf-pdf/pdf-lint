//! Annotation colors, borders, appearance characteristics (`/MK`), line
//! endings, quadding, and default appearance (`/DA`) strings.

use std::fmt;

use pdf_content_streams::Color;
use pdf_objects::fault::Sink;
use pdf_objects::{Dict, Object, ObjectId, Resolve};

use crate::error::Error;
use crate::fault::Fault;
use crate::resolve::Reader;

/// Parsed `/DA` (Default Appearance) string used by `/FreeText`, `/Redact`, and
/// variable-text `/Widget` annotations (ISO 32000-2 §12.7.4.3): `pdf-gfx`'s
/// type, re-exported so annotations and other variable text share one parser
/// and serializer. Its color is a `pdf-content-streams` [`Color`]
/// ([`AnnotColor::to_content_color`] converts one).
pub use pdf_gfx::DefaultAppearance;

/// Device color array used in `/C`, `/IC`, `/MK/BC`, and `/MK/BG`
/// (ISO 32000-2 §12.5.2 Table 166).
///
/// Exhaustive on purpose: PDF annotation color arrays have `0` (transparent),
/// `1` (`DeviceGray`), `3` (`DeviceRGB`), or `4` (`DeviceCMYK`) numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnnotColor {
    /// Empty array `[]`: transparent (no fill or stroke).
    Transparent,
    /// 1-element array `[g]` in `DeviceGray` (`0.0..=1.0`).
    Gray(f32),
    /// 3-element array `[r, g, b]` in `DeviceRGB` (`0.0..=1.0`).
    Rgb([f32; 3]),
    /// 4-element array `[c, m, y, k]` in `DeviceCMYK` (`0.0..=1.0`).
    Cmyk([f32; 4]),
}

impl AnnotColor {
    /// Constructs an sRGB/DeviceRGB color from 8-bit `[r, g, b]` channels.
    #[must_use]
    pub fn rgb8([r, g, b]: [u8; 3]) -> Self {
        Self::Rgb([
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
        ])
    }

    /// Converts this color to a `pdf-content-streams` [`Color`], returning
    /// `None` for [`Self::Transparent`].
    #[must_use]
    pub const fn to_content_color(self) -> Option<Color> {
        match self {
            Self::Transparent => None,
            Self::Gray(g) => Some(Color::Gray(g)),
            Self::Rgb(rgb) => Some(Color::Rgb(rgb)),
            Self::Cmyk(cmyk) => Some(Color::Cmyk(cmyk)),
        }
    }

    /// Converts this color to approximate 8-bit `[R, G, B]` using PDFium's
    /// `FXSYS_roundf` / truncation conversion rules (`None` for
    /// [`Self::Transparent`]).
    #[must_use]
    pub fn to_rgb8(self) -> Option<[u8; 3]> {
        let clamp = |x: f32| (x.clamp(0.0, 1.0) * 255.0) as u8;
        match self {
            Self::Transparent => None,
            Self::Gray(g) => {
                let v = clamp(g);
                Some([v, v, v])
            }
            Self::Rgb([r, g, b]) => Some([clamp(r), clamp(g), clamp(b)]),
            Self::Cmyk([c, m, y, k]) => {
                let c = c.clamp(0.0, 1.0);
                let m = m.clamp(0.0, 1.0);
                let y = y.clamp(0.0, 1.0);
                let k = k.clamp(0.0, 1.0);
                Some([
                    clamp((1.0 - c) * (1.0 - k)),
                    clamp((1.0 - m) * (1.0 - k)),
                    clamp((1.0 - y) * (1.0 - k)),
                ])
            }
        }
    }

    /// Serializes this color to a PDF [`Object`] array.
    #[must_use]
    pub fn to_object(self) -> Object {
        match self {
            Self::Transparent => Object::from(Vec::<Object>::new()),
            Self::Gray(g) => Object::from(vec![Object::from(f64::from(g))]),
            Self::Rgb([r, g, b]) => Object::from(vec![
                Object::from(f64::from(r)),
                Object::from(f64::from(g)),
                Object::from(f64::from(b)),
            ]),
            Self::Cmyk([c, m, y, k]) => Object::from(vec![
                Object::from(f64::from(c)),
                Object::from(f64::from(m)),
                Object::from(f64::from(y)),
                Object::from(f64::from(k)),
            ]),
        }
    }
}

impl From<Color> for AnnotColor {
    fn from(color: Color) -> Self {
        match color {
            Color::Gray(g) => Self::Gray(g),
            Color::Rgb(rgb) => Self::Rgb(rgb),
            Color::Cmyk(cmyk) => Self::Cmyk(cmyk),
        }
    }
}

pub(crate) fn read_color<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
    key: &'static str,
) -> Result<Option<AnnotColor>, Error> {
    let Some(raw) = dict.get(key.as_bytes()) else {
        return Ok(None);
    };
    let Some((obj, id)) = reader.reached(raw, within)? else {
        return Ok(None);
    };
    let Some(arr) = obj.as_array() else {
        reader.say(id, obj.at(), Fault::InvalidColor { key });
        return Ok(None);
    };
    let mut vals = Vec::with_capacity(arr.len().min(4));
    for item in arr {
        let Some(v) = reader.float_value(item, id, None)? else {
            reader.say(id, item.at(), Fault::InvalidColor { key });
            return Ok(None);
        };
        if !(0.0..=1.0).contains(&v) {
            reader.say(id, item.at(), Fault::InvalidColor { key });
        }
        vals.push(v.clamp(0.0, 1.0));
    }
    match vals.as_slice() {
        [] => Ok(Some(AnnotColor::Transparent)),
        &[g] => Ok(Some(AnnotColor::Gray(g))),
        &[r, g, b] => Ok(Some(AnnotColor::Rgb([r, g, b]))),
        &[c, m, y, k] => Ok(Some(AnnotColor::Cmyk([c, m, y, k]))),
        _ => {
            reader.say(id, obj.at(), Fault::InvalidColor { key });
            Ok(None)
        }
    }
}

/// Border line style (`/BS /S`, ISO 32000-2 §12.5.4 Table 168).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BorderStyle {
    /// `/S`: solid rectangle surrounding the annotation.
    #[default]
    Solid,
    /// `/D`: dashed rectangle using `Border::dash`.
    Dashed,
    /// `/B`: simulated embossed (beveled) 3D rectangle.
    Beveled,
    /// `/I`: simulated engraved (inset) 3D rectangle.
    Inset,
    /// `/U`: single line along the bottom of the annotation rectangle.
    Underline,
}

impl BorderStyle {
    #[must_use]
    pub(crate) fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"S" => Self::Solid,
            b"D" => Self::Dashed,
            b"B" => Self::Beveled,
            b"I" => Self::Inset,
            b"U" => Self::Underline,
            _ => return None,
        })
    }

    /// PDF `/S` name byte slice (`"S"`, `"D"`, `"B"`, `"I"`, or `"U"`).
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::Solid => "S",
            Self::Dashed => "D",
            Self::Beveled => "B",
            Self::Inset => "I",
            Self::Underline => "U",
        }
    }
}

/// Effective border specification resolved from `/BS` or legacy `/Border`
/// (ISO 32000-2 §12.5.4).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Border {
    /// Stroke width in points (`1.0` by default).
    pub width: f32,
    /// Border style (`Solid` by default, or `Dashed` when a 4-element `/Border`
    /// supplies a dash array).
    pub style: BorderStyle,
    /// Dash array when `style == BorderStyle::Dashed` (`[3.0]` by default).
    pub dash: Vec<f32>,
    /// Horizontal corner radius from legacy `/Border` (`0.0` when `/BS` is
    /// used).
    pub horizontal_radius: f32,
    /// Vertical corner radius from legacy `/Border` (`0.0` when `/BS` is used).
    pub vertical_radius: f32,
    /// Cloudy border effect intensity from `/BE` (`/S /C`, `/I` typically
    /// `0.0..=2.0`), if present.
    pub cloudy_intensity: Option<f32>,
}

impl Default for Border {
    fn default() -> Self {
        Self {
            width: 1.0,
            style: BorderStyle::Solid,
            dash: Vec::new(),
            horizontal_radius: 0.0,
            vertical_radius: 0.0,
            cloudy_intensity: None,
        }
    }
}

/// Bundled stroke, interior fill, border, and opacity parameters for geometric
/// and FreeText appearance synthesis and builders.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PaintStyle {
    /// Stroking / border color (`/C`).
    pub stroke: Option<AnnotColor>,
    /// Interior / background fill color (`/IC` on geometric annotations, `/C`
    /// background on `FreeText`).
    pub fill: Option<AnnotColor>,
    /// Border stroke width and dash specification (`/BS`).
    pub border: Border,
    /// Overall/stroking opacity (`/CA`, `1.0` by default).
    pub opacity: f32,
    /// Optional nonstroking fill opacity (`/ca`, falls back to `opacity` when
    /// `None`).
    pub fill_opacity: Option<f32>,
}

impl Default for PaintStyle {
    fn default() -> Self {
        Self {
            stroke: Some(AnnotColor::Gray(0.0)),
            fill: None,
            border: Border::default(),
            opacity: 1.0,
            fill_opacity: None,
        }
    }
}

impl PaintStyle {
    /// Creates a [`PaintStyle`] with `stroke` and `fill` colors, default
    /// `1.0 pt` solid border, and `1.0` opacity.
    #[must_use]
    pub fn new(stroke: Option<AnnotColor>, fill: Option<AnnotColor>) -> Self {
        Self {
            stroke,
            fill,
            ..Self::default()
        }
    }

    /// Sets the border specification.
    #[must_use]
    pub fn with_border(mut self, border: Border) -> Self {
        self.border = border;
        self
    }

    /// Sets overall opacity (`0.0..=1.0`).
    #[must_use]
    pub const fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }
}

pub(crate) struct LegacyBorder {
    pub(crate) hr: f32,
    pub(crate) vr: f32,
    pub(crate) bw: f32,
    pub(crate) dash: Option<Vec<f32>>,
}

pub(crate) fn read_legacy_border<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
) -> Result<Option<LegacyBorder>, Error> {
    let Some((arr, id)) = reader.array_entry(dict, within, "Border")? else {
        return Ok(None);
    };
    if arr.len() < 3 {
        reader.say(id, None, Fault::WrongType { key: "Border" });
        return Ok(None);
    }
    let hr = reader
        .float_value(&arr[0], id, Some("Border"))?
        .unwrap_or(0.0);
    let vr = reader
        .float_value(&arr[1], id, Some("Border"))?
        .unwrap_or(0.0);
    let bw = reader
        .float_value(&arr[2], id, Some("Border"))?
        .unwrap_or(0.0);
    let dash = if let Some(d_obj) = arr.get(3) {
        let Some((d_resolved, d_id)) = reader.reached(d_obj, id)? else {
            return Ok(Some(LegacyBorder {
                hr,
                vr,
                bw,
                dash: None,
            }));
        };
        if let Some(d_arr) = d_resolved.as_array() {
            let mut dashes = Vec::with_capacity(d_arr.len().min(32));
            for item in d_arr.iter().take(32) {
                if let Some(v) = reader.float_value(item, d_id, None)?
                    && v >= 0.0
                {
                    dashes.push(v);
                }
            }
            Some(dashes)
        } else {
            reader.say(d_id, d_resolved.at(), Fault::WrongType { key: "Border" });
            None
        }
    } else {
        None
    };
    Ok(Some(LegacyBorder { hr, vr, bw, dash }))
}

pub(crate) fn read_border<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
) -> Result<Border, Error> {
    let cloudy_intensity = if let Some((be, be_id)) = reader.dict_entry(dict, within, "BE")? {
        if reader.name_entry(be, be_id, "S")? == Some(b"C") {
            Some(reader.float_entry(be, be_id, "I")?.unwrap_or(0.0))
        } else {
            None
        }
    } else {
        None
    };

    if let Some((bs, bs_id)) = reader.dict_entry(dict, within, "BS")? {
        let width = reader.float_entry(bs, bs_id, "W")?.unwrap_or(1.0).max(0.0);
        let style = match reader.name_entry(bs, bs_id, "S")? {
            Some(n) => BorderStyle::from_name(n).unwrap_or_else(|| {
                reader.say(bs_id, None, Fault::WrongType { key: "S" });
                BorderStyle::Solid
            }),
            None => BorderStyle::Solid,
        };
        let mut dash = if let Some((d_arr, d_id)) = reader.array_entry(bs, bs_id, "D")? {
            let mut out = Vec::with_capacity(d_arr.len().min(32));
            for item in d_arr.iter().take(32) {
                if let Some(v) = reader.float_value(item, d_id, None)?
                    && v >= 0.0
                {
                    out.push(v);
                }
            }
            out
        } else {
            Vec::new()
        };
        if style == BorderStyle::Dashed && dash.is_empty() {
            dash.push(3.0);
        }
        return Ok(Border {
            width,
            style,
            dash,
            horizontal_radius: 0.0,
            vertical_radius: 0.0,
            cloudy_intensity,
        });
    }

    if let Some(lb) = read_legacy_border(reader, dict, within)? {
        let (style, dash) = match lb.dash {
            Some(d) => (
                BorderStyle::Dashed,
                if d.is_empty() { vec![3.0] } else { d },
            ),
            None => (BorderStyle::Solid, Vec::new()),
        };
        return Ok(Border {
            width: lb.bw.max(0.0),
            style,
            dash,
            horizontal_radius: lb.hr.max(0.0),
            vertical_radius: lb.vr.max(0.0),
            cloudy_intensity,
        });
    }

    Ok(Border {
        cloudy_intensity,
        ..Border::default()
    })
}

/// Line-ending style for `/Line`, `/PolyLine`, and `/FreeText` callouts
/// (ISO 32000-2 §12.5.6.7 Table 179).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LineEnding {
    /// `/None`: plain line end.
    #[default]
    None,
    /// `/Square`: filled/stroked square centered at endpoint.
    Square,
    /// `/Circle`: filled/stroked circle centered at endpoint.
    Circle,
    /// `/Diamond`: diamond centered at endpoint.
    Diamond,
    /// `/OpenArrow`: two angled strokes forming an open V-arrow.
    OpenArrow,
    /// `/ClosedArrow`: closed triangular arrowhead.
    ClosedArrow,
    /// `/Butt`: perpendicular tick mark at endpoint.
    Butt,
    /// `/ROpenArrow`: reversed open arrow pointing away from endpoint.
    ROpenArrow,
    /// `/RClosedArrow`: reversed closed triangular arrow.
    RClosedArrow,
    /// `/Slash`: diagonal slash across endpoint.
    Slash,
}

impl LineEnding {
    /// Parses a PDF line-ending name (`"None"`, `"OpenArrow"`, etc.).
    #[must_use]
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"None" => Self::None,
            b"Square" => Self::Square,
            b"Circle" => Self::Circle,
            b"Diamond" => Self::Diamond,
            b"OpenArrow" => Self::OpenArrow,
            b"ClosedArrow" => Self::ClosedArrow,
            b"Butt" => Self::Butt,
            b"ROpenArrow" => Self::ROpenArrow,
            b"RClosedArrow" => Self::RClosedArrow,
            b"Slash" => Self::Slash,
            _ => return None,
        })
    }

    /// PDF name string for this line-ending style.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Square => "Square",
            Self::Circle => "Circle",
            Self::Diamond => "Diamond",
            Self::OpenArrow => "OpenArrow",
            Self::ClosedArrow => "ClosedArrow",
            Self::Butt => "Butt",
            Self::ROpenArrow => "ROpenArrow",
            Self::RClosedArrow => "RClosedArrow",
            Self::Slash => "Slash",
        }
    }
}

/// Text justification (`/Q` quadding, ISO 32000-2 §12.5.6.6 / §12.7.4.3).
///
/// Exhaustive on purpose: PDF `/Q` specifies `0` (left), `1` (centered), or `2`
/// (right).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Quadding {
    /// `0`: left-justified.
    #[default]
    Left = 0,
    /// `1`: centered.
    Center = 1,
    /// `2`: right-justified.
    Right = 2,
}

impl Quadding {
    /// Maps an integer `/Q` value (`0`, `1`, `2`; out-of-range defaults to
    /// [`Self::Left`]).
    #[must_use]
    pub const fn from_pdf_int(n: i64) -> Self {
        match n {
            1 => Self::Center,
            2 => Self::Right,
            _ => Self::Left,
        }
    }
}

/// Widget / screen appearance characteristics dictionary (`/MK`, ISO 32000-2
/// §12.5.6.19 Table 192).
///
/// Its `Debug` implementation reports caption character lengths rather than
/// caption text.
#[derive(Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct AppearanceChars {
    /// Counterclockwise rotation in degrees (`/R`, typically `0`, `90`, `180`,
    /// or `270`).
    pub rotation: i32,
    /// Border color (`/BC`).
    pub border_color: Option<AnnotColor>,
    /// Background color (`/BG`).
    pub background_color: Option<AnnotColor>,
    /// Normal caption (`/CA`).
    pub normal_caption: Option<String>,
    /// Rollover caption (`/RC`).
    pub rollover_caption: Option<String>,
    /// Down/alternate caption (`/AC`).
    pub down_caption: Option<String>,
    /// Caption/icon layout position (`/TP`, `0..=6`).
    pub text_position: Option<i32>,
}

impl fmt::Debug for AppearanceChars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppearanceChars")
            .field("rotation", &self.rotation)
            .field("border_color", &self.border_color)
            .field("background_color", &self.background_color)
            .field(
                "normal_caption_len",
                &self.normal_caption.as_ref().map(String::len),
            )
            .field(
                "rollover_caption_len",
                &self.rollover_caption.as_ref().map(String::len),
            )
            .field(
                "down_caption_len",
                &self.down_caption.as_ref().map(String::len),
            )
            .field("text_position", &self.text_position)
            .finish()
    }
}

pub(crate) fn read_mk<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
) -> Result<Option<AppearanceChars>, Error> {
    let Some((mk, mk_id)) = reader.dict_entry(dict, within, "MK")? else {
        return Ok(None);
    };
    let rotation = reader.int_entry(mk, mk_id, "R")?.unwrap_or(0) as i32;
    let border_color = read_color(reader, mk, mk_id, "BC")?;
    let background_color = read_color(reader, mk, mk_id, "BG")?;
    let normal_caption = reader.text_entry(mk, mk_id, "CA")?;
    let rollover_caption = reader.text_entry(mk, mk_id, "RC")?;
    let down_caption = reader.text_entry(mk, mk_id, "AC")?;
    let text_position = reader.int_entry(mk, mk_id, "TP")?.map(|n| n as i32);
    Ok(Some(AppearanceChars {
        rotation,
        border_color,
        background_color,
        normal_caption,
        rollover_caption,
        down_caption,
        text_position,
    }))
}
