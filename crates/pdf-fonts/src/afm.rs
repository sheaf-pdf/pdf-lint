//! Adobe's Core 14 font metrics (`assets/afm`, with Adobe's `MustRead.html`, and
//! each file with its notice in it): each glyph's advance and box, and the font's heights.

use std::collections::HashMap;

/// One font's metrics.
#[derive(Debug)]
pub(crate) struct Afm {
    /// Each glyph's advance and box, per 1000 em, by name.
    glyphs: HashMap<&'static str, (u16, [i16; 4])>,
    /// `Ascender` and `Descender`, where it gives them: Symbol's and
    /// ZapfDingbats' don't.
    pub ascender: Option<i16>,
    pub descender: Option<i16>,
    /// `FontBBox`: left, bottom, right, top.
    pub bbox: [i16; 4],
}

impl Afm {
    /// The metrics in `text`, an AFM file: its header's `Ascender`,
    /// `Descender` and `FontBBox`, and its `C … ; WX … ; N … ; B … ;`
    /// lines. A line without a name or a whole width is passed over, and
    /// a glyph without a box has an empty one. `text` is only ever this crate's
    /// own assets, so nothing caps it.
    pub fn parse(text: &'static str) -> Afm {
        let mut afm = Afm {
            glyphs: HashMap::new(),
            ascender: None,
            descender: None,
            bbox: [0; 4],
        };
        let mut lines = text.lines();
        for line in lines.by_ref() {
            let mut words = line.split_whitespace();
            match words.next() {
                Some("StartCharMetrics") => break,
                Some("Ascender") => afm.ascender = words.next().and_then(|w| w.parse().ok()),
                Some("Descender") => afm.descender = words.next().and_then(|w| w.parse().ok()),
                Some("FontBBox") => afm.bbox = numbers(words).unwrap_or_default(),
                _ => {}
            }
        }
        for line in lines.take_while(|l| !l.starts_with("EndCharMetrics")) {
            let (mut width, mut name, mut bbox) = (None, None, None);
            for field in line.split(';') {
                let mut words = field.split_whitespace();
                match words.next() {
                    Some("WX") => width = words.next().and_then(|w| w.parse::<u16>().ok()),
                    Some("N") => name = words.next(),
                    Some("B") => bbox = numbers(words),
                    _ => {}
                }
            }
            let (Some(name), Some(width)) = (name, width) else {
                continue;
            };
            afm.glyphs.insert(name, (width, bbox.unwrap_or_default()));
        }
        afm
    }

    /// The advance of glyph `name`, per 1000 em; None for a glyph the
    /// font hasn't.
    pub fn width(&self, name: &str) -> Option<u16> {
        self.glyphs.get(name).map(|&(width, _)| width)
    }

    /// The box glyph `name`'s outline fits in, per 1000 em: left, bottom,
    /// right, top. None for a glyph the font hasn't.
    pub fn glyph_box(&self, name: &str) -> Option<[i16; 4]> {
        self.glyphs.get(name).map(|&(_, bbox)| bbox)
    }

    /// How many glyphs it has.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }
}

/// The first four of `words`, whole numbers.
fn numbers<'a>(mut words: impl Iterator<Item = &'a str>) -> Option<[i16; 4]> {
    let mut out = [0; 4];
    for n in &mut out {
        *n = words.next()?.parse().ok()?;
    }
    Some(out)
}
