//! Streaming PDF writers over `W: std::io::Write` ([`Writer`], [`UpdateWriter`]).
//!
//! Unlike [`full`](super::full) and [`update`](super::update), which collect all bodies and build
//! the output in a single `Vec<u8>`, [`Writer`] and [`UpdateWriter`] serialize and flush each
//! [`Body`] immediately on `push`, keeping only object byte offsets, referenced object numbers, and
//! an incremental SHA-256 state for `/ID[1]`. Peak internal scratch memory stays bounded by a
//! single body (shrinking back to at most 64 KiB after each `push`), and no `std::io::Seek` bound
//! is required on `W`.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::io::Write;

use pdf_objects::Resolve;
use pdf_objects::object::{Dict, Object, ObjectId, Value};
use pdf_objects::serialize;
use sha2::{Digest, Sha256};

use super::append::matched;
use super::copy::{Copying, as_read, written};
use super::crypt::{Guard, Place};
use super::{
    BINARY, Base, Body, Encrypting, Encryptor, Entry, Error, NUMBERS, Numbering, Options, Parts,
    ReadFrom, SECTION_KEYS, Section, Written, direct_info, first_id, fresh, id_value, stopped,
    trailer_entries, usable,
};
use crate::phases::phase;
use crate::source::XrefKind;

/// Maximum idle capacity retained by a streaming writer's reusable per-body buffer after flushing
/// an object (64 KiB).
const MAX_SCRATCH: usize = 64 * 1024;

/// Streams a complete PDF file one [`Body`] at a time to `W: std::io::Write` (no `Seek`).
///
/// Unencrypted writes produce the **exact same bytes** as [`full`](super::full) when given the
/// same bodies in the same order, trailer, and [`Options`], computing `/ID[1]` incrementally via
/// SHA-256 over every byte written before the cross-reference section. Unencrypted new files omit
/// `/ID` unless `trailer` supplies `/ID[0]`; encrypted files without `/ID[0]` use the SHA-256
/// digest for both elements, and draw fresh operating system IVs for AES ciphertexts.
///
/// Object streams ([`Options::pack`]) require buffering members across bodies and are rejected with
/// [`Error::PackedStream`] (or [`Error::PackedTable`] when paired with [`XrefKind::Table`]).
///
/// ```
/// use std::borrow::Cow;
///
/// use pdf_structure::pdf_objects::{Object, ObjectId, dict};
/// use pdf_structure::write::{self, Body, Options, Writer};
///
/// let catalog = Object::from(dict! {
///     "Type" => Object::name("Catalog"),
///     "Pages" => Object::reference(2),
/// });
/// let pages = Object::from(dict! {
///     "Type" => Object::name("Pages"),
///     "Kids" => Vec::<Object>::new(),
///     "Count" => 0,
/// });
/// let trailer = dict! { "Root" => Object::reference(1) };
///
/// let mut streamed = Vec::new();
/// let mut writer = Writer::begin(&mut streamed, None, &Options::DEFAULT).unwrap();
/// writer.push(ObjectId::new(1, 0), Body::New(Cow::Owned(catalog.clone()))).unwrap();
/// writer.push(ObjectId::new(2, 0), Body::New(Cow::Owned(pages.clone()))).unwrap();
/// writer.finish(&trailer).unwrap();
///
/// let mut whole = Vec::new();
/// write::full(
///     &mut whole,
///     [
///         (ObjectId::new(1, 0), Body::New(Cow::Owned(catalog))),
///         (ObjectId::new(2, 0), Body::New(Cow::Owned(pages))),
///     ],
///     &trailer,
///     None,
///     None,
///     &Options::DEFAULT,
/// )
/// .unwrap();
/// assert_eq!(streamed, whole);
/// ```
pub struct Writer<'w, W> {
    out: W,
    parts: Parts<'w>,
    encrypting: Option<&'w Encrypting<'w>>,
    fixed_encrypt_at: Option<ObjectId>,
    options: Options<'w>,
    section: Section,
    next: u32,
    given: HashSet<u32>,
    taken: HashSet<u32>,
    referenced: HashSet<u32>,
    dangling_extra: Vec<u32>,
    hasher: Sha256,
    scratch: Vec<u8>,
    written_bytes: usize,
}

impl<W> fmt::Debug for Writer<'_, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer")
            .field("bytes_written", &self.written_bytes)
            .field("objects_written", &self.taken.len())
            .field("encrypted", &self.encrypting.is_some())
            .finish_non_exhaustive()
    }
}

impl<'w, W: Write> Writer<'w, W> {
    /// Starts streaming a new unencrypted PDF file to `out`, writing the `%PDF-` header and binary
    /// marker immediately.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PackedTable`] or [`Error::PackedStream`] if `options.pack` is set,
    /// [`Error::Changed`] or [`Error::Io`] if `from` fails source verification, or
    /// [`Error::WriteIo`] if writing the header to `out` fails.
    pub fn begin(
        out: W,
        from: Option<&'w dyn ReadFrom>,
        options: &Options<'w>,
    ) -> Result<Writer<'w, W>, Error> {
        phase!(DEBUG, "writer_begin");
        if options.pack.is_some() {
            return Err(match options.kind {
                XrefKind::Table => Error::PackedTable,
                XrefKind::Stream => Error::PackedStream,
            });
        }
        let parts = Parts::plain(from, options.renumber)?;
        Writer::init(out, parts, None, None, Vec::new(), *options)
    }

    /// Starts streaming a new encrypted PDF file to `out` under `encrypting`
    /// ([`Encryptor::laid_out`]), writing the `%PDF-` header and binary marker immediately.
    ///
    /// Unlike [`full`](super::full), which inspects all bodies up front to relocate RC4 keystream
    /// collisions to fresh generations, streaming writes encrypt each pushed object at its target
    /// `(number, generation)` directly (refusing reused RC4 keystreams with
    /// [`Unencryptable::Keystream`](super::Unencryptable::Keystream) unless
    /// [`Encryptor::reusing_rc4_keystreams`] is set).
    ///
    /// # Errors
    ///
    /// Returns [`Error::PackedTable`] or [`Error::PackedStream`] if `options.pack` is active,
    /// [`Error::Number`] if the encryption dictionary's target object number is unusable,
    /// [`Error::Changed`] or [`Error::Io`] if `from` fails source verification, or
    /// [`Error::WriteIo`] if writing the header to `out` fails.
    pub fn begin_encrypted(
        out: W,
        encrypting: &'w Encrypting<'w>,
        from: Option<&'w dyn ReadFrom>,
        options: &Options<'w>,
    ) -> Result<Writer<'w, W>, Error> {
        phase!(DEBUG, "writer_begin_encrypted");
        let enc = encrypting.encryptor();
        if options.pack.is_some() {
            if options.kind == XrefKind::Table {
                return Err(Error::PackedTable);
            }
            if enc.packs() {
                return Err(Error::PackedStream);
            }
        }
        let fixed_encrypt_at = match enc.place() {
            Place::Direct => None,
            Place::At(_) => encrypting.at(),
            Place::New => encrypting.at().or(options.encrypt_at),
        };
        if let Some(at) = fixed_encrypt_at {
            usable(at.number)?;
        }
        let parts = Parts::new(encrypting, from)?;
        let dangling_extra: Vec<u32> = enc.dictionary_numbers(Numbering(None)).collect();
        Writer::init(
            out,
            parts,
            Some(encrypting),
            fixed_encrypt_at,
            dangling_extra,
            *options,
        )
    }

    /// Starts streaming an unencrypted incremental update after `base` to `out`
    /// ([`UpdateWriter::begin`]).
    ///
    /// # Errors
    ///
    /// Fails under the same conditions as [`UpdateWriter::begin`].
    pub fn begin_update(
        out: W,
        base: &'w Base,
        from: Option<&'w dyn ReadFrom>,
        options: &Options<'w>,
    ) -> Result<UpdateWriter<'w, W>, Error> {
        UpdateWriter::begin(out, base, from, options)
    }

    /// Starts streaming an encrypted incremental update after `base` to `out`
    /// ([`UpdateWriter::begin_encrypted`]).
    ///
    /// # Errors
    ///
    /// Fails under the same conditions as [`UpdateWriter::begin_encrypted`].
    pub fn begin_update_encrypted(
        out: W,
        base: &'w Base,
        encryptor: &'w Encryptor,
        resolve: &'w (dyn Resolve<'w> + Sync),
        from: Option<&'w dyn ReadFrom>,
        options: &Options<'w>,
    ) -> Result<UpdateWriter<'w, W>, Error> {
        UpdateWriter::begin_encrypted(out, base, encryptor, resolve, from, options)
    }

    fn init(
        out: W,
        parts: Parts<'w>,
        encrypting: Option<&'w Encrypting<'w>>,
        fixed_encrypt_at: Option<ObjectId>,
        dangling_extra: Vec<u32>,
        options: Options<'w>,
    ) -> Result<Writer<'w, W>, Error> {
        let mut writer = Writer {
            out,
            parts,
            encrypting,
            fixed_encrypt_at,
            options,
            section: Section::full(1),
            next: 1,
            given: HashSet::new(),
            taken: HashSet::new(),
            referenced: HashSet::new(),
            dangling_extra,
            hasher: Sha256::new(),
            scratch: Vec::new(),
            written_bytes: 0,
        };
        writer
            .scratch
            .extend_from_slice(format!("%PDF-{}\n", options.version).as_bytes());
        writer.scratch.extend_from_slice(BINARY);
        writer.flush_hashed()?;
        Ok(writer)
    }

    /// Total bytes written to the underlying `Write` sink so far.
    #[must_use]
    pub const fn bytes_written(&self) -> usize {
        self.written_bytes
    }

    /// Byte offset (counted from `%PDF-`) where the next pushed indirect object will start.
    #[must_use]
    pub const fn next_offset(&self) -> usize {
        self.written_bytes
    }

    #[cfg(test)]
    pub(crate) fn scratch_capacity(&self) -> usize {
        self.scratch.capacity()
    }

    /// Serializes and flushes `body` at `id` (or its renumbered target), dropping `body`
    /// immediately and returning the byte offset from `%PDF-` where `n g obj` was written.
    ///
    /// # Errors
    ///
    /// Fails under the same per-object conditions as [`full`](super::full), or with
    /// [`Error::WriteIo`] if flushing to the underlying writer fails.
    pub fn push(&mut self, id: ObjectId, body: Body<'_>) -> Result<usize, Error> {
        stopped(self.options.stop)?;
        let to = self.parts.numbering();
        if to.0.is_some() && !self.given.contains(&id.number) {
            // Delay insertion until validation passes so a failed push doesn't poison state if inspected.
        } else if to.0.is_some() {
            return Err(Error::Twice(id.number));
        }
        let at = to.at(id).ok_or(Error::Unnumbered(id))?;
        usable(at.number)?;
        if self.taken.contains(&at.number) {
            return Err(match to.0 {
                None => Error::Twice(at.number),
                Some(_) => Error::WrittenTwice(at.number),
            });
        }
        if self.fixed_encrypt_at.map(|e| e.number) == Some(at.number) {
            return Err(Error::EncryptAt(at.number));
        }
        if let Some(e) = self.encrypting {
            let enc = e.encryptor();
            if !enc.reusing() && enc.free_below(at) {
                return Err(Error::Free(at));
            }
            if let Some(moved) = enc.shared_with_read(to, id, at) {
                return Err(Error::Moved(moved));
            }
        }

        self.scratch.clear();
        self.parts.body(&mut self.scratch, id, at, &body)?;
        let pos = self.written_bytes;
        record_references(&mut self.referenced, body.object(), to);
        drop(body);
        self.flush_hashed()?;

        if to.0.is_some() {
            self.given.insert(id.number);
        }
        self.taken.insert(at.number);
        self.next = self.next.max(at.number + 1);
        self.section.insert(
            at.number,
            Entry::InUse {
                offset: pos,
                generation: at.generation,
            },
        );
        Ok(pos)
    }

    /// Writes any encryption/`/Info` objects, computes `/ID`, writes the cross-reference section
    /// and `%%EOF`, and returns [`Written`].
    ///
    /// # Errors
    ///
    /// Fails if trailer/cross-reference serialization fails, no usable object number remains, or
    /// writing to `W` fails ([`Error::WriteIo`]).
    pub fn finish(self, trailer: &Dict) -> Result<Written, Error> {
        self.finish_into(trailer).map(|(_, written)| written)
    }

    /// Like [`Writer::finish`], returning the inner writer `W` alongside [`Written`].
    ///
    /// # Errors
    ///
    /// Fails under the same conditions as [`Writer::finish`].
    pub fn finish_into(mut self, trailer: &Dict) -> Result<(W, Written), Error> {
        phase!(DEBUG, "writer_finish", objects = self.taken.len());
        let to = self.parts.numbering();
        record_dict_references(&mut self.referenced, trailer, to);
        for n in &self.taken {
            self.referenced.remove(n);
        }
        self.referenced.extend(self.dangling_extra.iter().copied());

        let given_id = first_id(trailer);
        let info_direct = (trailer.get(b"Info"))
            .is_some_and(|info| self.encrypting.is_some() && direct_info(info));
        let mut trailer = trailer_entries(trailer, to.0.unwrap_or(&()));

        if let Some(e) = self.encrypting {
            let encrypt_at = match self.fixed_encrypt_at {
                Some(at) => Some(at),
                None if e.encryptor().place() == Place::New => {
                    let at = fresh(self.next, |n| {
                        self.referenced.contains(&n) || self.section.lists(n)
                    });
                    usable(at)?;
                    Some(ObjectId::new(at, 0))
                }
                None => None,
            };
            match encrypt_at {
                None => trailer.set("Encrypt", e.dictionary().clone()),
                Some(at) => {
                    let dict = Object::from(e.dictionary().clone());
                    self.scratch.clear();
                    serialize::indirect(&mut self.scratch, at, &dict).map_err(Error::Serialize)?;
                    let pos = self.written_bytes;
                    self.flush_hashed()?;
                    self.section.insert(
                        at.number,
                        Entry::InUse {
                            offset: pos,
                            generation: at.generation,
                        },
                    );
                    trailer.set("Encrypt", Object::from(at));
                    self.next = self.next.max(at.number + 1);
                }
            }
        }

        let moved_info = info_direct.then(|| trailer.get(b"Info").cloned()).flatten();
        if let (Some(e), Some(info)) = (self.encrypting, moved_info) {
            let past = self.next.max(e.encryptor().past());
            let at = fresh(past, |n| {
                self.referenced.contains(&n) || self.section.lists(n)
            });
            usable(at)?;
            let at = ObjectId::new(at, 0);
            self.scratch.clear();
            serialize::indirect_with(&mut self.scratch, at, &info, e)?;
            let pos = self.written_bytes;
            self.flush_hashed()?;
            self.section.insert(
                at.number,
                Entry::InUse {
                    offset: pos,
                    generation: 0,
                },
            );
            trailer.set("Info", Object::from(at));
            self.next = self.next.max(at.number + 1);
        }

        if let Some(e) = self.encrypting {
            for (n, generation) in e.encryptor().listed() {
                if n < NUMBERS && !self.section.lists(n) {
                    self.section.insert(n, Entry::Free { generation });
                }
            }
        }

        let kept_id = (self
            .encrypting
            .and_then(Encrypting::first_id)
            .map(<[u8]>::to_vec))
        .or(given_id);
        let id = finish_file_id(kept_id, self.encrypting.is_some(), self.hasher.clone());
        if let Some(id) = &id {
            trailer.set("ID", id_value(id));
        }

        let pos = self.written_bytes;
        let mut kept = trailer.clone();
        self.scratch.clear();
        match self.options.kind {
            XrefKind::Table => self.section.table(&mut self.scratch, trailer, pos)?,
            XrefKind::Stream => {
                let at = fresh(self.next.max(self.options.xref_from), |n| {
                    self.referenced.contains(&n) || self.section.lists(n)
                });
                usable(at)?;
                self.section
                    .stream(&mut self.scratch, at, trailer, pos, self.options.deflate)?;
            }
        }
        self.flush_unhashed()?;

        let size = self.section.size();
        kept.set("Size", i64::from(size));
        let keystreams_reused = self
            .encrypting
            .map(Encrypting::keystreams_reused)
            .unwrap_or_default();
        let base = Base {
            len: self.written_bytes,
            ends_line: true,
            header: 0,
            prev: pos,
            size,
            kind: self.options.kind,
            first_id: id.as_ref().map(|[first, _]| first.clone()),
            trailer: kept,
            encryption: self.encrypting.map(|e| e.dictionary().clone()),
            key: self.encrypting.map(|e| e.encryptor().key_check()),
            source: None,
            renumbered: to.0.is_some(),
            startxref: None,
            shifted: Vec::new(),
            wrote: BTreeMap::new(),
        };
        let written = Written {
            section: self.section,
            start: pos,
            size,
            id,
            base,
            regenerated: Vec::new(),
            keystreams_reused,
        };
        Ok((self.out, written))
    }

    fn flush_hashed(&mut self) -> Result<(), Error> {
        self.hasher.update(&self.scratch);
        self.flush_unhashed()
    }

    fn flush_unhashed(&mut self) -> Result<(), Error> {
        self.out
            .write_all(&self.scratch)
            .map_err(|e| Error::write_io(&e))?;
        self.written_bytes += self.scratch.len();
        self.scratch.clear();
        if self.scratch.capacity() > MAX_SCRATCH {
            self.scratch.shrink_to(MAX_SCRATCH);
        }
        Ok(())
    }
}

/// Streams an incremental PDF update after a [`Base`] one [`Body`] at a time to `W: std::io::Write`
/// (no `Seek`).
///
/// Unchanged [`Body::Read`] objects from `base`'s own reading are passed over without writing
/// anything, and if every pushed object is passed over (or none are pushed), [`finish`](Self::finish)
/// returns `Ok(None)` having written zero bytes to `W`. When objects are written, unencrypted
/// output is byte-for-byte equal to [`update`](super::update) for the same bodies in the same
/// order.
///
/// ```
/// use std::borrow::Cow;
///
/// use pdf_structure::pdf_objects::{Object, ObjectId, dict};
/// use pdf_structure::read::{Options as ReadOptions, Reading};
/// use pdf_structure::write::{self, Base, Body, Options, UpdateWriter};
///
/// let initial = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
///     2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n\
///     xref\n0 3\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n\
///     trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n115\n%%EOF\n";
/// let reading = Reading::open(&initial[..], &ReadOptions::DEFAULT).unwrap();
/// let base = Base::of(&reading).unwrap();
///
/// let new_pages = Object::from(dict! {
///     "Type" => Object::name("Pages"),
///     "Kids" => Vec::<Object>::new(),
///     "Count" => 0,
/// });
///
/// let mut streamed = initial.to_vec();
/// let mut uw = UpdateWriter::begin(&mut streamed, &base, Some(&reading), &Options::DEFAULT).unwrap();
/// uw.push(ObjectId::new(2, 0), Body::New(Cow::Owned(new_pages.clone()))).unwrap();
/// uw.finish().unwrap();
///
/// let mut batch = initial.to_vec();
/// write::update(
///     &mut batch,
///     &base,
///     [(ObjectId::new(2, 0), Body::New(Cow::Owned(new_pages)))],
///     None,
///     Some(&reading),
///     &Options::DEFAULT,
/// )
/// .unwrap();
/// assert_eq!(streamed, batch);
/// ```
pub struct UpdateWriter<'w, W> {
    out: W,
    base: &'w Base,
    encryptor: Option<&'w Encryptor>,
    encrypting: Option<Encrypting<'w>>,
    from: Option<&'w dyn ReadFrom>,
    copying: Option<Copying<'w>>,
    to: Numbering<'w>,
    options: Options<'w>,
    section: Section,
    started: bool,
    passed: HashSet<u32>,
    given: HashSet<u32>,
    taken: HashSet<u32>,
    written_ids: Vec<ObjectId>,
    referenced: HashSet<u32>,
    hasher: Sha256,
    scratch: Vec<u8>,
    written_bytes: usize,
}

impl<W> fmt::Debug for UpdateWriter<'_, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateWriter")
            .field("started", &self.started)
            .field("bytes_written", &self.written_bytes)
            .field("objects_written", &self.written_ids.len())
            .field("encrypted", &self.encryptor.is_some())
            .finish_non_exhaustive()
    }
}

impl<'w, W: Write> UpdateWriter<'w, W> {
    /// Starts streaming an unencrypted incremental update after `base` to `out`.
    ///
    /// Nothing is written to `out` until the first modified object is pushed, so an update whose
    /// pushed objects are all unchanged [`Body::Read`] entries leaves `out` untouched.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Renumbered`] if `base` was renumbered and `from` is `Some`,
    /// [`Error::Rootless`] if `base`'s trailer lacks `/Root`, [`Error::Unmatched`] if `base` is
    /// encrypted, or [`Error::Changed`] / [`Error::Io`] if `from` fails source verification.
    pub fn begin(
        out: W,
        base: &'w Base,
        from: Option<&'w dyn ReadFrom>,
        options: &Options<'w>,
    ) -> Result<UpdateWriter<'w, W>, Error> {
        phase!(DEBUG, "update_writer_begin", at = base.len);
        if base.renumbered && from.is_some() {
            return Err(Error::Renumbered);
        }
        if base.trailer.get(b"Root").is_none() {
            return Err(Error::Rootless);
        }
        if !matched(base, None) {
            return Err(Error::Unmatched);
        }
        let copying = from.map(|from| Copying::new(from, None)).transpose()?;
        Ok(UpdateWriter {
            out,
            base,
            encryptor: None,
            encrypting: None,
            from,
            copying,
            to: Numbering(options.renumber),
            options: *options,
            section: Section::append(base.size, base.header),
            started: false,
            passed: HashSet::new(),
            given: HashSet::new(),
            taken: HashSet::new(),
            written_ids: Vec::new(),
            referenced: HashSet::new(),
            hasher: Sha256::new(),
            scratch: Vec::new(),
            written_bytes: 0,
        })
    }

    /// Starts streaming an encrypted incremental update after `base` to `out` under `encryptor`,
    /// resolving indirect catalog/metadata or crypt-filter references through `resolve`.
    ///
    /// # Errors
    ///
    /// Fails under the same conditions as [`UpdateWriter::begin`], plus [`Error::Unmatched`] if
    /// `encryptor` does not match `base`, [`Error::Moved`] if `options.renumber` moves an
    /// encryption dictionary object, or [`Error::Catalog`] if `/EncryptMetadata false` requires a
    /// catalog that `resolve` does not supply.
    pub fn begin_encrypted(
        out: W,
        base: &'w Base,
        encryptor: &'w Encryptor,
        resolve: &'w (dyn Resolve<'w> + Sync),
        from: Option<&'w dyn ReadFrom>,
        options: &Options<'w>,
    ) -> Result<UpdateWriter<'w, W>, Error> {
        phase!(DEBUG, "update_writer_begin_encrypted", at = base.len);
        if base.renumbered && from.is_some() {
            return Err(Error::Renumbered);
        }
        if base.trailer.get(b"Root").is_none() {
            return Err(Error::Rootless);
        }
        if !matched(base, Some(encryptor)) {
            return Err(Error::Unmatched);
        }
        let to = Numbering(options.renumber);
        if let Some(id) = encryptor.moved(to, std::iter::empty()) {
            return Err(Error::Moved(id));
        }
        let copying = from
            .map(|from| Copying::new(from, Some(encryptor)))
            .transpose()?;
        let guard = Guard::update(encryptor, base, from);
        let encrypting = encryptor
            .encrypting_at(&base.trailer, resolve, to)?
            .guarded(guard);
        Ok(UpdateWriter {
            out,
            base,
            encryptor: Some(encryptor),
            encrypting: Some(encrypting),
            from,
            copying,
            to,
            options: *options,
            section: Section::append(base.size, base.header),
            started: false,
            passed: HashSet::new(),
            given: HashSet::new(),
            taken: HashSet::new(),
            written_ids: Vec::new(),
            referenced: HashSet::new(),
            hasher: Sha256::new(),
            scratch: Vec::new(),
            written_bytes: 0,
        })
    }

    /// Bytes written to the underlying `Write` sink by this update so far.
    #[must_use]
    pub const fn bytes_written(&self) -> usize {
        self.written_bytes
    }

    /// File byte position (from byte 0 of the base file, including `base.len` and any pending
    /// newline before the first object) where the next modified object will start.
    #[must_use]
    pub const fn next_offset(&self) -> usize {
        let pending_newline = if !self.started && !self.base.ends_line {
            1
        } else {
            0
        };
        self.base.len + self.written_bytes + pending_newline
    }

    #[cfg(test)]
    pub(crate) fn scratch_capacity(&self) -> usize {
        self.scratch.capacity()
    }

    /// Serializes and flushes `body` at `id` (returning `Ok(Some(file_pos))`), or passes over it
    /// unchanged when `body` is an unmodified [`Body::Read`] of `base`'s reading (returning
    /// `Ok(None)`).
    ///
    /// # Errors
    ///
    /// Fails under the same per-object conditions as [`update`](super::update), or with
    /// [`Error::WriteIo`] if flushing to `W` fails.
    pub fn push(&mut self, id: ObjectId, body: Body<'_>) -> Result<Option<usize>, Error> {
        let is_held = match (&body, self.base.source) {
            (Body::Read { .. }, _) if self.base.renumbered => return Err(Error::Renumbered),
            (Body::Read { object, record }, Some(source)) if record.source == source => {
                as_read(id, object, record, source)?
            }
            _ => false,
        };
        let stays = is_held
            && match self.to.0 {
                None => true,
                Some(_) => {
                    self.to.at(id) == Some(id)
                        && body.object().references().all(|r| self.to.at(r) == Some(r))
                }
            };
        if stays {
            if self.passed.contains(&id.number) || self.given.contains(&id.number) {
                return Err(Error::Twice(id.number));
            }
            if self.taken.contains(&id.number) {
                return Err(match self.to.0 {
                    None => Error::Twice(id.number),
                    Some(_) => Error::WrittenTwice(id.number),
                });
            }
            self.passed.insert(id.number);
            return Ok(None);
        }

        stopped(self.options.stop)?;
        if self.passed.contains(&id.number)
            || (self.to.0.is_some() && self.given.contains(&id.number))
        {
            return Err(Error::Twice(id.number));
        }
        let written_at = self.to.at(id).ok_or(Error::Unnumbered(id))?;
        usable(written_at.number)?;
        if self.taken.contains(&written_at.number) || self.passed.contains(&written_at.number) {
            return Err(match self.to.0 {
                None => Error::Twice(written_at.number),
                Some(_) => Error::WrittenTwice(written_at.number),
            });
        }
        let encrypt_at = self.encryptor.and_then(Encryptor::at).map(|at| at.number);
        if encrypt_at == Some(written_at.number) {
            return Err(Error::EncryptAt(written_at.number));
        }
        if let Some(e) = self.encryptor {
            if !e.reusing() && e.free_below(written_at) {
                return Err(Error::Free(written_at));
            }
            if let Some(moved) = e.moved(self.to, [(id, written_at)]) {
                return Err(Error::Moved(moved));
            }
        }

        self.scratch.clear();
        written(
            &mut self.scratch,
            id,
            written_at,
            &body,
            self.encrypting.as_ref(),
            self.from,
            self.copying.as_ref(),
            self.to,
        )?;
        record_references(&mut self.referenced, body.object(), self.to);
        drop(body);

        if !self.started {
            if !self.base.ends_line {
                self.hasher.update(b"\n");
                self.out.write_all(b"\n").map_err(|e| Error::write_io(&e))?;
                self.written_bytes += 1;
            }
            self.started = true;
        }

        let pos = self.base.len + self.written_bytes;
        self.flush_hashed()?;

        self.given.insert(id.number);
        self.taken.insert(written_at.number);
        self.written_ids.push(written_at);
        self.section.insert(
            written_at.number,
            Entry::InUse {
                offset: pos,
                generation: written_at.generation,
            },
        );
        Ok(Some(pos))
    }

    /// Finishes the incremental update, returning `Ok(None)` without writing anything if no
    /// modified objects were pushed, or writing the trailer, cross-reference section, and `%%EOF`
    /// and returning `Ok(Some(Written))`.
    ///
    /// # Errors
    ///
    /// Fails if trailer/cross-reference serialization fails, no object number below [`NUMBERS`]
    /// remains, or writing to `W` fails ([`Error::WriteIo`]).
    pub fn finish(self) -> Result<Option<Written>, Error> {
        self.finish_into().map(|(_, written)| written)
    }

    /// Like [`UpdateWriter::finish`], returning the inner writer `W` alongside `Option<Written>`.
    ///
    /// # Errors
    ///
    /// Fails under the same conditions as [`UpdateWriter::finish`].
    pub fn finish_into(mut self) -> Result<(W, Option<Written>), Error> {
        if !self.started {
            return Ok((self.out, None));
        }
        phase!(
            DEBUG,
            "update_writer_finish",
            objects = self.written_ids.len()
        );
        let Some(root) = self.base.trailer.get(b"Root") else {
            return Err(Error::Rootless);
        };
        record_dict_references(&mut self.referenced, &self.base.trailer, self.to);
        for n in &self.taken {
            self.referenced.remove(n);
        }

        let to = self.to;
        let named = |value: &Object| {
            let mut value = value.clone();
            if let Some(to) = to.0 {
                value.renumber(|id| to.renumber(id));
            }
            value
        };

        let mut info_at = None;
        let info = match (self.base.trailer.get(b"Info"), &self.encrypting) {
            (Some(info), Some(e)) if direct_info(info) => {
                let past = self.base.size.max(self.section.size());
                let num = fresh(past, |n| self.referenced.contains(&n));
                if num >= NUMBERS {
                    return Err(Error::NoNumber(num));
                }
                let id = ObjectId::new(num, 0);
                let pos = self.base.len + self.written_bytes;
                self.scratch.clear();
                serialize::indirect_with(&mut self.scratch, id, &named(info), e)?;
                self.flush_hashed()?;
                self.section.insert(
                    num,
                    Entry::InUse {
                        offset: pos,
                        generation: 0,
                    },
                );
                info_at = Some(id);
                Some(Object::from(id))
            }
            (info, _) => info.map(named),
        };

        for &(number, generation, offset) in &self.base.shifted {
            if !self.section.lists(number) {
                self.section
                    .insert(number, Entry::InUse { offset, generation });
            }
        }

        let mut trailer = Dict::new();
        trailer.set("Root", named(root));
        trailer.set("Prev", self.base.prev as i64);
        if let Some(info) = info {
            trailer.set("Info", info);
        }
        if let Some(e) = self.encryptor {
            match e.place() {
                Place::At(at) => trailer.set("Encrypt", Object::from(at)),
                Place::Direct | Place::New => trailer.set("Encrypt", e.dictionary().clone()),
            }
        }
        let kept_id = (self
            .encryptor
            .and_then(Encryptor::first_id)
            .map(<[u8]>::to_vec))
        .or_else(|| self.base.first_id.clone());
        let id = finish_file_id(kept_id, self.encryptor.is_some(), self.hasher.clone());
        if let Some(id) = &id {
            trailer.set("ID", id_value(id));
        }
        let carried = (self.base.trailer.iter())
            .filter(|(key, _)| !SECTION_KEYS.contains(key) && !matches!(*key, b"Root" | b"Info"));
        for (key, value) in carried {
            trailer.set(key, named(value));
        }

        let pos = self.base.len + self.written_bytes;
        let mut kept = trailer.clone();
        self.scratch.clear();
        match self.base.kind {
            XrefKind::Table => self.section.table(&mut self.scratch, trailer, pos)?,
            XrefKind::Stream => {
                let past = [self.options.xref_from, self.base.size, self.section.size()]
                    .into_iter()
                    .fold(0, u32::max);
                let num = fresh(past, |n| self.referenced.contains(&n));
                if num >= NUMBERS {
                    return Err(Error::NoNumber(num));
                }
                self.section
                    .stream(&mut self.scratch, num, trailer, pos, self.options.deflate)?;
            }
        }
        self.flush_unhashed()?;

        let size = self.section.size();
        kept.set("Size", i64::from(size));
        let prev = pos - self.base.header;

        let freed: Vec<ObjectId> = match self.encryptor {
            Some(e) if e.reusing() => (self.written_ids.iter().copied())
                .filter(|&at| e.free_below(at))
                .collect(),
            _ => Vec::new(),
        };
        let mut keystreams_reused = self
            .encrypting
            .as_ref()
            .map(Encrypting::keystreams_reused)
            .unwrap_or_default();
        keystreams_reused.extend(freed);
        keystreams_reused.sort_unstable();
        keystreams_reused.dedup();

        let mut wrote = self.base.wrote.clone();
        if self.encryptor.is_some_and(Encryptor::rc4) {
            for at in self.written_ids.iter().copied().chain(info_at) {
                let had = wrote.entry(at.number).or_insert(at.generation);
                *had = (*had).max(at.generation);
            }
        }

        let next_base = Base {
            len: self.base.len + self.written_bytes,
            ends_line: true,
            header: self.base.header,
            prev,
            size,
            kind: self.base.kind,
            first_id: (id.as_ref().map(|[first, _]| first.clone()))
                .or_else(|| self.base.first_id.clone()),
            trailer: kept,
            encryption: self.base.encryption.clone(),
            source: self.base.source,
            renumbered: self.base.renumbered || to.0.is_some(),
            startxref: None,
            shifted: Vec::new(),
            key: self.base.key.clone(),
            wrote,
        };
        let written = Written {
            section: self.section,
            start: prev,
            size,
            id,
            base: next_base,
            regenerated: Vec::new(),
            keystreams_reused,
        };
        Ok((self.out, Some(written)))
    }

    fn flush_hashed(&mut self) -> Result<(), Error> {
        self.hasher.update(&self.scratch);
        self.flush_unhashed()
    }

    fn flush_unhashed(&mut self) -> Result<(), Error> {
        self.out
            .write_all(&self.scratch)
            .map_err(|e| Error::write_io(&e))?;
        self.written_bytes += self.scratch.len();
        self.scratch.clear();
        if self.scratch.capacity() > MAX_SCRATCH {
            self.scratch.shrink_to(MAX_SCRATCH);
        }
        Ok(())
    }
}

/// Computes trailer `/ID` from an incremental `Sha256` state, matching [`super::file_id`]
/// byte-for-byte.
fn finish_file_id(kept: Option<Vec<u8>>, encrypted: bool, hasher: Sha256) -> Option<[Vec<u8>; 2]> {
    if kept.is_none() && !encrypted {
        return None;
    }
    let hash = hasher.finalize()[..16].to_vec();
    let first = kept.unwrap_or_else(|| hash.clone());
    Some([first, hash])
}

/// Walks `root` iteratively (avoiding recursion on deeply nested values) and records every
/// referenced object number after mapping through `to`.
fn record_references(referenced: &mut HashSet<u32>, root: &Object, to: Numbering<'_>) {
    let mut stack = Vec::new();
    visit_object(referenced, &mut stack, root, to);
    drain_reference_stack(referenced, &mut stack, to);
}

/// Walks all values in `dict` iteratively and records every referenced object number after mapping
/// through `to`.
fn record_dict_references(referenced: &mut HashSet<u32>, dict: &Dict, to: Numbering<'_>) {
    let mut stack = Vec::new();
    for entry in dict.entries() {
        visit_object(referenced, &mut stack, entry.value(), to);
    }
    drain_reference_stack(referenced, &mut stack, to);
}

fn visit_object<'a>(
    referenced: &mut HashSet<u32>,
    stack: &mut Vec<&'a Object>,
    obj: &'a Object,
    to: Numbering<'_>,
) {
    match obj.value() {
        Value::Ref(id) => {
            if let Some(at) = to.at(*id) {
                referenced.insert(at.number);
            }
        }
        Value::Array(_) | Value::Dict(_) | Value::Stream(_) => stack.push(obj),
        _ => {}
    }
}

fn drain_reference_stack(
    referenced: &mut HashSet<u32>,
    stack: &mut Vec<&Object>,
    to: Numbering<'_>,
) {
    while let Some(object) = stack.pop() {
        match object.value() {
            Value::Array(array) => {
                for item in array {
                    visit_object(referenced, stack, item, to);
                }
            }
            Value::Dict(dict) => {
                for entry in dict.entries() {
                    visit_object(referenced, stack, entry.value(), to);
                }
            }
            Value::Stream(stream) => {
                for entry in stream.dict.entries() {
                    visit_object(referenced, stack, entry.value(), to);
                }
            }
            _ => {}
        }
    }
}
