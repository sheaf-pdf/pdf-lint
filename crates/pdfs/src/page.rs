//! A page's recipes, each a function over the family's traits, so that any
//! reader of a file's objects and streams' data can call it: pdf-structure's
//! reading, its document beside it, or pdf-objects' `WithStreams`. A
//! [`Page`](crate::Page) calls each with its file's defaults.

use std::fmt;

use pdf_content_streams::{Concat, Join, Limits as ContentLimits, Operations, ResCx};
use pdf_objects::{Resolve, Streams};
use pdf_semantics::pages::{self, Inheritable};

use crate::error::Error;
use crate::phases::phase;

/// A page's content: its streams decoded and joined as PDFium joins them, and
/// what reading it needs of the resources it's drawn with
/// ([`content`](fn@content)). Its `Debug` gives the joined bytes' length and
/// how many streams there are, not the bytes, which are content.
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
            .finish_non_exhaustive()
    }
}

/// `page`'s content, as `objects` give its streams' data: its `/Contents`,
/// a stream or an array of them (ISO 32000-2, 7.7.3.3), joined under
/// `limits` as PDFium joins them (pdf-content-streams'
/// [`Join::push_contents`]), and the color spaces its resources name, its
/// own `/Resources` or what it inherits, counted under `limits.color`
/// ([`ResCx::read`]). A place where the tree has no page, or a page with no
/// `/Contents`, has none: no stream.
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
/// a stream, or `/Contents` or an element of it, doesn't read for the
/// reader's own reason, its [`Why`](pdf_objects::Why) kept (pdf-content-streams'
/// [`Pushed`](pdf_content_streams::Pushed)); [`Error::Unreadable`] where an
/// object of its resources doesn't.
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
