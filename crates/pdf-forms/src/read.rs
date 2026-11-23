//! Internal object-following and typed dictionary-entry helpers.

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Str, Unreadable, Value, Why, text};

use crate::error::Error;
use crate::fault::Fault;
use crate::limits::Limits;

/// An array reached through an entry, with the object it was read within.
pub(crate) type ArrayAt<'o> = (&'o [Object], Option<ObjectId>);
/// A dictionary reached through an entry, with its own number when indirect.
pub(crate) type DictAt<'o> = (&'o Dict, Option<ObjectId>);

pub(crate) struct Reader<'s, R, S: ?Sized> {
    pub(crate) objects: R,
    pub(crate) rules: Rules,
    pub(crate) limits: &'s Limits,
    pub(crate) sink: &'s mut S,
}

impl<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized> Reader<'_, R, S> {
    #[inline]
    pub(crate) fn say(&mut self, within: Option<ObjectId>, span: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            self.sink.fault(Place::new(within, span), fault);
        }
    }

    /// Follows `value` through its references to what it names: `None` for
    /// `null`, a dangling reference, a damaged object, or a chain past
    /// [`Limits::hops`]. The second value is the object the result was read
    /// within: its own number when it was reached by reference, else `within`.
    pub(crate) fn reached(
        &mut self,
        value: &'o Object,
        within: Option<ObjectId>,
    ) -> Result<Option<(&'o Object, Option<ObjectId>)>, Error> {
        let (mut at, mut last) = (value, None);
        for _ in 0..self.limits.hops {
            match at.value() {
                Value::Ref(id) => {
                    last = Some(*id);
                    match self.objects.try_resolve(*id) {
                        Ok(Some(object)) => at = object,
                        Ok(None) => return Ok(None),
                        Err(unreadable) if unreadable.why == Why::Damaged => {
                            self.say(within, value.at(), Fault::DamagedObject(*id));
                            return Ok(None);
                        }
                        Err(unreadable) => return Err(Error::Unreadable(unreadable)),
                    }
                }
                Value::Null => return Ok(None),
                _ => return Ok(Some((at, last.or(within)))),
            }
        }
        self.say(within, value.at(), Fault::ReferenceLoop);
        Ok(None)
    }

    fn entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<(&'o Object, Option<ObjectId>)>, Error> {
        match dict.get(key.as_bytes()) {
            Some(raw) => self.reached(raw, within),
            None => Ok(None),
        }
    }

    pub(crate) fn dict_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<DictAt<'o>>, Error> {
        let Some((obj, id)) = self.entry(dict, within, key)? else {
            return Ok(None);
        };
        // A dictionary held directly has no number of its own.
        let own = dict
            .get(key.as_bytes())
            .is_some_and(|raw| raw.as_reference().is_some())
            .then_some(id)
            .flatten();
        match obj.as_dict() {
            Some(d) => Ok(Some((d, own))),
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn array_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<ArrayAt<'o>>, Error> {
        let Some((obj, id)) = self.entry(dict, within, key)? else {
            return Ok(None);
        };
        match obj.as_array() {
            Some(arr) => Ok(Some((arr, id))),
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn name_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<&'o [u8]>, Error> {
        let Some((obj, id)) = self.entry(dict, within, key)? else {
            return Ok(None);
        };
        match obj.as_name() {
            Some(name) => Ok(Some(name)),
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn text_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<String>, Error> {
        Ok(self
            .bytes_entry(dict, within, key)?
            .map(|bytes| text::decode_str(bytes, self.rules)))
    }

    pub(crate) fn bytes_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<&'o [u8]>, Error> {
        let Some((obj, id)) = self.entry(dict, within, key)? else {
            return Ok(None);
        };
        match obj.value() {
            Value::String(Str::Bytes(bytes), _) => Ok(Some(bytes.as_slice())),
            Value::String(Str::Encrypted(_), _) => Err(locked_string(id, key)),
            _ => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn bool_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<bool>, Error> {
        let Some((obj, id)) = self.entry(dict, within, key)? else {
            return Ok(None);
        };
        match obj.as_bool() {
            Some(b) => Ok(Some(b)),
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    /// An integer entry. Under [`Rules::Pdfium`] a finite real is taken too,
    /// truncated, as PDFium reads it; either way a real is a
    /// [`Fault::WrongType`].
    pub(crate) fn int_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<i64>, Error> {
        Ok(self.held_int_entry(dict, within, key)?.flatten())
    }

    /// An integer entry as [`int_entry`](Self::int_entry) reads it, told
    /// apart from one that reaches nothing: `None` where the entry reaches
    /// nothing (it's absent, null, or a reference to no object), `Some(None)`
    /// where it reaches an object that gives no integer.
    pub(crate) fn held_int_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<Option<i64>>, Error> {
        let Some((obj, id)) = self.entry(dict, within, key)? else {
            return Ok(None);
        };
        if let Some(n) = obj.as_integer() {
            return Ok(Some(Some(n)));
        }
        self.say(id, obj.at(), Fault::WrongType { key });
        match obj.as_number() {
            Some(v) if self.rules == Rules::Pdfium && v.is_finite() => Ok(Some(Some(v as i64))),
            _ => Ok(Some(None)),
        }
    }
}

pub(crate) fn locked_string(id: Option<ObjectId>, key: &str) -> Error {
    let id = id.unwrap_or(ObjectId::new(0, 0));
    Error::Unreadable(Unreadable::new(
        id,
        Why::Locked,
        format!("{id} R holds /{key} as ciphertext: it takes a password"),
    ))
}
