//! `Canvas::box_bands_in`: a beveled or inset border's bands in shades the
//! caller gives.

use pdf_content_streams::{Color, Reserved};

use crate::{BoxBorder, BoxBorderStyle, Canvas, Rect};

/// What one drawing writes on a canvas of its own, as text.
fn drawn(draw: impl FnOnce(&mut Canvas)) -> String {
    let mut canvas = Canvas::new();
    draw(&mut canvas);
    let (bytes, needs) = canvas.finish(&Reserved::default());
    assert!(needs.0.is_empty(), "bands need no resource");
    String::from_utf8(bytes).unwrap()
}

#[test]
fn box_bands_are_box_bands_in_their_styles_shades() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    // The shades `box_bands` chooses: white and the hint at half its
    // brightness (a middle gray with none) for beveled, a middle and a
    // light gray for inset, whatever the hint.
    let beveled = [
        (None, Color::Gray(0.5)),
        (Some(Color::Gray(0.8)), Color::Gray(0.4)),
        (
            Some(Color::Rgb([1.0, 0.5, 0.2])),
            Color::Rgb([0.5, 0.25, 0.1]),
        ),
        (
            Some(Color::Cmyk([0.2, 0.4, 0.0, 0.2])),
            Color::Cmyk([0.1, 0.2, 0.0, 0.6]),
        ),
    ];
    for width in [1.0, 2.0, 2.5] {
        for (hint, dark) in beveled {
            let border = BoxBorder::new(width, BoxBorderStyle::Beveled);
            assert_eq!(
                drawn(|c| c.box_bands(rect, &border, hint)),
                drawn(|c| c.box_bands_in(rect, &border, Color::Gray(1.0), dark)),
                "beveled {width} {hint:?}"
            );
            let border = BoxBorder::new(width, BoxBorderStyle::Inset);
            assert_eq!(
                drawn(|c| c.box_bands(rect, &border, hint)),
                drawn(|c| c.box_bands_in(rect, &border, Color::Gray(0.5), Color::Gray(0.75))),
                "inset {width} {hint:?}"
            );
        }
    }
}

#[test]
fn box_bands_in_fill_each_band_in_its_shade() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    let border = BoxBorder::new(2.0, BoxBorderStyle::Beveled);
    // The shades exchanged, as a button's down appearance has them: the
    // same bands, the first filled with the second's shade.
    assert_eq!(
        drawn(|c| c.box_bands_in(rect, &border, Color::Rgb([0.5; 3]), Color::Gray(1.0))),
        "q\n0.5 0.5 0.5 rg\n12 22 m\n12 68 l\n108 68 l\n106 66 l\n14 66 l\n14 24 l\nh\nf\n\
         1 g\n12 22 m\n108 22 l\n108 68 l\n106 66 l\n106 24 l\n14 24 l\nh\nf\nQ\n"
    );
    // An inset border's shades as any other's: black and white.
    let inset = BoxBorder::new(2.0, BoxBorderStyle::Inset);
    let both = drawn(|c| c.box_bands_in(rect, &inset, Color::Gray(0.0), Color::Gray(1.0)));
    assert!(both.starts_with("q\n0 g\n12 22 m\n"), "{both}");
    assert!(both.contains("h\nf\n1 g\n12 22 m\n108 22 l\n"), "{both}");

    // The caller's fill color is as it was after them.
    let after = drawn(|c| {
        c.fill_color(Color::Gray(0.25));
        c.box_bands_in(rect, &border, Color::Gray(0.0), Color::Gray(1.0));
        c.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
    });
    assert!(after.starts_with("0.25 g\nq\n"), "{after}");
    assert!(after.ends_with("Q\n0 0 1 1 re\nf\n"), "{after}");
}

#[test]
fn box_bands_in_draw_nothing_where_box_bands_draw_nothing() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    let (dark, light) = (Color::Gray(0.0), Color::Gray(1.0));
    for style in [
        BoxBorderStyle::Solid,
        BoxBorderStyle::Dashed,
        BoxBorderStyle::Underline,
    ] {
        let border = BoxBorder::new(2.0, style).with_dash(&[2.0]);
        assert_eq!(drawn(|c| c.box_bands_in(rect, &border, dark, light)), "");
    }
    for style in [BoxBorderStyle::Beveled, BoxBorderStyle::Inset] {
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let border = BoxBorder::new(width, style);
            assert_eq!(drawn(|c| c.box_bands_in(rect, &border, dark, light)), "");
        }
        let border = BoxBorder::new(2.0, style);
        for bad in [
            Rect {
                x0: f32::NAN,
                ..rect
            },
            Rect::new(0.0, 0.0, 7.9, 30.0),
            Rect::new(0.0, 0.0, 30.0, 7.9),
        ] {
            assert_eq!(
                drawn(|c| c.box_bands_in(bad, &border, dark, light)),
                drawn(|c| c.box_bands(bad, &border, None)),
                "{style:?} in {bad:?}"
            );
            assert_eq!(drawn(|c| c.box_bands_in(bad, &border, dark, light)), "");
        }
    }
}
