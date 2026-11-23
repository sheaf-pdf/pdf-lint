//! Standalone document-building helpers over [`pdf_structure::Document`].
//!
//! [`Document`](crate::Document) delegates to these functions when deduplicating
//! resources, assembling page dictionaries, creating Standard 14 font resources,
//! and writing XMP conformance claims.

use std::collections::HashMap;

use pdf_annots::DefaultAppearance;
use pdf_content_streams::{ContentWriter, Res, ResBody, Reserved, ResourceNeeds};
use pdf_filters::Prediction;
use pdf_fonts::Std14;
use pdf_objects::object::{Data, Stream, Value};
use pdf_objects::{Dict, Object, ObjectId, dict, text};
use pdf_semantics::files::{NewEmbeddedFile, write_embedded};
use pdf_semantics::info::{self, Info};
use pdf_semantics::labels::{self, LabelRange};
use pdf_semantics::outlines::{self, OutlineNode};
use pdf_semantics::pages::PdfBox;
use pdf_semantics::tree::{names, write_names};
use pdf_semantics::viewer::{self, CatalogView};
use pdf_semantics::xmp::{self, Claims, Metadata};
use pdf_structure::Document;
use pdf_structure::write::{self, Deflate};

use crate::error::Error;
use crate::limits::Limits;

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
/// ```
/// use std::collections::HashMap;
///
/// use pdfs::pdf_content_streams::Res;
/// use pdfs::pdf_objects::{Object, ObjectId, dict, version::Version};
/// use pdfs::pdf_structure::{Document, write::Deflate};
///
/// let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
/// let mut written = HashMap::new();
/// // A font and its widths, which it reaches as object 1 of the two.
/// let font = Res::of(vec![
///     Object::from(dict! { "Type" => Object::name("Font"), "Widths" => Object::reference(1) }),
///     Object::from(vec![Object::from(500)]),
/// ])
/// .unwrap();
/// let id = pdfs::build::resource(&mut doc, &mut written, &font, Deflate::DEFAULT).unwrap();
/// // Asked again, it's the same object: written once.
/// let again = pdfs::build::resource(&mut doc, &mut written, &font, Deflate::DEFAULT).unwrap();
/// assert_eq!(id, again);
/// let widths = doc.get_dict(id).unwrap().get(b"Widths").unwrap();
/// assert_eq!(widths.as_reference(), Some(ObjectId::new(id.number + 1, 0)));
/// ```
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
    // A resource is one object at least: pdf-content-streams refuses one of
    // none.
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
/// this page's own value. Its box is written as the content writer writes a
/// number, one that isn't finite as 0.
///
/// ```
/// use std::collections::HashMap;
///
/// use pdfs::Rect;
/// use pdfs::pdf_content_streams::{ContentWriter, Paint};
/// use pdfs::pdf_objects::{Object, dict, version::Version};
/// use pdfs::pdf_structure::{Document, write::Deflate};
///
/// let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
/// let mut content = ContentWriter::new();
/// content.rect(72.0, 72.0, 100.0, 50.0);
/// content.paint(Paint::Fill);
/// let mut written = HashMap::new();
/// let page = pdfs::build::page(&mut doc, &mut written, Rect::LETTER, &content, Deflate::DEFAULT).unwrap();
/// assert_eq!(page.get(b"Type").and_then(Object::as_name), Some(&b"Page"[..]));
/// let media_box = page.get(b"MediaBox").unwrap().as_array().unwrap();
/// assert_eq!(media_box[2].as_integer(), Some(612));
/// // Its content is a stream of its own, deflated.
/// let (_, contents) = doc.dereference(page.get(b"Contents").unwrap()).unwrap();
/// let filter = contents.as_stream().unwrap().dict.get(b"Filter");
/// assert_eq!(filter.and_then(Object::as_name), Some(&b"FlateDecode"[..]));
/// ```
///
/// # Errors
///
/// [`write::Error::Unencodable`] where its content or a resource's stream
/// doesn't deflate.
pub fn page(
    doc: &mut Document,
    written: &mut HashMap<Res, ObjectId>,
    media_box: impl Into<PdfBox>,
    writer: &ContentWriter,
    deflate: Deflate,
) -> Result<Dict, write::Error> {
    let media_box = media_box.into();
    let (content, needs) = writer.named(&Reserved::default());
    let mut stream = Stream::new(Dict::new(), content);
    deflated(&mut stream, deflate)?;
    let contents = doc.add(Object::from(stream));
    let mut resources = Object::from(Dict::new());
    bind(doc, written, &needs, &mut resources, deflate)?;
    Ok(dict! {
        "Type" => Object::name("Page"), "MediaBox" => rect(media_box), "Resources" => resources,
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
/// ```
/// use pdfs::pdf_objects::{Object, ObjectId, dict, version::Version};
/// use pdfs::pdf_semantics::xmp::Claims;
/// use pdfs::pdf_structure::Document;
///
/// let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
/// doc.insert(1, dict! { "Type" => Object::name("Catalog") });
/// let mut claims = Claims::default();
/// claims.pdfa_part = Some("2".into());
/// claims.pdfa_conformance = Some("B".into());
/// let id = pdfs::build::claim(&mut doc, &claims).unwrap();
/// let catalog = doc.get_dict(ObjectId::new(1, 0)).unwrap();
/// assert_eq!(catalog.get(b"Metadata").and_then(Object::as_reference), Some(id));
/// // Claimed again, the catalog names a packet already: refused.
/// assert!(matches!(pdfs::build::claim(&mut doc, &claims), Err(pdfs::Error::HasMetadata)));
/// ```
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
        dict! { "Type" => Object::name("Metadata"), "Subtype" => Object::name("XML") },
        packet.into_bytes(),
    );
    let id = doc.add(Object::from(metadata));
    if let Some(catalog) = doc.catalog_mut() {
        catalog.set("Metadata", id);
    }
    Ok(id)
}

/// Writes `info` as an indirect `/Info` dictionary in `doc` and sets `doc`'s
/// trailer `/Info` reference to it, returning the allocated [`ObjectId`].
pub fn set_info(doc: &mut Document, info: &Info) -> ObjectId {
    let id = doc.add(info::write(info));
    doc.trailer_mut().dict_mut().set("Info", id);
    id
}

/// Writes `meta` as an unfiltered `/Type /Metadata /Subtype /XML` XMP packet
/// stream in `doc` and sets its catalog's `/Metadata` reference to it.
///
/// Unlike [`claim`] (which writes conformance claims alone and refuses when
/// `/Metadata` is already set), [`set_metadata`] writes the complete [`Metadata`]
/// record and replaces any existing catalog `/Metadata` pointer.
///
/// # Errors
///
/// [`Error::NoCatalog`] if `/Root` names no catalog, or [`Error::Claims`] if a
/// metadata value cannot be represented identically in XML 1.0.
pub fn set_metadata(doc: &mut Document, meta: &Metadata) -> Result<ObjectId, Error> {
    if doc.catalog().is_none() {
        return Err(Error::NoCatalog);
    }
    let packet = xmp::write_metadata(meta).map_err(Error::Claims)?;
    let stream = Stream::new(
        dict! { "Type" => Object::name("Metadata"), "Subtype" => Object::name("XML") },
        packet.into_bytes(),
    );
    let id = doc.add(Object::from(stream));
    if let Some(catalog) = doc.catalog_mut() {
        catalog.set("Metadata", id);
    }
    Ok(id)
}

/// Writes `items` into `doc` as a doubly-linked `/Outlines` hierarchy and sets
/// its catalog's `/Outlines` reference to the new root.
///
/// # Errors
///
/// [`Error::NoCatalog`] if `/Root` names no catalog, or [`Error::OutlinesWrite`]
/// if `items` exceeds `limits` or contains an invalid RGB color.
pub fn set_outlines(
    doc: &mut Document,
    items: &[OutlineNode],
    limits: &pdf_semantics::Limits,
) -> Result<ObjectId, Error> {
    if doc.catalog().is_none() {
        return Err(Error::NoCatalog);
    }
    let written = outlines::write(items, limits, || doc.new_id())?;
    for (id, node) in written.nodes {
        doc.set(id, node);
    }
    if let Some(catalog) = doc.catalog_mut() {
        catalog.set("Outlines", written.root);
    }
    Ok(written.root)
}

/// Writes `ranges` as `doc`'s catalog `/PageLabels` number tree.
///
/// # Errors
///
/// [`Error::NoCatalog`] if `/Root` names no catalog, or
/// [`Error::PageLabelsWrite`] if any range is invalid or duplicated.
pub fn set_page_labels(doc: &mut Document, ranges: &[LabelRange]) -> Result<(), Error> {
    if doc.catalog().is_none() {
        return Err(Error::NoCatalog);
    }
    let tree = labels::write(ranges)?;
    if let Some(catalog) = doc.catalog_mut() {
        catalog.set("PageLabels", tree);
    }
    Ok(())
}

/// Applies `view` (`/PageLayout`, `/PageMode`, `/Lang`, and `/ViewerPreferences`)
/// onto `doc`'s catalog dictionary.
///
/// # Errors
///
/// [`Error::NoCatalog`] if `/Root` names no catalog.
pub fn set_catalog_view(doc: &mut Document, view: &CatalogView) -> Result<(), Error> {
    let catalog = doc.catalog_mut().ok_or(Error::NoCatalog)?;
    viewer::apply_catalog(catalog, view);
    Ok(())
}

/// Embeds `data` into `doc` according to `spec` (deflating the `/EmbeddedFile`
/// stream with `deflate`), registers its `/Filespec` in `doc`'s catalog
/// `/Names /EmbeddedFiles` name tree (preserving any existing entries), and—when
/// `spec.relationship.is_some()`—appends the `/Filespec` reference to the
/// catalog's `/AF` array. Returns the `/Filespec` [`ObjectId`].
///
/// # Errors
///
/// [`Error::NoCatalog`] if `/Root` names no catalog, [`Error::Write`] if
/// deflating `data` fails, or [`Error::Unreadable`] / [`Error::TreeWrite`] if
/// reading or rewriting `/Names /EmbeddedFiles` fails.
pub fn add_embedded_file(
    doc: &mut Document,
    spec: &NewEmbeddedFile,
    data: Vec<u8>,
    limits: &pdf_semantics::Limits,
    deflate: Deflate,
) -> Result<ObjectId, Error> {
    let mut existing_entries: Vec<(Vec<u8>, Object)> = Vec::new();
    let (mut names_dict, mut existing_af) = {
        let catalog = doc.catalog().ok_or(Error::NoCatalog)?;
        let names_d = owned(doc, catalog.get(b"Names"));
        if let Some(ef_root) = names_d.get(b"EmbeddedFiles") {
            for item in names(&*doc, ef_root, limits, ()) {
                let (k, v) = item?;
                existing_entries.push((k.to_vec(), v.clone()));
            }
        }
        let af_vec = catalog
            .get(b"AF")
            .and_then(|v| doc.dereference(v))
            .and_then(|(_, o)| o.as_array())
            .map(<[Object]>::to_vec)
            .unwrap_or_default();
        (names_d, af_vec)
    };

    let mut actual_spec = spec.clone();
    actual_spec.size = data.len();
    let stream_id = doc.new_id();
    let written = write_embedded(&actual_spec, stream_id);
    let mut ef_stream = Stream::new(written.stream_dict, data);
    deflated(&mut ef_stream, deflate)?;
    doc.set(stream_id, Object::from(ef_stream));
    let filespec_id = doc.add(written.filespec_dict);

    let new_key = text::encode(&actual_spec.name);
    existing_entries.retain(|(k, _)| *k != new_key);
    existing_entries.push((new_key, Object::from(filespec_id)));
    let ef_tree = write_names(existing_entries)?;
    names_dict.set("EmbeddedFiles", ef_tree);

    if actual_spec.relationship.is_some() {
        existing_af.push(Object::from(filespec_id));
    }

    if let Some(catalog) = doc.catalog_mut() {
        catalog.set("Names", names_dict);
        if actual_spec.relationship.is_some() {
            catalog.set("AF", existing_af);
        }
    }
    Ok(filespec_id)
}

/// Writes `built` (and any synthesized `/AP /N` and `/RO` Form XObject streams,
/// deflated by `deflate`) into `doc`, appends its `ObjectId` to `page_id`'s
/// `/Annots` array (copying an indirect `/Annots` array into `page_id` so shared
/// arrays are never mutated behind other pages' backs), and returns the new
/// annotation `ObjectId`.
///
/// A redaction's `/RO` overlay is also bound as `/AP /R` beside `/N`, so a
/// viewer shows what applying the mark leaves while the pointer is over it.
///
/// The font an annotation was built in
/// ([`BuiltAnnot::da_font`](pdf_annots::BuiltAnnot::da_font), from
/// `free_text_with_font` and `redact_with_font`) is put in the form's
/// default resources (`/AcroForm /DR /Font`) under the name its `/DA` gives,
/// as a `/DA` font is found there (ISO 32000-2, 12.7.4.3); where that name is
/// another font's, under the first free one pdf-forms'
/// [`font_name`](pdf_forms::default_resources::font_name) gives, and `/DA`
/// is written again to name it, nothing else of it changed. The font's
/// object is the number a resource already written names
/// ([`Res::written`]), as an embedded font's is; a resource of objects is
/// written first, as [`resource`] writes one, here for each annotation. The
/// form, its default resources and their fonts are extended where they are,
/// directly or in the object a reference names, and made where there are
/// none: a document with no form gains one with an empty `/Fields` and no
/// other entry but `/DR`. A `/DA` that names no font registers nothing.
///
/// # Errors
///
/// [`Error::Write`] if deflating an appearance stream or a resource's
/// stream fails; [`Error::NoPageTree`] if `page_id` is not a dictionary in
/// `doc`; and [`Error::NoCatalog`], with nothing changed, if there's a font
/// to register and the trailer's `/Root` names no catalog.
pub fn add_annot(
    doc: &mut Document,
    page_id: ObjectId,
    built: pdf_annots::BuiltAnnot,
    deflate: Deflate,
) -> Result<ObjectId, Error> {
    add_annot_in(doc, &mut HashMap::new(), page_id, built, deflate)
}

/// [`add_annot`], a resource of objects the font is written once through
/// `written`, as a page's resources are.
pub(crate) fn add_annot_in(
    doc: &mut Document,
    written: &mut HashMap<Res, ObjectId>,
    page_id: ObjectId,
    mut built: pdf_annots::BuiltAnnot,
    deflate: Deflate,
) -> Result<ObjectId, Error> {
    if doc.get_dict(page_id).is_none() {
        return Err(Error::NoPageTree);
    }
    if let Some(font) = built.da_font.take() {
        register(doc, written, &mut built.dict, &font, deflate)?;
    }
    let mut ro_id = None;
    if let Some(ro) = built.redact_overlay.take() {
        let mut s = ro.to_stream();
        deflated(&mut s, deflate)?;
        let id = doc.add(Object::from(s));
        pdf_annots::AnnotMut::new(&mut built.dict).set_redact_overlay(id);
        ro_id = Some(id);
    }
    if let Some(ap) = built.normal_ap.take() {
        let mut s = ap.to_stream();
        deflated(&mut s, deflate)?;
        let ap_id = doc.add(Object::from(s));
        let mut m = pdf_annots::AnnotMut::new(&mut built.dict);
        m.set_normal_appearance(ap_id);
        if let Some(ro_id) = ro_id {
            m.set_rollover_appearance(ro_id);
        }
    }
    let annot_id = doc.add(Object::from(built.dict));
    let existing_annots = doc
        .get_dict(page_id)
        .and_then(|p| p.get(b"Annots"))
        .and_then(|v| doc.dereference(v))
        .and_then(|(_, o)| o.as_array())
        .map(<[Object]>::to_vec);
    if let Some(page_mut) = doc.get_dict_mut(page_id) {
        let mut items = existing_annots.unwrap_or_default();
        items.push(Object::from(annot_id));
        page_mut.set("Annots", items);
    }
    Ok(annot_id)
}

/// `font` put in `doc`'s form's default resources under the name the
/// annotation `dict`'s `/DA` gives it, or the first free one after it, and
/// `/DA` written again where that moved ([`add_annot`]). Nothing where `/DA`
/// names no font.
fn register(
    doc: &mut Document,
    written: &mut HashMap<Res, ObjectId>,
    dict: &mut Dict,
    font: &Res,
    deflate: Deflate,
) -> Result<(), Error> {
    let da = dict.get(b"DA").and_then(Object::as_bytes);
    let da = da.and_then(|da| DefaultAppearance::parse(da, &pdf_content_streams::Limits::DEFAULT));
    let Some(mut da) = da else {
        return Ok(());
    };
    let Some(wanted) = da.font_name.take() else {
        return Ok(());
    };
    if doc.catalog().is_none() {
        return Err(Error::NoCatalog);
    }
    let id = resource(doc, written, font, deflate)?;

    // Each of the form, its default resources and their fonts: in the object
    // a reference names, changed there; else in its entry, put back.
    let catalog = doc.catalog().ok_or(Error::NoCatalog)?;
    let (form_at, form) = held(doc, catalog.get(b"AcroForm"));
    let mut form = form.unwrap_or_else(|| dict! { "Fields" => Vec::<Object>::new() });
    let (resources_at, resources) = held(doc, form.get(b"DR"));
    let mut resources = resources.unwrap_or_default();
    let (fonts_at, fonts) = held(doc, resources.get(b"Font"));
    let mut fonts = fonts.unwrap_or_default();
    let name = pdf_forms::default_resources::font_name(Some(&fonts), &wanted, id).into_owned();
    if fonts.get(&name).and_then(Object::as_reference) != Some(id) {
        // The innermost held in an object of its own is changed there, which
        // nothing above it needs to know; each held in its entry is put in
        // the one above it.
        fonts.set(name.clone(), id);
        if let Some(at) = fonts_at {
            if let Some(held) = doc.get_dict_mut(at) {
                held.set(name.clone(), id);
            }
        } else if let Some(at) = resources_at {
            if let Some(held) = doc.get_dict_mut(at) {
                held.set("Font", fonts);
            }
        } else {
            resources.set("Font", fonts);
            if let Some(at) = form_at {
                if let Some(held) = doc.get_dict_mut(at) {
                    held.set("DR", resources);
                }
            } else if let Some(catalog) = doc.catalog_mut() {
                form.set("DR", resources);
                catalog.set("AcroForm", form);
            }
        }
    }
    if name != wanted {
        da.font_name = Some(name);
        pdf_annots::AnnotMut::new(dict).set_default_appearance(&da);
    }
    Ok(())
}

/// The dictionary `value` is, a copy, and the object it's in where a chain
/// of references from `value` names one; for anything but a dictionary, or
/// for none, neither.
fn held(doc: &Document, value: Option<&Object>) -> (Option<ObjectId>, Option<Dict>) {
    match value.and_then(|value| doc.dereference(value)) {
        Some((at, object)) => match object.as_dict() {
            Some(dict) => (at, Some(dict.clone())),
            None => (None, None),
        },
        None => (None, None),
    }
}

/// Flattens eligible annotations on `page_id` in `doc` according to `opts` and
/// `limits`, isolating any existing `/Contents` streams inside `q ... Q`
/// without decompressing untouched base streams (so copy-on-write and
/// incremental updates remain intact), binding new `/Resources/XObject` entries,
/// and updating `/Annots`.
///
/// `/Redact` marks stay on the page, undrawn, unless `opts` sets
/// [`include_redactions`](pdf_annots::FlattenOpts::include_redactions):
/// flattening never removes the content under a mark, so drawing one would
/// only make the page look redacted.
///
/// # Errors
///
/// [`Error::NoPageTree`] if `page_id` is not a dictionary in `doc`,
/// [`Error::Annots`] if `plan_flatten` fails, or [`Error::Write`] if deflating
/// the overlay content stream fails.
pub fn flatten_page(
    doc: &mut Document,
    page_id: ObjectId,
    opts: pdf_annots::FlattenOpts,
    limits: &Limits,
    deflate: Deflate,
) -> Result<pdf_annots::FlattenPlan, Error> {
    let annots_limits = limits.reading_annots();
    let plan = {
        let page_dict = doc.get_dict(page_id).ok_or(Error::NoPageTree)?;
        let res = inherited_resources(doc, page_dict);
        pdf_annots::plan_flatten(&*doc, page_dict, Some(page_id), res, opts, &annots_limits)?
    };
    if !plan.changed() {
        return Ok(plan);
    }

    let mut page_res = {
        let page_dict = doc.get_dict(page_id).ok_or(Error::NoPageTree)?;
        owned(doc, inherited_resources(doc, page_dict))
    };
    if !plan.xobject_bindings.is_empty() {
        let mut xo = owned(doc, page_res.get(b"XObject"));
        for (name, form_id) in &plan.xobject_bindings {
            xo.set(name, *form_id);
        }
        page_res.set("XObject", xo);
    }

    // Collect existing `/Contents` stream references without decoding them.
    let existing_refs: Vec<Object> = {
        let page_dict = doc.get_dict(page_id).ok_or(Error::NoPageTree)?;
        match page_dict.get(b"Contents") {
            None => Vec::new(),
            Some(raw) => match doc.dereference(raw) {
                Some((_, obj)) => match obj.as_array() {
                    Some(arr) => arr.to_vec(),
                    None if obj.as_stream().is_some() => vec![raw.clone()],
                    None => Vec::new(),
                },
                None => Vec::new(),
            },
        }
    };

    let new_contents: Option<Object> = if plan.overlay_content.is_empty() {
        None
    } else if existing_refs.is_empty() {
        let mut s = Stream::new(Dict::new(), plan.overlay_content.clone());
        deflated(&mut s, deflate)?;
        Some(Object::from(doc.add(Object::from(s))))
    } else {
        let mut push_q = Stream::new(Dict::new(), b"q\n".to_vec());
        deflated(&mut push_q, deflate)?;
        let q_id = doc.add(Object::from(push_q));

        let mut pop_and_overlay = Vec::with_capacity(plan.overlay_content.len() + 3);
        pop_and_overlay.extend_from_slice(b"\nQ\n");
        pop_and_overlay.extend_from_slice(&plan.overlay_content);
        let mut overlay_s = Stream::new(Dict::new(), pop_and_overlay);
        deflated(&mut overlay_s, deflate)?;
        let ov_id = doc.add(Object::from(overlay_s));

        let mut seq = Vec::with_capacity(existing_refs.len() + 2);
        seq.push(Object::from(q_id));
        seq.extend(existing_refs);
        seq.push(Object::from(ov_id));
        Some(Object::from(seq))
    };

    if let Some(page_mut) = doc.get_dict_mut(page_id) {
        if !plan.xobject_bindings.is_empty() {
            page_mut.set("Resources", page_res);
        }
        if let Some(c) = new_contents {
            page_mut.set("Contents", c);
        }
        if plan.kept_annots.is_empty() {
            page_mut.remove(b"Annots");
        } else {
            page_mut.set("Annots", plan.kept_annots.clone());
        }
    }
    Ok(plan)
}

fn inherited_resources<'d>(doc: &'d Document, mut cur: &'d Dict) -> Option<&'d Object> {
    for _ in 0..64 {
        if let Some(res) = cur.get(b"Resources") {
            return Some(res);
        }
        let parent = cur.get(b"Parent")?;
        let (_, obj) = doc.dereference(parent)?;
        cur = obj.as_dict()?;
    }
    None
}

/// `stream`'s data deflated by `deflate`, and `/Filter /FlateDecode`, where
/// it's held in memory and names no filter; else it's left as it is.
pub(crate) fn deflated(stream: &mut Stream, deflate: Deflate) -> Result<(), write::Error> {
    if stream.dict.has(b"Filter") {
        return Ok(());
    }
    let Data::Bytes(data) = &stream.data else {
        return Ok(());
    };
    let mut out = Vec::new();
    deflate.encode(data, Prediction::None, &mut out)?;
    stream.dict.set("Filter", Object::name("FlateDecode"));
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
/// bits, as a reader reads it either way, else a real; and one that isn't
/// finite, which no file can hold, 0, as pdf-content-streams' writer writes
/// one.
pub(crate) fn rect(rect: PdfBox) -> Object {
    let number = |x: f64| {
        let x = if x.is_finite() { x } else { 0.0 };
        let whole = x.fract() == 0.0 && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&x);
        match whole {
            true => Object::from(x as i64),
            false => Object::from(x),
        }
    };
    Object::from(rect.0.map(number).to_vec())
}
