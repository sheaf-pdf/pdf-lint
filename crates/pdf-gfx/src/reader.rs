//! Zero-I/O PDFium-compatible dictionary/array/stream accessors over
//! [`Resolve`] and [`Streams`].

use std::cell::RefCell;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::object::{Stream, Value};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable, Why};

use crate::fault::Fault;

/// Read helper over `O: Resolve + Streams` that matches PDFium's reference
/// hopping (`GetDirect`, `GetDict`, `GetArray`, `GetNumber`, `GetInteger`)
/// while distinguishing damaged file objects (`Why::Damaged` -> reported to
/// `Sink<Fault>` and treated as absent) from caller/reader failures
/// (`Why::Locked`, `Why::Capped`, `Why::Source`, `Why::Detached`).
pub(crate) struct ObjReader<'c, O, S: ?Sized> {
    pub(crate) objects: O,
    sink: RefCell<&'c mut S>,
    failed: RefCell<Option<Unreadable>>,
}

impl<'c, O, S: Sink<Fault> + ?Sized> ObjReader<'c, O, S> {
    pub(crate) fn new(objects: O, sink: &'c mut S) -> Self {
        Self {
            objects,
            sink: RefCell::new(sink),
            failed: RefCell::new(None),
        }
    }

    pub(crate) fn sink_wants(&self) -> bool {
        self.sink.borrow().wants() && self.failed.borrow().is_none()
    }

    pub(crate) fn emit_place(&self, place: Place, fault: Fault) {
        let mut sink = self.sink.borrow_mut();
        if sink.wants() && self.failed.borrow().is_none() {
            sink.fault(place, fault);
        }
    }

    pub(crate) fn tell(&self, object: Option<ObjectId>, fault: Fault) {
        self.emit_place(Place::new(object, None), fault);
    }

    pub(crate) fn take_failure(&self) -> Option<Unreadable> {
        self.failed.borrow_mut().take()
    }

    fn fail(&self, unreadable: Unreadable) {
        self.failed.borrow_mut().get_or_insert(unreadable);
    }
}

impl<'o, O: Resolve<'o> + Streams, S: Sink<Fault> + ?Sized> ObjReader<'_, O, S> {
    pub(crate) fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        match self.objects.try_resolve(id) {
            Ok(found) => found,
            Err(unreadable) if unreadable.why == Why::Damaged => {
                self.tell(Some(id), Fault::Damaged { id });
                None
            }
            Err(unreadable) => {
                self.fail(unreadable);
                None
            }
        }
    }

    /// Follows at most one indirect reference (`CPDF_Object::GetDirect`).
    pub(crate) fn hop(&self, value: &'o Object) -> Option<&'o Object> {
        match value.as_reference() {
            Some(id) => self.resolve(id),
            None => Some(value),
        }
    }

    /// Follows one reference and rejects chained references (`Ref -> Ref`),
    /// matching PDFium's scalar accessors.
    pub(crate) fn seen(&self, value: &'o Object) -> Option<&'o Object> {
        self.hop(value).filter(|obj| obj.as_reference().is_none())
    }

    /// Dictionary of `value`, including a stream's dictionary, following up to
    /// two references as PDFium's `GetDict` on a value's direct object does.
    pub(crate) fn dict_of_value(&self, value: &'o Object) -> Option<&'o Dict> {
        let obj = self.hop(value)?;
        own_dict(self.seen(obj)?)
    }

    /// Plain dictionary (not a stream's) after one hop, matching
    /// `ToDictionary(GetDirect())`.
    pub(crate) fn plain_dict_of(&self, value: &'o Object) -> Option<&'o Dict> {
        match self.hop(value)?.value() {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub(crate) fn array_of(&self, value: &'o Object) -> Option<&'o [Object]> {
        self.seen(value)?.as_array()
    }

    pub(crate) fn array_for(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o [Object]> {
        self.array_of(dict.get(key)?)
    }

    pub(crate) fn float_of(&self, value: &'o Object) -> Option<f32> {
        let obj = self.seen(value)?;
        match obj.value() {
            Value::Integer(n) => Some(int32_as_f32(*n)),
            Value::Real(r) => Some(*r as f32),
            _ => None,
        }
    }

    pub(crate) fn float_at(&self, array: &'o [Object], idx: usize) -> f32 {
        array.get(idx).and_then(|v| self.float_of(v)).unwrap_or(0.0)
    }

    pub(crate) fn integer_of(&self, value: &'o Object) -> Option<i32> {
        let obj = self.seen(value)?;
        match obj.value() {
            Value::Integer(n) => Some(int32_of(*n)),
            Value::Real(r) => Some(*r as f32 as i32),
            Value::Bool(true) => Some(1),
            Value::Bool(false) => Some(0),
            _ => None,
        }
    }

    pub(crate) fn bool_for(&self, dict: &'o Dict, key: &[u8]) -> Option<bool> {
        match self.seen(dict.get(key)?)?.value() {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn name_or_bytes_for(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o [u8]> {
        let seen = self.seen(dict.get(key)?)?;
        seen.as_name().or_else(|| seen.as_bytes())
    }

    /// Matches PDFium's `CPDF_Array::GetByteStringAt` (used for `/ExtGState`'s
    /// `/Font` first element!).
    pub(crate) fn byte_string_at(&self, array: &'o [Object], idx: usize) -> Vec<u8> {
        let Some(seen) = array.get(idx).and_then(|v| self.seen(v)) else {
            return Vec::new();
        };
        match seen.value() {
            Value::Bool(true) => b"true".to_vec(),
            Value::Bool(false) => b"false".to_vec(),
            Value::Integer(n) => int32_of(*n).to_string().into_bytes(),
            Value::Real(r) => pdfium_real_text(*r as f32).into_bytes(),
            _ => seen
                .as_name()
                .or_else(|| seen.as_bytes())
                .unwrap_or_default()
                .to_vec(),
        }
    }

    pub(crate) fn stream(&self, id: ObjectId) -> Option<&'o Stream> {
        self.resolve(id)?.as_stream()
    }

    /// Resolves `value` via `Resolve::resolve` without recording `Fault::Damaged`
    /// or `Unreadable` on auxiliary rasterizer-only keys (`/Group`, `/BC`, `/TR`),
    /// preserving exact error and fault parity for non-rasterizing callers.
    pub(crate) fn quiet_hop(&self, value: &'o Object) -> Option<&'o Object> {
        match value.as_reference() {
            Some(id) => self.objects.resolve(id),
            None => Some(value),
        }
    }

    pub(crate) fn quiet_seen(&self, value: &'o Object) -> Option<&'o Object> {
        self.quiet_hop(value)
            .filter(|obj| obj.as_reference().is_none())
    }

    pub(crate) fn quiet_dict_of_value(&self, value: &'o Object) -> Option<&'o Dict> {
        let obj = self.quiet_hop(value)?;
        own_dict(self.quiet_seen(obj)?)
    }

    pub(crate) fn quiet_array_for(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o [Object]> {
        self.quiet_seen(dict.get(key)?)?.as_array()
    }

    pub(crate) fn quiet_float_of(&self, value: &'o Object) -> Option<f32> {
        let obj = self.quiet_seen(value)?;
        match obj.value() {
            Value::Integer(n) => Some(int32_as_f32(*n)),
            Value::Real(r) => Some(*r as f32),
            _ => None,
        }
    }

    pub(crate) fn quiet_bool_for(&self, dict: &'o Dict, key: &[u8]) -> Option<bool> {
        match self.quiet_seen(dict.get(key)?)?.value() {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn quiet_name_or_bytes_for(&self, dict: &'o Dict, key: &[u8]) -> Option<&'o [u8]> {
        let seen = self.quiet_seen(dict.get(key)?)?;
        seen.as_name().or_else(|| seen.as_bytes())
    }
}

fn own_dict(object: &Object) -> Option<&Dict> {
    match object.value() {
        Value::Dict(d) => Some(d),
        Value::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn int32_of(n: i64) -> i32 {
    if let Ok(u) = u32::try_from(n) {
        u as i32
    } else {
        i32::try_from(n).unwrap_or(0)
    }
}

fn int32_as_f32(n: i64) -> f32 {
    if let Ok(u) = u32::try_from(n) {
        u as f32
    } else if let Ok(s) = i32::try_from(n) {
        s as f32
    } else {
        0.0
    }
}

fn pdfium_real_text(value: f32) -> String {
    if value.is_nan() || value == 0.0 {
        return "0".into();
    }
    let value = if value.is_infinite() {
        f32::MAX.copysign(value)
    } else {
        value
    };
    let text = value.abs().to_string();
    let digits = text.strip_prefix('0').unwrap_or(&text);
    if value < 0.0 {
        format!("-{digits}")
    } else {
        digits.to_string()
    }
}
