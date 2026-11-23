//! A combo box's and a list box's appearances, as PDFium as of
//! chromium/8152 makes them (`fpdfsdk/cpdfsdk_appstream.cpp`,
//! `SetAsComboBox`, `SetAsListBox`).

use std::fmt;

use pdf_content_streams::{Color, Paint, Res, Reserved};
use pdf_gfx::{
    BoxBorder, BoxBorderStyle, Canvas, DefaultAppearance, LaidOutText, LayoutFont, Point, Rect,
    TextAlign,
};

use super::frame::{self, Appearance, Unmade, WidgetLook};
use super::line;
use super::text::{NO_SIZE, is_bigger};
use crate::limits::{Cap, Limits};

/// What a combo box shows, as plain data: the text in its box, and whether
/// a drop button is drawn beside it.
///
/// It may gain fields: build it with [`new`](Self::new) and set them.
/// `Debug` prints the text's length, never the text.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComboBox<'a> {
    /// The text shown: the selected option's label, or the value where no
    /// option has it, as an editable combo box's may be.
    pub text: &'a str,
    /// Whether the drop button is drawn at the box's right, as PDFium as of
    /// chromium/8152 draws it, the text set in the box less its 13 points;
    /// `false` draws none and gives the text the whole box.
    pub drop_button: bool,
}

impl<'a> ComboBox<'a> {
    /// A combo box that shows `text`, with its drop button.
    #[must_use]
    pub const fn new(text: &'a str) -> ComboBox<'a> {
        ComboBox {
            text,
            drop_button: true,
        }
    }
}

impl fmt::Debug for ComboBox<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComboBox")
            .field("text_len", &self.text.len())
            .field("drop_button", &self.drop_button)
            .finish()
    }
}

/// What a list box shows, as plain data: its options' labels in order,
/// which of them are selected, and the first shown at its top.
///
/// It may gain fields: build it with [`new`](Self::new) and set them.
/// `Debug` prints counts, never a label.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListBox<'a> {
    /// The options' labels, in the order they're listed.
    pub labels: &'a [&'a str],
    /// The indexes in [`labels`](Self::labels) of the selected options, in
    /// any order; an index given twice, or past the labels, adds nothing.
    pub selected: &'a [usize],
    /// The index of the option shown at the top of the box (`/TI`); the
    /// options before it aren't shown.
    pub top: usize,
}

impl<'a> ListBox<'a> {
    /// A list box of `labels`, none selected, from the first.
    #[must_use]
    pub const fn new(labels: &'a [&'a str]) -> ListBox<'a> {
        ListBox {
            labels,
            selected: &[],
            top: 0,
        }
    }
}

impl fmt::Debug for ListBox<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListBox")
            .field("labels", &self.labels.len())
            .field("selected", &self.selected.len())
            .field("top", &self.top)
            .finish()
    }
}

/// The width PDFium as of chromium/8152 gives a combo box's drop button, at
/// the right of the box less its border.
const DROP_BUTTON: f32 = 13.0;

/// The fill PDFium as of chromium/8152 gives a selected line of a list box:
/// RGB 0, 51, 113 of 255.
const HIGHLIGHT: Color = Color::Rgb([0.0, 51.0 / 255.0, 113.0 / 255.0]);

/// Makes a combo box's appearance: `field`'s text in `font`, named by
/// `font_res`, in the box `look` gives, at the size and in the color of
/// `da`, the field's default appearance, as PDFium as of chromium/8152
/// makes it for the same widget (ISO 32000-2, §12.7.5.4).
///
/// The box and matrix are `appearance::text`'s, turned by the rotation.
/// The text box is the box less the border's reach (twice the width for a
/// beveled or inset border) and, with the drop button, less 13 points at
/// its right, turned the right way round where the box is narrower than
/// that, as PDFium sets it. What's drawn, in PDFium's order:
///
/// 1. The frame, as a text field's ([`text`](super::text)'s first three
///    parts): the background, a beveled or inset border's bands, and the
///    border.
/// 2. `/Tx BMC`, then in a `q`..`Q` the text box as a clip (`re W n`), the
///    text color (`da`'s, black where it gives none), and the text in
///    `BT`..`ET`, then `EMC`; with no text, `/Tx BMC EMC` with nothing
///    between, as an empty text field's.
/// 3. The drop button, where there is one and the box less its border has
///    a height: filled in a light gray (220 of 255 in each component), a
///    beveled border of width 1 in white and a middle gray inside a black
///    ring 1 wide, and where it's taller and wider than 6 points a black
///    triangle pointing down at its centre.
///
/// **The text** is one line, its line breaks dropped, set at the text
/// box's left edge and at its middle by the font's ascent and descent, as
/// given, as PDFium sets a combo box's whatever its quadding (`/Q`). Its
/// size is `da`'s where it gives one of `0.0001` or more; otherwise (`0 Tf`
/// or no `Tf`) the largest of PDFium's sizes (4 to 144 points) at which it
/// fits, as a single-line text field's, or 4 where none does.
/// [`Appearance::overflowed`] says it passes the text box.
///
/// A color's components are written within `0.0..=1.0`, as
/// [`text`](super::text) writes them.
///
/// ```
/// use pdf_forms::appearance::{ComboBox, FieldFont, WidgetLook, combo_box};
/// use pdf_forms::pdf_annots::pdf_content_streams::Color;
/// use pdf_forms::pdf_annots::pdf_gfx::DefaultAppearance;
/// use pdf_forms::pdf_fonts::Std14;
/// use pdf_forms::Limits;
///
/// // A combo box 100 by 20, its border black, showing its selected label.
/// let mut look = WidgetLook::new(100.0, 20.0);
/// look.border_color = Some(Color::Gray(0.0));
/// let da = DefaultAppearance::parse(b"/Helv 10 Tf 0 g", &Limits::DEFAULT.annots.content).unwrap();
/// let font = FieldFont::standard(Std14::Helvetica);
///
/// let combo = ComboBox::new("Label two");
/// let made = combo_box(&combo, &look, &da, font.res().unwrap(), &font.layout(), &Limits::DEFAULT)
///     .unwrap();
/// let content = String::from_utf8(made.content).unwrap();
/// // The text set in the box less its border and the drop button's 13 points.
/// assert!(content.contains("/Tx BMC\nq\n1 1 85 18 re\nW\nn\n0 g\nBT\n"));
/// assert!(content.contains("(Label two) Tj\nET\nQ\nEMC\n"));
/// // The drop button after it.
/// assert!(content.contains("86 1 13 18 re\nf\n"));
///
/// // With no drop button, the text has the whole box.
/// let mut combo = ComboBox::new("Label two");
/// combo.drop_button = false;
/// let made = combo_box(&combo, &look, &da, font.res().unwrap(), &font.layout(), &Limits::DEFAULT)
///     .unwrap();
/// let content = String::from_utf8(made.content).unwrap();
/// assert!(content.contains("/Tx BMC\nq\n1 1 98 18 re\nW\nn\n"));
/// assert!(content.ends_with("EMC\n"));
/// ```
///
/// # Errors
///
/// [`Unmade::Limit`] with [`Cap::Text`] where the text is longer than
/// [`Limits::text`] bytes; [`Unmade::NoBox`] for a box with no width or no
/// height; [`Unmade::NotFinite`] where a number of `look` or `da`'s size or
/// color isn't finite, or the text laid out isn't, or reaches past `1e35`
/// from the box's origin; [`Unmade::Unencodable`] for a character `font`
/// has no code for, at its byte offset in the text. Never
/// [`Unmade::NotChoice`], [`Unmade::FontNotBindable`], or
/// [`Unmade::Unreadable`], which are the read layer's.
pub fn combo_box<F: LayoutFont + ?Sized>(
    field: &ComboBox<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    font: &F,
    limits: &Limits,
) -> Result<Appearance, Unmade> {
    if field.text.len() > limits.text {
        return Err(Unmade::Limit(Cap::Text));
    }
    let (bbox, matrix) = frame::placed(look)?;
    if da.font_size.is_some_and(|s| !s.is_finite()) || !frame::finite_color(da.color) {
        return Err(Unmade::NotFinite);
    }
    let look = &frame::in_range(look);
    let client = frame::client(look, bbox)?;
    // The text box and the drop button, as PDFium places them: the button
    // the client's right 13 points, the text box the rest, turned the right
    // way round where the client is narrower.
    let (edit, button) = match field.drop_button {
        true => {
            let left = client.x1 - DROP_BUTTON;
            (
                Rect::new(client.x0, client.y0, left, client.y1),
                Some(Rect::new(left, client.y0, client.x1, client.y1)),
            )
        }
        false => (client, None),
    };
    let laid = line::one_line(field.text, edit, da.size(), TextAlign::Left, font)?;

    let mut canvas = Canvas::new();
    frame::frame(&mut canvas, look, bbox);
    match &laid {
        Some(laid) => canvas.with_tagged("Tx", |c| {
            c.with_state(|c| {
                c.clip_rect(edit);
                c.fill_color(da.color.map_or(Color::Gray(0.0), frame::clamped));
                c.draw_layout(font_res, laid);
            });
        }),
        None => canvas.with_tagged("Tx", |_| {}),
    }
    if let Some(button) = button {
        drop_button(&mut canvas, button);
    }
    let (content, needs) = canvas.finish(&Reserved::default());
    Ok(Appearance {
        bbox,
        matrix,
        content,
        needs,
        font_size: laid.as_ref().map_or(0.0, |laid| laid.font_size),
        overflowed: laid
            .as_ref()
            .map_or(!field.text.is_empty(), |laid| laid.overflowed),
    })
}

/// Draws a combo box's drop button in `rect`, as PDFium as of chromium/8152
/// draws it (`fpdfsdk/cpdfsdk_appstream.cpp`, `GetDropButtonAppStream`):
/// filled in a light gray, a beveled border of PDFium's width 2 (a ring 1
/// wide and bands 1 wide) in black, white, and a middle gray, and a
/// triangle where it's wider and taller than 6 points; nothing where it has
/// no width or no height.
fn drop_button(canvas: &mut Canvas, rect: Rect) {
    if !(rect.width() > 0.0 && rect.height() > 0.0) {
        return;
    }
    canvas.with_state(|c| {
        c.fill_color(Color::Rgb([220.0 / 255.0; 3]));
        c.fill_rect(rect);
    });
    let bevel = BoxBorder::new(1.0, BoxBorderStyle::Beveled);
    canvas.box_bands_in(rect, &bevel, Color::Gray(1.0), Color::Gray(0.5));
    canvas.box_border(rect, &BoxBorder::solid(1.0), Color::Gray(0.0), None);
    if is_bigger(rect.width(), 6.0) && is_bigger(rect.height(), 6.0) {
        let (cx, cy) = ((rect.x0 + rect.x1) / 2.0, (rect.y0 + rect.y1) / 2.0);
        canvas.with_state(|c| {
            c.fill_color(Color::Gray(0.0));
            c.polyline(
                &[
                    Point::new(cx - 3.0, cy + 1.5),
                    Point::new(cx + 3.0, cy + 1.5),
                    Point::new(cx, cy - 1.5),
                ],
                true,
                Paint::Fill,
            );
        });
    }
}

/// Makes a list box's appearance: `field`'s options from its top, a line
/// each, in `font`, named by `font_res`, in the box `look` gives, at the
/// size and in the color of `da`, the field's default appearance, as PDFium
/// as of chromium/8152 makes it for the same widget (ISO 32000-2,
/// §12.7.5.4), but for the lines below the box, which aren't written.
///
/// The box and matrix are `appearance::text`'s, turned by the rotation.
/// What's drawn, in PDFium's order:
///
/// 1. The frame, as a text field's ([`text`](super::text)'s first three
///    parts).
/// 2. `/Tx BMC`, then in a `q`..`Q` the box less the border's reach (twice
///    the width for a beveled or inset border) as a clip, and each line in
///    turn: a selected one over a rectangle across the box from its top to
///    its bottom, filled in PDFium's blue (RGB 0, 51, 113 of 255), its text
///    white (`1 g`), the others in `da`'s color (black where it gives
///    none), each in `BT`..`ET`; then `EMC`. With no line, `/Tx BMC EMC`
///    with nothing between, as an empty text field's.
///
/// **The lines** are the labels from [`top`](ListBox::top) on, each one
/// line, its line breaks dropped, at the left edge of the box less the
/// border, the first from that box's top, each the font's line height (its
/// ascent less its descent, as given) below the one before, whatever the
/// quadding (`/Q`). The size is `da`'s where it gives one of `0.0001` or
/// more, else 12, as PDFium sizes a list box's lines: never automatic. A
/// line whose top is at or below that box's bottom isn't written, nor any
/// after it, where PDFium writes every option to the last, clipped away:
/// nothing visible differs, and what's written is bounded by that box. An
/// empty label is a line with no text. [`Appearance::overflowed`] says a
/// line passes the box, below it or across.
///
/// A color's components are written within `0.0..=1.0`, as
/// [`text`](super::text) writes them.
///
/// ```
/// use pdf_forms::appearance::{FieldFont, ListBox, WidgetLook, list_box};
/// use pdf_forms::pdf_annots::pdf_gfx::DefaultAppearance;
/// use pdf_forms::pdf_fonts::Std14;
/// use pdf_forms::Limits;
///
/// // A list box 80 by 40 of five options, the second selected.
/// let labels = ["alpha", "bravo", "charlie", "delta", "echo"];
/// let mut list = ListBox::new(&labels);
/// list.selected = &[1];
/// let look = WidgetLook::new(80.0, 40.0);
/// let da = DefaultAppearance::parse(b"/Helv 10 Tf 0 g", &Limits::DEFAULT.annots.content).unwrap();
/// let font = FieldFont::standard(Std14::Helvetica);
///
/// let made = list_box(&list, &look, &da, font.res().unwrap(), &font.layout(), &Limits::DEFAULT)
///     .unwrap();
/// let content = String::from_utf8(made.content).unwrap();
/// assert!(content.starts_with("/Tx BMC\nq\n1 1 78 38 re\nW\nn\n"));
/// // The selected line over its highlight, in white.
/// assert!(content.contains("0 0.2 0.4431 rg\n1 15.62 78 11.69 re\nf\n"));
/// assert!(content.contains("1 g\nBT\n"));
/// // Four lines reach into the box; the fifth, below it, isn't written.
/// assert!(content.contains("(delta) Tj"));
/// assert!(!content.contains("(echo) Tj"));
/// assert!(made.overflowed);
/// ```
///
/// # Errors
///
/// [`Unmade::Limit`] with [`Cap::Items`] where more labels than
/// [`Limits::items`] are listed from the top, or with [`Cap::Text`] where
/// their bytes together are more than [`Limits::text`], each before
/// anything is laid out; [`Unmade::NoBox`] for a box with no width or no
/// height; [`Unmade::NotFinite`] where a number of `look` or `da`'s size or
/// color isn't finite, or a line laid out isn't, or reaches past `1e35`
/// from the box's origin; [`Unmade::UnencodableOption`] for a character
/// `font` has no code for in a line that's written: the label's index in
/// [`labels`](ListBox::labels), and the character and its byte offset in
/// that label. Never [`Unmade::NotChoice`], [`Unmade::FontNotBindable`],
/// or [`Unmade::Unreadable`], which are the read layer's.
pub fn list_box<F: LayoutFont + ?Sized>(
    field: &ListBox<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    font: &F,
    limits: &Limits,
) -> Result<Appearance, Unmade> {
    let shown = field.labels.get(field.top..).unwrap_or_default();
    if shown.len() > limits.items {
        return Err(Unmade::Limit(Cap::Items));
    }
    let bytes = shown.iter().try_fold(0usize, |n, label| {
        n.checked_add(label.len()).filter(|n| *n <= limits.text)
    });
    if bytes.is_none() {
        return Err(Unmade::Limit(Cap::Text));
    }
    let (bbox, matrix) = frame::placed(look)?;
    if da.font_size.is_some_and(|s| !s.is_finite()) || !frame::finite_color(da.color) {
        return Err(Unmade::NotFinite);
    }
    let look = &frame::in_range(look);
    let client = frame::client(look, bbox)?;
    let size = da.size().filter(|s| *s >= NO_SIZE).unwrap_or(12.0);
    let height = line::line_height(font, size);
    if !height.is_finite() {
        return Err(Unmade::NotFinite);
    }

    // Each line shown from the top, until one starts at or below the
    // box's bottom: whether it's selected, its box, and its text laid out.
    let mut selected = vec![false; shown.len()];
    for index in field.selected {
        if let Some(at) = index.checked_sub(field.top)
            && let Some(mark) = selected.get_mut(at)
        {
            *mark = true;
        }
    }
    let mut lines: Vec<(bool, Rect, Option<LaidOutText>)> = Vec::new();
    let mut overflowed = false;
    let mut top = client.y1;
    if client.width() > 0.0 {
        for (at, label) in shown.iter().enumerate() {
            if top <= client.y0 {
                overflowed = true;
                break;
            }
            let rect = Rect::new(client.x0, top - height, client.x1, top);
            let laid =
                line::one_line(label, rect, Some(size), TextAlign::Left, font).map_err(|err| {
                    match err {
                        Unmade::Unencodable(character) => Unmade::UnencodableOption {
                            option: field.top + at,
                            character,
                        },
                        other => other,
                    }
                })?;
            overflowed |=
                is_bigger(client.y0, rect.y0) || laid.as_ref().is_some_and(|l| l.overflowed);
            lines.push((selected[at], rect, laid));
            top -= height;
        }
    } else {
        overflowed = !shown.is_empty();
    }

    let mut canvas = Canvas::new();
    frame::frame(&mut canvas, look, bbox);
    let color = da.color.map_or(Color::Gray(0.0), frame::clamped);
    canvas.with_tagged("Tx", |c| {
        if lines.is_empty() {
            return;
        }
        c.with_state(|c| {
            c.clip_rect(client);
            for (selected, rect, laid) in &lines {
                if *selected {
                    c.with_state(|c| {
                        c.fill_color(HIGHLIGHT);
                        c.fill_rect(*rect);
                    });
                }
                if let Some(laid) = laid {
                    c.fill_color(if *selected { Color::Gray(1.0) } else { color });
                    c.draw_layout(font_res.clone(), laid);
                }
            }
        });
    });
    let (content, needs) = canvas.finish(&Reserved::default());
    Ok(Appearance {
        bbox,
        matrix,
        content,
        needs,
        font_size: if lines.is_empty() { 0.0 } else { size },
        overflowed,
    })
}
