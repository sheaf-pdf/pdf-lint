//! The font a field's text is drawn in: the default resources' font a `/DA`
//! string names, read with `pdf-fonts`, or a standard 14 font chosen by that
//! name where the resources have none.

use std::borrow::Cow;
use std::fmt;

use pdf_content_streams::Res;
use pdf_fonts::{Font, FontKind, FontMetrics, Std14};
use pdf_gfx::{FontEncoder, LayoutFont};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable, Value, Why};

use crate::error::Error;
use crate::fault::Fault;
use crate::limits::{Cap, Limits};
use crate::read::Reader;

/// The font a field's text is drawn in, read once: a `pdf_fonts::Font`, the
/// resource an appearance names it by, and the heights its lines are measured
/// by.
///
/// A form's widgets share a handful of fonts, so a caller keeps one
/// `FieldFont` for each `/DA` font name and takes [`layout`](Self::layout) once
/// for a run of widgets. It owns all it holds, so it can be shared across
/// threads.
///
/// `Debug` prints what kind of font it is and numbers, never its name or the
/// name a `/DA` string gave.
pub struct FieldFont {
    font: Font,
    res: Option<Res>,
    fallback: bool,
    /// The heights lines are measured by, in glyph units, and the box comb
    /// text at an automatic size is fitted to, none for a standard font.
    metrics: FontMetrics,
}

impl FieldFont {
    /// The font `name`, a `/DA` string's font resource name, names in `fonts`,
    /// the default resources' font dictionary (`/DR /Font`) with its object
    /// number when it is indirect; else the standard 14 font that name falls
    /// back to ([`standard`](Self::standard)), told as
    /// [`Fault::FontNotInResources`]. [`Form::field_font`](crate::Form::field_font)
    /// passes the form's own.
    ///
    /// A font found is read with `pdf_fonts::Font::read` under
    /// [`Limits::fonts`], its streams decompressed within
    /// [`Limits::font_bytes`], and its departures told to `sink` as
    /// [`Fault::Font`]; what's read is the same whatever the sink. Its codes
    /// are its encoding's (WinAnsiEncoding, MacRomanEncoding, a built-in
    /// encoding with `/Differences`, each by glyph name), its widths its
    /// `/Widths`, or Adobe's metrics for a standard 14 font without them.
    ///
    /// A name the resources lack, or whose entry isn't a dictionary, falls
    /// back by name: one `pdf_fonts::Std14::named` knows (`Helvetica`, `Arial`,
    /// `ArialMT`, `Times-Roman`, `CourierNew,Bold`, …); else one of the
    /// abbreviations forms give the standard 14 (`Helv`, `HeBo`, `TiRo`,
    /// `Cour`, `ZaDb`, …); else the family its name begins with (`Tim`:
    /// Times, `Cour`: Courier, any other: Helvetica) in the face its name says
    /// (`Bold`, and `Italic` or `Oblique`, in any case), so
    /// `HelveticaLTStd-Bold` draws in Helvetica-Bold. PDFium draws its own
    /// Helvetica for every one of them.
    ///
    /// # Errors
    ///
    /// [`Error::Limit`] with [`Cap::FontBytes`] where the font's streams
    /// decompress past [`Limits::font_bytes`]; [`Error::Unreadable`] on
    /// locked, I/O, or reader-cap failures.
    pub fn find<'o, R, S>(
        fonts: Option<(&'o Dict, Option<ObjectId>)>,
        name: &[u8],
        objects: R,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<FieldFont, Error>
    where
        R: Resolve<'o> + Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        let within = fonts.and_then(|(_, id)| id);
        let mut found = None;
        if let Some(raw) = fonts.and_then(|(fonts, _)| fonts.get(name)) {
            let mut reader = Reader {
                objects,
                rules: Rules::Pdfium,
                limits,
                sink: &mut *sink,
            };
            if let Some((object, id)) = reader.reached(raw, within)? {
                let own = raw.as_reference().is_some().then_some(id).flatten();
                found = object.as_dict().map(|dict| (dict, own));
            }
        }
        let Some((dict, own)) = found else {
            if sink.wants() {
                sink.fault(Place::new(within, None), Fault::FontNotInResources);
            }
            return Ok(FieldFont::standard(fallback(name)));
        };

        let mut budget = limits.font_bytes;
        let mut lifted = Lift {
            sink,
            within: own.or(within),
        };
        let font = Font::read(objects, dict, &limits.fonts, &mut budget, &mut lifted)
            .map_err(font_error)?;
        let res = match own {
            Some(id) => Some(Res::written(id)),
            // A reference held in a resource's objects names one of them
            // (`ResBody::Objects`), not the file's object: such a font is
            // named by the caller that writes it.
            None if holds_reference(dict) => None,
            None => Res::dict(dict.clone()).ok(),
        };
        let face = match dict.get("BaseFont") {
            Some(raw) => match raw.as_reference() {
                Some(id) => objects.resolve(id),
                None => Some(raw),
            },
            None => None,
        }
        .and_then(Object::as_name)
        .and_then(Std14::named);
        let mut metrics = measured(&font, face);
        if face.is_some()
            && matches!(font.kind(), FontKind::Type1 | FontKind::MMType1)
            && !embeds_program(dict, objects)
        {
            metrics.bbox = None;
        }
        Ok(FieldFont {
            font,
            res,
            fallback: false,
            metrics,
        })
    }

    /// The standard 14 font `font`, as a field whose `/DA` font the default
    /// resources lack is drawn in: read from the dictionary it is written as
    /// (`pdf_fonts::Std14::dict`: not embedded, WinAnsiEncoding for the twelve
    /// Latin faces, Symbol's and ZapfDingbats' own encodings), so the codes and
    /// widths a layout measures are the ones a reader of the written font gets.
    /// It's a fallback: the caller adds it to the default resources.
    #[must_use]
    pub fn standard(font: Std14) -> FieldFont {
        let dict = font.dict();
        let read = Font::read(
            NoObjects,
            &dict,
            &pdf_fonts::Limits::DEFAULT,
            &mut 0,
            &mut (),
        )
        .expect("a font by name alone has no stream to decompress");
        let mut metrics = measured(&read, Some(font));
        metrics.bbox = None;
        FieldFont {
            font: read,
            res: Res::dict(dict).ok(),
            fallback: true,
            metrics,
        }
    }

    /// The resource an appearance names the font by: the font's object, for
    /// one the default resources hold by reference; its dictionary, for one
    /// held directly that holds no reference at any depth, and for a fallback.
    /// `None` for a direct font dictionary that holds a reference (to its
    /// descriptor, say), which a copy of the dictionary would name wrongly: a
    /// caller that writes the font as an object of its own names it by
    /// `Res::written`.
    #[must_use]
    pub fn res(&self) -> Option<Res> {
        self.res.clone()
    }

    /// Whether the default resources lack the font, so the caller adds it to
    /// them ([`Fault::FontNotInResources`]).
    #[must_use]
    pub const fn is_fallback(&self) -> bool {
        self.fallback
    }

    /// The font, as `pdf-fonts` read it.
    #[must_use]
    pub const fn font(&self) -> &Font {
        &self.font
    }

    /// What text is laid out in it with: its codes and widths, indexed once,
    /// and its heights. A standard 14 font whose heights neither a program read
    /// nor a descriptor with a `/FontBBox` gives has the ascent and
    /// descent of the face PDFium as of chromium/8152 draws it in, not Adobe's
    /// metrics', as PDFium measures a field's lines by them (where a
    /// descriptor with a box gives both as 0, PDFium measures its face's `A`
    /// and `g`, for which the face's ascent and descent stand in); any other
    /// font its own (`pdf_fonts::Font::metrics`).
    ///
    /// Its box is the font's own, but a standard 14 Type 1 font that embeds
    /// no program has none: the face it's drawn in is the viewer's, and
    /// PDFium sizes comb text in it by its table of sizes, where it fits comb
    /// text in any other font to a cell by the font's box
    /// (`fpdfsdk/pwl/cpwl_edit.cpp`, `GetCharArrayAutoFontSize`).
    #[must_use]
    pub fn layout(&self) -> FieldLayoutFont<'_> {
        FieldLayoutFont {
            encoder: FontEncoder::new(&self.font),
            metrics: self.metrics,
        }
    }
}

impl fmt::Debug for FieldFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldFont")
            .field("fallback", &self.fallback)
            .field("bindable", &self.res.is_some())
            .field("metrics", &self.metrics)
            .field("font", &self.font)
            .finish()
    }
}

/// A [`FieldFont`] as `pdf-gfx`'s text layout takes it: a
/// `pdf_gfx::LayoutFont` over its codes, widths, and heights
/// ([`FieldFont::layout`]).
///
/// It borrows its `FieldFont`. `Debug` prints counts and numbers alone.
#[derive(Debug, Clone)]
pub struct FieldLayoutFont<'a> {
    encoder: FontEncoder<'a>,
    metrics: FontMetrics,
}

impl LayoutFont for FieldLayoutFont<'_> {
    fn font_metrics(&self) -> FontMetrics {
        self.metrics
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        self.encoder.char_advance(ch)
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        self.encoder.encode_char(ch, out)
    }
}

/// The heights `font` is measured by, `face` the standard 14 font its name
/// names: its own, but for a simple font PDFium draws in a standard face of
/// its own, whose heights `pdf-fonts` guesses: no descriptor gives them with a
/// `/FontBBox`, and no program read does. PDFium takes those from its face
/// (`core/fpdfapi/font/cpdf_font.cpp`, `CheckFontMetrics`), so they're its
/// face's ([`pdfium_heights`]). A Type 3 font draws in no face.
fn measured(font: &Font, face: Option<Std14>) -> FontMetrics {
    let metrics = font.metrics();
    let simple = matches!(
        font.kind(),
        FontKind::Type1 | FontKind::MMType1 | FontKind::TrueType
    );
    match face {
        Some(face) if simple && font.approx().heights => {
            let (ascent, descent) = pdfium_heights(face);
            FontMetrics::new(ascent, descent, metrics.bbox)
        }
        _ => metrics,
    }
}

/// The ascent and descent, in glyph units, of the face PDFium as of
/// chromium/8152 draws a standard 14 font in when the file embeds none: its
/// own built-in faces' ascender and descender, the same with the machine's
/// fonts hidden. Each pair was measured from the appearances PDFium makes for
/// a text field at 100 points, a single line giving their sum and a
/// multiline field's first line the ascent. Symbol and ZapfDingbats were
/// measured on ASCII text, which their encodings draw as their own glyphs.
const fn pdfium_heights(face: Std14) -> (f32, f32) {
    match face {
        Std14::Helvetica => (945.0, -224.0),
        Std14::HelveticaBold => (962.0, -227.0),
        Std14::HelveticaOblique => (951.0, -224.0),
        Std14::HelveticaBoldOblique => (962.0, -227.0),
        Std14::Courier | Std14::CourierOblique => (803.0, -248.0),
        Std14::CourierBold | Std14::CourierBoldOblique => (811.0, -248.0),
        Std14::TimesRoman => (878.0, -249.0),
        Std14::TimesBold => (889.0, -249.0),
        Std14::TimesItalic => (868.0, -249.0),
        Std14::TimesBoldItalic => (869.0, -249.0),
        Std14::Symbol => (1010.0, -293.0),
        Std14::ZapfDingbats => (826.0, -163.0),
    }
}

/// The abbreviations forms give the standard 14 in their default resources,
/// matched as written: `HeBo` and `HeBO` are two faces.
const ABBREVIATIONS: [(&str, Std14); 14] = [
    ("Helv", Std14::Helvetica),
    ("HeBo", Std14::HelveticaBold),
    ("HeOb", Std14::HelveticaOblique),
    ("HeBO", Std14::HelveticaBoldOblique),
    ("Cour", Std14::Courier),
    ("CoBo", Std14::CourierBold),
    ("CoOb", Std14::CourierOblique),
    ("CoBO", Std14::CourierBoldOblique),
    ("TiRo", Std14::TimesRoman),
    ("TiBo", Std14::TimesBold),
    ("TiIt", Std14::TimesItalic),
    ("TiBI", Std14::TimesBoldItalic),
    ("Symb", Std14::Symbol),
    ("ZaDb", Std14::ZapfDingbats),
];

/// The standard 14 font a `/DA` font name the resources lack falls back to
/// ([`FieldFont::find`]).
fn fallback(name: &[u8]) -> Std14 {
    if let Some(face) = Std14::named(name) {
        return face;
    }
    if let Some(&(_, face)) = ABBREVIATIONS.iter().find(|(a, _)| a.as_bytes() == name) {
        return face;
    }
    let name = match name.iter().position(|&c| c == b'+') {
        Some(6) => &name[7..],
        _ => name,
    };
    let lower = name.to_ascii_lowercase();
    let has = |word: &[u8]| lower.windows(word.len()).any(|w| w == word);
    // Each family's faces: plain, bold, slanted, both.
    let faces = if lower.starts_with(b"tim") {
        [
            Std14::TimesRoman,
            Std14::TimesBold,
            Std14::TimesItalic,
            Std14::TimesBoldItalic,
        ]
    } else if lower.starts_with(b"cour") {
        [
            Std14::Courier,
            Std14::CourierBold,
            Std14::CourierOblique,
            Std14::CourierBoldOblique,
        ]
    } else {
        [
            Std14::Helvetica,
            Std14::HelveticaBold,
            Std14::HelveticaOblique,
            Std14::HelveticaBoldOblique,
        ]
    };
    let bold = has(b"bold");
    let slanted = has(b"italic") || has(b"oblique");
    faces[usize::from(bold) + 2 * usize::from(slanted)]
}

/// Whether the font `dict`'s descriptor holds a program (`/FontFile`,
/// `/FontFile2`, or `/FontFile3`), the descriptor followed once if it's a
/// reference.
fn embeds_program<'o, R: Resolve<'o>>(dict: &'o Dict, objects: R) -> bool {
    let descriptor = match dict.get("FontDescriptor") {
        Some(raw) => match raw.as_reference() {
            Some(id) => objects.resolve(id),
            None => Some(raw),
        },
        None => None,
    };
    descriptor.and_then(Object::as_dict).is_some_and(|d| {
        ["FontFile", "FontFile2", "FontFile3"]
            .iter()
            .any(|key| d.get(key.as_bytes()).is_some())
    })
}

/// Whether a value of `dict`, at any depth, is an indirect reference.
fn holds_reference(dict: &Dict) -> bool {
    let mut left: Vec<&Object> = dict.iter().map(|(_, value)| value).collect();
    while let Some(object) = left.pop() {
        match object.value() {
            Value::Ref(_) => return true,
            Value::Array(items) => left.extend(items),
            Value::Dict(inner) => left.extend(inner.iter().map(|(_, value)| value)),
            Value::Stream(stream) => left.extend(stream.dict.iter().map(|(_, value)| value)),
            _ => {}
        }
    }
    false
}

/// What reading a field's font failed by, as this crate's error.
fn font_error(err: pdf_fonts::Error) -> Error {
    match err {
        pdf_fonts::Error::TooLarge => Error::Limit(Cap::FontBytes),
        pdf_fonts::Error::Unreadable(unreadable) => Error::Unreadable(unreadable),
        // `pdf_fonts::Error` is non-exhaustive: a kind added later is a cap
        // or a read failure this crate can't name yet.
        _ => Error::Limit(Cap::FontBytes),
    }
}

/// Passes `pdf-fonts`' faults on as this crate's, a fault in the font
/// dictionary itself placed in the object it's read within.
struct Lift<'s, S: ?Sized> {
    sink: &'s mut S,
    within: Option<ObjectId>,
}

impl<S: Sink<Fault> + ?Sized> Sink<pdf_fonts::Fault> for Lift<'_, S> {
    fn fault(&mut self, at: Place, fault: pdf_fonts::Fault) {
        let at = match (at.object, self.within) {
            (None, Some(id)) => at.in_object(id),
            _ => at,
        };
        self.sink.fault(at, Fault::Font(fault));
    }

    fn wants(&self) -> bool {
        self.sink.wants()
    }
}

/// No objects and no streams: what a font written by name alone is read
/// over.
#[derive(Clone, Copy)]
struct NoObjects;

impl<'o> Resolve<'o> for NoObjects {
    fn resolve(&self, _: ObjectId) -> Option<&'o Object> {
        None
    }
}

impl Streams for NoObjects {
    fn decoded(&self, id: ObjectId, _: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        Err(Unreadable::new(
            id,
            Why::Damaged,
            format!("{id} R isn't there"),
        ))
    }
}
