//! What a font's reading goes through for what it reads of the file: objects and
//! streams, each with its reference followed, what doesn't read kept for the
//! reading's end, and faults told.

use std::cell::RefCell;

use pdf_objects::object::{Stream, Value};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Unreadable, Why};

use crate::{Decoded, Error, Fault, Limits, Place, Sink, Streams};

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

    /// `value` after one reference: PDFium's `GetDirect`, which is none for a
    /// reference to nothing, and which a reference to a reference leaves at a
    /// reference.
    pub(crate) fn hop(&self, value: &'o Object) -> Option<&'o Object> {
        match value.as_reference() {
            Some(id) => self.resolve(id),
            None => Some(value),
        }
    }

    /// `value` as a number's or a string's accessor sees it (`GetInteger`,
    /// `GetNumber`, `GetString`): a reference followed once, and no further, one to
    /// a reference being none.
    fn seen(&self, value: &'o Object) -> Option<&'o Object> {
        self.hop(value)
            .filter(|object| match object.as_reference() {
                Some(id) => {
                    self.tell(Some(id), None, Fault::Chained);
                    false
                }
                None => true,
            })
    }

    /// What PDFium's integer accessor reads of `value` ([`integer_of`]), told where
    /// it's no number, and in `object` where that's said.
    pub(crate) fn integer(&self, value: &'o Object, object: Option<ObjectId>) -> i32 {
        self.integer_telling(value, object, Fault::NotANumber)
    }

    /// [`Reader::integer`], `fault` what's told where it's no number.
    pub(crate) fn integer_telling(
        &self,
        value: &'o Object,
        object: Option<ObjectId>,
        fault: Fault,
    ) -> i32 {
        let (integer, number) = self.integer_and_whether_a_number(value);
        if !number {
            self.tell(object, Some(value), fault);
        }
        integer
    }

    /// [`Reader::integer`] of `value`, and whether that's a number, not what PDFium
    /// reads of what's no number.
    pub(crate) fn integer_and_whether_a_number(&self, value: &'o Object) -> (i32, bool) {
        let seen = self.seen(value);
        (
            seen.map_or(0, integer_of),
            seen.and_then(Object::as_number).is_some(),
        )
    }

    /// What PDFium's float accessor reads of `value` ([`float_of`]), told where it's
    /// no number.
    pub(crate) fn float(&self, value: &'o Object) -> f32 {
        let seen = self.seen(value);
        if seen.and_then(Object::as_number).is_none() {
            self.tell(None, Some(value), Fault::NotANumber);
        }
        seen.map_or(0.0, float_of)
    }

    /// [`Reader::integer`] of `dict`'s `key`, as PDFium's `GetIntegerFor`: `absent`
    /// where it has no such key, whatever the key holds being read otherwise.
    pub(crate) fn integer_for(
        &self,
        dict: &'o Dict,
        key: &[u8],
        absent: i32,
        object: Option<ObjectId>,
    ) -> i32 {
        dict.get(key)
            .map_or(absent, |value| self.integer(value, object))
    }

    /// [`Reader::integer`] of `array`'s element `i`, as PDFium's `GetIntegerAt`: 0
    /// past its end.
    pub(crate) fn integer_at(
        &self,
        array: &'o [Object],
        i: usize,
        object: Option<ObjectId>,
    ) -> i32 {
        array.get(i).map_or(0, |value| self.integer(value, object))
    }

    /// [`Reader::float`] of `array`'s element `i`, as PDFium's `GetFloatAt`: 0 past
    /// its end.
    pub(crate) fn float_at(&self, array: &'o [Object], i: usize) -> f32 {
        array.get(i).map_or(0.0, |value| self.float(value))
    }

    /// The bytes of the name or the string `dict`'s `key` is, as PDFium's
    /// `GetByteStringFor`; none of anything else, which it reads as bytes of its
    /// own text, none a font's reader looks for.
    pub(crate) fn byte_string(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o [u8]> {
        let seen = self.seen(dict.get(key)?)?;
        seen.as_name().or_else(|| seen.as_bytes())
    }

    /// The dictionary `value` is, as PDFium's `GetDict` of a value's direct object: a
    /// stream's among them, its reference followed, and one more where that's to a
    /// reference.
    pub(crate) fn dict_of_value(&self, value: &'o Object) -> Option<&'o Dict> {
        let object = self.hop(value)?;
        if let Some(id) = object.as_reference() {
            self.tell(Some(id), None, Fault::Chained);
        }
        own_dict(self.seen(object)?)
    }

    /// The dictionary `dict`'s `key` is ([`Reader::dict_of_value`]), as PDFium's
    /// `GetDictFor`.
    pub(crate) fn dict(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o Dict> {
        self.dict_of_value(dict.get(key)?)
    }

    /// The dictionary element `value` of an array is, as PDFium's `GetDictAt`: a
    /// stream's among them, its reference followed, and no further.
    pub(crate) fn dict_at(&self, value: &'o Object) -> Option<&'o Dict> {
        own_dict(self.hop(value)?)
    }

    /// The array `value` is, as PDFium's `GetArrayFor`: its reference followed, and
    /// no further.
    pub(crate) fn array(&self, value: &'o Object) -> Option<&'o [Object]> {
        let object = self.hop(value)?;
        if let Some(id) = object.as_reference() {
            self.tell(Some(id), None, Fault::Chained);
        }
        object.as_array()
    }

    /// The array `dict`'s `key` is ([`Reader::array`]).
    pub(crate) fn array_for(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o [Object]> {
        self.array(dict.get(key)?)
    }

    /// The stream `dict`'s `key` is, as PDFium's `GetStreamFor`: its reference
    /// followed, and no further.
    pub(crate) fn stream_for(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o Stream> {
        self.hop(dict.get(key)?)?.as_stream()
    }

    /// The stream object `id` is, with its own number, which is what its data is
    /// asked for by; none where it's anything else, a reference among them.
    pub(crate) fn stream(&self, id: ObjectId) -> Option<(ObjectId, &'o Stream)> {
        Some((id, self.resolve(id)?.as_stream()?))
    }

    /// The content of the stream object `id` is, decompressed within `budget`,
    /// charged to it ([`Reader::charged`]): None where it isn't a stream, or doesn't
    /// read.
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

/// The dictionary `object` is: no stream's, as PDFium's `ToDictionary`.
pub(crate) fn dict_of(object: &Object) -> Option<&Dict> {
    match object.value() {
        Value::Dict(dict) => Some(dict),
        _ => None,
    }
}

/// The dictionary `object` holds: its own, or a stream's, as PDFium's `GetDict`.
fn own_dict(object: &Object) -> Option<&Dict> {
    match object.value() {
        Value::Dict(dict) => Some(dict),
        Value::Stream(stream) => Some(&stream.dict),
        _ => None,
    }
}

/// An integer as PDFium's parser holds it: one with no sign in 32 bits unsigned, a
/// negative one in 32 bits signed, and past those 0. A positive one with a sign is
/// held by the first rule here, as an [`Object`] doesn't keep the sign; PDFium's has
/// only 31 bits.
fn held(n: i64) -> Number32 {
    if let Ok(unsigned) = u32::try_from(n) {
        Number32::Unsigned(unsigned)
    } else if let Ok(signed) = i32::try_from(n) {
        Number32::Signed(signed)
    } else {
        Number32::Unsigned(0)
    }
}

/// An integer as [`held`] holds it.
#[derive(Clone, Copy)]
enum Number32 {
    Unsigned(u32),
    Signed(i32),
}

/// What PDFium's integer accessor reads of `object`, itself no reference: an
/// integer as it holds it, past 31 bits with no sign wrapping to a negative; a real
/// cut from its 32-bit float, saturating; `true` as 1; anything else 0.
pub(crate) fn integer_of(object: &Object) -> i32 {
    match object.value() {
        Value::Integer(n) => match held(*n) {
            Number32::Unsigned(n) => n as i32,
            Number32::Signed(n) => n,
        },
        Value::Real(real) => *real as f32 as i32,
        Value::Bool(true) => 1,
        _ => 0,
    }
}

/// What PDFium's float accessor reads of `object`, itself no reference: a number's
/// 32-bit float, anything else 0, `true` too.
///
/// An [`Object`]'s real is an `f64`, parsed by the reader's rules, where PDFium parses
/// the bytes to a float itself: a real of many digits can round otherwise.
pub(crate) fn float_of(object: &Object) -> f32 {
    match object.value() {
        Value::Integer(n) => match held(*n) {
            Number32::Unsigned(n) => n as f32,
            Number32::Signed(n) => n as f32,
        },
        Value::Real(real) => *real as f32,
        _ => 0.0,
    }
}
