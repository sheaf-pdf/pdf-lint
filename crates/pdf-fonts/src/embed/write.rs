//! A program and the glyphs drawn in it written as a font: a Type 0 font over
//! `/Identity-H` and the objects it reaches (ISO 32000-2, 9.7, 9.8, 9.9 and 9.10.3).

use std::fmt::Write as _;

use pdf_objects::object::Stream;
use pdf_objects::{Dict, Object, dict};
use skrifa::MetadataProvider;
use skrifa::raw::{FontRef, TableProvider};
use skrifa::string::StringId;

use super::subset::Subset;
use super::{Error, Glyphs, Level, Program, face, sfnt};
use crate::phases::phase;

/// The most bytes of a program's name a font's name is given: with a subset's tag and
/// its `+`, 127, the most earlier versions of PDF recommended a name hold (ISO 32000-2,
/// Annex C.2).
const NAME: usize = 120;

/// How many entries a `/ToUnicode` CMap's block holds at most: the bound Adobe's CMap
/// format sets, which readers keep.
const BLOCK: usize = 100;

/// A descriptor's `/Flags` (ISO 32000-2, 9.8.2): its glyphs all as wide.
const FIXED_PITCH: i64 = 1 << 0;
/// See [`FIXED_PITCH`]: its glyphs have serifs.
const SERIF: i64 = 1 << 1;
/// See [`FIXED_PITCH`]: its glyphs are outside the standard Latin set, as a CIDFont's,
/// addressed by CID, are.
const SYMBOLIC: i64 = 1 << 2;
/// See [`FIXED_PITCH`]: its glyphs lean.
const ITALIC: i64 = 1 << 6;

/// The start of a `/ToUnicode` CMap, its every code two bytes.
const CMAP_HEAD: &str = "/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
";

/// The end of a `/ToUnicode` CMap, after its entries.
const CMAP_TAIL: &str = "endcmap
CMapName currentdict /CMap defineresource pop
end
end
";

/// How [`write()`] writes a font. Each choice is off by default: a subset, without a
/// `/CIDSet`, of a program whose licence lets it be embedded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Options {
    /// Whether the descriptor has a `/CIDSet`, the CIDs its program holds (ISO 32000-2,
    /// 9.8.3.1): which PDF/A-1 asks of a subset, and PDF 2.0 deprecates.
    pub cid_set: bool,
    /// Whether the program is embedded whole, every glyph, not cut to the glyphs drawn.
    /// A program whose licence asks to be embedded whole is, whatever this says
    /// ([`Embedding::subsetting`](super::Embedding::subsetting)).
    pub whole: bool,
    /// Whether the caller has the right to embed a program whose licence restricts it
    /// to its owner's leave, or to its bitmaps ([`Embedding`](super::Embedding)):
    /// without it, such a program is refused.
    pub restricted: bool,
}

/// The font text drawn in `program` shows in, as a PDF writes it: a Type 0 font over
/// `/Identity-H` (ISO 32000-2, 9.7.6.1) first, then the objects it reaches, a reference
/// `(i, 0)` among them to the `i`th, as
/// [`Res::of`](pdf_content_streams::Res::of) takes a resource's objects:
///
/// 0. The Type 0 font: its `/BaseFont`, `/Encoding /Identity-H`, its CIDFont and its
///    `/ToUnicode`.
/// 1. Its CIDFont, a CIDFontType2 (9.7.4): the same `/BaseFont`, `/CIDSystemInfo`
///    Adobe-Identity-0, each glyph drawn's width in `/W`, its [`Program::width`], and
///    its `/CIDToGIDMap`.
/// 2. Its descriptor (9.8): the same `/FontName`, and the program's flags, box, italic
///    angle, heights and stem, as below.
/// 3. Its `/FontFile2` (9.9): the program cut to the glyphs drawn, `.notdef` and the
///    glyphs their composites draw, `/Length1` its length.
/// 4. Its `/ToUnicode` CMap (9.10.3): each glyph drawn's text, as `glyphs` keeps it, in
///    blocks of at most 100; a character past the Basic Multilingual Plane is its UTF-16
///    surrogate pair, one code's, and a glyph kept with no text has none.
/// 5. Where the program is cut, its `/CIDToGIDMap`: two bytes a CID, from 0 to the last
///    glyph kept, the glyph in the cut program that CID's glyph in `program` is. A code's
///    CID is the glyph's number in `program` ([`Program::codes`]), not in the cut, so
///    codes drawn before more glyphs are drawn stay right when the font is written again.
///    Where it's embedded whole, the CIDFont's `/CIDToGIDMap` is `/Identity`, and there's
///    none.
/// 6. Where `options` asks for it, its `/CIDSet` (9.8.3.1): a bit a CID, CID 0's the
///    first byte's high bit, set for each glyph the program written holds.
///
/// Streams are unfiltered, as filtering is for what writes the file. A cut program's
/// name is a tag and `+` before the program's PostScript name (9.9.2): six capitals from
/// a digest of the program and the glyphs it's cut to, so the same glyphs of a program
/// are written in the same bytes, and others under another tag, but for a chance in
/// 26^6. A name's bytes are what a PDF name holds unescaped, `#` and the delimiters left
/// out, at most 120; where it has no PostScript name, its full name's; where neither,
/// `Unnamed`.
///
/// Every value of the descriptor is the program's, in thousandths of an em but its
/// angle: `/Flags` symbolic, as a CIDFont's glyphs are addressed by CID, fixed pitch as
/// its `post` says, serif where its `OS/2`'s family class is a serif one, italic where
/// its `head` says or its `post` gives an angle; `/FontBBox` its `head`'s, rounded out;
/// `/ItalicAngle` its `post`'s; `/Ascent` and `/Descent` its [`Program::metrics`];
/// `/CapHeight` its `OS/2`'s, else the top of the glyph `H` draws, else its ascent;
/// `/XHeight` its `OS/2`'s, where it has one. `/StemV` is estimated: 10 and 0.244 a unit
/// of its `OS/2`'s weight past 50, so 95 for a regular weight and 169 for a bold, or 80
/// without an `OS/2`. Widths and heights are whole thousandths, as PDFium reads each as
/// an integer.
///
/// Text is kept a glyph, not a character ([`Glyphs`]): two characters the program draws
/// with one glyph show one code, which reads back as the smaller of their texts.
///
/// ```
/// use pdf_fonts::embed::{Glyphs, Options, Program, write};
/// use pdf_fonts::pdf_content_streams::Res;
/// use pdf_fonts::pdf_objects::Object;
///
/// # let bytes = pdf_fixtures::font::sample(1000).build();
/// let program = Program::open(bytes, 0)?;
/// let mut glyphs = Glyphs::new();
/// let codes = glyphs.add(&program, "Ag")?;
/// let objects = write(&program, &glyphs, &Options::default())?;
///
/// // A Type 0 font, a subset's name, and A's and g's widths from CID 2, A's glyph.
/// let font = objects[0].as_dict().unwrap();
/// assert_eq!(font.get("Subtype").and_then(Object::as_name), Some(&b"Type0"[..]));
/// let name = font.get("BaseFont").and_then(Object::as_name).unwrap();
/// assert!(name[..6].iter().all(u8::is_ascii_uppercase));
/// assert_eq!(&name[6..], b"+FixtureSans-Regular");
/// let widths = objects[1].as_dict().unwrap().get("W").and_then(Object::as_array);
/// assert_eq!(widths.unwrap()[0].as_integer(), Some(2));
/// assert_eq!(codes, [0, 2, 0, 3]);
///
/// // A page's resource, its objects as they are.
/// let font = Res::of(objects)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// - [`Error::Restricted`] for a program whose licence restricts it to its owner's leave
///   or to its bitmaps, unless `options` says the caller has the right.
/// - [`Error::Glyph`] for a glyph `glyphs` keeps at or past the program's count: one of
///   another program's, or given [`Glyphs::insert`].
/// - [`Error::Malformed`], `loca`, for a glyph drawn, or one a composite among them
///   draws, whose `loca` entries don't place it; [`Error::Subset`] where the subsetter
///   refuses to cut the program.
pub fn write(program: &Program, glyphs: &Glyphs, options: &Options) -> Result<Vec<Object>, Error> {
    phase!(
        DEBUG,
        "embed",
        glyphs = glyphs.len(),
        bytes = program.data.len()
    );
    let embedding = program.embedding;
    let restricted = embedding.level == Level::Restricted || embedding.bitmaps_only;
    if restricted && !options.restricted {
        return Err(Error::Restricted(embedding));
    }
    let count = program.glyphs;
    if let Some((&glyph, _)) = glyphs.texts.last_key_value()
        && glyph >= count
    {
        return Err(Error::Glyph { glyph, count });
    }
    let whole = options.whole || !embedding.subsetting;
    let cut = match whole {
        true => program.whole()?,
        false => program.subset(glyphs)?,
    };
    let font = face(&program.data, program.index)?;
    let tag = (!whole).then(|| subset_tag(program, &cut.glyphs));
    let name = Object::name(font_name(&font, tag));

    // Each object's place, the streams after the dictionaries.
    let map = (!whole).then_some(5);
    let cid_set = options.cid_set.then(|| map.map_or(5, |map| map + 1));
    let to_gid = match map {
        Some(map) => Object::reference(map),
        None => Object::name("Identity"),
    };
    let type0 = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type0"),
        "BaseFont" => name.clone(),
        "Encoding" => Object::name("Identity-H"),
        "DescendantFonts" => vec![Object::reference(1)],
        "ToUnicode" => Object::reference(4),
    };
    let cid_font = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("CIDFontType2"),
        "BaseFont" => name.clone(),
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"),
            "Ordering" => Object::text("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => Object::reference(2),
        "W" => widths(program, glyphs),
        "CIDToGIDMap" => to_gid,
    };
    let mut descriptor = descriptor_of(program, &font, name)?;
    descriptor.push("FontFile2", Object::reference(3));
    if let Some(cid_set) = cid_set {
        descriptor.push("CIDSet", Object::reference(cid_set));
    }
    let Subset {
        program: cut,
        glyphs: kept,
    } = cut;
    let length = cut.len();
    let file = Stream::new(dict! { "Length1" => length as i64 }, cut);
    let mut objects = vec![
        Object::from(type0),
        Object::from(cid_font),
        Object::from(descriptor),
        Object::from(file),
        Object::from(Stream::new(Dict::new(), to_unicode(glyphs))),
    ];
    if map.is_some() {
        objects.push(Stream::new(Dict::new(), to_gid_map(&kept)).into());
    }
    if cid_set.is_some() {
        objects.push(Stream::new(Dict::new(), cids(&kept)).into());
    }
    Ok(objects)
}

/// The tag a subset of `program` to `kept`, its glyphs in order, is named by: six
/// capitals from XXH3-128 over a digest of the program's bytes, its face's number and
/// the glyphs' numbers, each high byte first. Digested in two so that the program's
/// bytes aren't copied: the one-shot digest is the one twox-hash gives without
/// allocating. A name, not a secret.
fn subset_tag(program: &Program, kept: &[u16]) -> [u8; 6] {
    let mut input = Vec::with_capacity(20 + 2 * kept.len());
    input.extend(twox_hash::XxHash3_128::oneshot(&program.data).to_be_bytes());
    input.extend(program.index.to_be_bytes());
    input.extend(kept.iter().flat_map(|glyph| glyph.to_be_bytes()));
    let mut digest = twox_hash::XxHash3_128::oneshot(&input);
    std::array::from_fn(|_| {
        let letter = b'A' + (digest % 26) as u8;
        digest /= 26;
        letter
    })
}

/// The name `font` is written under: `tag` and `+`, where it's a subset, before its
/// PostScript name's bytes that a name holds unescaped, its full name's where those are
/// none, or `Unnamed`.
fn font_name(font: &FontRef<'_>, tag: Option<[u8; 6]>) -> Vec<u8> {
    let named = |id| {
        let string = font.localized_strings(id).english_or_first()?;
        let kept: Vec<u8> = (string.chars())
            .filter_map(|c| u8::try_from(c).ok())
            .filter(|b| (b'!'..=b'~').contains(b) && !b"()<>[]{}/%#".contains(b))
            .take(NAME)
            .collect();
        (!kept.is_empty()).then_some(kept)
    };
    let base = named(StringId::POSTSCRIPT_NAME)
        .or_else(|| named(StringId::FULL_NAME))
        .unwrap_or_else(|| b"Unnamed".to_vec());
    match tag {
        Some(tag) => [&tag[..], b"+", &base].concat(),
        None => base,
    }
}

/// The `/W` of the glyphs `glyphs` keeps: each run of glyphs one after another its first
/// CID and an array of their widths, whole thousandths of an em ([`Program::width`]).
fn widths(program: &Program, glyphs: &Glyphs) -> Vec<Object> {
    let mut w = Vec::new();
    let mut run: Vec<Object> = Vec::new();
    let mut next = None;
    for &glyph in glyphs.texts.keys() {
        if next != Some(glyph) {
            if !run.is_empty() {
                w.push(Object::from(std::mem::take(&mut run)));
            }
            w.push(Object::from(glyph));
        }
        run.push(Object::from(program.width(glyph) as i64));
        next = glyph.checked_add(1);
    }
    if !run.is_empty() {
        w.push(Object::from(run));
    }
    w
}

/// `program`'s descriptor, named `name`, but for its program and `/CIDSet`: its values
/// as [`write()`] says, from `font`, its face.
fn descriptor_of(program: &Program, font: &FontRef<'_>, name: Object) -> Result<Dict, Error> {
    let em = |v: f32| v * 1000.0 / f32::from(program.upem);
    let round = |v: f32| Object::from(v.round() as i64);
    let metrics = program.metrics;
    let os2 = font.os2().ok();
    let post = font.post().ok();
    let angle = post
        .as_ref()
        .map_or(0.0, |post| post.italic_angle().to_f32());
    let leans = font
        .head()
        .is_ok_and(|head| head.mac_style().bits() & 0x0002 != 0);
    let mut flags = SYMBOLIC;
    if post.as_ref().is_some_and(|post| post.is_fixed_pitch() != 0) {
        flags |= FIXED_PITCH;
    }
    // Oldstyle, transitional, modern, Clarendon, slab and freeform serifs.
    let class = os2.as_ref().map_or(0, |os2| os2.s_family_class() >> 8);
    if matches!(class, 1..=5 | 7) {
        flags |= SERIF;
    }
    if leans || angle != 0.0 {
        flags |= ITALIC;
    }
    let given = |v: Option<i16>| v.filter(|&v| v != 0).map(|v| em(f32::from(v)));
    let cap = given(os2.as_ref().and_then(|os2| os2.s_cap_height()));
    let x_height = given(os2.as_ref().and_then(|os2| os2.sx_height()));
    let tables = sfnt::Tables::of(&program.data, font)?;
    let glyf = tables.glyf()?;
    // The top of the box a glyph's header gives.
    let top = |glyph: u16| {
        let data = glyf.glyph(glyph).filter(|data| data.len() >= 10)?;
        Some(em(f32::from(i16::from_be_bytes([data[8], data[9]]))))
    };
    let cap = cap
        .or_else(|| program.glyph('H').and_then(top))
        .unwrap_or(metrics.ascent);
    let weight = os2
        .as_ref()
        .map(|os2| os2.us_weight_class().clamp(50, 1000));
    let stem = weight.map_or(80.0, |weight| 10.0 + 0.244 * (f32::from(weight) - 50.0));
    let bbox = metrics.bbox.unwrap_or_default();
    let bbox = [
        bbox.x0.floor(),
        bbox.y0.floor(),
        bbox.x1.ceil(),
        bbox.y1.ceil(),
    ];
    let mut descriptor = dict! {
        "Type" => Object::name("FontDescriptor"),
        "FontName" => name,
        "Flags" => flags,
        "FontBBox" => bbox.map(round).to_vec(),
        "ItalicAngle" => number(angle),
        "Ascent" => round(metrics.ascent),
        "Descent" => round(metrics.descent),
        "CapHeight" => round(cap),
    };
    if let Some(x_height) = x_height {
        descriptor.push("XHeight", round(x_height));
    }
    descriptor.push("StemV", round(stem));
    Ok(descriptor)
}

/// `v`, an integer where it's whole, else the real nearest its shortest decimal.
fn number(v: f32) -> Object {
    match v.fract() == 0.0 && v.abs() < 16_777_216.0 {
        true => Object::from(v as i64),
        false => Object::from(v),
    }
}

/// The `/ToUnicode` CMap of `glyphs`: each glyph, its code, kept with a text, its text
/// as UTF-16BE, a character past the Basic Multilingual Plane as its surrogate pair, in
/// blocks of [`BLOCK`].
fn to_unicode(glyphs: &Glyphs) -> Vec<u8> {
    let texts: Vec<(&u16, &String)> = glyphs.texts.iter().filter(|t| !t.1.is_empty()).collect();
    let mut cmap = String::from(CMAP_HEAD);
    for block in texts.chunks(BLOCK) {
        let _ = writeln!(cmap, "{} beginbfchar", block.len());
        for (glyph, text) in block {
            let _ = write!(cmap, "<{glyph:04X}> <");
            for unit in text.encode_utf16() {
                let _ = write!(cmap, "{unit:04X}");
            }
            cmap.push_str(">\n");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str(CMAP_TAIL);
    cmap.into_bytes()
}

/// The `/CIDToGIDMap` of a program cut to `kept`, their numbers in it in order: for each
/// CID to the last kept, two bytes, high first, its glyph's number in the cut program,
/// its place among `kept`; 0 for a CID not kept.
fn to_gid_map(kept: &[u16]) -> Vec<u8> {
    let last = kept.last().map_or(0, |&glyph| usize::from(glyph));
    let mut map = vec![0; 2 * (last + 1)];
    for (new, &old) in kept.iter().enumerate() {
        let at = 2 * usize::from(old);
        map[at..at + 2].copy_from_slice(&(new as u16).to_be_bytes());
    }
    map
}

/// The `/CIDSet` of a program holding `kept`, each its CID: a bit a CID to the last,
/// CID 0's the first byte's high bit, set for each one kept.
fn cids(kept: &[u16]) -> Vec<u8> {
    let last = kept.last().map_or(0, |&glyph| usize::from(glyph));
    let mut bits = vec![0; (last + 1).div_ceil(8)];
    for &cid in kept {
        bits[usize::from(cid) / 8] |= 0x80 >> (cid % 8);
    }
    bits
}
