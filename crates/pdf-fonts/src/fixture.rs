//! Font programs built by hand, for the tests and the fuzz seeds: what a program
//! needs of each format and no more.

/// A TrueType program of 1000 units an em, its glyphs, each an outline's
/// box, as FreeType loads it unscaled:
///
/// | glyph | for | outline | advance |
/// |---|---|---|---|
/// | 0 | `.notdef` | a box, 50..450 × 0..700 | 500 |
/// | 1 | `A` | a triangle, 0..600 × 0..750 | 600 |
/// | 2 | `g` | 20..480 × -200..500, an off-curve point at 250,560 | 500 |
/// | 3 | space | empty | 250 |
/// | 4 | `B` | 100..400 × 0..300, its left side bearing 50, so drawn at 50..350 | 450 |
/// | 5 | `C` | glyph 1 moved by 100,50: 100..700 × 50..800 | 700 |
///
/// Its `cmap` (3,1) maps `A`, `B`, `C`, `g` and the space; its `head`'s box
/// is 0..700 × -200..800 and its `hhea` ascender and descender 800 and
/// -200; it names no glyph (`post` 3).
pub fn truetype_program() -> Vec<u8> {
    let rect = |x0: i16, y0: i16, x1: i16, y1: i16| {
        vec![
            (x0, y0, true),
            (x1, y0, true),
            (x1, y1, true),
            (x0, y1, true),
        ]
    };
    let glyphs = [
        simple_glyph(&rect(50, 0, 450, 700)),
        simple_glyph(&[(0, 0, true), (300, 750, true), (600, 0, true)]),
        simple_glyph(&[
            (20, -200, true),
            (480, -200, true),
            (480, 500, true),
            (250, 560, false),
            (20, 500, true),
        ]),
        Vec::new(),
        simple_glyph(&rect(100, 0, 400, 300)),
        // A composite: glyph 1, its offsets words and x and y.
        [
            &be(&[-1i16, 100, 50, 700, 800])[..],
            &be(&[0x0003, 1, 100, 50]),
        ]
        .concat(),
    ];
    let metrics: [(i16, i16); 6] = [
        (500, 50),
        (600, 0),
        (500, 20),
        (250, 0),
        (450, 50),
        (700, 100),
    ];
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for glyph in &glyphs {
        loca.extend((glyf.len() as u32).to_be_bytes());
        glyf.extend(glyph);
        glyf.resize(glyf.len().next_multiple_of(4), 0);
    }
    loca.extend((glyf.len() as u32).to_be_bytes());
    let maxp = [
        &0x0001_0000u32.to_be_bytes()[..],
        &be(&[6, 5, 1, 3, 1, 2, 0, 0, 0, 0, 0, 0, 1, 1]),
    ]
    .concat();
    let hmtx: Vec<u8> = metrics.iter().flat_map(|&(w, lsb)| be(&[w, lsb])).collect();
    sfnt(&mut [
        (
            *b"cmap",
            cmap(&[(0x20, 3), (0x41, 1), (0x42, 4), (0x43, 5), (0x67, 2)]),
        ),
        (*b"glyf", glyf),
        (*b"head", head()),
        (*b"hhea", hhea(6)),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
        (*b"post", post_3()),
    ])
}

/// A `head` of 1000 units an em, its box 0..700 × -200..800.
fn head() -> Vec<u8> {
    [
        &0x0001_0000u32.to_be_bytes()[..],
        &0x0001_0000u32.to_be_bytes(),
        &0u32.to_be_bytes(),
        &0x5F0F_3CF5u32.to_be_bytes(),
        &be(&[0, 1000]),
        &[0; 16],
        &be(&[0, -200, 700, 800, 0, 8, 2, 1, 0]),
    ]
    .concat()
}

/// An `hhea`, its ascender and descender 800 and -200, for `metrics` glyphs' metrics.
fn hhea(metrics: i16) -> Vec<u8> {
    [
        &0x0001_0000u32.to_be_bytes()[..],
        &be(&[
            800, -200, 0, 700, 0, 0, 700, 1, 0, 0, 0, 0, 0, 0, 0, metrics,
        ]),
    ]
    .concat()
}

/// A `post` of version 3, that names no glyph.
fn post_3() -> Vec<u8> {
    [&0x0003_0000u32.to_be_bytes()[..], &[0; 28]].concat()
}

/// A `post` of version 2 naming glyphs 0 to 3, by their numbers in the standard Macintosh
/// order: `.notdef`, the space, `g` and `A`.
fn post_2() -> Vec<u8> {
    [
        &0x0002_0000u32.to_be_bytes()[..],
        &[0; 28],
        &be(&[4, 0, 3, 74, 36]),
    ]
    .concat()
}

/// What an OpenType program built by hand is told by: whether its `post` names its
/// glyphs, the other way about from its CFF, or names none, and whether it has its
/// `cmap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenType {
    pub post_names: bool,
    pub cmap: bool,
}

/// An OpenType program of [`cff_program`]'s glyphs, by its charset's names (`post` 3
/// where `post_names` is false, as most have) and its standard encoding, its `cmap` (3,1)
/// mapping only `A`.
pub fn opentype_program(of: OpenType) -> Vec<u8> {
    opentype(cff_program(), of)
}

/// An OpenType program of [`cid_cff_program`]'s glyphs, as [`opentype_program`].
pub fn opentype_cid_program(of: OpenType) -> Vec<u8> {
    opentype(cid_cff_program(), of)
}

/// An `OTTO` SFNT of `cff`, a CFF of four glyphs, its advances 500.
fn opentype(cff: Vec<u8>, of: OpenType) -> Vec<u8> {
    let maxp = [&0x0000_5000u32.to_be_bytes()[..], &be(&[4])].concat();
    let hmtx = [500i16, 0].repeat(4);
    let post = match of.post_names {
        true => post_2(),
        false => post_3(),
    };
    let mut tables = vec![
        (*b"CFF ", cff),
        (*b"head", head()),
        (*b"hhea", hhea(4)),
        (*b"hmtx", be(&hmtx)),
        (*b"maxp", maxp),
        (*b"post", post),
    ];
    if of.cmap {
        tables.push((*b"cmap", cmap(&[(0x41, 2)])));
    }
    sfnt_tagged(*b"OTTO", &mut tables)
}

/// Big-endian 16-bit numbers.
fn be(values: &[i16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// A simple glyph of one contour through `points`, each its x, its y and
/// whether it's on the curve, its coordinates each a word.
fn simple_glyph(points: &[(i16, i16, bool)]) -> Vec<u8> {
    let xs = points.iter().map(|p| p.0);
    let ys = points.iter().map(|p| p.1);
    let bbox = [
        xs.clone().min(),
        ys.clone().min(),
        xs.clone().max(),
        ys.clone().max(),
    ]
    .map(Option::unwrap_or_default);
    let mut glyph = be(&[
        1,
        bbox[0],
        bbox[1],
        bbox[2],
        bbox[3],
        points.len() as i16 - 1,
        0,
    ]);
    glyph.extend(points.iter().map(|p| u8::from(p.2)));
    let mut last = (0, 0);
    let mut x = Vec::new();
    let mut y = Vec::new();
    for &(px, py, _) in points {
        x.push(px - last.0);
        y.push(py - last.1);
        last = (px, py);
    }
    glyph.extend(be(&x));
    glyph.extend(be(&y));
    glyph
}

/// A `cmap` of one (3,1) subtable of format 4 mapping each code point to
/// its glyph, a segment each.
fn cmap(map: &[(u16, u16)]) -> Vec<u8> {
    let segments = map.len() + 1;
    let ends: Vec<u16> = map.iter().map(|m| m.0).chain([0xFFFF]).collect();
    let deltas: Vec<u16> = map
        .iter()
        .map(|m| m.1.wrapping_sub(m.0))
        .chain([1])
        .collect();
    let mut table: Vec<u8> = Vec::new();
    let length = 16 + segments * 8;
    for v in [4, length as u16, 0, (segments * 2) as u16, 0, 0, 0] {
        table.extend(v.to_be_bytes());
    }
    for v in ends.iter().chain([&0]).chain(&ends).chain(&deltas) {
        table.extend(v.to_be_bytes());
    }
    table.extend(vec![0; segments * 2]);
    [&[0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12][..], &table].concat()
}

/// A TrueType SFNT of `tables`, in order of their tags.
fn sfnt(tables: &mut [([u8; 4], Vec<u8>)]) -> Vec<u8> {
    sfnt_tagged(0x0001_0000u32.to_be_bytes(), tables)
}

/// An SFNT of `tables`, in order of their tags, its format's tag `tag`.
fn sfnt_tagged(tag: [u8; 4], tables: &mut [([u8; 4], Vec<u8>)]) -> Vec<u8> {
    tables.sort_by_key(|t| t.0);
    let n = tables.len() as u16;
    let mut out = [&tag[..], &be(&[n as i16, 0, 0, 0])].concat();
    let mut offset = 12 + 16 * tables.len();
    let mut data = Vec::new();
    for (tag, table) in tables.iter() {
        out.extend(tag);
        out.extend(0u32.to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((table.len() as u32).to_be_bytes());
        data.extend(table);
        data.resize(data.len().next_multiple_of(4), 0);
        offset = 12 + 16 * tables.len() + data.len();
    }
    out.extend(data);
    out
}

/// A bare CFF program (`/FontFile3 /Type1C`) of 1000 units an em, its
/// glyphs `.notdef` and the space empty, `A` and `g` [`truetype_program`]'s
/// outlines (its `g`'s off-curve point two control points of a curve, at
/// 380,560 and 250,560), by its charset's names, in the standard
/// encoding; its `FontBBox` 0..600 × -200..750.
pub fn cff_program() -> Vec<u8> {
    cff(&[vec![14], vec![14], cff_a(), cff_g()], &[1, 34, 72], &[])
}

/// A CFF dictionary's real number, as its digits are written.
fn real(number: &str) -> Vec<u8> {
    let nibble = |c: char| match c {
        '.' => 0xa,
        '-' => 0xe,
        digit => digit.to_digit(10).unwrap() as u8,
    };
    let mut nibbles: Vec<u8> = number.chars().map(nibble).collect();
    nibbles.push(0xf);
    nibbles.push(0xf);
    let bytes = nibbles.as_chunks::<2>().0.iter().map(|n| n[0] << 4 | n[1]);
    std::iter::once(30).chain(bytes).collect()
}

/// A CFF glyph's charstring: `A`, [`truetype_program`]'s outline.
fn cff_a() -> Vec<u8> {
    [
        cs(&[0, 0]),
        vec![21],
        cs(&[300, 750]),
        vec![5],
        cs(&[300, -750]),
        vec![5, 14],
    ]
    .concat()
}

/// A CFF glyph's charstring: `g`, [`truetype_program`]'s outline.
fn cff_g() -> Vec<u8> {
    [
        cs(&[20, -200]),
        vec![21],
        cs(&[460, 0]),
        vec![5],
        cs(&[0, 700]),
        vec![5],
        cs(&[-100, 60, -130, 0, -130, -60]),
        vec![8, 14],
    ]
    .concat()
}

/// A CID-keyed CFF (`/FontFile3 /CIDFontType0C`) of 1000 units an em: glyph 0 `.notdef`,
/// empty, glyph 1 `A` and glyph 2 `g`, its charset giving both CID 5; glyph 3 `A` as
/// CID 7.
pub fn cid_cff_program() -> Vec<u8> {
    cid_cff_with(None, None)
}

/// [`cid_cff_program`] with a `/FontMatrix` of these numbers in its top dictionary and
/// in its font dictionary.
pub fn cid_cff_with(top_matrix: Option<[&str; 6]>, font_matrix: Option<[&str; 6]>) -> Vec<u8> {
    let index = |items: &[Vec<u8>]| -> Vec<u8> {
        let mut out = (items.len() as u16).to_be_bytes().to_vec();
        out.push(4);
        let mut at = 1u32;
        for item in items.iter().map(|i| i.len() as u32).chain([0]) {
            out.extend(at.to_be_bytes());
            at += item;
        }
        out.extend(items.concat());
        out
    };
    let int = |v: usize| [&[29u8][..], &(v as i32).to_be_bytes()].concat();
    let header = [1u8, 0, 4, 4];
    let names = index(&[b"Fix".to_vec()]);
    let strings = index(&[b"Adobe".to_vec(), b"Identity".to_vec()]);
    let globals = vec![0u8, 0];
    let charset: Vec<u8> = [0u8, 0, 5, 0, 5, 0, 7].to_vec();
    let fd_select: Vec<u8> = vec![0, 0, 0, 0, 0];
    let glyphs = index(&[vec![14], cff_a(), cff_g(), cff_a()]);
    let private = [cs(&[0]), vec![21]].concat();
    let matrix = |numbers: Option<[&str; 6]>| match numbers {
        Some(numbers) => [numbers.map(real).concat(), vec![12, 7]].concat(),
        None => Vec::new(),
    };
    let font_dict = |private_at: usize| {
        [
            matrix(font_matrix),
            int(private.len()),
            int(private_at),
            vec![18],
        ]
        .concat()
    };
    let top = |charset_at: usize, select_at: usize, glyphs_at: usize, array_at: usize| {
        [
            int(391),
            int(392),
            int(0),
            vec![12, 30],
            matrix(top_matrix),
            int(charset_at),
            vec![15],
            int(select_at),
            vec![12, 37],
            int(glyphs_at),
            vec![17],
            int(array_at),
            vec![12, 36],
        ]
        .concat()
    };
    let top_len = index(&[top(0, 0, 0, 0)]).len();
    let charset_at = header.len() + names.len() + top_len + strings.len() + globals.len();
    let select_at = charset_at + charset.len();
    let glyphs_at = select_at + fd_select.len();
    let private_at = glyphs_at + glyphs.len();
    let array_at = private_at + private.len();
    [
        &header[..],
        &names,
        &index(&[top(charset_at, select_at, glyphs_at, array_at)]),
        &strings,
        &globals,
        &charset,
        &fd_select,
        &glyphs,
        &private,
        &index(&[font_dict(private_at)]),
    ]
    .concat()
}

/// A bare CFF whose `A` calls its first subroutine, each of whose
/// `levels` calls the next eight times, the last drawing a line: at ten,
/// its work past any glyph's fuel.
pub fn heavy_cff_program(levels: i32) -> Vec<u8> {
    let leaf = [cs(&[10, 10]), vec![5, 11]].concat();
    let a = [cs(&[0, 0]), vec![21], cs(&[-107]), vec![10, 14]].concat();
    nested_cff(levels, leaf, a)
}

/// [`heavy_cff_program`] whose last level moves nowhere and whose `A` draws one
/// line once they've run: its operations can pass read-fonts' bound with few
/// points and a box to tell. At six levels, about 210,000 operations; at seven,
/// 1.68 million.
pub fn moving_cff_program(levels: i32) -> Vec<u8> {
    let leaf = [cs(&[0, 0]), vec![21, 11]].concat();
    let a = [
        cs(&[0, 0]),
        vec![21],
        cs(&[-107]),
        vec![10],
        cs(&[10, 10]),
        vec![5, 14],
    ]
    .concat();
    nested_cff(levels, leaf, a)
}

/// A bare CFF whose `A` is `moves` of a `hmoveto` in a row, then its end: `2 * moves + 1`
/// operations, with no subroutine.
pub fn long_cff_program(moves: usize) -> Vec<u8> {
    let a = [[139, 22].repeat(moves), vec![14]].concat();
    cff(&[vec![14], vec![14], a], &[1, 34], &[])
}

/// A bare CFF of `a`, as its `A`, and `levels` subroutines, each calling the next
/// eight times, the last `leaf`.
fn nested_cff(levels: i32, leaf: Vec<u8>, a: Vec<u8>) -> Vec<u8> {
    let subrs: Vec<Vec<u8>> = (0..levels)
        .map(|k| match k + 1 == levels {
            true => leaf.clone(),
            false => [cs(&[k + 1 - 107]), vec![10]]
                .concat()
                .repeat(8)
                .into_iter()
                .chain([11])
                .collect(),
        })
        .collect();
    cff(&[vec![14], vec![14], a], &[1, 34], &subrs)
}

/// A TrueType program whose glyph 0 is a composite of glyph 1 fifty
/// times, glyph 1 of glyph 2 fifty times, and so on to glyph 6, a
/// triangle: 50⁶ glyphs drawn for one.
pub fn composite_bomb_program() -> Vec<u8> {
    let component = |glyph: i16| be(&[0x0023, glyph, 0, 0]);
    let mut glyphs: Vec<Vec<u8>> = (1..7)
        .map(|next| {
            let mut glyph = be(&[-1, 0, 0, 600, 750]);
            for _ in 0..49 {
                glyph.extend(component(next));
            }
            // The last component, without MORE_COMPONENTS.
            glyph.extend(be(&[0x0003, next, 0, 0]));
            glyph
        })
        .collect();
    glyphs.push(simple_glyph(&[
        (0, 0, true),
        (300, 750, true),
        (600, 0, true),
    ]));
    composite_font(&glyphs)
}

/// A TrueType program whose glyph 0 is a composite of glyph 1 `groups` times, of
/// glyph 2 `leaves` times and of glyph 3 `empties` times; glyph 1 is of glyph 2 fifty
/// times, glyph 2 a triangle, glyph 3 empty. So `51 * groups + leaves` components
/// that aren't empty, nested, in few bytes.
pub fn composite_of_program(groups: usize, leaves: usize, empties: usize) -> Vec<u8> {
    let of = |glyph: i16, times: usize| {
        let mut components = be(&[0x0023, glyph, 0, 0]).repeat(times);
        components.extend(be(&[0x0003, 2, 0, 0]));
        components
    };
    let mut root = be(&[-1, 0, 0, 600, 750]);
    root.extend(be(&[0x0023, 1, 0, 0]).repeat(groups));
    root.extend(be(&[0x0023, 3, 0, 0]).repeat(empties));
    // The last component, without MORE_COMPONENTS, is one of the leaves.
    root.extend(of(2, leaves - 1));
    let mut group = be(&[-1, 0, 0, 600, 750]);
    group.extend(of(2, 49));
    let triangle = simple_glyph(&[(0, 0, true), (300, 750, true), (600, 0, true)]);
    composite_font(&[root, group, triangle, Vec::new()])
}

/// A TrueType program of `glyphs`, as they're given.
fn composite_font(glyphs: &[Vec<u8>]) -> Vec<u8> {
    let count = glyphs.len() as i16;
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for glyph in glyphs {
        loca.extend((glyf.len() as u32).to_be_bytes());
        glyf.extend(glyph);
        glyf.resize(glyf.len().next_multiple_of(4), 0);
    }
    loca.extend((glyf.len() as u32).to_be_bytes());
    let head = [
        &0x0001_0000u32.to_be_bytes()[..],
        &0x0001_0000u32.to_be_bytes(),
        &0u32.to_be_bytes(),
        &0x5F0F_3CF5u32.to_be_bytes(),
        &be(&[0, 1000]),
        &[0; 16],
        &be(&[0, 0, 600, 750, 0, 8, 2, 1, 0]),
    ]
    .concat();
    let hhea = [
        &0x0001_0000u32.to_be_bytes()[..],
        &be(&[800, -200, 0, 600, 0, 0, 600, 1, 0, 0, 0, 0, 0, 0, 0, count]),
    ]
    .concat();
    let maxp = [
        &0x0001_0000u32.to_be_bytes()[..],
        &be(&[count, 3, 1, 3, 1, 2, 0, 0, 0, 0, 0, 0, 50, 7]),
    ]
    .concat();
    let hmtx = be(&[600, 0].repeat(glyphs.len()));
    sfnt(&mut [
        (*b"glyf", glyf),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
    ])
}

/// A CFF of `charstrings`, `sids` its glyphs' names after `.notdef`, and
/// `subrs` its local subroutines.
fn cff(charstrings: &[Vec<u8>], sids: &[u16], subrs: &[Vec<u8>]) -> Vec<u8> {
    let index = |items: &[Vec<u8>]| -> Vec<u8> {
        if items.is_empty() {
            return vec![0, 0];
        }
        let mut out = (items.len() as u16).to_be_bytes().to_vec();
        out.push(4);
        let mut at = 1u32;
        for item in items.iter().map(|i| i.len() as u32).chain([0]) {
            out.extend(at.to_be_bytes());
            at += item;
        }
        out.extend(items.concat());
        out
    };
    // Operands of five bytes each, so the dictionary's size doesn't depend
    // on them.
    let int = |v: i32| [&[29u8][..], &v.to_be_bytes()].concat();
    let private = match subrs.is_empty() {
        true => [cs(&[0]), vec![21]].concat(),
        // Its subroutines just after it, 8 bytes on.
        false => [int(8), vec![19], cs(&[0]), vec![21]].concat(),
    };
    // Its box, then where its charset, charstrings and private dictionary
    // are.
    let top = |charset: usize, charstrings: usize, private_at: usize| {
        let at = |v: usize| int(v as i32);
        let small = |v: i16| [&[28u8][..], &v.to_be_bytes()].concat();
        let bbox = [small(0), small(-200), small(600), small(750), vec![5]].concat();
        let private = [int(private.len() as i32), at(private_at), vec![18]].concat();
        [
            bbox,
            at(charset),
            vec![15],
            at(charstrings),
            vec![17],
            private,
        ]
        .concat()
    };
    let header = [1u8, 0, 4, 4];
    let names = index(&[b"Fix".to_vec()]);
    let top_at = header.len() + names.len();
    let after_top = top_at + index(&[top(0, 0, 0)]).len() + index(&[]).len() + index(&[]).len();
    let charset: Vec<u8> = [
        &[0u8][..],
        &sids
            .iter()
            .flat_map(|s| s.to_be_bytes())
            .collect::<Vec<_>>(),
    ]
    .concat();
    let charstrings_at = after_top + charset.len();
    let glyphs = index(charstrings);
    let private_at = charstrings_at + glyphs.len();
    let top = top(after_top, charstrings_at, private_at);
    [
        &header[..],
        &names,
        &index(&[top]),
        &index(&[]),
        &index(&[]),
        &charset,
        &glyphs,
        &private,
        &index(subrs),
    ]
    .concat()
}

/// A Type 1 program (`/FontFile`): clear text, then its private dictionary
/// and charstrings encrypted (`eexec`, `lenIV` 4), its glyphs and box
/// [`cff_program`]'s, `g`'s side bearing 20, in the standard encoding.
pub fn type1_program() -> Vec<u8> {
    type1_of("g")
}

/// [`type1_program`], its `g` by another name.
pub fn type1_of(g: &str) -> Vec<u8> {
    let charstring = |sbw: [i32; 2], ops: Vec<u8>| -> Vec<u8> {
        let plain = [vec![0; 4], cs(&sbw), vec![13], ops, vec![14]].concat();
        encrypt(&plain, 4330)
    };
    let glyphs = [
        (".notdef", charstring([0, 500], Vec::new())),
        ("space", charstring([0, 250], Vec::new())),
        (
            "A",
            charstring(
                [0, 600],
                [
                    cs(&[0, 0]),
                    vec![21],
                    cs(&[300, 750]),
                    vec![5],
                    cs(&[300, -750]),
                    vec![5, 9],
                ]
                .concat(),
            ),
        ),
        (
            g,
            charstring(
                [20, 500],
                [
                    cs(&[0, -200]),
                    vec![21],
                    cs(&[460, 0]),
                    vec![5],
                    cs(&[0, 700]),
                    vec![5],
                    cs(&[-100, 60, -130, 0, -130, -60]),
                    vec![8, 9],
                ]
                .concat(),
            ),
        ),
    ];
    let mut private = b"dup /Private 8 dict dup begin\n\
        /RD{string currentfile exch readstring pop}executeonly def\n\
        /ND{noaccess def}executeonly def\n\
        /NP{noaccess put}executeonly def\n\
        /BlueValues [] def\n/MinFeature {16 16} def\n/password 5839 def\n/lenIV 4 def\n\
        2 index /CharStrings 4 dict dup begin\n"
        .to_vec();
    for (name, data) in glyphs {
        private.extend(format!("/{name} {} RD ", data.len()).bytes());
        private.extend(data);
        private.extend(b" ND\n");
    }
    private.extend(b"end\nend\nreadonly put\nnoaccess put\ndup/FontName get exch definefont pop\nmark currentfile closefile\n");
    let clear = b"%!PS-AdobeFont-1.0: Fix 001.000\n\
        11 dict begin\n/FontName /Fix def\n/PaintType 0 def\n/FontType 1 def\n\
        /FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/Encoding StandardEncoding def\n\
        /FontBBox {0 -200 600 750} readonly def\ncurrentdict end\ncurrentfile eexec\n";
    [
        &clear[..],
        &encrypt(&[&[0u8; 4][..], &private].concat(), 55665),
    ]
    .concat()
}

/// [`type1_program`] with `n` integers in a row in its clear text.
pub fn type1_integers_program(n: usize) -> Vec<u8> {
    let program = type1_program();
    let at = program.windows(4).position(|w| w == b"/Pai").unwrap();
    [&program[..at], "1 ".repeat(n).as_bytes(), &program[at..]].concat()
}

/// A Type 1 program of `clear` in its clear text and glyphs given as their names and
/// plain charstrings, none encrypted (`lenIV` -1), `/Encoding` left to `clear`.
pub fn type1_with<'a>(
    clear: &[u8],
    glyphs: impl IntoIterator<Item = (&'a str, Vec<u8>)>,
) -> Vec<u8> {
    let mut private = b"dup /Private 8 dict dup begin\n/lenIV -1 def\n\
        2 index /CharStrings 4 dict dup begin\n"
        .to_vec();
    for (name, data) in glyphs {
        private.extend(format!("/{name} {} RD ", data.len()).bytes());
        private.extend(data);
        private.extend(b" ND\n");
    }
    private.extend(b"end\nend\n");
    type1_private(clear, &private)
}

/// A Type 1 program of `clear` in its clear text and `private` encrypted after it.
pub fn type1_private(clear: &[u8], private: &[u8]) -> Vec<u8> {
    [
        &b"%!PS-AdobeFont-1.0: Fix 001.000\n11 dict begin\n/FontName /Fix def\n\
           /FontMatrix [0.001 0 0 0.001 0 0] readonly def\n"[..],
        clear,
        b"\ncurrentdict end\ncurrentfile eexec\n",
        &encrypt(&[&[0u8; 4][..], private].concat(), 55665),
    ]
    .concat()
}

/// A Type 1 charstring of a width of 500 that draws nothing.
pub fn type1_empty_glyph() -> Vec<u8> {
    [cs(&[0, 500]), vec![13, 14]].concat()
}

/// A Type 1 program of `n` glyphs, each named by `name`'s number, and an `/Encoding` of
/// `n` names, none of them a glyph's, each of which read-fonts looks for among them all.
pub fn type1_lookups_program(n: usize) -> Vec<u8> {
    let names: Vec<String> = (0..n).map(|g| format!("g{g}")).collect();
    let empty = type1_empty_glyph();
    let clear = format!("/Encoding [{}] def", "/nope ".repeat(n));
    type1_with(
        clear.as_bytes(),
        (names.iter()).map(|name| (name.as_str(), empty.clone())),
    )
}

/// A Type 1 program of `A`, `glyphs` glyphs, `B` and `C`, `A` accented `accents` times
/// over, by `seac`, of `B` and `C`: each time, read-fonts looks each of them up among all
/// the glyphs by their names, to the last.
pub fn type1_accents_program(glyphs: usize, accents: usize) -> Vec<u8> {
    let names: Vec<String> = (0..glyphs).map(|g| format!("g{g}")).collect();
    let empty = type1_empty_glyph();
    let accented = [
        cs(&[0, 500]),
        vec![13],
        [cs(&[0, 0, 0, 66, 67]), vec![12, 6]]
            .concat()
            .repeat(accents),
        vec![14],
    ]
    .concat();
    let rest = names.iter().map(|name| (name.as_str(), empty.clone()));
    let ends = [("B", empty.clone()), ("C", empty.clone())];
    type1_with(
        b"/Encoding StandardEncoding def",
        [(".notdef", empty.clone()), ("A", accented)]
            .into_iter()
            .chain(rest)
            .chain(ends),
    )
}

/// An OpenType program of [`truetype_program`]'s tables but `glyf`, [`cff_program`]
/// its `CFF `, and its `loca` empty, its record's offset past the end of
/// the program and of every other table's.
pub fn loca_past_end_program() -> Vec<u8> {
    let program = truetype_program();
    let at = |at: usize| u32::from_be_bytes(program[at..at + 4].try_into().unwrap()) as usize;
    let records =
        (0..usize::from(u16::from_be_bytes([program[4], program[5]]))).map(|i| 12 + 16 * i);
    let mut tables: Vec<([u8; 4], Vec<u8>)> = records
        .map(|r| {
            let tag: [u8; 4] = program[r..r + 4].try_into().unwrap();
            (tag, program[at(r + 8)..at(r + 8) + at(r + 12)].to_vec())
        })
        .filter(|(tag, _)| tag != b"glyf")
        .map(|(tag, table)| (tag, if &tag == b"loca" { Vec::new() } else { table }))
        .chain([(*b"CFF ", cff_program())])
        .collect();
    let mut out = sfnt_tagged(*b"OTTO", &mut tables);
    let loca = (0..tables.len())
        .map(|i| 12 + 16 * i)
        .find(|&r| &out[r..r + 4] == b"loca")
        .unwrap();
    out[loca + 8..loca + 12].copy_from_slice(&0xFFFF_FF00u32.to_be_bytes());
    out
}

/// `plain` encrypted as Type 1 encrypts it, from `key`.
fn encrypt(plain: &[u8], mut key: u16) -> Vec<u8> {
    plain
        .iter()
        .map(|&p| {
            let c = p ^ (key >> 8) as u8;
            key = (u16::from(c).wrapping_add(key))
                .wrapping_mul(52845)
                .wrapping_add(22719);
            c
        })
        .collect()
}

/// A charstring's numbers, encoded as Type 1 and Type 2 charstrings share:
/// a byte, or two, for -1131 to 1131.
fn cs(values: &[i32]) -> Vec<u8> {
    let mut out = Vec::new();
    for &v in values {
        match v {
            -107..=107 => out.push((v + 139) as u8),
            108..=1131 => out.extend([((v - 108) / 256 + 247) as u8, ((v - 108) % 256) as u8]),
            -1131..=-108 => out.extend([((-v - 108) / 256 + 251) as u8, ((-v - 108) % 256) as u8]),
            _ => panic!("a fixture's number fits two bytes"),
        }
    }
    out
}
