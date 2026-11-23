//! In-process fuzzing entrypoint exercised by unit tests and
//! `fuzz/fuzz_targets/forms.rs`.
//!
//! Verifies on arbitrary byte inputs:
//! 1. Zero panics across `Form::read`, `Form::read_with`, `Form::full_name`,
//!    `Form::font`, and `Widget::annot`, under small caps.
//! 2. Sink invariance: `&mut ()` and `Vec` fault sinks read the same form
//!    under both `Rules::Iso` and `Rules::Pdfium`.
//! 3. The tree is a tree: every field but a root has a parent before it that
//!    lists it once, and fields and widgets together stay within
//!    `Limits::fields`.
//! 4. `Debug` formatting never panics on a `Form`, `Field`, `Widget`, or
//!    `Fault`.
//! 5. Buttons and choices: for each field, `Field::options`, `selected`,
//!    `selection`, and `top_index`, and for each widget `Widget::states`,
//!    each alike under `&mut ()` and a `Vec` sink, within `Limits::items`
//!    or refused for it, a selection sorted, each once, and of an option;
//!    for each check box and radio button field, `Field::group` alike
//!    whatever the sink, and each of its widgets pressed from the states its
//!    `/AS` gives; and a `group::Group` cut from the input (`check_group`),
//!    its widgets pressed in turn. Each press answers a state for each
//!    widget and holds the group's rules: an off widget turned on with those
//!    of its on state and export value in unison and alone out of it, every
//!    other off; an on one off with those in unison with it, or a radio
//!    button with NoToggleToOff left as it was; an index past the widgets
//!    changing nothing. The value is the first on widget's on state, `Off`
//!    with none, each `/AS` its widget's on state or `Off`, and no `Debug`
//!    shows a state's name, an export value, or an option.
//! 6. Appearances (`text-layout`): `appearance::text` over a text, flags,
//!    `/MaxLen`, `/Q`, a box, a border, colors, a rotation, and a `/DA` cut
//!    from the input (`check_appearance`), in each standard 14 font; and for
//!    each widget of the form's text fields, `appearance::widget_text`, the
//!    font its `/DA` names in the form's default resources
//!    (`Form::field_font`, each of those fonts read alike whatever the
//!    sink), `WidgetText::make`, and `appearance::text_for_widget`, each under
//!    `&mut ()` and a `Vec` sink. Each appearance is made the same twice and
//!    whatever the sink, and `text_for_widget` makes what the three make and
//!    tells what they tell. What's made holds what `text` promises: a cap
//!    passed (the text's bytes, a comb field's cells), no box, or a number
//!    that isn't finite is refused as it says, as is a size or a box that
//!    sets a string past the reach `text` keeps to (`1e35` from the box's
//!    origin), and every string of made content is within that reach;
//!    a character the font lacks is told at its offset in the text, and for
//!    a password only as `*` at `0`; an appearance is of finite numbers, its
//!    box placed on the widget's rectangle by its matrix, with at most one
//!    resource, the font, and content whose `q`/`Q`, one `/Tx BMC`/`EMC`,
//!    and `BT`/`ET` are balanced, whose every number is finite, whose
//!    strings for a password are the font's `*` alone, and which, in a font
//!    of the standard 14 the caller writes, interprets with no error and no
//!    fault; no `Debug` of what's read or made, or of an error or a fault,
//!    shows the text.
//! 6. A fallback (`text-layout` and `embed`): where the input carries a
//!    TrueType program after its text that `pdf_fonts::embed::Program::open`
//!    opens, `appearance::text_with_fallback` draws the same text in each
//!    standard 14 font and the program (`check_fallback`), made the same
//!    twice: refused only where the font alone refuses it too, a character
//!    of the text told at its offset (for a password only `*` at `0`); made,
//!    its resources the font and the fallback, the fallback named exactly
//!    where glyphs of it are returned, each glyph the program's for a
//!    character of the text, as given or a cluster of it composed (for a
//!    password `*` alone), its content holding what `text`'s holds, and for
//!    text already composed that the font alone draws, `text`'s appearance
//!    byte for byte and no glyph.
//!
//! `over` checks the same of the form a parsed PDF file's catalog names.

use std::collections::BTreeMap;

use pdf_objects::fault::Place;
use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{
    Dict, Object, ObjectId, Resolve, Stream, Streams, Unreadable, Value, Why, WithStreams,
};

use crate::{Fault, Form, Limits};

/// Built-in seed inputs: a catalog, then objects 1, 2, … split on `0xFF`, an
/// object holding `\nstream\n` a stream of the bytes after it. The last are
/// [`check_appearance`]'s first: its choices, numbers, `/DA` and text. The
/// seeds that carry a fallback's program after their text are written by a
/// test, which builds the program (`the_seeds_are_written`).
pub const SEEDS: &[&[u8]] = &[
    b"",
    b"<< /AcroForm << /Fields [1 0 R 3 0 R] /DA (/Helv 0 Tf 0 g) /Q 1 /DR << /Font << /Helv 4 0 R >> >> >> >>\xff<< /T (address) /FT /Tx /Ff 4096 /Kids [2 0 R] >>\xff<< /T (city) /Parent 1 0 R /V (Trenton) /MaxLen 20 /Subtype /Widget /Rect [72 700 200 716] /MK << /BC [0] >> >>\xff<< /T (agree) /FT /Btn /V /Yes /Kids [5 0 R 6 0 R] >>\xff<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\xff<< /Parent 3 0 R /Subtype /Widget /Rect [72 680 82 690] /AS /Yes >>\xff<< /Parent 3 0 R /Subtype /Widget /Rect [92 680 102 690] /AS /Off >>",
    b"<< /AcroForm 1 0 R >>\xff<< /Fields [2 0 R 2 0 R << /T (direct) /FT /Ch /V [(a) (b) /c] >>] /NeedAppearances true /SigFlags 3 /XFA [] >>\xff<< /T (loop) /Kids [2 0 R 3 0 R] >>\xff<< /T (kid) /Parent 9 0 R /FT /Sig /V << /Type /Sig >> /Kids 7 >>",
    b"<< /AcroForm << /Fields [1 0 R] >> >>\xff<< /T (a) /Kids [2 0 R] >>\xff<< /T (b) /Kids [3 0 R] >>\xff<< /T (c) /Kids [4 0 R] >>\xff<< /T (d) /Kids [5 0 R] >>\xff<< /T (e) /FT /Tx /V 6 0 R /DV 3.5 /Ff 1.0 >>\xff<< /Length 2 >>\nstream\nhi",
    // Text fields' widgets: turned a quarter turn with a beveled border and
    // both colors; a comb password with a dashed border in a font the
    // resources lack; a multiline field whose value is a stream, its widget a
    // kid turned a half turn with an underline, in a font with widths and a
    // descriptor; and a password turned back a quarter turn, its dashed
    // border from `/Border`.
    b"<< /AcroForm << /Fields [1 0 R 2 0 R 3 0 R 4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R /F2 6 0 R >> >> >> >>\xff<< /T (name) /FT /Tx /V (Ada #1) /Q 1 /Subtype /Widget /Rect [0 0 100 20] /MK << /R 90 /BG [1 1 0] /BC [0 0 1] >> /BS << /W 2 /S /B >> >>\xff<< /T (pin) /FT /Tx /Ff 16785408 /MaxLen 4 /V (12@4) /DA (/TiRo 0 Tf 0 0 0.502 rg) /Subtype /Widget /Rect [0 0 80 20] /MK << /BC [0] >> /BS << /S /D /D [2 1] >> >>\xff<< /T (note) /FT /Tx /Ff 4096 /V 7 0 R /DA (/F2 9 Tf 0 0 0 1 k) /Kids [8 0 R] >>\xff<< /T (secret) /FT /Tx /Ff 8192 /V (hunter#2) /Subtype /Widget /Rect [0 0 60 15] /MK << /R -90 /BC [0.25] >> /Border [0 0 1 [3 2]] >>\xff<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\xff<< /Type /Font /Subtype /Type1 /BaseFont /Foo /FirstChar 97 /LastChar 101 /Widths [500 600 700 800 900] /FontDescriptor 9 0 R >>\xff<< /Length 16 >>\nstream\nabc dee\r\nbad cab\xff<< /Parent 3 0 R /Subtype /Widget /Rect [0 0 100 50] /MK << /R 180 /BC [0 0 0 1] >> /BS << /W 1 /S /U >> >>\xff<< /Type /FontDescriptor /FontName /Foo /Flags 32 /FontBBox [0 -200 1000 800] /Ascent 800 /Descent -200 >>",
    // An inset border turned three quarter turns, with no `/DA` anywhere;
    // and a field whose font is a direct dictionary that holds a reference,
    // which no appearance's resources can name.
    b"<< /AcroForm << /Fields [1 0 R 2 0 R] /DR << /Font << /D << /Type /Font /Subtype /Type1 /BaseFont /Foo /FontDescriptor 3 0 R >> >> >> >> >>\xff<< /T (plain) /FT /Tx /V (no /DA) /Subtype /Widget /Rect [10 10 110 40] /MK << /R 270 /BG [0.5] /BC [1 0 0] >> /BS << /W 3 /S /I >> >>\xff<< /T (unbound) /FT /Tx /V (ab) /DA (/D 10 Tf) /Subtype /Widget /Rect [0 0 50 20] >>\xff<< /Type /FontDescriptor /FontName /Foo /Flags 32 /Ascent 700 /Descent -300 >>",
    // Buttons: radio buttons in unison, one on and one with no `Off`; radio
    // buttons with NoToggleToOff, of the names `On` and `spouse`; check boxes
    // with `/Opt`, one named by its index, one by a name of its own, one with
    // no appearance; and a push button with captions.
    b"<< /AcroForm << /Fields [1 0 R 4 0 R 7 0 R 10 0 R] /DR << /Font << /ZaDb 13 0 R >> >> >> >>\xff<< /T (unison) /FT /Btn /Ff 33587200 /V /1 /Kids [2 0 R 3 0 R 14 0 R] >>\xff<< /Parent 1 0 R /Subtype /Widget /Rect [0 0 20 20] /AS /1 /AP << /N << /1 11 0 R /Off 12 0 R >> /D << /1 11 0 R /Off 12 0 R >> >> /MK << /CA (l) >> >>\xff<< /Parent 1 0 R /Subtype /Widget /Rect [30 0 50 20] /AS /Off /AP << /N << /2 11 0 R /Off 12 0 R >> >> /MK << /CA (8) >> >>\xff<< /T (radios) /FT /Btn /Ff 49152 /Kids [5 0 R 6 0 R] >>\xff<< /Parent 4 0 R /Subtype /Widget /Rect [0 30 20 50] /AS /On /AP << /N << /On 11 0 R /Off 12 0 R >> >> >>\xff<< /Parent 4 0 R /Subtype /Widget /Rect [30 30 50 50] /AS /Off /AP << /N << /spouse 11 0 R >> >> >>\xff<< /T (fruit) /FT /Btn /Opt [(apple) (pear) (apple)] /V /0 /Kids [8 0 R 9 0 R 15 0 R] >>\xff<< /Parent 7 0 R /Subtype /Widget /Rect [0 60 20 80] /AS /0 /AP << /N << /0 11 0 R /Off 12 0 R >> >> /MK << /CA (4) >> >>\xff<< /Parent 7 0 R /Subtype /Widget /Rect [30 60 50 80] /AS /Off /AP << /N << /a 11 0 R /Off 12 0 R >> >> >>\xff<< /T (push) /FT /Btn /Ff 65536 /H /P /Subtype /Widget /Rect [0 90 60 110] /DA (/Helv 0 Tf 0 g) /MK << /CA (Go) /RC (Over) /AC (Down) /TP 0 >> >>\xff<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] >>\nstream\n0 g 5 5 10 10 re f\xff<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] >>\nstream\n\xff<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>\xff<< /Parent 1 0 R /Subtype /Widget /Rect [60 0 80 20] /AS /Off /AP << /N << /1 11 0 R >> >> >>\xff<< /Parent 7 0 R /Subtype /Widget /Rect [60 60 80 80] /AS /Off >>",
    // Choices: an editable combo box of pairs and a wrong item, its `/I`
    // against its `/V`; and two list boxes that inherit `/Opt` and `/TI`,
    // one of multiple selection whose `/I` agrees with its `/V`, one whose
    // `/I` is a number.
    b"<< /AcroForm << /Fields [1 0 R 2 0 R] /DA (/Helv 0 Tf 0 g) >> >>\xff<< /T (combo) /FT /Ch /Ff 393216 /Opt [[(e1) (Label one)] [(e2) (Label two)] 7] /V (e2) /I [0] /TI 1 /Subtype /Widget /Rect [0 0 100 20] >>\xff<< /T (lists) /FT /Ch /Opt 5 0 R /TI 2 /Kids [3 0 R 4 0 R] >>\xff<< /T (multi) /Parent 2 0 R /Ff 2097152 /V [(alpha) (charlie)] /I [0 2] /Subtype /Widget /Rect [0 30 80 70] >>\xff<< /T (single) /Parent 2 0 R /V (bravo) /I 1 /TI -1 /Subtype /Widget /Rect [0 80 80 120] >>\xff[(alpha) (bravo) (charlie) [(d) (delta)] /wrong]",
    // `check_group`'s, each as it's cut. Radio buttons in unison with
    // NoToggleToOff, `/1`, `/2`, `/1`, the first on, each pressed and one
    // past them.
    b"\x07\x03\x01\x03\x03\x04\x03\x00\x01\x02\x04",
    // Check boxes of `/Opt [(apple) (pear) (apple)]` with no appearances,
    // one state given for three.
    b"\x00\x03\x00\x01\x08\x10\x08\x00\x02\x00\x01",
    // Check boxes of four on states, all on, the second pressed twice.
    b"\x00\x04\x0f\x04\x01\x02\x05\x06\x01\x01\x03\x00",
    // Radio buttons in unison of one on state, `/1`, and the export values
    // `apple`, `pear`, `apple`: the first and the third go together.
    b"\x05\x03\x00\x03\x0b\x13\x0b\x00\x01\x02",
    // `check_appearance`'s, each as it's cut, which a test holds. A single
    // line at an automatic size in a box 100 by 20, its border solid, 1 wide,
    // in black: `/Helv 0 Tf 0 g`, `Hello #1`.
    b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x04\n\x00\x00\xc8B\x00\x00\xa0A\x00\x00\x80?\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00/Helv 0 Tf 0 g\xffHello #1",
    // Comb, 5 cells, centered, turned a quarter turn, its border dashed
    // `[3 1]` in blue over a light gray background: `/Cour 12 Tf 0 0 0.502
    // rg`, `12é@4`.
    b"\x00\x00\x00\x01\x05\x00\x00\x00Z\x00\x00\x00\x8b\x09\n\x00\x00\xc8B\x00\x00\xa0A\x00\x00\x80?\x00\x00@@\x00\x00\x80?\x00\x00\x00\x00fff?\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x80?\x00\x00\x00\x00\x00\x00\x00\x00/Cour 12 Tf 0 0 0.502 rg\xff12\xc3\xa9@4",
    // A multiline password, right-aligned, turned back a quarter turn, its
    // border beveled, 2 wide, over a CMYK background: `/TiRo 10 Tf 0 0 0 1 k`,
    // two lines.
    b"\x000\x00\x00\x00\x00\x00\x00\xa6\xff\xff\xff\x14\x03\n\x00\x00\xc8B\x00\x00HB\x00\x00\x00@\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xcd\xcc\xcc=\xcd\xccL>\x9a\x99\x99>\xcd\xcc\xcc>\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00/TiRo 10 Tf 0 0 0 1 k\xffpass word\r\nsecret#",
    // An inset border turned `i32::MIN` degrees, `1e30` wide, its dash array
    // NaN, infinity, and -1, in black over gray, in a box 200 by 30:
    // `/Helv 9 Tf`, `x`.
    b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x80\xd8\x05\n\x00\x00HC\x00\x00\xf0A\xca\xf2Iq\x00\x00\xc0\x7f\x00\x00\x80\x7f\x00\x00\x80\xbf\x00\x00\x00?\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00/Helv 9 Tf\xffx",
    // Comb, `u32::MAX` cells under a `Limits::text` of 16, turned 450 degrees,
    // underlined in black: `/Helv 12 Tf`, `abc`, refused.
    b"\x00\x00\x00\x01\xff\xff\xff\xff\xc2\x01\x00\x00!\x04\x04\x00\x00\xc8B\x00\x00\xa0A\x00\x00\x80?\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00/Helv 12 Tf\xffabc",
    // A character no standard 14 font's encoding holds after two the Latin
    // ones do: `/Helv 12 Tf`, `é€x☃`.
    b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\n\x00\x00\xc8B\x00\x00\xa0A\x00\x00\x80?\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00/Helv 12 Tf\xff\xc3\xa9\xe2\x82\xacx\xe2\x98\x83",
    // A gray background that isn't a number: `/Helv 12 Tf`, `bg`, refused.
    b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01\n\x00\x00\xc8B\x00\x00\xa0A\x00\x00\x80?\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xc0\x7f\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00/Helv 12 Tf\xffbg",
];

/// Reads the form of a catalog and objects split on `0xFF`, and checks what
/// the module's docs list; with `text-layout`, makes appearances of what the
/// same bytes are cut into too ([`check_appearance`]).
pub fn check(data: &[u8]) {
    #[cfg(feature = "text-layout")]
    check_appearance(data);

    let parsed = Parsed::of(data);
    read(parsed.objects(), &parsed.catalog);
    check_group(data);
}

/// A catalog and objects 1, 2, … split on `0xFF` and parsed, the first 24
/// objects of them; an object holding `\nstream\n` a stream of its
/// dictionary before it and the bytes after.
struct Parsed {
    catalog: Dict,
    objects: BTreeMap<ObjectId, Object>,
    /// Each stream's data, as it's given.
    data: BTreeMap<ObjectId, Vec<u8>>,
}

impl Parsed {
    fn of(data: &[u8]) -> Parsed {
        let mut parts = data.split(|&b| b == 0xFF);
        let mut parse_limits = pdf_objects::Limits::DEFAULT;
        parse_limits.nesting = 16;
        parse_limits.values = 512;
        let parse = |bytes: &[u8]| {
            Parser::at(bytes, 0)
                .rules(Rules::Pdfium)
                .limits(parse_limits)
                .value(&mut ())
                .ok()
        };

        let catalog: Dict = parts
            .next()
            .and_then(parse)
            .and_then(|object| match object.into_value() {
                Value::Dict(dict) => Some(dict),
                _ => None,
            })
            .unwrap_or_default();
        let mut parsed = Parsed {
            catalog,
            objects: BTreeMap::new(),
            data: BTreeMap::new(),
        };
        for (index, chunk) in parts.take(24).enumerate() {
            let id = ObjectId::new(index as u32 + 1, 0);
            match split_once(chunk, b"\nstream\n") {
                Some((dict, data)) => {
                    if let Some(Value::Dict(dict)) = parse(dict).map(Object::into_value) {
                        parsed.data.insert(id, data.to_vec());
                        let stream = Stream::new(dict, data.to_vec());
                        parsed.objects.insert(id, Object::from(stream));
                    }
                }
                None => {
                    if let Some(object) = parse(chunk) {
                        parsed.objects.insert(id, object);
                    }
                }
            }
        }
        parsed
    }

    /// The objects, each stream's data as it's given, refused past the cap
    /// asked for.
    fn objects(&self) -> impl Resolve<'_> + Streams + Copy + '_ {
        WithStreams::new(&self.objects, move |id: ObjectId, cap: usize| {
            match self.data.get(&id) {
                Some(data) if data.len() <= cap => Ok(data.clone()),
                Some(_) => Err(Unreadable::new(id, Why::Limit, "past its cap")),
                None => Err(Unreadable::new(id, Why::Damaged, "no stream")),
            }
        })
    }
}

/// Reads the form the catalog of a parsed PDF file names, and checks what the
/// module's docs list.
pub fn over<'o, R: Resolve<'o> + Streams + Copy>(objects: R, trailer: &'o Dict) {
    if let Ok(Some(catalog)) = pdf_semantics::catalog(objects, trailer) {
        read(objects, catalog);
    }
}

fn read<'o, R: Resolve<'o> + Streams + Copy>(objects: R, catalog: &'o Dict) {
    let mut limits = Limits::DEFAULT;
    limits.hops = 8;
    limits.fields = 48;
    limits.depth = 4;
    limits.items = 8;
    limits.text = 1 << 10;
    limits.comb_cells = 1 << 6;
    limits.font_budget = 1 << 16;

    for rules in [Rules::Iso, Rules::Pdfium] {
        let quiet = Form::read(objects, catalog, rules, &limits);
        let mut faults: Vec<(Place, Fault)> = Vec::new();
        let told = Form::read_with(objects, catalog, rules, &limits, &mut faults);
        assert_eq!(format!("{quiet:?}"), format!("{told:?}"));
        let _ = format!("{faults:?}");
        for (_, fault) in &faults {
            let _ = fault.to_string();
        }

        let Ok(Some(form)) = quiet else { continue };
        let Ok(Some(told)) = told else {
            panic!("a sink changed whether the form was read")
        };
        let fields = form.fields();
        assert_eq!(fields.len(), told.fields().len());
        let widgets: usize = fields.iter().map(|field| field.widgets().len()).sum();
        assert!(fields.len() + widgets <= limits.fields);
        for (index, field) in fields.iter().enumerate() {
            assert_eq!(format!("{field:?}"), format!("{:?}", told.fields()[index]));
            match field.parent() {
                Some(parent) => {
                    assert!(parent < index);
                    let listed = fields[parent].kids().iter().filter(|&&k| k == index);
                    assert_eq!(listed.count(), 1);
                }
                None => assert!(form.roots().contains(&index)),
            }
            assert!(form.full_name(index).is_some());
            let _ = field.kind();
            for widget in field.widgets() {
                let _ = format!("{widget:?}");
                let _ = widget.annot(objects, rules, &limits);
            }
        }
        assert!(form.full_name(fields.len()).is_none());
        for name in form.font_names() {
            let _ = form.font(objects, name, &limits);
        }
        let _ = (form.has_xfa(), form.quadding(), form.sig_flags());
        buttons_and_choices(objects, &form, rules, &limits);

        #[cfg(feature = "text-layout")]
        widget_appearances(objects, &form, rules, &limits);
    }
}

/// `haystack` before and after the first `needle` in it.
fn split_once<'a>(haystack: &'a [u8], needle: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let at = haystack.windows(needle.len()).position(|w| w == needle)?;
    Some((&haystack[..at], &haystack[at + needle.len()..]))
}

/// Makes a text field's appearance with `appearance::text` from what `data`
/// is cut into ([`Cut`]) in each standard 14 font, each twice the same, and
/// holds what `text` promises of it ([`holds`]); each one made interprets
/// with no error and no fault.
#[cfg(feature = "text-layout")]
pub fn check_appearance(data: &[u8]) {
    use crate::appearance::text;

    let cut = Cut::of(data);
    let (field, look, limits) = (cut.field(), cut.look(), cut.limits());
    let store: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let nothing = WithStreams::new(&store, |id: ObjectId, _: usize| {
        Err(Unreadable::new(id, Why::Damaged, "no stream"))
    });
    for font in faces() {
        let layout = font.layout();
        let res = font
            .res()
            .expect("a standard 14 font is named by its dictionary");
        let made = text(&field, &look, &cut.da, res.clone(), &layout, &limits);
        let again = text(&field, &look, &cut.da, res.clone(), &layout, &limits);
        assert_eq!(made, again, "the same appearance made twice");
        holds(&field, &look, &cut.da, &limits, &layout, &res, &made);
        if let Ok(made) = &made {
            let resources = resources(made);
            if let Err(why) = interprets(nothing, made, &resources) {
                panic!("an appearance in a standard 14 font interprets with {why}");
            }
        }
    }
    #[cfg(feature = "embed")]
    check_fallback(&cut);
}

/// Where `cut` carries a TrueType program that opens, makes its text's
/// appearance with `appearance::text_with_fallback` in each standard 14 font
/// and the program, each twice the same, and holds what it promises beside
/// what `appearance::text` makes in the font alone.
#[cfg(all(feature = "text-layout", feature = "embed"))]
fn check_fallback(cut: &Cut<'_>) {
    use pdf_content_streams::Res;
    use pdf_fonts::embed::Program;
    use pdf_gfx::LayoutFont;
    use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick};
    use unicode_segmentation::UnicodeSegmentation;

    use crate::FieldFlags;
    use crate::appearance::{Fallback, Unmade, text, text_with_fallback};

    let Some(bytes) = cut.program else {
        return;
    };
    let Ok(program) = Program::open(bytes.to_vec(), 0) else {
        return;
    };
    let (field, look, limits) = (cut.field(), cut.look(), cut.limits());
    let res = Res::written(ObjectId::new(7, 0));
    let fallback = Fallback::new(res.clone(), &program);
    let given = field.text;
    let password = field.flags.contains(FieldFlags::PASSWORD);
    let composed = is_nfc_quick(given.chars()) == IsNormalized::Yes;
    // Each character the text may be drawn as: as given, or a cluster of it
    // composed, a `\t` drawn as a space.
    let drawable = |ch: char| {
        given.contains(ch)
            || (ch == ' ' && given.contains('\t'))
            || given.graphemes(true).any(|c| c.nfc().any(|n| n == ch))
    };
    for font in faces() {
        let layout = font.layout();
        let font_res = font
            .res()
            .expect("a standard 14 font is named by its dictionary");
        let make = || {
            text_with_fallback(
                &field,
                &look,
                &cut.da,
                font_res.clone(),
                &layout,
                &fallback,
                &limits,
            )
        };
        let made = make();
        let again = make();
        let glyphs = |made: &Result<crate::appearance::MadeWithFallback, Unmade>| match made {
            Ok(m) => Ok((
                m.appearance.clone(),
                m.glyphs
                    .iter()
                    .map(|(g, t)| (g, t.to_owned()))
                    .collect::<Vec<_>>(),
            )),
            Err(err) => Err(err.clone()),
        };
        assert_eq!(
            glyphs(&made),
            glyphs(&again),
            "the same appearance made twice"
        );
        let alone = text(&field, &look, &cut.da, font_res.clone(), &layout, &limits);
        let made = match made {
            Ok(made) => made,
            Err(Unmade::Unencodable(told)) => {
                match password {
                    true => assert_eq!(
                        (told.at, told.character),
                        (0, '*'),
                        "a password's characters reach no error"
                    ),
                    false => assert!(
                        given
                            .get(told.at..)
                            .is_some_and(|t| t.starts_with(told.character)),
                        "a character told at its offset in the text given"
                    ),
                }
                assert!(
                    alone.is_err(),
                    "refused with a fallback, where the font alone draws it"
                );
                continue;
            }
            Err(err) => {
                assert!(
                    !shows(&format!("{err:?} {err}"), given),
                    "an error shows the text"
                );
                continue;
            }
        };
        let needs = &made.appearance.needs.0;
        assert!(
            needs.len() <= 2
                && needs
                    .iter()
                    .all(|need| need.res == font_res || need.res == res),
            "the resources the font and the fallback"
        );
        assert_eq!(
            needs.iter().any(|need| need.res == res),
            !made.glyphs.is_empty(),
            "the fallback named exactly where glyphs of it are drawn"
        );
        for (glyph, drawn) in made.glyphs.iter() {
            let mut chars = drawn.chars();
            let ch = chars.next().expect("a glyph's text");
            assert!(chars.next().is_none(), "a glyph a character");
            assert_eq!(
                program.glyph(ch),
                Some(glyph),
                "a glyph the program's for its character"
            );
            match password {
                true => assert_eq!(ch, '*', "a password's glyph the star's alone"),
                false => assert!(drawable(ch), "a glyph for a character of the text"),
            }
        }
        let star = password.then(|| {
            let mut code = Vec::new();
            match layout.encode_char('*', &mut code) {
                true => code,
                false => program.codes("*").unwrap_or_default(),
            }
        });
        content_holds(&made.appearance.content, star.as_deref());
        assert!(
            !shows(&format!("{made:?}"), given),
            "an appearance shows the text"
        );
        if let Ok(alone) = alone
            && (composed || password)
        {
            assert_eq!(
                made.appearance, alone,
                "text the font draws whole drawn as in it alone"
            );
            assert!(made.glyphs.is_empty(), "no glyph of the fallback drawn");
        }
    }
}

/// What [`check_appearance`] makes an appearance of, cut from its input,
/// each byte missing `0`:
///
/// - bytes 0 to 3: the field's flags (`/Ff`); 4 to 7: its maximum length
///   (`/MaxLen`), where byte 12's bit 0 is set; 8 to 11: the rotation
///   (`/MK /R`), signed; each little-endian.
/// - byte 12: bit 0, a maximum length; bits 1 and 2, the quadding (`3` is
///   `0`); bits 3 to 5, the border's style (solid, dashed, beveled, inset,
///   underline; `5` to `7` solid); bits 6 and 7, how many of the three
///   dash numbers its dash array takes.
/// - byte 13: bits 0 and 1, the background's color space, and bits 2 and 3
///   the border color's (none, gray, RGB, CMYK); bit 4, the `/DA`'s size
///   replaced by number 14.
/// - byte 14: `Limits::text` is 2 to its power modulo 11 (1 to 1,024).
/// - bytes 15 to 74: 15 numbers, each an `f32` of any bits, little-endian:
///   the box's width and height, the border's width, three dash numbers,
///   four background components and four border color components (as many
///   of each as the color space takes), and a size.
/// - after them, a `/DA` string up to the first `0xFF`, and the text after
///   it up to the next `0xFF`, read as UTF-8 with what isn't replaced, and
///   after that a TrueType program, the fallback's ([`check_fallback`]).
#[cfg(feature = "text-layout")]
struct Cut<'a> {
    flags: crate::FieldFlags,
    max_len: Option<u32>,
    quadding: crate::Quadding,
    rotation: i32,
    width: f32,
    height: f32,
    border: f32,
    style: pdf_gfx::BoxBorderStyle,
    dash: Vec<f32>,
    background: Option<pdf_content_streams::Color>,
    border_color: Option<pdf_content_streams::Color>,
    cap: usize,
    da: pdf_gfx::DefaultAppearance,
    text: std::borrow::Cow<'a, str>,
    /// The fallback's program, where the input carries one.
    #[cfg_attr(not(feature = "embed"), allow(dead_code))]
    program: Option<&'a [u8]>,
}

#[cfg(feature = "text-layout")]
impl<'a> Cut<'a> {
    /// The bytes before the `/DA` string.
    const HEAD: usize = 75;

    fn of(data: &'a [u8]) -> Cut<'a> {
        use pdf_content_streams::Color;
        use pdf_gfx::{BoxBorderStyle, DefaultAppearance};

        let byte = |at: usize| data.get(at).copied().unwrap_or(0);
        let word = |at: usize| [byte(at), byte(at + 1), byte(at + 2), byte(at + 3)];
        let number = |i: usize| f32::from_le_bytes(word(15 + 4 * i));
        let color = |space: u8, first: usize| match space & 3 {
            0 => None,
            1 => Some(Color::Gray(number(first))),
            2 => Some(Color::Rgb([
                number(first),
                number(first + 1),
                number(first + 2),
            ])),
            _ => Some(Color::Cmyk([
                number(first),
                number(first + 1),
                number(first + 2),
                number(first + 3),
            ])),
        };
        let (choices, colors) = (byte(12), byte(13));

        let rest = data.get(Self::HEAD..).unwrap_or_default();
        let (da, text) = match rest.iter().position(|&b| b == 0xFF) {
            Some(at) => (&rest[..at], &rest[at + 1..]),
            None => (rest, &[][..]),
        };
        let (text, program) = match text.iter().position(|&b| b == 0xFF) {
            Some(at) => (&text[..at], Some(&text[at + 1..])),
            None => (text, None),
        };
        let mut da =
            DefaultAppearance::parse(da, &pdf_content_streams::Limits::DEFAULT).unwrap_or_default();
        if colors & 0x10 != 0 {
            da.font_size = Some(number(14));
        }
        Cut {
            flags: crate::FieldFlags::from_bits(u32::from_le_bytes(word(0))),
            max_len: (choices & 1 != 0).then(|| u32::from_le_bytes(word(4))),
            quadding: crate::Quadding::from_pdf_int(i64::from(choices >> 1 & 3)),
            rotation: i32::from_le_bytes(word(8)),
            width: number(0),
            height: number(1),
            border: number(2),
            style: match choices >> 3 & 7 {
                1 => BoxBorderStyle::Dashed,
                2 => BoxBorderStyle::Beveled,
                3 => BoxBorderStyle::Inset,
                4 => BoxBorderStyle::Underline,
                _ => BoxBorderStyle::Solid,
            },
            dash: (3..3 + usize::from(choices >> 6)).map(number).collect(),
            background: color(colors, 6),
            border_color: color(colors >> 2, 10),
            cap: 1 << (byte(14) % 11),
            da,
            text: String::from_utf8_lossy(text),
            program,
        }
    }

    fn field(&self) -> crate::appearance::TextField<'_> {
        let mut field = crate::appearance::TextField::new(&self.text);
        field.flags = self.flags;
        field.max_len = self.max_len;
        field.quadding = self.quadding;
        field
    }

    fn look(&self) -> crate::appearance::WidgetLook<'_> {
        let mut look = crate::appearance::WidgetLook::new(self.width, self.height);
        look.background = self.background;
        look.border_color = self.border_color;
        look.rotation = self.rotation;
        look.border = pdf_gfx::BoxBorder::new(self.border, self.style).with_dash(&self.dash);
        look
    }

    fn limits(&self) -> Limits {
        let mut limits = Limits::DEFAULT;
        limits.text = self.cap;
        limits.comb_cells = self.cap;
        limits
    }
}

/// The standard 14 fonts, each as a field whose `/DA` font the default
/// resources lack is drawn in, read once.
#[cfg(feature = "text-layout")]
fn faces() -> &'static [crate::appearance::FieldFont] {
    use std::sync::OnceLock;

    use crate::appearance::FieldFont;

    static FACES: OnceLock<Vec<FieldFont>> = OnceLock::new();
    FACES.get_or_init(|| {
        pdf_fonts::Std14::ALL
            .into_iter()
            .map(FieldFont::standard)
            .collect()
    })
}

/// Holds what `appearance::text` promises of `made`, made of `field` in
/// `look` at `da`'s size and color in `font`, named by `res`, under `limits`.
#[cfg(feature = "text-layout")]
fn holds<F: pdf_gfx::LayoutFont + ?Sized>(
    field: &crate::appearance::TextField<'_>,
    look: &crate::appearance::WidgetLook<'_>,
    da: &pdf_gfx::DefaultAppearance,
    limits: &Limits,
    font: &F,
    res: &pdf_content_streams::Res,
    made: &Result<crate::appearance::Appearance, crate::appearance::Unmade>,
) {
    use pdf_content_streams::{Color, ResKind};

    use crate::appearance::Unmade;
    use crate::{Cap, FieldFlags};

    let text = field.text;
    if text.len() > limits.text {
        assert_eq!(made, &Err(Unmade::Limit(Cap::Text)), "a text past its cap");
        return;
    }
    let sized = |v: f32| v.is_finite() && v > 0.0;
    let (w, h) = (look.width, look.height);
    assert_eq!(
        matches!(made, Err(Unmade::NoBox)),
        !(sized(w) && sized(h)),
        "no box exactly where it has no width or no height"
    );
    let password = field.flags.contains(FieldFlags::PASSWORD);
    let cells = field
        .max_len
        .filter(|&n| n > 0 && field.flags.contains(FieldFlags::COMB));
    let past = cells.is_some_and(|n| usize::try_from(n).map_or(true, |n| n > limits.comb_cells));
    let made = match made {
        Ok(made) => made,
        Err(Unmade::Unencodable(told)) => {
            match password {
                true => assert_eq!(
                    (told.at, told.character),
                    (0, '*'),
                    "a password's characters reach no error"
                ),
                false => assert!(
                    text.get(told.at..)
                        .is_some_and(|t| t.starts_with(told.character)),
                    "a character told at its offset in the text given"
                ),
            }
            return;
        }
        Err(err) => {
            assert!(
                match err {
                    Unmade::Limit(Cap::CombCells) => past,
                    Unmade::Limit(_) => false,
                    _ => true,
                },
                "refused for a cap not passed"
            );
            assert!(
                !shows(&format!("{err:?} {err}"), text),
                "an error shows the text"
            );
            return;
        }
    };

    // Made of finite numbers, within its caps.
    let finite = |color: Option<Color>| {
        color.is_none_or(|c| match c {
            Color::Gray(g) => g.is_finite(),
            Color::Rgb(c) => c.iter().all(|v| v.is_finite()),
            Color::Cmyk(c) => c.iter().all(|v| v.is_finite()),
        })
    };
    assert!(
        look.border.width.is_finite()
            && finite(look.background)
            && finite(look.border_color)
            && da.font_size.is_none_or(f32::is_finite)
            && finite(da.color),
        "made of a number that isn't finite"
    );
    assert!(!past, "a comb field's cells past the cap");
    assert!(made.font_size.is_finite() && made.font_size >= 0.0);

    // Its box placed on the widget's rectangle, `[0 0 w h]`, by its matrix.
    let [a, b, c, d, e, f] = made.matrix.to_array();
    let bbox = made.bbox;
    let corners = [
        (bbox.x0, bbox.y0),
        (bbox.x1, bbox.y0),
        (bbox.x0, bbox.y1),
        (bbox.x1, bbox.y1),
    ]
    .map(|(x, y)| (a * x + c * y + e, b * x + d * y + f));
    let low = corners.iter().fold((f32::INFINITY, f32::INFINITY), |m, p| {
        (m.0.min(p.0), m.1.min(p.1))
    });
    let high = corners
        .iter()
        .fold((f32::NEG_INFINITY, f32::NEG_INFINITY), |m, p| {
            (m.0.max(p.0), m.1.max(p.1))
        });
    assert_eq!(
        (low, high),
        ((0.0, 0.0), (w, h)),
        "the box placed on the rectangle"
    );

    assert!(made.needs.0.len() <= 1, "one resource at most");
    for need in &made.needs.0 {
        assert!(
            need.kind == ResKind::Font && need.res == *res,
            "the one resource the font given"
        );
    }
    let star = password.then(|| {
        let mut code = Vec::new();
        font.encode_char('*', &mut code);
        code
    });
    content_holds(&made.content, star.as_deref());
    assert!(
        !shows(&format!("{made:?}"), text),
        "an appearance shows the text"
    );
}

/// Holds what a made appearance's content promises, read as operations:
/// `q`/`Q`, `BT`/`ET`, and one `/Tx BMC`/`EMC` balanced, every number
/// finite, each string shown within the reach `text` keeps to (its place by
/// `Tm`, and one more than its bytes each the `Tf` size and `Tw` wide), and
/// where `star` is given, as for a password, each string shown the font's
/// code for `*` alone.
#[cfg(feature = "text-layout")]
fn content_holds(content: &[u8], star: Option<&[u8]>) {
    use pdf_content_streams::{Limits as ContentLimits, ResCx, operations};

    let limits = ContentLimits::DEFAULT;
    let (mut states, mut marked, mut texts, mut tagged) = (0usize, 0usize, 0usize, 0usize);
    let (mut at, mut size, mut word_space) = ((0.0f32, 0.0f32), 0.0f32, 0.0f32);
    // Operand `i` from the last, as `Operation::arg` takes them.
    let number = |operation: &pdf_content_streams::Operation<'_>, i: usize| {
        operation.arg(i).map_or(0.0, |n| n.number())
    };
    let close = |depth: &mut usize, what: &str| {
        *depth = depth
            .checked_sub(1)
            .unwrap_or_else(|| panic!("{what} closed before it's opened"));
    };
    for operation in operations(content, &ResCx::none(), &limits) {
        let operation = operation.expect("a made appearance's content reads");
        for operand in operation.operands() {
            let numbers: Vec<f32> = match operand.is_array() {
                true => operand
                    .elements()
                    .filter(|e| e.is_number())
                    .map(|e| e.number())
                    .collect(),
                false => operand
                    .is_number()
                    .then(|| operand.number())
                    .into_iter()
                    .collect(),
            };
            assert!(
                numbers.iter().all(|n| n.is_finite()),
                "a number written not finite"
            );
        }
        match operation.name() {
            b"q" => states += 1,
            b"Q" => close(&mut states, "a graphics state"),
            b"BT" => {
                texts += 1;
                at = (0.0, 0.0);
            }
            b"ET" => close(&mut texts, "a text object"),
            b"Tf" => size = number(&operation, 0),
            b"Tw" => word_space = number(&operation, 0),
            b"Tm" => at = (number(&operation, 1), number(&operation, 0)),
            b"EMC" => close(&mut marked, "a marked-content sequence"),
            b"BMC" | b"BDC" => {
                let tag = operation.arg(0).and_then(|tag| tag.name());
                assert_eq!(tag.as_deref(), Some(&b"Tx"[..]), "a sequence but /Tx");
                marked += 1;
                tagged += 1;
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                let shown = operation
                    .arg(0)
                    .and_then(|s| s.object(&limits))
                    .and_then(|s| s.as_bytes().map(<[u8]>::to_vec))
                    .expect("a made appearance's text is shown by Tj alone");
                let wide = (shown.len() as f64 + 1.0)
                    * (f64::from(size.abs()) + f64::from(word_space.abs()));
                assert!(
                    f64::from(at.0.abs()) + f64::from(at.1.abs()) + wide
                        <= crate::appearance::REACH,
                    "a string shown past the reach promised"
                );
                let Some(star) = star else { continue };
                assert!(
                    !star.is_empty()
                        && shown.len() % star.len() == 0
                        && shown.chunks(star.len()).all(|code| code == star),
                    "a password shown as anything but *"
                );
            }
            _ => {}
        }
    }
    assert_eq!(
        (states, marked, texts),
        (0, 0, 0),
        "q/Q, BMC/EMC and BT/ET balanced"
    );
    assert_eq!(tagged, 1, "one /Tx sequence");
}

/// `made`'s resources: each it needs under its name, the font, an object
/// already written by reference and a dictionary as it is.
#[cfg(feature = "text-layout")]
fn resources(made: &crate::appearance::Appearance) -> Object {
    use pdf_content_streams::ResBody;

    let mut fonts = Dict::new();
    for need in &made.needs.0 {
        let bound = match need.res.body() {
            ResBody::Written(id) => Object::from(*id),
            ResBody::Objects(objects) => objects[0].clone(),
            _ => continue,
        };
        fonts.set(need.name.clone(), bound);
    }
    let mut resources = Dict::new();
    resources.set("Font", fonts);
    Object::from(resources)
}

/// `made`'s content interpreted over `objects` and `resources`: its error
/// or its faults, where it has any.
#[cfg(feature = "text-layout")]
fn interprets<'a, R: Resolve<'a> + Streams + Copy>(
    objects: R,
    made: &crate::appearance::Appearance,
    resources: &'a Object,
) -> Result<(), String> {
    use std::ops::ControlFlow;

    use pdf_content_streams::Concat;
    use pdf_fonts::Fonts;
    use pdf_gfx::{Opts, interpret_with};

    let mut faults = Vec::new();
    let summary = interpret_with(
        objects,
        &Concat::new(&[&made.content]),
        Some(resources),
        &pdf_gfx::Limits::DEFAULT,
        &Fonts::default(),
        Opts::default(),
        &mut faults,
        |_| ControlFlow::Continue(()),
    );
    match summary {
        Ok(_) if faults.is_empty() => Ok(()),
        summary => Err(format!("{summary:?} {faults:?}")),
    }
}

/// Whether `debug` shows `text`: `text`, escaped as `Debug` escapes it,
/// found in it, where that holds a character no `Debug` without a text
/// does (one other than a letter, a digit, a space, and the punctuation of
/// numbers, lists, and structs), so a text of `0` or `bbox` isn't taken for
/// one shown.
fn shows(debug: &str, text: &str) -> bool {
    let escaped: String = text.escape_debug().collect();
    let telling = escaped
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || " {}()[]:,._-+'\"\\".contains(c)));
    telling && debug.contains(&escaped)
}

/// `objects`, read for no longer than a borrow of the caller's own, such as
/// resources made for an appearance.
#[cfg(feature = "text-layout")]
#[derive(Clone, Copy)]
struct Within<'o, R>(R, std::marker::PhantomData<&'o ()>);

#[cfg(feature = "text-layout")]
impl<'a, 'o: 'a, R: Resolve<'o>> Resolve<'a> for Within<'o, R> {
    fn resolve(&self, id: ObjectId) -> Option<&'a Object> {
        self.0.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'a Object>, Unreadable> {
        self.0.try_resolve(id)
    }
}

#[cfg(feature = "text-layout")]
impl<R: Streams> Streams for Within<'_, R> {
    fn decoded(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<std::borrow::Cow<'_, [u8]>, Unreadable> {
        self.0.decoded(id, limit)
    }
}

/// Reads each font of `form`'s default resources alike whatever the sink,
/// and makes the appearance of each widget of its text fields as the
/// module's docs list.
#[cfg(feature = "text-layout")]
fn widget_appearances<'o, R: Resolve<'o> + Streams + Copy>(
    objects: R,
    form: &Form<'o>,
    rules: Rules,
    limits: &Limits,
) {
    use crate::FieldKind;
    use crate::appearance::{Unmade, text_for_widget, widget_text};

    for name in form.font_names() {
        let quiet = form.field_font(objects, name, limits, &mut ());
        let mut faults: Vec<(Place, Fault)> = Vec::new();
        let told = form.field_font(objects, name, limits, &mut faults);
        same_font(&quiet, &told);
        let _ = format!("{faults:?}");
    }

    for field in form.fields() {
        for widget in field.widgets() {
            let quiet = widget_text(field, widget, objects, rules, None, limits, &mut ());
            if field.kind() != Some(FieldKind::Text) {
                assert!(matches!(quiet, Err(Unmade::NotText)));
                continue;
            }
            let mut told: Vec<(Place, Fault)> = Vec::new();
            let read = widget_text(field, widget, objects, rules, None, limits, &mut told);
            same_read(&quiet, &read);
            let expected = match &read {
                Ok(read) => made_in_its_font(objects, form, read, limits, &mut told),
                Err(err) => Err(err.clone()),
            };

            let mut faults: Vec<(Place, Fault)> = Vec::new();
            let made = [
                text_for_widget(form, field, widget, objects, rules, None, limits, &mut ()),
                text_for_widget(
                    form,
                    field,
                    widget,
                    objects,
                    rules,
                    None,
                    limits,
                    &mut faults,
                ),
            ];
            assert!(
                faults == told,
                "text_for_widget tells what reading the widget and its font tell"
            );
            if let Ok(read) = &read {
                let shown = format!("{read:?} {faults:?} {made:?}");
                assert!(!shows(&shown, read.field().text), "a widget's text shown");
            }
            for made in made {
                match (made, &expected) {
                    (Ok(made), Ok(appearance)) => assert!(made.appearance == *appearance),
                    (Err(err), Err(expected)) => assert!(err == *expected),
                    _ => panic!("text_for_widget makes what widget_text, field_font and make make"),
                }
            }
        }
    }
}

/// `read` made in the font its `/DA` names in `form`'s default resources
/// (Helvetica where it names none), that font read under `&mut ()` and
/// `told` alike; made twice the same, holding what `text` promises, and in
/// a font the default resources lack, interpreted with no error and no
/// fault.
#[cfg(feature = "text-layout")]
fn made_in_its_font<'o, R: Resolve<'o> + Streams + Copy>(
    objects: R,
    form: &Form<'o>,
    read: &crate::appearance::WidgetText<'_>,
    limits: &Limits,
    told: &mut Vec<(Place, Fault)>,
) -> Result<crate::appearance::Appearance, crate::appearance::Unmade> {
    use pdf_fonts::Std14;

    use crate::appearance::{FieldFont, Unmade};

    let (quiet, font) = match read.font_name() {
        Some(name) => (
            form.field_font(objects, name, limits, &mut ()),
            form.field_font(objects, name, limits, told),
        ),
        None => (
            Ok(FieldFont::standard(Std14::Helvetica)),
            Ok(FieldFont::standard(Std14::Helvetica)),
        ),
    };
    same_font(&quiet, &font);
    let font = font?;
    if let Some(name) = read.font_name() {
        let name = String::from_utf8_lossy(name);
        assert!(
            !shows(&format!("{font:?} {read:?}"), &name),
            "a font's name shown"
        );
    }
    let res = font.res().ok_or(Unmade::FontNotBindable)?;
    let layout = font.layout();
    let made = read.make(res.clone(), &layout, limits);
    assert_eq!(
        made,
        read.make(res.clone(), &layout, limits),
        "made twice the same"
    );
    holds(
        &read.field(),
        &read.look(),
        read.default_appearance(),
        limits,
        &layout,
        &res,
        &made,
    );
    if let Ok(made) = &made {
        let resources = resources(made);
        let interpreted = interprets(Within(objects, std::marker::PhantomData), made, &resources);
        if let (Err(why), true) = (interpreted, font.is_fallback()) {
            panic!("an appearance in a font the resources lack interprets with {why}");
        }
    }
    made
}

/// Holds two readings of a widget, under `&mut ()` and a `Vec` sink, alike:
/// the same text, font name, and the rest by their `Debug`, which takes two
/// numbers that aren't numbers for the same.
#[cfg(feature = "text-layout")]
fn same_read(
    quiet: &Result<crate::appearance::WidgetText<'_>, crate::appearance::Unmade>,
    told: &Result<crate::appearance::WidgetText<'_>, crate::appearance::Unmade>,
) {
    match (quiet, told) {
        (Ok(quiet), Ok(told)) => {
            assert!(
                quiet.field().text == told.field().text,
                "a sink changed the text"
            );
            assert_eq!(quiet.font_name(), told.font_name());
            assert_eq!(format!("{quiet:?}"), format!("{told:?}"));
        }
        (Err(quiet), Err(told)) => assert_eq!(quiet, told),
        _ => panic!("a sink changed whether a widget was read"),
    }
}

/// Holds a field's font read under `&mut ()` and a `Vec` sink alike: the
/// same resource, fallback or not, and the rest by its `Debug`.
#[cfg(feature = "text-layout")]
fn same_font(
    quiet: &Result<crate::appearance::FieldFont, crate::Error>,
    told: &Result<crate::appearance::FieldFont, crate::Error>,
) {
    match (quiet, told) {
        (Ok(quiet), Ok(told)) => {
            assert_eq!(quiet.res().map(|r| r.id()), told.res().map(|r| r.id()));
            assert_eq!(quiet.is_fallback(), told.is_fallback());
            assert_eq!(format!("{quiet:?}"), format!("{told:?}"));
        }
        (Err(quiet), Err(told)) => assert_eq!(quiet, told),
        _ => panic!("a sink changed whether a font was read"),
    }
}

/// Reads each field's options, selected indexes, selection, and top index,
/// and each widget's states, alike whatever the sink, and presses every
/// widget of each check box and radio button field, as the module's docs
/// list.
fn buttons_and_choices<'o, R: Resolve<'o> + Copy>(
    objects: R,
    form: &Form<'o>,
    rules: Rules,
    limits: &Limits,
) {
    use crate::FieldKind;

    for field in form.fields() {
        let quiet = field.options(objects, rules, limits);
        let mut faults: Vec<(Place, Fault)> = Vec::new();
        let told = field.options_with(objects, rules, limits, &mut faults);
        assert_eq!(quiet, told, "a sink changed a field's options");
        if let Ok(options) = &told {
            assert!(options.len() <= limits.items, "options past the cap");
            for option in options {
                let shown = format!("{option:?} {told:?}");
                assert!(!shows(&shown, &option.export), "an option shown");
                assert!(!shows(&shown, option.label()), "an option's label shown");
            }
        }

        let quiet = field.selected(objects, rules, limits);
        let told_selected = field.selected_with(objects, rules, limits, &mut faults);
        assert_eq!(quiet, told_selected, "a sink changed a field's /I");
        if let Ok(selected) = &told_selected {
            assert!(selected.len() <= limits.items, "/I past the cap");
        }

        let quiet = field.selection(objects, rules, limits);
        let selection = field.selection_with(objects, rules, limits, &mut faults);
        assert_eq!(quiet, selection, "a sink changed a field's selection");
        match (&selection, &told, &told_selected) {
            (Ok(selection), Ok(options), Ok(_)) => {
                assert!(
                    selection.windows(2).all(|w| w[0] < w[1]),
                    "a selection unsorted or repeated"
                );
                assert!(
                    selection.iter().all(|&i| i < options.len()),
                    "a selection of no option"
                );
            }
            (Ok(_), _, _) => panic!("a selection made of options or /I unread"),
            (Err(_), _, _) => assert!(told.is_err() || told_selected.is_err()),
        }
        let _ = (field.top_index(), format!("{faults:?}"));

        for widget in field.widgets() {
            let quiet = widget.states(objects, rules, limits);
            let mut faults: Vec<(Place, Fault)> = Vec::new();
            let told = widget.states_with(objects, rules, limits, &mut faults);
            assert_eq!(quiet, told, "a sink changed a widget's states");
            if let Ok(states) = &told {
                assert!(
                    states
                        .on_state
                        .as_deref()
                        .is_none_or(|n| !n.is_empty() && n != b"Off"),
                    "an on state of no name, or Off"
                );
                let shown = format!("{states:?}");
                for name in [&states.on_state, &states.state].into_iter().flatten() {
                    assert!(
                        !shows(&shown, &String::from_utf8_lossy(name)),
                        "a state shown"
                    );
                }
            }
        }

        let quiet = field.group(objects, rules, limits);
        let mut faults: Vec<(Place, Fault)> = Vec::new();
        let told = field.group_with(objects, rules, limits, &mut faults);
        let (quiet, told) = match (quiet, told) {
            (Ok(quiet), Ok(told)) => (quiet, told),
            (Err(quiet), Err(told)) => {
                assert_eq!(quiet, told);
                continue;
            }
            _ => panic!("a sink changed whether a group was read"),
        };
        assert_eq!(format!("{quiet:?}"), format!("{told:?}"));
        assert_eq!(quiet.states(), told.states());
        let group = told.group();
        let toggle = matches!(field.kind(), Some(FieldKind::CheckBox | FieldKind::Radio));
        let widgets = if toggle { field.widgets().len() } else { 0 };
        assert_eq!(
            group.widgets.len(),
            widgets,
            "a group of the field's widgets"
        );
        assert_eq!(told.states().len(), widgets, "a state for each widget");
        for index in 0..=widgets {
            let after = group.press(told.states(), index);
            holds_press(&group, told.states(), index, &after);
        }
        for widget in group.widgets {
            let shown = format!("{group:?} {widget:?} {told:?}");
            for text in [
                widget.export.as_deref(),
                widget
                    .on_state
                    .as_deref()
                    .map(|n| std::str::from_utf8(n).unwrap_or_default()),
            ]
            .into_iter()
            .flatten()
            {
                assert!(!shows(&shown, text), "a group's name or export value shown");
            }
        }
    }
}

/// The names [`check_group`] gives a widget's on state, by three bits.
const GROUP_NAMES: [Option<&[u8]>; 8] = [
    None,
    Some(b"Yes"),
    Some(b"On"),
    Some(b"1"),
    Some(b"2"),
    Some(b"spouse"),
    Some(b"Choice1"),
    Some(b"Off"),
];

/// The export values [`check_group`] gives a widget, by three bits.
const GROUP_EXPORTS: [Option<&str>; 8] = [
    None,
    Some("apple"),
    Some("pear"),
    Some(""),
    Some("Yes"),
    Some("spouse"),
    Some("1"),
    Some("caf\u{e9}"),
];

/// Cuts `data` into a check box or radio button group and presses its
/// widgets in turn, each press holding the group's rules ([`holds_press`]),
/// each byte missing `0`:
///
/// - byte 0: bit 0, radio buttons (else check boxes); bit 1, NoToggleToOff;
///   bit 2, RadiosInUnison.
/// - byte 1: the widgets, modulo 9; byte 2, their states before, a bit
///   each; byte 3: how many of those states are given, modulo 10, so fewer
///   or more than the widgets.
/// - a byte for each widget: bits 0 to 2 its on state's name, bits 3 to 5
///   its export value ([`GROUP_NAMES`], [`GROUP_EXPORTS`]).
/// - after them, up to 16 presses, each a widget's index modulo two more
///   than the widgets, so past them too.
pub fn check_group(data: &[u8]) {
    use std::borrow::Cow;

    use crate::group::{Group, GroupKind, GroupWidget};

    let byte = |at: usize| data.get(at).copied().unwrap_or(0);
    let flags = byte(0);
    let count = usize::from(byte(1) % 9);
    let widgets: Vec<GroupWidget<'_>> = (0..count)
        .map(|i| {
            let choice = byte(4 + i);
            let mut widget = GroupWidget::new(GROUP_NAMES[usize::from(choice & 7)]);
            widget.export = GROUP_EXPORTS[usize::from(choice >> 3 & 7)].map(Cow::Borrowed);
            widget
        })
        .collect();
    let kind = match flags & 1 {
        0 => GroupKind::CheckBox,
        _ => GroupKind::Radio,
    };
    let mut group = Group::new(kind, &widgets);
    group.no_toggle_to_off = flags & 2 != 0;
    group.in_unison = flags & 4 != 0;
    let given = usize::from(byte(3) % 10);
    let mut states: Vec<bool> = (0..given).map(|i| byte(2) >> (i % 8) & 1 != 0).collect();
    let presses = data.get(4 + count..).unwrap_or_default();
    for &press in presses.iter().take(16) {
        let index = usize::from(press) % (count + 2);
        let after = group.press(&states, index);
        assert_eq!(after, group.press(&states, index), "a press made twice");
        holds_press(&group, &states, index, &after);
        states = after;
    }
    let shown = format!("{group:?} {widgets:?}");
    for word in ["spouse", "Choice1", "apple", "pear", "caf"] {
        assert!(
            !shown.contains(word),
            "a group's name or export value shown"
        );
    }
}

/// Holds what the group's rules give of widget `index` pressed from
/// `before` (`after`), and of the value, the export value, and the states'
/// names after.
fn holds_press(group: &crate::group::Group<'_>, before: &[bool], index: usize, after: &[bool]) {
    use crate::group::GroupKind;

    let count = group.widgets.len();
    assert_eq!(after.len(), count, "a state for each widget");
    let was: Vec<bool> = (0..count)
        .map(|i| before.get(i).copied().unwrap_or(false))
        .collect();
    let together = |a: usize, b: usize| {
        group.on_state(a) == group.on_state(b) && group.export_value(a) == group.export_value(b)
    };
    let unison = group.is_in_unison();
    if index >= count {
        assert_eq!(after, was, "a press past the widgets changed one");
    } else if !was[index] {
        for (i, &on) in after.iter().enumerate() {
            assert_eq!(
                on,
                i == index || (unison && together(i, index)),
                "a press turning on"
            );
        }
        if !unison {
            assert_eq!(
                after.iter().filter(|&&on| on).count(),
                1,
                "out of unison, one on"
            );
        }
    } else if group.kind == GroupKind::Radio && group.no_toggle_to_off {
        assert_eq!(after, was, "NoToggleToOff, and a radio button turned off");
    } else {
        for (i, (&on, &was)) in after.iter().zip(&was).enumerate() {
            let goes = i == index || (unison && together(i, index));
            assert_eq!(on, was && !goes, "a press turning off");
        }
    }

    let first = after.iter().position(|&on| on);
    let value = group.value(after);
    match first {
        Some(first) => {
            assert_eq!(Some(value.clone()), group.on_state(first));
            assert_eq!(group.export(after), group.export_value(first));
        }
        None => {
            assert_eq!(&*value, b"Off");
            assert_eq!(group.export(after), None);
        }
    }
    assert!(group.states_for(b"Off").iter().all(|&on| !on));
    let named = group.states_for(&value);
    assert_eq!(named.len(), count);
    for (i, &on) in after.iter().enumerate() {
        let name = group.state_name(i, on);
        match on {
            true => assert_eq!(Some(name), group.on_state(i)),
            false => assert_eq!(&*name, b"Off"),
        }
        if named[i] {
            assert_eq!(
                group.on_state(i).as_deref(),
                Some(&*value),
                "states_for by another name"
            );
        }
        let export = group.export_value(i).expect("a widget's export value");
        let pressed = group
            .states_for_export(&export)
            .expect("a widget's own export value");
        let found = (0..count).position(|j| group.export_value(j).as_deref() == Some(&*export));
        assert_eq!(found.map(|j| pressed[j]), Some(true), "states_for_export");
    }
    assert_eq!(group.export_value(count), None);
    assert_eq!(group.on_state(count), None);
}

/// The objects and catalog a seed is parsed into, for the crate's tests.
#[cfg(test)]
pub(crate) fn parsed(data: &[u8]) -> (BTreeMap<ObjectId, Object>, Dict) {
    let parsed = Parsed::of(data);
    (parsed.objects, parsed.catalog)
}

/// The seeds of forms of buttons and of choices, and `check_group`'s.
#[cfg(test)]
pub(crate) const BUTTON_SEEDS: &[&[u8]] = &[SEEDS[6], SEEDS[7]];
#[cfg(test)]
pub(crate) const GROUP_SEEDS: &[&[u8]] = &[SEEDS[8], SEEDS[9], SEEDS[10], SEEDS[11]];

#[cfg(test)]
mod tests {
    use super::SEEDS;

    /// `PDF_FORMS_FUZZ_SEEDS` names a folder: writes the seed inputs there for
    /// `just fuzz forms` to begin from, with `embed` those that carry a
    /// fallback's program too ([`fallback_seeds`]).
    #[test]
    #[ignore = "needs PDF_FORMS_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_FORMS_FUZZ_SEEDS").expect("PDF_FORMS_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in SEEDS.iter().enumerate() {
            std::fs::write(dir.join(format!("seed-{i}")), seed).unwrap();
        }
        #[cfg(all(feature = "text-layout", feature = "embed"))]
        for (i, seed) in fallback_seeds().iter().enumerate() {
            std::fs::write(dir.join(format!("seed-fallback-{i}")), seed).unwrap();
        }
    }

    /// The seeds that carry a fallback's program, pdf-fixtures' Latin one,
    /// after their text: the value it draws with Helvetica, "Łukasz Nguyễn
    /// Ölçer"; the same decomposed, in comb cells; on lines at an automatic
    /// size; a password; a CJK ideograph and an emoji, refused; and a letter
    /// with an accent no font has composed, a comb cell.
    #[cfg(all(feature = "text-layout", feature = "embed"))]
    fn fallback_seeds() -> Vec<Vec<u8>> {
        use unicode_normalization::UnicodeNormalization;

        const VALUE: &str = "\u{141}ukasz Nguy\u{1ec5}n \u{d6}l\u{e7}er";
        let nfd: &'static str = VALUE.nfd().collect::<String>().leak();
        let program = pdf_fixtures::font::latin(1000).build();
        let inputs = [
            Input::new(b"/Helv 12 Tf 0 g", VALUE).with(0, &[200.0]),
            Input {
                flags: COMB,
                max_len: Some(24),
                ..Input::new(b"/Helv 0 Tf 0 g", nfd)
            }
            .with(0, &[300.0]),
            Input {
                flags: 1 << 12,
                ..Input::new(
                    b"/Helv 0 Tf 0 g",
                    "\u{141}ukasz Nguy\u{1ec5}n\n\u{d6}l\u{e7}er",
                )
            }
            .with(1, &[50.0]),
            Input {
                flags: 1 << 13,
                ..Input::new(b"/Helv 12 Tf 0 g", "\u{141}ukasz\u{4e2d}")
            },
            Input::new(b"/Helv 12 Tf 0 g", "\u{d6}l\u{e7}er \u{4e2d}\u{1f600}"),
            Input {
                flags: COMB,
                max_len: Some(3),
                ..Input::new(b"/Helv 12 Tf 0 g", "q\u{301}ab")
            },
        ];
        inputs
            .iter()
            .map(|input| {
                let mut seed = input.bytes();
                seed.push(0xFF);
                seed.extend(&program);
                seed
            })
            .collect()
    }

    /// The seeds with a fallback's program reach it, and hold what
    /// `check_fallback` holds, each cut short and with a byte flipped too:
    /// a name WinAnsiEncoding has all but two characters of is drawn in
    /// Helvetica and the program, the two Helvetica lacks the program's.
    #[cfg(all(feature = "text-layout", feature = "embed"))]
    #[test]
    fn the_fallback_seeds_reach_the_fallback() {
        use pdf_fonts::embed::Program;

        use crate::appearance::{Fallback, text_with_fallback};

        let seeds = fallback_seeds();
        for seed in &seeds {
            super::check(seed);
            for cut in (0..seed.len()).step_by(97) {
                super::check(&seed[..cut]);
                let mut flipped = seed.clone();
                flipped[cut] ^= 0x20;
                super::check(&flipped);
            }
        }
        let cut = super::Cut::of(&seeds[0]);
        let program = Program::open(cut.program.unwrap().to_vec(), 0).unwrap();
        let res = pdf_content_streams::Res::written(pdf_objects::ObjectId::new(7, 0));
        let fallback = Fallback::new(res, &program);
        let helvetica = &super::faces()[0];
        let made = text_with_fallback(
            &cut.field(),
            &cut.look(),
            &cut.da,
            helvetica.res().unwrap(),
            &helvetica.layout(),
            &fallback,
            &cut.limits(),
        )
        .unwrap();
        assert_eq!(made.glyphs.len(), 2);
        assert_eq!(made.appearance.needs.0.len(), 2);
    }

    /// What `Cut::of` cuts an input into, written as it reads it.
    #[cfg(feature = "text-layout")]
    #[derive(Clone)]
    struct Input {
        flags: u32,
        max_len: Option<u32>,
        rotation: i32,
        quadding: u8,
        /// Solid, dashed, beveled, inset, underline.
        style: u8,
        dashes: u8,
        /// None, gray, RGB, CMYK.
        background: u8,
        border_color: u8,
        size: bool,
        cap: u8,
        /// The box, the border's width, the dash numbers, the background's
        /// components, the border color's, and the size.
        numbers: [f32; 15],
        da: &'static [u8],
        text: &'static str,
    }

    #[cfg(feature = "text-layout")]
    impl Input {
        /// A single line in a box 100 by 20 with a solid border 1 wide and
        /// no colors, under a `Limits::text` of 1,024.
        fn new(da: &'static [u8], text: &'static str) -> Input {
            let mut numbers = [0.0; 15];
            numbers[..3].copy_from_slice(&[100.0, 20.0, 1.0]);
            Input {
                flags: 0,
                max_len: None,
                rotation: 0,
                quadding: 0,
                style: 0,
                dashes: 0,
                background: 0,
                border_color: 0,
                size: false,
                cap: 10,
                numbers,
                da,
                text,
            }
        }

        fn with(mut self, at: usize, numbers: &[f32]) -> Input {
            self.numbers[at..at + numbers.len()].copy_from_slice(numbers);
            self
        }

        fn bytes(&self) -> Vec<u8> {
            let choices = u8::from(self.max_len.is_some())
                | self.quadding << 1
                | self.style << 3
                | self.dashes << 6;
            let colors = self.background | self.border_color << 2 | u8::from(self.size) << 4;
            let mut out = Vec::new();
            out.extend(self.flags.to_le_bytes());
            out.extend(self.max_len.unwrap_or(0).to_le_bytes());
            out.extend(self.rotation.to_le_bytes());
            out.extend([choices, colors, self.cap]);
            out.extend(self.numbers.iter().flat_map(|n| n.to_le_bytes()));
            out.extend(self.da);
            out.push(0xFF);
            out.extend(self.text.as_bytes());
            out
        }
    }

    #[cfg(feature = "text-layout")]
    const COMB: u32 = 1 << 24;

    /// What each of the appearance's seeds is said to be, made in each of
    /// the standard 14 fonts.
    #[cfg(feature = "text-layout")]
    fn seeds_made() -> Vec<Vec<Result<crate::appearance::Appearance, crate::appearance::Unmade>>> {
        use crate::appearance::text;

        let seeds = [
            Input {
                border_color: 1,
                ..Input::new(b"/Helv 0 Tf 0 g", "Hello #1")
            },
            Input {
                flags: COMB,
                max_len: Some(5),
                rotation: 90,
                quadding: 1,
                style: 1,
                dashes: 2,
                background: 1,
                border_color: 2,
                ..Input::new(b"/Cour 12 Tf 0 0 0.502 rg", "12\u{e9}@4")
            }
            .with(3, &[3.0, 1.0, 0.0, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
            Input {
                flags: 1 << 12 | 1 << 13,
                rotation: -90,
                quadding: 2,
                style: 2,
                background: 3,
                ..Input::new(b"/TiRo 10 Tf 0 0 0 1 k", "pass word\r\nsecret#")
            }
            .with(1, &[50.0, 2.0])
            .with(6, &[0.1, 0.2, 0.3, 0.4]),
            Input {
                rotation: i32::MIN,
                style: 3,
                dashes: 3,
                background: 1,
                border_color: 1,
                ..Input::new(b"/Helv 9 Tf", "x")
            }
            .with(0, &[200.0, 30.0, 1e30, f32::NAN, f32::INFINITY, -1.0, 0.5]),
            Input {
                flags: COMB,
                max_len: Some(u32::MAX),
                rotation: 450,
                style: 4,
                border_color: 1,
                cap: 4,
                ..Input::new(b"/Helv 12 Tf", "abc")
            },
            Input::new(b"/Helv 12 Tf", "\u{e9}\u{20ac}x\u{2603}"),
            Input {
                background: 1,
                ..Input::new(b"/Helv 12 Tf", "bg")
            }
            .with(6, &[f32::NAN]),
        ];
        let written = &SEEDS[SEEDS.len() - seeds.len()..];
        for (input, seed) in seeds.iter().zip(written) {
            assert_eq!(input.bytes(), *seed, "a seed isn't what it's said to be");
        }
        written
            .iter()
            .map(|seed| {
                let cut = super::Cut::of(seed);
                let (field, look, limits) = (cut.field(), cut.look(), cut.limits());
                super::faces()
                    .iter()
                    .map(|font| {
                        text(
                            &field,
                            &look,
                            &cut.da,
                            font.res().unwrap(),
                            &font.layout(),
                            &limits,
                        )
                    })
                    .collect()
            })
            .collect()
    }

    #[cfg(feature = "text-layout")]
    #[test]
    fn the_appearance_seeds_are_what_they_are_said_to_be() {
        use crate::Cap;
        use crate::appearance::Unmade;

        let made = seeds_made();
        // The twelve Latin faces, which draw what the seeds hold but the
        // snowman.
        let latin = |seed: &[Result<_, Unmade>]| seed[..12].to_vec();
        let strings = |content: &[u8]| String::from_utf8_lossy(content).into_owned();
        for seed in &made[..4] {
            assert!(latin(seed).iter().all(Result::is_ok));
        }
        let line = made[0][0].as_ref().unwrap();
        assert!(strings(&line.content).contains("(Hello #1) Tj"));
        let comb = made[1][0].as_ref().unwrap();
        assert_eq!((comb.bbox.x1, comb.bbox.y1), (20.0, 100.0));
        assert_eq!(strings(&comb.content).matches("[3 1] 0 d").count(), 2);
        // Seven of Courier's stars at 10 points fill the 42 points the box
        // gives, and their sum in `f32` passes it, as PDFium sums it: six to
        // a line.
        let password = made[2][0].as_ref().unwrap();
        assert!(strings(&password.content).contains("(******) Tj"));
        assert!(!strings(&password.content).contains("(*******"));
        assert!(!strings(&password.content).contains("secret"));
        let inset = made[3][0].as_ref().unwrap();
        assert!(strings(&inset.content).contains("(x) Tj"));
        assert!(
            made[4]
                .iter()
                .all(|m| matches!(m, Err(Unmade::Limit(Cap::Text | Cap::CombCells))))
        );
        for unencodable in latin(&made[5]) {
            let Err(Unmade::Unencodable(told)) = unencodable else {
                panic!("the snowman is drawn")
            };
            assert_eq!((told.at, told.character), (6, '\u{2603}'));
        }
        assert!(made[6].iter().all(|m| *m == Err(Unmade::NotFinite)));
    }

    /// The text fields of the seeds' forms, each widget made by
    /// `text_for_widget`: whether it's made, in a fallback font, or why not.
    #[cfg(feature = "text-layout")]
    #[test]
    fn the_form_seeds_reach_their_widgets_appearances() {
        use pdf_objects::pdf_tokenizer::Rules;

        use crate::appearance::{Unmade, text_for_widget};
        use crate::{FieldKind, Form, Limits};

        let made = |seed: &[u8]| {
            let parsed = super::Parsed::of(seed);
            let objects = parsed.objects();
            let limits = Limits::DEFAULT;
            let form = Form::read(objects, &parsed.catalog, Rules::Pdfium, &limits)
                .unwrap()
                .unwrap();
            let mut made = Vec::new();
            for field in form.fields() {
                assert_eq!(field.kind(), Some(FieldKind::Text));
                for widget in field.widgets() {
                    let mut faults = Vec::new();
                    let widget = text_for_widget(
                        &form,
                        field,
                        widget,
                        objects,
                        Rules::Pdfium,
                        None,
                        &limits,
                        &mut faults,
                    );
                    made.push(widget.map(|made| (made.font.is_fallback(), faults.len())));
                }
            }
            made
        };
        // The name, the comb password in Times-Roman, the stream, and the
        // password from `/Border`: each in Helvetica but the comb, told so.
        assert_eq!(
            made(SEEDS[4]),
            [
                Ok((false, 0)),
                Ok((true, 1)),
                Ok((false, 0)),
                Ok((false, 0))
            ]
        );
        // No `/DA`, told; and a font no resource can name.
        assert_eq!(
            made(SEEDS[5]),
            [Ok((true, 1)), Err(Unmade::FontNotBindable)]
        );
    }

    /// Drives `check_appearance` to each edge its numbers reach: every style
    /// with a border color and without, at quarter turns, negative, odd and
    /// extreme rotations; dash arrays that can't be used and border widths
    /// up to past finite, comb and not; `/DA` sizes and boxes so large the
    /// text reaches past what can be placed, and two inputs the target
    /// stopped on there; and comb fields with more cells than a small
    /// `Limits::text`.
    #[cfg(feature = "text-layout")]
    #[test]
    fn appearances_hold_at_every_edge() {
        use crate::Cap;
        use crate::appearance::{Unmade, text};

        let base = Input {
            border_color: 1,
            ..Input::new(b"/Helv 0 Tf 0 g", "a#b c\u{e9}")
        };
        let mut inputs = Vec::new();
        for rotation in [
            0,
            90,
            180,
            270,
            360,
            450,
            -90,
            -180,
            -270,
            45,
            -1,
            i32::MIN,
            i32::MAX,
            i32::MIN + 8,
        ] {
            for style in 0..5 {
                for border_color in [0, 1] {
                    inputs.push(Input {
                        rotation,
                        style,
                        border_color,
                        background: 2,
                        ..base.clone()
                    });
                }
            }
        }
        for flags in [0, COMB] {
            for style in [0, 1] {
                for dash in [[f32::NAN, f32::INFINITY, -1.0], [3.0, 1.0, 0.0], [0.0; 3]] {
                    for width in [
                        0.0,
                        -1.0,
                        1.0,
                        2.5,
                        30.0,
                        1e30,
                        f32::MAX,
                        f32::INFINITY,
                        f32::NAN,
                    ] {
                        let input = Input {
                            flags,
                            max_len: Some(3),
                            style,
                            dashes: 3,
                            ..base.clone()
                        };
                        inputs.push(input.with(2, &[width]).with(3, &dash));
                    }
                }
            }
        }
        // A single-line password whose line break is dropped before its
        // first character, which ZapfDingbats has no `*` for.
        inputs.push(Input {
            flags: 1 << 13,
            ..Input::new(b"/Helv 0 Tf 0 g", "\r\nsecret")
        });
        // `/DA` sizes from huge but within reach, through those whose
        // strings reach past it with every number finite (from about `3e35`
        // a width times the size passes `f32::MAX` in the interpreter), to
        // past finite, for a single line, multiline, comb and password field.
        for size in [
            1e20,
            1e33,
            1e34,
            6e35,
            8e35,
            1e36,
            5.243_934_6e36,
            3e37,
            f32::MAX,
            f32::INFINITY,
            f32::NAN,
            -1.0,
            0.0,
        ] {
            for flags in [0, 1 << 12, COMB, 1 << 13] {
                let input = Input {
                    flags,
                    max_len: Some(3),
                    size: true,
                    ..base.clone()
                };
                inputs.push(input.with(14, &[size]));
            }
        }
        // Boxes so wide or tall that text set by its quadding, or a line's
        // middle, is far from the box's origin.
        for (w, h) in [
            (1e30, 20.0),
            (1e36, 20.0),
            (3e37, 20.0),
            (f32::MAX, 20.0),
            (100.0, 1e38),
            (100.0, f32::MAX),
            (1e38, 1e38),
        ] {
            for quadding in 0..3 {
                for flags in [0, 1 << 12, COMB] {
                    let input = Input {
                        flags,
                        max_len: Some(3),
                        quadding,
                        ..base.clone()
                    };
                    inputs.push(input.with(0, &[w, h]));
                }
            }
        }
        for max_len in [0, 1, 16, 17, 1 << 20, u32::MAX] {
            for text in ["", "abc", "abcdefghijklmnop", "abcdefghijklmnopq"] {
                inputs.push(Input {
                    flags: COMB,
                    max_len: Some(max_len),
                    cap: 4,
                    text,
                    ..base.clone()
                });
            }
        }
        for input in &inputs {
            super::check_appearance(&input.bytes());
        }
        // Two inputs the target stopped on: each cut into a password at a
        // `/DA` size of `5.2439346e36`, whose stars, every number written
        // finite, the interpreter refused to place.
        for found in [
            &b"<< /Acro 0 R << /T 3 0 R] /DA (/Hel /Tw\x00lds [A\xfe\xff\xff\xff1 0\"R 3 0 R]|||||||||||||||||||\xfc|||||||/Tw( >>\xff<< /T (kid)/Ff /Pv 0 Tf 0 g)a"[..],
            &b"<< /AcroForm << /Fields [1 0 R ||||||||||||||||||||||||||||%PDF-|||||||||||||||||||||||||||||||rect) /FT ) /Kids [/T (c) /Kids [4 0 R] >>\xff<< //Ch /V [T ("[..],
        ] {
            super::check(found);
        }

        // A comb field of more cells than `Limits::comb_cells` is refused,
        // as is a text longer than `Limits::text`.
        let font = &super::faces()[0];
        for input in &inputs[inputs.len() - 24..] {
            let bytes = input.bytes();
            let cut = super::Cut::of(&bytes);
            let (field, look, limits) = (cut.field(), cut.look(), cut.limits());
            let made = text(
                &field,
                &look,
                &cut.da,
                font.res().unwrap(),
                &font.layout(),
                &limits,
            );
            let past = input.max_len.is_some_and(|n| n > 16) || input.text.len() > 16;
            assert_eq!(
                matches!(made, Err(Unmade::Limit(Cap::Text | Cap::CombCells))),
                past
            );
        }
    }
}
