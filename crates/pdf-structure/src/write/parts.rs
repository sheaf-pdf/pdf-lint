//! The per-body writer: what a whole file writes each object and object stream
//! through ([`full`](super::full)), for a caller that lays a file out itself and
//! so has numbers of its own to write them at.

use std::fmt;
use std::sync::atomic::AtomicBool;

use pdf_objects::object::ObjectId;
use pdf_objects::serialize::Renumber;

use super::copy::{Copying, Member, written};
use super::full::containers;
use super::{Body, Deflate, Encrypting, Error, Numbering, ReadFrom};

/// An object stream's member as [`Parts::containers`] takes it: the number it's
/// given at, the number it's written at, and its body.
pub type Packed<'a> = (ObjectId, ObjectId, &'a Body<'a>);

/// Bodies written one at a time, as [`full`](super::full) writes them, each
/// object encrypted under the number it's written at, its references as a
/// renumbering names them, copied where the rules for a copy hold
/// ([`Body::Read`]), and object streams of them, by a caller that lays the file
/// out itself. What's around them, its numbers among it, is that caller's: the
/// guards [`full`](super::full) keeps on a file as a whole aren't here (a number
/// given twice, one its encryptor's file lists free, what's written where a
/// reader looks for what's in the clear), and [`Encryptor`](super::Encryptor)
/// says what comes of leaving each out.
pub struct Parts<'w> {
    encrypting: Option<&'w Encrypting<'w>>,
    from: Option<&'w dyn ReadFrom>,
    copying: Option<Copying<'w>>,
    to: Numbering<'w>,
}

impl fmt::Debug for Parts<'_> {
    /// What encrypts, and whether a reading is copied from and a caller's
    /// renumbering names the numbers, as [`Options`](super::Options)' says
    /// a caller's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Parts")
            .field("encrypting", &self.encrypting)
            .field("from", &self.from.map(|_| "a caller's"))
            .field("to", &self.to.0.map(|_| "a caller's"))
            .finish_non_exhaustive()
    }
}

impl<'w> Parts<'w> {
    /// What writes bodies: `encrypting` what encrypts them, none for a file in
    /// the clear; `from` the reading the objects were read in; `to` where each
    /// goes, none for each at the number it's given at. What's copied follows from
    /// those: in the clear, what `from` holds in the clear; encrypted, only what's
    /// encrypted under the key it was read under, so an `encrypting` of an
    /// [`Encryptor::of`](super::Encryptor::of) `from`, renumbered and made whole or not. Under any other
    /// key each object is written anew.
    ///
    /// # Errors
    ///
    /// Refused where `from`'s source says its bytes aren't those it was read
    /// from ([`Error::Changed`]), or can't tell ([`Error::Io`]).
    pub fn new(
        encrypting: Option<&'w Encrypting<'w>>,
        from: Option<&'w dyn ReadFrom>,
        to: Option<&'w (dyn Renumber + Sync + 'w)>,
    ) -> Result<Parts<'w>, Error> {
        let encryptor = encrypting.map(Encrypting::encryptor);
        let copying = from.map(|from| Copying::new(from, encryptor)).transpose()?;
        Ok(Parts::of(encrypting, from, copying, Numbering(to)))
    }

    /// What writes bodies, what's copied by already made.
    pub(super) fn of(
        encrypting: Option<&'w Encrypting<'w>>,
        from: Option<&'w dyn ReadFrom>,
        copying: Option<Copying<'w>>,
        to: Numbering<'w>,
    ) -> Parts<'w> {
        Parts {
            encrypting,
            from,
            copying,
            to,
        }
    }

    /// Whether what's written goes by a renumbering.
    pub(super) fn renumbers(&self) -> bool {
        self.to.0.is_some()
    }

    /// Object `id`, `body`, written on its own at the end of `out` at `at`, `n g
    /// obj` to `endobj` and a line's end: where it starts in `out`. Copied where
    /// it's copied, else written anew, a stream's data where it was written read
    /// from the reading.
    ///
    /// # Errors
    ///
    /// Refused as [`full`](super::full) refuses what's in a body: a copy
    /// whose record is another reading's or another object's
    /// ([`Error::Elsewhere`]), or whose bytes aren't those it was read from
    /// ([`Error::Changed`]) or aren't there to read ([`Error::Gone`],
    /// [`Error::Uncontained`]), a stream's data no reading given holds
    /// ([`Error::Placed`]), and what pdf-objects' writer or the encryptor
    /// refuses.
    pub fn body(
        &self,
        out: &mut Vec<u8>,
        id: ObjectId,
        at: ObjectId,
        body: &Body<'_>,
    ) -> Result<usize, Error> {
        let copying = self.copying.as_ref();
        written(
            out,
            id,
            at,
            body,
            self.encrypting,
            self.from,
            copying,
            self.to,
        )
    }

    /// Each of `groups`, an object stream's number and its members, each given
    /// at, written at, and its body, as that object stream written whole, members
    /// copied where they're copied, else written anew, and encrypted when what
    /// writes them encrypts, once, as a stream, under its number. None for one
    /// whose data would be more than the reader takes
    /// ([`MAX_DATA`](super::MAX_DATA)), or that has more members than a place in
    /// it can say. Built on up to `threads` threads, 0 for as many as the machine
    /// runs at once, up to 8, the same objects on any number (the same bytes in
    /// the clear, and under the same IVs), `stop` checked before each.
    ///
    /// # Errors
    ///
    /// Refused as [`Parts::body`] refuses, the first in their order; and
    /// stopped where `stop` says so ([`Error::Stopped`]).
    pub fn containers(
        &self,
        groups: &[(u32, Vec<Packed<'_>>)],
        deflate: Deflate,
        threads: usize,
        stop: Option<&AtomicBool>,
    ) -> Result<Vec<Option<Vec<u8>>>, Error> {
        // Each member at the number it's written at: copied, where it's given
        // as read and a reading is given, as its object stream is built; else
        // its value.
        let copying = self.copying.as_ref();
        let groups: Vec<(u32, Vec<(ObjectId, Member<'_>)>)> = (groups.iter())
            .map(|(n, members)| {
                let members = members.iter().map(|&(id, at, body)| {
                    let member = match (copying, body) {
                        (Some(_), Body::Read { .. }) => Member::Read(id, body),
                        _ => Member::Object(body.object()),
                    };
                    (at, member)
                });
                (*n, members.collect())
            })
            .collect();
        let threads = match threads {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get().min(8)),
            n => n,
        };
        containers(
            &groups,
            copying,
            self.encrypting,
            self.to,
            deflate,
            threads,
            stop,
        )
    }
}
