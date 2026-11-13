//! A [`Document`](pdf_structure::Document) written as a linearized file: the
//! graph [`plan()`] takes built from its objects, each body written through
//! pdf-structure's writer in the plan's numbers, and [`assemble`] around them.
//!
//! [`linearize`] is for a caller that holds a document, and [`linearize_bytes`]
//! for one that holds a file: a whole parse and write again, so a convenience
//! rather than a way past anything. Either writes every object at the number
//! and place a plan gives it, as Annex F has it, so anything that names one
//! from outside the file, a signature's byte range above all, doesn't hold
//! afterwards, as under any rewrite. [`check()`] says whether a file that
//! says it's linearized holds, and [`facts()`] gives what that's judged from;
//! [`check_with_reading`] and [`facts_with_reading`] do it of a file read
//! whole beside its reading.
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
//! streams are still what the file holds, and whose key there's none of; and,
//! under RC4, a string or stream that would go at a keystream the file read
//! used, unless the caller accepts what that leaks ("Encrypted", below).
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
//! A trailer isn't encrypted (7.6.2), so under an encryptor a direct `/Info`,
//! whose strings it would hold in the clear, is written as an object, encrypted
//! as any, in part 9 (F.3.10), the trailer naming it, as a whole file of
//! pdf-structure's writer writes it; it's given the graph a number past every
//! one an object has or a reference names, and the encryption dictionary's.
//! In the clear it stays in the trailer. And only strings and streams are
//! encrypted, so what a file held only inside an object stream, its
//! dictionaries' keys, names and numbers, encrypted with the stream's data, is
//! in the clear once written unpacked, as [`write::document`] writes it; the
//! catalog and the pages are never packed (F.3.1). A caller who minds packs
//! ([`Options::pack`]), under an encryption that does.
//!
//! **Under keys made per object, numbers are reused.** A section of this
//! layout lists no number free but 0 (F.3.4, F.3.11), its numbers run from 1
//! without a gap, and every object is at generation 0 (F.3.1), so an object
//! can't be moved past the keystreams the file read used, as a whole file of
//! pdf-structure's writer moves it, and what [`Encryptor::of`] says of a file's
//! free numbers can't hold here: none is listed, and an object may be written
//! at one. Objects move to numbers others had, so under RC4 a number's
//! keystream would encrypt another object's plaintext, and one who holds both
//! the file read and this one would have the XOR of the two
//! ([`Method::Rc4`](pdf_structure::pdf_crypt::Method::Rc4)). So under an
//! encryptor as given, the first string or stream that would go at a keystream
//! the file used is refused ([`Error::Write`] of [`Unencryptable::Keystream`]),
//! as pdf-structure's writer refuses it, and a file whose strings and streams
//! all go at keystreams it never used is written. A caller who accepts that
//! leak tells the encryptor to reuse them
//! ([`Encryptor::reusing_rc4_keystreams`], or [`Options::rc4`] for
//! [`linearize_bytes`]), and each object so written is listed
//! ([`Linearized::keystreams_reused`]). AES's IVs are random, each write's own
//! from the operating system, so nothing leaks that way, and nothing is refused
//! or listed. A caller who wants neither gives [`Encryptor::create`] a new
//! password, whose key is new.
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
use pdf_structure::pdf_filters::{self, Prediction};
use pdf_structure::read::{self, ByteSource, Reading};
use pdf_structure::write::{self, Deflate, Encrypting, Encryptor, Parts, ReadFrom, Unencryptable};
use pdf_structure::{Encryption, XrefKind};

use crate::Limits;
use crate::check::Attribute;
use crate::layout::{self, Encoded, Layout, Piece, Stream, StreamKind, assemble};
use crate::phases::phase;
pub use crate::plan::FirstPage;
use crate::plan::{self, Kind, Plan, What, plan};

mod facts;
pub(crate) mod graph;

pub use facts::{check, check_with_reading, facts, facts_with_reading};
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
    /// they're the same objects, and in the clear the same bytes.
    pub threads: usize,
    /// The version the header says: by default the document's, at least 1.5
    /// where a stream lists the file (7.5.8); given, it's written as given.
    pub version: Option<Version>,
    /// Which page goes first.
    pub first_page: FirstPage,
    /// The plan's caps, the most pages among them.
    pub limits: Limits,
    /// How [`linearize_bytes`] opens the file it's given: the passwords it
    /// tries ([`read::Options::passwords`], none for the empty one), the caps
    /// it reads under and the backend it inflates with. A caller given a file
    /// it doesn't trust lowers the caps here; [`linearize`] takes a document
    /// already read.
    pub read: read::Options<'a>,
    /// What [`linearize_bytes`] does with a file encrypted with RC4, which
    /// it writes under the file's own encryption: refused, by default, where
    /// a string or stream would go at a keystream the file used. [`linearize`]
    /// writes under the encryptor it's given, which its caller tells to reuse
    /// them or not ([`Encryptor::reusing_rc4_keystreams`]), so it doesn't read
    /// this.
    pub rc4: Rc4,
    /// A flag that, set while it runs, stops it: it's checked before each
    /// object and object stream, and nothing's written
    /// ([`write::Error::Stopped`]).
    pub stop: Option<&'a AtomicBool>,
}

impl Options<'static> {
    /// A file with tables, nothing in object streams, deflated as
    /// [`Deflate::DEFAULT`] deflates, on as many threads as the machine runs,
    /// its header the document's, opening where `/OpenAction` says, under
    /// [`Limits::DEFAULT`], read under [`read::Options::DEFAULT`], an RC4
    /// file refused, nothing to stop it.
    pub const DEFAULT: Options<'static> = Options {
        kind: XrefKind::Table,
        pack: false,
        deflate: Deflate::DEFAULT,
        threads: 0,
        version: None,
        first_page: FirstPage::OpenAction,
        limits: Limits::DEFAULT,
        read: read::Options::DEFAULT,
        rc4: Rc4::Refused,
        stop: None,
    };
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options::DEFAULT
    }
}

/// What [`linearize_bytes`] does with a file encrypted with RC4, under which
/// this layout puts a string or stream at a keystream the file used where an
/// object moves to a number another had (the module's "Encrypted").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Rc4 {
    /// Refused at the first string or stream that would go at a keystream
    /// the file used ([`Error::Write`] of [`Unencryptable::Keystream`]), so
    /// that nothing leaks; a file whose strings and streams all go at
    /// keystreams it never used is written.
    #[default]
    Refused,
    /// Written, each such string and stream encrypted with the keystream
    /// the file used, which leaks the XOR of the two plaintexts under it to
    /// whoever holds both files: the file's encryptor told to reuse them
    /// ([`Encryptor::reusing_rc4_keystreams`]). [`linearize`] lists each
    /// object so written ([`Linearized::keystreams_reused`]).
    ReusingKeystreams,
}

/// What was written, beside the file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Linearized {
    /// The trailer's `/ID`, when it has one: the first string the encryptor's,
    /// else the document's, the second the first 16 bytes of SHA-256 of the
    /// bodies in file order, so in the clear the same document writes the same
    /// bytes. A document with no first string has none, but written under a new
    /// encryption: that has the hash for both.
    pub id: Option<[Vec<u8>; 2]>,
    /// What's around the bodies, and where everything is, counted from the
    /// file's first byte.
    pub layout: Layout,
    /// Each object written at a keystream the file read used, by the number
    /// it's written at, ascending: its strings or its stream's data encrypted
    /// with that keystream, by RC4, as its encryptor was told to reuse them
    /// ([`Encryptor::reusing_rc4_keystreams`]); an object stream among them,
    /// as it's encrypted whole. Each leaks the XOR of its plaintexts and what
    /// the file held under that keystream, as that says. Where the file held
    /// the same plaintexts there, or ones they start, nothing new leaks, but
    /// the object is listed all the same, as what the file held isn't read.
    /// None in the clear, under AES and under a new encryption.
    pub keystreams_reused: Vec<ObjectId>,
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
        page: u64,
    },
    /// A place in the tree with no page at it.
    #[error("page {page} is a place with no page at it")]
    NoPage {
        /// The place.
        page: u64,
    },
    /// A page written in place, which can't be numbered.
    #[error("page {page} is written in place, so it has no number")]
    Direct {
        /// The place.
        page: u64,
    },
    /// A page listed before.
    #[error("page {page} is a page listed before")]
    Repeated {
        /// The place.
        page: u64,
    },
    /// An inheritable attribute of a page PDFium's rules read otherwise than
    /// ISO's, as pushing it down would change one reader's page.
    #[error("{attribute} of page {page} reads otherwise by PDFium's rules than by ISO 32000-2's")]
    Attribute {
        /// The place.
        page: u64,
        /// The attribute.
        attribute: Attribute,
    },
    /// An inheritable attribute of a page that reads otherwise going down
    /// `/Kids` from the root than up its `/Parent`s, where a page's `/Parent`
    /// isn't the node that lists it, as pushing it down would change one
    /// reader's page.
    #[error("{attribute} of page {page} reads otherwise going down /Kids than up /Parent")]
    Kids {
        /// The place.
        page: u64,
        /// The attribute.
        attribute: Attribute,
    },
    /// A tree node written in place that holds an inheritable attribute, which
    /// can't be taken off it without writing what holds it anew.
    #[error("a page tree node written in place holds {attribute}")]
    DirectNode {
        /// The attribute.
        attribute: Attribute,
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
/// file was, and it's written under it as given (the module's "Encrypted" says
/// what that means for its keys): under RC4, what goes at a keystream the
/// file it was read from used is refused, unless it's told to reuse them,
/// and then listed ([`Linearized::keystreams_reused`]).
///
/// # Errors
///
/// Refused, with `out` as it was given, by an [`Error`]: what the module says,
/// a page tree that gives a page no place of its own ([`Error::PageTree`]) or
/// doesn't read, past [`Limits::pages`](crate::Limits::pages) among it
/// ([`Error::Pages`]), and a document no password opened
/// ([`Error::Locked`]); a trailer with no catalog ([`Error::NoCatalog`]), or a
/// catalog that's a page, or a document past [`Options::limits`]
/// ([`Error::Plan`]); a file past the 4 GiB a hint table can describe
/// ([`Error::Layout`]); and, as [`Error::Write`], a table asked to pack
/// ([`write::Error::PackedTable`]), an object at the number the encryption
/// dictionary has in the file an `encryptor` was read from
/// ([`write::Error::EncryptAt`]), as a whole file's write refuses, what
/// pdf-structure's writer or the encryptor refuses of a body or of the hint
/// stream's data, a string or stream at a keystream used among it
/// ([`Unencryptable::Keystream`]), and a write stopped
/// ([`write::Error::Stopped`]).
pub fn linearize(
    out: &mut Vec<u8>,
    doc: &pdf_structure::Document,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
) -> Result<Linearized, Error> {
    phase!(DEBUG, "linearize");
    let start = out.len();
    let written = file(out, start, doc, encryptor, from, options);
    if written.is_err() {
        out.truncate(start);
    }
    written
}

/// `bytes`, read and written again as a linearized file: opened as
/// [`Options::read`] says, by its passwords ([`read::Options::passwords`]),
/// and an encrypted file written encrypted as it was ([`Encryptor::of`]).
/// Under RC4, that's refused where a string or stream would go at a
/// keystream the file used (the module's "Encrypted"), unless
/// [`Options::rc4`] says to reuse them, and then each such object leaks, as
/// [`Rc4::ReusingKeystreams`] says: [`linearize`], under the file's
/// encryptor told to reuse them, lists which.
///
/// A file of one page, written with cross-reference and object streams, and
/// checked as it reads back:
///
/// ```
/// use pdf_linearize::Limits;
/// use pdf_linearize::check::Finding;
/// use pdf_linearize::family::{Options, check, linearize_bytes};
/// use pdf_linearize::pdf_structure::{Reading, XrefKind, read};
///
/// let mut file = b"%PDF-1.7\n".to_vec();
/// let mut rows = Vec::new();
/// for (number, object) in [
///     (1, "<< /Type /Catalog /Pages 2 0 R >>"),
///     (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 \
///          /MediaBox [0 0 612 792] /Resources << >> >>"),
///     (3, "<< /Type /Page /Parent 2 0 R >>"),
/// ] {
///     rows.push(file.len());
///     let object = format!("{number} 0 obj\n{object}\nendobj\n");
///     file.extend_from_slice(object.as_bytes());
/// }
/// let at = file.len();
/// file.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
/// for row in rows {
///     file.extend_from_slice(format!("{row:010} 00000 n \n").as_bytes());
/// }
/// let trailer = "trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n";
/// file.extend_from_slice(format!("{trailer}{at}\n%%EOF\n").as_bytes());
///
/// let mut options = Options::DEFAULT;
/// options.kind = XrefKind::Stream;
/// options.pack = true;
/// let linearized = linearize_bytes(&file, &options).unwrap();
/// assert!(linearized.windows(11).any(|w| w == b"/Linearized"));
///
/// // What's written holds, and nothing else is to be said of it.
/// let opened = Reading::open(&linearized[..], &read::Options::DEFAULT);
/// let reading = opened.unwrap();
/// let mut findings: Vec<Finding> = Vec::new();
/// let verdict = check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
/// assert!(verdict.linearized && verdict.holds && verdict.hints_read);
/// assert_eq!(findings, []);
/// ```
///
/// # Errors
///
/// Refused as [`linearize`] refuses, and where it doesn't read
/// ([`Error::Read`]), no password opens it ([`Error::Locked`]), or its
/// security handler can't be opened ([`Error::Write`] of
/// [`write::Error::Unsupported`]).
pub fn linearize_bytes(bytes: &[u8], options: &Options<'_>) -> Result<Vec<u8>, Error> {
    let reading = Reading::open(bytes, &options.read).map_err(Error::Read)?;
    let doc = reading.to_document().map_err(Error::Read)?;
    let encryptor = Encryptor::of(&reading).map_err(|why| match why {
        write::Error::Locked => Error::Locked,
        why => Error::Write(why),
    })?;
    let encryptor = match options.rc4 {
        Rc4::Refused => encryptor,
        Rc4::ReusingKeystreams => encryptor.map(Encryptor::reusing_rc4_keystreams),
    };
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
/// depend on. So the file is never held twice. Each stage is a span of its
/// own, with the `tracing` feature: the graph, made again for each plan after
/// the first; each plan; each round's object streams; the bodies; the bytes
/// around them, the trailer's `/ID` and entries among them; and the file laid
/// out whole.
fn file(
    out: &mut Vec<u8>,
    start: usize,
    doc: &pdf_structure::Document,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
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
    // An object stream too big for the reader means planning again without its
    // members, so nothing's numbered as if they were in it: each round takes
    // some out, and there's no cost to a file without one.
    let mut alone = HashSet::new();
    let (prepared, mut graph) = {
        phase!(DEBUG, "graph");
        let prepared = Prepared::of(doc, encryptor, options)?;
        let graph = prepared.graph(kind, pack, &alone);
        (prepared, graph)
    };
    let writing = Writing {
        doc,
        prepared: &prepared,
        encryptor,
        from,
    };
    let (plan, containers, mut reused) = loop {
        let plan = {
            phase!(DEBUG, "plan");
            plan(&graph, &options.limits).map_err(Error::Plan)?
        };
        let (built, reused) = {
            phase!(DEBUG, "containers", containers = plan.containers().len());
            writing.containers(&plan, &alone, options)?
        };
        let before = alone.len();
        alone.extend(unfitted(&plan, &built));
        if alone.len() == before {
            break (plan, built, reused);
        }
        graph = {
            phase!(DEBUG, "graph");
            prepared.graph(kind, pack, &alone)
        };
    };
    let (done, written) = writing.under(&plan, encryptor, |parts, encrypting| {
        let lengths = {
            phase!(DEBUG, "bodies", writes = plan.writes().len());
            bodies(
                out, &prepared, &plan, containers, parts, encrypting, options,
            )?
        };
        let (id, layout) = {
            phase!(DEBUG, "assemble");
            let given = doc.trailer().dict();
            let kept = encrypting
                .and_then(Encrypting::first_id)
                .map(<[u8]>::to_vec);
            let kept = kept.or_else(|| write::first_id(given));
            let id = write::file_id(kept, encrypting.is_some(), &out[start..]);
            let to = |id: ObjectId| plan.number(id);
            let mut trailer = write::trailer_entries(given, &to);
            trailer.remove(b"Root");
            // A direct /Info written as an object, at its number: never left
            // in the clear, as every body is numbered.
            if let Some(info) = prepared.info() {
                match plan.number(info) {
                    Some(at) => trailer.set("Info", Object::from(at)),
                    None => {
                        trailer.remove(b"Info");
                    }
                }
            }
            if let Some(encrypting) = encrypting {
                match encrypting.at() {
                    Some(at) => trailer.set("Encrypt", Object::from(at)),
                    None => trailer.set("Encrypt", encrypting.dictionary().clone()),
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
            let mut around = layout::Options::new(version);
            around.trailer = &trailer;
            let layout = assemble(&plan, &lengths, &around, |stream| {
                encoded(stream, options.deflate, encrypting)
            })
            .map_err(|why| match why {
                // What the encryptor refuses of the hint stream's data, as of
                // a body's.
                layout::Error::Encode(why @ write::Error::Encrypt { .. }) => Error::Write(why),
                why => Error::Layout(why),
            })?;
            (id, layout)
        };
        {
            phase!(DEBUG, "whole", bytes = layout.len());
            laid_out(out, start, &lengths, &layout);
        }
        Ok((id, layout))
    })?;
    let (id, layout) = done;
    // The object streams' and the rest's, each written under an encryption of
    // its own: no number is in both.
    reused.extend(written);
    reused.sort_unstable();
    reused.dedup();
    Ok(Linearized {
        id,
        layout,
        keystreams_reused: reused,
    })
}

/// Whether `why` is RC4's refusal of a string or stream at a keystream the
/// file used.
fn at_a_keystream_used(why: &Error) -> bool {
    let keystream = Unencryptable::Keystream;
    matches!(why, Error::Write(write::Error::Encrypt { why, .. }) if *why == keystream)
}

/// The members of `plan`'s object streams of which `built` has none, as they
/// don't fit.
fn unfitted<'p>(
    plan: &'p Plan,
    built: &'p [Option<Vec<u8>>],
) -> impl Iterator<Item = ObjectId> + 'p {
    let containers = plan.containers().iter().zip(built);
    let left = containers.filter(|(_, built)| built.is_none());
    left.flat_map(|(container, _)| container.members.iter().map(|(id, _)| *id))
}

/// A plan's object streams, each as written, in order, or none where it
/// doesn't fit.
type Built = Vec<Option<Vec<u8>>>;

/// What's the same however a document is planned.
struct Writing<'w, 'd> {
    doc: &'d pdf_structure::Document,
    prepared: &'w Prepared<'d>,
    encryptor: Option<&'w Encryptor>,
    from: Option<&'w dyn ReadFrom>,
}

impl Writing<'_, '_> {
    /// `then`, given what writes bodies at `plan`'s numbers, and the encryption
    /// in those numbers, as a whole file has it, where there's one
    /// ([`Encryptor::laid_out`]): its dictionary at the number planned for it,
    /// and what's encrypted under each object's. With what `then` gives, each
    /// object it wrote at a keystream the file `encryptor` was read from used,
    /// as `encryptor` is told to reuse them, ascending.
    fn under<R>(
        &self,
        plan: &Plan,
        encryptor: Option<&Encryptor>,
        then: impl FnOnce(&Parts<'_>, Option<&Encrypting<'_>>) -> Result<R, Error>,
    ) -> Result<(R, Vec<ObjectId>), Error> {
        let to = |id: ObjectId| plan.number(id);
        let at = self.prepared.dictionary().and_then(|id| plan.number(id));
        // In the numbers objects are held at, as they're asked of it.
        let doc = self.doc;
        let trailer = doc.trailer().dict();
        let encrypting = encryptor
            .map(|encryptor| encryptor.laid_out(Some(&to), at, trailer, &doc))
            .transpose()
            .map_err(Error::Write)?;
        let parts = match &encrypting {
            Some(encrypting) => Parts::new(encrypting, self.from),
            None => Parts::plain(self.from, Some(&to)),
        };
        let parts = parts.map_err(Error::Write)?;
        let done = then(&parts, encrypting.as_ref())?;
        let reused = encrypting.as_ref().map(Encrypting::keystreams_reused);
        Ok((done, reused.unwrap_or_default()))
    }

    /// Each of `plan`'s object streams as written, in order, each none where
    /// it doesn't fit, under the caller's encryptor, with what [`under`]
    /// gives beside. Under RC4 one at a keystream the file used is refused,
    /// unless the encryptor is told to reuse them; but a plan whose object
    /// streams don't all fit is made again without the members of those that
    /// don't, and its own aren't written: so a refusal is given only where
    /// none of the members that don't fit, as a write that refuses nothing
    /// tells, is left to plan again, `alone` holding those already.
    ///
    /// [`under`]: Writing::under
    fn containers(
        &self,
        plan: &Plan,
        alone: &HashSet<ObjectId>,
        options: &Options<'_>,
    ) -> Result<(Built, Vec<ObjectId>), Error> {
        let built = |encryptor: Option<&Encryptor>| {
            self.under(plan, encryptor, |parts, _| {
                containers(self.prepared, plan, parts, options)
            })
        };
        match built(self.encryptor) {
            Err(refused) if at_a_keystream_used(&refused) => {
                let reusing = (self.encryptor).map(|e| e.clone().reusing_rc4_keystreams());
                let (tried, _) = built(reusing.as_ref())?;
                let again = unfitted(plan, &tried).any(|id| !alone.contains(&id));
                match again {
                    true => Ok((tried, Vec::new())),
                    false => Err(refused),
                }
            }
            built => built,
        }
    }
}

/// Each of `plan`'s object streams as written, in order, each none where it
/// doesn't fit.
fn containers(
    prepared: &Prepared<'_>,
    plan: &Plan,
    parts: &Parts<'_>,
    options: &Options<'_>,
) -> Result<Built, Error> {
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
/// The encryption dictionary is written as `encrypting` has it, in the numbers
/// written, and in the clear (7.6.2).
fn bodies(
    out: &mut Vec<u8>,
    prepared: &Prepared<'_>,
    plan: &Plan,
    mut containers: Built,
    parts: &Parts<'_>,
    encrypting: Option<&Encrypting<'_>>,
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
            What::Object(id) => match encrypting.filter(|_| prepared.dictionary() == Some(id)) {
                Some(encrypting) => {
                    let dictionary = Object::from(encrypting.dictionary().clone());
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
