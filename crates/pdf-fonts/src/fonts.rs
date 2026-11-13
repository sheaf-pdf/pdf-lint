//! A source's fonts, each read once.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use pdf_objects::object::Value;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable, Why, dict};

use crate::font::Font;
use crate::reader::{Reader, dict_of};
use crate::{Error, Limits};

/// A source's fonts, each read once: those that are objects by number, kept for
/// every page that draws in them. Nothing is told to a
/// [`Sink`](pdf_objects::fault::Sink) here: a font kept would be told to no one
/// twice, so who wants faults calls [`Font::read`]. A font that doesn't read isn't
/// kept: asked again, it's read again, and a source that reads it then gives it. A
/// [`FontScope`] keeps it for its picks.
///
/// [`Fonts::default`] reads each font's program with it, for its glyphs' boxes, its
/// heights and what its encoding leaves to it of its text, charged to the budget it's
/// read within. Fonts made for text ([`Fonts::for_text`]) read none.
///
/// It's meant to be used under one [`Limits`]: a font it holds is given as it was first
/// read, whatever limits are asked for later, and its CID glyphs are drawn under those.
#[derive(Debug, Default)]
pub struct Fonts {
    read: Mutex<HashMap<ObjectId, Arc<Font>>>,
    stock: OnceLock<Arc<Font>>,
    /// Whether it reads no font's program.
    for_text: bool,
}

/// Nothing: what a font written in code is read over.
#[derive(Clone, Copy)]
struct NoObjects;

impl<'o> Resolve<'o> for NoObjects {
    fn resolve(&self, _: ObjectId) -> Option<&'o Object> {
        None
    }
}

impl Streams for NoObjects {
    fn decoded(&self, id: ObjectId, _: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        Err(Unreadable::new(
            id,
            Why::Damaged,
            format!("{id} R isn't there"),
        ))
    }
}

impl Fonts {
    /// Fonts read for the text their strings are and where their glyphs are placed,
    /// and no more: no program is read, nor charged to a budget, so each glyph's box
    /// is its metrics' alone, and heights a program would give are guessed
    /// ([`FontApprox`](crate::FontApprox)). So is the text of a code PDFium's encoding
    /// leaves to a program, by the font's name, as a symbolic font with no `/ToUnicode`
    /// for that code and no encoding of a name comes to.
    #[must_use]
    pub fn for_text() -> Fonts {
        Fonts {
            for_text: true,
            ..Fonts::default()
        }
    }

    /// The font `font`, a font resource as written, is: read once where it's an
    /// object, kept; read anew where it's a dictionary. None for anything else. What
    /// its streams decompress to is charged to `budget` whether it's read now or was
    /// before ([`Font::cost`]), so what it costs is the same whichever page read it
    /// first.
    ///
    /// # Errors
    ///
    /// [`Error::TooLarge`] where what it costs is past what's left of `budget`, and
    /// [`Error::Unreadable`] where `font`'s object, or what it reads, doesn't read for a
    /// reason that's the caller's, as [`Font::read`] says. `budget` is then as it was.
    pub fn get<'o, O>(
        &self,
        objects: O,
        font: &'o Object,
        limits: &Limits,
        budget: &mut usize,
    ) -> Result<Option<Arc<Font>>, Error>
    where
        O: Resolve<'o> + Streams + Copy,
    {
        let Value::Ref(id) = *font.value() else {
            let Some(dict) = dict_of(font) else {
                return Ok(None);
            };
            return Ok(Some(Arc::new(self.read(objects, dict, limits, budget)?)));
        };
        if let Some(known) = self.lock().get(&id) {
            *budget = budget.checked_sub(known.cost()).ok_or(Error::TooLarge)?;
            return Ok(Some(known.clone()));
        }
        let mut none = ();
        let reader = Reader::new(objects, limits, &mut none);
        let dict = reader.hop(font).and_then(dict_of);
        if let Some(unreadable) = reader.failure() {
            return Err(unreadable.into());
        }
        let Some(dict) = dict else {
            return Ok(None);
        };
        let mut read = self.read(objects, dict, limits, budget)?;
        read.set_id(id);
        Ok(Some(
            self.lock().entry(id).or_insert(Arc::new(read)).clone(),
        ))
    }

    /// What PDFium draws text in where the resources have no font by the name `Tf`
    /// gives: its stock Helvetica, in WinAnsiEncoding.
    #[must_use]
    pub fn stock(&self) -> Arc<Font> {
        let read = || {
            let helvetica: Dict = dict! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
                "Encoding" => "WinAnsiEncoding",
            };
            let read = Font::read(NoObjects, &helvetica, &Limits::DEFAULT, &mut 0, &mut ());
            Arc::new(read.expect("a font by name alone has no stream to decompress"))
        };
        self.stock.get_or_init(read).clone()
    }

    /// `font` read, its program read but for text.
    fn read<'o, O>(
        &self,
        objects: O,
        font: &'o Dict,
        limits: &Limits,
        budget: &mut usize,
    ) -> Result<Font, Error>
    where
        O: Resolve<'o> + Streams + Copy,
    {
        Font::read_as(objects, font, limits, budget, &mut (), !self.for_text)
    }

    /// The fonts one interpretation of content picks.
    #[must_use]
    pub fn scope(&self) -> FontScope<'_> {
        FontScope {
            fonts: self,
            picked: Vec::new(),
            by_name: HashMap::new(),
            charged: HashMap::new(),
            unread: HashMap::new(),
            stock: None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<ObjectId, Arc<Font>>> {
        self.read.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The fonts one interpretation of content picks (`Tf`), by name in the resources
/// they're picked from: each looked up once however often it's picked, and one
/// written in the resources as a dictionary read once. So is one that doesn't read for
/// a reason that's the caller's, whose `Err` is kept. Each is known by its place among
/// them ([`FontId`]), which a graphics state can keep, so saving the state counts no
/// reference to it.
#[derive(Debug)]
pub struct FontScope<'f> {
    fonts: &'f Fonts,
    /// The fonts picked, each once, by [`FontId`].
    picked: Vec<Arc<Font>>,
    /// By the object whose resources they're in, then by name.
    by_name: HashMap<ObjectId, HashMap<Vec<u8>, Picked>>,
    /// The fonts by object picked, each charged once.
    charged: HashMap<ObjectId, FontId>,
    /// The fonts by object that didn't read for a reason that's the caller's.
    unread: HashMap<ObjectId, Unreadable>,
    /// The stock Helvetica, once it's picked.
    stock: Option<FontId>,
}

/// What a name picked: its font, none, or why it didn't read.
type Picked = Result<Option<FontId>, Unreadable>;

/// A font an interpretation picked: its place among its [`FontScope`]'s, and good only
/// for that scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontId(u32);

impl FontScope<'_> {
    /// Font `name` of `resources`, the `/Resources` as written of `owner`: the page,
    /// form, pattern or Type 3 font whose content picks it. None where they have no
    /// such font. What its streams decompress to is charged to `budget` the first
    /// time it's picked, by any name, and a font written in resources as a dictionary
    /// each time it's read.
    ///
    /// # Errors
    ///
    /// As [`Fonts::get`]'s, and [`Error::Unreadable`] too where `resources`, or its
    /// `/Font`, doesn't read for a reason that's the caller's. `budget` is then as it
    /// was. An [`Error::Unreadable`] is kept: a later pick by that name, or of that
    /// font by another name, gives it again without reading anything.
    /// [`Error::TooLarge`] isn't, as within more of a budget the font may read.
    pub fn font<'o, O>(
        &mut self,
        objects: O,
        owner: ObjectId,
        resources: Option<&'o Object>,
        name: &[u8],
        limits: &Limits,
        budget: &mut usize,
    ) -> Result<Option<FontId>, Error>
    where
        O: Resolve<'o> + Streams + Copy,
    {
        if let Some(picked) = self.by_name.get(&owner).and_then(|n| n.get(name)) {
            return picked.clone().map_err(Error::Unreadable);
        }
        let picked = self.pick(objects, resources, name, limits, budget);
        let kept = match &picked {
            Ok(font) => Ok(*font),
            Err(Error::Unreadable(unreadable)) => Err(unreadable.clone()),
            Err(_) => return picked,
        };
        let by_name = self.by_name.entry(owner).or_default();
        by_name.insert(name.to_vec(), kept);
        picked
    }

    /// [`FontScope::font`], looked up anew by its name.
    fn pick<'o, O>(
        &mut self,
        objects: O,
        resources: Option<&'o Object>,
        name: &[u8],
        limits: &Limits,
        budget: &mut usize,
    ) -> Result<Option<FontId>, Error>
    where
        O: Resolve<'o> + Streams + Copy,
    {
        let mut none = ();
        let reader = Reader::new(objects, limits, &mut none);
        let written = resources
            .and_then(|r| reader.dict_of_value(r))
            .and_then(|r| reader.dict(r, b"Font"))
            .and_then(|f| f.get(name));
        if let Some(unreadable) = reader.failure() {
            return Err(unreadable.into());
        }
        let Some(written) = written else {
            return Ok(None);
        };
        let object = written.as_reference();
        if let Some(id) = object {
            if let Some(&font) = self.charged.get(&id) {
                return Ok(Some(font));
            }
            if let Some(unreadable) = self.unread.get(&id) {
                return Err(Error::Unreadable(unreadable.clone()));
            }
        }
        let read = match self.fonts.get(objects, written, limits, budget) {
            Err(Error::Unreadable(unreadable)) => {
                if let Some(id) = object {
                    self.unread.insert(id, unreadable.clone());
                }
                return Err(Error::Unreadable(unreadable));
            }
            read => read?,
        };
        Ok(read.map(|read| {
            let object = read.id();
            let font = self.add(read);
            if let Some(id) = object {
                self.charged.insert(id, font);
            }
            font
        }))
    }

    /// PDFium's stock Helvetica ([`Fonts::stock`]), picked.
    #[must_use]
    pub fn stock(&mut self) -> FontId {
        match self.stock {
            Some(font) => font,
            None => {
                let font = self.add(self.fonts.stock());
                self.stock = Some(font);
                font
            }
        }
    }

    /// The font `font` is.
    ///
    /// # Panics
    ///
    /// Where `font` isn't one of this scope's: another scope's id may panic, or name
    /// another font.
    #[must_use]
    pub fn get(&self, font: FontId) -> &Font {
        &self.picked[font.0 as usize]
    }

    /// [`FontScope::get`], as it's kept: shared.
    pub(crate) fn shared(&self, font: FontId) -> &Arc<Font> {
        &self.picked[font.0 as usize]
    }

    /// `font` among those picked. There are fewer than operators interpreted, which
    /// are capped far below 2^32.
    fn add(&mut self, font: Arc<Font>) -> FontId {
        self.picked.push(font);
        FontId((self.picked.len() - 1) as u32)
    }
}
