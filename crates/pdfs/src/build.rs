//! A document's writing recipes, each a function over pdf-structure's
//! document, so that a caller building one of its own can call it: what
//! content draws with written once a file and bound in the resources it
//! goes with, a page's dictionary, a standard 14 font, and claims as XMP. A
//! [`Document`](crate::Document) calls each with its defaults.

use std::collections::HashMap;

use pdf_content_streams::{ContentWriter, Res, ResBody, Reserved, ResourceNeeds};
use pdf_filters::Prediction;
use pdf_fonts::Std14;
use pdf_objects::object::{Data, Stream, Value};
use pdf_objects::{Dict, Object, ObjectId, dict};
use pdf_semantics::pages::Rect;
use pdf_semantics::xmp::{self, Claims};
use pdf_structure::Document;
use pdf_structure::write::{self, Deflate};

use crate::error::Error;

/// The resource a standard 14 font is, to draw text in it
/// ([`ContentWriter::font`]): its dictionary, pdf-fonts'
/// [`Std14::dict`], the twelve Latin ones in WinAnsiEncoding and Symbol and
/// ZapfDingbats in their own. The text shown in it is put in its codes by
/// [`Std14::codes`], which refuses a character its encoding has no code
/// for, so nothing is written as another character.
///
/// ```
/// use pdfs::pdf_fonts::Std14;
///
/// let helvetica = pdfs::standard_font(Std14::Helvetica);
/// assert_eq!(helvetica, pdfs::standard_font(Std14::Helvetica));
/// assert_ne!(helvetica, pdfs::standard_font(Std14::TimesRoman));
/// assert_eq!(Std14::Helvetica.codes("Größe"), Ok(b"Gr\xF6\xDFe".to_vec()));
/// ```
#[must_use]
pub fn standard_font(font: Std14) -> Res {
    Res::dict(font.dict()).expect("a dictionary of names holds no stream")
}

/// `res`'s object in `doc`, written the first time `written` is asked for
/// it, so that a resource many pages draw with is one object a file: one
/// already written ([`ResBody::Written`]) is that object; one of objects
/// ([`ResBody::Objects`]) is added, each object at a number of its own
/// ([`Document::new_id`]), each reference `(i, 0)` among them made the
/// `i`th's number and one past them `null`, as a reference to no object
/// reads (ISO 32000-2, 7.3.10), and each stream's data in memory that names
/// no filter deflated by `deflate`. `written` is for one document only.
///
/// # Errors
///
/// [`write::Error::Unencodable`] where a stream's data doesn't deflate,
/// and nothing is added.
pub fn resource(
    doc: &mut Document,
    written: &mut HashMap<Res, ObjectId>,
    res: &Res,
    deflate: Deflate,
) -> Result<ObjectId, write::Error> {
    let objects = match res.body() {
        ResBody::Written(id) => return Ok(*id),
        ResBody::Objects(objects) => objects,
        // The facade pins pdf-content-streams exactly, so a kind of body it
        // adds comes with a release of the facade that reads it.
        _ => unreachable!("a resource is objects or one written"),
    };
    if let Some(&id) = written.get(res) {
        return Ok(id);
    }
    let mut made = Vec::with_capacity(objects.len());
    for object in objects.iter() {
        let mut object = object.clone();
        if let Value::Stream(stream) = object.value_mut() {
            deflated(stream, deflate)?;
        }
        made.push(object);
    }
    let ids: Vec<ObjectId> = made.iter().map(|_| doc.new_id()).collect();
    for (mut object, &id) in made.into_iter().zip(&ids) {
        object.renumber(|to| match to.generation {
            0 => ids.get(to.number as usize).copied(),
            _ => None,
        });
        doc.set(id, object);
    }
    written.insert(res.clone(), ids[0]);
    Ok(ids[0])
}

/// `needs`, what content draws with ([`ContentWriter::finish`]), each
/// written into `doc` once through `written` ([`resource`]) and bound under
/// its name in its category in `resources`, a `/Resources` value about to be
/// written. A dictionary is bound into where it is; one given by reference,
/// and each category given so, is copied in first, the dictionary the
/// reference names read through `doc`, so that a dictionary other pages
/// share is never changed; anything else is a dictionary of its own. With
/// no needs, `resources` is left as it is.
///
/// ```
/// use std::collections::HashMap;
///
/// use pdfs::pdf_content_streams::{ContentWriter, Reserved};
/// use pdfs::pdf_fonts::Std14;
/// use pdfs::pdf_objects::{Object, ObjectId, dict, version::Version};
/// use pdfs::pdf_structure::{Document, write::Deflate};
///
/// let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
/// // A font dictionary two pages share by reference.
/// doc.insert(5, dict! { "F1" => Object::reference(6) });
/// let mut written = HashMap::new();
/// let mut page = ContentWriter::new();
/// page.font(pdfs::standard_font(Std14::Helvetica), 12.0);
/// let (_, needs) = page.finish(&Reserved::default());
///
/// let mut resources = Object::from(dict! { "Font" => Object::reference(5) });
/// pdfs::build::bind(&mut doc, &mut written, &needs, &mut resources, Deflate::DEFAULT).unwrap();
/// let fonts = resources.as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap();
/// assert_eq!(fonts.len(), 2);
/// // The shared one is as it was, and the font is written once.
/// assert_eq!(doc.get_dict(ObjectId::new(5, 0)).unwrap().len(), 1);
/// assert_eq!(written.len(), 1);
/// ```
///
/// # Errors
///
/// As [`resource`] fails, where a stream doesn't deflate: `resources` is
/// left as it was.
pub fn bind(
    doc: &mut Document,
    written: &mut HashMap<Res, ObjectId>,
    needs: &ResourceNeeds,
    resources: &mut Object,
    deflate: Deflate,
) -> Result<(), write::Error> {
    if needs.0.is_empty() {
        return Ok(());
    }
    let mut bound = owned(doc, Some(resources));
    for need in &needs.0 {
        let id = resource(doc, written, &need.res, deflate)?;
        let key = need.kind.key();
        let mut category = owned(doc, bound.get(key));
        category.set(&need.name, id);
        bound.set(key, category);
    }
    *resources = Object::from(bound);
    Ok(())
}

/// A page's dictionary for content `writer` has written, to add to `doc`:
/// `/Type /Page`, `/MediaBox` `media_box`, `/Contents` its content as a
/// stream object of its own, deflated by `deflate` (`/Filter
/// /FlateDecode`), and `/Resources` of its own, what the content draws with
/// bound in it ([`bind`]), each resource written once through `written`.
/// The content's names are the writer's own, as no resources come before
/// them ([`Reserved::default`]); the writer is kept, so the same content can
/// be written again. Its `/Parent` is the caller's to set, as is any other
/// attribute: one a page tree's node gives every page under it is given by
/// this page's own value.
///
/// # Errors
///
/// [`write::Error::Unencodable`] where its content or a resource's stream
/// doesn't deflate.
pub fn page(
    doc: &mut Document,
    written: &mut HashMap<Res, ObjectId>,
    media_box: Rect,
    writer: &ContentWriter,
    deflate: Deflate,
) -> Result<Dict, write::Error> {
    let (content, needs) = writer.named(&Reserved::default());
    let mut stream = Stream::new(Dict::new(), content);
    deflated(&mut stream, deflate)?;
    let contents = doc.add(Object::from(stream));
    let mut resources = Object::from(Dict::new());
    bind(doc, written, &needs, &mut resources, deflate)?;
    Ok(dict! {
        "Type" => "Page", "MediaBox" => rect(media_box), "Resources" => resources,
        "Contents" => contents,
    })
}

/// What `claims` says, as an XMP packet ([`xmp::write`]) in a metadata
/// stream (`/Type /Metadata /Subtype /XML`, ISO 32000-2, 14.3.2), unfiltered,
/// as a PDF/A part wants it, added to `doc` and named by its catalog's
/// `/Metadata`: its number. The packet holds the claims alone, so a catalog
/// that names a `/Metadata` already, whose packet would be replaced by it
/// and the rest of what it says lost, is refused; a caller that means to
/// replace it sets `/Metadata` itself.
///
/// # Errors
///
/// Refused with nothing changed: [`Error::NoCatalog`] where its trailer's
/// `/Root` names no dictionary; [`Error::HasMetadata`] where the catalog
/// names a `/Metadata`; [`Error::Claims`] where a claim wouldn't read back
/// as it's given.
pub fn claim(doc: &mut Document, claims: &Claims) -> Result<ObjectId, Error> {
    let catalog = doc.catalog().ok_or(Error::NoCatalog)?;
    if catalog.has(b"Metadata") {
        return Err(Error::HasMetadata);
    }
    let packet = xmp::write(claims).map_err(Error::Claims)?;
    let metadata = Stream::new(
        dict! { "Type" => "Metadata", "Subtype" => "XML" },
        packet.into_bytes(),
    );
    let id = doc.add(Object::from(metadata));
    if let Some(catalog) = doc.catalog_mut() {
        catalog.set("Metadata", id);
    }
    Ok(id)
}

/// `stream`'s data deflated by `deflate`, and `/Filter /FlateDecode`, where
/// it's held in memory and names no filter; else it's left as it is.
fn deflated(stream: &mut Stream, deflate: Deflate) -> Result<(), write::Error> {
    if stream.dict.has(b"Filter") {
        return Ok(());
    }
    let Data::Bytes(data) = &stream.data else {
        return Ok(());
    };
    let mut out = Vec::new();
    deflate.encode(data, Prediction::None, &mut out)?;
    stream.dict.set("Filter", "FlateDecode");
    stream.set_bytes(out);
    Ok(())
}

/// The dictionary `value` is, a copy of it to change: its own, or the one a
/// chain of references from it names in `doc`; a new one for anything
/// else, or for none.
fn owned(doc: &Document, value: Option<&Object>) -> Dict {
    let named = value.and_then(|value| doc.dereference(value));
    (named.and_then(|(_, object)| object.as_dict()))
        .cloned()
        .unwrap_or_default()
}

/// `rect` as written: each number an integer where it's whole and within 32
/// bits, as a reader reads it either way, else a real.
pub(crate) fn rect(rect: Rect) -> Object {
    let number = |x: f64| {
        let whole = x.fract() == 0.0 && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&x);
        match whole {
            true => Object::from(x as i64),
            false => Object::from(x),
        }
    };
    Object::from(rect.0.map(number).to_vec())
}
