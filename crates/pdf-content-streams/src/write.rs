//! Writing content (ISO 32000-2, 7.8.2): operators with their operands
//! written the same way every time, and the resources the content draws
//! with named only when it's finished (7.8.3). [`ContentWriter`] has the
//! rules.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;

use pdf_objects::{Data, Dict, Object, Resolve, Value, dict, serialize};

use crate::names::ResKind;
use crate::operator::{FillRule, Paint};
use crate::res::{Res, ResId};

/// A color in a device color space, each component from 0 to 1, as the
/// color operators set it (8.6.8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    /// A gray level: `g`, `G`.
    Gray(f32),
    /// Red, green and blue: `rg`, `RG`.
    Rgb([f32; 3]),
    /// Cyan, magenta, yellow and black: `k`, `K`.
    Cmyk([f32; 4]),
}

impl Color {
    /// An RGB color from components of 0 to 255.
    pub fn rgb8(c: [u8; 3]) -> Color {
        Color::Rgb(c.map(|v| f32::from(v) / 255.0))
    }
}

/// A blend mode, an ExtGState's `/BM` (11.3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    /// `Normal`: the source alone.
    Normal,
    /// `Multiply`.
    Multiply,
    /// `Screen`.
    Screen,
    /// `Overlay`.
    Overlay,
    /// `Darken`.
    Darken,
    /// `Lighten`.
    Lighten,
    /// `ColorDodge`.
    ColorDodge,
    /// `ColorBurn`.
    ColorBurn,
    /// `HardLight`.
    HardLight,
    /// `SoftLight`.
    SoftLight,
    /// `Difference`.
    Difference,
    /// `Exclusion`.
    Exclusion,
    /// `Hue`.
    Hue,
    /// `Saturation`.
    Saturation,
    /// `Color`.
    Color,
    /// `Luminosity`.
    Luminosity,
}

impl Blend {
    /// Its name, as `/BM` names it.
    fn name(self) -> &'static str {
        match self {
            Blend::Normal => "Normal",
            Blend::Multiply => "Multiply",
            Blend::Screen => "Screen",
            Blend::Overlay => "Overlay",
            Blend::Darken => "Darken",
            Blend::Lighten => "Lighten",
            Blend::ColorDodge => "ColorDodge",
            Blend::ColorBurn => "ColorBurn",
            Blend::HardLight => "HardLight",
            Blend::SoftLight => "SoftLight",
            Blend::Difference => "Difference",
            Blend::Exclusion => "Exclusion",
            Blend::Hue => "Hue",
            Blend::Saturation => "Saturation",
            Blend::Color => "Color",
            Blend::Luminosity => "Luminosity",
        }
    }
}

/// The graphics state an ExtGState sets (8.4.5): opacity and blending,
/// each set only where it's given.
///
/// An alpha is written from 0 to 1, one outside that range clamped to it,
/// and one that isn't finite, NaN or an infinity, written 0, as the
/// writer's numbers are ([`write_number`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GState {
    /// `/ca`, the alpha of what's filled, 0 to 1.
    pub fill_alpha: Option<f32>,
    /// `/CA`, the alpha of what's stroked, 0 to 1.
    pub stroke_alpha: Option<f32>,
    /// `/BM`, the blend mode.
    pub blend: Option<Blend>,
}

impl GState {
    /// It as an ExtGState resource.
    pub(crate) fn res(self) -> Res {
        let mut dict = dict! { "Type" => "ExtGState" };
        if let Some(a) = self.fill_alpha {
            dict.set("ca", alpha(a));
        }
        if let Some(a) = self.stroke_alpha {
            dict.set("CA", alpha(a));
        }
        if let Some(b) = self.blend {
            dict.set("BM", b.name());
        }
        Res::dict(dict)
    }
}

/// An alpha as written: from 0 to 1, and 0 for one that isn't finite.
fn alpha(a: f32) -> f32 {
    if a.is_finite() {
        a.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// How a line's ends are drawn, `J`'s operand (8.4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    /// 0: squared off at the end.
    Butt,
    /// 1: a half circle past the end.
    Round,
    /// 2: a half square past the end.
    Square,
}

/// How a path's corners are drawn, `j`'s operand (8.4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoin {
    /// 0: the edges extended to meet.
    Miter,
    /// 1: a circle at the corner.
    Round,
    /// 2: the corner cut off.
    Bevel,
}

/// How text is drawn, `Tr`'s operand, the text rendering mode (9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextMode {
    /// 0: filled.
    Fill,
    /// 1: stroked.
    Stroke,
    /// 2: filled, then stroked.
    FillStroke,
    /// 3: neither: text to find and select, as recognized text is.
    Invisible,
    /// 4: filled, and added to the clip.
    FillClip,
    /// 5: stroked, and added to the clip.
    StrokeClip,
    /// 6: filled, then stroked, and added to the clip.
    FillStrokeClip,
    /// 7: added to the clip alone.
    Clip,
}

impl TextMode {
    /// The mode `n Tr` sets; none for a number that's none.
    pub fn of(n: i32) -> Option<TextMode> {
        Some(match n {
            0 => TextMode::Fill,
            1 => TextMode::Stroke,
            2 => TextMode::FillStroke,
            3 => TextMode::Invisible,
            4 => TextMode::FillClip,
            5 => TextMode::StrokeClip,
            6 => TextMode::FillStrokeClip,
            7 => TextMode::Clip,
            _ => return None,
        })
    }

    /// Whether text shown in it is added to the clip (4 to 7).
    pub fn clips(self) -> bool {
        matches!(
            self,
            TextMode::FillClip | TextMode::StrokeClip | TextMode::FillStrokeClip | TextMode::Clip
        )
    }
}

/// The resources content draws with, each named: what the resources
/// dictionary it goes with must bind. In the order the content first
/// draws with them, one entry a resource.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResourceNeeds(pub Vec<Need>);

/// A resource content draws with, under its name in its category.
#[derive(Debug, Clone, PartialEq)]
pub struct Need {
    /// Its category: the key of the resources dictionary it goes under.
    pub kind: ResKind,
    /// Its name, without its `/`.
    pub name: Vec<u8>,
    /// The resource.
    pub res: Res,
}

/// The names a resources dictionary already binds, which
/// [`ContentWriter::finish`] keeps clear of: each with what it binds, as
/// far as that can be told by what it is (an object that reaches no other
/// and holds each stream's data in memory) or by number (a reference). A name bound to the very resource
/// being named is used again, not bumped, so content written again for
/// the same resources gets the same names.
#[derive(Debug, Clone, Default)]
pub struct Reserved {
    names: BTreeMap<(ResKind, Vec<u8>), Vec<ResId>>,
}

/// How many references in a row [`Reserved::read`] follows to find what a
/// resources dictionary, a category or a name binds.
const HOPS: usize = 8;

impl Reserved {
    /// The names `resources`, a resources dictionary, binds in each
    /// category, read through `objects`: the dictionary, or a category of
    /// it, given by reference is followed, as are references to
    /// references, up to eight in a row; a stream is read as its
    /// dictionary, as pdf-objects reads one.
    ///
    /// A name bound by reference binds that object, as one written already
    /// ([`Res::written`]); and, where what the name binds, followed so,
    /// refers to no other object and holds each stream's data in memory, it
    /// binds a resource of that object ([`Res::of`]) too, told by its digest.
    /// One that holds a stream's data where a reader found it, or none, is
    /// told by its reference alone: a place tells apart the streams of one
    /// reading, not of two ([`ResId`]), so a page's image is never taken
    /// for another file's at the same place.
    pub fn read<'o>(objects: &impl Resolve<'o>, resources: Option<&'o Object>) -> Reserved {
        let mut names = BTreeMap::new();
        let resources = resources
            .and_then(|r| direct(objects, r))
            .and_then(Object::as_dict);
        for &kind in ResKind::ALL {
            let Some(category) = resources
                .and_then(|r| direct(objects, r.get(kind.key())?))
                .and_then(Object::as_dict)
            else {
                continue;
            };
            for (name, bound) in category.iter() {
                let mut ids = Vec::new();
                if let Value::Ref(id) = bound.value() {
                    ids.push(Res::written(*id).id);
                }
                if let Some(resolved) = direct(objects, bound).filter(|o| told_by_content(o)) {
                    ids.push(ResId::of(std::slice::from_ref(resolved)));
                }
                names.insert((kind, name.to_vec()), ids);
            }
        }
        Reserved { names }
    }

    /// The names `needs` gives, kept too: content written after with
    /// these resources goes with it on one page.
    pub fn take(&mut self, needs: &ResourceNeeds) {
        for need in &needs.0 {
            let ids = self
                .names
                .entry((need.kind, need.name.clone()))
                .or_default();
            if !ids.contains(&need.res.id) {
                ids.push(need.res.id);
            }
        }
    }

    /// `name` in `kind` bound to nothing: a name the resources go without
    /// once what used it is taken out, free to name anything again.
    pub fn forget(&mut self, kind: ResKind, name: &[u8]) {
        self.names.remove(&(kind, name.to_vec()));
    }

    /// Every name it keeps, each in its category.
    pub fn names(&self) -> impl Iterator<Item = (ResKind, &[u8])> {
        (self.names.keys()).map(|(kind, name)| (*kind, name.as_slice()))
    }

    /// Whether `name` may name `id` in `kind`.
    fn allows(&self, kind: ResKind, name: &[u8], id: ResId) -> bool {
        self.names
            .get(&(kind, name.to_vec()))
            .is_none_or(|bound| bound.contains(&id))
    }
}

/// `object`, or what the references from it lead to, followed through
/// `objects`, at most [`HOPS`] in a row; none for a reference to no
/// object, or past that.
fn direct<'o>(objects: &impl Resolve<'o>, mut object: &'o Object) -> Option<&'o Object> {
    for _ in 0..HOPS {
        match object.value() {
            Value::Ref(id) => object = objects.resolve(*id)?,
            _ => return Some(object),
        }
    }
    (!matches!(object.value(), Value::Ref(_))).then_some(object)
}

/// Whether `object` is told by what it is: it refers to no other object,
/// and each stream in it holds its data in memory.
fn told_by_content(object: &Object) -> bool {
    match object.value() {
        Value::Ref(_) => false,
        Value::Array(items) => items.iter().all(told_by_content),
        Value::Dict(dict) => dict.iter().all(|(_, o)| told_by_content(o)),
        Value::Stream(stream) => {
            matches!(stream.data, Data::Bytes(_))
                && stream.dict.iter().all(|(_, o)| told_by_content(o))
        }
        _ => true,
    }
}

/// Content being written: operators with their operands, each operation a
/// line of its own, and the resources it draws with ([`Res`]) named only
/// when it's finished ([`finish`](ContentWriter::finish)), so the names
/// fit the resources dictionary it goes with ([`Reserved`]). The same
/// calls give the same bytes and names every time, on any thread.
///
/// - **Numbers** ([`write_number`]): at most four decimals, trailing
///   zeros and a bare point trimmed, never an exponent or `-0`, and 0 for
///   NaN and the infinities (ISO 32000-2, 7.3.3).
/// - **Strings** ([`write_string`]): literal, `(`, `)` and `\` escaped,
///   unless a byte is a control code (below 0x20, or 0x7F), when the whole
///   string is written in hex (7.3.4), so a CR, which PDFium reads in a
///   literal string as written and ISO 32000-2 as LF, is never written
///   raw.
/// - **Names**, a tag's among them, as pdf-objects writes them
///   ([`serialize::name`]): `#` and two hex digits for `#`, white-space, a
///   delimiter or a byte outside `!` to `~` (7.3.5), 0x80 and 0xFF among
///   them, which PDFium reads as white-space.
/// - **Resources named at the end** (7.8.3): each resource drawn is one
///   need ([`ResourceNeeds`]), in the order first drawn, however often it's
///   drawn. Its name is `Sh` and six base32 characters of the top 30 bits
///   of its digest ([`ResId`]), counted up past a name the resources
///   dictionary binds to another resource, or one this content named
///   another with already. A name it binds to the very resource is used
///   again, so content written again for the same resources gets the
///   same names.
///
/// It refuses nothing: what PDFium would read otherwise is the caller's
/// to keep from it, a name past 255 bytes, its `/` among them, of which
/// PDFium reads the first 255, and a string past 32,767 bytes, of which
/// it reads the first 32,767. Each call's operator and operands read back
/// through [`operations`](crate::operations) as written: a number as
/// [`Operand::number`](crate::Operand::number) reads it, within its
/// rounding, but a whole number PDFium holds no integer of, 2^32 or more,
/// or below −2^31, which it reads as 0; a name as [`name`](crate::name)
/// reads it; a string as [`object`](crate::object) reads it.
///
/// ```
/// use pdf_content_streams::pdf_objects::dict;
/// use pdf_content_streams::{ContentWriter, Paint, Res, ResKind, Reserved};
///
/// let mut w = ContentWriter::new();
/// w.save();
/// w.fill_color(pdf_content_streams::Color::Gray(0.5));
/// w.rect(10.0, 10.0, 100.0 / 3.0, 50.0);
/// w.paint(Paint::Fill);
/// w.begin_text();
/// w.font(Res::dict(dict! { "Type" => "Font", "Subtype" => "Type1" }), 12.0);
/// w.show(b"(Hi)");
/// w.end_text();
/// w.restore();
/// let (content, needs) = w.finish(&Reserved::default());
/// let font = &needs.0[0];
/// assert_eq!(font.kind, ResKind::Font);
/// let name = String::from_utf8_lossy(&font.name);
/// let want = format!(
///     "q\n0.5 g\n10 10 33.3333 50 re\nf\nBT\n/{name} 12 Tf\n(\\(Hi\\)) Tj\nET\nQ\n"
/// );
/// assert_eq!(content, want.as_bytes());
/// ```
#[derive(Debug, Clone, Default)]
pub struct ContentWriter {
    out: Vec<u8>,
    /// Where a resource's name goes in `out`, in order: just past its
    /// `/`, and which of `needs` it names.
    slots: Vec<(usize, usize)>,
    needs: Vec<(ResKind, Res)>,
}

impl ContentWriter {
    /// A writer of no content yet.
    pub fn new() -> ContentWriter {
        ContentWriter::default()
    }

    /// `q`: the graphics state saved (8.4.4).
    pub fn save(&mut self) {
        self.op(&[], "q");
    }

    /// `Q`: the graphics state the last `q` saved restored.
    pub fn restore(&mut self) {
        self.op(&[], "Q");
    }

    /// `cm`: the current matrix multiplied by `m`, `a b c d e f`.
    pub fn transform(&mut self, m: [f32; 6]) {
        self.op(&m, "cm");
    }

    /// `g`, `rg` or `k`: the color filled with (8.6.8).
    pub fn fill_color(&mut self, color: Color) {
        self.color(color, false);
    }

    /// `G`, `RG` or `K`: the color stroked with.
    pub fn stroke_color(&mut self, color: Color) {
        self.color(color, true);
    }

    fn color(&mut self, color: Color, stroke: bool) {
        let (values, fill_op, stroke_op): (&[f32], _, _) = match &color {
            Color::Gray(v) => (std::slice::from_ref(v), "g", "G"),
            Color::Rgb(v) => (v, "rg", "RG"),
            Color::Cmyk(v) => (v, "k", "K"),
        };
        self.op(values, if stroke { stroke_op } else { fill_op });
    }

    /// `gs`, with `state` an ExtGState resource, named at the end.
    pub fn gstate(&mut self, state: GState) {
        self.name(ResKind::ExtGState, state.res());
        self.out.extend_from_slice(b" gs\n");
    }

    /// `w`: the line width.
    pub fn line_width(&mut self, width: f32) {
        self.op(&[width], "w");
    }

    /// `d`: `dashes` on and off in turn, from `phase` into them; none for
    /// a solid line.
    pub fn dash(&mut self, dashes: &[f32], phase: f32) {
        self.out.push(b'[');
        for (i, &d) in dashes.iter().enumerate() {
            if i > 0 {
                self.out.push(b' ');
            }
            write_number(&mut self.out, d);
        }
        self.out.extend_from_slice(b"] ");
        self.op(&[phase], "d");
    }

    /// `J`: the line cap.
    pub fn line_cap(&mut self, cap: LineCap) {
        self.op(&[cap as u8 as f32], "J");
    }

    /// `j`: the line join.
    pub fn line_join(&mut self, join: LineJoin) {
        self.op(&[join as u8 as f32], "j");
    }

    /// `m`: a new subpath from (`x`, `y`) (8.5.2).
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.op(&[x, y], "m");
    }

    /// `l`: a line to (`x`, `y`).
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.op(&[x, y], "l");
    }

    /// `c`: a Bézier curve through control points (`x1`, `y1`) and (`x2`,
    /// `y2`) to (`x3`, `y3`).
    pub fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) {
        self.op(&[x1, y1, x2, y2, x3, y3], "c");
    }

    /// `re`: a rectangle from (`x`, `y`), `w` wide and `h` high.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.op(&[x, y, w, h], "re");
    }

    /// `h`: the subpath closed.
    pub fn close(&mut self) {
        self.op(&[], "h");
    }

    /// The path painted, as `paint` says, and ended (8.5.3): `f` for a
    /// fill, never its old synonym `F`.
    pub fn paint(&mut self, paint: Paint) {
        self.op(&[], paint_op(paint));
    }

    /// `W` or `W*`: the path clips what follows, once it's ended (8.5.4).
    pub fn clip(&mut self, rule: FillRule) {
        let op = match rule {
            FillRule::NonZero => "W",
            FillRule::EvenOdd => "W*",
        };
        self.op(&[], op);
    }

    /// `BT`: a text object begun (9.4.1).
    pub fn begin_text(&mut self) {
        self.op(&[], "BT");
    }

    /// `ET`: the text object ended.
    pub fn end_text(&mut self) {
        self.op(&[], "ET");
    }

    /// `Tf`: text drawn in `font` at `size` (9.3), the font's resource
    /// named at the end. What's shown in it ([`show`](Self::show)) is the
    /// caller's to encode as the font does.
    pub fn font(&mut self, font: Res, size: f32) {
        self.name(ResKind::Font, font);
        self.out.push(b' ');
        self.op(&[size], "Tf");
    }

    /// `Tm`: the text matrix and the text line matrix set to `m`, `a b c d
    /// e f` (9.4.2).
    pub fn text_matrix(&mut self, m: [f32; 6]) {
        self.op(&m, "Tm");
    }

    /// `Td`: the next line, (`x`, `y`) from the start of this one.
    pub fn text_move(&mut self, x: f32, y: f32) {
        self.op(&[x, y], "Td");
    }

    /// `T*`: the next line, by the leading.
    pub fn next_line(&mut self) {
        self.op(&[], "T*");
    }

    /// `Tr`: the text rendering mode.
    pub fn text_mode(&mut self, mode: TextMode) {
        self.op(&[mode as u8 as f32], "Tr");
    }

    /// `Tz`: the horizontal scale, in percent.
    pub fn horizontal_scale(&mut self, percent: f32) {
        self.op(&[percent], "Tz");
    }

    /// `TL`: the leading.
    pub fn leading(&mut self, leading: f32) {
        self.op(&[leading], "TL");
    }

    /// `Tj`: `codes`, a string of the codes the font [`font`](Self::font)
    /// set encodes text as, shown (9.4.3), written as [`write_string`]
    /// writes them, every byte: PDFium reads a string's first 32,767.
    pub fn show(&mut self, codes: &[u8]) {
        write_string(&mut self.out, codes);
        self.out.extend_from_slice(b" Tj\n");
    }

    /// `Do`: an external object, an image or a form, drawn as the current
    /// matrix places it, its resource named at the end.
    pub fn draw(&mut self, xobject: Res) {
        self.name(ResKind::XObject, xobject);
        self.out.extend_from_slice(b" Do\n");
    }

    /// Image `image` drawn to fill the box from (`x`, `y`), `w` wide and
    /// `h` high: `q`, the matrix that maps the unit square onto it, `Do`
    /// and `Q`.
    pub fn image(&mut self, image: Res, x: f32, y: f32, w: f32, h: f32) {
        self.save();
        self.transform([w, 0.0, 0.0, h, x, y]);
        self.draw(image);
        self.restore();
    }

    /// `BMC`: content tagged `tag`, with no property list (14.6). Its name
    /// is written as every name is, every byte: PDFium reads a name's
    /// first 255, its `/` among them.
    pub fn begin_tagged(&mut self, tag: &str) {
        serialize::name(&mut self.out, tag.as_bytes());
        self.out.extend_from_slice(b" BMC\n");
    }

    /// `BDC`: content tagged `tag`, with property list `props`, named in
    /// the resources' `/Properties` at the end (14.6.2): lists are never
    /// written inline.
    pub fn begin_marked(&mut self, tag: &str, props: Dict) {
        serialize::name(&mut self.out, tag.as_bytes());
        self.out.push(b' ');
        self.name(ResKind::Properties, Res::dict(props));
        self.out.extend_from_slice(b" BDC\n");
    }

    /// `EMC`: the marked content last begun ended.
    pub fn end_marked(&mut self) {
        self.op(&[], "EMC");
    }

    /// Operator `op` as content read gives it, `operands` before it, each
    /// as written, a line of its own: content written again. An inline
    /// image is `op`, its token whole, `BI` to `EI`. What it does isn't
    /// followed: the names it gives are the resources' already, none of
    /// [`ResourceNeeds`].
    ///
    /// Each operation of content read ([`operations`](crate::operations))
    /// so written, its operands' and operator's bytes, gives content that
    /// reads as the same tokens, white-space and comments aside, and so as
    /// the same operations; but where its reading looked past a token's
    /// end, at what isn't written again. An inline image PDFium keeps no
    /// stream of ([`ImageStream::Missing`](crate::ImageStream::Missing))
    /// may have: one whose filter's end was found past its `EI`, in a
    /// comment, say, may run on, written again, to a later `EI`. And where
    /// a reading decodes DCT and CCITTFax data to find its end
    /// ([`CodedEnds::Decoded`](crate::CodedEnds)), an image of either may
    /// have, its decoder reading on past its `EI` before it fails, as
    /// PDFium's does: written again, with other bytes after it, it may be
    /// decoded through them, and PDFium then keep no stream of it. Operands
    /// at the end that no operator takes are no operation, and aren't
    /// written.
    pub fn operation<'o>(&mut self, operands: impl IntoIterator<Item = &'o [u8]>, op: &[u8]) {
        for operand in operands {
            self.out.extend_from_slice(operand);
            self.out.push(b' ');
        }
        self.out.extend_from_slice(op);
        self.out.push(b'\n');
    }

    /// The content, and the resources it draws with, named: each `Sh` and
    /// its digest's first six base32 characters, the next name up where
    /// `reserved`, or a name made before, has it.
    pub fn finish(self, reserved: &Reserved) -> (Vec<u8>, ResourceNeeds) {
        self.named(reserved)
    }

    /// The content as [`finish`](Self::finish) gives it, the writer kept:
    /// what's written once can be named for each page it goes on.
    pub fn named(&self, reserved: &Reserved) -> (Vec<u8>, ResourceNeeds) {
        let mut taken = BTreeSet::new();
        let needs: Vec<Need> = self
            .needs
            .iter()
            .map(|(kind, res)| {
                let (kind, res) = (*kind, res.clone());
                let name = fresh(kind, res.id, reserved, &taken);
                taken.insert((kind, name.clone()));
                Need { kind, name, res }
            })
            .collect();
        let mut out = Vec::with_capacity(self.out.len() + 8 * self.slots.len());
        let mut at = 0;
        for &(slot, need) in &self.slots {
            out.extend_from_slice(&self.out[at..slot]);
            out.extend_from_slice(&needs[need].name);
            at = slot;
        }
        out.extend_from_slice(&self.out[at..]);
        (out, ResourceNeeds(needs))
    }

    /// `operands` and `op`, a line of its own.
    fn op(&mut self, operands: &[f32], op: &str) {
        for &n in operands {
            write_number(&mut self.out, n);
            self.out.push(b' ');
        }
        self.out.extend_from_slice(op.as_bytes());
        self.out.push(b'\n');
    }

    /// `/` and the place `res`'s name goes, once it has one.
    fn name(&mut self, kind: ResKind, res: Res) {
        let need = match self.needs.iter().position(|(_, r)| r.id == res.id) {
            Some(i) => i,
            None => {
                self.needs.push((kind, res));
                self.needs.len() - 1
            }
        };
        self.out.push(b'/');
        self.slots.push((self.out.len(), need));
    }
}

/// The operator that paints a path as `paint` says.
fn paint_op(paint: Paint) -> &'static str {
    match paint {
        Paint::Fill => "f",
        Paint::FillEvenOdd => "f*",
        Paint::Stroke => "S",
        Paint::CloseStroke => "s",
        Paint::FillStroke => "B",
        Paint::FillStrokeEvenOdd => "B*",
        Paint::CloseFillStroke => "b",
        Paint::CloseFillStrokeEvenOdd => "b*",
        Paint::None => "n",
    }
}

/// A name for `id` in `kind`: `Sh` and six base32 characters, its
/// digest's first thirty bits, counted up past any name `reserved` keeps
/// for something else or this content has given already.
fn fresh(
    kind: ResKind,
    id: ResId,
    reserved: &Reserved,
    taken: &BTreeSet<(ResKind, Vec<u8>)>,
) -> Vec<u8> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    const BITS: u32 = 30;
    let mut n = (id.0 >> (128 - BITS)) as u32;
    loop {
        let mut name = b"Sh".to_vec();
        name.extend((0..6).rev().map(|i| ALPHABET[(n >> (5 * i)) as usize & 31]));
        if !taken.contains(&(kind, name.clone())) && reserved.allows(kind, &name, id) {
            return name;
        }
        n = (n + 1) & ((1 << BITS) - 1);
    }
}

/// `x` as an operand, as [`ContentWriter`] writes every number: at most
/// four decimals, trailing zeros and a bare point trimmed, with no
/// exponent and no `-0`. Anything but a finite number is 0.
///
/// ```
/// use pdf_content_streams::write_number;
///
/// let mut out = Vec::new();
/// for x in [1.0 / 3.0, 2.5, -0.00001, 1e7, f32::NAN] {
///     write_number(&mut out, x);
///     out.push(b' ');
/// }
/// assert_eq!(out, b"0.3333 2.5 0 10000000 0 ");
/// ```
pub fn write_number(out: &mut Vec<u8>, x: f32) {
    let x = if x.is_finite() { x } else { 0.0 };
    let start = out.len();
    let _ = write!(out, "{x:.4}");
    while out.last() == Some(&b'0') {
        out.pop();
    }
    if out.last() == Some(&b'.') {
        out.pop();
    }
    if out[start..] == *b"-0" {
        out.truncate(start);
        out.push(b'0');
    }
}

/// Codes `codes` as a string operand (7.3.4): literal, `(`, `)` and `\`
/// escaped, unless one is a control code (below 0x20, or 0x7F, as a
/// two-byte font's codes may be), when it's written in hex. Every byte is
/// written: PDFium reads a string's first 32,767.
///
/// ```
/// use pdf_content_streams::write_string;
///
/// let mut out = Vec::new();
/// write_string(&mut out, b"a(b)\\");
/// write_string(&mut out, b"\x00\x41");
/// assert_eq!(out, b"(a\\(b\\)\\\\)<0041>");
/// ```
pub fn write_string(out: &mut Vec<u8>, codes: &[u8]) {
    if codes.iter().any(|&b| b < 0x20 || b == 0x7f) {
        out.push(b'<');
        for b in codes {
            let _ = write!(out, "{b:02X}");
        }
        out.push(b'>');
        return;
    }
    out.push(b'(');
    for &b in codes {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
}

#[cfg(test)]
mod tests;
