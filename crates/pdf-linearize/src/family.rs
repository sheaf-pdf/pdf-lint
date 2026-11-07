//! A [`Document`](pdf_structure::Document) written as a linearized file: the
//! graph [`plan()`] takes built from its objects, each body written through
//! pdf-structure's writer in the plan's numbers, and [`assemble`] around them.
//!
//! [`linearize`] is for a caller that holds a document, and [`linearize_bytes`]
//! for one that holds a file: a whole parse and write again, so a convenience
//! rather than a way past anything. Either writes every object at a new number,
//! as Annex F has it, so anything that names one from outside the file, a
//! signature's byte range above all, doesn't hold afterwards, as under any
//! rewrite. [`check()`] says whether a file that says it's linearized holds,
//! and [`facts()`] gives what that's judged from, [`facts_with_reading`] of a
//! file read whole beside its reading.
//!
//! # What changes in the document
//!
//! - **Inheritable attributes are pushed down** (F.3.7, F.3.10): each page gets
//!   what it inherited, as it was written, a reference staying one, and each
//!   page tree node loses it, so a page the tree doesn't list gets what it
//!   inherited from such a node too. A page or node that needs neither is
//!   copied as read.
//! - **What the trailer doesn't reach isn't written**, nor object streams,
//!   cross-reference streams and a linearization dictionary, as under
//!   [`write::document`]; nor an object only a stream's `/Length` names, which
//!   the writer writes direct, nor a null object, nor an object that's only a
//!   reference that ends at one of those or at nothing, a reference to any of
//!   which reads the same as one to none (7.3.10) and is written `null`.
//! - **The header** is the document's, at least 1.5 where a stream lists the
//!   file, unless [`Options::version`] says.
//!
//! # What's refused
//!
//! Annex F gives each page one place, and the file's numbers and hint tables
//! then depend on which pages a viewer finds, so a tree that gives a page no
//! place of its own isn't laid out ([`Error::PageTree`]): a place with no page
//! at it, a page written in place or listed twice, a tree PDFium's rules and
//! ISO's read as other pages, or a page they read an inheritable attribute of
//! differently, or that reads otherwise going down `/Kids` from the root than
//! up its `/Parent`s, as pushing one value down would change the other's page.
//! So is a document no password opened ([`Error::Locked`]), whose strings and
//! streams are still what the file holds, and whose key there's none of.
//!
//! # Encrypted
//!
//! Under an [`Encryptor`], the reading's own ([`Encryptor::of`]) or a new one
//! ([`Encryptor::create`]), every string and stream is encrypted under the
//! number it's written at (ISO 32000-2, 7.6), the hint stream's data too; a
//! cross-reference stream isn't (7.5.8.2), nor the encryption dictionary, nor
//! what its values refer to (7.6.2), which part 4 holds at their new numbers
//! (F.3.5). An object stream goes as a stream; asked to pack under an
//! encryption that doesn't ([`Encryptor::packs`]), the file is written
//! unpacked. An object as read is copied at its new number only where no key is
//! made from the number (AES-256), and written anew otherwise. With no
//! encryptor, an encrypted reading is written in the clear, as
//! [`write::document`] writes it; [`linearize_bytes`] keeps a file's
//! encryption.
//!
//! **Under keys made per object, every number is reused.** A section of this
//! layout lists no number free but 0 (F.3.4, F.3.11), and its numbers run from
//! 1 without a gap (F.3.1), so what [`Encryptor::of`] says of a file's free
//! numbers can't hold here: none is listed, and an object may be written at
//! one. Every object moves, so under RC4 every number's keystream now encrypts
//! another object's plaintext. One who holds both the file read and this one
//! has, for each number, the XOR of the two plaintexts
//! ([`Method::Rc4`](pdf_structure::pdf_crypt::Method::Rc4)). AES-128's IVs are
//! made from the data and the number, so nothing leaks that way. A caller who
//! doesn't want that gives [`Encryptor::create`] a new password, whose key is
//! new.
//!
//! # Where everything goes
//!
//! Part 4 has the catalog and the values of its `/ViewerPreferences`,
//! `/PageMode`, `/OpenAction`, `/AcroForm` (that object alone) and `/Threads`
//! with its thread dictionaries (F.3.5). Each page's objects are found by
//! walking from it, in the order F.3.7 recommends: `/Annots`, `/B`,
//! `/Resources`, the rest, `/Contents`, and images and font files last, never
//! through `/Parent` or `/Thumb`, stopping at the page tree's and the document
//! parts' nodes. Part 9 has the groups a hint table names: the outline in the
//! order F.3.10 has it, threads' information dictionaries, named destinations,
//! `/Info`, the form's fields, the logical structure, renditions, and each
//! embedded file its own. The outline goes in part 6 instead under `/PageMode
//! /UseOutlines`.
//!
//! No page's content is read, so a resource isn't placed by where a content
//! stream first uses it, as F.3.7 would like, but where its page's `/Resources`
//! names it. `/Encrypt` and its values are in part 4 too (F.3.5).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use pdf_objects::object::{Object, ObjectId};
use pdf_objects::serialize::{self, Encrypt as _};
use pdf_objects::version::Version;
use pdf_semantics::pages;
use pdf_structure::pdf_crypt::Password;
use pdf_structure::pdf_filters::{self, Prediction};
use pdf_structure::read::{self, ByteSource, Reading};
use pdf_structure::write::{self, Deflate, Encrypting, Encryptor, Parts, ReadFrom};
use pdf_structure::{Encryption, XrefKind};

use crate::Limits;
use crate::layout::{self, Encoded, Layout, Piece, Stream, StreamKind, assemble};
pub use crate::plan::FirstPage;
use crate::plan::{self, Kind, Plan, What, plan};

mod facts;
pub(crate) mod graph;

pub use facts::{check, facts, facts_with_reading};
use graph::Prepared;

/// How a file is laid out. A caller sets the fields on a copy of
/// [`Options::DEFAULT`] (`options.kind = XrefKind::Stream`, say), as it does a
/// write's.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Options<'a> {
    /// What lists the file: cross-reference tables by default, or streams.
    pub kind: XrefKind,
    /// Whether what may go in an object stream goes in one, grouped by the
    /// pages that use it. Only a stream lists a member, so under a table it's
    /// refused ([`write::Error::PackedTable`]).
    pub pack: bool,
    /// How what's compressed is deflated: each object stream, the hint stream
    /// and a cross-reference stream.
    pub deflate: Deflate,
    /// How many threads object streams are built on, at most: 0, the default,
    /// for as many as the machine runs at once, up to 8. Built on any number,
    /// they're the same bytes.
    pub threads: usize,
    /// The version the header says: by default the document's, at least 1.5
    /// where a stream lists the file (7.5.8); given, it's written as given.
    pub version: Option<Version>,
    /// Which page goes first.
    pub first_page: FirstPage,
    /// The plan's caps, the most pages among them.
    pub limits: Limits,
    /// How [`linearize_bytes`] opens the file it's given: the caps it reads
    /// under and the backend it inflates with. A caller given a file it doesn't
    /// trust lowers them here; [`linearize`] takes a document already read.
    /// Their passwords aren't read: [`linearize_bytes`] opens a file by those
    /// it's given.
    pub read: read::Options<'a>,
    /// A flag that, set while it runs, stops it: it's checked before each
    /// object and object stream, and nothing's written
    /// ([`write::Error::Stopped`]).
    pub stop: Option<&'a AtomicBool>,
}

impl Options<'static> {
    /// A file with tables, nothing in object streams, deflated as
    /// [`Deflate::DEFAULT`] deflates, on as many threads as the machine runs,
    /// its header the document's, opening where `/OpenAction` says, under
    /// [`Limits::DEFAULT`], read under [`read::Options::DEFAULT`], nothing to
    /// stop it.
    pub const DEFAULT: Options<'static> = Options {
        kind: XrefKind::Table,
        pack: false,
        deflate: Deflate::DEFAULT,
        threads: 0,
        version: None,
        first_page: FirstPage::OpenAction,
        limits: Limits::DEFAULT,
        read: read::Options::DEFAULT,
        stop: None,
    };
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options::DEFAULT
    }
}

/// What was written, beside the file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Linearized {
    /// The trailer's `/ID`, when it has one: the first string the encryptor's,
    /// else the document's, the second XXH3-128 of the bodies in file order, so
    /// the same document writes the same bytes. A document with no first string
    /// has none, but written under a new encryption: that has the hash for
    /// both.
    pub id: Option<[Vec<u8>; 2]>,
    /// What's around the bodies, and where everything is, counted from the
    /// file's first byte.
    pub layout: Layout,
}

/// Why a page tree can't be laid out, each page by its place in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PageTree {
    /// PDFium's rules and ISO's list other pages: `page` is the first that
    /// differs, or the first past the shorter's end.
    #[error("page {page} is another page, or none, by PDFium's rules than by ISO 32000-2's")]
    Rules {
        /// The place.
        page: usize,
    },
    /// A place in the tree with no page at it.
    #[error("page {page} is a place with no page at it")]
    NoPage {
        /// The place.
        page: usize,
    },
    /// A page written in place, which can't be numbered.
    #[error("page {page} is written in place, so it has no number")]
    Direct {
        /// The place.
        page: usize,
    },
    /// A page listed before.
    #[error("page {page} is a page listed before")]
    Repeated {
        /// The place.
        page: usize,
    },
    /// An inheritable attribute of a page PDFium's rules read otherwise than
    /// ISO's, as pushing it down would change one reader's page.
    #[error("/{attribute} of page {page} reads otherwise by PDFium's rules than by ISO 32000-2's")]
    Attribute {
        /// The place.
        page: usize,
        /// The attribute's key.
        attribute: &'static str,
    },
    /// An inheritable attribute of a page that reads otherwise going down
    /// `/Kids` from the root than up its `/Parent`s, where a page's `/Parent`
    /// isn't the node that lists it, as pushing it down would change one
    /// reader's page.
    #[error("/{attribute} of page {page} reads otherwise going down /Kids than up /Parent")]
    Kids {
        /// The place.
        page: usize,
        /// The attribute's key.
        attribute: &'static str,
    },
    /// A tree node written in place that holds an inheritable attribute, which
    /// can't be taken off it without writing what holds it anew.
    #[error("a page tree node written in place holds /{attribute}")]
    DirectNode {
        /// The attribute's key.
        attribute: &'static str,
    },
}

/// Why nothing was written.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A trailer whose `/Root` names no dictionary.
    #[error("the trailer's /Root names no catalog")]
    NoCatalog,
    /// A document or a file no password given opened, so there's nothing to
    /// write its strings and streams from: whether they're encrypted or not, as
    /// an attachment's may be where they aren't ([`Encryption::Plain`]).
    #[error("it's encrypted, and no password opened it")]
    Locked,
    /// A page tree that gives a page no place of its own.
    #[error("{0}")]
    PageTree(PageTree),
    /// The page tree didn't read.
    #[error("{0}")]
    Pages(pages::Error),
    /// The graph can't be planned.
    #[error("{0}")]
    Plan(plan::Error),
    /// The bytes around the bodies can't be made.
    #[error("{0}")]
    Layout(layout::Error<write::Error>),
    /// A body can't be written.
    #[error("{0}")]
    Write(write::Error),
    /// The file didn't read ([`linearize_bytes`]).
    #[error("{0}")]
    Read(read::Error),
}

/// `doc`, as a linearized file, at the end of `out`: every object it reaches
/// written at the number a plan gives it, copied from `from`, the reading it
/// was made of ([`Reading::to_document`]), where the rules for a copy hold
/// ([`Body::Read`](write::Body::Read)). What's written counts its positions
/// from where it starts in `out`, so `out` may hold other bytes before it.
/// `encryptor` is how it's encrypted, none for in the clear, whatever `doc`'s
/// file was (the module's "Encrypted" says what that means for its keys).
///
/// Refused, with `out` as it was given, by an [`Error`]: what the module says;
/// a table asked to pack; a trailer with no catalog, or a catalog that's a
/// page; a document past [`Options::limits`], or a file past the 4 GiB a hint
/// table can describe; an object at the number the encryption dictionary has in
/// the file an `encryptor` was read from ([`write::Error::EncryptAt`]), as a
/// whole file's write refuses; what pdf-structure's writer or the encryptor
/// refuses of a body; and a write stopped.
pub fn linearize(
    out: &mut Vec<u8>,
    doc: &pdf_structure::Document,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
) -> Result<Linearized, Error> {
    lapped(out, doc, encryptor, from, options, &mut |_| {})
}

/// [`linearize`], `lap` told as each stage ends: `graph`, `plan`, `bodies`,
/// `assemble` and `whole`, so a timing can say where it went. For a timing's
/// own build: nothing of a caller's.
#[cfg(any(test, feature = "timing"))]
#[doc(hidden)]
pub fn linearize_timed(
    out: &mut Vec<u8>,
    doc: &pdf_structure::Document,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
    lap: &mut dyn FnMut(&'static str),
) -> Result<Linearized, Error> {
    lapped(out, doc, encryptor, from, options, lap)
}

/// [`linearize`], told as each stage ends.
fn lapped(
    out: &mut Vec<u8>,
    doc: &pdf_structure::Document,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
    lap: &mut dyn FnMut(&'static str),
) -> Result<Linearized, Error> {
    let start = out.len();
    let written = file(out, start, doc, encryptor, from, options, lap);
    if written.is_err() {
        out.truncate(start);
    }
    written
}

/// `bytes`, read and written again as a linearized file: `passwords` as
/// [`Reading::open`] tries them ([`read::Options::passwords`]), in the
/// place of any `options.read` gives, on any reading that needs one, and an
/// encrypted file written encrypted as it was ([`Encryptor::of`]), so every
/// number reused under its keys where they're made per object (the module's
/// "Encrypted"). Refused as [`linearize`] refuses, and where it doesn't read
/// ([`Error::Read`]), no password opens it ([`Error::Locked`]), or its security
/// handler can't be opened ([`write::Error::Unsupported`]).
pub fn linearize_bytes(
    bytes: &[u8],
    passwords: &[Password],
    options: &Options<'_>,
) -> Result<Vec<u8>, Error> {
    let mut read = options.read;
    read.passwords = passwords;
    let reading = Reading::open(bytes, &read).map_err(Error::Read)?;
    let doc = reading.to_document().map_err(Error::Read)?;
    let encryptor = Encryptor::of(&reading).map_err(|why| match why {
        write::Error::Locked => Error::Locked,
        why => Error::Write(why),
    })?;
    let mut out = Vec::new();
    let from = from(&reading);
    linearize(&mut out, &doc, encryptor.as_ref(), Some(from), options)?;
    Ok(out)
}

/// `reading` as what a write copies from.
fn from<S: ByteSource>(reading: &Reading<S>) -> &dyn ReadFrom {
    reading
}

/// [`linearize`]'s file, `out` from `start` on: the bodies first, where they're
/// then moved up to make room for what goes around them, which they don't
/// depend on. So the file is never held twice.
fn file(
    out: &mut Vec<u8>,
    start: usize,
    doc: &pdf_structure::Document,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
    lap: &mut dyn FnMut(&'static str),
) -> Result<Linearized, Error> {
    let kind = match (options.kind, options.pack) {
        (XrefKind::Table, true) => return Err(Error::Write(write::Error::PackedTable)),
        (XrefKind::Stream, _) => Kind::Stream,
        _ => Kind::Table,
    };
    match &doc.encryption() {
        Encryption::Locked | Encryption::Plain => return Err(Error::Locked),
        Encryption::Unsupported(why) => {
            return Err(Error::Write(write::Error::Unsupported(why.clone())));
        }
        _ => {}
    }
    // As a whole file's write has it: asked to pack where an encryption
    // doesn't, it's written unpacked.
    let pack = options.pack && encryptor.is_none_or(Encryptor::packs);
    let prepared = Prepared::of(doc, encryptor, options)?;
    let writing = Writing {
        doc,
        prepared: &prepared,
        encryptor,
        from,
    };
    // An object stream too big for the reader means planning again without its
    // members, so nothing's numbered as if they were in it: each round takes
    // some out, and there's no cost to a file without one.
    let mut alone = HashSet::new();
    let mut graph = prepared.graph(kind, pack, &alone);
    lap("graph");
    let (plan, containers) = loop {
        let plan = plan(&graph, &options.limits).map_err(Error::Plan)?;
        lap("plan");
        let built = writing.under(&plan, |parts, _, _| {
            containers(&prepared, &plan, parts, options)
        })?;
        let before = alone.len();
        for (container, built) in plan.containers().iter().zip(&built) {
            if built.is_none() {
                alone.extend(container.members.iter().map(|(id, _)| *id));
            }
        }
        if alone.len() == before {
            break (plan, built);
        }
        graph = prepared.graph(kind, pack, &alone);
    };
    writing.under(&plan, |parts, now, encrypting| {
        let lengths = bodies(out, &prepared, &plan, containers, parts, now, options)?;
        lap("bodies");

        let given = doc.trailer().dict();
        let kept = now.and_then(Encryptor::first_id).map(<[u8]>::to_vec);
        let kept = kept.or_else(|| write::first_id(given));
        let id = write::file_id(kept, now.is_some(), &out[start..]);
        let to = |id: ObjectId| plan.number(id);
        let mut trailer = write::trailer_entries(given, &to);
        trailer.remove(b"Root");
        if let Some(encryptor) = now {
            match prepared.dictionary().and_then(|id| plan.number(id)) {
                Some(at) => trailer.set("Encrypt", Object::from(at)),
                None => trailer.set("Encrypt", encryptor.dictionary().clone()),
            }
        }
        if let Some(id) = &id {
            trailer.set("ID", write::id_value(id));
        }
        let trailer = written_entries(&trailer)?;
        let version = options.version.unwrap_or_else(|| {
            let header = doc.header_version().unwrap_or(Version::new(1, 7));
            match kind {
                Kind::Stream => header.max(Version::new(1, 5)),
                Kind::Table => header,
            }
        });
        let mut around = layout::Options::new((version.major, version.minor));
        around.trailer = &trailer;
        let layout = assemble(&plan, &lengths, &around, |stream| {
            encoded(stream, options.deflate, encrypting)
        })
        .map_err(Error::Layout)?;
        lap("assemble");

        laid_out(out, start, &lengths, &layout);
        lap("whole");
        Ok(Linearized { id, layout })
    })
}

/// What's the same however a document is planned.
struct Writing<'w, 'd> {
    doc: &'d pdf_structure::Document,
    prepared: &'w Prepared<'d>,
    encryptor: Option<&'w Encryptor>,
    from: Option<&'w dyn ReadFrom>,
}

impl Writing<'_, '_> {
    /// `then`, given what writes bodies at `plan`'s numbers, and the encryption
    /// in those numbers, as a whole file has it, where there's one: its
    /// dictionary at the number planned for it, and what's encrypted under each
    /// object's.
    fn under<R>(
        &self,
        plan: &Plan,
        then: impl FnOnce(&Parts<'_>, Option<&Encryptor>, Option<&Encrypting<'_>>) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let to = |id: ObjectId| plan.number(id);
        let renumbered = self.encryptor.map(|encryptor| encryptor.renumbered(&to));
        let at = self.prepared.dictionary().and_then(|id| plan.number(id));
        let whole = renumbered.as_deref().map(|encryptor| encryptor.whole(at));
        let now = whole.as_deref();
        // In the numbers objects are held at, as they're asked of it.
        let doc = self.doc;
        let trailer = doc.trailer().dict();
        let encrypting = now
            .map(|encryptor| encryptor.encrypting_renumbered(trailer, &doc, &to))
            .transpose()
            .map_err(Error::Write)?;
        let encrypting = encrypting.as_ref();
        let parts = Parts::new(encrypting, self.from, Some(&to)).map_err(Error::Write)?;
        then(&parts, now, encrypting)
    }
}

/// Each of `plan`'s object streams as written, in order, each none where it
/// doesn't fit.
fn containers(
    prepared: &Prepared<'_>,
    plan: &Plan,
    parts: &Parts<'_>,
    options: &Options<'_>,
) -> Result<Vec<Option<Vec<u8>>>, Error> {
    let groups: Vec<_> = (plan.containers().iter())
        .map(|container| {
            let members = container.members.iter().filter_map(|&(id, at)| {
                // Each member is one of the bodies the graph was given.
                prepared
                    .body(id)
                    .map(|body| (id, ObjectId::new(at, 0), body))
            });
            (container.number, members.collect())
        })
        .collect();
    (parts.containers(&groups, options.deflate, options.threads, options.stop))
        .map_err(Error::Write)
}

/// Each of `plan`'s writes, in file order, at the end of `out`: their lengths.
/// The encryption dictionary is written from `encryptor`, in the numbers
/// written, and in the clear (7.6.2).
fn bodies(
    out: &mut Vec<u8>,
    prepared: &Prepared<'_>,
    plan: &Plan,
    mut containers: Vec<Option<Vec<u8>>>,
    parts: &Parts<'_>,
    encryptor: Option<&Encryptor>,
    options: &Options<'_>,
) -> Result<Vec<u64>, Error> {
    let mut lengths = Vec::with_capacity(plan.writes().len());
    for write in plan.writes() {
        if options
            .stop
            .is_some_and(|stop| stop.load(Ordering::Relaxed))
        {
            return Err(Error::Write(write::Error::Stopped));
        }
        let at = out.len();
        match write.what {
            What::Object(id) => match encryptor.filter(|_| prepared.dictionary() == Some(id)) {
                Some(encryptor) => {
                    let dictionary = Object::from(encryptor.dictionary().clone());
                    serialize::indirect(out, ObjectId::new(write.number, 0), &dictionary)
                        .map_err(|why| Error::Write(write::Error::Serialize(why)))?;
                }
                None => {
                    if let Some(body) = prepared.body(id) {
                        (parts.body(out, id, ObjectId::new(write.number, 0), body))
                            .map_err(Error::Write)?;
                    }
                }
            },
            What::Container(container) => {
                if let Some(built) = containers[container].take() {
                    out.extend_from_slice(&built);
                }
            }
        }
        lengths.push((out.len() - at) as u64);
    }
    Ok(lengths)
}

/// `entries` as a section's trailer takes them: each key and value as written,
/// with spaces between.
fn written_entries(entries: &pdf_objects::object::Dict) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    for entry in entries.entries() {
        if !out.is_empty() {
            out.push(b' ');
        }
        serialize::name(&mut out, &entry.key);
        out.push(b' ');
        serialize::write(&mut out, &entry.value)
            .map_err(|e| Error::Write(write::Error::Serialize(e)))?;
    }
    Ok(out)
}

/// A stream of the layout's own, deflated: a cross-reference stream's rows each
/// less the row above it, as a whole file's are, and left in the clear
/// (7.5.8.2); the hint stream's data then encrypted by `encrypting`, under its
/// number, as any stream's.
fn encoded(
    stream: Stream<'_>,
    deflate: Deflate,
    encrypting: Option<&Encrypting<'_>>,
) -> Result<Encoded, write::Error> {
    let mut data = Vec::new();
    let mut entries = b"/Filter /FlateDecode".to_vec();
    match stream.columns {
        Some(columns) => {
            let layout = pdf_filters::Layout {
                columns,
                ..pdf_filters::Layout::default()
            };
            let up = Prediction::Png(layout, pdf_filters::Row::Up);
            deflate.encode(stream.data, up, &mut data)?;
            let parameters = format!(
                " /DecodeParms << /Predictor {} /Columns {columns} >>",
                up.predictor()
            );
            entries.extend_from_slice(parameters.as_bytes());
        }
        None => deflate.encode(stream.data, Prediction::None, &mut data)?,
    }
    if let (StreamKind::Hint, Some(encrypting)) = (stream.what, encrypting) {
        let id = ObjectId::new(stream.number, 0);
        let dict = pdf_objects::dict! { "Filter" => "FlateDecode" };
        data = (encrypting.stream(id, &dict, &data))
            .map_err(|why| write::Error::Encrypt { id, why })?
            .into_owned();
    }
    Ok(Encoded::new(entries, data))
}

/// `out` from `start` on, which holds the bodies, each `lengths` long, as
/// `layout` says the file is: from the back, as each body only moves up.
fn laid_out(out: &mut Vec<u8>, start: usize, lengths: &[u64], layout: &Layout) {
    let mut starts = Vec::with_capacity(lengths.len());
    let mut at = start;
    for &length in lengths {
        starts.push(at);
        at += length as usize;
    }
    let pieces: Vec<Piece<'_>> = layout.pieces().collect();
    let mut to = start + layout.len() as usize;
    out.resize(to, 0);
    for piece in pieces.iter().rev() {
        match *piece {
            Piece::Bytes(bytes) => {
                to -= bytes.len();
                out[to..to + bytes.len()].copy_from_slice(bytes);
            }
            Piece::Body(write) => {
                let length = lengths[write] as usize;
                to -= length;
                out.copy_within(starts[write]..starts[write] + length, to);
            }
        }
    }
    debug_assert_eq!(to, start);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stages are told in the order they end, what a timing goes by.
    #[test]
    fn a_timing_is_told_each_stage_in_order() {
        let mut file = b"%PDF-1.7\n".to_vec();
        let mut rows = Vec::new();
        for (number, object) in [
            (1, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (
                3,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources << >> >>",
            ),
        ] {
            rows.push(file.len());
            file.extend_from_slice(format!("{number} 0 obj\n{object}\nendobj\n").as_bytes());
        }
        let at = file.len();
        file.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        for row in rows {
            file.extend_from_slice(format!("{row:010} 00000 n \n").as_bytes());
        }
        let tail = format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n");
        file.extend_from_slice(tail.as_bytes());
        let reading = Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
        let doc = reading.to_document().unwrap();
        let mut told = Vec::new();
        let mut out = Vec::new();
        linearize_timed(
            &mut out,
            &doc,
            None,
            Some(&reading),
            &Options::DEFAULT,
            &mut |stage| {
                told.push(stage);
            },
        )
        .unwrap();
        assert_eq!(told, ["graph", "plan", "bodies", "assemble", "whole"]);
    }
}
