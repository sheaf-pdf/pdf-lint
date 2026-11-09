//! Encryption as the writer writes it: how a file is encrypted, a
//! reading's or a new one's ([`Encryptor`]), and what encrypts one file's
//! objects as pdf-objects' writer writes them ([`Encrypting`]), by the list
//! of what's left in the clear that the reader decrypts by.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::convert::Infallible;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};

use pdf_crypt::{Fault, Handler, Ivs, KeyCheck, Method, Protection, Unusable};
use pdf_objects::Resolve;
use pdf_objects::object::{Dict, Object, ObjectId, Str, Value};
use pdf_objects::serialize::{self, Encrypt, Renumber};

use super::{Error, Numbering, ReadFrom};
use crate::crypt::{self, List, Stream};
use crate::filter;
use crate::read::{ByteSource, Faults, Options, Reading, SourceId, XrefEntry};
use crate::source::Encryption;

/// How a written file is encrypted: a security handler and its key, a
/// reading's or a new file's; its encryption dictionary as it's written,
/// and where the file it was read from had it; the `/ID[0]` the key is
/// made from; what it leaves in the clear, the list the reader decrypts by
/// (see [`read`](crate::read), "What's encrypted"); and what every
/// cross-reference section of the file it was read from lists, each
/// number's generations used, which keystreams RC4 has encrypted with
/// there and which generation each number is to be used at next. It gives
/// out nothing of the key, its `Debug` neither; a clone clones the
/// handler, each key wiped when it's dropped (pdf-crypt's `Handler`).
///
/// **Under RC4** (pdf-crypt's `Method::Rc4`, a method of a reading's), each
/// string and stream of an object is encrypted from the start of one
/// keystream, made of the file's key and the object's number and generation
/// (ISO 32000-2, 7.6.3, Algorithm 1), so two plaintexts under one keystream
/// leak their XOR to whoever holds both. The writer puts no new plaintext
/// under a keystream the file it was read from shows used: a whole file
/// ([`full`](super::full), [`document`](super::document)) writes each
/// object it writes anew at a generation its number hasn't used, or at a
/// number the file never listed where its generations are spent, and says
/// which it moved ([`Written::regenerated`](super::Written::regenerated));
/// an update ([`update`](super::update)) refuses an object whose strings or
/// data would be new under a keystream used, its own number's among them
/// ([`Unencryptable::Keystream`], [`Error::Free`]), unless it's told to
/// reuse them ([`Encryptor::reusing_rc4_keystreams`]). What the rule can't
/// see: two whole files written from one reading put what each writes anew
/// at the same fresh generations, so where the two differ and both are
/// kept, their XOR leaks; a caller saving twice opens the file it wrote
/// before writing again, or writes under [`Encryptor::create`].
///
/// A caller that lays a file out itself, as [`full`](super::full) does,
/// writes it encrypted through [`Encryptor::laid_out`], given where each
/// object and the dictionary go: what that gives encrypts each object
/// under the number it's written at, says what to write of the dictionary
/// ([`Encrypting::dictionary`], [`Encrypting::at`], [`Encrypting::first_id`]),
/// and is what [`Parts::new`](super::Parts::new) writes the bodies by, in the
/// same numbers. Such a caller puts in no object stream an object
/// [`Encrypting::dictionary_object`] says is an encryption dictionary's, and
/// numbers no object but that one there, as what's there is left in the clear.
/// Where the dictionary goes, which numbers are listed free, and which
/// generations objects are written at stay its own: a whole file of the
/// writer's lists each number the file it was read from lists and it doesn't
/// write free at the generation that number is to be used at next, and under
/// RC4 writes no object anew at a keystream that file used. Under RC4, what it
/// lays out at a keystream that file's sections show used is refused
/// ([`Unencryptable::Keystream`]), unless it's told to reuse them.
///
/// A caller a layout binds otherwise can't keep those rules: a linearized
/// file's sections list none free but 0, and its numbers run from 1 without
/// a gap (ISO 32000-2, F.3.1, F.3.4, F.3.11). It writes objects at those
/// numbers and lists none. Moving every object, under RC4 it encrypts each
/// with another object's keystream, as pdf-crypt's `Method::Rc4` says, so
/// it writes under the encryptor told to reuse them, says so to its own
/// callers, and points them to [`Encryptor::create`], a new key.
#[derive(Clone)]
pub struct Encryptor {
    handler: Handler,
    list: List,
    dict: Dict,
    place: Place,
    first_id: Option<Vec<u8>>,
    /// The numbers the file it was read from lists free in its newest
    /// entries, each with the generation its entry gives, where a method
    /// of its is RC4: an update's object at one, below that generation,
    /// would be encrypted with a deleted object's keystream, whose
    /// ciphertext the file may not hold to compare ([`Error::Free`]).
    /// Empty otherwise.
    free: BTreeMap<u32, u16>,
    /// Whether a method of its is RC4 (its strings', its streams', a crypt
    /// filter's its dictionary has, or `/EFF`'s), whose keystreams it puts
    /// no new plaintext under where the file shows them used.
    rc4: bool,
    /// What every section of the file it was read from lists: each
    /// number's generations used, and past every number. Shared by its
    /// clones, as it's the file's, whichever numbers it's made for.
    used: Arc<Used>,
    /// Whether an update under it writes an object whose strings or data
    /// are new under an RC4 keystream the file used, or at a number its file
    /// lists free below that generation, rather than refuse it, saying which
    /// ([`Encryptor::reusing_rc4_keystreams`]).
    reusing: bool,
    /// Whether a method of its makes keys per object (RC4, AES-128), from
    /// the object's number and generation: then what's encrypted under it
    /// is right at its own number alone. AES-256's key is the file's
    /// (Algorithm 1.A), its IV beside its ciphertext.
    per_object: bool,
    /// The reading it was made of, whose objects it encrypts as the
    /// reading's file has them: none for a new file's.
    source: Option<SourceId>,
    /// What it left in the clear in the numbers it was read in, where
    /// [`Encryptor::renumbered`] or [`whole`] has made `list` another
    /// file's, so that what's written under it says which of a reading's
    /// objects are encrypted as they're read.
    read_list: Option<Box<List>>,
    /// What makes each write's IVs: pdf-crypt's `Ivs::new`, the operating
    /// system's, but where a test sets another (`with_test_ivs`, under
    /// `insecure-test-writer` alone).
    ivs: fn() -> Ivs,
}

/// What a file's cross-reference sections show of its numbers: each
/// number's generations whose RC4 keystream, made of the file's key and an
/// object's number and generation (ISO 32000-2, 7.6.3, Algorithm 1), they
/// show used, and the generation each is to be used at next (7.5.4). Every
/// section counts, an earlier revision's as well as the newest entries: an
/// object an update wrote over, or a number it freed, may have its
/// ciphertext still in the file.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Used {
    /// Each number the sections list but 0, with past every generation
    /// whose keystream they show used (an object's in use at an offset, its
    /// own and those before it, or those below a free entry's generation),
    /// and the generation it's to be used at next (an object's in use, at an
    /// offset or in an object stream, plus one; a free entry's own). Up to
    /// 65,536, one past the last generation, where it's spent.
    numbers: BTreeMap<u32, (u32, u32)>,
    /// Past every number the file lists, in use or free, its trailers'
    /// `/Size` and its cross-reference streams' numbers: an object the
    /// writer numbers itself at or past it is at no number that file used,
    /// so under no key or keystream of it. 1 for a new file's.
    past: u32,
    /// Whether the sections read are every section of the file: not where
    /// the reading is as of a section ([`Options::section`]) and the file's
    /// sections from its last `startxref` don't read as far as that one,
    /// nor where it's a reading of a file's first bytes
    /// ([`Reading::prefix`]), whose later sections it doesn't read. Where
    /// they aren't, any keystream may be one a later section used.
    whole: bool,
}

impl Used {
    /// A new file's: nothing used.
    fn new() -> Used {
        Used {
            numbers: BTreeMap::new(),
            past: 1,
            whole: true,
        }
    }

    /// What `reading`'s file's sections show: its own sections' rows and
    /// its entries, a scan's among them; and, for a reading as of a section,
    /// the rows and entries of its source read whole, from its last
    /// `startxref`, which may list what later revisions wrote.
    fn of<S: ByteSource>(reading: &Reading<S>) -> Used {
        let mut used = Used {
            numbers: BTreeMap::new(),
            past: 1,
            whole: !reading.part(),
        };
        let read = used.take(reading);
        used.whole &= read.is_some();
        if let Some(at) = reading.as_of() {
            let options = Options {
                limits: *reading.limits(),
                passwords: &[],
                faults: Faults::Off,
                ..Options::DEFAULT
            };
            let reached = match Reading::open(reading.source(), &options) {
                Ok(whole) => {
                    let sections = used.take(&whole);
                    whole.trailer().unwritten.is_none()
                        && sections.is_some_and(|ats| ats.contains(&at))
                }
                Err(_) => false,
            };
            used.whole &= reached;
        }
        used
    }

    /// `reading`'s rows and entries, taken: where each of its sections was
    /// read, and where it was said to be where it was read elsewhere; none
    /// where its source didn't give their bytes.
    fn take<S: ByteSource>(&mut self, reading: &Reading<S>) -> Option<Vec<usize>> {
        for (n, generation, entry) in reading.entries() {
            self.row(n, generation, entry);
        }
        let size = (reading.trailer().dict().get(b"Size"))
            .and_then(Object::as_integer)
            .and_then(|size| u32::try_from(size).ok());
        let streams = (reading.facts().max_stream).map(|n| n.saturating_add(1));
        self.past = [size, streams]
            .into_iter()
            .flatten()
            .fold(self.past, u32::max);
        let sections = reading.sections().ok()?;
        let mut ats = Vec::with_capacity(sections.sections.len());
        for section in &sections.sections {
            ats.extend([Some(section.at), section.said].into_iter().flatten());
            let hybrid = section.hybrid.iter().flat_map(|h| &h.rows);
            for row in section.rows.iter().chain(hybrid) {
                if let Some(entry) = row.entry {
                    self.row(row.number, row.generation, entry);
                }
            }
        }
        Some(ats)
    }

    /// Number `n`'s row, of `generation`, saying `entry`.
    fn row(&mut self, n: u32, generation: u16, entry: XrefEntry) {
        self.past = self.past.max(n.saturating_add(1));
        if n == 0 {
            return;
        }
        let g = u32::from(generation);
        let (keyed, next) = match entry {
            XrefEntry::InUse { .. } => (g + 1, g + 1),
            XrefEntry::Compressed { .. } => (0, 1),
            _ => (g, g),
        };
        let had = self.numbers.entry(n).or_insert((0, 0));
        *had = (had.0.max(keyed), had.1.max(next));
    }

    /// Whether RC4's keystream for an object at `at` is one the sections
    /// show used.
    pub(super) fn keyed(&self, at: ObjectId) -> bool {
        (self.numbers.get(&at.number)).is_some_and(|&(keyed, _)| u32::from(at.generation) < keyed)
    }

    /// The generation number `n` is to be used at next, where the sections
    /// list it: 65,536 where it's spent.
    fn next(&self, n: u32) -> Option<u32> {
        self.numbers.get(&n).map(|&(_, next)| next)
    }
}

/// Where an encryption dictionary was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    /// At its number, in the file it was read from.
    At(ObjectId),
    /// In that file's trailer.
    Direct,
    /// Nowhere: it's a new file's.
    New,
}

impl fmt::Debug for Encryptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encryptor")
            .field("handler", &self.handler)
            .field("place", &self.place)
            .finish_non_exhaustive()
    }
}

impl Encryptor {
    /// How `reading`'s file is encrypted, to write it encrypted as it is:
    /// its handler, as the password the reading was given opened it; its
    /// `/Encrypt` dictionary as the file stores it, its strings as written,
    /// at its number or in the trailer; its first `/ID` element, as the
    /// handler opened by it (empty where the trailer has none); and what it
    /// leaves in the clear, as the reading decrypts it. None for a file that
    /// isn't encrypted.
    ///
    /// It reads every cross-reference section of the reading's file, not
    /// its newest entries alone, for each number's generations used: a
    /// whole file written under it lists each number those list and it
    /// doesn't write free at the generation that number is to be used at
    /// next (ISO 32000-2, 7.5.4), so that a tool that follows its free list
    /// doesn't take one at a deleted object's key; and where any of its
    /// methods is RC4 (its strings', its streams', a crypt filter's its
    /// dictionary has, or `/EFF`'s), they're the keystreams it puts no new
    /// plaintext under (see [`Encryptor`], "Under RC4"). For a reading as
    /// of a section ([`Options::section`](crate::Options::section)), they're
    /// its file's sections read whole too, from the last `startxref`, as a
    /// later revision's may have used a keystream; where those don't read
    /// as far as its section, or for a reading of a file's first bytes
    /// ([`Reading::prefix`]), any keystream may have been, and under RC4
    /// every object written anew is refused, as an update's is, unless it's
    /// told to reuse them.
    ///
    /// # Errors
    ///
    /// Refused for a file no password opened ([`Error::Locked`], whether its
    /// strings and streams are encrypted or not, [`Encryption::Plain`]), and
    /// for one whose security handler the reader can't open
    /// ([`Error::Unsupported`]): there's nothing to encrypt what's written
    /// by.
    pub fn of<S: ByteSource>(reading: &Reading<S>) -> Result<Option<Encryptor>, Error> {
        match (reading.encryption(), reading.handler(), reading.stored()) {
            (Encryption::None, ..) => Ok(None),
            (Encryption::Unsupported(why), ..) => Err(Error::Unsupported(why.clone())),
            (Encryption::Opened { .. }, Some(handler), Some(stored)) => {
                let per_object = per_object(handler, &stored.dict);
                let rc4 = rc4(handler, &stored.dict);
                let free = match rc4 {
                    true => (reading.entries())
                        .filter(|&(n, _, entry)| n != 0 && entry == XrefEntry::Free)
                        .map(|(n, generation, _)| (n, generation))
                        .collect(),
                    false => BTreeMap::new(),
                };
                Ok(Some(Encryptor {
                    handler: handler.clone(),
                    list: reading.list().clone(),
                    dict: stored.dict.clone(),
                    place: stored.at.map_or(Place::Direct, Place::At),
                    first_id: Some(stored.id.clone()),
                    free,
                    rc4,
                    used: Arc::new(Used::of(reading)),
                    reusing: false,
                    per_object,
                    source: Some(reading.id()),
                    read_list: None,
                    ivs: Ivs::new,
                }))
            }
            _ => Err(Error::Locked),
        }
    }

    /// A new file's encryption, of revision 6 (AES-256), protected as
    /// `protection` says (its passwords, prepared, its permissions, and
    /// whether the document's metadata is encrypted), as pdf-crypt's
    /// `Handler::create` makes it, its key and salts from the operating
    /// system; its dictionary with its crypt filter's `/Length 32` (bytes,
    /// as qpdf writes it) and `/AuthEvent /DocOpen`. Its key isn't made from
    /// the file's `/ID` ([`Encryptor::first_id`] is none).
    ///
    /// # Errors
    ///
    /// Refused, as `Handler::create` refuses: for a password that isn't
    /// prepared, or is empty or too long; for no password; for permissions
    /// withheld where the owner's password is the user's; and where the
    /// operating system's random number generator fails
    /// (`Unusable::Random`).
    pub fn create(protection: &Protection) -> Result<Encryptor, Unusable> {
        Handler::create(protection).map(Encryptor::created)
    }

    /// [`Encryptor::create`], its key and salts drawn from `rng` in
    /// pdf-crypt's order, as the test writer's `create` draws them: for a
    /// test that pins a new file's bytes, never a document's protection.
    ///
    /// # Errors
    ///
    /// Refused as [`Encryptor::create`] is.
    #[cfg(any(test, feature = "insecure-test-writer"))]
    #[doc(hidden)]
    pub fn create_with(
        protection: &Protection,
        rng: &mut (impl pdf_crypt::rand_core::CryptoRng + ?Sized),
    ) -> Result<Encryptor, Unusable> {
        pdf_crypt::writer::create(protection, rng).map(Encryptor::created)
    }

    /// It, each write's IVs made by `ivs` in place of the operating
    /// system's: pdf-crypt's `Ivs::fixed` or `Ivs::derived`, for a test that
    /// writes the same file to the same bytes, never a document's
    /// protection. Every write under it and its clones takes them, whole,
    /// as an update, or laid out by another crate.
    #[cfg(any(test, feature = "insecure-test-writer"))]
    #[doc(hidden)]
    #[must_use]
    pub fn with_test_ivs(mut self, ivs: fn() -> Ivs) -> Encryptor {
        self.ivs = ivs;
        self
    }

    /// The encryptor of a new file whose handler and dictionary's values
    /// pdf-crypt made.
    fn created((handler, values): (Handler, pdf_crypt::Dictionary)) -> Encryptor {
        let mut list = List {
            encrypts: true,
            ..List::default()
        };
        list.settle(&values, Ok(&handler));
        Encryptor {
            handler,
            list,
            dict: encryption(&values, Some(32)),
            place: Place::New,
            first_id: None,
            free: BTreeMap::new(),
            rc4: false,
            used: Arc::new(Used::new()),
            reusing: false,
            per_object: false,
            source: None,
            read_list: None,
            ivs: Ivs::new,
        }
    }

    /// It, an update under it writing an object whose strings or data are
    /// new under an RC4 keystream the file used rather than refusing it
    /// ([`Unencryptable::Keystream`]), and one at a number the file lists
    /// free below the generation its entry gives ([`Error::Free`]); a
    /// whole file under it, of a reading whose file's later sections it
    /// can't read ([`Encryptor::of`]), every object it writes anew; and a
    /// file a caller lays out under it ([`Encryptor::laid_out`]), each
    /// string and stream's data at a keystream the file used. What's
    /// written lists each so ([`Written::keystreams_reused`],
    /// [`Encrypting::keystreams_reused`]).
    ///
    /// What that leaks, to whoever holds the file (an update's file holds
    /// the versions before it too) or the file before it: for each object
    /// listed, the XOR of each of its strings and its stream's data with
    /// what the file encrypted before under the same keystream, which,
    /// where one of the two is known or guessed, is the other. A caller
    /// that would leak nothing writes the file whole instead, under a new
    /// encryption ([`Encryptor::create`]), or under this one, whose whole
    /// files write each object anew at a fresh generation. Under AES it
    /// changes nothing: its IVs are random.
    ///
    /// [`Written::keystreams_reused`]: super::Written::keystreams_reused
    #[must_use]
    pub fn reusing_rc4_keystreams(mut self) -> Encryptor {
        self.reusing = true;
        self
    }

    /// Whether objects may go in object streams under it: a container is
    /// encrypted as a stream, whole, its members' strings with it, so not
    /// where streams are left in the clear (`/StmF /Identity`) and strings
    /// aren't, whose strings would then be in the clear; nor where either's
    /// method isn't one.
    #[must_use]
    pub fn packs(&self) -> bool {
        crypt::packs(self.handler.streams(), self.handler.strings())
    }

    /// The encryption dictionary, as the file it was read from stores it,
    /// its strings as written; or a new file's.
    #[must_use]
    pub const fn dictionary(&self) -> &Dict {
        &self.dict
    }

    /// Where the file it was read from has the dictionary: its number.
    /// None where it's in that file's trailer, and for a new file's, which
    /// has no number yet.
    #[must_use]
    pub const fn at(&self) -> Option<ObjectId> {
        match self.place {
            Place::At(id) => Some(id),
            Place::Direct | Place::New => None,
        }
    }

    /// Whether the file it was read from has the dictionary in its
    /// trailer, direct, as a whole file and an update written under it
    /// keep it, where one at a number is written as an object.
    #[must_use]
    pub fn direct(&self) -> bool {
        self.place == Place::Direct
    }

    /// The first `/ID` element the key is made from: the file's it was read
    /// from, as its handler opened by it, empty where that file's trailer
    /// has none. None for a new file's, whose key isn't made from it.
    #[must_use]
    pub fn first_id(&self) -> Option<&[u8]> {
        self.first_id.as_deref()
    }

    /// Where the file it was read from has its dictionary.
    pub(super) fn place(&self) -> Place {
        self.place
    }

    /// It in the numbers a file laid out by `to` writes, made whole with its
    /// dictionary at `at`, a number written, or direct where that's none
    /// ([`Encryptor::renumbered`], then [`whole`]): what that file's
    /// objects are encrypted by, as [`Encryptor::laid_out`] and
    /// [`full`](super::full) encrypt them.
    pub(super) fn laid(&self, to: Numbering<'_>, at: Option<ObjectId>) -> Cow<'_, Encryptor> {
        let renumbered = match to.0 {
            Some(to) => self.renumbered(to),
            None => Cow::Borrowed(self),
        };
        whole(renumbered, at)
    }

    /// Where the file a renumbering `to` writes has its dictionary, as
    /// [`Encryptor::renumbered`] places it: at the number `to` gives its
    /// own, nowhere yet where `to` gives none, as a new file's; direct or
    /// new as it is.
    pub(super) fn placed(&self, to: Numbering<'_>) -> Place {
        match self.place {
            Place::At(at) => to.at(at).map_or(Place::New, Place::At),
            place => place,
        }
    }

    /// It, for a file whose objects are written at the numbers `to` gives
    /// them ([`Options::renumber`](super::Options::renumber)), in those
    /// numbers: what it leaves in the clear by number, each encryption
    /// dictionary and what one refers to, at the number `to` gives it, one
    /// it gives none left out, as it isn't written; where its dictionary
    /// is, at the number `to` gives it, or, where it gives none, nowhere
    /// yet, as a new file's is, a whole file then writing it past every
    /// object; and its dictionary's references as `to` names them, null
    /// where it names none. What its file's sections list stays as it is:
    /// those numbers are the file written's too, whose keys and keystreams
    /// are that file's. Where `to` moves none of those, it's itself.
    ///
    /// A whole file written under a renumbering is encrypted through this,
    /// then [`whole`]; and so is an update after it, given
    /// objects at the numbers written ([`Written::base`](super::Written::base)),
    /// whose place and dictionary are then the file's, as an update's
    /// encryptor must have ([`Error::Unmatched`]), where the whole write
    /// put the dictionary where `to` puts its number: not where `to` names
    /// it none, or [`Options::encrypt_at`](super::Options::encrypt_at)
    /// moved it. An object's strings and data are encrypted under the number
    /// it's written at, the document's metadata stream found from the
    /// objects given and kept as its number written. An update's own
    /// renumbering doesn't go through this: the update doesn't write the
    /// dictionary again, so it encrypts by the list as it is, in its file's
    /// numbers, which its renumbering moves nothing of
    /// ([`Error::Moved`]).
    pub(super) fn renumbered<N: Renumber + ?Sized>(&self, to: &N) -> Cow<'_, Encryptor> {
        let list = &self.list;
        let numbers = list.dictionaries.iter().chain(&list.sections);
        let place = match self.place {
            Place::At(at) => to.renumber(at).map_or(Place::New, Place::At),
            place => place,
        };
        let mut dict = Object::from(self.dict.clone());
        let named = dict.renumber(|id| to.renumber(id));
        if !named
            && place == self.place
            && numbers.into_iter().all(|&id| to.renumber(id) == Some(id))
        {
            return Cow::Borrowed(self);
        }
        let mut renumbered = self.clone();
        renumbered.keep_read_list();
        let translated =
            |ids: &HashSet<ObjectId>| ids.iter().filter_map(|&id| to.renumber(id)).collect();
        renumbered.list.dictionaries = translated(&list.dictionaries);
        renumbered.list.sections = translated(&list.sections);
        renumbered.place = place;
        if named {
            renumbered.dict = match dict.into_value() {
                Value::Dict(dict) => dict,
                _ => unreachable!("a dictionary renumbered is one"),
            };
        }
        Cow::Owned(renumbered)
    }

    /// The reading it was made of ([`Encryptor::of`]), whose objects a
    /// write under it may copy as their bytes: none for a new file's.
    pub(super) fn read_in(&self) -> Option<SourceId> {
        self.source
    }

    /// `list`, kept as what it was read in, before it's made another file's.
    fn keep_read_list(&mut self) {
        if self.read_list.is_none() {
            self.read_list = Some(Box::new(self.list.clone()));
        }
    }

    /// What it leaves in the clear in the numbers it was read in, whichever
    /// file it's since been made for.
    fn as_read(&self) -> &List {
        self.read_list.as_deref().unwrap_or(&self.list)
    }

    /// Whether any method of its makes keys per object (RC4, AES-128: its
    /// strings', its streams', a crypt filter's its dictionary has, or
    /// `/EFF`'s), so that what it encrypted is right at its own number
    /// alone, which a copy at another number isn't.
    pub(super) fn per_object(&self) -> bool {
        self.per_object
    }

    /// Where the file it was read from lists number `n` free in its newest
    /// entries, under RC4: the generation its entry gives, the one the
    /// number is to be used at next. An update writes nothing there below
    /// it, as it would be encrypted with a deleted object's keystream, whose
    /// ciphertext the file may not hold to compare ([`Error::Free`]), unless
    /// it's told to reuse keystreams. None for any other number, and every
    /// number under a key no method of which is RC4.
    pub(crate) fn free(&self, n: u32) -> Option<u16> {
        self.free.get(&n).copied()
    }

    /// Each number [`Encryptor::free`] gives a generation for, with it,
    /// ascending.
    #[cfg(any(test, fuzzing))]
    pub(crate) fn free_numbers(
        &self,
    ) -> impl std::iter::FusedIterator<Item = (u32, u16)> + Clone + '_ {
        self.free.iter().map(|(&n, &generation)| (n, generation))
    }

    /// Whether an object at `at`, in an update under it, is refused as at a
    /// number its file lists free ([`Error::Free`]): under RC4, below the
    /// generation the free entry gives, but where it's told to reuse
    /// keystreams.
    pub(super) fn free_below(&self, at: ObjectId) -> bool {
        self.free(at.number).is_some_and(|g| at.generation < g)
    }

    /// Whether a method of its is RC4.
    pub(crate) fn rc4(&self) -> bool {
        self.rc4
    }

    /// Whether it's told to reuse RC4 keystreams
    /// ([`Encryptor::reusing_rc4_keystreams`]).
    pub(super) fn reusing(&self) -> bool {
        self.reusing
    }

    /// What every section of the file it was read from lists.
    pub(super) fn used(&self) -> &Used {
        &self.used
    }

    /// Whether every section of the file it was read from read.
    pub(super) fn sections_whole(&self) -> bool {
        self.used.whole
    }

    /// Each number the file it was read from lists but 0, with the
    /// generation it's to be used at next, up to 65,535, a free entry's
    /// last: what a whole file written under it lists free where it writes
    /// nothing there, as its key, or its keystream, is that file's. None
    /// for a new file's.
    pub(super) fn listed(&self) -> impl Iterator<Item = (u32, u16)> + '_ {
        (self.used.numbers.iter())
            .map(|(&n, &(_, next))| (n, u16::try_from(next).unwrap_or(u16::MAX)))
    }

    /// Where a whole file under it writes an object it writes anew at `at`,
    /// a number and generation it's given or a renumbering gives it, under
    /// RC4: at `at`, where RC4's keystream there is unused; else at the
    /// generation its number is to be used at next, or none where that's
    /// spent (65,535 or past), as no generation of it is left unused. Under
    /// another method, at `at`.
    pub(super) fn regenerated(&self, at: ObjectId) -> Option<ObjectId> {
        if !self.rc4 || !self.used.keyed(at) {
            return Some(at);
        }
        let next = self.used.next(at.number)?;
        let generation = u16::try_from(next).ok().filter(|&g| g < u16::MAX)?;
        Some(ObjectId::new(at.number, generation))
    }

    /// Whether it's a new file's that encrypts the document's metadata, which
    /// is then written without its own `/Crypt` filter that would leave it in
    /// the clear ([`Encrypting::anew`]): for the fuzz body, which reads
    /// back what's written.
    #[cfg(any(test, fuzzing))]
    pub(crate) fn writes_metadata_anew(&self) -> bool {
        self.place == Place::New && !self.list.unencrypted_metadata
    }

    /// Past every number the file it was read from lists ([`Encryptor::of`]):
    /// where the writer numbers an object of its own that's under no key or
    /// keystream that file used.
    pub(super) fn past(&self) -> u32 {
        self.used.past
    }

    /// Its key's check (pdf-crypt's `Handler::key_check`), which an update
    /// after a file written under it is matched by.
    pub(super) fn key_check(&self) -> KeyCheck {
        self.handler.key_check()
    }

    /// Whether its key is the one `check` was made of, compared in constant
    /// time (pdf-crypt's `Handler::has_key`).
    pub(super) fn has_key(&self, check: &KeyCheck) -> bool {
        self.handler.has_key(check)
    }

    /// Whether object `id` is an encryption dictionary, or what one's
    /// entries refer to, by number, which a reader reads before it has a
    /// key: left in the clear, and never put in an object stream, which is
    /// encrypted. [`packable`](super::packable) tells an encryption
    /// dictionary by its shape alone.
    #[must_use]
    pub fn dictionary_object(&self, id: ObjectId) -> bool {
        self.list.dictionaries.contains(&id)
    }

    /// For an update under the renumbering `to`, whose objects are each
    /// given at, and written at, as `written` pairs them: the least of the
    /// objects [`Encryptor::dictionary_object`] says are an encryption
    /// dictionary's, by number in the file, that `to` moves, or names none,
    /// or that `written` puts another object's number on. None where
    /// there's none, and with no renumbering. A whole file writes the
    /// dictionary again, in the numbers written ([`Encryptor::laid`]); an
    /// update doesn't, so the file names those where they are, and its
    /// reader reads what's there in the clear.
    pub(super) fn moved(
        &self,
        to: Numbering<'_>,
        written: impl IntoIterator<Item = (ObjectId, ObjectId)>,
    ) -> Option<ObjectId> {
        to.0?;
        let dictionaries = &self.list.dictionaries;
        let moved = (dictionaries.iter().copied()).filter(|&id| to.at(id) != Some(id));
        let mut numbers: BTreeMap<u32, ObjectId> = BTreeMap::new();
        for &id in dictionaries {
            let least = numbers.entry(id.number).or_insert(id);
            *least = id.min(*least);
        }
        let onto = (written.into_iter())
            .filter(|(given, at)| given != at)
            .filter_map(|(_, at)| numbers.get(&at.number).copied());
        moved.chain(onto).min()
    }

    /// For a whole file under the renumbering `to`, itself the encryptor as
    /// given and `now` it in the numbers written ([`Encryptor::laid`]), whose
    /// objects are each given at, and written at, as `written` pairs them: the
    /// least of the objects [`Encryptor::dictionary_object`] says are an
    /// encryption dictionary's, by number in the file, at whose number written
    /// `written` puts an object that isn't one of those. `now` leaves what's
    /// there in the clear, as the reader of the file written reads it, which
    /// that object's strings would then be. None where there's none, and with
    /// no renumbering, which puts no object at another's number.
    pub(super) fn shared(
        &self,
        now: &Encryptor,
        to: Numbering<'_>,
        written: impl IntoIterator<Item = (ObjectId, ObjectId)>,
    ) -> Option<ObjectId> {
        to.0?;
        let (then, now) = (&self.list.dictionaries, &now.list.dictionaries);
        let shared = (written.into_iter())
            .filter(|(given, at)| now.contains(at) && !then.contains(given))
            .map(|(_, at)| at);
        // Each named by the least of those `to` puts there; as written,
        // where none is, which `now`, being them in the numbers written,
        // doesn't leave.
        let named = |at: ObjectId| {
            let there = then.iter().copied().filter(|&id| to.at(id) == Some(at));
            there.min().unwrap_or(at)
        };
        shared.map(named).min()
    }

    /// The numbers [`Encryptor::dictionary_object`] says are an encryption
    /// dictionary's, or what one refers to, in the numbers the renumbering
    /// `to` writes, as [`Encryptor::renumbered`] has them: what's at them is
    /// left in the clear, so the writer numbers no object of its own there.
    pub(super) fn dictionary_numbers(&self, to: Numbering<'_>) -> impl Iterator<Item = u32> {
        (self.list.dictionaries.iter()).filter_map(move |&id| to.at(id).map(|id| id.number))
    }

    /// Whether `/EncryptMetadata false` leaves the document's metadata
    /// stream's data in the clear under it, which the catalog's
    /// `/Metadata` says.
    pub(super) fn plain_metadata(&self) -> bool {
        self.list.unencrypted_metadata
    }

    /// What encrypts the objects of a file a caller lays out itself, as
    /// [`full`](super::full) lays out a whole one: pdf-objects' encryptor,
    /// for its `indirect_with` and `indirect_stream_with`, and what
    /// [`Parts::new`](super::Parts::new) writes the file's bodies by. It's
    /// itself in the numbers the file is written in: each object at the
    /// number the renumbering `to` gives it (none for each at the number
    /// it's given at), what it leaves in the clear by number, an encryption
    /// dictionary and what one refers to, at the number `to` gives each,
    /// and its dictionary's references as `to` names them, `null` where it
    /// names none; and made whole, every section of the file naming the one
    /// dictionary at `at`, a number written, or the one in the trailer
    /// where that's none, so that an object at a number the file it was
    /// read from named `/Encrypt` by, in its trailer or an earlier
    /// section's, is encrypted as any is, as a reader of the file written
    /// decrypts it; what the dictionary refers to is still left in the
    /// clear.
    ///
    /// `trailer` and `resolve` are in the numbers the objects are held at,
    /// as the dictionary of a stream pdf-objects' writer asks it to encrypt
    /// is, its references not renumbered (`serialize::Encrypt`): `trailer`
    /// the file's, whose catalog's `/Metadata` says which stream is the
    /// document's metadata, found through `resolve` and kept as the number
    /// `to` writes it at; and `resolve` what gives the objects a reference
    /// names, through which a stream's `/Filter` and `/DecodeParms` say
    /// whether it has a crypt filter of its own.
    ///
    /// What's left to the caller: the dictionary written at `at` as
    /// [`Encrypting::dictionary`] has it, or in the trailer, `at` none only
    /// where the file it was read from had it there ([`Encryptor::direct`]); no
    /// object stream holding, and no other object at the number of, what
    /// [`Encrypting::dictionary_object`] says is left in the clear; a direct
    /// `/Info`, whose strings the trailer would hold in the clear
    /// ([`trailer_entries`](super::trailer_entries) keeps it as it is), written
    /// as an object, as a whole file of the writer's writes it; the document's
    /// metadata stream written through [`Parts`](super::Parts), which takes its
    /// own `/Identity` out where the encryption encrypts metadata, as it's
    /// refused otherwise ([`Unencryptable::Metadata`]); which numbers it lists
    /// free, which a whole file of the writer's lists at the generation each is
    /// to be used at next where the file it was read from lists it and the
    /// write doesn't; and, under RC4, which generations objects go at, which a
    /// whole file of the writer's moves past every one the file it was read
    /// from used. What's laid out here is encrypted by its number and
    /// generation as given, so under RC4 a string or stream's data at a
    /// keystream the file's sections show used is refused
    /// ([`Unencryptable::Keystream`]), as nothing here reads the versions there
    /// to tell it's unchanged, but where the encryptor is told to reuse them,
    /// which writes and lists it ([`Encrypting::keystreams_reused`]; see
    /// [`Encryptor`], "Under RC4").
    ///
    /// # Errors
    ///
    /// Refused where `/EncryptMetadata false` leaves the document's metadata
    /// stream in the clear and `resolve` doesn't give the catalog, which says
    /// which stream that is ([`Error::Catalog`]), rather than encrypt it.
    pub fn laid_out<'a>(
        &'a self,
        to: Option<&'a (dyn Renumber + Sync + 'a)>,
        at: Option<ObjectId>,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
    ) -> Result<Encrypting<'a>, Error> {
        let to = Numbering(to);
        let encrypting = Encrypting::of(self.laid(to, at), trailer, resolve, to, at)?;
        Ok(encrypting.guarded(Guard::whole(self, None)))
    }

    /// What encrypts the objects of a file written under it as it is, each
    /// at the number it's given at ([`Encrypting::of`]).
    #[cfg(test)]
    pub(super) fn encrypting<'a>(
        &'a self,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
    ) -> Result<Encrypting<'a>, Error> {
        self.encrypting_at(trailer, resolve, Numbering::NONE)
    }

    /// What encrypts an update's objects under it as it is, in its file's
    /// numbers, each written at the number `to` gives it, the document's
    /// metadata stream found in the numbers given and kept as its number
    /// written ([`Encrypting::of`]): an update doesn't write the dictionary
    /// again, so it encrypts by the list as it is.
    pub(super) fn encrypting_at<'a>(
        &'a self,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
        to: Numbering<'a>,
    ) -> Result<Encrypting<'a>, Error> {
        Encrypting::of(Cow::Borrowed(self), trailer, resolve, to, self.at())
    }
}

/// `encryptor`, for a file written whole that writes its dictionary at `at`,
/// or direct where that's none, every section of it naming that one: an
/// object at a number the file it was read from named `/Encrypt` by, in its
/// trailer or an earlier section's, is encrypted as any is, as a reader of
/// the file written decrypts it, where it would otherwise be left as
/// written; what the dictionary refers to is still left in the clear. A
/// file written whole is encrypted through this.
fn whole(encryptor: Cow<'_, Encryptor>, at: Option<ObjectId>) -> Cow<'_, Encryptor> {
    let list = &encryptor.list;
    let named: Vec<ObjectId> = (list.dictionaries.iter().copied())
        .filter(|id| list.sections.contains(id) && Some(*id) != at)
        .collect();
    if named.is_empty() {
        return encryptor;
    }
    let mut whole = encryptor.into_owned();
    whole.keep_read_list();
    whole.list.dictionaries.retain(|id| !named.contains(id));
    whole.list.sections = at.into_iter().collect();
    Cow::Owned(whole)
}

/// What encrypts the objects of one file written under an [`Encryptor`],
/// in the numbers they're written at ([`Encryptor::laid_out`]), as
/// pdf-objects' writer writes them: its `serialize::Encrypt`, which
/// every string and stream's data of an indirect object written by
/// `indirect_with` or `indirect_stream_with` passes through, encrypted by
/// the handler with the object's key, AES's IV the next of the write's
/// own (pdf-crypt's `Ivs`, made with it and dropped with it): 16 bytes from
/// the operating system, so no IV serves two plaintexts, and two saves of
/// one document differ in every AES ciphertext written anew. The threads of
/// one write share them. What it leaves in the clear is the list
/// the reader decrypts by (see [`read`](crate::read), "What's
/// encrypted"): an encryption dictionary and what its entries refer to, a
/// cross-reference stream, a signature's `/Contents`, a stream whose crypt
/// filter is `/Identity`, and the document's metadata stream's data under
/// `/EncryptMetadata false`. It gives out nothing of the key: it encrypts
/// as the reading that opened the file decrypts. What a layout writes of
/// the encryption is its to say: the dictionary, where it goes, and the
/// first `/ID` element.
pub struct Encrypting<'a> {
    /// The encryptor in the numbers written.
    encryptor: Cow<'a, Encryptor>,
    resolve: &'a (dyn Resolve<'a> + Sync),
    /// The document's metadata stream, where `/EncryptMetadata false`
    /// leaves its data in the clear, at the number it's written at.
    metadata: Option<ObjectId>,
    /// The write's IVs, which its every AES string and stream takes.
    ivs: Ivs,
    /// The document's metadata stream, at the number it's written at,
    /// where a new encryption encrypts it: written without its own `/Crypt`
    /// filter that would leave it in the clear ([`Encrypting::anew`]).
    metadata_anew: Option<ObjectId>,
    /// Where each object it encrypts is written: the renumbering
    /// [`Parts::new`](super::Parts::new) writes by.
    to: Numbering<'a>,
    /// Where the file written has the dictionary, none for its trailer.
    at: Option<ObjectId>,
    /// Under RC4, what keeps the write from putting a new plaintext under
    /// a keystream its file used: a whole file's, an update's, or a
    /// caller's layout's, which reads no versions.
    guard: Option<Guard<'a>>,
}

/// What keeps a write under RC4 from putting a new plaintext under a
/// keystream its file used (see [`Encryptor`], "Under RC4"): each string
/// and stream's data RC4 encrypts at a number and generation whose
/// keystream the file shows used is written only where it's the same as,
/// or the start of, a plaintext a version of that object the file holds
/// has under it, which puts nothing new in the file; else it's refused
/// ([`Unencryptable::Keystream`]), or, told to reuse keystreams, written
/// and listed.
pub(super) struct Guard<'a> {
    /// What every section of the encryptor's file lists.
    used: Arc<Used>,
    /// The numbers at which every keystream counts as used: those a write
    /// after the encryptor's reading may have used, from past what its file
    /// lists to past what the file now does; every number, where that file
    /// isn't known to be the file written to, or not every section of it
    /// read. None for a whole file of a file whose every section read.
    beyond: Range<u32>,
    /// Below those, each number an update after the encryptor's reading
    /// wrote at, with the highest generation it wrote: each keystream there
    /// and below counts as used too, as the reading's sections don't show
    /// it ([`Base`](super::Base)'s, as [`Written::base`](super::Written::base)
    /// gives it back).
    wrote: BTreeMap<u32, u16>,
    /// The reading the versions are read from, where it reads the file
    /// written to, or the one a whole file is written of.
    from: Option<&'a dyn ReadFrom>,
    /// The encryptor's key's check, which a reading's handler must have for
    /// its plaintexts to be under the same keystreams.
    key: KeyCheck,
    /// Whether the versions are read in each earlier revision too, as an
    /// update's are, or in the reading alone, as a whole file's, which
    /// writes anew there only what's as read.
    earlier: bool,
    /// The plaintexts under each keystream asked of, as read.
    versions: Mutex<HashMap<ObjectId, Arc<[Vec<u8>]>>>,
    /// Whether what it would refuse is written, and listed.
    reusing: bool,
    /// What it wrote that it would otherwise have refused.
    reused: Mutex<BTreeSet<ObjectId>>,
}

impl<'a> Guard<'a> {
    /// A whole file's under `encryptor`, as given, written of `from`: every
    /// object written anew is at a keystream unused, but where every section
    /// of its file didn't read, so this guards what's written anew at its
    /// own number and generation, as read, and every keystream there.
    pub(super) fn whole(encryptor: &Encryptor, from: Option<&'a dyn ReadFrom>) -> Guard<'a> {
        let beyond = match encryptor.sections_whole() {
            true => 0..0,
            false => 0..u32::MAX,
        };
        let from = from.filter(|from| Some(from.id()) == encryptor.read_in());
        Guard::new(encryptor, beyond, BTreeMap::new(), from, false)
    }

    /// An update's under `encryptor`, after `base`: a file the reading
    /// `base.source` was made of, or written after one, which now uses the
    /// numbers below `base.size`, and at those the reading's file listed
    /// what the updates since wrote (`base.wrote`); none where it's of no
    /// reading. The versions are read from `from` where that's the reading
    /// `base.source` is, in every revision of it.
    pub(super) fn update(
        encryptor: &Encryptor,
        base: &super::Base,
        from: Option<&'a dyn ReadFrom>,
    ) -> Guard<'a> {
        let source = base.source;
        let own = source.is_some() && source == encryptor.read_in();
        let beyond = match (encryptor.sections_whole(), own) {
            (false, _) => 0..u32::MAX,
            (true, true) => encryptor.past()..base.size,
            (true, false) => 0..base.size,
        };
        let wrote = match own {
            true => base.wrote.clone(),
            false => BTreeMap::new(),
        };
        let from = from.filter(|from| source == Some(from.id()));
        Guard::new(encryptor, beyond, wrote, from, true)
    }

    fn new(
        encryptor: &Encryptor,
        beyond: Range<u32>,
        wrote: BTreeMap<u32, u16>,
        from: Option<&'a dyn ReadFrom>,
        earlier: bool,
    ) -> Guard<'a> {
        Guard {
            used: Arc::clone(&encryptor.used),
            beyond,
            wrote,
            from,
            key: encryptor.key_check(),
            earlier,
            versions: Mutex::new(HashMap::new()),
            reusing: encryptor.reusing,
            reused: Mutex::new(BTreeSet::new()),
        }
    }

    /// Whether RC4's keystream at `at` is one the file written to may have
    /// encrypted another plaintext with.
    pub(super) fn used(&self, at: ObjectId) -> bool {
        self.beyond.contains(&at.number)
            || self.used.keyed(at)
            || (self.wrote.get(&at.number)).is_some_and(|&g| at.generation <= g)
    }

    /// The versions of each of `ids` read at once, for an update, which
    /// asks of each object it writes at a keystream used.
    pub(super) fn load(&self, ids: &[ObjectId]) {
        let Some(from) = self.from else {
            return;
        };
        let read = from.versions(ids, self.earlier, &self.key);
        let mut versions = self.versions.lock().unwrap_or_else(PoisonError::into_inner);
        for (id, plaintexts) in read {
            versions.insert(id, plaintexts.into());
        }
    }

    /// Nothing, where RC4 may encrypt `plaintext` of the object at `at`:
    /// empty, or at a keystream unused, or the same as, or the start of, a
    /// plaintext a version there has; or where it's told to reuse
    /// keystreams, which lists it. Refused otherwise.
    fn check(&self, at: ObjectId, plaintext: &[u8]) -> Result<(), Unencryptable> {
        if plaintext.is_empty() || !self.used(at) || self.held(at, plaintext) {
            return Ok(());
        }
        if !self.reusing {
            return Err(Unencryptable::Keystream);
        }
        let mut reused = self.reused.lock().unwrap_or_else(PoisonError::into_inner);
        reused.insert(at);
        Ok(())
    }

    /// Whether a version of the object at `at` the file holds has a
    /// plaintext under its keystream that `plaintext` is, or starts.
    fn held(&self, at: ObjectId, plaintext: &[u8]) -> bool {
        let Some(from) = self.from else {
            return false;
        };
        let versions = {
            let mut versions = self.versions.lock().unwrap_or_else(PoisonError::into_inner);
            let read = versions.entry(at).or_insert_with(|| {
                let mut read = from.versions(&[at], self.earlier, &self.key);
                read.remove(&at).unwrap_or_default().into()
            });
            Arc::clone(read)
        };
        versions
            .iter()
            .any(|version| version.starts_with(plaintext))
    }

    /// What it wrote that it would otherwise have refused, ascending.
    fn reused(&self) -> Vec<ObjectId> {
        let reused = self.reused.lock().unwrap_or_else(PoisonError::into_inner);
        reused.iter().copied().collect()
    }
}

impl fmt::Debug for Encrypting<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encrypting")
            .field("encryptor", &self.encryptor)
            .field("metadata", &self.metadata)
            .field("metadata_anew", &self.metadata_anew)
            .field("ivs", &self.ivs)
            .field("to", &self.to.0.map(|_| "a caller's"))
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

/// Why a string or a stream's data of a file written encrypted isn't
/// encrypted, which the writer refuses ([`Error::Encrypt`]), writing
/// nothing in the clear.
/// Not exhaustive, as reasons are found as the writer learns.
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Unencryptable {
    /// pdf-crypt's fault: a crypt filter's method that isn't one, or one
    /// the dictionary hasn't, or AES that the file's key isn't for; or the
    /// operating system's random number generator failing, so AES had no
    /// IV.
    #[error("{0}")]
    Fault(Fault),
    /// It's a stream an encryption dictionary refers to, whose strings are
    /// left as written: its data is neither said to be plaintext nor to be
    /// encrypted.
    #[error(
        "it's a stream an encryption dictionary refers to, whose data isn't said to be encrypted or not"
    )]
    Dictionary,
    /// It's an embedded file's data, without a crypt filter of its own, and
    /// the encryption dictionary's `/EFF` isn't a name, so doesn't say how
    /// embedded files are encrypted.
    #[error(
        "it's an embedded file's data, and the encryption dictionary's /EFF, which says how that's encrypted, isn't a name"
    )]
    Unsaid,
    /// A string or a stream's data RC4 would encrypt with a keystream the
    /// file already encrypted another plaintext with, which it isn't the
    /// same as, nor the start of, in any version the file holds: the file
    /// would hold the XOR of the two, which leaks each where the other is
    /// known (see [`Encryptor`], "Under RC4"). The file written whole under
    /// a new encryption ([`Encryptor::create`]) leaks nothing; under the
    /// file's own, [`Encryptor::reusing_rc4_keystreams`] writes it, saying
    /// so.
    #[error(
        "RC4 would encrypt it with a keystream the file already encrypted something else with, which leaks the two's XOR: written whole under a new encryption (Encryptor::create), it wouldn't"
    )]
    Keystream,
    /// It's the document's metadata stream, under a new encryption that
    /// encrypts metadata, and its own `/Crypt` filter (`/Identity`) would leave
    /// its data in the clear: written through [`Parts`](super::Parts), as every
    /// write of the writer's is, that filter is taken out and the stream
    /// encrypted, as its `/EncryptMetadata` says (ISO 32000-2, 7.6.4.2).
    #[error(
        "it's the document's metadata stream, whose own /Identity crypt filter would leave it in the clear under an encryption that encrypts metadata: written through Parts, which takes that filter out, it's encrypted"
    )]
    Metadata,
}

impl<'a> Encrypting<'a> {
    /// What encrypts the objects of a file written under `encryptor`, which
    /// is in the numbers written, each object written at the number `to`
    /// gives it, the dictionary at `at`: `trailer` and `resolve`, in the
    /// numbers the objects are held at, say which stream is the document's
    /// metadata, kept as the number `to` writes it at, and a stream's own
    /// crypt filter ([`Encryptor::laid_out`]). Under a new encryption that
    /// encrypts metadata, the metadata stream is kept so too, as it's
    /// written without its own `/Crypt` that would leave it in the clear
    /// ([`Encrypting::anew`]). Refused where `/EncryptMetadata false` leaves
    /// the document's metadata stream in the clear and `resolve` doesn't
    /// give the catalog ([`Error::Catalog`]).
    pub(super) fn of(
        encryptor: Cow<'a, Encryptor>,
        trailer: &Dict,
        resolve: &'a (dyn Resolve<'a> + Sync),
        to: Numbering<'a>,
        at: Option<ObjectId>,
    ) -> Result<Encrypting<'a>, Error> {
        let found = || {
            let get: crypt::Get<'_, '_, Infallible> = &|id| Ok(resolve.resolve(id));
            let Ok(metadata) = crypt::metadata_stream(trailer, get);
            metadata.and_then(|id| to.at(id))
        };
        let unencrypted = encryptor.list.unencrypted_metadata;
        let metadata = match unencrypted {
            true => {
                let root = trailer.get(b"Root").and_then(Object::as_reference);
                if let Some(root) = root.filter(|&root| resolve.resolve(root).is_none()) {
                    return Err(Error::Catalog(root));
                }
                found()
            }
            false => None,
        };
        let metadata_anew = match (encryptor.place, unencrypted) {
            (Place::New, false) => found(),
            _ => None,
        };
        let ivs = (encryptor.ivs)();
        Ok(Encrypting {
            encryptor,
            resolve,
            metadata,
            ivs,
            metadata_anew,
            to,
            at,
            guard: None,
        })
    }

    /// It, kept by `guard` from putting a new plaintext under an RC4
    /// keystream its file used, where its encryptor's methods have RC4.
    pub(super) fn guarded(mut self, guard: Guard<'a>) -> Encrypting<'a> {
        if self.encryptor.rc4 {
            self.guard = Some(guard);
        }
        self
    }

    /// Its guard, where it has one.
    pub(super) fn guard(&self) -> Option<&Guard<'a>> {
        self.guard.as_ref()
    }

    /// Under RC4, each object at whose number and generation it encrypted a
    /// string or stream's data under a keystream the file used, as its
    /// encryptor was told to reuse them
    /// ([`Encryptor::reusing_rc4_keystreams`]), ascending: what leaks, as that
    /// says. None under another method, and none where it refused.
    #[must_use]
    pub fn keystreams_reused(&self) -> Vec<ObjectId> {
        self.guard.as_ref().map(Guard::reused).unwrap_or_default()
    }

    /// Nothing, where `plaintext` of the object at `at` may be encrypted by
    /// RC4: its guard's word, where it has one.
    fn guarded_rc4(&self, at: ObjectId, plaintext: &[u8]) -> Result<(), Unencryptable> {
        match &self.guard {
            Some(guard) => guard.check(at, plaintext),
            None => Ok(()),
        }
    }

    /// The encryption dictionary, as the file it's laid out for writes it:
    /// the encryptor's, as the file it was read from stores it, or a new
    /// file's, its references in the numbers written.
    #[must_use]
    pub fn dictionary(&self) -> &Dict {
        &self.encryptor.dict
    }

    /// Where the file it's laid out for has the encryption dictionary: the
    /// number [`Encryptor::laid_out`] was given, none where it's in the
    /// trailer.
    #[must_use]
    pub const fn at(&self) -> Option<ObjectId> {
        self.at
    }

    /// The first `/ID` element the key is made from, which the file's
    /// trailer keeps: [`Encryptor::first_id`]'s.
    #[must_use]
    pub fn first_id(&self) -> Option<&[u8]> {
        self.encryptor.first_id()
    }

    /// Whether the object at `at`, a number written, is an encryption
    /// dictionary, or what one's entries refer to, by number, which a
    /// reader reads before it has a key: left in the clear, so never put in
    /// an object stream, which is encrypted, and no other object written
    /// there. [`Encryptor::dictionary_object`] says it of the numbers the
    /// objects are given at.
    #[must_use]
    pub fn dictionary_object(&self, at: ObjectId) -> bool {
        self.encryptor.list.dictionaries.contains(&at)
    }

    /// Object `obj`, written at `at`, as a new encryption writes it where
    /// that isn't as it's held: the document's metadata stream, where the
    /// encryption encrypts metadata, without its own `/Crypt` filter that
    /// would leave its data in the clear (`/Identity`, named or not), so
    /// that it's encrypted as the file's streams are, as its
    /// `/EncryptMetadata` says (ISO 32000-2, 7.6.4.2, 7.6.6, 14.3.2). A
    /// reading's own encryption ([`Encryptor::of`]) writes it as read, as
    /// any other stream's own `/Crypt`. None where it's written as it is.
    pub(super) fn anew(&self, at: ObjectId, obj: &Object) -> Option<Object> {
        if self.metadata_anew != Some(at) {
            return None;
        }
        let Value::Stream(stream) = obj.value() else {
            return None;
        };
        let resolve = |id| self.resolve.resolve(id);
        let dict = filter::without_identity_crypt(&stream.dict, Some(&resolve))?;
        let mut stream = stream.clone();
        stream.dict = dict;
        Some(Object::new(Value::Stream(stream)))
    }

    /// The encryption it's of, in the numbers written.
    pub(super) fn encryptor(&self) -> &Encryptor {
        &self.encryptor
    }

    /// The renumbering the objects it encrypts are written by.
    pub(super) fn renumbering(&self) -> Numbering<'a> {
        self.to
    }

    /// Whether object `obj`, which the reading `read` was made of read at
    /// `id`, written here at `at`, is encrypted by this as that reading
    /// decrypted it: its strings by the same rule, the list asked of each
    /// number in its own file, and its stream's data, its own crypt filter
    /// followed through what's written here and through `there`, the
    /// reading's objects, there, and the reading's document metadata
    /// stream `metadata`. So, where `read`'s key is this one's, its bytes,
    /// written at the number they were read at, read back as the reading
    /// read them.
    pub(super) fn keeps<'o>(
        &self,
        read: &Encryptor,
        there: &dyn Fn(ObjectId) -> Option<&'o Object>,
        metadata: Option<ObjectId>,
        (id, at): (ObjectId, ObjectId),
        obj: &'o Object,
    ) -> bool
    where
        'a: 'o,
    {
        let (now, then) = (&self.encryptor.list, read.as_read());
        if now.encrypted(at) != then.encrypted(id) {
            return false;
        }
        let Value::Stream(stream) = obj.value() else {
            return true;
        };
        let dict = &stream.dict;
        let here = |id| -> Option<&'o Object> { self.resolve.resolve(id) };
        let own_here = || Ok::<_, Infallible>(filter::crypt_filter(dict, Some(&here)));
        let own_there = || Ok::<_, Infallible>(filter::crypt_filter(dict, Some(there)));
        let Ok(here) = now.stream(at, dict, own_here, || Ok(self.metadata));
        let Ok(there) = then.stream(id, dict, own_there, || Ok(metadata));
        here == there
    }
}

impl Encrypt for Encrypting<'_> {
    type Error = Unencryptable;

    /// In the clear: every object of a file that doesn't encrypt; an
    /// encryption dictionary and what its entries refer to, by number
    /// (ISO 32000-2, 7.6.2); and a cross-reference stream (7.5.8.2).
    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        clear(&self.encryptor.list, id, dict, stream)
    }

    /// A signature's `/Contents`, in a dictionary with `/ByteRange` and a
    /// string `/Contents`, of `/Type /Sig`, `/DocTimeStamp` or none (7.6.2).
    fn clear_key(&self, _: ObjectId, dict: &Dict) -> Option<&'static [u8]> {
        crypt::signature(dict).then_some(b"Contents")
    }

    /// Encrypted with the object's key as the handler's strings are, but
    /// where they're left in the clear (`/StrF /Identity`); under RC4, as
    /// its guard says.
    fn string<'t>(&self, id: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Unencryptable> {
        let handler = &self.encryptor.handler;
        let Some(method) = string_method(&self.encryptor.list, handler, id) else {
            return Ok(Cow::Borrowed(text));
        };
        if matches!(method, Method::Rc4) {
            self.guarded_rc4(id, text)?;
        }
        (handler.encrypt_string(id.into(), text, &self.ivs))
            .map(Cow::Owned)
            .map_err(Unencryptable::Fault)
    }

    /// Encrypted by its own crypt filter when it has one, else an embedded
    /// file's as `/EFF` says, else as the handler's streams are (7.6.6); in the
    /// clear where that's `/Identity`, and the document's metadata stream's
    /// under `/EncryptMetadata false` but where its own crypt filter encrypts
    /// it (7.6.4.2, 14.3.2); under RC4, as its guard says. The document's
    /// metadata stream under a new encryption that encrypts metadata, with its
    /// own `/Identity` still, is refused ([`Unencryptable::Metadata`]).
    fn stream<'d>(
        &self,
        id: ObjectId,
        dict: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, Unencryptable> {
        let resolve = |id| self.resolve.resolve(id);
        // The metadata a new encryption encrypts, its own /Identity still
        // there, as only a caller writing it past `Parts` gives it: in the
        // clear as it says, which its /EncryptMetadata says it isn't.
        if self.metadata_anew == Some(id)
            && filter::without_identity_crypt(dict, Some(&resolve)).is_some()
        {
            return Err(Unencryptable::Metadata);
        }
        let own = || filter::crypt_filter(dict, Some(&resolve));
        let handler = &self.encryptor.handler;
        let list = &self.encryptor.list;
        let Some(method) = stream_method(list, handler, self.metadata, id, dict, own)? else {
            return Ok(Cow::Borrowed(data));
        };
        if matches!(*method, Method::Rc4) {
            self.guarded_rc4(id, data)?;
        }
        (handler.encrypt_stream(id.into(), data, Some(&method), &self.ivs))
            .map(Cow::Owned)
            .map_err(Unencryptable::Fault)
    }
}

/// Whether object `id`, its dictionary, or its stream's, `dict`, and
/// whether it's a stream, is written in the clear under `list`, as
/// [`Encrypting`] writes it.
fn clear(list: &List, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
    !list.encrypts
        || list.dictionaries.contains(&id)
        || (stream && dict.is_some_and(crypt::is_xref))
}

/// The method object `id`'s strings are encrypted by under `list` and
/// `handler`, as [`Encrypting`] encrypts them: the handler's strings', none
/// where `list` leaves them in the clear.
fn string_method<'h>(list: &List, handler: &'h Handler, id: ObjectId) -> Option<&'h Method> {
    list.encrypted(id).then(|| handler.strings())
}

/// The method stream `id`'s data is encrypted by under `list` and
/// `handler`, its dictionary `dict`, as [`Encrypting`] encrypts it: by its
/// own crypt filter, `own` naming it, else an embedded file's as `/EFF`
/// says, else as the handler's streams are; none where it's in the clear,
/// `/Identity` and `/None` among them, the document's metadata stream
/// `metadata`'s under `/EncryptMetadata false` too. Refused where it's
/// neither said to be in the clear nor to be encrypted.
fn stream_method<'h>(
    list: &List,
    handler: &'h Handler,
    metadata: Option<ObjectId>,
    id: ObjectId,
    dict: &Dict,
    own: impl FnOnce() -> Option<Vec<u8>>,
) -> Result<Option<Cow<'h, Method>>, Unencryptable> {
    let own = || Ok::<_, Infallible>(own());
    let Ok(stream) = list.stream(id, dict, own, || Ok(metadata));
    let method = match stream {
        Stream::Clear => return Ok(None),
        Stream::Dictionary => return Err(Unencryptable::Dictionary),
        Stream::Unsaid => return Err(Unencryptable::Unsaid),
        Stream::By(Some(name)) => Cow::Owned(handler.named(&name)),
        Stream::By(None) => Cow::Borrowed(handler.streams()),
    };
    Ok((!matches!(*method, Method::Identity | Method::None)).then_some(method))
}

/// The plaintexts RC4 encrypted object `id` of `reading`'s file with,
/// under its keystream: each string RC4 encrypts and its stream's data
/// where RC4 encrypts that, as `reading` decrypts them, which is as
/// [`Encrypting`] encrypts what it writes, by the one list. None where
/// `reading` has no handler, or `id` isn't in use at an offset there, as
/// one in an object stream is encrypted under its container's keystream.
pub(super) fn rc4_plaintexts<S: ByteSource>(reading: &Reading<S>, id: ObjectId) -> Vec<Vec<u8>> {
    let listed = matches!(
        reading.entry(id.number),
        Some((generation, XrefEntry::InUse { .. })) if generation == id.generation
    );
    let (Some(handler), true) = (reading.handler(), listed) else {
        return Vec::new();
    };
    let Ok(object) = reading.get(id) else {
        return Vec::new();
    };
    let get: crypt::Get<'_, '_, Infallible> = &|id| Ok(reading.resolve(id));
    let Ok(metadata) = crypt::metadata_stream(reading.trailer().dict(), get);
    let resolve = |id| reading.resolve(id);
    let plaintexts = Plaintexts {
        list: reading.list(),
        handler,
        metadata,
        resolve: &resolve,
        found: RefCell::new(Vec::new()),
    };
    let mut out = Vec::new();
    // What doesn't write is what holds no plaintext to compare.
    let _ = match object.value() {
        Value::Stream(stream) => {
            let data = reading.data(id).unwrap_or_default();
            serialize::indirect_stream_with(&mut out, id, &stream.dict, &data, &plaintexts)
        }
        _ => serialize::indirect_with(&mut out, id, object, &plaintexts),
    };
    plaintexts.found.into_inner()
}

/// What RC4 encrypts of an object, as [`Encrypting`] would, taken as it's
/// passed through pdf-objects' writer: each string and stream's data RC4
/// encrypts, as it is.
struct Plaintexts<'r, 'o> {
    list: &'r List,
    handler: &'r Handler,
    /// The document's metadata stream, which `/EncryptMetadata false`
    /// leaves in the clear.
    metadata: Option<ObjectId>,
    /// What a reference in a stream's `/Filter` names.
    resolve: &'r dyn Fn(ObjectId) -> Option<&'o Object>,
    found: RefCell<Vec<Vec<u8>>>,
}

impl Encrypt for Plaintexts<'_, '_> {
    type Error = Infallible;

    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        clear(self.list, id, dict, stream)
    }

    fn clear_key(&self, _: ObjectId, dict: &Dict) -> Option<&'static [u8]> {
        crypt::signature(dict).then_some(b"Contents")
    }

    fn string<'t>(&self, id: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Infallible> {
        if matches!(
            string_method(self.list, self.handler, id),
            Some(Method::Rc4)
        ) {
            self.found.borrow_mut().push(text.to_vec());
        }
        Ok(Cow::Borrowed(text))
    }

    fn stream<'d>(
        &self,
        id: ObjectId,
        dict: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, Infallible> {
        let resolve = |id| (self.resolve)(id);
        let own = || filter::crypt_filter(dict, Some(&resolve));
        let method = stream_method(self.list, self.handler, self.metadata, id, dict, own);
        if let Ok(Some(method)) = method
            && matches!(*method, Method::Rc4)
        {
            self.found.borrow_mut().push(data.to_vec());
        }
        Ok(Cow::Borrowed(data))
    }
}

/// Whether any method `handler` encrypts by makes keys per object (RC4,
/// AES-128): its strings', its streams', those of the crypt filters `dict`,
/// its encryption dictionary, has in `/CF`, or `/EFF`'s.
fn per_object(handler: &Handler, dict: &Dict) -> bool {
    any_method(handler, dict, |m| matches!(m, Method::Rc4 | Method::Aes128))
}

/// Whether any method `handler` encrypts by is RC4: its strings', its
/// streams', those of the crypt filters `dict`, its encryption dictionary,
/// has in `/CF`, or `/EFF`'s.
fn rc4(handler: &Handler, dict: &Dict) -> bool {
    any_method(handler, dict, |m| matches!(m, Method::Rc4))
}

/// Whether any method `handler` encrypts by is one `is` says: its
/// strings', its streams', those of the crypt filters `dict`, its
/// encryption dictionary, has in `/CF`, or `/EFF`'s.
fn any_method(handler: &Handler, dict: &Dict, is: impl Fn(&Method) -> bool) -> bool {
    let filters = dict.get(b"CF").and_then(Object::as_dict);
    let named = (filters
        .into_iter()
        .flat_map(|cf| cf.iter().map(|(name, _)| name)))
    .chain(dict.get(b"EFF").and_then(Object::as_name));
    is(handler.strings())
        || is(handler.streams())
        || named.into_iter().any(|name| is(&handler.named(name)))
}

/// The encryption dictionary of `values`, pdf-crypt's form of a standard
/// security handler's (ISO 32000-2, 7.6.2, 7.6.4.2 and 7.6.6), as the values
/// say it: `/Filter`, where it's a name; `/V` and `/R`; each value `values`
/// holds of `/O`, `/U`, `/P`, `/Length`, `/OE`, `/UE`, `/Perms` and
/// `/EncryptMetadata`; `/CF`, where it names crypt filters, each by its name,
/// its method its `/CFM`, with `/AuthEvent /DocOpen` and a `/Length` of
/// `length` where that's given; and `/StmF` and `/StrF` as given. A value
/// `values` hasn't is left out. So a new file's ([`Encryptor::create`],
/// `length` `Some(32)`) names its one crypt filter `/StdCF` for strings and
/// streams both, as its values do, and a dictionary [`read::encryption`]
/// read is written back naming what the file named: `read::encryption` of
/// what this writes is `values`. What pdf-crypt's values don't hold isn't
/// written: `/EFF`, a crypt filter's own `/AuthEvent` and `/Length` but
/// `length`, and any key the standard security handler doesn't read.
///
/// [`read::encryption`]: crate::read::encryption
#[must_use]
pub fn encryption(values: &pdf_crypt::Dictionary, length: Option<i64>) -> Dict {
    let s = |bytes: &[u8]| Object::new(Value::String(Str::Bytes(bytes.to_vec()), None));
    let name = |bytes: &[u8]| Object::new(Value::Name(bytes.to_vec()));
    let mut dict = Dict::new();
    if !values.filter.is_empty() {
        dict.set("Filter", name(&values.filter));
    }
    dict.set("V", values.v);
    dict.set("R", values.r);
    for (key, value) in [(&b"O"[..], &values.o), (b"U", &values.u)] {
        if let Some(value) = value {
            dict.set(key, s(value));
        }
    }
    if let Some(p) = values.p {
        dict.set("P", p);
    }
    if let Some(length) = values.length {
        dict.set("Length", length);
    }
    let keys = [
        (&b"OE"[..], &values.oe),
        (b"UE", &values.ue),
        (b"Perms", &values.perms),
    ];
    for (key, value) in keys {
        if let Some(value) = value {
            dict.set(key, s(value));
        }
    }
    if let Some(encrypt_metadata) = values.encrypt_metadata {
        dict.set("EncryptMetadata", encrypt_metadata);
    }
    if !values.crypt_filters.is_empty() {
        let mut filters = Dict::new();
        for filter in &values.crypt_filters {
            let mut cf = Dict::new();
            if let Some(method) = &filter.method {
                cf.set("CFM", name(method));
            }
            cf.set("AuthEvent", "DocOpen");
            if let Some(length) = length {
                cf.set("Length", length);
            }
            filters.set(&filter.name[..], cf);
        }
        dict.set("CF", filters);
    }
    for (key, value) in [(&b"StmF"[..], &values.stm_f), (b"StrF", &values.str_f)] {
        if let Some(value) = value {
            dict.set(key, name(value));
        }
    }
    dict
}
