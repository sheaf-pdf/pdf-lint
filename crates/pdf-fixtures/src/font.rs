//! TrueType programs built by hand, for the tests of what embeds a font and the fuzz
//! seeds' writers: a program of the glyphs, characters and tables a test asks for, each
//! table as OpenType lays it out, and the SFNT or collection that holds them. A test
//! that wants a malformed program takes its tables ([`TrueType::tables`]), changes one
//! ([`set`]) and puts them together again ([`sfnt`]).

/// The tag a TrueType program's table directory begins with.
pub const TRUETYPE: [u8; 4] = 0x0001_0000u32.to_be_bytes();

/// A glyph's outline in a [`TrueType`] program, in its units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Glyph {
    /// None: a space. No data.
    Empty,
    /// A rectangle from (`x0`, `y0`) to (`x1`, `y1`), one contour of four points on
    /// the curve.
    Box(i16, i16, i16, i16),
    /// One contour through these points, each its x, its y and whether it's on the
    /// curve.
    Points(Vec<(i16, i16, bool)>),
    /// A composite of these glyphs, each moved by its x and y.
    Composite(Vec<(u16, i16, i16)>),
    /// Its data as given, malformed or not.
    Raw(Vec<u8>),
}

/// A TrueType program's parts, each as a test sets it: [`TrueType::new`] gives one of
/// a single glyph, `.notdef`, and every table but `cvt `, `fpgm` and `prep`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrueType {
    /// Its units per em, which its `head` gives.
    pub units_per_em: u16,
    /// Its glyphs, `.notdef` first, each with its advance in its units.
    pub glyphs: Vec<(Glyph, u16)>,
    /// How many of its glyphs have their advance in `hmtx`, every later one taking the
    /// last's: all of them where none is given.
    pub long_metrics: Option<u16>,
    /// What its `cmap` maps: each character's glyph.
    pub cmap: Vec<(char, u16)>,
    /// Whether its `cmap` has a (3,1) subtable of format 4, of what it maps in the
    /// Basic Multilingual Plane.
    pub format_4: bool,
    /// Whether its `cmap` has a (3,10) subtable of format 12, of all it maps.
    pub format_12: bool,
    /// Its `hhea`'s ascender and descender.
    pub ascender: i16,
    /// See [`TrueType::ascender`].
    pub descender: i16,
    /// Its `head`'s style bits: 1 bold, 2 italic.
    pub mac_style: u16,
    /// Its `OS/2` table, where it has one.
    pub os2: Option<Os2>,
    /// Its `post` table.
    pub post: Post,
    /// Its `name` table's names, each its ID and its text, written as Windows' Unicode
    /// (3,1), US English: none, and it has no `name` table.
    pub names: Vec<(u16, String)>,
    /// Whether it has a `cvt `, an `fpgm` and a `prep`, short but valid.
    pub hinting: bool,
    /// Whether its `loca` is long whatever its `glyf`'s size: it's long anyway where
    /// its glyphs' data is past what a short one reaches.
    pub long_loca: bool,
}

/// A [`TrueType`] program's `OS/2` table: what tests read of it, the rest 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Os2 {
    /// Its version, 0 to 5, which says how long it is.
    pub version: u16,
    /// `usWeightClass`: 400 regular, 700 bold.
    pub weight: u16,
    /// `sFamilyClass`: its class in the high byte, its subclass in the low.
    pub family_class: i16,
    /// `fsType`: what its licence allows.
    pub fs_type: u16,
    /// `fsSelection`: bit 7 to use its typographic heights.
    pub fs_selection: u16,
    /// `sTypoAscender`.
    pub typo_ascender: i16,
    /// `sTypoDescender`.
    pub typo_descender: i16,
    /// `usWinAscent`.
    pub win_ascent: u16,
    /// `usWinDescent`.
    pub win_descent: u16,
    /// `sxHeight`, from version 2.
    pub x_height: i16,
    /// `sCapHeight`, from version 2.
    pub cap_height: i16,
}

impl Os2 {
    /// A version 4 table for a program of `units_per_em`: regular weight, no class, no
    /// licence restriction, its heights those of [`TrueType::new`].
    pub fn new(units_per_em: u16) -> Os2 {
        let em = |v: i32| scaled(v, units_per_em);
        Os2 {
            version: 4,
            weight: 400,
            family_class: 0,
            fs_type: 0,
            fs_selection: 0,
            typo_ascender: em(800),
            typo_descender: em(-200),
            win_ascent: em(900) as u16,
            win_descent: em(250) as u16,
            x_height: em(480),
            cap_height: em(700),
        }
    }
}

/// A [`TrueType`] program's `post` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Post {
    /// Its italic angle, in degrees, as 16.16.
    pub italic_angle: i32,
    /// Whether it says its glyphs are all as wide.
    pub fixed_pitch: bool,
    /// Whether it names its glyphs (version 2), each glyph past `.notdef` `gN`, N its
    /// number; else it names none (version 3).
    pub names: bool,
}

impl TrueType {
    /// A program of `units_per_em` with one glyph, `.notdef`, a box: every table it
    /// needs, an `OS/2`, a `post` naming no glyph, and names whose PostScript name is
    /// `FixtureSans-Regular`.
    pub fn new(units_per_em: u16) -> TrueType {
        let em = |v: i32| scaled(v, units_per_em);
        TrueType {
            units_per_em,
            glyphs: vec![(Glyph::Box(em(50), 0, em(450), em(700)), em(500) as u16)],
            long_metrics: None,
            cmap: Vec::new(),
            format_4: true,
            format_12: true,
            ascender: em(800),
            descender: em(-200),
            mac_style: 0,
            os2: Some(Os2::new(units_per_em)),
            post: Post::default(),
            names: [
                (1, "Fixture Sans"),
                (2, "Regular"),
                (4, "Fixture Sans Regular"),
                (6, "FixtureSans-Regular"),
            ]
            .map(|(id, name)| (id, name.to_owned()))
            .to_vec(),
            hinting: false,
            long_loca: false,
        }
    }

    /// Adds `glyph`, its advance `advance`: its number.
    pub fn glyph(&mut self, glyph: Glyph, advance: u16) -> u16 {
        self.glyphs.push((glyph, advance));
        (self.glyphs.len() - 1) as u16
    }

    /// Maps `c` to `glyph` in its `cmap`.
    pub fn map(&mut self, c: char, glyph: u16) {
        self.cmap.push((c, glyph));
    }

    /// Its tables, each with its tag, in order of their tags.
    pub fn tables(&self) -> Vec<([u8; 4], Vec<u8>)> {
        let (glyf, loca, long) = self.glyf();
        let mut tables = vec![
            (*b"cmap", self.cmap_table()),
            (*b"glyf", glyf),
            (*b"head", self.head(long)),
            (*b"hhea", self.hhea()),
            (*b"hmtx", self.hmtx()),
            (*b"loca", loca),
            (*b"maxp", self.maxp()),
            (*b"post", self.post_table()),
        ];
        if let Some(os2) = &self.os2 {
            tables.push((*b"OS/2", os2_table(os2)));
        }
        if !self.names.is_empty() {
            let names: Vec<(u16, &str)> = self.names.iter().map(|(i, n)| (*i, &**n)).collect();
            tables.push((*b"name", names_table(&names)));
        }
        if self.hinting {
            tables.push((*b"cvt ", words(&[0, 50, 100])));
            // An empty function 0, defined; then a number pushed and popped.
            tables.push((*b"fpgm", vec![0xB0, 0x00, 0x2C, 0x2D]));
            tables.push((*b"prep", vec![0xB0, 0x00, 0x21]));
        }
        tables.sort_by_key(|t| t.0);
        tables
    }

    /// The program: its tables in an SFNT.
    pub fn build(&self) -> Vec<u8> {
        sfnt(TRUETYPE, &self.tables())
    }

    /// Its `glyf` and `loca`, and whether `loca` is long.
    fn glyf(&self) -> (Vec<u8>, Vec<u8>, bool) {
        let mut glyf = Vec::new();
        let mut offsets = Vec::with_capacity(self.glyphs.len() + 1);
        for (glyph, _) in &self.glyphs {
            offsets.push(glyf.len());
            glyf.extend(self.glyph_data(glyph));
            glyf.resize(glyf.len().next_multiple_of(4), 0);
        }
        offsets.push(glyf.len());
        let long = self.long_loca || glyf.len() > 0x1FFFE;
        let loca = match long {
            true => offsets
                .iter()
                .flat_map(|&o| (o as u32).to_be_bytes())
                .collect(),
            false => offsets
                .iter()
                .flat_map(|&o| ((o / 2) as u16).to_be_bytes())
                .collect(),
        };
        (glyf, loca, long)
    }

    /// `glyph`'s data in `glyf`.
    fn glyph_data(&self, glyph: &Glyph) -> Vec<u8> {
        match glyph {
            Glyph::Empty => Vec::new(),
            &Glyph::Box(x0, y0, x1, y1) => simple(&[
                (x0, y0, true),
                (x0, y1, true),
                (x1, y1, true),
                (x1, y0, true),
            ]),
            Glyph::Points(points) => simple(points),
            Glyph::Composite(components) => {
                let [x0, y0, x1, y1] = self.bbox(glyph, 0).unwrap_or_default();
                let mut data = words(&[-1, x0, y0, x1, y1]);
                for (i, &(component, dx, dy)) in components.iter().enumerate() {
                    // Its offsets words, and x and y values; more after it but the last.
                    let more = if i + 1 < components.len() { 0x0020 } else { 0 };
                    data.extend((0x0003u16 | more).to_be_bytes());
                    data.extend(component.to_be_bytes());
                    data.extend(words(&[dx, dy]));
                }
                data
            }
            Glyph::Raw(data) => data.clone(),
        }
    }

    /// The box `glyph`'s outline fits in, its composites' components followed `depth`
    /// deep at most: none for one that draws nothing.
    fn bbox(&self, glyph: &Glyph, depth: u8) -> Option<[i16; 4]> {
        match glyph {
            &Glyph::Box(x0, y0, x1, y1) => Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]),
            Glyph::Points(points) => {
                let xs = points.iter().map(|p| p.0);
                let ys = points.iter().map(|p| p.1);
                Some([xs.clone().min()?, ys.clone().min()?, xs.max()?, ys.max()?])
            }
            Glyph::Composite(components) if depth < 16 => components
                .iter()
                .filter_map(|&(c, dx, dy)| {
                    let [x0, y0, x1, y1] = self.bbox(&self.glyphs.get(c as usize)?.0, depth + 1)?;
                    Some([
                        x0.saturating_add(dx),
                        y0.saturating_add(dy),
                        x1.saturating_add(dx),
                        y1.saturating_add(dy),
                    ])
                })
                .reduce(|a, b| {
                    [
                        a[0].min(b[0]),
                        a[1].min(b[1]),
                        a[2].max(b[2]),
                        a[3].max(b[3]),
                    ]
                }),
            _ => None,
        }
    }

    /// Its `head`, its `loca` long or not.
    fn head(&self, long: bool) -> Vec<u8> {
        let all = self.glyphs.iter().filter_map(|(g, _)| self.bbox(g, 0));
        let [x0, y0, x1, y1] = all
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            })
            .unwrap_or_default();
        let mut head = Vec::with_capacity(54);
        head.extend(0x0001_0000u32.to_be_bytes());
        head.extend(0x0001_0000u32.to_be_bytes());
        head.extend(0u32.to_be_bytes());
        head.extend(0x5F0F_3CF5u32.to_be_bytes());
        head.extend(0x0001u16.to_be_bytes());
        head.extend(self.units_per_em.to_be_bytes());
        head.extend([0; 16]);
        head.extend(words(&[x0, y0, x1, y1]));
        head.extend(self.mac_style.to_be_bytes());
        head.extend(words(&[8, 2, i16::from(long), 0]));
        head
    }

    /// Its `hhea`.
    fn hhea(&self) -> Vec<u8> {
        let most = self.glyphs.iter().map(|g| g.1).max().unwrap_or(0);
        let mut hhea = 0x0001_0000u32.to_be_bytes().to_vec();
        hhea.extend(words(&[self.ascender, self.descender, 0]));
        hhea.extend(most.to_be_bytes());
        hhea.extend(words(&[0, 0, most as i16, 1, 0, 0, 0, 0, 0, 0, 0]));
        hhea.extend(self.long().to_be_bytes());
        hhea
    }

    /// How many advances its `hmtx` holds.
    fn long(&self) -> u16 {
        let all = self.glyphs.len() as u16;
        self.long_metrics.map_or(all, |n| n.min(all))
    }

    /// Its `hmtx`: each glyph's advance and left side bearing, its box's left.
    fn hmtx(&self) -> Vec<u8> {
        let long = usize::from(self.long());
        let mut hmtx = Vec::new();
        for (i, (glyph, advance)) in self.glyphs.iter().enumerate() {
            if i < long {
                hmtx.extend(advance.to_be_bytes());
            }
            let left = self.bbox(glyph, 0).map_or(0, |b| b[0]);
            hmtx.extend(left.to_be_bytes());
        }
        hmtx
    }

    /// Its `maxp`, of version 1.
    fn maxp(&self) -> Vec<u8> {
        let components = self.glyphs.iter().map(|(g, _)| match g {
            Glyph::Composite(c) => c.len(),
            _ => 0,
        });
        let components = components.max().unwrap_or(0).min(0xFFFF) as u16;
        let points = self.glyphs.iter().map(|(g, _)| match g {
            Glyph::Points(p) => p.len(),
            _ => 4,
        });
        let points = points.max().unwrap_or(4).min(0xFFFF) as u16;
        let mut maxp = 0x0001_0000u32.to_be_bytes().to_vec();
        maxp.extend((self.glyphs.len() as u16).to_be_bytes());
        let functions = u16::from(self.hinting);
        for value in [
            points, 1, points, 1, 2, 0, 0, functions, 0, 8, 0, components, 2,
        ] {
            maxp.extend(value.to_be_bytes());
        }
        maxp
    }

    /// Its `cmap`: a (3,1) subtable of format 4, a segment a character, and a (3,10)
    /// one of format 12, a group a character, as it has them.
    fn cmap_table(&self) -> Vec<u8> {
        let mut map = self.cmap.clone();
        map.sort();
        let mut subtables = Vec::new();
        if self.format_4 {
            let bmp: Vec<(u16, u16)> = map
                .iter()
                .filter_map(|&(c, g)| Some((u16::try_from(u32::from(c)).ok()?, g)))
                .collect();
            subtables.push((1u16, format_4(&bmp)));
        }
        if self.format_12 {
            subtables.push((10, format_12(&map)));
        }
        let mut cmap = words(&[0, subtables.len() as i16]);
        let mut offset = 4 + 8 * subtables.len();
        for (encoding, subtable) in &subtables {
            cmap.extend(3u16.to_be_bytes());
            cmap.extend(encoding.to_be_bytes());
            cmap.extend((offset as u32).to_be_bytes());
            offset += subtable.len();
        }
        for (_, subtable) in subtables {
            cmap.extend(subtable);
        }
        cmap
    }

    /// Its `post`, of version 2 where it names its glyphs, else 3.
    fn post_table(&self) -> Vec<u8> {
        let version: u32 = if self.post.names {
            0x0002_0000
        } else {
            0x0003_0000
        };
        let mut post = version.to_be_bytes().to_vec();
        post.extend(self.post.italic_angle.to_be_bytes());
        post.extend(words(&[-100, 50]));
        post.extend(u32::from(self.post.fixed_pitch).to_be_bytes());
        post.extend([0; 16]);
        if self.post.names {
            let count = self.glyphs.len();
            post.extend((count as u16).to_be_bytes());
            post.extend(0u16.to_be_bytes());
            for i in 1..count {
                post.extend(((257 + i) as u16).to_be_bytes());
            }
            for i in 1..count {
                let name = format!("g{i}");
                post.push(name.len() as u8);
                post.extend(name.bytes());
            }
        }
        post
    }
}

/// `v`, a length in thousandths of an em, in a program's `units_per_em`, rounded.
fn scaled(v: i32, units_per_em: u16) -> i16 {
    (f64::from(v) * f64::from(units_per_em) / 1000.0).round() as i16
}

/// Big-endian 16-bit numbers.
fn words(values: &[i16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// A simple glyph of one contour through `points`, each its x, its y and whether
/// it's on the curve, its coordinates each a word.
pub fn simple(points: &[(i16, i16, bool)]) -> Vec<u8> {
    let xs = points.iter().map(|p| p.0);
    let ys = points.iter().map(|p| p.1);
    let bbox = [
        xs.clone().min(),
        ys.clone().min(),
        xs.clone().max(),
        ys.clone().max(),
    ]
    .map(Option::unwrap_or_default);
    let mut glyph = words(&[1, bbox[0], bbox[1], bbox[2], bbox[3]]);
    glyph.extend((points.len() as u16).saturating_sub(1).to_be_bytes());
    glyph.extend(0u16.to_be_bytes());
    glyph.extend(points.iter().map(|p| u8::from(p.2)));
    let mut last = (0i16, 0i16);
    let mut x = Vec::new();
    let mut y = Vec::new();
    for &(px, py, _) in points {
        x.push(px.wrapping_sub(last.0));
        y.push(py.wrapping_sub(last.1));
        last = (px, py);
    }
    glyph.extend(words(&x));
    glyph.extend(words(&y));
    glyph
}

/// A `cmap` subtable of format 4 mapping each code point to its glyph, a segment each,
/// then the last segment, U+FFFF to `.notdef`.
fn format_4(map: &[(u16, u16)]) -> Vec<u8> {
    let segments = map.len() + 1;
    let ends: Vec<u16> = map.iter().map(|m| m.0).chain([0xFFFF]).collect();
    let deltas: Vec<u16> = map
        .iter()
        .map(|m| m.1.wrapping_sub(m.0))
        .chain([1])
        .collect();
    let range = 2 * (1usize << segments.ilog2());
    let header = [
        4,
        (16 + segments * 8) as u16,
        0,
        (segments * 2) as u16,
        range as u16,
        range.ilog2() as u16 - 1,
        (segments * 2 - range) as u16,
    ];
    let mut table = Vec::new();
    for v in header
        .iter()
        .chain(&ends)
        .chain([&0])
        .chain(&ends)
        .chain(&deltas)
    {
        table.extend(v.to_be_bytes());
    }
    table.extend(vec![0; segments * 2]);
    table
}

/// A `cmap` subtable of format 12 mapping each character to its glyph, a group each.
fn format_12(map: &[(char, u16)]) -> Vec<u8> {
    let mut table = words(&[12, 0]);
    table.extend(((16 + 12 * map.len()) as u32).to_be_bytes());
    table.extend(0u32.to_be_bytes());
    table.extend((map.len() as u32).to_be_bytes());
    for &(c, glyph) in map {
        table.extend(u32::from(c).to_be_bytes());
        table.extend(u32::from(c).to_be_bytes());
        table.extend(u32::from(glyph).to_be_bytes());
    }
    table
}

/// An `OS/2` table, as long as its version makes it.
fn os2_table(os2: &Os2) -> Vec<u8> {
    let mut table = os2.version.to_be_bytes().to_vec();
    table.extend(words(&[0]));
    table.extend(os2.weight.to_be_bytes());
    table.extend(5u16.to_be_bytes());
    table.extend(os2.fs_type.to_be_bytes());
    table.extend([0; 20]);
    table.extend(os2.family_class.to_be_bytes());
    table.extend([0; 10 + 16 + 4]);
    table.extend(os2.fs_selection.to_be_bytes());
    table.extend(words(&[0x20, -1]));
    table.extend(words(&[os2.typo_ascender, os2.typo_descender, 0]));
    table.extend(os2.win_ascent.to_be_bytes());
    table.extend(os2.win_descent.to_be_bytes());
    if os2.version >= 1 {
        table.extend([0; 8]);
    }
    if os2.version >= 2 {
        table.extend(words(&[os2.x_height, os2.cap_height, 0, 0x20, 0]));
    }
    if os2.version >= 5 {
        table.extend([0; 4]);
    }
    table
}

/// A `name` table of `names`, each its ID and its text, written as Windows' Unicode
/// (3,1), US English, its text UTF-16BE.
pub fn names_table(names: &[(u16, &str)]) -> Vec<u8> {
    let mut records = Vec::new();
    let mut storage = Vec::new();
    for &(id, name) in names {
        let text: Vec<u8> = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
        records.push([3, 1, 0x409, id, text.len() as u16, storage.len() as u16]);
        storage.extend(text);
    }
    records.sort();
    name_table(&records, &storage)
}

/// A `name` table of version 0 of `records`, each its platform, its encoding, its
/// language, its name's ID, its length and its offset in `storage`, as given; `storage`
/// after them, where its offset can say so, else at 65,535, among them.
pub fn name_table(records: &[[u16; 6]], storage: &[u8]) -> Vec<u8> {
    let count = records.len() as u16;
    let mut table = 0u16.to_be_bytes().to_vec();
    table.extend(count.to_be_bytes());
    table.extend(((6 + 12 * records.len()).min(0xFFFF) as u16).to_be_bytes());
    for record in records {
        for field in record {
            table.extend(field.to_be_bytes());
        }
    }
    table.extend(storage);
    table
}

/// `tables` with the one tagged `tag` made `data`, or `data` added in order of the
/// tags where it hasn't one.
pub fn set(tables: &mut Vec<([u8; 4], Vec<u8>)>, tag: [u8; 4], data: Vec<u8>) {
    match tables.binary_search_by_key(&tag, |t| t.0) {
        Ok(at) => tables[at].1 = data,
        Err(at) => tables.insert(at, (tag, data)),
    }
}

/// `tables` with the one tagged `tag` taken out.
pub fn remove(tables: &mut Vec<([u8; 4], Vec<u8>)>, tag: [u8; 4]) {
    tables.retain(|t| t.0 != tag);
}

/// An SFNT whose table directory begins with `tag` ([`TRUETYPE`], or `OTTO` for CFF
/// outlines), of `tables` in the order given, each padded to four bytes: in order of
/// their tags, as OpenType asks, where they're given so.
pub fn sfnt(tag: [u8; 4], tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len() as u16;
    let range = if n == 0 { 0 } else { 16 * (1u16 << n.ilog2()) };
    let selector = if n == 0 { 0 } else { n.ilog2() as u16 };
    let mut out = tag.to_vec();
    for v in [n, range, selector, (16 * n).saturating_sub(range)] {
        out.extend(v.to_be_bytes());
    }
    let mut offset = 12 + 16 * tables.len();
    let mut data = Vec::new();
    for (tag, table) in tables {
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

/// A collection (`ttcf`, version 1.0) of `fonts`, each an SFNT whose tables' offsets
/// are its own, moved to where it's put.
pub fn collection(fonts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"ttcf".to_vec();
    out.extend(0x0001_0000u32.to_be_bytes());
    out.extend((fonts.len() as u32).to_be_bytes());
    let mut at = 12 + 4 * fonts.len();
    let mut faces = Vec::new();
    for font in fonts {
        out.extend((at as u32).to_be_bytes());
        faces.extend(moved(font, at));
        faces.resize(faces.len().next_multiple_of(4), 0);
        at = 12 + 4 * fonts.len() + faces.len();
    }
    out.extend(faces);
    out
}

/// `font`, an SFNT, its tables' offsets moved on by `by`.
pub fn moved(font: &[u8], by: usize) -> Vec<u8> {
    let mut font = font.to_vec();
    let n = u16::from_be_bytes([font[4], font[5]]) as usize;
    for i in 0..n {
        let at = 12 + 16 * i + 8;
        let offset = u32::from_be_bytes([font[at], font[at + 1], font[at + 2], font[at + 3]]);
        font[at..at + 4].copy_from_slice(&(offset + by as u32).to_be_bytes());
    }
    font
}

/// A program of every table [`sample`] describes, its coordinates and advances in
/// thousandths of an em scaled to `units_per_em`: Latin letters, two composites, Hebrew
/// and Arabic letters, Chinese characters, and a character past the Basic Multilingual
/// Plane, each glyph its own box, every one of its characters on a glyph of its own. Its
/// `cmap` has both subtables, and it has hinting tables.
///
/// | glyph | character | outline | advance |
/// |---|---|---|---|
/// | 0 | `.notdef` | box 50..450 × 0..700 | 500 |
/// | 1 | space | none | 250 |
/// | 2 | `A` | box 10..590 × 0..700 | 600 |
/// | 3 | `g` | box 30..470 × -200..480 | 500 |
/// | 4 | `H` | box 60..640 × 0..700 | 700 |
/// | 5 | `¨` U+00A8 | box 150..450 × 760..860 | 600 |
/// | 6 | `Ä` U+00C4 | glyphs 2 and 5 | 600 |
/// | 7 | `¯` U+00AF | box 120..480 × 900..960 | 600 |
/// | 8 | `Ǟ` U+01DE | glyphs 6 and 7 | 600 |
/// | 9 to 12 | `ש` `ל` `ו` `ם` U+05E9, U+05DC, U+05D5, U+05DD | boxes | 660, 520, 260, 610 |
/// | 13 to 16 | `س` `ل` `ا` `م` U+0633, U+0644, U+0627, U+0645 | boxes | 860, 480, 240, 520 |
/// | 17 to 299 | none | none | 500 |
/// | 300 to 303 | `中` `文` `世` `界` U+4E2D, U+6587, U+4E16, U+754C | boxes | 1000 |
/// | 304 to 319 | none | none | 500 |
/// | 320 | `𝄞` U+1D11E | box 20..560 × -300..1100 | 580 |
pub fn sample(units_per_em: u16) -> TrueType {
    let em = |v: i32| scaled(v, units_per_em);
    let mut font = TrueType::new(units_per_em);
    font.hinting = true;
    let boxed = |x0, y0, x1, y1| Glyph::Box(em(x0), em(y0), em(x1), em(y1));
    let glyphs: [(char, Glyph, i32); 16] = [
        (' ', Glyph::Empty, 250),
        ('A', boxed(10, 0, 590, 700), 600),
        ('g', boxed(30, -200, 470, 480), 500),
        ('H', boxed(60, 0, 640, 700), 700),
        ('\u{A8}', boxed(150, 760, 450, 860), 600),
        ('\u{C4}', Glyph::Composite(vec![(2, 0, 0), (5, 0, 0)]), 600),
        ('\u{AF}', boxed(120, 900, 480, 960), 600),
        ('\u{1DE}', Glyph::Composite(vec![(6, 0, 0), (7, 0, 0)]), 600),
        ('\u{5E9}', boxed(40, 0, 620, 520), 660),
        ('\u{5DC}', boxed(50, 0, 470, 760), 520),
        ('\u{5D5}', boxed(60, 0, 200, 520), 260),
        ('\u{5DD}', boxed(50, 0, 560, 520), 610),
        ('\u{633}', boxed(30, -250, 830, 300), 860),
        ('\u{644}', boxed(40, -250, 440, 760), 480),
        ('\u{627}', boxed(70, 0, 170, 760), 240),
        ('\u{645}', boxed(40, -400, 480, 300), 520),
    ];
    for (c, glyph, advance) in glyphs {
        let n = font.glyph(glyph, em(advance) as u16);
        font.map(c, n);
    }
    while font.glyphs.len() < 300 {
        font.glyph(Glyph::Empty, em(500) as u16);
    }
    let cjk = [
        ('\u{4E2D}', boxed(80, -80, 920, 840)),
        ('\u{6587}', boxed(40, -60, 960, 820)),
        ('\u{4E16}', boxed(60, -40, 940, 800)),
        ('\u{754C}', boxed(70, -70, 930, 830)),
    ];
    for (c, glyph) in cjk {
        let n = font.glyph(glyph, em(1000) as u16);
        font.map(c, n);
    }
    while font.glyphs.len() < 320 {
        font.glyph(Glyph::Empty, em(500) as u16);
    }
    let clef = font.glyph(boxed(20, -300, 560, 1100), em(580) as u16);
    font.map('\u{1D11E}', clef);
    font
}

/// A program of Latin text, without CJK or emoji, its coordinates and advances in
/// thousandths of an em scaled to `units_per_em`: printable ASCII, the letters `é`
/// U+00E9, `ç` U+00E7, `Ö` U+00D6, `Ł` U+0141 and `ễ` U+1EC5, and a combining acute
/// accent U+0301, each glyph its own box, in that order from glyph 1, every one of its
/// characters on a glyph of its own. A program that draws "Łukasz Nguyễn Ölçer", for a
/// font a caller falls back to where WinAnsiEncoding has no code. Its `hhea` heights
/// are 850 and -250, its `OS/2`'s typographic ones 780 and -210, not used, and its
/// win heights 900 and 260; its box, its glyphs', is -230..950 high.
///
/// | character | outline | advance |
/// |---|---|---|
/// | space | none | 260 |
/// | `g`, `j`, `p`, `q`, `y` | box 30..(advance - 30) × -200..480 | 420 + (code × 37 mod 260) |
/// | other lowercase ASCII | box 30..(advance - 30) × 0..500 | 420 + (code × 37 mod 260) |
/// | other printable ASCII | box 30..(advance - 30) × 0..700 | 420 + (code × 37 mod 260) |
/// | `é` U+00E9 | box 40..520 × -10..720 | 560 |
/// | `ç` U+00E7 | box 40..470 × -230..500 | 520 |
/// | `Ö` U+00D6 | box 40..680 × 0..880 | 720 |
/// | `Ł` U+0141 | box 20..560 × 0..700 | 590 |
/// | `ễ` U+1EC5 | box 40..520 × -10..950 | 560 |
/// | U+0301 | box 100..300 × 560..720 | 0 |
pub fn latin(units_per_em: u16) -> TrueType {
    let em = |v: i32| scaled(v, units_per_em);
    let mut font = TrueType::new(units_per_em);
    font.ascender = em(850);
    font.descender = em(-250);
    if let Some(os2) = font.os2.as_mut() {
        os2.typo_ascender = em(780);
        os2.typo_descender = em(-210);
        os2.win_ascent = em(900) as u16;
        os2.win_descent = em(260) as u16;
    }
    let boxed = |x0, y0, x1, y1| Glyph::Box(em(x0), em(y0), em(x1), em(y1));
    let ascii = (0x20u8..0x7F).map(char::from);
    let letters = [
        '\u{E9}', '\u{E7}', '\u{D6}', '\u{141}', '\u{1EC5}', '\u{301}',
    ];
    for c in ascii.chain(letters) {
        let advance = 420 + (u32::from(c) * 37) % 260;
        let right = advance as i32 - 30;
        let (glyph, advance) = match c {
            ' ' => (Glyph::Empty, 260),
            '\u{E9}' => (boxed(40, -10, 520, 720), 560),
            '\u{E7}' => (boxed(40, -230, 470, 500), 520),
            '\u{D6}' => (boxed(40, 0, 680, 880), 720),
            '\u{141}' => (boxed(20, 0, 560, 700), 590),
            '\u{1EC5}' => (boxed(40, -10, 520, 950), 560),
            '\u{301}' => (boxed(100, 560, 300, 720), 0),
            'g' | 'j' | 'p' | 'q' | 'y' => (boxed(30, -200, right, 480), advance),
            c if c.is_ascii_lowercase() => (boxed(30, 0, right, 500), advance),
            _ => (boxed(30, 0, right, 700), advance),
        };
        let n = font.glyph(glyph, em(advance as i32) as u16);
        font.map(c, n);
    }
    font
}
