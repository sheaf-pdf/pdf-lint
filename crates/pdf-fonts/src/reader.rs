//! What a font's reading goes through for what it reads of the file: objects and
//! streams, each with its reference followed, what doesn't read kept for the
//! reading's end, and faults told.

use std::cell::RefCell;

use pdf_objects::object::{Stream, Value};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Unreadable, Why};

use crate::{Decoded, Error, Fault, Limits, Place, Sink, Streams};

/// How many references in a row are followed from an object, the first after
/// it and each past that. A chain longer than this, a loop among them, reads as
/// none.
const CHAIN: usize = 65;

/// One font's reading: its objects, its caps and its sink, and the first object
/// that didn't read for a reason that's the caller's to know.
///
/// Where something fails to read the reading goes on as if it weren't there, and
/// its failure is what the reading comes to in the end ([`Reader::failure`]), so
/// that one path through it does for both, and whatever a reading of the same
/// objects comes to it comes to by the same steps.
pub(crate) struct Reader<'c, O, S: ?Sized> {
    objects: O,
    pub(crate) limits: &'c Limits,
    sink: RefCell<&'c mut S>,
    failed: RefCell<Option<Unreadable>>,
}

impl<'c, O, S: Sink + ?Sized> Reader<'c, O, S> {
    pub(crate) fn new(objects: O, limits: &'c Limits, sink: &'c mut S) -> Reader<'c, O, S> {
        Reader {
            objects,
            limits,
            sink: RefCell::new(sink),
            failed: RefCell::new(None),
        }
    }

    /// `fault`, in `object` where that's said, at where `value` was written where
    /// there's one.
    pub(crate) fn tell(&self, object: Option<ObjectId>, value: Option<&Object>, fault: Fault) {
        let mut sink = self.sink.borrow_mut();
        if sink.wants() {
            let at = value.and_then(|value| value.at);
            sink.fault(Place::new(object, at), fault);
        }
    }

    /// The first failure it met, for the end of a reading.
    pub(crate) fn failure(&self) -> Option<Unreadable> {
        self.failed.borrow_mut().take()
    }

    fn fail(&self, unreadable: Unreadable) {
        self.failed.borrow_mut().get_or_insert(unreadable);
    }
}

impl<'o, O: Resolve<'o> + Streams, S: Sink + ?Sized> Reader<'_, O, S> {
    /// Object `id`; None where it isn't there, or doesn't read.
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        match self.objects.try_resolve(id) {
            Ok(found) => found,
            Err(unreadable) if unreadable.why == Why::Damaged => {
                self.tell(Some(id), None, Fault::Damaged { id });
                None
            }
            Err(unreadable) => {
                self.fail(unreadable);
                None
            }
        }
    }

    /// `object`, or what its chain of references ends at.
    pub(crate) fn direct(&self, object: &'o Object) -> Option<&'o Object> {
        let mut object = object;
        let mut chain = 0;
        while let Some(id) = object.as_reference() {
            if chain == CHAIN {
                return None;
            }
            chain += 1;
            if chain == 2 {
                self.tell(Some(id), None, Fault::Chained);
            }
            object = self.resolve(id)?;
        }
        Some(object)
    }

    /// The name `dict`'s `key` is, its reference followed.
    pub(crate) fn get_name(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o [u8]> {
        self.direct(dict.get(key)?)?.as_name()
    }

    /// The dictionary `dict`'s `key` is, its reference followed.
    pub(crate) fn get_dict(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o Dict> {
        dict_of(self.direct(dict.get(key)?)?)
    }

    /// The array `dict`'s `key` is, its reference followed.
    pub(crate) fn get_array(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o [Object]> {
        self.direct(dict.get(key)?)?.as_array()
    }

    /// The whole number `dict`'s `key` is ([`Reader::whole`]), `dict` being
    /// `object` where it's a numbered one, told where it's no number.
    pub(crate) fn get_whole(
        &self,
        dict: &'o Dict,
        key: &[u8],
        object: Option<ObjectId>,
    ) -> Option<f32> {
        let value = dict.get(key)?;
        let whole = self.whole(value);
        if whole.is_none() {
            self.tell(object, Some(value), Fault::NotANumber);
        }
        whole
    }

    /// A number, cut to its whole part and kept within an `i32`, as PDFium reads
    /// one where it wants a whole one.
    pub(crate) fn integer(&self, object: &'o Object) -> Option<i64> {
        let n = self.direct(object)?.as_number()?;
        Some(n.trunc().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i64)
    }

    /// [`Reader::integer`], as a metric.
    pub(crate) fn whole(&self, object: &'o Object) -> Option<f32> {
        self.integer(object).map(|n| n as f32)
    }

    /// A number, as written.
    pub(crate) fn number_of(&self, object: &'o Object) -> Option<f32> {
        let n = self.direct(object)?.as_number()? as f32;
        n.is_finite().then_some(n)
    }

    /// The stream `id` is, or its chain of references ends at, with its own
    /// number, which is what its data is asked for by.
    pub(crate) fn stream(&self, id: ObjectId) -> Option<(ObjectId, &'o Stream)> {
        let mut id = id;
        for _ in 0..=CHAIN {
            let object = self.resolve(id)?;
            match object.as_reference() {
                Some(next) => id = next,
                None => return Some((id, object.as_stream()?)),
            }
        }
        None
    }

    /// The content of the stream `id` is or leads to, decompressed within
    /// `budget`, charged to it ([`Reader::charged`]): None where it isn't a
    /// stream, or doesn't read.
    pub(crate) fn decompressed(
        &self,
        id: ObjectId,
        budget: &mut usize,
    ) -> Result<Option<Vec<u8>>, Error> {
        match self.stream(id) {
            Some((id, _)) => self.charged(id, budget),
            None => Ok(None),
        }
    }

    /// The content of stream `id`, its own number, decompressed within `budget`
    /// and charged to it: every stream a font reads goes through here. None where
    /// it doesn't decode. Too large past `budget`, whatever gave it.
    pub(crate) fn charged(
        &self,
        id: ObjectId,
        budget: &mut usize,
    ) -> Result<Option<Vec<u8>>, Error> {
        // What's read after a failure counts for nothing: it needn't be decoded.
        if self.failed.borrow().is_some() {
            return Ok(None);
        }
        match self.objects.decoded(id, *budget) {
            Decoded::Data(data) => {
                *budget = budget.checked_sub(data.len()).ok_or(Error::TooLarge)?;
                Ok(Some(data))
            }
            Decoded::TooLarge => Err(Error::TooLarge),
            Decoded::None => Ok(None),
            Decoded::Unreadable(unreadable) if unreadable.why == Why::Damaged => {
                self.tell(Some(id), None, Fault::Damaged { id });
                Ok(None)
            }
            Decoded::Unreadable(unreadable) => {
                self.fail(unreadable);
                Ok(None)
            }
        }
    }
}

/// The dictionary `object` is: no stream's.
pub(crate) fn dict_of(object: &Object) -> Option<&Dict> {
    match object.value() {
        Value::Dict(dict) => Some(dict),
        _ => None,
    }
}
