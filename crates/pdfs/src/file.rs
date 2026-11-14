//! A file's recipes, each a function over pdf-structure's reading, so that
//! a caller with a reading of its own, read under its own options, can call
//! it: what version the file is, what it says of itself and claims, and
//! whether its linearization holds. A [`Pdf`](crate::Pdf) calls each with its
//! file's defaults.

use pdf_linearize::check::{self, Verdict};
use pdf_objects::fault::Sink;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::version::Version;
use pdf_objects::{Unreadable, Why};
use pdf_semantics::info::{self, Info};
use pdf_semantics::xmp::{self, Claims};
use pdf_semantics::{Fault, catalog};
use pdf_structure::read::{ByteSource, Reading};

use crate::error::Error;
use crate::limits::Limits;
use crate::phases::phase;

/// The version of the file `reading` reads: the later, by number, of its
/// header's and its catalog's `/Version`, a name, which a file updated to a
/// later version than its header says has (ISO 32000-2, 7.7.2); the header's
/// where the catalog has none that reads as one, or there's no catalog, or
/// it's damaged; none where neither says one. So `/1.10` is later than
/// `1.9`.
///
/// ```
/// use pdfs::pdf_objects::version::Version;
/// use pdfs::pdf_structure::read::{Options, Reading};
///
/// // A 1.9 file updated to 1.10, which its catalog says.
/// let file = b"%PDF-1.9\n1 0 obj\n<< /Type /Catalog /Version /1.10 >>\nendobj\n\
///     xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
///     trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n60\n%%EOF\n";
/// let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
/// assert!(reading.faults().is_empty());
/// assert_eq!(pdfs::file::version(&reading), Ok(Some(Version::new(1, 10))));
/// ```
///
/// # Errors
///
/// The [`Unreadable`] of a catalog that doesn't read for the reader's own
/// reason, past a cap it reads under, behind a password, or whose bytes the
/// source didn't give: never read as no `/Version`.
pub fn version<S: ByteSource>(reading: &Reading<S>) -> Result<Option<Version>, Unreadable> {
    let header = reading.header_version();
    let root = match catalog(reading, reading.trailer().dict()) {
        Ok(root) => root,
        Err(unreadable) if unreadable.why == Why::Damaged => None,
        Err(unreadable) => return Err(unreadable),
    };
    let named = (root.and_then(|root| root.get(b"Version")))
        .and_then(|version| version.as_name())
        .and_then(Version::parse);
    Ok(header.max(named))
}

/// What the file `reading` reads says of itself: its trailer's `/Info`, the
/// document information dictionary (ISO 32000-2, 14.3.3), read typed, its
/// text strings decoded by `rules` (pdf-semantics' [`info::read`]); none
/// where the trailer has none, or it isn't a dictionary. What departs from
/// the clause is told to `sink`.
///
/// ```
/// use pdfs::pdf_objects::dict;
/// use pdfs::prelude::*;
///
/// // A file of one page whose trailer names an author.
/// let mut doc = Document::new();
/// doc.add_page(Rect::LETTER);
/// let info = doc.structure_mut().add(dict! { "Author" => Object::text("Ada") });
/// doc.structure_mut().trailer_mut().dict_mut().set("Info", info);
/// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
///
/// // Over the reading, its text decoded by ISO's rules, every fault told.
/// let mut faults = Vec::new();
/// let info = pdfs::file::info(pdf.reading(), Rules::Iso, &mut faults).unwrap().unwrap();
/// assert_eq!(info.author.as_deref(), Some("Ada"));
/// assert!(faults.is_empty());
/// ```
///
/// # Errors
///
/// As [`info::read`] fails: the [`Unreadable`] of an object on the way that
/// doesn't read for the reader's own reason, and one of
/// [`Why::Locked`] for a string a file no password opened holds as
/// ciphertext.
pub fn info<S: ByteSource, K: Sink<Fault> + ?Sized>(
    reading: &Reading<S>,
    rules: Rules,
    sink: &mut K,
) -> Result<Option<Info>, Unreadable> {
    phase!(DEBUG, "info");
    match reading.trailer().dict().get(b"Info") {
        Some(dict) => info::read(reading, dict, rules, sink),
        None => Ok(None),
    }
}

/// What the file `reading` reads claims to conform to: its catalog's
/// metadata stream's packet, read under `limits` (pdf-semantics'
/// [`xmp::read_from`]); none where the catalog has no metadata stream.
/// What departs from XMP is told to `sink`. A packet that isn't UTF-8 is
/// told and has nothing read, so with `()` as the sink it reads as one that
/// claims nothing.
///
/// ```
/// use pdfs::prelude::*;
///
/// let mut claims = Claims::default();
/// claims.pdfvt_version = Some("PDF/VT-2".into());
/// let mut doc = Document::new();
/// doc.add_page(Rect::LETTER);
/// doc.claim(&claims).unwrap();
/// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
///
/// let semantics = pdfs::Limits::DEFAULT.semantics;
/// let read = pdfs::file::claims(pdf.reading(), &semantics, &mut ()).unwrap();
/// assert_eq!(read, Some(claims));
/// ```
///
/// # Errors
///
/// [`Error::NoCatalog`] where there's no catalog, [`Error::Unreadable`]
/// where it doesn't read, and [`Error::Xmp`] where the packet isn't read
/// whole, or at all: past `limits.xmp_bytes`, or its stream doesn't read for
/// the reader's own reason, among the rest.
pub fn claims<S: ByteSource, K: Sink<Fault> + ?Sized>(
    reading: &Reading<S>,
    limits: &pdf_semantics::Limits,
    sink: &mut K,
) -> Result<Option<Claims>, Error> {
    phase!(DEBUG, "claims");
    let root = catalog(reading, reading.trailer().dict())?.ok_or(Error::NoCatalog)?;
    Ok(xmp::read_from(reading, root, limits, sink)?)
}

/// Whether the file `reading` reads is linearized and its linearization
/// holds (pdf-linearize's [`check`](pdf_linearize::family::check())), under
/// `limits.linearize` with the walks of its page tree and name trees under
/// `limits.semantics`' caps on them, its pages under the lesser of the two
/// crates' ([`Limits::linearize`]): a whole read only where the file says
/// it's linearized. What's wrong with it is told to `sink`, and what the
/// reader didn't read, for a cap, a password or filters it doesn't take, is
/// said as not judged ([`Verdict::judged_whole`]), never as the file's.
///
/// ```
/// use pdfs::prelude::*;
///
/// // A file of two pages, laid out for reading over a network.
/// let mut doc = Document::new();
/// doc.add_page(Rect::LETTER);
/// doc.add_page(Rect([0.0, 0.0, 595.0, 842.0]));
/// let mut file = Vec::new();
/// let options = doc.linearize_options();
/// doc.save_linearized_to(&mut file, &options).unwrap();
/// let pdf = pdfs::from_bytes(file).unwrap();
///
/// // Over the reading, every finding told: none.
/// let mut findings = Vec::new();
/// let verdict = pdfs::file::linearized(pdf.reading(), pdf.limits(), &mut findings).unwrap();
/// assert!(verdict.linearized && verdict.holds && verdict.judged_whole);
/// assert!(findings.is_empty());
/// ```
///
/// # Errors
///
/// Only where the source doesn't give its bytes
/// ([`family::Error::Read`](pdf_linearize::family::Error::Read)).
pub fn linearized<S: ByteSource, K: check::Sink + ?Sized>(
    reading: &Reading<S>,
    limits: &Limits,
    sink: &mut K,
) -> Result<Verdict, pdf_linearize::family::Error> {
    phase!(DEBUG, "linearized", bytes = reading.size());
    pdf_linearize::family::check(reading, &limits.linearizing(), sink)
}
