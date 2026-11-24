//! A check box's, a radio button's, and a push button's appearances, as
//! PDFium as of chromium/8152 makes them (`fpdfsdk/cpdfsdk_appstream.cpp`,
//! `SetAsCheckBox`, `SetAsRadioButton`, `SetAsPushButton`), but for a check
//! box's or radio button's mark, which is drawn in ZapfDingbats.

use std::fmt;

use pdf_content_streams::{Color, Res, Reserved, ResourceNeeds};
use pdf_fonts::{Encoding, GlyphBox, Std14};
use pdf_gfx::{Canvas, DefaultAppearance, LaidOutText, LayoutFont, Point, Rect, TextAlign};

use super::frame::{self, Appearance, Unmade, WidgetLook};
use super::line;
use super::text::{NO_SIZE, REACH};
use crate::limits::{Cap, Limits};

/// The mark a check box or radio button shows when it's on: a glyph of
/// ZapfDingbats, by the code its normal caption (`/MK /CA`) gives (ISO
/// 32000-2, §12.7.5.2.3, and its example of a check box drawn in it).
///
/// PDFium as of chromium/8152 draws six of them as shapes of its own, by
/// the caption's first character, and any other caption as its default:
/// those six are named here, and any other glyph is [`Other`](Self::Other).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Mark {
    /// A check mark: `4`, glyph `a20` (✔).
    Check,
    /// A filled circle: `l`, glyph `a71` (●).
    Circle,
    /// A cross: `8`, glyph `a24` (✘).
    Cross,
    /// A filled diamond: `u`, glyph `a78` (◆).
    Diamond,
    /// A filled square: `n`, glyph `a73` (■).
    Square,
    /// A star: `H`, glyph `a35` (★).
    Star,
    /// Any other glyph ZapfDingbats has a box for, by its code.
    Other(u8),
}

impl Mark {
    /// The mark a caption names: its first character, where it's one of
    /// ZapfDingbats' codes for a glyph with a box (a printable ASCII
    /// character but the space, as a caption string holds a code), else
    /// `default`, as for no caption: [`Mark::Check`] for a check box and
    /// [`Mark::Circle`] for a radio button, PDFium's defaults.
    #[must_use]
    pub fn from_caption(caption: Option<&str>, default: Mark) -> Mark {
        let Some(code) = caption
            .and_then(|c| c.chars().next())
            .and_then(|ch| u8::try_from(u32::from(ch)).ok())
            .filter(|code| (0x21..=0x7E).contains(code))
        else {
            return default;
        };
        match code {
            b'4' => Mark::Check,
            b'l' => Mark::Circle,
            b'8' => Mark::Cross,
            b'u' => Mark::Diamond,
            b'n' => Mark::Square,
            b'H' => Mark::Star,
            _ if glyph(code).is_some() => Mark::Other(code),
            _ => default,
        }
    }

    /// The mark's code in ZapfDingbats' own encoding (ISO 32000-2, D.6).
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Mark::Check => b'4',
            Mark::Circle => b'l',
            Mark::Cross => b'8',
            Mark::Diamond => b'u',
            Mark::Square => b'n',
            Mark::Star => b'H',
            Mark::Other(code) => code,
        }
    }

    /// The share of the side of the box's centre square that the glyph's
    /// box fits: the larger side of where PDFium's shape for the mark
    /// reaches, measured on its own appearances, so a mark is as large as
    /// PDFium's.
    fn share(self, radio: bool) -> f32 {
        match self {
            Mark::Check => 0.49,
            Mark::Cross => 1.0,
            Mark::Star => 0.70,
            Mark::Circle if radio => 0.5,
            _ => 2.0 / 3.0,
        }
    }
}

/// ZapfDingbats' glyph for `code`: its advance and its box, per 1000 em;
/// none for a code it has no glyph for, or a glyph whose box is empty.
fn glyph(code: u8) -> Option<(f32, GlyphBox)> {
    let name = Encoding::ZapfDingbats.glyph(code)?;
    let font = Std14::ZapfDingbats;
    let (advance, bounds) = (font.glyph_width(name)?, font.glyph_box(name)?);
    (bounds.x1 > bounds.x0 && bounds.y1 > bounds.y0).then_some((advance, bounds))
}

/// What a check box or radio button shows, as plain data: its normal
/// caption (`/MK /CA`), which names its mark ([`Mark::from_caption`]).
///
/// It may gain fields: build it with [`new`](Self::new) and set them.
/// `Debug` prints the caption's length, never the caption.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ToggleButton<'a> {
    /// The normal caption; none for the default mark.
    pub caption: Option<&'a str>,
}

impl ToggleButton<'_> {
    /// A button with no caption, which shows the default mark.
    #[must_use]
    pub const fn new() -> Self {
        ToggleButton { caption: None }
    }
}

impl fmt::Debug for ToggleButton<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToggleButton")
            .field("caption_len", &self.caption.map(str::len))
            .finish()
    }
}

/// A check box's or radio button's appearances: the normal (`/AP /N`) and
/// the down (`/AP /D`) appearance of its on state and of `Off`, each a
/// state of its slot's dictionary.
///
/// An `Off` appearance that would paint nothing isn't made (`None`): ISO
/// 32000-2, §12.7.5.2.3 makes it optional, and a widget whose normal
/// appearance has no `Off` shows nothing when it's off, as one with an
/// empty stream does. The caller binds each one there is under its state's
/// name, the on state's as [`group::on_state_name`](crate::group::on_state_name)
/// names it.
///
/// `Debug` prints each appearance as [`Appearance`]'s does.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ToggleAppearances {
    /// The normal appearance of the on state: the frame and the mark.
    pub on: Appearance,
    /// The normal appearance of `Off`: the frame alone; none where there is
    /// no background and no border is drawn.
    pub off: Option<Appearance>,
    /// The down appearance of the on state: the down frame and the mark.
    pub down_on: Appearance,
    /// The down appearance of `Off`: the down frame alone; none where it
    /// would paint nothing, which, the down background being drawn with
    /// none of its own, is only a radio button's round frame's, beveled,
    /// with no background and no width.
    pub down_off: Option<Appearance>,
}

/// Makes a check box's appearances ([`ToggleAppearances`]): `button`'s mark
/// in ZapfDingbats, named by `font_res`, in the box `look` gives, in the
/// color of `da`, the field's default appearance, as PDFium as of
/// chromium/8152 makes them for the same widget, but for the mark: a glyph
/// of ZapfDingbats, as ISO 32000-2's example of a check box draws it and as
/// a `/DA` names the font, where PDFium fills a shape of its own.
///
/// The box and matrix are `appearance::text`'s, turned by the rotation.
/// What's drawn in each, in PDFium's order:
///
/// 1. The frame, as a text field's ([`text`](super::text)'s first three
///    parts): the background, a beveled or inset border's bands, and the
///    border. In the down appearances, the background less `0.25` in each
///    component, none below `0` (`0.75 0.75 0.75 rg` where there is none),
///    a beveled border's two shades exchanged, and an inset border's black
///    and white.
/// 2. In the on state's, the mark in a `q`..`Q`: the text color (`da`'s,
///    black where it gives none), and `BT`, the font at the mark's size, the
///    glyph's code at its place, `ET`. No `/Tx` sequence: a button's
///    caption isn't variable text (ISO 32000-2, §12.7.4.3).
///
/// **The mark:** [`Mark::from_caption`] of the caption, [`Mark::Check`] by
/// default. Its glyph's box (ZapfDingbats' metrics, the font being drawn by
/// name whatever `font_res` is) fits a square centred on the centre square
/// of the box less the border's reach (as a text's box is), whose side is
/// the centre square's times the mark's share: `0.49` for the check, `1`
/// for the cross, `0.70` for the star, and two thirds for any other, the
/// size of where PDFium's own shape for it reaches. The size is `da`'s
/// instead where it gives one of `0.0001` or more. Across, the glyph's
/// advance is centred on the square; up, its box is.
///
/// A color's components are written within `0.0..=1.0`, as
/// [`text`](super::text) writes them.
///
/// ```
/// use pdf_forms::appearance::{FieldFont, ToggleButton, WidgetLook, check_box};
/// use pdf_forms::pdf_annots::pdf_content_streams::Color;
/// use pdf_forms::pdf_annots::pdf_gfx::DefaultAppearance;
/// use pdf_forms::pdf_fonts::Std14;
/// use pdf_forms::Limits;
///
/// // A check box 12 by 12, its border black, its mark a cross.
/// let mut button = ToggleButton::new();
/// button.caption = Some("8");
/// let mut look = WidgetLook::new(12.0, 12.0);
/// look.border_color = Some(Color::Gray(0.0));
/// let da = DefaultAppearance::parse(b"/ZaDb 0 Tf 0 g", &Limits::DEFAULT.annots.content).unwrap();
/// let font = FieldFont::standard(Std14::ZapfDingbats);
///
/// let made = check_box(&button, &look, &da, font.res().unwrap(), &Limits::DEFAULT).unwrap();
/// let on = String::from_utf8(made.on.content.clone()).unwrap();
/// assert!(on.ends_with("(8) Tj\nET\nQ\n"));
/// // `Off` is the frame alone: the border, so it's made.
/// let off = made.off.unwrap();
/// assert!(on.starts_with(std::str::from_utf8(&off.content).unwrap()));
/// // The down appearances are filled a darker gray.
/// assert!(made.down_on.content.starts_with(b"q\n0.75 0.75 0.75 rg\n"));
/// ```
///
/// # Errors
///
/// [`Unmade::Limit`] with [`Cap::Text`] where the caption is longer than
/// [`Limits::text`] bytes; [`Unmade::NoBox`] for a box with no width or no
/// height; [`Unmade::NotFinite`] where a number of `look` or `da`'s size or
/// color isn't finite, or the mark at that size reaches past `1e35` from the
/// box's origin. Never [`Unmade::Unencodable`]: a caption ZapfDingbats has
/// no glyph for shows the default mark.
pub fn check_box(
    button: &ToggleButton<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    limits: &Limits,
) -> Result<ToggleAppearances, Unmade> {
    toggle(button, look, da, font_res, limits, false)
}

/// Makes a radio button's appearances ([`ToggleAppearances`]), as
/// [`check_box`] makes a check box's but for two things, as PDFium as of
/// chromium/8152 makes them: the default mark is [`Mark::Circle`], drawn at
/// half the centre square (two thirds for a check box's); and with that
/// mark the frame is round. The round frame, in the box's centre square
/// less `1` on each side: the circle filled in the background; a solid,
/// dashed, or underline border's circle half its width inside, stroked at
/// it; a beveled or inset border's circle stroked at its width in its color,
/// then two half circles inside, the upper left and the lower right, in the
/// border style's shades (white and the background less `0.25` for
/// beveled, a middle and a light gray for inset); in the down appearances,
/// the fill less `0.25` (a beveled border's the background itself) and the
/// shades the background less `0.25` and white for beveled, black and
/// white for inset.
///
/// # Errors
///
/// As [`check_box`].
pub fn radio_button(
    button: &ToggleButton<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    limits: &Limits,
) -> Result<ToggleAppearances, Unmade> {
    toggle(button, look, da, font_res, limits, true)
}

/// A check box's or (`radio`) a radio button's appearances.
fn toggle(
    button: &ToggleButton<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    limits: &Limits,
    radio: bool,
) -> Result<ToggleAppearances, Unmade> {
    if button.caption.is_some_and(|c| c.len() > limits.text) {
        return Err(Unmade::Limit(Cap::Text));
    }
    let (bbox, matrix) = frame::placed(look)?;
    if da.font_size.is_some_and(|s| !s.is_finite()) || !frame::finite_color(da.color) {
        return Err(Unmade::NotFinite);
    }
    let look = &frame::in_range(look);
    let client = frame::client(look, bbox)?;
    let default = if radio { Mark::Circle } else { Mark::Check };
    let mark = Mark::from_caption(button.caption, default);
    let round = radio && mark == Mark::Circle;
    let placed = place_mark(mark, mark.share(radio), client, da)?;

    // Each frame drawn once, and the mark once, its font its one resource.
    let drawn = |draw: &dyn Fn(&mut Canvas)| {
        let mut canvas = Canvas::new();
        draw(&mut canvas);
        canvas.finish(&Reserved::default()).0
    };
    let (normal, down) = match round {
        true => (
            drawn(&|c| frame::round_frame(c, look, bbox, false)),
            drawn(&|c| frame::round_frame(c, look, bbox, true)),
        ),
        false => (
            drawn(&|c| frame::frame(c, look, bbox)),
            drawn(&|c| frame::down_frame(c, look, bbox)),
        ),
    };
    let (shown, needs) = match &placed {
        Some(placed) => {
            let mut canvas = Canvas::new();
            let color = da.color.map_or(Color::Gray(0.0), frame::clamped);
            canvas.with_state(|c| {
                c.fill_color(color);
                c.text_at(font_res, placed.size, placed.origin, &[mark.code()]);
            });
            canvas.finish(&Reserved::default())
        }
        None => (Vec::new(), ResourceNeeds::default()),
    };
    let (size, overflowed) = placed.map_or((0.0, false), |p| (p.size, p.overflowed));
    let on = |frame: &[u8]| Appearance {
        bbox,
        matrix,
        content: [frame, &shown].concat(),
        needs: needs.clone(),
        font_size: size,
        overflowed,
    };
    let off = |frame: Vec<u8>| {
        (!frame.is_empty()).then(|| Appearance {
            bbox,
            matrix,
            content: frame,
            needs: ResourceNeeds::default(),
            font_size: 0.0,
            overflowed: false,
        })
    };
    Ok(ToggleAppearances {
        on: on(&normal),
        down_on: on(&down),
        off: off(normal),
        down_off: off(down),
    })
}

/// A mark's glyph as it's drawn: its size and its origin, and whether its
/// box passes the box it's centred in.
#[derive(Clone, Copy)]
struct PlacedMark {
    size: f32,
    origin: Point,
    overflowed: bool,
}

/// Where `mark`'s glyph is drawn, centred on `client`'s centre square, its
/// box fitting `share` of the square's side, or at `da`'s size where it
/// gives one; none where the square has no side, so nothing is drawn.
fn place_mark(
    mark: Mark,
    share: f32,
    client: Rect,
    da: &DefaultAppearance,
) -> Result<Option<PlacedMark>, Unmade> {
    let side = client.width().min(client.height());
    let Some((advance, bounds)) = glyph(mark.code()) else {
        return Ok(None);
    };
    if side.is_nan() || side <= 0.0 {
        return Ok(None);
    }
    let size = match da.size().filter(|s| *s >= NO_SIZE) {
        Some(size) => size,
        None => side * share * 1000.0 / (bounds.x1 - bounds.x0).max(bounds.y1 - bounds.y0),
    };
    let (cx, cy) = ((client.x0 + client.x1) / 2.0, (client.y0 + client.y1) / 2.0);
    let origin = Point::new(
        cx - size * advance / 2000.0,
        cy - size * (bounds.y0 + bounds.y1) / 2000.0,
    );
    let reach = f64::from(origin.x.abs()) + f64::from(origin.y.abs()) + 2.0 * f64::from(size);
    if !(size.is_finite() && origin.is_finite() && reach <= REACH) {
        return Err(Unmade::NotFinite);
    }
    let drawn = Rect::new(
        origin.x + size * bounds.x0 / 1000.0,
        origin.y + size * bounds.y0 / 1000.0,
        origin.x + size * bounds.x1 / 1000.0,
        origin.y + size * bounds.y1 / 1000.0,
    );
    Ok(Some(PlacedMark {
        size,
        origin,
        overflowed: !client.expand(0.001).contains_rect(drawn),
    }))
}

/// What a push button shows, as plain data: its normal caption (`/MK
/// /CA`), its rollover and down captions (`/MK /RC`, `/MK /AC`), and whether
/// it has rollover and down appearances, which its highlighting mode
/// (`/H`) calls for where it's `/P`, push, or `/T`, toggle.
///
/// It may gain fields: build it with [`new`](Self::new) and set them.
/// `Debug` prints the captions' lengths, never a caption.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PushButton<'a> {
    /// The normal caption; empty for none, which draws the frame alone.
    pub caption: &'a str,
    /// The rollover caption; the normal one where it's none or empty.
    pub rollover_caption: Option<&'a str>,
    /// The down caption; the normal one where it's none or empty.
    pub down_caption: Option<&'a str>,
    /// Whether the rollover and down appearances are made: the widget's
    /// `/H` is `/P` or `/T`.
    pub pressed_states: bool,
}

impl<'a> PushButton<'a> {
    /// A button that shows `caption`, with no other captions, and its
    /// normal appearance alone.
    #[must_use]
    pub const fn new(caption: &'a str) -> PushButton<'a> {
        PushButton {
            caption,
            rollover_caption: None,
            down_caption: None,
            pressed_states: false,
        }
    }
}

impl fmt::Debug for PushButton<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushButton")
            .field("caption_len", &self.caption.len())
            .field("rollover_caption_len", &self.rollover_caption.map(str::len))
            .field("down_caption_len", &self.down_caption.map(str::len))
            .field("pressed_states", &self.pressed_states)
            .finish()
    }
}

/// A push button's appearances: the normal one (`/AP /N`), and where the
/// button has them ([`PushButton::pressed_states`]) the rollover (`/AP /R`)
/// and the down one (`/AP /D`), each a single stream.
///
/// `Debug` prints each appearance as [`Appearance`]'s does.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PushAppearances {
    /// The normal appearance.
    pub normal: Appearance,
    /// The rollover appearance: as the normal one, with the rollover
    /// caption.
    pub rollover: Option<Appearance>,
    /// The down appearance: the down frame, with the down caption.
    pub down: Option<Appearance>,
}

/// Makes a push button's appearances ([`PushAppearances`]): `button`'s
/// caption in `font`, named by `font_res`, in the box `look` gives, at the
/// size and in the color of `da`, the field's default appearance, as PDFium
/// as of chromium/8152 makes them for the same widget. An icon (`/MK /I`)
/// isn't drawn: a widget that shows one is refused by
/// [`widget_push`](super::widget_push), and a caller of this maker draws
/// none.
///
/// The box and matrix are `appearance::text`'s, turned by the rotation.
/// What's drawn in each, in PDFium's order:
///
/// 1. The frame, as a text field's; in the down appearance, as a check
///    box's down appearance has it ([`check_box`]): the background less
///    `0.25`, a beveled border's shades exchanged, an inset border's black
///    and white.
/// 2. Where there's a caption, in a `q`..`Q`: the box less the border's
///    reach as a clip (`re W n`), the text color (`da`'s, black where it
///    gives none), and the caption in `BT`..`ET`. No `/Tx` sequence: a push
///    button's caption isn't variable text (ISO 32000-2, §12.7.4.3).
///
/// **The caption** is one line, its line breaks dropped, centred both ways
/// in the box less the border's reach (twice the width for a beveled or
/// inset border), past both sides where it's wider. Its size is `da`'s where
/// it gives one of `0.0001` or more; otherwise (`0 Tf`, or no `Tf` at all,
/// as PDFium sizes both) the largest of PDFium's sizes (4 to 144 points) at
/// which it fits, as a text field's single line, or 4 where none does. The
/// rollover appearance shows the rollover caption and the down one the down
/// caption, each the normal caption where it's none or empty. A caption that
/// is empty is no caption: the frame alone, no clip and no text.
///
/// ```
/// use pdf_forms::appearance::{FieldFont, PushButton, WidgetLook, push_button};
/// use pdf_forms::pdf_annots::pdf_content_streams::Color;
/// use pdf_forms::pdf_annots::pdf_gfx::{BoxBorder, BoxBorderStyle, DefaultAppearance};
/// use pdf_forms::pdf_fonts::Std14;
/// use pdf_forms::Limits;
///
/// // A push button 60 by 20, beveled on gray, that shows "Down" when pressed.
/// let mut button = PushButton::new("Push");
/// button.down_caption = Some("Down");
/// button.pressed_states = true;
/// let mut look = WidgetLook::new(60.0, 20.0);
/// look.background = Some(Color::Gray(0.75));
/// look.border_color = Some(Color::Gray(0.0));
/// look.border = BoxBorder::new(1.0, BoxBorderStyle::Beveled);
/// let da = DefaultAppearance::parse(b"/Helv 10 Tf 0 g", &Limits::DEFAULT.annots.content).unwrap();
/// let font = FieldFont::standard(Std14::Helvetica);
///
/// let made = push_button(&button, &look, &da, font.res().unwrap(), &font.layout(), &Limits::DEFAULT)
///     .unwrap();
/// let normal = String::from_utf8(made.normal.content).unwrap();
/// assert!(normal.contains("q\n2 2 56 16 re\nW\nn\n0 g\nBT\n"));
/// assert!(normal.ends_with("(Push) Tj\nET\nQ\n"));
/// let down = String::from_utf8(made.down.unwrap().content).unwrap();
/// assert!(down.starts_with("q\n0.5 g\n0 0 60 20 re\nf\nQ\n"));
/// assert!(down.ends_with("(Down) Tj\nET\nQ\n"));
/// assert!(made.rollover.is_some());
/// ```
///
/// # Errors
///
/// [`Unmade::Limit`] with [`Cap::Text`] where a caption is longer than
/// [`Limits::text`] bytes; [`Unmade::NoBox`] for a box with no width or no
/// height; [`Unmade::NotFinite`] where a number of `look` or `da`'s size or
/// color isn't finite, or a caption laid out isn't, or reaches past `1e35`
/// from the box's origin; [`Unmade::Unencodable`] for a character `font`
/// has no code for, at its offset in the first caption made that has it, in
/// the order normal, rollover, down. Never [`Unmade::NotButton`],
/// [`Unmade::Icon`], [`Unmade::FontNotBindable`], or [`Unmade::Unreadable`],
/// which are the read layer's.
pub fn push_button<F: LayoutFont + ?Sized>(
    button: &PushButton<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    font: &F,
    limits: &Limits,
) -> Result<PushAppearances, Unmade> {
    let (rollover, down) = (
        or_normal(button.rollover_caption, button.caption),
        or_normal(button.down_caption, button.caption),
    );
    if [button.caption, rollover, down]
        .iter()
        .any(|c| c.len() > limits.text)
    {
        return Err(Unmade::Limit(Cap::Text));
    }
    let (bbox, matrix) = frame::placed(look)?;
    if da.font_size.is_some_and(|s| !s.is_finite()) || !frame::finite_color(da.color) {
        return Err(Unmade::NotFinite);
    }
    let look = &frame::in_range(look);
    let client = frame::client(look, bbox)?;
    let color = da.color.map_or(Color::Gray(0.0), frame::clamped);
    let lay = |caption: &str| line::one_line(caption, client, da.size(), TextAlign::Center, font);

    let normal = lay(button.caption)?;
    let made = |down: bool, laid: &Option<LaidOutText>| {
        let mut canvas = Canvas::new();
        match down {
            true => frame::down_frame(&mut canvas, look, bbox),
            false => frame::frame(&mut canvas, look, bbox),
        }
        if let Some(laid) = laid {
            canvas.with_state(|c| {
                c.clip_rect(client);
                c.fill_color(color);
                c.draw_layout(font_res.clone(), laid);
            });
        }
        let (content, needs) = canvas.finish(&Reserved::default());
        Appearance {
            bbox,
            matrix,
            content,
            needs,
            font_size: laid.as_ref().map_or(0.0, |laid| laid.font_size),
            overflowed: laid.as_ref().is_some_and(|laid| laid.overflowed),
        }
    };
    let (rollover, down) = match button.pressed_states {
        true => {
            let again = |caption: &str| match caption == button.caption {
                true => Ok(normal.clone()),
                false => lay(caption),
            };
            let rollover = again(rollover)?;
            let down = again(down)?;
            (Some(made(false, &rollover)), Some(made(true, &down)))
        }
        false => (None, None),
    };
    Ok(PushAppearances {
        normal: made(false, &normal),
        rollover,
        down,
    })
}

/// `caption`, or `normal` where it's none or empty.
fn or_normal<'a>(caption: Option<&'a str>, normal: &'a str) -> &'a str {
    caption.filter(|c| !c.is_empty()).unwrap_or(normal)
}
