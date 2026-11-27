//! Bounded PDF content-stream and Form XObject interpreter
//! (`CPDF_StreamContentParser`).
//!
//! Walks a [`Concat`] content stream (and, under [`Forms::Descend`], each Form
//! XObject it draws via `Do`) in PDFium's order, emitting typed [`Event`]s to a
//! caller closure while enforcing [`Limits`] across the whole page.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::{ControlFlow, Range};
use std::rc::Rc;
use std::sync::Arc;

use pdf_content_streams::{
    Concat, ImageStream, InlineImage, Limits as ContentLimits, Operand, Operation, Operator,
    Operators, Paint, ResCx, operations,
};
use pdf_fonts::{Error as FontError, FontId, FontKind, FontScope, Fonts};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::object::{Span, Value};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams};

use crate::clip;
use crate::error::Error;
use crate::fault::Fault;
use crate::geom::Rect;
use crate::limits::{Cap, Limits};
use crate::marked::{Mark, MarkProps, Marks};
use crate::path::{self, Builder, PathDrawn};
use crate::phases::phase;
use crate::reader::ObjReader;
use crate::state::{
    self, ActiveColorSpace, BlendState, ColorValues, DashPattern, RenderingIntent, SoftMask,
    SoftMaskKind, SoftMaskParams, State, nums,
};
use crate::text::{self, TextShow};
use crate::xobject::{
    Form, FormUse, ImageDrawn, ImageFacts, ImageOf, Refused, ShadingDrawn, UseId, XObject,
    dict_array4,
};

/// Sentinel owner `ObjectId` representing the top-level page's `/Resources` in
/// [`FontScope`]. Indirect PDF objects never have object number `0`.
const PAGE_OWNER: ObjectId = ObjectId::new(0, 0);

/// How Form XObjects invoked by `Do` are handled during interpretation.
///
/// Exhaustive on purpose: a `Do` either descends into the form stream or skips
/// entering Form XObjects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Forms {
    /// Enter each Form XObject invocation in place between [`Kind::FormBegin`]
    /// and [`Kind::FormEnd`] (refusing cycles and uses deeper than
    /// [`Limits::form_depth`] via [`Kind::FormRefused`]).
    #[default]
    Descend,
    /// Do not decode or enter Form XObject streams on `Do` (Image XObjects in
    /// the current stream still emit [`Kind::Image`]).
    Skip,
}

/// Selective event filter passed in [`Opts::wants`].
///
/// Skipping unwanted [`Kind::Op`], [`Kind::Text`], or [`Kind::Path`] events
/// avoids constructing visitor payloads while keeping state transitions,
/// [`Summary`] counters, [`Limits`] enforcement, and [`Error::NotFinite`]
/// checks identical to [`Wants::ALL`]. Structural events (`FormBegin`,
/// `FormEnd`, `FormRefused`, `Image`, `Shading`, `Ignored`, and marked-content
/// events) are always delivered.
///
/// Exhaustive on purpose so callers can construct literals like
/// `Wants { text: true, ..Wants::NONE }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Wants {
    /// Which operators emit [`Kind::Op`] before executing.
    pub ops: Operators,
    /// Whether non-empty strings emit [`Kind::Text`].
    pub text: bool,
    /// Whether painted/clipping paths emit [`Kind::Path`].
    pub paths: bool,
}

impl Wants {
    /// Deliver all operator, text, and path events.
    pub const ALL: Wants = Wants {
        ops: Operators::ALL,
        text: true,
        paths: true,
    };

    /// Suppress [`Kind::Op`], [`Kind::Text`], and [`Kind::Path`] while keeping
    /// structural events (`Form*`, `Image`, `Shading`, `Marked*`) and full
    /// state/cap validation.
    pub const NONE: Wants = Wants {
        ops: Operators::NONE,
        text: false,
        paths: false,
    };
}

impl Default for Wants {
    fn default() -> Self {
        Self::ALL
    }
}

/// Options controlling [`interpret`] and [`interpret_with`].
///
/// Exhaustive on purpose so callers can construct `Opts { forms, wants }`
/// directly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Opts {
    /// Form XObject traversal policy.
    pub forms: Forms,
    /// Which event kinds the visitor wants delivered.
    pub wants: Wants,
}

/// Source location and raw operation of an [`Event`].
///
/// Its `Debug` implementation reports byte ranges and lengths without printing
/// content-stream plaintext or operand bytes.
#[derive(Clone)]
pub struct At<'e> {
    /// Enclosing Form XObject invocation, or `None` in the page's own content
    /// stream.
    pub stream: Option<&'e UseId>,
    pub(crate) res: &'e ResCx,
    resources: Option<&'e Object>,
    page_resources: Option<&'e Object>,
    content: &'e [u8],
    operation: Option<Operation<'e>>,
}

impl fmt::Debug for At<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("At")
            .field("stream", &self.stream)
            .field("op_range", &self.op_range())
            .field("range", &self.range())
            .field("content_len", &self.content.len())
            .finish()
    }
}

impl<'e> At<'e> {
    /// Active `/Resources` object for the content stream containing this event
    /// (the Form XObject's own `/Resources`, or the page's when inherited or at
    /// top level).
    #[must_use]
    pub const fn resources(&self) -> Option<&'e Object> {
        self.resources
    }

    /// Top-level page `/Resources` object passed to [`interpret`] /
    /// [`interpret_with`].
    #[must_use]
    pub const fn page_resources(&self) -> Option<&'e Object> {
        self.page_resources
    }

    /// Raw decoded bytes of the content stream containing this event.
    #[must_use]
    pub const fn content(&self) -> &'e [u8] {
        self.content
    }

    /// Underlying `pdf-content-streams` [`Operation`], or `None` for implicit
    /// end-of-stream [`Kind::MarkedEnd`] closures.
    #[must_use]
    pub const fn operation(&self) -> Option<&Operation<'e>> {
        self.operation.as_ref()
    }

    /// Parsed [`Operator`] (`Operator::BeginInlineImage` for inline images;
    /// `Operator::Other` for unknown operators or implicit end-of-stream
    /// closures).
    #[must_use]
    pub fn operator(&self) -> Operator {
        self.operation
            .as_ref()
            .map_or(Operator::Other, Operation::operator)
    }

    /// Raw operator keyword bytes (`b"BI"` for inline images; `b""` at implicit
    /// end-of-stream closure).
    #[must_use]
    pub fn name(&self) -> &'e [u8] {
        self.operation.as_ref().map_or(&b""[..], Operation::name)
    }

    /// Byte range of the operator token itself (or `BI..EI` for an inline
    /// image; `len..len` at implicit end-of-stream closure).
    #[must_use]
    pub fn op_range(&self) -> Range<usize> {
        match &self.operation {
            Some(op) => op.at(),
            None => self.content.len()..self.content.len(),
        }
    }

    /// Byte range from the first operand (if any) through the end of the
    /// operator.
    #[must_use]
    pub fn range(&self) -> Range<usize> {
        match &self.operation {
            Some(op) => op.range(),
            None => self.content.len()..self.content.len(),
        }
    }

    /// Inline image view when this operation is a `BI..ID..EI` inline image.
    #[must_use]
    pub fn inline_image(&self) -> Option<InlineImage<'e>> {
        self.operation.as_ref()?.inline_image()
    }

    /// Operand `i` counted from the last operand (`0` = last operand, matching
    /// PDFium).
    #[must_use]
    pub fn operand(&self, i: usize) -> Option<Operand<'e>> {
        self.operation.as_ref()?.arg(i)
    }

    /// All operands of this operation in forward written order.
    pub fn operands(&self) -> impl Iterator<Item = Operand<'e>> + '_ {
        self.operation.iter().flat_map(Operation::operands)
    }
}

/// One step delivered to the visitor passed to [`interpret`] /
/// [`interpret_with`].
#[derive(Debug)]
#[non_exhaustive]
pub struct Event<'e> {
    /// Where in the page or Form XObject content stream this event occurred.
    pub at: At<'e>,
    /// Graphics and text state active at this event:
    /// - for [`Kind::Op`], the state *before* the operator executes;
    /// - for [`Kind::Text`], the state where the operator's text begins (after
    ///   `'` / `"` have advanced to the next line);
    /// - for [`Kind::Path`], the state (including `clip`) *before* any new clip
    ///   from this path takes effect.
    pub state: &'e State,
    /// Open marked-content sequences enclosing this event (for
    /// `MarkedBegin`/`MarkedEnd`, the sequences *outside* the one opening or
    /// closing).
    pub marks: &'e Marks,
    /// Event payload.
    pub kind: Kind<'e>,
}

/// Payload of an [`Event`].
#[derive(Debug)]
#[non_exhaustive]
pub enum Kind<'e> {
    /// An operation (or inline image), emitted before its state transition.
    Op,
    /// A non-empty string shown by `Tj`, `TJ`, `'`, or `"`.
    Text(TextShow<'e>),
    /// Entering a Form XObject (`Do`); its operations follow until the matching
    /// [`Kind::FormEnd`].
    FormBegin(FormUse<'e>),
    /// Exiting a Form XObject (`Do`) previously opened by [`Kind::FormBegin`].
    FormEnd(FormUse<'e>),
    /// A Form XObject (`Do`) skipped due to recursion or depth cap.
    FormRefused {
        /// Indirect object ID of the Form XObject.
        form: ObjectId,
        /// Reason it was not entered.
        why: Refused,
    },
    /// A path painted or turned into a clip.
    Path(PathDrawn<'e>),
    /// An Image XObject (`Do`) or inline image (`BI..EI`) placed on the page.
    Image(ImageDrawn),
    /// A shading painted by `sh`.
    Shading(ShadingDrawn),
    /// A stream XObject invoked by `Do` whose `/Subtype` PDFium ignores (such
    /// as `/PS` or an unknown subtype).
    Ignored(ObjectId),
    /// Opening a marked-content sequence (`BMC` or `BDC`).
    MarkedBegin(&'e Mark),
    /// Closing a marked-content sequence (`EMC` or implicit end-of-stream).
    MarkedEnd(&'e Mark),
    /// A marked-content point (`MP` or `DP`).
    MarkedPoint(&'e Mark),
}

/// Aggregate work and nesting metrics returned by [`interpret`] and
/// [`interpret_with`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Summary {
    /// Operations interpreted across the page and all entered Form XObject
    /// uses.
    pub operators: usize,
    /// Glyphs placed across the page and all entered Form XObject uses.
    pub glyphs: usize,
    /// Total content bytes interpreted across the page and every Form XObject
    /// use.
    pub interpreted: usize,
    /// Bytes charged against [`Limits::decompressed`] (page content, distinct
    /// Form XObject streams, font streams, and CID glyph outlines).
    pub decompressed: usize,
    /// Placed path segments across painted and clipping paths.
    pub segments: usize,
    /// Form XObject uses entered.
    pub uses: usize,
    /// Form XObject uses refused (`Cycle` or `Depth`).
    pub refused: usize,
    /// Deepest Form XObject nesting reached (`0` if no forms were entered).
    pub deepest_form: usize,
    /// Peak simultaneous `q` saves across active streams.
    pub deepest_q: usize,
    /// Deepest nested `BMC`/`BDC` count within a single stream.
    pub deepest_marked: usize,
    /// `true` if the visitor returned [`ControlFlow::Break`].
    pub stopped: bool,
}

/// Interprets `content` with `resources` (the page's or form's `/Resources`
/// object as written), resolving fonts through `fonts` and indirect
/// objects/streams through `objects`, without collecting non-fatal [`Fault`]s.
///
/// # Errors
///
/// Returns [`Error`] if content lexing fails, a Form XObject stream fails to
/// decode, a [`Limits`] cap is exceeded, placed coordinates overflow `f32`
/// ([`Error::NotFinite`]), or `objects` fails to read an indirect object/stream
/// ([`Error::Unreadable`]).
pub fn interpret<'o, O>(
    objects: O,
    content: &Concat<'_>,
    resources: Option<&'o Object>,
    limits: &Limits,
    fonts: &Fonts,
    opts: Opts,
    visit: impl FnMut(Event<'_>) -> ControlFlow<()>,
) -> Result<Summary, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    interpret_with(
        objects,
        content,
        resources,
        limits,
        fonts,
        opts,
        &mut (),
        visit,
    )
}

/// Interprets `content` like [`interpret`], reporting recoverable ISO 32000-2
/// departures to `sink`.
///
/// Passing `&mut ()` disables fault collection at zero cost and produces the
/// exact same events, [`Summary`], and [`Error`]s as any active sink.
///
/// # Errors
///
/// Same error conditions as [`interpret`].
#[allow(clippy::too_many_arguments)]
pub fn interpret_with<'o, O, S>(
    objects: O,
    content: &Concat<'_>,
    resources: Option<&'o Object>,
    limits: &Limits,
    fonts: &Fonts,
    opts: Opts,
    sink: &mut S,
    mut visit: impl FnMut(Event<'_>) -> ControlFlow<()>,
) -> Result<Summary, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    phase!(DEBUG, "interpret", bytes = content.bytes().len());
    let page_res = ResCx::read(objects, resources, &limits.content)?;
    let reader = ObjReader::new(objects, sink);
    if content.bytes().len() > limits.decompressed {
        return Err(Error::Limit(Cap::Decompressed));
    }
    let budget = limits.decompressed - content.bytes().len();
    let mut run = Run {
        reader: &reader,
        lim: limits,
        page: resources,
        page_res: &page_res,
        fonts: fonts.scope(),
        opts,
        visit: &mut visit,
        summary: Summary::default(),
        saved: 0,
        path: Vec::new(),
        chain: Vec::new(),
        xobjects: HashMap::new(),
        outlines: HashSet::new(),
        budget,
    };
    let top = Frame {
        content: content.bytes(),
        form_id: None,
        stream: None,
        resources,
        owner: PAGE_OWNER,
        res: &page_res,
    };
    match run
        .stream(&top, State::default(), Marks::default())
        .map_err(|e| e.within(None))
    {
        Ok(()) => {}
        Err(Halt::Break) => run.summary.stopped = true,
        Err(Halt::Fail(e)) => return Err(e),
    }
    if let Some(unreadable) = reader.take_failure() {
        return Err(Error::Unreadable(unreadable));
    }
    run.summary.decompressed = limits.decompressed - run.budget;
    Ok(run.summary)
}

struct Run<'a, 'c, 'o, O, S: ?Sized> {
    reader: &'a ObjReader<'c, O, S>,
    lim: &'a Limits,
    page: Option<&'o Object>,
    page_res: &'a ResCx,
    fonts: FontScope<'a>,
    opts: Opts,
    visit: &'a mut dyn FnMut(Event<'_>) -> ControlFlow<()>,
    summary: Summary,
    saved: usize,
    path: Vec<u32>,
    chain: Vec<ObjectId>,
    xobjects: HashMap<ObjectId, Rc<Drawn<'o>>>,
    outlines: HashSet<(FontId, u32)>,
    budget: usize,
}

enum Drawn<'o> {
    Form {
        form: Form<'o>,
        res: Option<Box<ResCx>>,
    },
    SkippedForm,
    Image(ImageFacts),
    Other,
    None,
}

struct Frame<'f, 'o> {
    content: &'f [u8],
    form_id: Option<ObjectId>,
    stream: Option<&'f UseId>,
    resources: Option<&'o Object>,
    owner: ObjectId,
    res: &'f ResCx,
}

struct Local {
    state: State,
    saved: Vec<State>,
    path: Builder,
    marks: Marks,
    own: usize,
    clip_glyphs: Vec<crate::geom::Quad>,
    clip_texts: usize,
}

enum Stop {
    Lex(pdf_content_streams::Error),
    Halt(Halt),
}

enum Halt {
    Break,
    Fail(Error),
}

impl From<pdf_content_streams::Error> for Stop {
    fn from(e: pdf_content_streams::Error) -> Self {
        Stop::Lex(e)
    }
}

impl From<Error> for Stop {
    fn from(e: Error) -> Self {
        Stop::Halt(Halt::Fail(e))
    }
}

impl Stop {
    fn within(self, form: Option<ObjectId>) -> Halt {
        match self {
            Stop::Lex(error) => Halt::Fail(Error::Content { form, error }),
            Stop::Halt(halt) => halt,
        }
    }
}

fn past(cap: Cap) -> Stop {
    Stop::Halt(Halt::Fail(Error::Limit(cap)))
}

fn not_finite() -> Stop {
    Stop::Halt(Halt::Fail(Error::NotFinite))
}

impl<'o, O: Resolve<'o> + Streams + Copy, S: Sink<Fault> + ?Sized> Run<'_, '_, 'o, O, S> {
    fn tell_at(&self, frame: &Frame<'_, 'o>, offset: usize, fault: Fault) {
        self.reader.tell_at(frame.form_id, offset, fault);
    }

    fn check_reader(&self) -> Result<(), Stop> {
        match self.reader.take_failure() {
            Some(unreadable) => Err(Stop::from(Error::Unreadable(unreadable))),
            None => Ok(()),
        }
    }

    fn stream(&mut self, frame: &Frame<'_, 'o>, state: State, marks: Marks) -> Result<(), Stop> {
        self.summary.interpreted = self.summary.interpreted.saturating_add(frame.content.len());
        if self.summary.interpreted > self.lim.interpreted {
            return Err(past(Cap::Interpreted));
        }
        let mut local = Local {
            state,
            saved: Vec::new(),
            path: Builder::default(),
            marks,
            own: 0,
            clip_glyphs: Vec::new(),
            clip_texts: 0,
        };
        let mut run_ops = || -> Result<(), Stop> {
            for step in operations(frame.content, frame.res, &self.lim.content) {
                let step = step?;
                self.step(frame, &mut local, step)?;
            }
            Ok(())
        };
        let done = run_ops();
        self.saved -= local.saved.len();
        done?;

        if local.own > 0 {
            self.tell_at(
                frame,
                frame.content.len(),
                Fault::UnclosedMarked { count: local.own },
            );
        }
        let eof_at = At {
            stream: frame.stream,
            res: frame.res,
            resources: frame.resources,
            page_resources: self.page,
            content: frame.content,
            operation: None,
        };
        while local.own > 0 {
            local.own -= 1;
            if let Some(closed) = local.marks.pop() {
                self.emit(
                    &eof_at,
                    &local.state,
                    &local.marks,
                    Kind::MarkedEnd(closed.mark()),
                )?;
            }
        }
        Ok(())
    }

    fn step(
        &mut self,
        frame: &Frame<'_, 'o>,
        local: &mut Local,
        step: Operation<'_>,
    ) -> Result<(), Stop> {
        self.summary.operators += 1;
        if self.summary.operators > self.lim.operators {
            return Err(past(Cap::Operators));
        }
        let op = step.operator();
        let inline = step.inline_image();
        let at = At {
            stream: frame.stream,
            res: frame.res,
            resources: frame.resources,
            page_resources: self.page,
            content: frame.content,
            operation: Some(step.clone()),
        };
        if self.opts.wants.ops.contains(op) {
            self.emit(&at, &local.state, &local.marks, Kind::Op)?;
        }
        if inline.is_none() && op.builds_path() {
            local.path.build(op, &step);
            if local.state.clip.segments() + local.path.len() > self.lim.segments {
                return Err(past(Cap::Segments));
            }
            return Ok(());
        }
        local.path.interrupt();

        if let Some(image) = inline {
            return self.inline_image(local, &at, &image);
        }

        match op {
            Operator::Transform
            | Operator::MoveLine
            | Operator::MoveLineLeading
            | Operator::NextLine => {
                local.state.set(op, &step, &self.lim.content);
                if !local.state.is_finite() {
                    return Err(not_finite());
                }
            }
            Operator::LineWidth
            | Operator::LineCap
            | Operator::LineJoin
            | Operator::MiterLimit
            | Operator::Dash
            | Operator::RenderingIntent
            | Operator::Flatness
            | Operator::StrokeGray
            | Operator::FillGray
            | Operator::StrokeRgb
            | Operator::FillRgb
            | Operator::StrokeCmyk
            | Operator::FillCmyk
            | Operator::StrokeColor
            | Operator::FillColor
            | Operator::BeginText
            | Operator::TextMatrix
            | Operator::CharSpace
            | Operator::WordSpace
            | Operator::Scale
            | Operator::Leading
            | Operator::Rise
            | Operator::RenderMode => local.state.set(op, &step, &self.lim.content),
            Operator::StrokeColorSpace | Operator::FillColorSpace => {
                local.state.set(op, &step, &self.lim.content);
                if self.reader.wants_faults() {
                    let space = if op == Operator::StrokeColorSpace {
                        &local.state.stroke_color_space
                    } else {
                        &local.state.fill_color_space
                    };
                    if let ActiveColorSpace::Named(name) = space
                        && self.entry(frame, b"ColorSpace", name).is_none()
                    {
                        self.tell_at(
                            frame,
                            step.at().start,
                            Fault::MissingResource {
                                category: "ColorSpace",
                                name: name.to_vec(),
                            },
                        );
                    }
                    self.check_reader()?;
                }
            }
            Operator::StrokeColorN | Operator::FillColorN => {
                local.state.set(op, &step, &self.lim.content);
                if self.reader.wants_faults() {
                    let pat = if op == Operator::StrokeColorN {
                        local.state.stroke_pattern.as_deref()
                    } else {
                        local.state.fill_pattern.as_deref()
                    };
                    if let Some(name) = pat
                        && self.entry(frame, b"Pattern", name).is_none()
                    {
                        self.tell_at(
                            frame,
                            step.at().start,
                            Fault::MissingResource {
                                category: "Pattern",
                                name: name.to_vec(),
                            },
                        );
                    }
                    self.check_reader()?;
                }
            }
            Operator::Save => {
                if local.saved.len() >= self.lim.states {
                    return Err(past(Cap::States));
                }
                local.saved.push(local.state.clone());
                self.saved += 1;
                self.summary.deepest_q = self.summary.deepest_q.max(self.saved);
            }
            Operator::Restore => match local.saved.pop() {
                Some(restored) => {
                    local.state = restored;
                    self.saved -= 1;
                }
                None => self.tell_at(frame, step.at().start, Fault::ExtraRestore),
            },
            Operator::Font => {
                let name = string_or_name(step.arg(1), &self.lim.content);
                let name_bytes = name.as_deref().unwrap_or_default();
                let prev = local.state.text.font;
                self.pick_font(frame, &mut local.state, name_bytes, prev, step.at().start)?;
                if let Some([size]) = nums(&step) {
                    local.state.text.size = size;
                }
            }
            Operator::ExtGState => self.ext_gstate(frame, &mut local.state, &step)?,
            Operator::Show => self.show(local, &at, step.arg(0))?,
            Operator::NextLineShow | Operator::NextLineShowSpaced => {
                local.state.next_line_of(op, &step);
                if !local.state.is_finite() {
                    return Err(not_finite());
                }
                self.show(local, &at, step.arg(0))?;
            }
            Operator::ShowArray => self.show_array(local, &at, step.arg(0))?,
            Operator::EndText => end_text(local),
            Operator::Draw => self.draw(frame, local, &at, &step)?,
            Operator::Shade => self.shade(frame, local, &at, &step)?,
            Operator::BeginMarked | Operator::BeginMarkedProps => {
                self.begin_marked(frame, local, &at, &step, op)?;
            }
            Operator::EndMarked => self.end_marked(frame, local, &at, step.at().start)?,
            Operator::MarkedPoint | Operator::MarkedPointProps => {
                self.point(frame, local, &at, &step, op)?;
            }
            _ => {
                if let Some(paint) = op.paint() {
                    return self.paint(local, &at, paint);
                }
            }
        }
        Ok(())
    }

    fn emit(
        &mut self,
        at: &At<'_>,
        state: &State,
        marks: &Marks,
        kind: Kind<'_>,
    ) -> Result<(), Stop> {
        hand(self.visit, at, state, marks, kind)
    }

    fn pick_font(
        &mut self,
        frame: &Frame<'_, 'o>,
        state: &mut State,
        name: &[u8],
        unloaded: Option<FontId>,
        offset: usize,
    ) -> Result<(), Stop> {
        let holder = self.holder(frame, b"Font");
        self.check_reader()?;
        let picked = match holder {
            Some((owner, resources)) => self
                .fonts
                .font(
                    self.reader.objects,
                    owner,
                    Some(resources),
                    name,
                    &self.lim.fonts,
                    &mut self.budget,
                )
                .map_err(|err| match err {
                    FontError::TooLarge => past(Cap::Decompressed),
                    FontError::Unreadable(u) => Stop::from(Error::Unreadable(u)),
                    _ => past(Cap::Decompressed),
                })?,
            None => None,
        };
        match picked {
            Some(id) if self.fonts.get(id).loaded() => {
                state.text.stock = false;
                state.text.font = Some(id);
            }
            Some(_) => {
                state.text.font = unloaded;
            }
            None => {
                self.tell_at(
                    frame,
                    offset,
                    Fault::MissingResource {
                        category: "Font",
                        name: name.to_vec(),
                    },
                );
                state.text.stock = true;
                state.text.font = Some(self.fonts.stock());
            }
        }
        Ok(())
    }

    fn ext_gstate(
        &mut self,
        frame: &Frame<'_, 'o>,
        state: &mut State,
        step: &Operation<'_>,
    ) -> Result<(), Stop> {
        let Some(name) = string_or_name(step.arg(0), &self.lim.content) else {
            return Ok(());
        };
        let gs = self
            .entry(frame, b"ExtGState", &name)
            .and_then(|val| self.reader.dict_of_value(val));
        self.check_reader()?;
        let Some(gs) = gs else {
            self.tell_at(
                frame,
                step.at().start,
                Fault::MissingResource {
                    category: "ExtGState",
                    name,
                },
            );
            return Ok(());
        };

        let finite = |key: &[u8]| -> Option<f32> {
            let v = self.reader.float_of(gs.get(key)?)?;
            v.is_finite().then_some(v)
        };
        if let Some(width) = finite(b"LW") {
            state.line_width = width;
        }
        if let Some(n) = finite(b"LC") {
            state.line_cap = state::cap(state::int(n));
        }
        if let Some(n) = finite(b"LJ") {
            state.line_join = state::join(state::int(n));
        }
        if let Some(limit) = finite(b"ML") {
            state.miter_limit = limit;
        }
        if let Some(d_arr) = self.reader.array_for(gs, b"D")
            && d_arr.len() >= 2
            && let Some(pat_arr) = self.reader.array_of(&d_arr[0])
            && let Some(phase) = self.reader.float_of(&d_arr[1]).filter(|v| v.is_finite())
        {
            let dashes: Vec<f32> = pat_arr
                .iter()
                .filter_map(|v| self.reader.float_of(v))
                .filter(|v| v.is_finite())
                .collect();
            state.dash = DashPattern {
                array: dashes.into(),
                phase,
            };
        }
        if let Some(ri) = self.reader.name_or_bytes_for(gs, b"RI") {
            state.rendering_intent = RenderingIntent::from_name(ri);
        }
        if let Some(fl) = finite(b"FL") {
            state.flatness = fl;
        }
        if let Some(ca) = finite(b"CA") {
            state.stroke_alpha = ca.clamp(0.0, 1.0);
        }
        if let Some(ca) = finite(b"ca") {
            state.fill_alpha = ca.clamp(0.0, 1.0);
        }
        if let Some(ais) = self.reader.bool_for(gs, b"AIS") {
            state.alpha_is_shape = ais;
        }
        if let Some(tk) = self.reader.bool_for(gs, b"TK") {
            state.text_knockout = tk;
        }
        if let Some(op_stroke) = self.reader.bool_for(gs, b"OP") {
            state.stroke_overprint = op_stroke;
            if gs.get(b"op").is_none() {
                state.fill_overprint = op_stroke;
            }
        }
        if let Some(op_fill) = self.reader.bool_for(gs, b"op") {
            state.fill_overprint = op_fill;
        }
        if let Some(opm) = gs.get(b"OPM").and_then(|v| self.reader.integer_of(v)) {
            state.overprint_mode = opm;
        }
        if let Some(bm_val) = gs.get(b"BM") {
            if let Some(bm_name) = self
                .reader
                .seen(bm_val)
                .and_then(|o| o.as_name().or_else(|| o.as_bytes()))
            {
                state.blend_mode = BlendState::from_name(bm_name);
            } else if let Some(arr) = self.reader.array_of(bm_val) {
                for item in arr {
                    if let Some(bm_name) = self
                        .reader
                        .seen(item)
                        .and_then(|o| o.as_name().or_else(|| o.as_bytes()))
                    {
                        let candidate = BlendState::from_name(bm_name);
                        if !matches!(candidate, BlendState::Other(_)) {
                            state.blend_mode = candidate;
                            break;
                        }
                    }
                }
            }
        }
        if let Some(smask_val) = gs.get(b"SMask") {
            if self.reader.seen(smask_val).and_then(Object::as_name) == Some(b"None") {
                state.soft_mask = SoftMask::None;
                state.soft_mask_params = None;
            } else if let Some(smask_dict) = self.reader.dict_of_value(smask_val) {
                let kind = match self.reader.name_or_bytes_for(smask_dict, b"S") {
                    Some(b"Alpha") => Some(SoftMaskKind::Alpha),
                    Some(b"Luminosity") => Some(SoftMaskKind::Luminosity),
                    _ => None,
                };
                let group = smask_dict
                    .get(b"G")
                    .and_then(Object::as_reference)
                    .filter(|&id| self.reader.stream(id).is_some());
                let backdrop = self
                    .reader
                    .quiet_array_for(smask_dict, b"BC")
                    .and_then(|arr| {
                        let mut vals = Vec::with_capacity(arr.len().min(16));
                        for item in arr.iter().take(16) {
                            let v = self.reader.quiet_float_of(item)?;
                            if !v.is_finite() {
                                return None;
                            }
                            vals.push(v);
                        }
                        Some(ColorValues::from_slice(&vals))
                    });
                let transfer = smask_dict.get(b"TR").and_then(|tr| {
                    let seen_name = self.reader.quiet_seen(tr).and_then(Object::as_name);
                    if matches!(seen_name, Some(b"Identity" | b"Default")) {
                        None
                    } else {
                        Some(Arc::new(tr.clone()))
                    }
                });
                state.soft_mask = SoftMask::Mask { kind, group };
                state.soft_mask_params = Some(SoftMaskParams {
                    backdrop,
                    transfer,
                    ctm: state.ctm,
                });
            }
        }
        if let Some(font_arr) = self.reader.array_for(gs, b"Font") {
            let font_name = self.reader.byte_string_at(font_arr, 0);
            self.pick_font(frame, state, &font_name, None, step.at().start)?;
            let size = font_arr
                .get(1)
                .and_then(|v| self.reader.float_of(v))
                .unwrap_or(0.0);
            if size.is_finite() {
                state.text.size = size;
            }
        }
        self.check_reader()
    }

    /// `Tj`, `'` or `"`: the bytes of their string operand, of a name, or
    /// `true` or `false` as written, as PDFium's `Handle_ShowText` takes them
    /// (`GetString`).
    fn show(
        &mut self,
        local: &mut Local,
        at: &At<'_>,
        operand: Option<Operand<'_>>,
    ) -> Result<(), Stop> {
        let Some(string) = string_or_name(operand, &self.lim.content) else {
            return Ok(());
        };
        if string.is_empty() {
            return Ok(());
        }
        self.text(local, at, &[(string, 0.0)], 0.0)
    }

    fn show_array(
        &mut self,
        local: &mut Local,
        at: &At<'_>,
        operand: Option<Operand<'_>>,
    ) -> Result<(), Stop> {
        let Some(array) = operand.filter(Operand::is_array) else {
            return Ok(());
        };
        let items: Vec<Operand<'_>> = array.elements().collect();
        let adjust =
            |item: &Operand<'_>| Some(item.number()).filter(|n| n.is_finite() && *n != 0.0);
        if !items.iter().any(Operand::is_string) {
            let text = &mut local.state.text;
            for n in items.iter().filter_map(adjust) {
                text.pen.x -= n * text.size / 1000.0 * text.scale;
            }
            return if text.pen.x.is_finite() {
                Ok(())
            } else {
                Err(not_finite())
            };
        }
        let mut strings: Vec<(Vec<u8>, f32)> = Vec::new();
        let mut first = 0.0;
        let mut passed_over = false;
        for item in items {
            if !item.is_string() {
                let n = adjust(&item).unwrap_or(0.0);
                match strings.last_mut() {
                    Some((_, after)) => *after += n,
                    None => first += n,
                }
                continue;
            }
            match string_only(Some(item), &self.lim.content).filter(|s| !s.is_empty()) {
                Some(s) => strings.push((s, 0.0)),
                None => passed_over = true,
            }
        }
        if passed_over && let Some((_, after)) = strings.last_mut() {
            *after = 0.0;
        }
        self.text(local, at, &strings, first)
    }

    fn text(
        &mut self,
        local: &mut Local,
        at: &At<'_>,
        strings: &[(Vec<u8>, f32)],
        first: f32,
    ) -> Result<(), Stop> {
        let wanted = self.opts.wants.text;
        let id = local.state.text.font;
        let Some(font) = id.map(|id| self.fonts.get(id)) else {
            if wanted {
                for (string, _) in strings {
                    let show = TextShow::new(string, &local.state, None, 0.0);
                    hand(self.visit, at, &local.state, &local.marks, Kind::Text(show))?;
                }
            }
            return Ok(());
        };
        let vertical = font.is_vertical();
        let adjust = |state: &mut State, n: f32| {
            let text = &mut state.text;
            if vertical {
                text.pen.y -= n * text.size / 1000.0;
            } else {
                text.pen.x -= n * text.size / 1000.0 * text.scale;
            }
        };
        let clips = font.kind() != FontKind::Type3 && local.state.text.mode.clips();
        let guessed = font.approx().codes || font.approx().widths;
        if clips && !strings.is_empty() {
            local.clip_texts += 1;
        }
        adjust(&mut local.state, first);
        let mut run = 0.0;
        for (i, (string, after)) in strings.iter().enumerate() {
            let (end, glyphs) = text::advance(font, &local.state.text, string, run);
            self.summary.glyphs = self.summary.glyphs.saturating_add(glyphs);
            if self.summary.glyphs > self.lim.glyphs {
                return Err(past(Cap::Glyphs));
            }
            let outlines = &mut self.outlines;
            let first_seen = |glyph| id.is_some_and(|fid| outlines.insert((fid, glyph)));
            font.charge_outlines(string, first_seen, &mut self.budget)
                .map_err(|_| past(Cap::Decompressed))?;
            let show = TextShow::new(string, &local.state, Some(font), run);
            if !show.bounded(font, glyphs, end) {
                return Err(not_finite());
            }
            if clips && local.clip_texts <= clip::TEXTS {
                local.clip_glyphs.extend(show.glyphs().map(|g| g.quad));
            }
            if wanted {
                hand(self.visit, at, &local.state, &local.marks, Kind::Text(show))?;
            }
            let text = &mut local.state.text;
            text.guessed = text.guessed || guessed || font.width_guessed_in(string);
            run = end;
            if i + 1 < strings.len() {
                run -= after * text.size / 1000.0;
            }
        }
        let text = &mut local.state.text;
        if vertical {
            text.pen.y += run;
        } else {
            text.pen.x += run * text.scale;
        }
        if let Some((_, after)) = strings.last() {
            adjust(&mut local.state, *after);
        }
        if local.state.is_finite() {
            Ok(())
        } else {
            Err(not_finite())
        }
    }

    fn paint(&mut self, local: &mut Local, at: &At<'_>, paint: Paint) -> Result<(), Stop> {
        let Some(ended) = local.path.end(paint, &local.state) else {
            return Ok(());
        };
        self.summary.segments = self.summary.segments.saturating_add(ended.segs.len());
        if self.summary.segments > self.lim.segments {
            return Err(past(Cap::Segments));
        }
        if !(path::finite(ended.segs) && ended.bounds.is_finite()) {
            return Err(not_finite());
        }
        if self.opts.wants.paths {
            let drawn = PathDrawn {
                segments: ended.segs,
                user_segments: ended.user_segs,
                paint: ended.paint,
                clip: ended.clip,
                bounds: ended.bounds,
            };
            self.emit(at, &local.state, &local.marks, Kind::Path(drawn))?;
        }
        if let Some(rule) = ended.clip {
            local.state.clip = local.state.clip.with_path(ended.segs, rule, ended.points);
            if local.state.clip.segments() > self.lim.segments {
                return Err(past(Cap::Segments));
            }
        }
        Ok(())
    }

    fn inline_image(
        &mut self,
        local: &Local,
        at: &At<'_>,
        image: &InlineImage<'_>,
    ) -> Result<(), Stop> {
        if image.stream() == ImageStream::Missing {
            return Ok(());
        }
        let facts = ImageFacts::from_inline(image, &self.lim.content);
        let drawn = ImageDrawn::new(
            ImageOf::Inline { data: image.data() },
            &facts,
            local.state.ctm,
        );
        if !drawn.quad.is_finite() {
            return Err(not_finite());
        }
        self.emit(at, &local.state, &local.marks, Kind::Image(drawn))
    }

    fn draw(
        &mut self,
        frame: &Frame<'_, 'o>,
        local: &Local,
        at: &At<'_>,
        step: &Operation<'_>,
    ) -> Result<(), Stop> {
        let Some(name) = string_or_name(step.arg(0), &self.lim.content) else {
            return Ok(());
        };
        let id = self
            .entry(frame, b"XObject", &name)
            .and_then(Object::as_reference);
        self.check_reader()?;
        let Some(id) = id else {
            self.tell_at(
                frame,
                step.at().start,
                Fault::MissingResource {
                    category: "XObject",
                    name,
                },
            );
            return Ok(());
        };
        let (state, marks) = (&local.state, &local.marks);
        let drawn = self.xobject(id)?;
        let (form, res) = match &*drawn {
            Drawn::Form { form, res } => (form, res),
            Drawn::SkippedForm => return Ok(()),
            Drawn::Image(facts) => {
                let image = ImageDrawn::new(ImageOf::XObject(id), facts, state.ctm);
                if !image.quad.is_finite() {
                    return Err(not_finite());
                }
                return self.emit(at, state, marks, Kind::Image(image));
            }
            Drawn::Other => return self.emit(at, state, marks, Kind::Ignored(id)),
            Drawn::None => return Ok(()),
        };
        let why = if self.chain.contains(&id) {
            Some(Refused::Cycle)
        } else if self.chain.len() >= self.lim.form_depth {
            Some(Refused::Depth)
        } else {
            None
        };
        if let Some(why) = why {
            self.summary.refused += 1;
            let fault = match why {
                Refused::Cycle => Fault::FormCycle { form: id },
                Refused::Depth => Fault::FormDepth { form: id },
            };
            self.tell_at(frame, step.at().start, fault);
            return self.emit(at, state, marks, Kind::FormRefused { form: id, why });
        }
        let matrix = form.matrix.then(state.ctm);
        let clip_rect = form.bbox.map(|bbox| matrix.apply_rect(bbox));
        if !(matrix.is_finite() && clip_rect.is_none_or(Rect::is_finite)) {
            return Err(not_finite());
        }
        self.path.push(step.at().start as u32);
        self.chain.push(id);
        self.summary.uses += 1;
        self.summary.deepest_form = self.summary.deepest_form.max(self.chain.len());
        let use_id = UseId::new(&self.path);
        let shown = FormUse {
            form: id,
            id: &use_id,
            matrix,
            bbox: form.bbox,
            oc: form.oc,
            group: form.group,
        };
        let (resources, owner, res_cx) = match form.resources {
            Some(own) => {
                let res_cx = res.as_deref().unwrap_or(self.page_res);
                (Some(own), id, res_cx)
            }
            None => (frame.resources, frame.owner, frame.res),
        };
        let inner = Frame {
            content: &form.data,
            form_id: Some(id),
            stream: Some(&use_id),
            resources,
            owner,
            res: res_cx,
        };
        let drawn_in = self
            .emit(at, state, marks, Kind::FormBegin(shown))
            .and_then(|()| {
                let begun = state.form(shown.matrix, shown.bbox);
                if begun.clip.segments() > self.lim.segments {
                    return Err(past(Cap::Segments));
                }
                self.stream(&inner, begun, marks.clone())
            })
            .map_err(|e| Stop::Halt(e.within(Some(id))));
        self.path.pop();
        self.chain.pop();
        drawn_in?;
        self.emit(at, state, marks, Kind::FormEnd(shown))
    }

    fn xobject(&mut self, id: ObjectId) -> Result<Rc<Drawn<'o>>, Stop> {
        if let Some(known) = self.xobjects.get(&id) {
            return Ok(known.clone());
        }
        let decode_forms = self.opts.forms == Forms::Descend;
        let read = XObject::read(self.reader, id, self.budget, decode_forms)?;
        let drawn = match read {
            XObject::Form(form) => {
                self.budget = self
                    .budget
                    .checked_sub(form.data.len())
                    .ok_or_else(|| past(Cap::Decompressed))?;
                let res = match form
                    .resources
                    .filter(|r| self.category_dict(r, b"ColorSpace").is_some())
                {
                    Some(r) => {
                        let cx = ResCx::read(self.reader.objects, Some(r), &self.lim.content)
                            .map_err(Error::Unreadable)?;
                        Some(Box::new(cx))
                    }
                    None => None,
                };
                self.check_reader()?;
                Drawn::Form { form, res }
            }
            XObject::SkippedForm => Drawn::SkippedForm,
            XObject::Image(facts) => Drawn::Image(facts),
            XObject::Other => Drawn::Other,
            XObject::None => Drawn::None,
            XObject::TooLarge => return Err(past(Cap::Decompressed)),
        };
        let drawn = Rc::new(drawn);
        self.xobjects.insert(id, drawn.clone());
        Ok(drawn)
    }

    fn shade(
        &mut self,
        frame: &Frame<'_, 'o>,
        local: &Local,
        at: &At<'_>,
        step: &Operation<'_>,
    ) -> Result<(), Stop> {
        let Some(name) = string_or_name(step.arg(0), &self.lim.content) else {
            return Ok(());
        };
        let written = self.entry(frame, b"Shading", &name);
        self.check_reader()?;
        let Some(written) = written else {
            self.tell_at(
                frame,
                step.at().start,
                Fault::MissingResource {
                    category: "Shading",
                    name,
                },
            );
            return Ok(());
        };
        let Some(dict) = self.reader.dict_of_value(written) else {
            self.check_reader()?;
            return Ok(());
        };
        let bbox = dict.get(b"BBox").is_some().then(|| {
            let [a, b, c, d] = dict_array4(self.reader, dict, b"BBox").unwrap_or([0.0; 4]);
            local.state.ctm.apply_rect(Rect::new(a, b, c, d))
        });
        self.check_reader()?;
        let bounds = match (local.state.clip_bounds(), bbox) {
            (Some(clip), Some(bbox)) => Some(clip.intersect_or_empty(bbox)),
            (clip, bbox) => clip.or(bbox),
        };
        if !bounds.is_none_or(Rect::is_finite) {
            return Err(not_finite());
        }
        let shading = ShadingDrawn {
            shading: written.as_reference(),
            bounds,
        };
        self.emit(at, &local.state, &local.marks, Kind::Shading(shading))
    }

    fn begin_marked(
        &mut self,
        frame: &Frame<'_, 'o>,
        local: &mut Local,
        at: &At<'_>,
        step: &Operation<'_>,
        op: Operator,
    ) -> Result<(), Stop> {
        let tag = |i: usize| string_or_name(step.arg(i), &self.lim.content).unwrap_or_default();
        let mark = match op {
            Operator::BeginMarkedProps => {
                let parsed = step.arg(0).and_then(|list| self.props(frame, list));
                self.check_reader()?;
                match parsed {
                    Some((MarkProps::Named(missing_name, None), _)) => {
                        self.tell_at(
                            frame,
                            step.at().start,
                            Fault::MissingResource {
                                category: "Properties",
                                name: missing_name,
                            },
                        );
                        return Ok(());
                    }
                    None => return Ok(()),
                    Some((props, named)) => {
                        Mark::new(tag(1), Some(props), named, frame.stream, at.range())
                    }
                }
            }
            _ => Mark::new(tag(0), None, None, frame.stream, at.range()),
        };
        let total_open = local.marks.iter().count();
        if total_open >= self.lim.marked {
            return Err(past(Cap::Marked));
        }
        self.emit(at, &local.state, &local.marks, Kind::MarkedBegin(&mark))?;
        local.marks.push(mark);
        local.own += 1;
        self.summary.deepest_marked = self.summary.deepest_marked.max(local.own);
        Ok(())
    }

    fn end_marked(
        &mut self,
        frame: &Frame<'_, 'o>,
        local: &mut Local,
        at: &At<'_>,
        offset: usize,
    ) -> Result<(), Stop> {
        if local.own == 0 {
            self.tell_at(frame, offset, Fault::ExtraEndMarked);
            return Ok(());
        }
        local.own -= 1;
        let Some(closed) = local.marks.pop() else {
            return Ok(());
        };
        self.emit(
            at,
            &local.state,
            &local.marks,
            Kind::MarkedEnd(closed.mark()),
        )
    }

    fn point(
        &mut self,
        frame: &Frame<'_, 'o>,
        local: &Local,
        at: &At<'_>,
        step: &Operation<'_>,
        op: Operator,
    ) -> Result<(), Stop> {
        let tag = |i: usize| string_or_name(step.arg(i), &self.lim.content).unwrap_or_default();
        let mark = match op {
            Operator::MarkedPointProps => {
                let parsed = step.arg(0).and_then(|list| self.props(frame, list));
                self.check_reader()?;
                match parsed {
                    Some((props, named)) => {
                        if let MarkProps::Named(ref name, None) = props {
                            self.tell_at(
                                frame,
                                step.at().start,
                                Fault::MissingResource {
                                    category: "Properties",
                                    name: name.clone(),
                                },
                            );
                        }
                        Mark::new(tag(1), Some(props), named, frame.stream, at.range())
                    }
                    None => Mark::new(tag(1), None, None, frame.stream, at.range()),
                }
            }
            _ => Mark::new(tag(0), None, None, frame.stream, at.range()),
        };
        self.emit(at, &local.state, &local.marks, Kind::MarkedPoint(&mark))
    }

    fn props(
        &self,
        frame: &Frame<'_, 'o>,
        operand: Operand<'_>,
    ) -> Option<(MarkProps, Option<&'o Object>)> {
        let Some(name) = operand.name() else {
            let obj = operand.object(&self.lim.content)?;
            let Value::Dict(dict) = obj.into_value() else {
                return None;
            };
            return Some((MarkProps::Inline(dict), None));
        };
        let written = self.entry(frame, b"Properties", &name);
        let list = written.and_then(|w| self.reader.plain_dict_of(w));
        Some((MarkProps::Named(name.into_owned(), list.cloned()), written))
    }

    fn holder(&self, frame: &Frame<'_, 'o>, key: &[u8]) -> Option<(ObjectId, &'o Object)> {
        let has = |r: &&'o Object| self.category_dict(r, key).is_some();
        match frame.resources.filter(has) {
            Some(own) => Some((frame.owner, own)),
            None if frame.owner != PAGE_OWNER => {
                self.page.filter(has).map(|page| (PAGE_OWNER, page))
            }
            None => None,
        }
    }

    fn category_dict(&self, resources: &'o Object, key: &[u8]) -> Option<&'o Dict> {
        let res_dict = self.reader.dict_of_value(resources)?;
        self.reader.plain_dict_of(res_dict.get(key)?)
    }

    fn entry(&self, frame: &Frame<'_, 'o>, key: &[u8], name: &[u8]) -> Option<&'o Object> {
        let (_, resources) = self.holder(frame, key)?;
        self.category_dict(resources, key)?.get(name)
    }
}

impl<'c, O, S: Sink<Fault> + ?Sized> ObjReader<'c, O, S> {
    pub(crate) fn wants_faults(&self) -> bool {
        self.sink_wants()
    }

    pub(crate) fn tell_at(&self, object: Option<ObjectId>, byte: usize, fault: Fault) {
        let span = object.and_then(|id| Span::try_new(Some(id), byte, byte));
        self.emit_place(Place::new(object, span), fault);
    }
}

fn hand(
    visit: &mut dyn FnMut(Event<'_>) -> ControlFlow<()>,
    at: &At<'_>,
    state: &State,
    marks: &Marks,
    kind: Kind<'_>,
) -> Result<(), Stop> {
    let event = Event {
        at: at.clone(),
        state,
        marks,
        kind,
    };
    match visit(event) {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(()) => Err(Stop::Halt(Halt::Break)),
    }
}

fn end_text(local: &mut Local) {
    let glyphs = std::mem::take(&mut local.clip_glyphs);
    let texts = std::mem::take(&mut local.clip_texts);
    if texts > 0 && local.state.text.mode.clips() {
        local.state.clip = local.state.clip.with_text(glyphs, texts);
    }
}

/// Decodes a `/Name`, string, or boolean operand
/// (`CPDF_StreamContentParser::GetString`).
fn string_or_name(operand: Option<Operand<'_>>, lim: &ContentLimits) -> Option<Vec<u8>> {
    let op = operand?;
    if let Some(name) = op.name() {
        return Some(name.into_owned());
    }
    if matches!(op.bytes(), b"true" | b"false") {
        return Some(op.bytes().to_vec());
    }
    string_only(Some(op), lim)
}

/// Decodes a literal or hex string operand: a `TJ` array's element, which
/// PDFium shows only where it's a string (`Handle_ShowText_Positioning`).
fn string_only(operand: Option<Operand<'_>>, lim: &ContentLimits) -> Option<Vec<u8>> {
    let op = operand.filter(Operand::is_string)?;
    let obj = op.object(lim)?;
    obj.as_bytes().map(<[u8]>::to_vec)
}
