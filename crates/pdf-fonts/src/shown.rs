//! The strings content shows, in the order it shows them, each in the font it's
//! shown in, the forms it draws followed: what a page's text is, before any
//! layout.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use pdf_content_streams::{Concat, Join, Operand, Operator, Pushed, ResCx};
use pdf_objects::object::Stream;
use pdf_objects::{Object, ObjectId, Resolve, Streams, Why};

use crate::fonts::{FontId, FontScope, Fonts};
use crate::phases::phase;
use crate::reader::Reader;
use crate::{Error, Font, Limits};

/// Whose resources the content given is drawn with, as a [`FontScope`] keys them:
/// object 0, which no object is (ISO 32000-2, 7.5.4), so a form's never are.
const GIVEN: ObjectId = ObjectId::new(0, 0);

/// The strings `content` shows, in the order it shows them, each in the font it's
/// shown in, decoded through it, and the forms it draws followed, as PDFium
/// (chromium/8125) reads them for its text: what a page's text is, with no layout.
/// `content` is a page's content, its streams joined (pdf-content-streams'
/// [`Concat`]), drawn with `resources`, its `/Resources` as written, and `res` what
/// lexing it needs of them ([`ResCx::read`]). Each font is picked from `fonts`, read
/// once and kept for every page that draws in it.
///
/// - `Tf` picks a font by its name in the resources' `/Font`; one the resources
///   haven't, as a dictionary, is PDFium's stock Helvetica ([`Fonts::stock`]); one
///   PDFium doesn't load ([`Font::loaded`]) leaves the font as it was.
/// - `q` saves the font and `Q` restores it; a `Q` with nothing saved is passed over.
///   `BT` and `ET` change nothing here.
/// - `Tj`, `'` and `"` show a string, a name or `true` or `false` as written, as
///   PDFium reads their operand; `TJ` shows each string of its array, its numbers
///   passed over. An empty string shows nothing, nor does a string shown before any
///   font is picked: PDFium draws neither.
/// - `Do` of a form (`/Subtype /Form`) shows what the form's content shows, in its
///   own state: it starts in the font of what draws it, its `q` and `Q` its own, and
///   what it picks isn't kept after it. Its resources are its own `/Resources`, else
///   those of what draws it; a category its own lack, such as `/Font`, is looked up
///   in `resources`, the page's. A form is followed each time it's drawn, a form
///   within itself too, to [`Limits::forms`] deep: PDFium draws none deeper.
///
/// Each form's content is decoded once a call, and is charged to `budget` each time
/// it's drawn, as reading it is work each time; so is each font's streams, once a
/// call ([`FontScope::font`]). Forms are drawn [`Limits::form_uses`] times a call at
/// most, as forms that each draw another twice are drawn 2 to the 40 times, each
/// little work, but not so many. A Type 3 font's glyphs, which are content, aren't
/// walked: a string in one is its codes' text, as for any font. Nor is anything an
/// annotation draws: only `content` and the forms it draws. Nor is any position,
/// size or order on the page: strings are given as the content shows them, so where
/// PDFium's text orders them by where they are, or leaves out what it can't see, it
/// reads otherwise.
///
/// What isn't read is said ([`Shown::unread`]), never guessed: a form drawn too deep,
/// or past how many may be drawn; a form or a font past what's left of `budget`, or that doesn't read for a reason
/// that's the caller's ([`Error`]), whose strings then aren't shown; and content that
/// doesn't lex past a point, a cap of `limits.content` among it. A form's or font's
/// damage is the file's, and it's read as PDFium reads it: a form whose stream
/// doesn't decode shows nothing, a font that doesn't load leaves the one before.
/// Nothing in it panics on what a file holds.
///
/// ```
/// use std::collections::BTreeMap;
///
/// use pdf_fonts::pdf_content_streams::{Concat, ResCx};
/// use pdf_fonts::pdf_objects::{Object, ObjectId, Stream, Unreadable, WithStreams, Why, dict};
/// use pdf_fonts::{Fonts, Limits, shown};
///
/// // A form, object 1, that shows a string in the font of what draws it.
/// let form = Stream::new(dict! { "Subtype" => "Form" }, b"(Hi) Tj".to_vec());
/// let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(form))]);
/// let resolve = |id: ObjectId| objects.get(&id);
/// let file = WithStreams::new(&resolve, |id, _| {
///     let data = objects.get(&id).and_then(Object::as_stream);
///     let data = data.and_then(|stream| stream.data.bytes()).map(<[u8]>::to_vec);
///     data.ok_or_else(|| Unreadable::new(id, Why::Damaged, "no such stream"))
/// });
///
/// // A page that picks Helvetica, shows a string and draws the form.
/// let helvetica = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" };
/// let resources = Object::from(dict! {
///     "Font" => dict! { "F1" => helvetica },
///     "XObject" => dict! { "X" => Object::reference(1) },
/// });
/// let content = Concat::new(&[b"BT /F1 12 Tf [(Hel) -20 (lo)] TJ ET /X Do"]);
/// let mut budget = 1 << 20;
/// let read = shown(
///     file, &Fonts::default(), Some(&resources), &content, &ResCx::none(),
///     &Limits::DEFAULT, &mut budget,
/// );
/// assert!(read.unread.is_empty());
/// assert_eq!(read.text(), "HelloHi");
/// let forms: Vec<_> = read.strings.iter().map(|s| s.form).collect();
/// assert_eq!(forms, [None, None, Some(ObjectId::new(1, 0))]);
/// ```
#[must_use]
pub fn shown<'o, O>(
    objects: O,
    fonts: &Fonts,
    resources: Option<&'o Object>,
    content: &Concat<'_>,
    res: &ResCx,
    limits: &Limits,
    budget: &mut usize,
) -> Shown
where
    O: Resolve<'o> + Streams + Copy,
{
    phase!(DEBUG, "shown", bytes = content.bytes().len());
    let page = Frame {
        form: None,
        owner: GIVEN,
        resources,
        res: ResOf::Given,
        script: Rc::new(steps_of(content, res, &limits.content)),
        next: 0,
        current: Current::None,
        saved: Vec::new(),
    };
    let mut walk = Walk {
        objects,
        scope: fonts.scope(),
        limits,
        budget,
        given: (resources, res),
        forms: HashMap::new(),
        scripts: HashMap::new(),
        spaces: Vec::new(),
        uses: 0,
        shown: Shown::default(),
    };
    let mut frames = vec![page];
    loop {
        let depth = frames.len();
        let Some(frame) = frames.last_mut() else {
            break;
        };
        let script = Rc::clone(&frame.script);
        let Some(step) = script.get(frame.next) else {
            frames.pop();
            continue;
        };
        frame.next += 1;
        let drawn = match step {
            Step::Save => {
                frame.saved.push(frame.current);
                None
            }
            Step::Restore => {
                if let Some(saved) = frame.saved.pop() {
                    frame.current = saved;
                }
                None
            }
            Step::Font(name) => {
                frame.current = walk.pick(frame, name);
                None
            }
            Step::Show { bytes, at } => {
                if let Current::Font(font) = frame.current {
                    walk.shown.strings.push(ShownString {
                        bytes: bytes.clone(),
                        font: Arc::clone(walk.scope.shared(font)),
                        form: frame.form,
                        at: at.clone(),
                    });
                }
                None
            }
            Step::Draw(name) => walk.draw(frame, name, depth),
            Step::Stop(error) => {
                let (form, error) = (frame.form, *error);
                walk.shown.unread.push(Unread::Content { form, error });
                None
            }
        };
        frames.extend(drawn);
    }
    walk.shown
}

/// What content shows ([`shown`]): its strings, in the order it shows them, and what
/// wasn't read. Its `Debug` gives how many strings there are and what wasn't read,
/// never a string's bytes or text.
#[derive(Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Shown {
    /// Each string shown, in the order the content and the forms it draws show them.
    pub strings: Vec<ShownString>,
    /// What wasn't read, in the order it was met. Empty where every string the
    /// content shows is in [`strings`](Shown::strings).
    pub unread: Vec<Unread>,
}

impl Shown {
    /// The strings' text, joined, each as [`ShownString::text`] reads it, with
    /// nothing between them. A string whose font doesn't say the text of every code is
    /// a U+FFFD for each of its codes ([`Font::codes`]), as what it is isn't known.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::new();
        for string in &self.strings {
            match string.text() {
                Some(read) => text.push_str(&read),
                None => {
                    let codes = string.font.codes(&string.bytes).count();
                    text.extend(std::iter::repeat_n(char::REPLACEMENT_CHARACTER, codes));
                }
            }
        }
        text
    }

    /// Whether every string the content shows was read: nothing is
    /// [`unread`](Shown::unread).
    #[must_use]
    pub fn is_whole(&self) -> bool {
        self.unread.is_empty()
    }
}

impl fmt::Debug for Shown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shown")
            .field("strings", &self.strings.len())
            .field("unread", &self.unread)
            .finish()
    }
}

/// A string content shows ([`shown`]): its bytes, the font it's shown in, and where
/// it's written. Its `Debug` gives its length, its font's object and where it is,
/// never its bytes or its text.
#[derive(Clone)]
#[non_exhaustive]
pub struct ShownString {
    /// Its bytes, as the operator shows them: its codes in [`font`](ShownString::font).
    pub bytes: Vec<u8>,
    /// The font it's shown in: the one `Tf` picked, or PDFium's stock Helvetica
    /// ([`Fonts::stock`]) where the resources have no font by its name.
    pub font: Arc<Font>,
    /// The form whose content shows it, the innermost; none for the content given.
    pub form: Option<ObjectId>,
    /// Where it's written in that content: the operand shown, or, in a `TJ`'s
    /// array, the string's place in it.
    pub at: Range<usize>,
}

impl ShownString {
    /// Its text, through its font ([`Font::text`]): none where the font doesn't say
    /// the text of one of its codes.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        self.font.text(&self.bytes)
    }
}

impl PartialEq for ShownString {
    /// The same bytes, at the same place in the same form, in the same font, by
    /// the [`Arc`]: one font read once.
    fn eq(&self, other: &ShownString) -> bool {
        self.bytes == other.bytes
            && Arc::ptr_eq(&self.font, &other.font)
            && self.form == other.form
            && self.at == other.at
    }
}

impl fmt::Debug for ShownString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShownString")
            .field("bytes", &self.bytes.len())
            .field("font", &self.font.id())
            .field("form", &self.form)
            .field("at", &self.at)
            .finish()
    }
}

/// What [`shown`] didn't read, and why. Its `Debug` gives a name by its length, never
/// its bytes.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unread {
    /// Form `form`, drawn deeper in forms than [`Limits::forms`]: not followed, so
    /// what it shows isn't read.
    Deep {
        /// The form.
        form: ObjectId,
    },
    /// Form `form`, drawn after [`Limits::form_uses`] forms were drawn in all: not
    /// followed, so what it shows isn't read.
    Many {
        /// The form.
        form: ObjectId,
    },
    /// The form `Do` draws by `name`, which isn't drawn: what its content decodes to
    /// is past what's left of the budget ([`Error::TooLarge`]), or it, its resources,
    /// or what's on the way to it doesn't read for a reason that's the caller's
    /// ([`Error::Unreadable`]).
    Form {
        /// Its name in the resources' `/XObject`.
        name: Vec<u8>,
        /// Its object, where that was found.
        form: Option<ObjectId>,
        /// Why it isn't drawn.
        error: Error,
    },
    /// The font `Tf` picks by `name`, past what's left of the budget or not read for
    /// a reason that's the caller's: what's shown in it isn't read, until a `Tf`
    /// picks another or `Q` restores one.
    Font {
        /// Its name in the resources' `/Font`.
        name: Vec<u8>,
        /// Why it isn't read.
        error: Error,
    },
    /// Content read as far as `error` says, past a cap of
    /// [`Limits::content`](crate::Limits::content) or where it can't be lexed: what
    /// it shows after that isn't read.
    Content {
        /// The form it's of; none for the content given.
        form: Option<ObjectId>,
        /// Where and why its reading stopped (pdf-content-streams').
        error: pdf_content_streams::Error,
    },
}

impl fmt::Debug for Unread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unread::Deep { form } => f.debug_struct("Deep").field("form", form).finish(),
            Unread::Many { form } => f.debug_struct("Many").field("form", form).finish(),
            Unread::Form { name, form, error } => f
                .debug_struct("Form")
                .field("name", &name.len())
                .field("form", form)
                .field("error", error)
                .finish(),
            Unread::Font { name, error } => f
                .debug_struct("Font")
                .field("name", &name.len())
                .field("error", error)
                .finish(),
            Unread::Content { form, error } => f
                .debug_struct("Content")
                .field("form", form)
                .field("error", error)
                .finish(),
        }
    }
}

/// What content does that the walk reads, in order: its operations lexed once, so a
/// form drawn many times isn't lexed again.
#[derive(Debug)]
enum Step {
    /// `q`.
    Save,
    /// `Q`.
    Restore,
    /// `Tf`, by the name it picks.
    Font(Vec<u8>),
    /// A string shown, not empty, and where it's written.
    Show { bytes: Vec<u8>, at: Range<usize> },
    /// `Do`, by the name it draws.
    Draw(Vec<u8>),
    /// The content lexed no further.
    Stop(pdf_content_streams::Error),
}

/// `content`'s steps, lexed under `limits` with `res`.
fn steps_of(content: &Concat<'_>, res: &ResCx, limits: &pdf_content_streams::Limits) -> Vec<Step> {
    let mut steps = Vec::new();
    for operation in content.operations(res, limits) {
        let operation = match operation {
            Ok(operation) => operation,
            Err(error) => {
                steps.push(Step::Stop(error));
                break;
            }
        };
        match operation.operator() {
            Operator::Save => steps.push(Step::Save),
            Operator::Restore => steps.push(Step::Restore),
            Operator::Font => {
                let name = operation.arg(1).and_then(|o| string(o, limits));
                steps.push(Step::Font(name.unwrap_or_default()));
            }
            Operator::Show | Operator::NextLineShow | Operator::NextLineShowSpaced => {
                if let Some(operand) = operation.arg(0)
                    && let Some(bytes) = string(operand, limits)
                    && !bytes.is_empty()
                {
                    let at = operand.range();
                    steps.push(Step::Show { bytes, at });
                }
            }
            Operator::ShowArray => {
                let array = operation.arg(0).filter(Operand::is_array);
                for element in array.iter().flat_map(Operand::elements) {
                    if !element.is_string() {
                        continue;
                    }
                    let bytes = element.object(limits);
                    if let Some(bytes) = bytes.as_ref().and_then(Object::as_bytes)
                        && !bytes.is_empty()
                    {
                        let at = element.range();
                        let bytes = bytes.to_vec();
                        steps.push(Step::Show { bytes, at });
                    }
                }
            }
            Operator::Draw => {
                let name = operation.arg(0).and_then(|o| string(o, limits));
                steps.push(Step::Draw(name.unwrap_or_default()));
            }
            _ => {}
        }
    }
    steps
}

/// What PDFium's content parser reads of `operand` as a string (`GetString`): a
/// string's bytes, a name's, its `#` escapes decoded, and `true` or `false` as
/// written; none of anything else, a number, an array or a dictionary.
fn string(operand: Operand<'_>, limits: &pdf_content_streams::Limits) -> Option<Vec<u8>> {
    if operand.is_name() {
        return operand.name().map(|name| name.into_owned());
    }
    if operand.is_string() {
        return operand.object(limits)?.as_bytes().map(<[u8]>::to_vec);
    }
    matches!(operand.bytes(), b"true" | b"false").then(|| operand.bytes().to_vec())
}

/// The font strings are shown in.
#[derive(Debug, Clone, Copy)]
enum Current {
    /// None picked yet: PDFium shows nothing.
    None,
    Font(FontId),
    /// One that didn't read for the caller's reason, told when it was picked: what's
    /// shown in it isn't read.
    Unread,
}

/// What lexing a form's content needs of the resources it's drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ResOf {
    /// The content given's, `res`.
    Given,
    /// A form's own, at its place among those read.
    Own(usize),
}

/// Content being walked: the content given's, or a form's each time it's drawn.
struct Frame<'o> {
    /// The form it's of; none for the content given.
    form: Option<ObjectId>,
    /// Whose resources it's drawn with: the content given's ([`GIVEN`]), or a form's
    /// own.
    owner: ObjectId,
    resources: Option<&'o Object>,
    res: ResOf,
    script: Rc<Vec<Step>>,
    /// The next step.
    next: usize,
    current: Current,
    /// What `q` saved.
    saved: Vec<Current>,
}

/// Why a form isn't drawn, and its object where it was found.
type Missed = (Option<ObjectId>, Error);

/// A form's content, decoded once a call, and the resources it has of its own.
struct FormRead<'o> {
    content: Concat<'static>,
    /// Its own `/Resources`, where it has a dictionary there, and what lexing needs of
    /// them: its own color spaces, or the content given's where they have none.
    own: Option<(&'o Object, ResOf)>,
}

/// One call of [`shown`].
struct Walk<'w, 'o, O> {
    objects: O,
    scope: FontScope<'w>,
    limits: &'w Limits,
    budget: &'w mut usize,
    /// The content given's resources, and what lexing needs of them.
    given: (Option<&'o Object>, &'w ResCx),
    forms: HashMap<ObjectId, Result<Rc<FormRead<'o>>, Error>>,
    /// Each form's steps, by what lexing them needed.
    scripts: HashMap<(ObjectId, ResOf), Rc<Vec<Step>>>,
    /// The color spaces of the forms' own resources that have some.
    spaces: Vec<ResCx>,
    /// How many times a form was drawn.
    uses: usize,
    shown: Shown,
}

impl<'o, O: Resolve<'o> + Streams + Copy> Walk<'_, 'o, O> {
    /// Where `frame` looks up a resource of `category`, as PDFium's
    /// `FindResourceHolder`: in its resources, but in a form whose own resources
    /// have no such dictionary, the content given's. Or why that doesn't read.
    fn holder(
        &self,
        frame: &Frame<'o>,
        category: &[u8],
    ) -> Result<(ObjectId, Option<&'o Object>), Error> {
        if frame.owner == GIVEN {
            return Ok((GIVEN, frame.resources));
        }
        let mut none = ();
        let reader = Reader::new(self.objects, self.limits, &mut none);
        let has = (frame.resources)
            .and_then(|resources| reader.dict_of_value(resources))
            .and_then(|resources| reader.dict(resources, category))
            .is_some();
        if let Some(unreadable) = reader.failure() {
            return Err(unreadable.into());
        }
        Ok(match has {
            true => (frame.owner, frame.resources),
            false => (GIVEN, self.given.0),
        })
    }

    /// The font `Tf` picks by `name` in `frame`: what strings are then shown in.
    fn pick(&mut self, frame: &Frame<'o>, name: &[u8]) -> Current {
        let picked = self.holder(frame, b"Font").and_then(|(owner, resources)| {
            let limits = self.limits;
            (self.scope).font(self.objects, owner, resources, name, limits, self.budget)
        });
        match picked {
            Ok(Some(font)) if self.scope.get(font).loaded() => Current::Font(font),
            Ok(Some(_)) => frame.current,
            Ok(None) => Current::Font(self.scope.stock()),
            Err(error) => {
                let name = name.to_vec();
                self.shown.unread.push(Unread::Font { name, error });
                Current::Unread
            }
        }
    }

    /// The form `Do` draws by `name` in `caller`, `depth` forms deep counting the
    /// content given, as a frame to walk: none where it draws no form, or none that's
    /// read, which is told.
    fn draw(&mut self, caller: &Frame<'o>, name: &[u8], depth: usize) -> Option<Frame<'o>> {
        let found = self
            .holder(caller, b"XObject")
            .map_err(|error| (None, error))
            .and_then(|(_, resources)| self.form(resources, name));
        let (id, stream) = match found {
            Ok(found) => found?,
            Err((form, error)) => {
                let name = name.to_vec();
                self.shown.unread.push(Unread::Form { name, form, error });
                return None;
            }
        };
        if depth > self.limits.forms {
            self.shown.unread.push(Unread::Deep { form: id });
            return None;
        }
        if self.uses >= self.limits.form_uses {
            self.shown.unread.push(Unread::Many { form: id });
            return None;
        }
        let read = match self.read(id, stream) {
            Ok(read) => read,
            Err(error) => {
                let (name, form) = (name.to_vec(), Some(id));
                self.shown.unread.push(Unread::Form { name, form, error });
                return None;
            }
        };
        let len = read.content.bytes().len();
        let Some(left) = self.budget.checked_sub(len) else {
            let (name, form, error) = (name.to_vec(), Some(id), Error::TooLarge);
            self.shown.unread.push(Unread::Form { name, form, error });
            return None;
        };
        *self.budget = left;
        self.uses += 1;
        let (owner, resources, res) = match read.own {
            Some((own, res)) => (id, Some(own), res),
            None => (caller.owner, caller.resources, caller.res),
        };
        let script = match self.scripts.get(&(id, res)) {
            Some(script) => Rc::clone(script),
            None => {
                let res_cx = match res {
                    ResOf::Given => self.given.1,
                    ResOf::Own(i) => &self.spaces[i],
                };
                let script = Rc::new(steps_of(&read.content, res_cx, &self.limits.content));
                self.scripts.insert((id, res), Rc::clone(&script));
                script
            }
        };
        Some(Frame {
            form: Some(id),
            owner,
            resources,
            res,
            script,
            next: 0,
            current: caller.current,
            saved: Vec::new(),
        })
    }

    /// The form `resources`' `/XObject` names `name`, as PDFium's
    /// `Handle_ExecuteXObject` finds it: a stream object whose `/Subtype` is `Form`.
    /// None for anything else, an image or a stream written in place, which has no
    /// number to ask its data by; or why it doesn't read, and its object where that
    /// was found.
    fn form(
        &self,
        resources: Option<&'o Object>,
        name: &[u8],
    ) -> Result<Option<(ObjectId, &'o Stream)>, Missed> {
        let mut none = ();
        let reader = Reader::new(self.objects, self.limits, &mut none);
        let id = resources
            .and_then(|resources| reader.dict_of_value(resources))
            .and_then(|resources| reader.dict(resources, b"XObject"))
            .and_then(|xobjects| xobjects.get(name))
            .and_then(Object::as_reference);
        let form = id
            .and_then(|id| reader.stream(id))
            .filter(|(_, stream)| reader.byte_string(&stream.dict, b"Subtype") == Some(b"Form"));
        match reader.failure() {
            Some(unreadable) => Err((id, unreadable.into())),
            None => Ok(form),
        }
    }

    /// Form `id`'s content and its own resources, read once a call: its content
    /// decoded within what's left of the budget, as a form whose stream doesn't decode
    /// is empty, and its own color spaces counted.
    fn read(&mut self, id: ObjectId, stream: &'o Stream) -> Result<Rc<FormRead<'o>>, Error> {
        if let Some(read) = self.forms.get(&id) {
            return read.clone();
        }
        let read = self.read_anew(id, stream).map(Rc::new);
        self.forms.insert(id, read.clone());
        read
    }

    fn read_anew(&mut self, id: ObjectId, stream: &'o Stream) -> Result<FormRead<'o>, Error> {
        let mut none = ();
        let reader = Reader::new(self.objects, self.limits, &mut none);
        let own = stream
            .dict
            .get(b"Resources")
            .filter(|own| reader.dict_of_value(own).is_some());
        let spaces = own.is_some_and(|own| {
            let own = reader.dict_of_value(own);
            own.and_then(|own| reader.dict(own, b"ColorSpace"))
                .is_some()
        });
        if let Some(unreadable) = reader.failure() {
            return Err(unreadable.into());
        }
        let mut limits = self.limits.content;
        limits.content = *self.budget;
        let mut join = Join::new(&limits);
        match join.push_from(&self.objects, id) {
            Ok(()) => {}
            Err(Pushed::Decode(unreadable))
                if matches!(unreadable.why, Why::Damaged | Why::Filter) => {}
            Err(Pushed::Decode(unreadable)) => return Err(unreadable.into()),
            // Past the room, which is what's left of the budget (`TooLarge`); and any
            // other refusal, which leaves the form unread as that does.
            Err(_) => return Err(Error::TooLarge),
        }
        let own = match (own, spaces) {
            (Some(own), true) => {
                let res = ResCx::read(self.objects, Some(own), &self.limits.content)?;
                self.spaces.push(res);
                Some((own, ResOf::Own(self.spaces.len() - 1)))
            }
            (Some(own), false) => Some((own, ResOf::Given)),
            (None, _) => None,
        };
        Ok(FormRead {
            content: join.finish(),
            own,
        })
    }
}
