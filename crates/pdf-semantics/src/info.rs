//! The document information dictionary (ISO 32000-2, 14.3.3): what a file
//! says of itself in its trailer's `/Info`, its title and author and the
//! like as text, its dates as dates, and whether it's trapped, read typed
//! ([`read`]). PDF 2.0 deprecates all of it but the two dates, for the
//! metadata stream's XMP ([`xmp`](crate::xmp)), but files of every version
//! have one.
//!
//! Each entry is read as Table 349 types it: a text string decoded
//! (7.9.2.2) by the [`Rules`] given, as pdf-objects'
//! [`decode_with`](pdf_objects::text::decode_with) decodes it, a date read
//! from its text ([`Date::read`]), and `/Trapped` a name. One that's absent
//! or null is none. One of another type is told and read as none, and a
//! date that doesn't read is told and kept as its text ([`Dated::Text`]).
//! Entries other than Table 349's, which 14.3.3 has as text strings too,
//! aren't read: they're the dictionary's, for a caller to read.
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_semantics::info::{self, Dated, Trapped};
//! use pdf_semantics::pdf_objects::pdf_tokenizer::Rules;
//! use pdf_semantics::pdf_objects::{Object, ObjectId, dict};
//!
//! let info = dict! {
//!     "Title" => Object::text("Annual report"),
//!     "CreationDate" => Object::text("D:20261112093000Z"),
//!     "ModDate" => Object::text("last Tuesday"),
//!     "Trapped" => "False",
//! };
//! let objects = BTreeMap::from([(ObjectId::new(7, 0), Object::from(info))]);
//! let resolve = |id: ObjectId| objects.get(&id);
//!
//! let mut faults = Vec::new();
//! let read = info::read(resolve, &Object::reference(7), Rules::Iso, &mut faults).unwrap();
//! let read = read.unwrap();
//! assert_eq!(read.title.as_deref(), Some("Annual report"));
//! assert_eq!(read.created.as_ref().and_then(Dated::date).map(|date| date.year()), Some(2026));
//! assert_eq!(read.modified, Some(Dated::Text("last Tuesday".into())));
//! assert_eq!(read.trapped, Some(Trapped::False));
//! assert_eq!(read.author, None);
//!
//! // The date that doesn't read is told, where it's written.
//! assert_eq!(faults.len(), 1);
//! assert_eq!(faults[0].1.to_string(), "/ModDate isn't a date: a date begins D:");
//! assert_eq!(faults[0].0.object, Some(ObjectId::new(7, 0)));
//! ```

use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Object, ObjectId, Resolve, Str, Unreadable, Value, Why, text};

use crate::date::Date;
use crate::fault::Fault;
use crate::{HOPS, too_long};

#[cfg(test)]
mod tests;

/// What a document information dictionary says (14.3.3, Table 349), each
/// entry none where it's absent, null, or not of its type. Its `Debug` gives
/// each text's length, not its text, as a file's may be decrypted.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Info {
    /// `/Title`: the document's title.
    pub title: Option<String>,
    /// `/Author`: who made the document.
    pub author: Option<String>,
    /// `/Subject`: what it's about.
    pub subject: Option<String>,
    /// `/Keywords`: its keywords, as written.
    pub keywords: Option<String>,
    /// `/Creator`: the program that made the document it was made from,
    /// where it was converted to PDF.
    pub creator: Option<String>,
    /// `/Producer`: the program that made the PDF.
    pub producer: Option<String>,
    /// `/CreationDate`: when the document was made.
    pub created: Option<Dated>,
    /// `/ModDate`: when it was last changed.
    pub modified: Option<Dated>,
    /// `/Trapped`: whether it's trapped (14.11.6). Table 349 has `Unknown`
    /// stand for it where it's none.
    pub trapped: Option<Trapped>,
}

/// A date entry: a date, or the text of one that doesn't read as one.
///
/// Exhaustive on purpose: a date's text reads as a date or doesn't, so a
/// match on both is checked. Its `Debug` gives a text's length, not its
/// text.
#[derive(Clone, PartialEq, Eq)]
pub enum Dated {
    /// It reads as a date (7.9.4).
    Date(Date),
    /// It doesn't, and is its text as decoded, told as
    /// [`Fault::InfoDate`]. [`Date::read`] of it says why again.
    Text(String),
}

impl Dated {
    /// The date, where it reads as one.
    #[must_use]
    pub const fn date(&self) -> Option<&Date> {
        match self {
            Dated::Date(date) => Some(date),
            Dated::Text(_) => None,
        }
    }
}

/// Whether a document is trapped, as `/Trapped` names it (14.3.3, Table
/// 349).
///
/// Exhaustive on purpose: Table 349 names these three, and PDF 2.0
/// deprecates the entry, so a match on all three is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trapped {
    /// `/True`: trapped fully, needing no more.
    True,
    /// `/False`: not trapped yet.
    False,
    /// `/Unknown`: not known, or trapped in part.
    Unknown,
}

/// What a document information dictionary at `info` says: `info` as
/// written, the trailer's `/Info` or a thread's `/I` (12.4.3), its
/// references followed; none where it's null, a reference to nothing, or
/// not a dictionary, which last is told. Each entry is read as the module
/// says, its references followed, its text strings decoded by `rules`; and
/// what departs from Table 349 is told to `sink`, at the entry's value, in
/// the object it's in: one of another type ([`Fault::InfoNotText`],
/// [`Fault::InfoTrapped`]), and a date that doesn't read
/// ([`Fault::InfoDate`]). What's read is the same whatever the sink.
///
/// An object that doesn't read ([`Why::Damaged`]) is one that isn't there,
/// told as [`Fault::Damaged`]. A string the reader gave as ciphertext, in a
/// file no password opened, isn't read as no entry: it fails the read, as
/// an object that takes a password does.
///
/// # Errors
///
/// An [`Unreadable`], and nothing read, where an object on the way is past
/// a cap the reader reads under, takes a password, or wasn't given by the
/// source, as `objects` say, or a chain of references is too long to
/// follow ([`Why::Capped`]); and of [`Why::Locked`] for a string the reader
/// gave as ciphertext, in the object that holds it.
pub fn read<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    info: &'o Object,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<Info>, Unreadable> {
    let mut read = Read {
        objects,
        rules,
        sink,
    };
    let (dict, id) = match read.reached(info, None)? {
        Some((object, id)) => match object.value() {
            Value::Dict(dict) => (dict, id),
            _ => {
                read.say(Place::new(id, object.at), Fault::InfoNotDictionary);
                return Ok(None);
            }
        },
        None => return Ok(None),
    };
    let mut text = |key: &'static str| match dict.get(key.as_bytes()) {
        Some(value) => Ok(read.text(value, id, key)?.map(|(text, _)| text)),
        None => Ok(None),
    };
    let (title, author, subject) = (text("Title")?, text("Author")?, text("Subject")?);
    let (keywords, creator, producer) = (text("Keywords")?, text("Creator")?, text("Producer")?);
    let mut dated = |key: &'static str| match dict.get(key.as_bytes()) {
        Some(value) => read.dated(value, id, key),
        None => Ok(None),
    };
    let (created, modified) = (dated("CreationDate")?, dated("ModDate")?);
    let trapped = match dict.get(b"Trapped") {
        Some(value) => read.trapped(value, id)?,
        None => None,
    };
    Ok(Some(Info {
        title,
        author,
        subject,
        keywords,
        creator,
        producer,
        created,
        modified,
        trapped,
    }))
}

/// A read of one dictionary: what gives its objects, how its text strings
/// are decoded, and where what departs is told.
struct Read<'s, R, S: ?Sized> {
    objects: R,
    rules: Rules,
    sink: &'s mut S,
}

impl<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized> Read<'_, R, S> {
    /// `value`, written in object `within`, followed: what it comes to and
    /// the numbered object that is, `within` where it's written in place;
    /// none for null, a reference to nothing, or an object that doesn't read,
    /// which is told.
    fn reached(
        &mut self,
        value: &'o Object,
        within: Option<ObjectId>,
    ) -> Result<Option<(&'o Object, Option<ObjectId>)>, Unreadable> {
        let (mut at, mut last) = (value, None);
        for _ in 0..HOPS {
            match at.value() {
                Value::Ref(id) => {
                    last = Some(*id);
                    match self.objects.try_resolve(*id) {
                        Ok(Some(object)) => at = object,
                        Ok(None) => return Ok(None),
                        Err(unreadable) if unreadable.why == Why::Damaged => {
                            let place = Place::new(within, value.at);
                            self.say(place, Fault::Damaged { id: *id });
                            return Ok(None);
                        }
                        Err(unreadable) => return Err(unreadable),
                    }
                }
                Value::Null => return Ok(None),
                _ => return Ok(Some((at, last.or(within)))),
            }
        }
        Err(too_long(last.unwrap_or(ObjectId::new(0, 0))))
    }

    /// The text string `value` is, under `key` of a dictionary in object
    /// `within`, decoded, and where it's written; none where it isn't one,
    /// which is told.
    fn text(
        &mut self,
        value: &'o Object,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<(String, Place)>, Unreadable> {
        let Some((object, id)) = self.reached(value, within)? else {
            return Ok(None);
        };
        let place = Place::new(id, object.at);
        let text = match object.value() {
            Value::String(Str::Bytes(bytes), _) => text::decode_with(bytes, self.rules).text,
            Value::String(Str::Text(text), _) => text.clone(),
            Value::String(Str::Encrypted(_), _) => {
                let id = id.unwrap_or(ObjectId::new(0, 0));
                let message = format!("{id} R holds /{key} as ciphertext: it takes a password");
                return Err(Unreadable::new(id, Why::Locked, message));
            }
            _ => {
                self.say(place, Fault::InfoNotText { key });
                return Ok(None);
            }
        };
        Ok(Some((text, place)))
    }

    /// The date `value` is, under `key`: its text read as one, or kept where
    /// it doesn't read, which is told.
    fn dated(
        &mut self,
        value: &'o Object,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<Dated>, Unreadable> {
        let Some((text, place)) = self.text(value, within, key)? else {
            return Ok(None);
        };
        Ok(Some(match Date::read(&text) {
            Ok(date) => Dated::Date(date),
            Err(error) => {
                self.say(place, Fault::InfoDate { key, error });
                Dated::Text(text)
            }
        }))
    }

    /// What `/Trapped`, `value`, names; none where it's no name of Table
    /// 349's, which is told.
    fn trapped(
        &mut self,
        value: &'o Object,
        within: Option<ObjectId>,
    ) -> Result<Option<Trapped>, Unreadable> {
        let Some((object, id)) = self.reached(value, within)? else {
            return Ok(None);
        };
        let trapped = match object.as_name() {
            Some(b"True") => Trapped::True,
            Some(b"False") => Trapped::False,
            Some(b"Unknown") => Trapped::Unknown,
            _ => {
                self.say(Place::new(id, object.at), Fault::InfoTrapped);
                return Ok(None);
            }
        };
        Ok(Some(trapped))
    }

    fn say(&mut self, place: Place, fault: Fault) {
        if self.sink.wants() {
            self.sink.fault(place, fault);
        }
    }
}

/// A text's length, which a `Debug` gives for it.
pub(crate) struct Len(pub(crate) usize);

impl fmt::Debug for Len {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.0)
    }
}

/// A text, by its length, where there's one.
pub(crate) fn len(text: &Option<String>) -> Option<Len> {
    text.as_ref().map(|text| Len(text.len()))
}

impl fmt::Debug for Info {
    /// Each text's length, each date and `/Trapped`; not the texts, which
    /// may be decrypted.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Info")
            .field("title", &len(&self.title))
            .field("author", &len(&self.author))
            .field("subject", &len(&self.subject))
            .field("keywords", &len(&self.keywords))
            .field("creator", &len(&self.creator))
            .field("producer", &len(&self.producer))
            .field("created", &self.created)
            .field("modified", &self.modified)
            .field("trapped", &self.trapped)
            .finish()
    }
}

impl fmt::Debug for Dated {
    /// The date, or the text's length; not the text, which may be
    /// decrypted.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Dated::Date(date) => f.debug_tuple("Date").field(date).finish(),
            Dated::Text(text) => f.debug_tuple("Text").field(&Len(text.len())).finish(),
        }
    }
}
