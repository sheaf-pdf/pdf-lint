//! A page's recipes, each a function over the family's traits, so that any
//! reader of a file's objects and streams' data can call it: pdf-structure's
//! reading, its document beside it, or pdf-objects' `WithStreams`. A
//! [`Page`](crate::Page) calls each with its file's defaults.

use std::fmt;
use std::sync::Arc;

use pdf_content_streams::{Concat, Join, Limits as ContentLimits, Operations, ResCx};
use pdf_fonts::{Font, Fonts, Shown};
use pdf_objects::object::Value;
use pdf_objects::{Dict, Object, Resolve, Streams, Unreadable};
use pdf_semantics::pages::{self, Inheritable};

use crate::error::Error;
use crate::limits::Limits;
use crate::phases::phase;

/// A page's content: its streams decoded and joined as PDFium joins them, and
/// what reading it needs of the resources it's drawn with
/// ([`content`](fn@content)). Its `Debug` gives the joined bytes' length, how
/// many streams there are and how many were joined as none, not the bytes,
/// which are content.
#[derive(Clone)]
pub struct Content {
    concat: Concat<'static>,
    res: ResCx,
    limits: ContentLimits,
}

impl Content {
    /// The streams joined, each placed in them (pdf-content-streams'
    /// [`Concat`]).
    #[must_use]
    pub const fn concat(&self) -> &Concat<'static> {
        &self.concat
    }

    /// The streams' bytes, joined, a space between each.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.concat.bytes()
    }

    /// What reading it needs of the page's resources: how many components
    /// each color space it names has, which sizes an inline image drawn in
    /// one.
    #[must_use]
    pub const fn res(&self) -> &ResCx {
        &self.res
    }

    /// Its operations, in order, under the caps it was joined under
    /// ([`Limits::content`](crate::Limits::content)), at offsets into the
    /// joined bytes ([`Concat::operations`]).
    pub fn operations(&self) -> Operations<'_, '_> {
        self.concat.operations(&self.res, &self.limits)
    }

    /// The streams joined, taken.
    #[must_use]
    pub fn into_concat(self) -> Concat<'static> {
        self.concat
    }
}

impl fmt::Debug for Content {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Content")
            .field("bytes", &self.concat.bytes().len())
            .field("streams", &self.concat.stream_count())
            .field("undecoded", &self.concat.undecoded().len())
            .finish_non_exhaustive()
    }
}

/// `page`'s content, as `objects` give its streams' data: its `/Contents`,
/// a stream or an array of them (ISO 32000-2, 7.7.3.3), joined under
/// `limits` as PDFium joins them (pdf-content-streams'
/// [`Join::push_contents`]), and the color spaces its resources name, its
/// own `/Resources` or what it inherits, counted under `limits.color`
/// ([`ResCx::read`]). A place where the tree has no page, or a page with no
/// `/Contents`, has none: no stream. A stream whose data doesn't decode for
/// the file's own reason, damaged or through a filter the reader doesn't
/// take, is joined as one of no data, its place kept, and said
/// ([`Concat::undecoded`]), where PDFium reads its data as written: the one
/// way this joins them otherwise than PDFium.
///
/// Over any reader of a file's objects and streams' data, such as
/// pdf-objects' `WithStreams` of a map and a function:
///
/// ```
/// use std::collections::BTreeMap;
///
/// use pdfs::pdf_content_streams::Limits;
/// use pdfs::pdf_objects::{Object, ObjectId, Unreadable, WithStreams, Why, dict};
/// use pdfs::pdf_semantics::pages;
/// use pdfs::pdf_tokenizer::Rules;
///
/// let objects = BTreeMap::from([
///     (ObjectId::new(2, 0), Object::from(dict! { "Type" => "Pages", "Kids" => vec![Object::reference(3)], "Count" => 1 })),
///     (ObjectId::new(3, 0), Object::from(dict! {
///         "Type" => "Page", "Parent" => Object::reference(2),
///         "Contents" => vec![Object::reference(4), Object::reference(5)],
///     })),
///     (ObjectId::new(4, 0), Object::stream(dict! {})),
///     (ObjectId::new(5, 0), Object::stream(dict! {})),
/// ]);
/// let file = WithStreams::new(|id: ObjectId| objects.get(&id), |id: ObjectId, _: usize| match id.number {
///     4 => Ok(b"0 0 m".to_vec()),
///     5 => Ok(b"9 9 l S".to_vec()),
///     _ => Err(Unreadable::new(id, Why::Damaged, "no stream")),
/// });
/// let catalog = dict! { "Pages" => Object::reference(2) };
/// let semantics = pdfs::pdf_semantics::Limits::DEFAULT;
/// let table = pages::pages(file, &catalog, Rules::Pdfium, &semantics, &mut ()).unwrap();
/// let content = pdfs::page::content(file, table.get(0).unwrap(), &Limits::DEFAULT).unwrap();
/// assert_eq!(content.bytes(), b"0 0 m 9 9 l S");
/// ```
///
/// # Errors
///
/// [`Error::Content`] where its streams joined are past `limits.content`, or
/// a stream's data, or `/Contents` or an element of it, doesn't read for the
/// reader's own reason, past a cap, behind a password or not given by the
/// source ([`Why::Capped`](pdf_objects::Why::Capped),
/// [`Why::Locked`](pdf_objects::Why::Locked),
/// [`Why::Source`](pdf_objects::Why::Source), as the reader says it), its
/// `Why` kept (pdf-content-streams' [`Pushed`](pdf_content_streams::Pushed));
/// never for the file's own damage. [`Error::Unreadable`] where an object of
/// its resources doesn't read for the reader's own reason.
pub fn content<'o, O>(
    objects: O,
    page: &pages::Page<'o>,
    limits: &ContentLimits,
) -> Result<Content, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    phase!(DEBUG, "content");
    let mut join = Join::new(limits);
    if let Some(contents) = page.dict.and_then(|dict| dict.get(b"Contents")) {
        join.push_contents(objects, contents)?;
    }
    let resources = page.attribute(Inheritable::Resources);
    let res = ResCx::read(
        objects,
        resources.map(|resources| resources.written),
        limits,
    )?;
    Ok(Content {
        concat: join.finish(),
        res,
        limits: *limits,
    })
}

/// A font a page's resources name ([`fonts`](fn@fonts)): its name in their
/// `/Font`, and the font as pdf-fonts reads it, none where what's there is no
/// font dictionary, or why it wasn't read. Its `Debug` gives the name's length,
/// not the name, and the font's object.
#[derive(Clone)]
#[non_exhaustive]
pub struct NamedFont {
    /// Its name in the resources' `/Font`.
    pub name: Vec<u8>,
    /// The font, read once a file and kept ([`Fonts::get`]); none where the
    /// name's value is no font dictionary; or why it wasn't read: past what's
    /// left of the budget, or an object or a stream of it that doesn't read for
    /// the reader's own reason.
    pub font: Result<Option<Arc<Font>>, pdf_fonts::Error>,
}

impl fmt::Debug for NamedFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let font = self.font.as_ref().map(|font| font.as_ref().map(|f| f.id()));
        f.debug_struct("NamedFont")
            .field("name", &self.name.len())
            .field("font", &font)
            .finish()
    }
}

/// The fonts `page`'s resources name, its own `/Resources` or what it
/// inherits, each by its name in their `/Font`, in the order it's written:
/// each read through `fonts` ([`Fonts::get`]), once a file and kept, under
/// `limits.fonts`, its `content` taken from `limits.content`, and charged to
/// `budget`, a font kept too. A font that isn't read says why, and the rest are
/// read. The dictionaries are reached as PDFium reaches them, each its own or a
/// stream's, through two references at most.
///
/// ```
/// use pdfs::pdf_fonts::Fonts;
/// use pdfs::pdf_semantics::pages;
/// use pdfs::prelude::*;
///
/// // A page that shows a string in Times.
/// let mut doc = Document::new();
/// let page = doc.add_page(Rect::LETTER);
/// page.begin_text();
/// page.font(standard_font(Std14::TimesRoman), 12.0);
/// page.show(&Std14::TimesRoman.codes("Hi").unwrap());
/// page.end_text();
/// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
///
/// // Over the reading, as any caller with a reading of its own calls it.
/// let (reading, limits) = (pdf.reading(), pdfs::Limits::DEFAULT);
/// let catalog = pdf.catalog().unwrap();
/// let table = pages::pages(reading, catalog, Rules::Pdfium, &limits.semantics, &mut ()).unwrap();
/// let fonts = Fonts::default();
/// let mut budget = limits.font_budget;
/// let named = pdfs::page::fonts(reading, &fonts, table.get(0).unwrap(), &limits, &mut budget).unwrap();
/// assert_eq!(named.len(), 1);
/// assert!(named[0].font.as_ref().is_ok_and(Option::is_some));
/// ```
///
/// # Errors
///
/// [`Error::Unreadable`] where the resources, or their `/Font`, don't read for
/// the reader's own reason: past a cap, behind a password, or not given by the
/// source. One that's damaged isn't there: the page names no font.
pub fn fonts<'o, O>(
    objects: O,
    fonts: &Fonts,
    page: &pages::Page<'o>,
    limits: &Limits,
    budget: &mut usize,
) -> Result<Vec<NamedFont>, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    phase!(DEBUG, "fonts");
    let resources = page.attribute(Inheritable::Resources);
    let resources = match resources {
        Some(resources) => dictionary(objects, resources.written)?,
        None => None,
    };
    let named = match resources.and_then(|resources| resources.get(b"Font")) {
        Some(named) => dictionary(objects, named)?,
        None => None,
    };
    let limits_fonts = limits.reading_fonts();
    let read = |(name, font): (&[u8], &'o Object)| NamedFont {
        name: name.to_vec(),
        font: fonts.get(objects, font, &limits_fonts, budget),
    };
    Ok(named.map_or_else(Vec::new, |named| named.iter().map(read).collect()))
}

/// The strings `page`'s content shows, in order, each in its font, the forms it
/// draws followed, as PDFium reads them for its text, with no layout
/// (pdf-fonts' [`shown`](fn@pdf_fonts::shown)): its content joined as
/// [`content`](fn@content) joins it, under `limits.content`, handed to it with
/// the page's resources, its fonts read through `fonts` under `limits.fonts`,
/// its `content` taken from `limits.content`, and what its fonts' streams and
/// its forms' content decompress to charged to `budget`. What isn't read is in
/// the answer ([`Shown::unread`]), never guessed: a stream of the page's or a
/// form's whose data doesn't decode among it, which PDFium reads as written
/// ([`Unread::Undecoded`](pdf_fonts::Unread::Undecoded)).
///
/// ```
/// use pdfs::pdf_fonts::Fonts;
/// use pdfs::pdf_semantics::pages;
/// use pdfs::pdf_tokenizer::Rules;
///
/// let file = b"%PDF-1.7\n\
///     1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
///     2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
///     3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 \
///     << /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>\nendobj\n\
///     4 0 obj\n<< /Length 35 >>\nstream\nBT /F1 12 Tf [(Hel) -20 (lo)] TJ ET\nendstream\nendobj\n\
///     trailer\n<< /Size 5 /Root 1 0 R >>\n%%EOF\n";
/// let pdf = pdfs::from_bytes(&file[..]).unwrap();
///
/// // Over the reading, as any caller with a reading of its own calls it.
/// let reading = pdf.reading();
/// let catalog = pdf.catalog().unwrap();
/// let limits = pdfs::Limits::DEFAULT;
/// let table = pages::pages(reading, catalog, Rules::Pdfium, &limits.semantics, &mut ()).unwrap();
/// let fonts = Fonts::default();
/// let mut budget = limits.font_budget;
/// let shown = pdfs::page::text(reading, &fonts, table.get(0).unwrap(), &limits, &mut budget).unwrap();
/// assert_eq!(shown.text(), "Hello");
/// assert_eq!(shown.strings.len(), 2);
/// assert!(shown.is_whole());
/// ```
///
/// # Errors
///
/// As [`content`](fn@content) fails: [`Error::Content`] where its streams are
/// past the cap or one doesn't read for the reader's own reason (past a cap,
/// behind a password or not given), its [`Why`](pdf_objects::Why) kept, and
/// [`Error::Unreadable`] where its resources don't read for it.
pub fn text<'o, O>(
    objects: O,
    fonts: &Fonts,
    page: &pages::Page<'o>,
    limits: &Limits,
    budget: &mut usize,
) -> Result<Shown, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    phase!(DEBUG, "text");
    let content = content(objects, page, &limits.content)?;
    let resources = page.attribute(Inheritable::Resources);
    Ok(pdf_fonts::shown(
        objects,
        fonts,
        resources.map(|resources| resources.written),
        content.concat(),
        content.res(),
        &limits.reading_fonts(),
        budget,
    ))
}

/// The dictionary `value` is, as PDFium reaches one (`GetDict`): its own, or a
/// stream's, through two references at most; none for anything else, or past
/// that, or for a reference to nothing or to an object that's damaged.
fn dictionary<'o, O: Resolve<'o>>(
    objects: O,
    value: &'o Object,
) -> Result<Option<&'o Dict>, Unreadable> {
    let mut value = value;
    for _ in 0..2 {
        let Value::Ref(id) = *value.value() else {
            break;
        };
        value = match objects.try_resolve(id) {
            Ok(Some(object)) => object,
            Ok(None) => return Ok(None),
            Err(unreadable) if unreadable.why == pdf_objects::Why::Damaged => return Ok(None),
            Err(unreadable) => return Err(unreadable),
        };
    }
    Ok(match value.value() {
        Value::Dict(dict) => Some(dict),
        Value::Stream(stream) => Some(&stream.dict),
        _ => None,
    })
}
