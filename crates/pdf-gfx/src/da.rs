//! Default appearance (`/DA`) strings: the font, size, color, and text-state
//! scalars that variable text is drawn with (ISO 32000-2 §12.7.4.3).

use std::fmt;

use pdf_content_streams::{
    Color, Limits, Operation, Operator, ResCx, TextMode, operations, write_number,
};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::serialize;

use crate::fault::Fault;

/// Parsed `/DA` (Default Appearance) string used by `/FreeText`, `/Redact`, and
/// variable-text `/Widget` annotations and by interactive form fields
/// (ISO 32000-2 §12.7.4.3).
///
/// A `/DA` is a fragment of content: a `Tf` naming a font resource and its
/// size, an optional color operator, and optional text-state operators. Each
/// field is `None` where its operator was absent; where an operator is written
/// more than once, the last one wins.
///
/// Its `Debug` implementation reports `font_name`'s byte length rather than the
/// raw resource name.
///
/// ```
/// use pdf_gfx::DefaultAppearance;
/// use pdf_gfx::pdf_content_streams::{Color, Limits};
///
/// let da = DefaultAppearance::parse(b"/Helv 0 Tf 0 0 1 rg 2 Tc", &Limits::DEFAULT).unwrap();
/// assert_eq!(da.font_name.as_deref(), Some(&b"Helv"[..]));
/// assert_eq!(da.font_size, Some(0.0));
/// assert_eq!(da.size(), None); // `0 Tf`: auto-size
/// assert_eq!(da.color, Some(Color::Rgb([0.0, 0.0, 1.0])));
/// assert_eq!(da.char_space, Some(2.0));
/// assert_eq!(da.to_bytes(), b"/Helv 0 Tf 0 0 1 rg 2 Tc");
/// ```
#[derive(Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct DefaultAppearance {
    /// Font resource name selected by `Tf` (decoded without leading `/`, e.g.
    /// `b"Helv"`), if `Tf` was present. A name can't hold a NUL byte: for
    /// one set by hand that does, [`to_bytes`](Self::to_bytes) writes
    /// `/Helv` in its place.
    pub font_name: Option<Vec<u8>>,
    /// Font size selected by `Tf` (`0.0` asks for auto-sizing, see
    /// [`auto_sized`](Self::auto_sized) and [`size`](Self::size)). `Tf`
    /// takes a name and a size together, so a size set by hand with no
    /// `font_name` isn't written by [`to_bytes`](Self::to_bytes).
    pub font_size: Option<f32>,
    /// Text color set by `g`/`G`, `rg`/`RG`, or `k`/`K` in `/DA`, its
    /// components clamped to `0.0..=1.0`.
    pub color: Option<Color>,
    /// Character spacing set by `Tc`, in unscaled text space units.
    pub char_space: Option<f32>,
    /// Word spacing set by `Tw`, in unscaled text space units.
    pub word_space: Option<f32>,
    /// Horizontal scaling set by `Tz`, as a percentage (`100.0` is normal).
    pub horizontal_scale: Option<f32>,
    /// Leading set by `TL`, in unscaled text space units.
    pub leading: Option<f32>,
    /// Text rendering mode set by `Tr` (`0..=7`).
    pub text_mode: Option<TextMode>,
    /// Text rise set by `Ts`, in unscaled text space units.
    pub rise: Option<f32>,
}

impl fmt::Debug for DefaultAppearance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultAppearance")
            .field("font_name_len", &self.font_name.as_ref().map(Vec::len))
            .field("font_size", &self.font_size)
            .field("color", &self.color)
            .field("char_space", &self.char_space)
            .field("word_space", &self.word_space)
            .field("horizontal_scale", &self.horizontal_scale)
            .field("leading", &self.leading)
            .field("text_mode", &self.text_mode)
            .field("rise", &self.rise)
            .finish()
    }
}

/// Operand `i` from the last as a number, where it's there and finite.
fn finite(step: &Operation<'_>, i: usize) -> Option<f32> {
    step.arg(i).map(|o| o.number()).filter(|v| v.is_finite())
}

/// Operand `i` from the last as a color component, clamped to `0.0..=1.0`.
fn component(step: &Operation<'_>, i: usize) -> Option<f32> {
    finite(step, i).map(|v| v.clamp(0.0, 1.0))
}

/// Sets `field` to a text-state operator's operand where it's there and
/// finite, else leaves it as it was; whether it was.
fn scalar(field: &mut Option<f32>, step: &Operation<'_>) -> bool {
    let value = finite(step, 0);
    if value.is_some() {
        *field = value;
    }
    value.is_some()
}

impl DefaultAppearance {
    /// Constructs a [`DefaultAppearance`] from font resource name, size, and
    /// optional color, with no text-state scalar set.
    #[must_use]
    pub fn new(font_name: impl Into<Vec<u8>>, font_size: f32, color: Option<Color>) -> Self {
        Self {
            font_name: Some(font_name.into()),
            font_size: Some(font_size),
            color,
            ..Self::default()
        }
    }

    /// Parses a `/DA` content fragment under `limits`.
    ///
    /// Reads `Tf`, the device color operators (`g`/`G`, `rg`/`RG`, `k`/`K`,
    /// stroking and nonstroking alike), and the text-state operators `Tc`,
    /// `Tw`, `Tz`, `TL`, `Tr`, and `Ts`; every other operator is passed over.
    /// An operator is ignored where an operand it needs is missing or not
    /// finite, and `Tr` where its operand is outside `0..=7`.
    ///
    /// Returns `None` if lexing fails or a cap of `limits` is passed. A `/DA`
    /// without `Tf` parses with `font_name` `None`, which callers may diagnose.
    /// [`parse_with`](Self::parse_with) reads the same and says what it
    /// passed over.
    #[must_use]
    pub fn parse(da: &[u8], limits: &Limits) -> Option<Self> {
        Self::parse_with(da, limits, &mut ())
    }

    /// Parses a `/DA` content fragment under `limits` as
    /// [`parse`](Self::parse) does, the same value whatever `sink` is, and
    /// tells `sink` what departs from ISO 32000-2, §12.7.4.3, each at its
    /// byte offset into `da` where it has one:
    ///
    /// - [`Fault::DefaultAppearanceUnreadable`]: `da` failed to lex or passed
    ///   a cap of `limits`, and `None` is returned;
    /// - [`Fault::DefaultAppearanceOperand`]: an operator ignored, or a `Tf`
    ///   read in part (its name or its size), for an operand it needs that's
    ///   missing or not finite, or a font that isn't a name;
    /// - [`Fault::DefaultAppearanceTextMode`]: a `Tr` ignored for a mode
    ///   outside `0..=7`;
    /// - [`Fault::DefaultAppearanceNoFont`]: no `Tf` names a font.
    ///
    /// Each is told at a [`Place`] with no object and no span: a `/DA` is a
    /// string, and where it was read from is the caller's to say. With a
    /// sink whose [`wants`](Sink::wants) is `false`, `&mut ()` among them,
    /// nothing is gathered for it.
    ///
    /// ```
    /// use pdf_gfx::pdf_content_streams::{Limits, Operator};
    /// use pdf_gfx::{DefaultAppearance, Fault};
    ///
    /// let mut faults = Vec::new();
    /// let da = DefaultAppearance::parse_with(b"Tc 9 Tr 0 g", &Limits::DEFAULT, &mut faults);
    /// assert_eq!(da, DefaultAppearance::parse(b"Tc 9 Tr 0 g", &Limits::DEFAULT));
    /// let faults: Vec<Fault> = faults.into_iter().map(|(_, fault)| fault).collect();
    /// assert_eq!(
    ///     faults,
    ///     [
    ///         Fault::DefaultAppearanceOperand { at: 0, operator: Operator::CharSpace },
    ///         Fault::DefaultAppearanceTextMode { at: 5 },
    ///         Fault::DefaultAppearanceNoFont,
    ///     ]
    /// );
    /// ```
    #[must_use]
    pub fn parse_with<S: Sink<Fault> + ?Sized>(
        da: &[u8],
        limits: &Limits,
        sink: &mut S,
    ) -> Option<Self> {
        let mut tell = |fault: Fault| {
            if sink.wants() {
                sink.fault(Place::new(None, None), fault);
            }
        };
        let mut out = Self::default();
        for step in operations(da, &ResCx::none(), limits) {
            let step = match step {
                Ok(step) => step,
                Err(error) => {
                    tell(Fault::DefaultAppearanceUnreadable(error));
                    return None;
                }
            };
            let operator = step.operator();
            // Whether every operand the operator needs was there to read.
            let read = match operator {
                Operator::Font => {
                    let name = step.arg(1).and_then(|o| o.name());
                    let size = finite(&step, 0);
                    let whole = name.is_some() && size.is_some();
                    if let Some(name) = name {
                        out.font_name = Some(name.into_owned());
                    }
                    if let Some(sz) = size {
                        out.font_size = Some(sz);
                    }
                    whole
                }
                Operator::FillGray | Operator::StrokeGray => {
                    let gray = component(&step, 0);
                    if let Some(g) = gray {
                        out.color = Some(Color::Gray(g));
                    }
                    gray.is_some()
                }
                Operator::FillRgb | Operator::StrokeRgb => {
                    if let (Some(r), Some(g), Some(b)) = (
                        component(&step, 2),
                        component(&step, 1),
                        component(&step, 0),
                    ) {
                        out.color = Some(Color::Rgb([r, g, b]));
                        true
                    } else {
                        false
                    }
                }
                Operator::FillCmyk | Operator::StrokeCmyk => {
                    if let (Some(c), Some(m), Some(y), Some(k)) = (
                        component(&step, 3),
                        component(&step, 2),
                        component(&step, 1),
                        component(&step, 0),
                    ) {
                        out.color = Some(Color::Cmyk([c, m, y, k]));
                        true
                    } else {
                        false
                    }
                }
                Operator::CharSpace => scalar(&mut out.char_space, &step),
                Operator::WordSpace => scalar(&mut out.word_space, &step),
                Operator::Scale => scalar(&mut out.horizontal_scale, &step),
                Operator::Leading => scalar(&mut out.leading, &step),
                Operator::RenderMode => match finite(&step, 0) {
                    // Cut to its whole part, as the interpreter reads `Tr`.
                    Some(n) => {
                        match TextMode::of(n as i32) {
                            Some(mode) => out.text_mode = Some(mode),
                            None => tell(Fault::DefaultAppearanceTextMode {
                                at: step.at().start,
                            }),
                        }
                        true
                    }
                    None => false,
                },
                Operator::Rise => scalar(&mut out.rise, &step),
                _ => true,
            };
            if !read {
                tell(Fault::DefaultAppearanceOperand {
                    at: step.at().start,
                    operator,
                });
            }
        }
        if out.font_name.is_none() {
            tell(Fault::DefaultAppearanceNoFont);
        }
        Some(out)
    }

    /// Serializes this default appearance into a canonical `/DA` byte string
    /// (e.g. `b"/Helv 12 Tf 0 g"`).
    ///
    /// Deterministic, in one order whatever order the operators were read in:
    /// `Tf` (where a font name is set; `/Helv` where the name holds a NUL byte
    /// and so can't be written, and size `0` where none is set), the color
    /// (`g`, `rg`, or `k`, its components clamped to `0.0..=1.0` as
    /// [`parse`](Self::parse) reads them), then `Tc`, `Tw`, `Tz`, `TL`, `Tr`,
    /// and `Ts`, each only where set, separated by single spaces. Numbers are
    /// written as content numbers, a value that isn't finite as `0`. A size
    /// with no font name is left out, as `Tf` needs both.
    ///
    /// What it writes parses back ([`parse`](Self::parse)) to an appearance
    /// that writes the same bytes again, as long as the font name's written
    /// form fits the 255 bytes a content parser keeps of one word.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        fn sep(out: &mut Vec<u8>) {
            if !out.is_empty() {
                out.push(b' ');
            }
        }
        fn scalar(out: &mut Vec<u8>, value: Option<f32>, operator: &[u8]) {
            if let Some(v) = value {
                sep(out);
                write_number(out, v);
                out.push(b' ');
                out.extend_from_slice(operator);
            }
        }

        let mut out = Vec::new();
        if let Some(name) = &self.font_name {
            // `serialize::name` leaves `out` as it was where it fails.
            if serialize::name(&mut out, name).is_err() {
                out.extend_from_slice(b"/Helv");
            }
            out.push(b' ');
            write_number(&mut out, self.font_size.unwrap_or(0.0));
            out.extend_from_slice(b" Tf");
        }
        if let Some(color) = self.color {
            sep(&mut out);
            let (components, operator): (&[f32], &[u8]) = match &color {
                Color::Gray(g) => (std::slice::from_ref(g), b"g"),
                Color::Rgb(rgb) => (rgb, b"rg"),
                Color::Cmyk(cmyk) => (cmyk, b"k"),
            };
            for &c in components {
                write_number(&mut out, c.clamp(0.0, 1.0));
                out.push(b' ');
            }
            out.extend_from_slice(operator);
        }
        scalar(&mut out, self.char_space, b"Tc");
        scalar(&mut out, self.word_space, b"Tw");
        scalar(&mut out, self.horizontal_scale, b"Tz");
        scalar(&mut out, self.leading, b"TL");
        if let Some(mode) = self.text_mode {
            sep(&mut out);
            out.push(b'0' + mode as u8);
            out.extend_from_slice(b" Tr");
        }
        scalar(&mut out, self.rise, b"Ts");
        out
    }

    /// The fixed font size `Tf` asks for: its size where that is finite and
    /// above `0`, else `None`.
    ///
    /// `None` covers two cases a caller tells apart with
    /// [`auto_sized`](Self::auto_sized): a size of exactly `0`, which asks
    /// for the text to be fitted to its box, and a `Tf` that is absent or
    /// whose size is negative or not finite, which asks for nothing and is
    /// left to the caller's default.
    #[must_use]
    pub fn size(&self) -> Option<f32> {
        self.font_size.filter(|s| s.is_finite() && *s > 0.0)
    }

    /// Whether `Tf` asks for auto-sizing: a size of exactly `0`
    /// (ISO 32000-2 §12.7.4.3). A `/DA` with no `Tf`, or with a negative
    /// size, doesn't: the standard gives those no meaning.
    #[must_use]
    pub fn auto_sized(&self) -> bool {
        self.font_size == Some(0.0)
    }

    /// A [`TextBoxStyle`](crate::TextBoxStyle) drawing at this appearance's
    /// [`size`](Self::size), auto-sized where it is
    /// [`auto_sized`](Self::auto_sized), and otherwise the default style
    /// (so at the default style's size, `12`, where `Tf` is absent or its
    /// size is negative or not finite).
    ///
    /// Only the size is mapped: the text-state scalars (`Tc`, `Tw`, `Tz`,
    /// `TL`, `Tr`, `Ts`) are kept for writing back and aren't applied to the
    /// style, so a `/DA` saying `3 Tr` doesn't lay out invisible text.
    #[cfg(feature = "text-layout")]
    #[must_use]
    pub fn text_box_style(&self) -> crate::wrap::TextBoxStyle {
        let default = crate::wrap::TextBoxStyle::default();
        crate::wrap::TextBoxStyle {
            size: if self.auto_sized() {
                None
            } else {
                self.size().or(default.size)
            },
            ..default
        }
    }
}
