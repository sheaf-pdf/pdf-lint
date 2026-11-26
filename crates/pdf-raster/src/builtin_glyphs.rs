//! Built-in vector fallback contours for unembedded fonts (`Std14` Latin/Symbol
//! and `ZapfDingbats` form/annotation marks) when no external [`crate::FontFallback`]
//! face supplies an outline.
//!
//! Emits closed [`OutlineVerb`] contours in `1000.0`-unit glyph space scaled to
//! the target glyph's horizontal advance so text width, alignment, comb cells,
//! fill, stroke, and text clipping (`4..=7 Tr`) all work out of the box with
//! zero external `.ttf` binary assets.

use pdf_fonts::{FontStyle, GlyphBox, OutlineVerb, Std14};

/// Emits closed [`OutlineVerb`] contours for `ch` (or ZapfDingbats `code` when
/// `std14 == Some(Std14::ZapfDingbats)`), scaled to `target_width` glyph units.
///
/// Returns `true` if `ch` is whitespace (emitting zero contours) or a visible
/// fallback glyph was emitted.
pub(crate) fn emit_builtin_glyph(
    ch: Option<char>,
    code: u32,
    target_width: f32,
    std14: Option<Std14>,
    style: FontStyle,
    glyph_bbox: Option<GlyphBox>,
    out: &mut Vec<OutlineVerb>,
) -> bool {
    if std14 == Some(Std14::ZapfDingbats) {
        return emit_dingbat(code, glyph_bbox, target_width, out);
    }
    let c = ch.unwrap_or_else(|| {
        if (0x20..=0x7E).contains(&code) {
            char::from_u32(code).unwrap_or('?')
        } else {
            '?'
        }
    });
    if c.is_whitespace() || c == '\u{00A0}' || c == '\u{00AD}' {
        return true;
    }
    // Check common Dingbat/Unicode symbols directly before stroke glyphs.
    match c {
        '\u{2713}' | '\u{2714}' => {
            return emit_dingbat(u32::from(b'4'), glyph_bbox, target_width, out);
        }
        '\u{2717}' | '\u{2718}' => {
            return emit_dingbat(u32::from(b'8'), glyph_bbox, target_width, out);
        }
        '\u{25CF}' | '\u{2022}' => {
            return emit_dingbat(u32::from(b'l'), glyph_bbox, target_width, out);
        }
        '\u{25C6}' => return emit_dingbat(u32::from(b'u'), glyph_bbox, target_width, out),
        '\u{25A0}' => return emit_dingbat(u32::from(b'n'), glyph_bbox, target_width, out),
        '\u{2605}' => return emit_dingbat(u32::from(b'H'), glyph_bbox, target_width, out),
        _ => {}
    }

    let base_ascii = fold_to_ascii(c);
    let nominal_w = 600.0f32;
    let sx = if target_width > 10.0 {
        (target_width / nominal_w).clamp(0.25, 3.0)
    } else {
        1.0
    };
    let half_stem = if style.bold { 56.0 } else { 38.0 };
    let slant = if style.italic { 0.19 } else { 0.0 };

    let mut b = StrokeBuilder {
        sx,
        half_stem,
        slant,
        out,
    };
    draw_ascii_glyph(base_ascii, &mut b);
    true
}

fn emit_dingbat(
    code: u32,
    bbox: Option<GlyphBox>,
    target_width: f32,
    out: &mut Vec<OutlineVerb>,
) -> bool {
    let b = bbox.unwrap_or_else(|| {
        let w = if target_width > 0.0 {
            target_width
        } else {
            750.0
        };
        GlyphBox::new(50.0, 0.0, (w - 50.0).max(100.0), 700.0)
    });
    let (x0, y0, x1, y1) = (b.x0, b.y0, b.x1, b.y1);
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let (cx, cy) = (x0 + w * 0.5, y0 + h * 0.5);

    match code as u8 {
        b' ' => true,
        // Check mark (`4` / `3`)
        b'3' | b'4' => {
            let t = w.min(h) * 0.11;
            push_quad_stroke(
                out,
                (x0 + w * 0.08, y0 + h * 0.42),
                (x0 + w * 0.36, y0 + h * 0.12),
                t,
            );
            push_quad_stroke(
                out,
                (x0 + w * 0.34, y0 + h * 0.12),
                (x0 + w * 0.94, y0 + h * 0.88),
                t,
            );
            true
        }
        // Cross (`7` / `8` / `5` / `6`)
        b'5' | b'6' | b'7' | b'8' => {
            let t = w.min(h) * 0.10;
            push_quad_stroke(
                out,
                (x0 + w * 0.10, y0 + h * 0.10),
                (x1 - w * 0.10, y1 - h * 0.10),
                t,
            );
            push_quad_stroke(
                out,
                (x0 + w * 0.10, y1 - h * 0.10),
                (x1 - w * 0.10, y0 + h * 0.10),
                t,
            );
            true
        }
        // Filled circle (`l`)
        b'l' => {
            let (rx, ry) = (w * 0.48, h * 0.48);
            push_ellipse(out, cx, cy, rx, ry);
            true
        }
        // Filled diamond (`u`)
        b'u' => {
            out.push(OutlineVerb::MoveTo(cx, y1));
            out.push(OutlineVerb::LineTo(x1, cy));
            out.push(OutlineVerb::LineTo(cx, y0));
            out.push(OutlineVerb::LineTo(x0, cy));
            out.push(OutlineVerb::Close);
            true
        }
        // Star (`H`)
        b'H' => {
            let r_outer = w.min(h) * 0.50;
            let r_inner = r_outer * 0.40;
            for i in 0..10 {
                let angle = std::f32::consts::FRAC_PI_2 + (i as f32) * std::f32::consts::PI / 5.0;
                let r = if i % 2 == 0 { r_outer } else { r_inner };
                let px = cx + r * angle.cos();
                let py = cy + r * angle.sin();
                if i == 0 {
                    out.push(OutlineVerb::MoveTo(px, py));
                } else {
                    out.push(OutlineVerb::LineTo(px, py));
                }
            }
            out.push(OutlineVerb::Close);
            true
        }
        // Filled square (`n`) and general fallback symbol box
        _ => {
            out.push(OutlineVerb::MoveTo(x0, y0));
            out.push(OutlineVerb::LineTo(x1, y0));
            out.push(OutlineVerb::LineTo(x1, y1));
            out.push(OutlineVerb::LineTo(x0, y1));
            out.push(OutlineVerb::Close);
            true
        }
    }
}

fn push_ellipse(out: &mut Vec<OutlineVerb>, cx: f32, cy: f32, rx: f32, ry: f32) {
    const K: f32 = 0.552_284_8;
    let (kx, ky) = (rx * K, ry * K);
    out.push(OutlineVerb::MoveTo(cx + rx, cy));
    out.push(OutlineVerb::CurveTo(
        cx + rx,
        cy + ky,
        cx + kx,
        cy + ry,
        cx,
        cy + ry,
    ));
    out.push(OutlineVerb::CurveTo(
        cx - kx,
        cy + ry,
        cx - rx,
        cy + ky,
        cx - rx,
        cy,
    ));
    out.push(OutlineVerb::CurveTo(
        cx - rx,
        cy - ky,
        cx - kx,
        cy - ry,
        cx,
        cy - ry,
    ));
    out.push(OutlineVerb::CurveTo(
        cx + kx,
        cy - ry,
        cx + rx,
        cy - ky,
        cx + rx,
        cy,
    ));
    out.push(OutlineVerb::Close);
}

fn push_quad_stroke(out: &mut Vec<OutlineVerb>, p0: (f32, f32), p1: (f32, f32), half_w: f32) {
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let len = dx.hypot(dy);
    if len < 1e-3 {
        out.push(OutlineVerb::MoveTo(p0.0 - half_w, p0.1 - half_w));
        out.push(OutlineVerb::LineTo(p0.0 + half_w, p0.1 - half_w));
        out.push(OutlineVerb::LineTo(p0.0 + half_w, p0.1 + half_w));
        out.push(OutlineVerb::LineTo(p0.0 - half_w, p0.1 + half_w));
        out.push(OutlineVerb::Close);
        return;
    }
    let (nx, ny) = (-dy / len * half_w, dx / len * half_w);
    // Counterclockwise quad so non-zero winding unions positive area cleanly.
    out.push(OutlineVerb::MoveTo(p0.0 - nx, p0.1 - ny));
    out.push(OutlineVerb::LineTo(p1.0 - nx, p1.1 - ny));
    out.push(OutlineVerb::LineTo(p1.0 + nx, p1.1 + ny));
    out.push(OutlineVerb::LineTo(p0.0 + nx, p0.1 + ny));
    out.push(OutlineVerb::Close);
}

struct StrokeBuilder<'a> {
    sx: f32,
    half_stem: f32,
    slant: f32,
    out: &'a mut Vec<OutlineVerb>,
}

impl StrokeBuilder<'_> {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.sx + y * self.slant, y)
    }

    fn seg(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let p0 = self.map(x0, y0);
        let p1 = self.map(x1, y1);
        push_quad_stroke(self.out, p0, p1, self.half_stem);
    }

    fn poly(&mut self, pts: &[(f32, f32)]) {
        for pair in pts.windows(2) {
            self.seg(pair[0].0, pair[0].1, pair[1].0, pair[1].1);
        }
    }

    fn dot(&mut self, x: f32, y: f32) {
        let (px, py) = self.map(x, y);
        let r = self.half_stem * 1.25;
        push_ellipse(self.out, px, py, r, r);
    }
}

fn fold_to_ascii(c: char) -> u8 {
    if c.is_ascii() {
        return c as u8;
    }
    match c {
        'À'..='Å' | 'Ā' => b'A',
        'Æ' => b'A',
        'Ç' | 'Ć' | 'Č' => b'C',
        'È'..='Ë' | 'Ē' => b'E',
        'Ì'..='Ï' | 'Ī' => b'I',
        'Ñ' | 'Ń' => b'N',
        'Ò'..='Ö' | 'Ø' | 'Ō' => b'O',
        'Ù'..='Ü' | 'Ū' => b'U',
        'Ý' | 'Ÿ' => b'Y',
        'à'..='å' | 'ā' => b'a',
        'æ' => b'a',
        'ç' | 'ć' | 'č' => b'c',
        'è'..='ë' | 'ē' => b'e',
        'ì'..='ï' | 'ī' => b'i',
        'ñ' | 'ń' => b'n',
        'ò'..='ö' | 'ø' | 'ō' => b'o',
        'ù'..='ü' | 'ū' => b'u',
        'ý' | 'ÿ' => b'y',
        'ß' => b's',
        '\u{FB01}' | '\u{FB02}' => b'f', // fi / fl ligatures
        '–' | '—' | '−' => b'-',
        '‘' | '’' => b'\'',
        '“' | '”' | '«' | '»' => b'"',
        '€' => b'E',
        '£' => b'L',
        '¥' => b'Y',
        '¢' => b'c',
        '°' => b'o',
        '×' => b'x',
        _ => b'?',
    }
}

#[allow(clippy::too_many_lines)]
fn draw_ascii_glyph(ch: u8, b: &mut StrokeBuilder<'_>) {
    // Coordinates in a 600-wide × 700-cap-height / 500-x-height / -200-descender grid.
    const L: f32 = 90.0;
    const R: f32 = 510.0;
    const M: f32 = 300.0;
    const TOP: f32 = 700.0;
    const MID: f32 = 360.0;
    const XH: f32 = 500.0;
    const XMID: f32 = 250.0;
    const BOT: f32 = 0.0;
    const DESC: f32 = -190.0;

    match ch {
        b'A' => {
            b.poly(&[(L, BOT), (M, TOP), (R, BOT)]);
            b.seg(155.0, 240.0, 445.0, 240.0);
        }
        b'B' => {
            b.poly(&[
                (L, BOT),
                (L, TOP),
                (430.0, TOP),
                (490.0, 530.0),
                (430.0, MID),
                (L, MID),
            ]);
            b.poly(&[(430.0, MID), (500.0, 180.0), (430.0, BOT), (L, BOT)]);
        }
        b'C' => b.poly(&[
            (R, 560.0),
            (420.0, TOP),
            (190.0, TOP),
            (L, 540.0),
            (L, 160.0),
            (190.0, BOT),
            (420.0, BOT),
            (R, 140.0),
        ]),
        b'D' => b.poly(&[
            (L, BOT),
            (L, TOP),
            (390.0, TOP),
            (R, 530.0),
            (R, 170.0),
            (390.0, BOT),
            (L, BOT),
        ]),
        b'E' => {
            b.poly(&[(R, TOP), (L, TOP), (L, BOT), (R, BOT)]);
            b.seg(L, MID, 440.0, MID);
        }
        b'F' => {
            b.poly(&[(R, TOP), (L, TOP), (L, BOT)]);
            b.seg(L, MID, 440.0, MID);
        }
        b'G' => {
            b.poly(&[
                (R, 560.0),
                (420.0, TOP),
                (190.0, TOP),
                (L, 540.0),
                (L, 160.0),
                (190.0, BOT),
                (440.0, BOT),
                (R, 160.0),
                (R, MID),
                (320.0, MID),
            ]);
        }
        b'H' => {
            b.seg(L, BOT, L, TOP);
            b.seg(R, BOT, R, TOP);
            b.seg(L, MID, R, MID);
        }
        b'I' => {
            b.seg(M, BOT, M, TOP);
            b.seg(190.0, TOP, 410.0, TOP);
            b.seg(190.0, BOT, 410.0, BOT);
        }
        b'J' => b.poly(&[
            (450.0, TOP),
            (450.0, 150.0),
            (350.0, BOT),
            (190.0, BOT),
            (L, 140.0),
        ]),
        b'K' => {
            b.seg(L, BOT, L, TOP);
            b.seg(R, TOP, L, MID);
            b.seg(230.0, 420.0, R, BOT);
        }
        b'L' => b.poly(&[(L, TOP), (L, BOT), (R, BOT)]),
        b'M' => b.poly(&[(L, BOT), (L, TOP), (M, 260.0), (R, TOP), (R, BOT)]),
        b'N' => b.poly(&[(L, BOT), (L, TOP), (R, BOT), (R, TOP)]),
        b'O' => b.poly(&[
            (190.0, TOP),
            (410.0, TOP),
            (R, 540.0),
            (R, 160.0),
            (410.0, BOT),
            (190.0, BOT),
            (L, 160.0),
            (L, 540.0),
            (190.0, TOP),
        ]),
        b'P' => b.poly(&[
            (L, BOT),
            (L, TOP),
            (420.0, TOP),
            (R, 540.0),
            (420.0, MID),
            (L, MID),
        ]),
        b'Q' => {
            draw_ascii_glyph(b'O', b);
            b.seg(340.0, 180.0, 530.0, -40.0);
        }
        b'R' => {
            draw_ascii_glyph(b'P', b);
            b.seg(330.0, MID, R, BOT);
        }
        b'S' => b.poly(&[
            (490.0, 580.0),
            (400.0, TOP),
            (200.0, TOP),
            (L, 560.0),
            (200.0, 400.0),
            (400.0, 320.0),
            (R, 160.0),
            (400.0, BOT),
            (190.0, BOT),
            (L, 120.0),
        ]),
        b'T' => {
            b.seg(L, TOP, R, TOP);
            b.seg(M, BOT, M, TOP);
        }
        b'U' => b.poly(&[
            (L, TOP),
            (L, 160.0),
            (200.0, BOT),
            (400.0, BOT),
            (R, 160.0),
            (R, TOP),
        ]),
        b'V' => b.poly(&[(L, TOP), (M, BOT), (R, TOP)]),
        b'W' => b.poly(&[
            (70.0, TOP),
            (185.0, BOT),
            (M, 420.0),
            (415.0, BOT),
            (530.0, TOP),
        ]),
        b'X' => {
            b.seg(L, TOP, R, BOT);
            b.seg(R, TOP, L, BOT);
        }
        b'Y' => {
            b.poly(&[(L, TOP), (M, MID), (R, TOP)]);
            b.seg(M, MID, M, BOT);
        }
        b'Z' => b.poly(&[(L, TOP), (R, TOP), (L, BOT), (R, BOT)]),

        b'a' => {
            b.poly(&[
                (140.0, 420.0),
                (240.0, XH),
                (420.0, XH),
                (480.0, 390.0),
                (480.0, BOT),
            ]);
            b.poly(&[
                (480.0, 270.0),
                (220.0, 270.0),
                (110.0, 140.0),
                (220.0, BOT),
                (480.0, BOT),
            ]);
        }
        b'b' => {
            b.seg(120.0, TOP, 120.0, BOT);
            b.poly(&[
                (120.0, 390.0),
                (260.0, XH),
                (420.0, XH),
                (490.0, 360.0),
                (490.0, 140.0),
                (420.0, BOT),
                (260.0, BOT),
                (120.0, 110.0),
            ]);
        }
        b'c' => b.poly(&[
            (480.0, 400.0),
            (380.0, XH),
            (210.0, XH),
            (110.0, 360.0),
            (110.0, 140.0),
            (210.0, BOT),
            (380.0, BOT),
            (480.0, 100.0),
        ]),
        b'd' => {
            b.seg(480.0, TOP, 480.0, BOT);
            b.poly(&[
                (480.0, 390.0),
                (340.0, XH),
                (180.0, XH),
                (110.0, 360.0),
                (110.0, 140.0),
                (180.0, BOT),
                (340.0, BOT),
                (480.0, 110.0),
            ]);
        }
        b'e' => {
            b.poly(&[
                (110.0, XMID),
                (490.0, XMID),
                (490.0, 380.0),
                (390.0, XH),
                (210.0, XH),
                (110.0, 360.0),
                (110.0, 140.0),
                (210.0, BOT),
                (450.0, BOT),
            ]);
        }
        b'f' => {
            b.poly(&[(440.0, TOP), (300.0, TOP), (240.0, 620.0), (240.0, BOT)]);
            b.seg(130.0, XH, 410.0, XH);
        }
        b'g' => {
            b.poly(&[(480.0, XH), (480.0, -90.0), (380.0, DESC), (190.0, DESC)]);
            b.poly(&[
                (480.0, 390.0),
                (340.0, XH),
                (180.0, XH),
                (110.0, 360.0),
                (110.0, 140.0),
                (180.0, BOT),
                (340.0, BOT),
                (480.0, 110.0),
            ]);
        }
        b'h' => {
            b.seg(120.0, TOP, 120.0, BOT);
            b.poly(&[
                (120.0, 380.0),
                (260.0, XH),
                (420.0, XH),
                (480.0, 380.0),
                (480.0, BOT),
            ]);
        }
        b'i' => {
            b.seg(M, XH, M, BOT);
            b.dot(M, 660.0);
        }
        b'j' => {
            b.poly(&[(360.0, XH), (360.0, -90.0), (250.0, DESC), (150.0, DESC)]);
            b.dot(360.0, 660.0);
        }
        b'k' => {
            b.seg(120.0, TOP, 120.0, BOT);
            b.seg(460.0, XH, 120.0, 220.0);
            b.seg(240.0, 310.0, 480.0, BOT);
        }
        b'l' => b.seg(M, TOP, M, BOT),
        b'm' => {
            b.seg(80.0, XH, 80.0, BOT);
            b.poly(&[(80.0, 400.0), (190.0, XH), (300.0, 400.0), (300.0, BOT)]);
            b.poly(&[(300.0, 400.0), (410.0, XH), (520.0, 400.0), (520.0, BOT)]);
        }
        b'n' => {
            b.seg(120.0, XH, 120.0, BOT);
            b.poly(&[
                (120.0, 380.0),
                (260.0, XH),
                (420.0, XH),
                (480.0, 380.0),
                (480.0, BOT),
            ]);
        }
        b'o' => b.poly(&[
            (210.0, XH),
            (390.0, XH),
            (490.0, 360.0),
            (490.0, 140.0),
            (390.0, BOT),
            (210.0, BOT),
            (110.0, 140.0),
            (110.0, 360.0),
            (210.0, XH),
        ]),
        b'p' => {
            b.seg(120.0, XH, 120.0, DESC);
            b.poly(&[
                (120.0, 390.0),
                (260.0, XH),
                (420.0, XH),
                (490.0, 360.0),
                (490.0, 140.0),
                (420.0, BOT),
                (260.0, BOT),
                (120.0, 110.0),
            ]);
        }
        b'q' => {
            b.seg(480.0, XH, 480.0, DESC);
            b.poly(&[
                (480.0, 390.0),
                (340.0, XH),
                (180.0, XH),
                (110.0, 360.0),
                (110.0, 140.0),
                (180.0, BOT),
                (340.0, BOT),
                (480.0, 110.0),
            ]);
        }
        b'r' => {
            b.seg(160.0, XH, 160.0, BOT);
            b.poly(&[(160.0, 360.0), (290.0, XH), (440.0, XH)]);
        }
        b's' => b.poly(&[
            (460.0, 420.0),
            (360.0, XH),
            (200.0, XH),
            (120.0, 380.0),
            (200.0, 280.0),
            (400.0, 220.0),
            (480.0, 120.0),
            (380.0, BOT),
            (190.0, BOT),
            (110.0, 90.0),
        ]),
        b't' => {
            b.poly(&[(260.0, 640.0), (260.0, 90.0), (360.0, BOT), (450.0, BOT)]);
            b.seg(140.0, XH, 420.0, XH);
        }
        b'u' => {
            b.poly(&[
                (120.0, XH),
                (120.0, 120.0),
                (220.0, BOT),
                (380.0, BOT),
                (480.0, 120.0),
            ]);
            b.seg(480.0, XH, 480.0, BOT);
        }
        b'v' => b.poly(&[(100.0, XH), (M, BOT), (500.0, XH)]),
        b'w' => b.poly(&[
            (70.0, XH),
            (185.0, BOT),
            (M, 330.0),
            (415.0, BOT),
            (530.0, XH),
        ]),
        b'x' => {
            b.seg(110.0, XH, 490.0, BOT);
            b.seg(490.0, XH, 110.0, BOT);
        }
        b'y' => {
            b.seg(110.0, XH, M, BOT);
            b.poly(&[(490.0, XH), (M, BOT), (210.0, DESC), (120.0, DESC)]);
        }
        b'z' => b.poly(&[(110.0, XH), (490.0, XH), (110.0, BOT), (490.0, BOT)]),

        b'0' => {
            draw_ascii_glyph(b'O', b);
            b.seg(150.0, 140.0, 450.0, 560.0);
        }
        b'1' => {
            b.poly(&[(190.0, 560.0), (330.0, TOP), (330.0, BOT)]);
            b.seg(170.0, BOT, 470.0, BOT);
        }
        b'2' => b.poly(&[
            (110.0, 540.0),
            (210.0, TOP),
            (410.0, TOP),
            (490.0, 540.0),
            (490.0, 410.0),
            (100.0, BOT),
            (500.0, BOT),
        ]),
        b'3' => {
            b.poly(&[
                (120.0, 570.0),
                (220.0, TOP),
                (410.0, TOP),
                (490.0, 540.0),
                (390.0, MID),
                (260.0, MID),
            ]);
            b.poly(&[
                (390.0, MID),
                (500.0, 180.0),
                (410.0, BOT),
                (210.0, BOT),
                (110.0, 130.0),
            ]);
        }
        b'4' => {
            b.poly(&[(390.0, BOT), (390.0, TOP), (90.0, 220.0), (510.0, 220.0)]);
        }
        b'5' => b.poly(&[
            (480.0, TOP),
            (130.0, TOP),
            (110.0, 390.0),
            (390.0, 410.0),
            (490.0, 260.0),
            (490.0, 130.0),
            (390.0, BOT),
            (190.0, BOT),
            (110.0, 110.0),
        ]),
        b'6' => {
            b.poly(&[
                (450.0, 620.0),
                (350.0, TOP),
                (200.0, TOP),
                (110.0, 500.0),
                (110.0, 150.0),
                (210.0, BOT),
                (400.0, BOT),
                (490.0, 150.0),
                (490.0, 290.0),
                (390.0, 410.0),
                (110.0, 350.0),
            ]);
        }
        b'7' => b.poly(&[(100.0, TOP), (500.0, TOP), (230.0, BOT)]),
        b'8' => {
            b.poly(&[
                (210.0, TOP),
                (390.0, TOP),
                (480.0, 540.0),
                (390.0, MID),
                (210.0, MID),
                (120.0, 540.0),
                (210.0, TOP),
            ]);
            b.poly(&[
                (210.0, MID),
                (100.0, 170.0),
                (200.0, BOT),
                (400.0, BOT),
                (500.0, 170.0),
                (390.0, MID),
            ]);
        }
        b'9' => {
            b.poly(&[
                (150.0, 80.0),
                (250.0, BOT),
                (400.0, BOT),
                (490.0, 200.0),
                (490.0, 550.0),
                (390.0, TOP),
                (200.0, TOP),
                (110.0, 550.0),
                (110.0, 410.0),
                (210.0, 290.0),
                (490.0, 350.0),
            ]);
        }

        b'.' => b.dot(M, 50.0),
        b',' => {
            b.dot(M, 60.0);
            b.seg(M, 60.0, 230.0, -90.0);
        }
        b':' => {
            b.dot(M, 420.0);
            b.dot(M, 60.0);
        }
        b';' => {
            b.dot(M, 420.0);
            b.dot(M, 60.0);
            b.seg(M, 60.0, 230.0, -90.0);
        }
        b'!' => {
            b.seg(M, TOP, M, 200.0);
            b.dot(M, 50.0);
        }
        b'?' => {
            b.poly(&[
                (130.0, 560.0),
                (220.0, TOP),
                (400.0, TOP),
                (480.0, 550.0),
                (480.0, 440.0),
                (M, 310.0),
                (M, 200.0),
            ]);
            b.dot(M, 50.0);
        }
        b'-' => b.seg(130.0, 300.0, 470.0, 300.0),
        b'_' => b.seg(60.0, -40.0, 540.0, -40.0),
        b'+' => {
            b.seg(120.0, 300.0, 480.0, 300.0);
            b.seg(M, 120.0, M, 480.0);
        }
        b'=' => {
            b.seg(120.0, 390.0, 480.0, 390.0);
            b.seg(120.0, 210.0, 480.0, 210.0);
        }
        b'/' => b.seg(120.0, -40.0, 480.0, TOP),
        b'\\' => b.seg(120.0, TOP, 480.0, -40.0),
        b'|' => b.seg(M, DESC, M, TOP),
        b'(' => b.poly(&[(400.0, TOP), (230.0, 460.0), (230.0, 180.0), (400.0, -80.0)]),
        b')' => b.poly(&[(200.0, TOP), (370.0, 460.0), (370.0, 180.0), (200.0, -80.0)]),
        b'[' => b.poly(&[(410.0, TOP), (220.0, TOP), (220.0, -80.0), (410.0, -80.0)]),
        b']' => b.poly(&[(190.0, TOP), (380.0, TOP), (380.0, -80.0), (190.0, -80.0)]),
        b'{' => {
            b.poly(&[
                (410.0, TOP),
                (280.0, 640.0),
                (280.0, 380.0),
                (170.0, 310.0),
                (280.0, 240.0),
                (280.0, -20.0),
                (410.0, -80.0),
            ]);
        }
        b'}' => {
            b.poly(&[
                (190.0, TOP),
                (320.0, 640.0),
                (320.0, 380.0),
                (430.0, 310.0),
                (320.0, 240.0),
                (320.0, -20.0),
                (190.0, -80.0),
            ]);
        }
        b'<' => b.poly(&[(460.0, 520.0), (140.0, 300.0), (460.0, 80.0)]),
        b'>' => b.poly(&[(140.0, 520.0), (460.0, 300.0), (140.0, 80.0)]),
        b'\'' | b'`' => b.seg(M, TOP, 270.0, 500.0),
        b'"' => {
            b.seg(210.0, TOP, 210.0, 500.0);
            b.seg(390.0, TOP, 390.0, 500.0);
        }
        b'*' => {
            b.seg(M, 640.0, M, 340.0);
            b.seg(170.0, 570.0, 430.0, 410.0);
            b.seg(430.0, 570.0, 170.0, 410.0);
        }
        b'#' => {
            b.seg(200.0, BOT, 240.0, TOP);
            b.seg(360.0, BOT, 400.0, TOP);
            b.seg(100.0, 460.0, 500.0, 460.0);
            b.seg(90.0, 230.0, 490.0, 230.0);
        }
        b'$' => {
            draw_ascii_glyph(b'S', b);
            b.seg(M, 760.0, M, -60.0);
        }
        b'%' => {
            b.seg(120.0, BOT, 480.0, TOP);
            b.dot(180.0, 560.0);
            b.dot(420.0, 140.0);
        }
        b'&' => b.poly(&[
            (480.0, BOT),
            (160.0, 470.0),
            (240.0, TOP),
            (380.0, TOP),
            (430.0, 560.0),
            (120.0, 200.0),
            (220.0, BOT),
            (380.0, BOT),
            (500.0, 220.0),
        ]),
        b'@' => {
            draw_ascii_glyph(b'O', b);
            b.poly(&[
                (380.0, 440.0),
                (240.0, 440.0),
                (200.0, 260.0),
                (380.0, 260.0),
                (380.0, 440.0),
            ]);
        }
        b'^' => b.poly(&[(150.0, 460.0), (M, TOP), (450.0, 460.0)]),
        b'~' => b.poly(&[
            (110.0, 300.0),
            (220.0, 400.0),
            (380.0, 200.0),
            (490.0, 300.0),
        ]),
        _ => {
            b.poly(&[
                (120.0, BOT),
                (480.0, BOT),
                (480.0, TOP),
                (120.0, TOP),
                (120.0, BOT),
            ]);
        }
    }
}
