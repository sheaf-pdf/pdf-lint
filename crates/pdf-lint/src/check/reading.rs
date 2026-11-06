//! What only a file's reading gives a check, beside the document read from
//! it ([`check_with_reading`](super::check_with_reading)): what an earlier
//! revision had at a number a later revision wrote over or freed, read
//! from the file as that revision was written ([`Reading::prefix`]), each
//! revision's reading dropped once its trailer's walk is over; and
//! the facts of the file's linearization, as pdf-linearize reads them from
//! its bytes and the document read beside them
//! ([`family::facts_with_reading`]), or from the file as it was first
//! written ([`family::facts`]), where an update after its linearization
//! voids it.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;

use pdf_linearize::family;

use super::linearization::Placing;
use crate::bytes::{ByteSource, Limits, Prefix, Reading, WithReading};
use crate::object::{Object, ObjectId, Span};
use crate::source::{Revision, Source};

/// How many earlier revisions' readings a check opens at most, the newest
/// first, as it visits their trailers: an object of one past them is
/// counted as not checked, as a document alone has it.
const READINGS: usize = 256;

/// How many entries the earlier revisions' readings may list in all, as
/// many times what the file's own reading may list.
const ENTRIES: usize = 4;

/// What part of the file's reading's budget each earlier revision's
/// reading decodes and keeps its streams within: a 64th, so that
/// [`READINGS`] of them keep at most four times what the file's may, as
/// their entries are.
const SHARE: usize = 64;

/// What a check is given that only a file's reading has, beside the
/// objects of its newest revision.
pub(super) trait Reads {
    /// Object `id` as revision `revision`, counted from the first as 1,
    /// had it, read from the file as that revision was written; none where
    /// it isn't read there, or the revision isn't, or its walk is over
    /// ([`Reads::done`]) and `id` wasn't asked for before.
    fn earlier(&self, revision: usize, id: ObjectId) -> Option<&Object>;

    /// That the walk of revision `revision`'s trailer is over, so nothing
    /// asks for one of its objects again: its reading is dropped, and what
    /// was asked of it kept, never opened again.
    fn done(&self, revision: usize);

    /// The facts pdf-linearize judges a linearized file by, read from the
    /// file's reading under its own caps (`Limits::DEFAULT`): the first
    /// object, and where that says the file's linearized, its sections, its
    /// hint stream's data, its page tree and the graph of its objects, of
    /// the document read whole beside the reading. Read anew each time it's
    /// asked; an error only where the source doesn't give the bytes asked of
    /// it.
    fn linearization(&self) -> Result<pdf_linearize::Facts, family::Error>;

    /// The file's first `len` bytes, read as a file of their own under the
    /// file's reading's own limits ([`Reading::prefix`]), and the facts
    /// pdf-linearize judges them by, as [`Reads::linearization`] reads
    /// them: at the end of the first revision, the file as it was first
    /// written, for its linearization as that. Opened the first time it's
    /// asked for, and kept, a check asking for one length alone; its facts
    /// read anew each time. An error only where the source doesn't give
    /// the bytes asked of it.
    fn first_written(
        &self,
        len: usize,
    ) -> Result<(pdf_linearize::Facts, &dyn Placing), family::Error>;
}

/// The file as an earlier revision was written, read: its first bytes, as
/// a file of their own ([`Reading::prefix`]).
type AsWritten<'f, S> = Reading<Prefix<&'f S>>;

/// The earlier revisions of a file read beside it: each the file's bytes
/// up to where the revision ends ([`Revision::end`]), opened as a file of
/// their own by the file's reading ([`Reading::prefix`]) the first time one
/// of its objects is asked for, and dropped once its trailer's walk is over
/// ([`Reads::done`]), what was asked of it kept for the check. At most
/// [`READINGS`] of them, their entries in all at most [`ENTRIES`] times what
/// the file's reading may list ([`Limits::entries`]), each under the file's
/// reading's limits but its budget, of which each has a [`SHARE`]th; past
/// either, and for a revision whose end isn't known, none is read.
pub(super) struct Readings<'f, S> {
    /// The file read whole beside its reading.
    file: &'f WithReading<S>,
    reading: &'f Reading<S>,
    revisions: &'f [Revision],
    /// Each one's limits.
    limits: Limits,
    /// How many entries they may list in all.
    most: usize,
    /// Each revision's reading, the first first, as it stands.
    each: Box<[RefCell<Opened<AsWritten<'f, S>>>]>,
    /// How many have been opened.
    opened: Cell<usize>,
    /// How many entries those list in all.
    entries: Cell<usize>,
    /// What each revision's reading gave of each object asked of it, by the
    /// revision's place among them and the object: where among `kept`, or
    /// none where it didn't read.
    asked: RefCell<HashMap<(usize, ObjectId), Option<usize>>>,
    /// Each object an earlier revision's reading gave, kept for the check.
    kept: Kept,
    /// The file as it was first written, for its linearization, under the
    /// file's reading's own limits, once it's been asked for
    /// ([`Reads::first_written`]): no reading of an earlier revision's
    /// objects, and none of the caps theirs are under.
    first: OnceCell<AsWritten<'f, S>>,
}

/// An earlier revision's reading, as it stands.
enum Opened<R> {
    /// Not asked for yet.
    Not,
    /// Opened, and its walk not over.
    Open(R),
    /// Not read, or its walk is over: never opened again.
    Closed,
}

/// Objects kept, each where it was put while the store grows: in chunks of
/// slots, each set once, the `k`th chunk of `16 << k`, made as the slots
/// before it fill, so a slot is never moved and what's handed out of it
/// stands as long as the store.
#[derive(Default)]
struct Kept {
    chunks: [OnceCell<Box<[OnceCell<Object>]>>; 32],
    /// How many are kept.
    len: Cell<usize>,
}

impl Kept {
    /// Where slot `i` is: its chunk, and its place there.
    fn place(i: usize) -> (usize, usize) {
        let k = (i / 16 + 1).ilog2() as usize;
        (k, i - 16 * ((1 << k) - 1))
    }

    /// `object` kept, and where.
    fn keep(&self, object: Object) -> usize {
        let i = self.len.get();
        let (k, at) = Kept::place(i);
        let chunk = self.chunks[k].get_or_init(|| (0..16 << k).map(|_| OnceCell::new()).collect());
        let _ = chunk[at].set(object);
        self.len.set(i + 1);
        i
    }

    /// The object kept at `i`.
    fn get(&self, i: usize) -> Option<&Object> {
        let (k, at) = Kept::place(i);
        self.chunks.get(k)?.get()?.get(at)?.get()
    }
}

impl<'f, S: ByteSource> Readings<'f, S> {
    /// The earlier revisions of `file`, none of them read yet.
    pub(super) fn new(file: &'f WithReading<S>) -> Readings<'f, S> {
        let reading = file.reading();
        let mut limits = *reading.limits();
        limits.budget /= SHARE;
        // As many as the file has bytes, or 2^20 if more, unless the
        // reading was given a number.
        let listable = (reading.limits().entries).unwrap_or_else(|| reading.size().max(1 << 20));
        let revisions = Source::revisions(file);
        Readings {
            file,
            reading,
            revisions,
            limits,
            most: listable.saturating_mul(ENTRIES),
            each: revisions
                .iter()
                .map(|_| RefCell::new(Opened::Not))
                .collect(),
            opened: Cell::new(0),
            entries: Cell::new(0),
            asked: RefCell::default(),
            kept: Kept::default(),
            first: OnceCell::new(),
        }
    }

    /// The reading of the revision at `i`, the first at 0, opened now:
    /// none where its end isn't known, where [`READINGS`] have been opened
    /// already or their entries reach what they may list, or where its own
    /// take them past that, which counts them, so that none is opened
    /// after.
    fn open(&self, i: usize) -> Option<AsWritten<'f, S>> {
        let end = self.revisions.get(i)?.end?.line();
        if self.opened.get() >= READINGS || self.entries.get() >= self.most {
            return None;
        }
        self.opened.set(self.opened.get() + 1);
        let prefix = self.reading.prefix(end, &self.limits).ok()?;
        let entries = (self.entries.get()).saturating_add(prefix.entries().count());
        self.entries.set(entries);
        (entries <= self.most).then_some(prefix)
    }
}

impl<S: ByteSource> Reads for Readings<'_, S> {
    fn earlier(&self, revision: usize, id: ObjectId) -> Option<&Object> {
        let i = revision.checked_sub(1)?;
        let each = self.each.get(i)?;
        if let Some(&at) = self.asked.borrow().get(&(i, id)) {
            return self.kept.get(at?);
        }
        let mut opened = each.borrow_mut();
        if let Opened::Not = *opened {
            *opened = self.open(i).map_or(Opened::Closed, Opened::Open);
        }
        let object = match &*opened {
            Opened::Open(reading) => reading.object_unkept(id).ok(),
            _ => None,
        };
        let at = object.map(|(object, _)| self.kept.keep(object));
        self.asked.borrow_mut().insert((i, id), at);
        self.kept.get(at?)
    }

    fn done(&self, revision: usize) {
        if let Some(each) = revision.checked_sub(1).and_then(|i| self.each.get(i)) {
            *each.borrow_mut() = Opened::Closed;
        }
    }

    fn linearization(&self) -> Result<pdf_linearize::Facts, family::Error> {
        let limits = pdf_linearize::Limits::DEFAULT;
        let facts = family::facts_with_reading(self.file, &limits);
        // The facts of a whole read made again, as the document beside the
        // reading is its whole read: the same, in every test and fuzz input.
        #[cfg(any(test, fuzzing))]
        {
            let again = family::facts(self.reading, &limits);
            assert_eq!(format!("{facts:?}"), format!("{again:?}"));
        }
        facts
    }

    fn first_written(
        &self,
        len: usize,
    ) -> Result<(pdf_linearize::Facts, &dyn Placing), family::Error> {
        let first = match self.first.get() {
            Some(first) => first,
            None => {
                let limits = self.reading.limits();
                let opened = (self.reading.prefix(len, limits)).map_err(family::Error::Read)?;
                self.first.get_or_init(|| opened)
            }
        };
        let facts = family::facts(first, &pdf_linearize::Limits::DEFAULT)?;
        Ok((facts, first))
    }
}

/// A file's reading, as a linearization's places are found in it: each
/// object read and kept as [`Reading::get`] reads it, so that its record is
/// known.
impl<S: ByteSource> Placing for Reading<S> {
    fn size(&self) -> usize {
        Reading::size(self)
    }

    fn ids(&self) -> Vec<ObjectId> {
        Reading::ids(self).collect()
    }

    fn object(&self, id: ObjectId) -> Option<&Object> {
        self.get(id).ok()
    }

    fn record(&self, id: ObjectId) -> Option<Span> {
        self.get(id).ok()?;
        Reading::record(self, id).map(|record| record.at)
    }

    fn decoded(&self, id: ObjectId) -> Option<Vec<u8>> {
        self.decoded_within(id, usize::MAX).ok()
    }
}

#[cfg(test)]
mod tests {
    //! An earlier revision's trailer checked beside its file's reading:
    //! what it refers to that a later revision wrote over or freed, read as
    //! that revision had it, within the caps, and counted where it isn't.

    use pdf_structure::fixture;

    use super::*;
    use crate::bytes;
    use crate::check::tests::{CATALOG, NO_PAGES, PAGES, Sections, Walked, not_made, walked};
    use crate::check::{
        self, Hold, Made, Options, Report, Rule, WRITTEN_OVER, check, check_with_reading,
    };
    use crate::dict;
    use crate::object::{Str, Value};
    use crate::pdf_crypt::Password;
    use crate::source::RevisionEnd;

    /// `file` read beside its reading under `options`, and checked so and
    /// as its document alone, every object held as merged.
    fn both(file: &[u8], options: &bytes::Options) -> (Report, Report) {
        let read = WithReading::new(file, &[], options).unwrap();
        let hold = Hold::All(Made::Merged);
        let beside = check_with_reading(&read, &hold, &Options::default());
        (beside, check(read.document(), &hold, &Options::default()))
    }

    /// The findings in earlier revisions' trailers.
    fn earlier(report: &Report) -> Vec<Walked<'_>> {
        let found = report.findings.iter().zip(walked(report));
        found
            .filter(|(f, _)| f.revision.is_some())
            .map(|(_, w)| w)
            .collect()
    }

    /// A file whose first revision's trailer's `/Info`, object 5, is
    /// `info`, and whose second writes 5 again as `again`.
    fn info_again(info: &str, again: &str) -> Vec<u8> {
        let mut file = Sections::new("1.7");
        (file.object(1, CATALOG).object(2, PAGES)).object(5, info);
        let first = file.table(&[0, 1, 2, 5], "/Size 6 /Root 1 0 R /Info 5 0 R");
        file.end(first).object(5, again);
        let trailer = format!("/Size 6 /Root 1 0 R /Info 5 0 R /Prev {first}");
        let second = file.table(&[5], &trailer);
        file.end(second);
        file.out
    }

    /// The first revision's `/Info` and `/Root`, which the second wrote
    /// again, are read as it had them beside the file's reading: its
    /// `/Info`'s SpecialCase, that it have `/ModDate` where the catalog has
    /// `/PieceInfo`, is held as the first revision had them, its own
    /// information dictionary without one, not the second's with one; and
    /// nothing is counted as not checked. Alone, the two are counted.
    #[test]
    fn an_earlier_trailers_objects_written_over_are_read_as_its_revision_had_them() {
        let catalog =
            "<< /Type /Catalog /Pages 2 0 R /PieceInfo << >> /LastModified (D:20240101) >>";
        let mut file = Sections::new("1.7");
        (file.object(1, catalog).object(2, PAGES)).object(5, "<< /Title (Old) >>");
        let first = file.table(&[0, 1, 2, 5], "/Size 6 /Root 1 0 R /Info 5 0 R");
        let new = "<< /Title (New) /ModDate (D:20240102) >>";
        file.end(first).object(1, catalog).object(5, new);
        let trailer = format!("/Size 6 /Root 1 0 R /Info 5 0 R /Prev {first}");
        let second = file.table(&[1, 5], &trailer);
        file.end(second);
        let (beside, alone) = both(&file.out, &bytes::Options::DEFAULT);
        let special = "<< /Title (Old) >> (fn:Eval(fn:SinceVersion(1.4,fn:IsPresent(trailer::Catalog::PieceInfo,fn:IsPresent(trailer::Info::ModDate)))))";
        let info = (
            None,
            "trailer 1",
            "FileTrailer",
            "Info",
            Rule::Special,
            special,
        );
        assert_eq!(walked(&beside), [info, NO_PAGES], "{beside}");
        assert_eq!(not_made(&beside), [], "{beside}");
        assert_eq!(beside.findings[0].revision, Some(1));
        assert_eq!(walked(&alone), [NO_PAGES], "{alone}");
        assert_eq!(
            not_made(&alone),
            [
                ("Type", WRITTEN_OVER, "FileTrailer", "Info", 1),
                ("Type", WRITTEN_OVER, "FileTrailer", "Root", 1),
            ],
            "{alone}"
        );
        assert_eq!(beside.keys, alone.keys);
    }

    /// An earlier `/Info` that was an array, which a later revision wrote
    /// again as a dictionary, is a `type` finding in the trailer that names
    /// it, as it was then; and alone, counted, never found.
    #[test]
    fn an_earlier_trailers_object_is_checked_as_it_was_not_as_it_is() {
        let file = info_again("[1 2]", "<< /Title (New) >>");
        let (beside, alone) = both(&file, &bytes::Options::DEFAULT);
        let info = (
            None,
            "trailer 1",
            "FileTrailer",
            "Info",
            Rule::Type,
            "[1 2]",
        );
        assert_eq!(walked(&beside), [info, NO_PAGES], "{beside}");
        assert_eq!(not_made(&beside), [], "{beside}");
        assert_eq!(walked(&alone), [NO_PAGES], "{alone}");
        let counted = ("Type", WRITTEN_OVER, "FileTrailer", "Info", 1);
        assert_eq!(not_made(&alone), [counted], "{alone}");
    }

    /// A number freed and then used again: the first revision's `/Info`
    /// there is its own, read; the second's, which freed it, refers to an
    /// object its revision hasn't, which doesn't read there, so it's
    /// counted, as alone.
    #[test]
    fn a_number_freed_and_used_again_is_read_where_its_revision_has_it() {
        let mut file = Sections::new("1.7");
        (file.object(1, CATALOG).object(2, PAGES)).object(5, "<< /Title (Old) >>");
        let first = file.table(&[0, 1, 2, 5], "/Size 6 /Root 1 0 R /Info 5 0 R");
        file.end(first).free(5);
        let trailer = format!("/Size 6 /Root 1 0 R /Info 5 0 R /Prev {first}");
        let second = file.table(&[5], &trailer);
        file.end(second).object(5, "(used again)");
        let third = file.table(&[5], &format!("/Size 6 /Root 1 0 R /Prev {second}"));
        file.end(third);
        let (beside, alone) = both(&file.out, &bytes::Options::DEFAULT);
        assert_eq!(earlier(&beside), [], "{beside}");
        let counted = |n| [("Type", WRITTEN_OVER, "FileTrailer", "Info", n)];
        assert_eq!(not_made(&beside), counted(1), "{beside}");
        assert_eq!(not_made(&alone), counted(2), "{alone}");
        assert_eq!(walked(&beside), walked(&alone));
    }

    /// A chain of references is followed as the revision had each object:
    /// `/Info` through an object no later revision wrote, to one the second
    /// wrote again as an array; and `/Root` through one the second wrote
    /// again as a string, which was a reference to the catalog, which none
    /// did. Both are what they were, a dictionary and the catalog.
    #[test]
    fn a_chain_is_followed_as_its_revision_had_each_object() {
        let mut file = Sections::new("1.7");
        (file.object(1, CATALOG).object(2, PAGES)).object(5, "6 0 R");
        file.object(6, "<< /Title (Old) >>").object(7, "1 0 R");
        let first = file.table(&[0, 1, 2, 5, 6, 7], "/Size 8 /Root 7 0 R /Info 5 0 R");
        file.end(first)
            .object(6, "[(New)]")
            .object(7, "(no catalog)");
        let trailer = format!("/Size 8 /Root 1 0 R /Prev {first}");
        let second = file.table(&[6, 7], &trailer);
        file.end(second);
        let (beside, alone) = both(&file.out, &bytes::Options::DEFAULT);
        assert_eq!(earlier(&beside), [], "{beside}");
        assert_eq!(not_made(&beside), [], "{beside}");
        assert_eq!(earlier(&alone), [], "{alone}");
        assert_eq!(
            not_made(&alone),
            [
                ("Type", WRITTEN_OVER, "FileTrailer", "Info", 1),
                ("Type", WRITTEN_OVER, "FileTrailer", "Root", 1),
            ],
            "{alone}"
        );
    }

    /// A revision whose section has no `startxref` and `%%EOF` of its own
    /// has no end, so isn't read: what its trailer refers to that a later
    /// revision wrote again is counted, as alone.
    #[test]
    fn a_revision_with_no_end_isnt_read() {
        let mut file = Sections::new("1.7");
        (file.object(1, CATALOG).object(2, PAGES)).object(5, "[1 2]");
        let first = file.table(&[0, 1, 2, 5], "/Size 6 /Root 1 0 R /Info 5 0 R");
        file.object(5, "<< /Title (New) >>");
        let trailer = format!("/Size 6 /Root 1 0 R /Info 5 0 R /Prev {first}");
        let second = file.table(&[5], &trailer);
        file.end(second);
        let read = WithReading::new(&file.out[..], &[], &bytes::Options::DEFAULT).unwrap();
        assert_eq!(read.document().revisions.len(), 2);
        assert_eq!(read.document().revisions[0].end, None);
        let (beside, alone) = both(&file.out, &bytes::Options::DEFAULT);
        let counted = ("Type", WRITTEN_OVER, "FileTrailer", "Info", 1);
        assert_eq!(not_made(&beside), [counted], "{beside}");
        assert_eq!(beside, alone);
        assert!(
            Readings::new(&read)
                .earlier(1, ObjectId::new(5, 0))
                .is_none()
        );
    }

    /// A file of `earlier` revisions, each writing `/Info`, object 5, an
    /// empty dictionary, again, and a newest one after them, the first
    /// listing 9 objects: 0, the catalog, the page tree, 5 and five more.
    fn many(earlier: usize) -> Vec<u8> {
        let mut file = Sections::new("1.7");
        (file.object(1, CATALOG).object(2, PAGES)).object(5, "<< >>");
        for n in [3, 4, 6, 7, 8] {
            file.object(n, "<< >>");
        }
        let numbers: Vec<u32> = (0..=8).collect();
        let mut at = file.table(&numbers, "/Size 9 /Root 1 0 R /Info 5 0 R");
        for _ in 0..earlier {
            file.end(at).object(5, "<< >>");
            at = file.table(&[5], &format!("/Size 9 /Root 1 0 R /Info 5 0 R /Prev {at}"));
        }
        file.end(at);
        file.out
    }

    /// At most 256 earlier revisions are read, the newest first, as the
    /// walk visits their trailers: of 257, the first isn't, and what its
    /// trailer refers to that later ones wrote again is counted.
    #[test]
    fn at_most_256_earlier_revisions_are_read() {
        let file = many(257);
        let read = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        assert_eq!(read.document().revisions.len(), 258);
        let readings = Readings::new(&read);
        for revision in (2..=257).rev() {
            assert!(
                readings.earlier(revision, ObjectId::new(5, 0)).is_some(),
                "{revision}"
            );
        }
        assert!(readings.earlier(1, ObjectId::new(5, 0)).is_none());
        assert_eq!(readings.opened.get(), READINGS);
        let (beside, alone) = both(&file, &bytes::Options::DEFAULT);
        let counted = |n| [("Type", WRITTEN_OVER, "FileTrailer", "Info", n)];
        assert_eq!(not_made(&beside), counted(1), "{beside}");
        assert_eq!(not_made(&alone), counted(257), "{alone}");
    }

    /// A revision's reading is dropped once its walk is over, and never
    /// opened again: what was asked of it before is what it gave, at the
    /// same place, and what wasn't is none. So of 257 earlier revisions,
    /// the newest asked for twice, once after its walk, the 256 newest are
    /// still read, and the first isn't.
    #[test]
    fn a_revisions_reading_is_dropped_once_its_walk_is_over_and_never_opened_again() {
        let file = many(257);
        let read = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let readings = Readings::new(&read);
        let info = readings.earlier(257, ObjectId::new(5, 0)).unwrap();
        assert!(matches!(*readings.each[256].borrow(), Opened::Open(_)));
        readings.done(257);
        assert!(matches!(*readings.each[256].borrow(), Opened::Closed));
        assert!(std::ptr::eq(
            readings.earlier(257, ObjectId::new(5, 0)).unwrap(),
            info
        ));
        assert!(readings.earlier(257, ObjectId::new(1, 0)).is_none());
        assert_eq!(readings.opened.get(), 1);
        for revision in (2..=256).rev() {
            assert!(
                readings.earlier(revision, ObjectId::new(5, 0)).is_some(),
                "{revision}"
            );
            readings.done(revision);
        }
        assert!(readings.earlier(1, ObjectId::new(5, 0)).is_none());
        assert_eq!(readings.opened.get(), READINGS);
        // The newest's reading is the file's, never asked for.
        let (newest, earlier) = readings.each.split_last().unwrap();
        assert!(
            earlier
                .iter()
                .all(|each| matches!(*each.borrow(), Opened::Closed))
        );
        assert!(matches!(*newest.borrow(), Opened::Not));
    }

    /// A check drops each earlier revision's reading as its trailer's walk
    /// ends, and finds what it found keeping them.
    #[test]
    fn a_check_drops_each_earlier_revisions_reading_after_its_walk() {
        let file = info_again("[1 2]", "<< /Title (New) >>");
        let read = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let readings = Readings::new(&read);
        let hold = Hold::All(Made::Merged);
        let report = check::check_in(&read, Some(&readings), &hold, &Options::default());
        assert!(matches!(*readings.each[0].borrow(), Opened::Closed));
        assert_eq!(readings.opened.get(), 1);
        assert!(readings.kept.len.get() > 0);
        assert_eq!(report, both(&file, &bytes::Options::DEFAULT).0);
    }

    /// What's kept stays where it was put as more is: across the chunks'
    /// edges, each object read back is the one put there, at its place.
    #[test]
    fn what_an_earlier_reading_gave_stays_where_it_was_kept() {
        let kept = Kept::default();
        let at: Vec<*const Object> = (0..1_000)
            .map(|n: i64| ptr_of(kept.get(kept.keep(Object::from(n))).unwrap()))
            .collect();
        for (n, &at) in at.iter().enumerate() {
            let object = kept.get(n).unwrap();
            assert!(std::ptr::eq(object, at), "{n}");
            assert_eq!(object.as_integer(), Some(n as i64));
        }
        assert!(kept.get(1_000).is_none());
        let places: Vec<_> = [0, 15, 16, 47, 48, 111, 112].map(Kept::place).into();
        assert_eq!(
            places,
            [(0, 0), (0, 15), (1, 0), (1, 31), (2, 0), (2, 63), (3, 0)]
        );
    }

    fn ptr_of(object: &Object) -> *const Object {
        object
    }

    /// The entries the earlier revisions' readings list are at most four
    /// times what the file's reading may list, in all: under a reading that
    /// may list 9, as its file's newest revision does, each earlier one's
    /// lists 9, so of five, the four newest are read, and the first's
    /// `/Info` is counted.
    #[test]
    fn the_earlier_revisions_list_at_most_four_times_the_files_entries() {
        let file = many(5);
        let mut options = bytes::Options::DEFAULT;
        options.limits.entries = Some(9);
        let read = WithReading::new(&file[..], &[], &options).unwrap();
        assert_eq!(read.reading().entries().count(), 9);
        assert_eq!(read.document().revisions.len(), 6);
        let readings = Readings::new(&read);
        assert_eq!(readings.most, 36);
        for revision in (2..=5).rev() {
            assert!(
                readings.earlier(revision, ObjectId::new(5, 0)).is_some(),
                "{revision}"
            );
        }
        assert_eq!(readings.entries.get(), 36);
        assert!(readings.earlier(1, ObjectId::new(5, 0)).is_none());
        assert_eq!(readings.opened.get(), 4);
        let (beside, alone) = both(&file, &options);
        let counted = |n| [("Type", WRITTEN_OVER, "FileTrailer", "Info", n)];
        assert_eq!(not_made(&beside), counted(1), "{beside}");
        assert_eq!(not_made(&alone), counted(5), "{alone}");
        // The file's reading's default: as many as it has bytes, or 2^20.
        let read = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        assert_eq!(Readings::new(&read).most, 4 << 20);
    }

    /// Each earlier revision's reading decodes within a 64th of the file's
    /// reading's budget: an earlier `/Info` in an object stream whose data
    /// is past that share doesn't read there, and is counted, though under
    /// a budget 64 times both its cross-reference stream's rows and the
    /// object stream's data it's read.
    #[test]
    fn an_earlier_revisions_reading_decodes_within_its_share_of_the_budget() {
        let info = "<< /Title (An information dictionary of some length, past the share) >>";
        let mut file = Sections::new("1.7");
        file.object(1, CATALOG).object(2, PAGES);
        file.hex_object_stream(10, &[(5, info)]);
        let first = file.stream(11, &[0, 1, 2, 5, 10], "/Root 1 0 R /Info 5 0 R");
        // Six rows of seven bytes: 0, 1, 2, 5, 10 and 11.
        let rows = 6 * 7;
        file.end(first).object(5, "<< /Title (New) >>");
        let trailer = format!("/Size 12 /Root 1 0 R /Info 5 0 R /Prev {first}");
        let second = file.table(&[5], &trailer);
        file.end(second);
        let mut options = bytes::Options::DEFAULT;
        // A share of the rows and 8 bytes more, short of the data.
        options.limits.budget = SHARE * (rows + 8);
        let read = WithReading::new(&file.out[..], &[], &options).unwrap();
        let readings = Readings::new(&read);
        assert_eq!(readings.limits.budget, rows + 8);
        assert!(readings.earlier(1, ObjectId::new(5, 0)).is_none());
        assert!(readings.earlier(1, ObjectId::new(1, 0)).is_some());
        let counted = ("Type", WRITTEN_OVER, "XRefStream", "Info", 1);
        let (beside, _) = both(&file.out, &options);
        assert_eq!(not_made(&beside), [counted], "{beside}");
        // A share of both, the data's hex two bytes a byte.
        options.limits.budget = SHARE * (rows + 2 * info.len() + 64);
        let (beside, alone) = both(&file.out, &options);
        assert_eq!(not_made(&beside), [], "{beside}");
        assert_eq!(not_made(&alone), [counted], "{alone}");
    }

    /// An encrypted file's earlier revision is read by its reading's
    /// handler, where its encryption dictionary and first `/ID` element are
    /// the same, though the user's password opened the file: the first
    /// trailer's `/Encrypt` and `/Info`, which the update wrote again, are
    /// read, and the information dictionary's string decrypted, without the
    /// password asked for again.
    #[test]
    fn an_encrypted_files_earlier_revision_is_read_by_its_readings_handler() {
        let id = b"0123456789abcdef";
        let settings = pdf_crypt::writer::Settings {
            kind: pdf_crypt::writer::Kind::Aes128,
            user: b"user",
            owner: b"owner",
            p: -4,
            id,
            encrypt_metadata: true,
        };
        let objects = [
            (
                1,
                fixture::Body::from(dict! { "Type" => "Catalog", "Pages" => Object::reference(2) }),
            ),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 }.into(),
            ),
            (5, dict! { "Title" => Object::text("Old") }.into()),
        ];
        let trailer = dict! { "Root" => Object::reference(1), "Info" => Object::reference(5) };
        let mut file = fixture::encrypted(&settings, &objects, trailer);
        // The update writes the encryption dictionary, 6, again as it was,
        // and the information dictionary anew.
        let text = String::from_utf8_lossy(&file).into_owned();
        let six = text.find("6 0 obj\n").unwrap();
        let encrypt = &text[six..six + text[six..].find("endobj").unwrap()];
        let previous: usize = (text.rsplit("startxref").next().unwrap().split_whitespace())
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let (five, six) = (file.len(), file.len() + "5 0 obj\n<< >>\nendobj\n".len());
        file.extend_from_slice(format!("5 0 obj\n<< >>\nendobj\n{encrypt}endobj\n").as_bytes());
        let xref = file.len();
        let id = String::from_utf8_lossy(id);
        file.extend_from_slice(
            format!(
                "xref\n5 2\n{five:010} 00000 n \n{six:010} 00000 n \n\
                 trailer\n<< /Size 7 /Root 1 0 R /Info 5 0 R /Encrypt 6 0 R /ID [({id}) ({id})] /Prev {previous} >>\n\
                 startxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let user = [Password("user")];
        let read = WithReading::new(&file[..], &user, &bytes::Options::DEFAULT).unwrap();
        assert!(
            read.document().warnings.is_empty(),
            "{:?}",
            read.document().warnings
        );
        assert_eq!(read.document().revisions.len(), 2);
        let readings = Readings::new(&read);
        let info = readings
            .earlier(1, ObjectId::new(5, 0))
            .unwrap()
            .as_dict()
            .unwrap();
        let title = info.get(b"Title").unwrap().value();
        assert!(
            matches!(title, Value::String(Str::Bytes(b), _) if b == b"Old"),
            "{title:?}"
        );
        assert!(
            readings
                .earlier(1, ObjectId::new(6, 0))
                .unwrap()
                .as_dict()
                .is_some()
        );
        let hold = Hold::All(Made::Merged);
        let beside = check_with_reading(&read, &hold, &Options::default());
        let alone = check(read.document(), &hold, &Options::default());
        let over = |report: &Report| -> Vec<String> {
            let rows = report
                .unchecked
                .keys()
                .filter(|row| row.why == WRITTEN_OVER);
            rows.map(|row| format!("{}/{}", row.table, row.key))
                .collect()
        };
        assert_eq!(over(&beside), Vec::<String>::new(), "{beside}");
        // Alone, the two, and whether the /ID, whose IndirectReference reads
        // whether the file is encrypted, may be a reference.
        assert_eq!(
            over(&alone),
            [
                "FileTrailer/ID",
                "TrailerIDArray/0",
                "TrailerIDArray/1",
                "FileTrailer/Encrypt",
                "FileTrailer/Info"
            ],
            "{alone}"
        );
    }

    /// An earlier revision's trailer's `fn:FileSize` is where its revision
    /// ends: a `/Prev` past that, into the next revision's bytes, is a
    /// `value` finding, though the file is longer, by `check` as by
    /// `check_with_reading`, as a document's revisions say where they end.
    #[test]
    fn an_earlier_trailers_file_size_is_where_its_revision_ends() {
        // Written once to learn where the first revision ends, with five
        // digits for /Prev, then again with /Prev 20 bytes past it.
        let written = |prev: usize| {
            let mut file = Sections::new("1.7");
            file.object(1, CATALOG).object(2, PAGES);
            let trailer = format!("/Size 3 /Root 1 0 R /Prev {prev:05}");
            let first = file.table(&[0, 1, 2], &trailer);
            file.end(first);
            let end = file.out.len();
            file.object(2, PAGES);
            let second = file.table(&[2], &format!("/Size 3 /Root 1 0 R /Prev {first}"));
            file.end(second);
            (file.out, end)
        };
        let (_, end) = written(99999);
        let prev = end + 20;
        let (file, again) = written(prev);
        assert_eq!(again, end);
        assert!(prev < file.len());
        let read = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        assert_eq!(
            read.document().revisions[0].end,
            RevisionEnd::new(end - 1, end)
        );
        let value = format!("{prev} (not one of fn:Eval((@Prev>=0) && (@Prev<=fn:FileSize())))");
        let prev = (
            None,
            "trailer 1",
            "FileTrailer",
            "Prev",
            Rule::Value,
            value.as_str(),
        );
        let (beside, alone) = both(&file, &bytes::Options::DEFAULT);
        for report in [&beside, &alone] {
            let found: Vec<_> = (walked(report).into_iter())
                .filter(|f| f.4 != Rule::Reader)
                .collect();
            assert_eq!(found, [prev, NO_PAGES], "{report}");
        }
    }
}
