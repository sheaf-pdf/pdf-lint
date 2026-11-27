//! Lifetime-narrowing `Resolve`/`Streams` wrappers, collision-free ephemeral
//! object overlays, and `Sink` adapters for domain crate faults.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::marker::PhantomData;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{DecodedStream, Object, ObjectId, Resolve, Streams, Unreadable, Why};

use crate::fault::Fault;

/// Reserved generation number (`65535`, ISO 32000-2 §7.5.4 free-head generation)
/// used for ephemeral synthesized appearance resources, paired with an explicit
/// `base.resolve` collision check so caller objects can never be shadowed.
pub(crate) const EPHEMERAL_GEN: u16 = u16::MAX;

/// Covariant lifetime-narrowing adapter over `O: Resolve<'o> + Streams + Copy`
/// that preserves `Streams::decoded_standard` while allowing local shorter-lived
/// `Dict` / `Object` resource trees to be interpreted alongside `'o` indirect
/// objects.
#[derive(Clone, Copy)]
pub(crate) struct Narrow<'o, O> {
    inner: O,
    _lt: PhantomData<&'o ()>,
}

impl<'o, O> Narrow<'o, O> {
    pub(crate) const fn new(inner: O) -> Self {
        Self {
            inner,
            _lt: PhantomData,
        }
    }
}

impl<'a, 'o: 'a, O: Resolve<'o>> Resolve<'a> for Narrow<'o, O> {
    fn resolve(&self, id: ObjectId) -> Option<&'a Object> {
        self.inner.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'a Object>, Unreadable> {
        self.inner.try_resolve(id)
    }
}

impl<'o, O: Streams> Streams for Narrow<'o, O> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        self.inner.decoded(id, limit)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        self.inner.decoded_standard(id, limit)
    }
}

/// Overlay resolver/streams provider binding ephemeral synthesized indirect
/// objects over `base` without mutating or shadowing the caller's document.
#[derive(Clone, Copy)]
pub(crate) struct EphemeralOverlay<'a, O> {
    base: O,
    extra: &'a BTreeMap<ObjectId, Object>,
}

impl<'a, O> EphemeralOverlay<'a, O> {
    pub(crate) const fn new(base: O, extra: &'a BTreeMap<ObjectId, Object>) -> Self {
        Self { base, extra }
    }
}

/// Finds a contiguous block of `len` unallocated `(num, EPHEMERAL_GEN)` IDs
/// disjoint from both `base` and `extra`.
pub(crate) fn alloc_ephemeral_block<'o, O: Resolve<'o>>(
    base: O,
    extra: &BTreeMap<ObjectId, Object>,
    cursor: &mut u32,
    len: u32,
) -> Option<u32> {
    if len == 0 {
        return None;
    }
    loop {
        let start = *cursor;
        let end = start.checked_add(len)?;
        let conflict = (start..end).find(|&n| {
            let candidate = ObjectId::new(n, EPHEMERAL_GEN);
            extra.contains_key(&candidate) || base.resolve(candidate).is_some()
        });
        match conflict {
            Some(bad) => *cursor = bad.checked_add(1)?,
            None => {
                *cursor = end;
                return Some(start);
            }
        }
    }
}

impl<'o, 'a: 'o, O: Resolve<'o>> Resolve<'o> for EphemeralOverlay<'a, O> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        if id.generation == EPHEMERAL_GEN
            && let Some(obj) = self.extra.get(&id)
        {
            return Some(obj);
        }
        self.base.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        if id.generation == EPHEMERAL_GEN
            && let Some(obj) = self.extra.get(&id)
        {
            return Ok(Some(obj));
        }
        self.base.try_resolve(id)
    }
}

impl<'a, O: Streams> Streams for EphemeralOverlay<'a, O> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        if id.generation == EPHEMERAL_GEN
            && let Some(obj) = self.extra.get(&id)
        {
            let bytes = obj
                .as_stream()
                .and_then(|s| s.data.bytes())
                .ok_or_else(|| Unreadable::new(id, Why::Damaged, "not an in-memory stream"))?;
            if bytes.len() > limit {
                return Err(Unreadable::new(
                    id,
                    Why::Limit,
                    "ephemeral stream exceeds limit",
                ));
            }
            return Ok(Cow::Borrowed(bytes));
        }
        self.base.decoded(id, limit)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        if id.generation == EPHEMERAL_GEN
            && let Some(obj) = self.extra.get(&id)
        {
            let bytes = obj
                .as_stream()
                .and_then(|s| s.data.bytes())
                .ok_or_else(|| Unreadable::new(id, Why::Damaged, "not an in-memory stream"))?;
            if bytes.len() > limit {
                return Err(Unreadable::new(
                    id,
                    Why::Limit,
                    "ephemeral stream exceeds limit",
                ));
            }
            return Ok(DecodedStream::new(Cow::Borrowed(bytes), None));
        }
        self.base.decoded_standard(id, limit)
    }
}

pub(crate) struct EraseSink<'a, S: ?Sized>(pub(crate) &'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<Fault> for EraseSink<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.fault(at, fault);
    }
}

pub(crate) struct SharedSink<'s> {
    pub(crate) wants: bool,
    inner: RefCell<&'s mut dyn Sink<Fault>>,
}

impl<'s> SharedSink<'s> {
    pub(crate) fn new(sink: &'s mut dyn Sink<Fault>) -> Self {
        Self {
            wants: sink.wants(),
            inner: RefCell::new(sink),
        }
    }

    pub(crate) fn emit(&self, at: Place, fault: Fault) {
        if self.wants
            && let Ok(mut guard) = self.inner.try_borrow_mut()
        {
            guard.fault(at, fault);
        }
    }
}

pub(crate) struct GfxSinkAdapter<'a, 's>(pub(crate) &'a SharedSink<'s>);

impl Sink<pdf_gfx::Fault> for GfxSinkAdapter<'_, '_> {
    fn wants(&self) -> bool {
        self.0.wants
    }

    fn fault(&mut self, at: Place, fault: pdf_gfx::Fault) {
        self.0.emit(at, Fault::Gfx(fault));
    }
}

pub(crate) struct LocalFaultSink<'a, 's>(pub(crate) &'a SharedSink<'s>);

impl Sink<Fault> for LocalFaultSink<'_, '_> {
    fn wants(&self) -> bool {
        self.0.wants
    }

    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.emit(at, fault);
    }
}

pub(crate) struct ImageSinkAdapter<'a, S: ?Sized>(pub(crate) &'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_images::Fault> for ImageSinkAdapter<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_images::Fault) {
        self.0.fault(at, Fault::Image(fault));
    }
}

pub(crate) struct SemSinkAdapter<'a, S: ?Sized>(pub(crate) &'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_semantics::Fault> for SemSinkAdapter<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_semantics::Fault) {
        self.0.fault(at, Fault::Semantics(fault));
    }
}

pub(crate) struct AnnotSinkAdapter<'a, S: ?Sized>(pub(crate) &'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_annots::Fault> for AnnotSinkAdapter<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_annots::Fault) {
        self.0.fault(at, Fault::Annot(fault));
    }
}

#[cfg(feature = "text-layout")]
pub(crate) struct FormSinkAdapter<'a, S: ?Sized>(pub(crate) &'a mut S);

#[cfg(feature = "text-layout")]
impl<S: Sink<Fault> + ?Sized> Sink<pdf_forms::Fault> for FormSinkAdapter<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_forms::Fault) {
        self.0.fault(at, Fault::Form(fault));
    }
}
