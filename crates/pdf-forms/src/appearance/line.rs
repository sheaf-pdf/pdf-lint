//! One line of text in a box, as PDFium as of chromium/8152 sets a push
//! button's caption (`fpdfsdk/cpdfsdk_appstream.cpp`,
//! `GetPushButtonAppStream`, over `fpdfsdk/pwl/cpwl_edit_impl.cpp`): its
//! size, its layout, and whether it can be placed.

use pdf_gfx::{
    LaidOutText, LayoutFont, Rect, TextAlign, TextBoxStyle, UnencodableChar, VerticalAlign,
    WrapMode, layout_text,
};

use super::frame::Unmade;
use super::text::{AUTO_SIZES, NO_SIZE, REACH, is_bigger};

/// `text` on one line in `box_`, set at its middle by the font's ascent and
/// descent as given (`pdf_gfx::TextBoxStyle::font_heights_as_given`), and
/// across by `align`, past both sides where it's wider than the box with
/// [`TextAlign::Center`]; none where there's nothing to draw: no text once
/// its line breaks are dropped, or a box with no width.
///
/// The size is `size` where it's `0.0001` or more; otherwise (none, `0`, or
/// below, which PDFium takes for none) the largest of PDFium's sizes (4 to
/// 144 points) at which the line fits the box, its width and its height
/// each no bigger by more than `0.0001`, found as PDFium's search finds it
/// (`CPVT_VariableText::GetAutoFontSize`), or 4 where none does.
///
/// # Errors
///
/// [`Unmade::Unencodable`] for a character `font` has no code for, at its
/// byte offset in `text`; [`Unmade::NotFinite`] where the size isn't finite,
/// or the line laid out at it isn't, or reaches past `1e35` from the box's
/// origin, as `appearance::text` refuses its own.
pub(super) fn one_line<F: LayoutFont + ?Sized>(
    text: &str,
    box_: Rect,
    size: Option<f32>,
    align: TextAlign,
    font: &F,
) -> Result<Option<LaidOutText>, Unmade> {
    if size.is_some_and(|s| !s.is_finite()) {
        return Err(Unmade::NotFinite);
    }
    // The line's characters, its breaks dropped, each with its offset in
    // the caller's text, and its advances, the first the font lacks refused.
    let mut shown = String::with_capacity(text.len());
    let mut from = Vec::new();
    let mut advances = Vec::new();
    for (at, ch) in text.char_indices() {
        if ch == '\r' || ch == '\n' {
            continue;
        }
        let mapped = if ch == '\t' { ' ' } else { ch };
        let Some(advance) = font.char_advance(mapped) else {
            return Err(Unmade::Unencodable(UnencodableChar { at, character: ch }));
        };
        shown.push(ch);
        from.push(at);
        advances.push(advance);
    }
    if shown.is_empty() || box_.width() <= 0.0 {
        return Ok(None);
    }

    let mut style = TextBoxStyle {
        align,
        vertical_align: VerticalAlign::Middle,
        wrap: WrapMode::NoWrap,
        leading_factor: 1.0,
        clip: false,
        line_end_spaces: true,
        font_heights_as_given: true,
        ..TextBoxStyle::default()
    };
    let size = match size.filter(|s| *s >= NO_SIZE) {
        Some(size) => size,
        None => auto_size(&advances, box_, font),
    };
    style.size = Some(size);
    let laid = layout_text(font, &shown, box_, &style).map_err(|err| {
        let index = shown[..err.at].chars().count();
        Unmade::Unencodable(UnencodableChar {
            at: from.get(index).copied().unwrap_or(0),
            character: err.character,
        })
    })?;
    if !placeable(&laid) {
        return Err(Unmade::NotFinite);
    }
    Ok(Some(laid))
}

/// The size an automatic size comes to for a line of `advances`, in glyph
/// units, in `box_`: the largest of [`AUTO_SIZES`] at which it's no bigger
/// than the box, by PDFium's binary search over them (`std::lower_bound`
/// for the first size at which it's bigger, its width or its height by
/// more than `0.0001`, and the size before that one; the first size where
/// even that is bigger). A text field's single line is sized the same way
/// (`appearance::text`).
fn auto_size<F: LayoutFont + ?Sized>(advances: &[f32], box_: Rect, font: &F) -> f32 {
    let metrics = font.font_metrics();
    // The heights the layout sets a line by: the font's as given, or its
    // floors where they give no height.
    let (ascent, descent) = {
        let (a, d) = (metrics.ascent / 1000.0, metrics.descent / 1000.0);
        match a.is_finite() && d.is_finite() && a > d {
            true => (metrics.ascent, metrics.descent),
            false => (a.max(0.70) * 1000.0, d.min(-0.15) * 1000.0),
        }
    };
    let bigger = |size: f32| {
        let wide = advances.iter().fold(0.0, |w, a| w + a * size * 0.001);
        let tall = ascent * size * 0.001 - descent * size * 0.001;
        is_bigger(wide, box_.width()) || is_bigger(tall, box_.height())
    };
    let (mut first, mut count) = (0, AUTO_SIZES.len());
    while count > 0 {
        let step = count / 2;
        if bigger(AUTO_SIZES[first + step]) {
            count = step;
        } else {
            first += step + 1;
            count -= step + 1;
        }
    }
    match first {
        0 => AUTO_SIZES[0],
        n => AUTO_SIZES[n - 1],
    }
}

/// Whether `laid` can be placed: its size and every line's origin finite,
/// and every string within [`REACH`] of the box's origin (its place, and
/// one more than the bytes of its codes, each its size and word spacing
/// wide), as `appearance::text` keeps its own.
fn placeable(laid: &LaidOutText) -> bool {
    let size = f64::from(laid.font_size.abs());
    laid.font_size.is_finite()
        && laid.lines.iter().all(|line| {
            let wide = (line.codes.len() as f64 + 1.0) * (size + f64::from(line.word_space.abs()));
            let at = line.origin;
            at.is_finite()
                && (line.codes.is_empty()
                    || f64::from(at.x.abs()) + f64::from(at.y.abs()) + wide <= REACH)
        })
}
