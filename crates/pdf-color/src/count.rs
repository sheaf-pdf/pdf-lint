//! How many components a color space has, as PDFium counts them: what sizes
//! an unfiltered inline image drawn in it, so where that image's data ends,
//! whatever ISO 32000-2 says the space should have held.
//!
//! PDFium has two paths into a space, and they part on what they're given. Its
//! first, for what a resources dictionary holds, an Indexed space's base and a
//! Pattern space's underlying one, takes a name or an array and goes into an
//! array of one ([`Walk::top`]). Its second, for an array of more and for an
//! alternate space, also takes a stream, and doesn't go into an array of one
//! ([`Walk::family`]). A reference is followed once by what reads a value, so a
//! reference to a reference is what neither path takes, though a value's
//! accessors follow one more. The numbers it asks of a space are read as its
//! accessors read them ([`integer`], [`float`]).

use pdf_objects::{Object, ObjectId, Resolve, Unreadable, Value, Why};

use crate::function::{self, Shape};
use crate::limits::Limits;

/// What a space that doesn't load counts as: PDFium's default for an inline
/// image's data whose space it can't make out.
const UNLOADED: u32 = 3;

/// The most components a Pattern space's underlying space has in PDFium, past
/// which the Pattern space doesn't load. No clause sets it.
const MOST_UNDERLYING: u32 = 16;

/// The components of a color space as PDFium counts them, as of
/// chromium/8125. `value` is what a `/ColorSpace` dictionary holds under a
/// name, as written, or an inline image's array.
///
/// `Some(n)` is the count, 3 where PDFium loads no space. None is a reference
/// that leads to nothing, for which PDFium has no space at all, as for a name
/// the resources lack: an inline image drawn in it is 1 component of 1 bit.
///
/// A number's read as PDFium's accessors read it, a family by the first four
/// bytes of a name or a string, and a function by its dictionary alone: no
/// stream is decoded, so a sampled function too short for its sizes and a
/// PostScript function that doesn't parse count as loading, where PDFium's
/// don't. Past a cap of [`Limits`], a branch doesn't load.
///
/// An object that's [`Why::Damaged`] isn't there, as it isn't for PDFium; any
/// other failure to read one is the `Err`, whole.
///
/// A count where a cap was met can't be told from one of a space that doesn't
/// load: [`counted`] says which it is.
///
/// ```
/// use pdf_color::pdf_objects::{Object, ObjectId};
/// use pdf_color::{Limits, components};
///
/// let file: Vec<Object> = vec![
///     Object::from("DeviceCMYK"),
///     Object::from(vec![Object::from("Pattern"), Object::from("DeviceRGB")]),
///     Object::from("CalGray"),
///     Object::reference(9),
/// ];
/// let nothing = |_: ObjectId| None;
/// let count = |at: usize| components(nothing, &file[at], &Limits::DEFAULT);
/// assert_eq!(count(0), Ok(Some(4)));
/// // The underlying space's components, and the pattern's name.
/// assert_eq!(count(1), Ok(Some(4)));
/// // No stock space has that name, and a family alone isn't a space.
/// assert_eq!(count(2), Ok(Some(3)));
/// // A reference to nothing.
/// assert_eq!(count(3), Ok(None));
/// ```
pub fn components<'o, R: Resolve<'o>>(
    objects: R,
    value: &'o Object,
    limits: &Limits,
) -> Result<Option<u32>, Unreadable> {
    counted(objects, value, limits).map(|counted| counted.count)
}

/// What [`counted`] gives: a space's count, and whether it's sure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Counted {
    /// What [`components`] gives.
    pub count: Option<u32>,
    /// Whether a cap of [`Limits`] ended a branch of it, so that `count` is what
    /// that branch not loading makes it, which may not be what PDFium counts.
    pub capped: bool,
}

/// What [`components`] gives, and whether a cap ended a branch of it, for a
/// caller that acts on where an image's data ends and would refuse what it can't
/// be sure of, or read it again under larger caps.
///
/// ```
/// use pdf_color::pdf_objects::{Object, ObjectId};
/// use pdf_color::{Limits, counted};
///
/// let nested = Object::from(vec![Object::from(vec![Object::from("DeviceGray")])]);
/// let nothing = |_: ObjectId| None;
/// let whole = counted(nothing, &nested, &Limits::DEFAULT).unwrap();
/// assert_eq!((whole.count, whole.capped), (Some(1), false));
///
/// let mut limits = Limits::DEFAULT;
/// limits.space_depth = 0;
/// let cut = counted(nothing, &nested, &limits).unwrap();
/// assert_eq!((cut.count, cut.capped), (Some(3), true));
/// ```
pub fn counted<'o, R: Resolve<'o>>(
    objects: R,
    value: &'o Object,
    limits: &Limits,
) -> Result<Counted, Unreadable> {
    let mut walk = Walk::within(objects, limits, 0);
    let count = match walk.hop(value)? {
        Some(at) => Some(walk.top(at, 0)?.map_or(UNLOADED, |space| space.components)),
        None if walk.capped => Some(UNLOADED),
        None => None,
    };
    Ok(Counted {
        count,
        capped: walk.capped,
    })
}

/// Object `id`, or none where it isn't there: one that's [`Why::Damaged`] is
/// read as one that isn't, and any other failure is the `Err`.
pub(crate) fn look<'o, R: Resolve<'o>>(
    objects: &R,
    id: ObjectId,
) -> Result<Option<&'o Object>, Unreadable> {
    match objects.try_resolve(id) {
        Err(unreadable) if unreadable.why == Why::Damaged => Ok(None),
        read => read,
    }
}

/// A space that loaded, as far as what holds it asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Loaded {
    components: u32,
    kind: Kind,
}

/// What a space that holds another asks of the space it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A device, CIE-based or ICCBased space.
    Plain,
    Indexed,
    Pattern,
    /// A Separation or DeviceN space.
    Tint,
}

impl Loaded {
    fn new(components: u32, kind: Kind) -> Loaded {
        Loaded { components, kind }
    }
}

/// A value after one reference, what PDFium calls its direct object, which may
/// itself be a reference; and the number it had, if it had one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Hop<'o> {
    pub(crate) object: &'o Object,
    pub(crate) id: Option<ObjectId>,
}

/// One space's reading: what's been spent, and what's on the way down.
pub(crate) struct Walk<'l, R> {
    objects: R,
    limits: &'l Limits,
    visits: usize,
    /// The objects on the current path, by number, as PDFium's `visited` has
    /// them: one seen again doesn't load. Pushed to only for a reference, and
    /// at most as long as the depth lets the path be.
    path: Vec<ObjectId>,
    capped: bool,
}

/// A family's first four bytes, zero bytes after a shorter name: what PDFium
/// tells a family by, so a name that begins as one is it.
fn first_four(family: &[u8]) -> [u8; 4] {
    let mut four = [0; 4];
    for (to, from) in four.iter_mut().zip(family) {
        *to = *from;
    }
    four
}

/// PDFium's stock space of this name: the device spaces and their inline
/// abbreviations (8.9.7), and Pattern. No other family is a space by its name.
fn stock(name: &[u8]) -> Option<Loaded> {
    match name {
        b"DeviceGray" | b"G" => Some(Loaded::new(1, Kind::Plain)),
        b"DeviceRGB" | b"RGB" => Some(Loaded::new(3, Kind::Plain)),
        b"DeviceCMYK" | b"CMYK" => Some(Loaded::new(4, Kind::Plain)),
        b"Pattern" => Some(Loaded::new(1, Kind::Pattern)),
        _ => None,
    }
}

/// An integer as PDFium's parser holds it: one with no sign in 32 bits
/// unsigned, a negative one in 32 bits signed, and past those 0. A positive one
/// with a sign is held by the first rule here, as an `Object` doesn't keep the
/// sign; PDFium's has only 31 bits, and nothing here tells the two apart but a
/// white point's number past them.
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
/// integer as it holds it, past 31 bits with no sign wrapping to a negative; a
/// real cut from its 32-bit float, saturating; `true` as 1; anything else 0.
fn integer(object: &Object) -> i32 {
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

/// What PDFium's float accessor reads of `object`, itself no reference: a
/// number's 32-bit float, anything else 0, `true` too.
///
/// An `Object`'s real is an `f64`, parsed by the reader's rules, where PDFium
/// parses the bytes to a float itself: a real of many digits can round
/// otherwise (`1.00000005960464478`).
pub(crate) fn float(object: &Object) -> f32 {
    match object.value() {
        Value::Integer(n) => match held(*n) {
            Number32::Unsigned(n) => n as f32,
            Number32::Signed(n) => n as f32,
        },
        Value::Real(real) => *real as f32,
        _ => 0.0,
    }
}

impl<'l, 'o, R: Resolve<'o>> Walk<'l, R> {
    /// A walk with `visits` already spent, by one that goes on from there.
    pub(crate) fn within(objects: R, limits: &'l Limits, visits: usize) -> Walk<'l, R> {
        Walk {
            objects,
            limits,
            visits,
            path: Vec::new(),
            capped: false,
        }
    }

    /// The visits spent so far, those it was given among them.
    pub(crate) fn spent(&self) -> usize {
        self.visits
    }

    /// Whether a cap ended a branch.
    pub(crate) fn capped(&self) -> bool {
        self.capped
    }

    /// Object `id`, a visit spent on it; past the cap, none.
    fn lookup(&mut self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        if !self.spend() {
            return Ok(None);
        }
        look(&self.objects, id)
    }

    /// One visit spent, or none left to spend.
    pub(crate) fn spend(&mut self) -> bool {
        if self.visits >= self.limits.space_visits {
            self.capped = true;
            return false;
        }
        self.visits += 1;
        true
    }

    /// `value` after one reference: what PDFium's readers of a value have,
    /// which is none for a reference to nothing.
    pub(crate) fn hop(&mut self, value: &'o Object) -> Result<Option<Hop<'o>>, Unreadable> {
        let Some(id) = value.as_reference() else {
            return Ok(Some(Hop {
                object: value,
                id: None,
            }));
        };
        let object = self.lookup(id)?;
        Ok(object.map(|object| Hop {
            object,
            id: Some(id),
        }))
    }

    /// `value` as a number's or a string's accessor sees it: a reference
    /// followed once, and no further, one to a reference being none.
    fn seen(&mut self, value: &'o Object) -> Result<Option<&'o Object>, Unreadable> {
        Ok(self.hop(value)?.and_then(|at| {
            let object = at.object;
            (object.as_reference().is_none()).then_some(object)
        }))
    }

    /// What PDFium's integer accessor reads of `value`, 0 for none.
    pub(crate) fn integer(&mut self, value: Option<&'o Object>) -> Result<i32, Unreadable> {
        let Some(value) = value else { return Ok(0) };
        Ok(self.seen(value)?.map_or(0, integer))
    }

    /// The bytes of a name or a string, as PDFium's accessor reads them; none
    /// of anything else, which it reads as bytes no family and no colorant has.
    fn text(&mut self, value: &'o Object) -> Result<Option<&'o [u8]>, Unreadable> {
        let seen = self.seen(value)?;
        Ok(seen.and_then(|object| object.as_name().or_else(|| object.as_bytes())))
    }

    /// The array `value` is after one reference.
    pub(crate) fn array(
        &mut self,
        value: Option<&'o Object>,
    ) -> Result<Option<&'o [Object]>, Unreadable> {
        let Some(value) = value else { return Ok(None) };
        Ok(self.hop(value)?.and_then(|at| at.object.as_array()))
    }

    /// `read`, of the object `at`, with it on the path: if it's already on the
    /// path, or past the depth, what's there doesn't load.
    pub(crate) fn on_path<T>(
        &mut self,
        at: Hop<'o>,
        depth: usize,
        read: fn(&mut Self, Hop<'o>, usize) -> Result<Option<T>, Unreadable>,
    ) -> Result<Option<T>, Unreadable> {
        if depth > self.limits.space_depth {
            self.capped = true;
            return Ok(None);
        }
        let Some(id) = at.id else {
            return read(self, at, depth);
        };
        if self.path.contains(&id) {
            return Ok(None);
        }
        self.path.push(id);
        let read = read(self, at, depth);
        self.path.pop();
        read
    }

    /// PDFium's first path: a name by its stock space, an array of one by what
    /// it holds, this path again, an array of more by [`Walk::of_array`], and
    /// anything else none, a stream and a reference among them.
    fn top(&mut self, at: Hop<'o>, depth: usize) -> Result<Option<Loaded>, Unreadable> {
        self.on_path(at, depth, |walk, at, depth| match at.object.value() {
            Value::Name(name) => Ok(stock(name)),
            Value::Array(parts) => match parts.as_slice() {
                [] => Ok(None),
                [one] => match walk.hop(one)? {
                    Some(inner) => walk.top(inner, depth + 1),
                    None => Ok(None),
                },
                _ => walk.of_array(at, parts, depth),
            },
            _ => Ok(None),
        })
    }

    /// PDFium's second path: a name by its stock space, a stream by the first
    /// of its values that names one, an array by [`Walk::of_array`], and
    /// anything else none.
    fn family(&mut self, at: Hop<'o>, depth: usize) -> Result<Option<Loaded>, Unreadable> {
        self.on_path(at, depth, |walk, at, depth| match at.object.value() {
            Value::Name(name) => Ok(stock(name)),
            Value::Stream(stream) => {
                // PDFium's dictionary is sorted by its keys, where a `Dict` is
                // as written, and it drops an empty key as it parses one. A
                // value by reference is no name here.
                let stocked = (stream.dict.last_entries())
                    .filter(|entry| !entry.key.is_empty())
                    .filter_map(|entry| Some((&entry.key, stock(entry.value.as_name()?)?)));
                let first = stocked.min_by_key(|(key, _)| key.as_slice());
                Ok(first.map(|(_, space)| space))
            }
            Value::Array(parts) if !parts.is_empty() => walk.of_array(at, parts, depth),
            _ => Ok(None),
        })
    }

    /// An array on PDFium's second path: of one, by the stock space of its
    /// element's bytes; of more, by its family.
    fn of_array(
        &mut self,
        at: Hop<'o>,
        parts: &'o [Object],
        depth: usize,
    ) -> Result<Option<Loaded>, Unreadable> {
        let Some(family) = self.hop(&parts[0])? else {
            return Ok(None);
        };
        let family = self.text(family.object)?.unwrap_or_default();
        if parts.len() == 1 {
            return Ok(stock(family));
        }
        match &first_four(family) {
            b"CalG" => self.cie(parts, 1),
            b"CalR" | b"Lab\0" => self.cie(parts, 3),
            b"ICCB" => self.icc(parts),
            b"Inde" | b"I\0\0\0" => self.indexed(parts, depth),
            b"Sepa" => self.separation(parts, depth),
            b"Devi" => self.device_n(parts, depth),
            b"Patt" => self.pattern(at, parts, depth),
            _ => Ok(None),
        }
    }

    /// The object at `parts[at]` after one reference; none past the array's
    /// end.
    fn part(&mut self, parts: &'o [Object], at: usize) -> Result<Option<Hop<'o>>, Unreadable> {
        match parts.get(at) {
            Some(part) => self.hop(part),
            None => Ok(None),
        }
    }

    /// A CalGray, CalRGB or Lab space: its second element a dictionary or a
    /// stream's, whose white point is three numbers, the first and third above
    /// 0 and the second 1, as 32-bit floats.
    fn cie(&mut self, parts: &'o [Object], components: u32) -> Result<Option<Loaded>, Unreadable> {
        let Some(at) = self.part(parts, 1)? else {
            return Ok(None);
        };
        let dict = match at.object.value() {
            Value::Dict(dict) => dict,
            Value::Stream(stream) => &stream.dict,
            _ => return Ok(None),
        };
        let Some([x, y, z]) = self.array(dict.get(b"WhitePoint"))? else {
            return Ok(None);
        };
        let mut read = |value| Ok::<f32, Unreadable>(self.seen(value)?.map_or(0.0, float));
        let (x, y, z) = (read(x)?, read(y)?, read(z)?);
        let holds = x > 0.0 && y == 1.0 && z > 0.0;
        Ok(holds.then_some(Loaded::new(components, Kind::Plain)))
    }

    /// An ICCBased space: its second element a stream whose `/N` is 1, 3 or 4.
    /// No profile's read: PDFium keeps the count whatever it finds in one.
    fn icc(&mut self, parts: &'o [Object]) -> Result<Option<Loaded>, Unreadable> {
        let Some(at) = self.part(parts, 1)? else {
            return Ok(None);
        };
        let Some(stream) = at.object.as_stream() else {
            return Ok(None);
        };
        Ok(match self.integer(stream.dict.get(b"N"))? {
            n @ (1 | 3 | 4) => Some(Loaded::new(n as u32, Kind::Plain)),
            _ => None,
        })
    }

    /// An Indexed space: at least four elements, a base by the first path that
    /// isn't Indexed or Pattern, and a fourth that's there, whatever it is. The
    /// high value isn't asked of, as PDFium clamps it.
    fn indexed(&mut self, parts: &'o [Object], depth: usize) -> Result<Option<Loaded>, Unreadable> {
        if parts.len() < 4 {
            return Ok(None);
        }
        let Some(base) = self.part(parts, 1)? else {
            return Ok(None);
        };
        let Some(base) = self.top(base, depth + 1)? else {
            return Ok(None);
        };
        if matches!(base.kind, Kind::Indexed | Kind::Pattern) {
            return Ok(None);
        }
        let table = self.part(parts, 3)?;
        Ok(table.map(|_| Loaded::new(1, Kind::Indexed)))
    }

    /// A Separation space: the colorant `None` reads nothing more; any other
    /// has an alternate, by the second path, that's no special space. Its tint
    /// transform isn't asked of: PDFium loads the space without one.
    fn separation(
        &mut self,
        parts: &'o [Object],
        depth: usize,
    ) -> Result<Option<Loaded>, Unreadable> {
        let tint = Some(Loaded::new(1, Kind::Tint));
        if let Some(name) = parts.get(1) {
            if self.text(name)? == Some(b"None") {
                return Ok(tint);
            }
        }
        let Some(alternate) = self.part(parts, 2)? else {
            return Ok(None);
        };
        let Some(alternate) = self.family(alternate, depth + 1)? else {
            return Ok(None);
        };
        Ok(tint.filter(|_| alternate.kind == Kind::Plain))
    }

    /// A DeviceN space: as many components as its second element, an array,
    /// has, at least one, whatever they are; an alternate, by the second path,
    /// that's no special space; and a function that loads, with an output for
    /// each of the alternate's components.
    fn device_n(
        &mut self,
        parts: &'o [Object],
        depth: usize,
    ) -> Result<Option<Loaded>, Unreadable> {
        let Some(names) = self.part(parts, 1)?.and_then(|at| at.object.as_array()) else {
            return Ok(None);
        };
        let Some(alternate) = self.part(parts, 2)? else {
            return Ok(None);
        };
        let Some(alternate) = self.family(alternate, depth + 1)? else {
            return Ok(None);
        };
        let function = self.part(parts, 3)?;
        let Some(Shape { outputs, .. }) = function::shape(self, function, depth + 1)? else {
            return Ok(None);
        };
        if alternate.kind != Kind::Plain || outputs < alternate.components {
            return Ok(None);
        }
        let count = u32::try_from(names.len()).ok().filter(|n| *n > 0);
        Ok(count.map(|n| Loaded::new(n, Kind::Tint)))
    }

    /// A Pattern space with something for an underlying space: if it's the
    /// array itself this doesn't load; by the first path, one that doesn't
    /// load leaves the Pattern space with none, of 1 component; one that does,
    /// unless it's a Pattern space or past [`MOST_UNDERLYING`], counts its own
    /// and the pattern's name.
    fn pattern(
        &mut self,
        at: Hop<'o>,
        parts: &'o [Object],
        depth: usize,
    ) -> Result<Option<Loaded>, Unreadable> {
        let pattern = |n| Loaded::new(n, Kind::Pattern);
        let Some(under) = self.part(parts, 1)? else {
            return Ok(Some(pattern(1)));
        };
        if under.id.is_some() && under.id == at.id {
            return Ok(None);
        }
        Ok(match self.top(under, depth + 1)? {
            None => Some(pattern(1)),
            Some(under) if under.kind == Kind::Pattern => None,
            Some(under) if under.components > MOST_UNDERLYING => None,
            Some(under) => Some(pattern(under.components + 1)),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    use pdf_objects::parse::Parser;

    use super::*;

    const F2_1: &str = "<< /FunctionType 2 /Domain [0 1] /N 1 >>";
    const F2_4: &str = "<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0 0] /C1 [1 1 1 1] >>";
    const CAL: &str = "<< /WhitePoint [0.95 1 1.08] >>";

    /// `text` as one value.
    fn value(text: &str) -> Object {
        let mut parser = Parser::at(text.as_bytes(), 0);
        (parser.value(&mut ())).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    /// `extra` read, the first of them object 5, as in the cases at
    /// `tests/pdfium`, a dictionary with `/Length` a stream.
    fn objects(extra: &[&str]) -> BTreeMap<ObjectId, Object> {
        (extra.iter().zip(5..))
            .map(|(text, n)| {
                let object = value(text);
                let stream = (object.as_dict())
                    .filter(|dict| dict.has(b"Length"))
                    .map(|dict| Object::stream(dict.clone()));
                (ObjectId::new(n, 0), stream.unwrap_or(object))
            })
            .collect()
    }

    /// What `text` counts as, with `extra` for its objects.
    fn counted(text: &str, extra: &[&str]) -> Option<u32> {
        let (value, objects) = (value(text), objects(extra));
        let resolve = |id: ObjectId| objects.get(&id);
        components(resolve, &value, &Limits::DEFAULT).unwrap()
    }

    /// Each `(value, count)`, with no objects.
    #[track_caller]
    fn each(rows: &[(&str, u32)]) {
        for (text, want) in rows {
            assert_eq!(counted(text, &[]), Some(*want), "{text}");
        }
    }

    /// A DeviceN of the names `[/A /B]`, alternate CMYK, with this function.
    fn device_n_2(function: &str) -> String {
        format!("[/DeviceN [/A /B] /DeviceCMYK {function}]")
    }

    #[test]
    fn stock_names_count_and_a_family_alone_is_no_space() {
        each(&[
            ("/DeviceGray", 1),
            ("/G", 1),
            ("/DeviceRGB", 3),
            ("/RGB", 3),
            ("/DeviceCMYK", 4),
            ("/CMYK", 4),
            ("/Pattern", 1),
            ("/CalGray", 3),
            ("/CalRGB", 3),
            ("/Lab", 3),
            ("/Indexed", 3),
            ("/I", 3),
            ("/Separation", 3),
            ("/DeviceN", 3),
            ("/ICCBased", 3),
            ("/Foo", 3),
            ("/DeviceGrayX", 3),
            ("[/Indexed]", 3),
        ]);
    }

    #[test]
    fn what_is_no_name_or_array_is_three() {
        each(&[
            ("7", 3),
            ("(DeviceRGB)", 3),
            ("null", 3),
            ("true", 3),
            ("<< >>", 3),
            ("[]", 3),
        ]);
    }

    #[test]
    fn a_reference_is_followed_once() {
        assert_eq!(counted("99 0 R", &[]), None);
        assert_eq!(counted("5 0 R", &["/DeviceCMYK"]), Some(4));
        assert_eq!(counted("5 0 R", &["6 0 R", "/DeviceCMYK"]), Some(3));
        assert_eq!(counted("5 0 R", &["[/Pattern /DeviceRGB]"]), Some(4));
        assert_eq!(counted("5 0 R", &["null"]), Some(3));
        assert_eq!(counted("5 0 R", &["<< /Length 0 /A /G >>"]), Some(3));
        assert_eq!(counted("5 0 R", &["5 0 R"]), Some(3));
    }

    #[test]
    fn an_array_of_one_is_what_it_holds() {
        each(&[
            ("[/DeviceGray]", 1),
            ("[/DeviceRGB]", 3),
            ("[/CMYK]", 4),
            ("[/Pattern]", 1),
            ("[/Foo]", 3),
            ("[[/DeviceCMYK]]", 4),
            ("[[[/G]]]", 1),
            ("[(DeviceCMYK)]", 3),
            ("[7]", 3),
            ("[null]", 3),
            ("[<< >>]", 3),
        ]);
        assert_eq!(counted("[5 0 R]", &["/DeviceCMYK"]), Some(4));
        assert_eq!(counted("[5 0 R]", &["6 0 R", "/DeviceCMYK"]), Some(3));
        assert_eq!(counted("[99 0 R]", &[]), Some(3));
        assert_eq!(counted("[5 0 R]", &["<< /Length 0 /A /G >>"]), Some(3));
    }

    #[test]
    fn a_device_name_in_an_array_of_more_is_a_device_n_that_does_not_load() {
        each(&[
            ("[/DeviceGray 1]", 3),
            ("[/G 1]", 3),
            ("[/DeviceCMYK 1]", 3),
            ("[/CMYK 1]", 3),
            ("[/Foo 1]", 3),
            ("[/DeviceCMYK null]", 3),
            ("[null /DeviceRGB]", 3),
            ("[7 /DeviceRGB]", 3),
        ]);
        let one_colorant = format!("[/DeviceCMYK [/A] /DeviceGray {F2_1}]");
        assert_eq!(counted(&one_colorant, &[]), Some(1));
    }

    #[test]
    fn a_cie_space_needs_its_white_point() {
        let wants = [("CalGray", 1), ("CalRGB", 3), ("Lab", 3)];
        for (family, want) in wants {
            let of = |parameters: &str| format!("[/{family} {parameters}]");
            assert_eq!(counted(&of(CAL), &[]), Some(want), "{family}");
            assert_eq!(counted(&of("5 0 R"), &[CAL]), Some(want), "{family}");
            let stream = "<< /Length 0 /WhitePoint [1 1 1] >>";
            assert_eq!(counted(&of("5 0 R"), &[stream]), Some(want), "{family}");
            for bad in [
                "7",
                "<< >>",
                "<< /WhitePoint [1 1] >>",
                "<< /WhitePoint [1 1 1 1] >>",
                "<< /WhitePoint [0.9 0.9 0.9] >>",
                "<< /WhitePoint [0 1 1] >>",
                "<< /WhitePoint [1 1 0] >>",
                "<< /WhitePoint [-1 1 1] >>",
            ] {
                assert_eq!(counted(&of(bad), &[]), Some(3), "{family} {bad}");
            }
        }
        // Through a chain, the parameters aren't read.
        let chained = counted("[/CalGray 5 0 R]", &["6 0 R", CAL]);
        assert_eq!(chained, Some(3));
    }

    #[test]
    fn a_white_point_is_read_as_32_bit_floats() {
        let cal = |point: &str| format!("[/CalGray << /WhitePoint {point} >>]");
        for (point, want) in [
            ("[1 0.99999999 1]", 1),
            ("[1 1.0000000001 1]", 1),
            ("[1 1.00000001 1]", 1),
            ("[1 1.0000001 1]", 3),
            ("[1 0.9999999 1]", 3),
            ("[1 true 1]", 3),
            ("[true 1 1]", 3),
            ("[1 1 true]", 3),
            ("[1 (1) 1]", 3),
            ("[1 1.0 1]", 1),
            ("[0.5 1 2]", 1),
            // Past 32 bits an integer is held as 0.
            ("[4294967296 1 1]", 3),
            ("[1 4294967297 1]", 3),
            // Within them, it's a number.
            ("[4294967295 1 1]", 1),
        ] {
            assert_eq!(counted(&cal(point), &[]), Some(want), "{point}");
        }
        let elements = "[/CalGray << /WhitePoint [5 0 R 1 1] >>]";
        assert_eq!(counted(elements, &["1"]), Some(1));
        assert_eq!(counted(elements, &["6 0 R", "1"]), Some(3));
        let array = "[/CalGray << /WhitePoint 5 0 R >>]";
        assert_eq!(counted(array, &["[1 1 1]"]), Some(1));
        assert_eq!(counted(array, &["6 0 R", "[1 1 1]"]), Some(3));
    }

    #[test]
    fn a_family_is_its_first_four_bytes() {
        each(&[
            ("[(CalGray) << /WhitePoint [1 1 1] >>]", 1),
            ("[/CalGrayX << /WhitePoint [1 1 1] >>]", 1),
            ("[/CalG << /WhitePoint [1 1 1] >>]", 1),
            ("[/Cal << /WhitePoint [1 1 1] >>]", 3),
            ("[/IndexedX /RGB 1 <00>]", 1),
            ("[/Inde /RGB 1 <00>]", 1),
            ("[/Ind /RGB 1 <00>]", 3),
            ("[/I /RGB 1 <00>]", 1),
            ("[/Iz /RGB 1 <00>]", 3),
            ("[(I) /RGB 1 <00>]", 1),
            ("[/PatternX /DeviceRGB]", 4),
            ("[(Pattern) /DeviceRGB]", 4),
            ("[/SeparationX /X /DeviceCMYK]", 1),
            // Lab counts 3 whichever way, so as an underlying space.
            ("[/Pattern [/Lab << /WhitePoint [1 1 1] >>]]", 4),
            ("[/Pattern [/Labz << /WhitePoint [1 1 1] >>]]", 1),
            ("[/Pattern [/Lab#00zz << /WhitePoint [1 1 1] >>]]", 4),
        ]);
        let by_reference = "[5 0 R /DeviceRGB]";
        assert_eq!(counted(by_reference, &["/Pattern"]), Some(4));
        assert_eq!(counted(by_reference, &["6 0 R", "/Pattern"]), Some(4));
        let thrice = ["6 0 R", "7 0 R", "/Pattern"];
        assert_eq!(counted(by_reference, &thrice), Some(3));
    }

    #[test]
    fn an_icc_based_space_is_its_n() {
        let icc = |n: &str| counted("[/ICCBased 5 0 R]", &[&format!("<< /Length 0 /N {n} >>")]);
        for (n, want) in [
            ("1", 1),
            ("2", 3),
            ("3", 3),
            ("4", 4),
            ("5", 3),
            ("0", 3),
            ("-1", 3),
            ("3.0", 3),
            ("3.7", 3),
            ("4.7", 4),
            ("1.5", 1),
            ("true", 1),
            ("false", 3),
            ("(4)", 3),
            ("/4", 3),
            ("1.99999999999", 3),
            ("3.9999999999999", 4),
            ("4294967300", 3),
            ("4294967297", 3),
            ("2147483652", 3),
        ] {
            assert_eq!(icc(n), Some(want), "/N {n}");
        }
        let no_n = counted("[/ICCBased 5 0 R]", &["<< /Length 0 >>"]);
        assert_eq!(no_n, Some(3));
        assert_eq!(counted("[/ICCBased << /N 4 >>]", &[]), Some(3));
        assert_eq!(counted("[/ICCBased 7]", &[]), Some(3));
        assert_eq!(
            counted("[/ICCBasedX 5 0 R]", &["<< /Length 0 /N 4 >>"]),
            Some(4)
        );
        // /N by a reference: one is followed, a second isn't.
        let by_reference = "<< /Length 0 /N 6 0 R >>";
        assert_eq!(counted("[/ICCBased 5 0 R]", &[by_reference, "4"]), Some(4));
        assert_eq!(
            counted("[/ICCBased 5 0 R]", &[by_reference, "true"]),
            Some(1)
        );
        let chain = [by_reference, "7 0 R", "4"];
        assert_eq!(counted("[/ICCBased 5 0 R]", &chain), Some(3));
        // The stream through two isn't read.
        let through_two = ["6 0 R", "<< /Length 0 /N 4 >>"];
        assert_eq!(counted("[/ICCBased 5 0 R]", &through_two), Some(3));
    }

    #[test]
    fn an_indexed_space_needs_a_base_and_a_table_that_is_there() {
        each(&[
            ("[/Indexed /DeviceRGB 1 <00>]", 1),
            ("[/I /RGB 1 <00>]", 1),
            ("[/Indexed /DeviceRGB 1]", 3),
            ("[/Indexed /DeviceRGB]", 3),
            ("[/Indexed /Foo 1 <00>]", 3),
            ("[/Indexed /Pattern 1 <00>]", 3),
            ("[/Indexed [/Indexed /G 1 <00>] 1 <00>]", 3),
            ("[/Indexed [/G] 1 <00>]", 1),
            ("[/Indexed [[/G]] 1 <00>]", 1),
            ("[/Indexed /DeviceRGB 999 <00>]", 1),
            ("[/Indexed /DeviceRGB -1 <00>]", 1),
            ("[/Indexed /DeviceRGB (x) <00>]", 1),
            ("[/Indexed /DeviceRGB 1 7]", 1),
            ("[/Indexed /DeviceRGB 1 null]", 1),
            ("[/Indexed /DeviceRGB 1 99 0 R]", 3),
        ]);
        let separation = format!("[/Indexed [/Separation /X /DeviceCMYK {F2_4}] 1 <00>]");
        assert_eq!(counted(&separation, &[]), Some(1));
        let by_reference = "[/Indexed 5 0 R 1 <00>]";
        assert_eq!(counted(by_reference, &["/DeviceRGB"]), Some(1));
        assert_eq!(counted(by_reference, &["6 0 R", "/DeviceRGB"]), Some(3));
        // The first path takes no stream.
        assert_eq!(counted(by_reference, &["<< /Length 0 /A /G >>"]), Some(3));
        // Itself as its base.
        assert_eq!(counted(by_reference, &[by_reference]), Some(3));
        let table = "[/Indexed /G 1 5 0 R]";
        assert_eq!(counted(table, &["null"]), Some(1));
        assert_eq!(counted(table, &["6 0 R", "<00>"]), Some(1));
    }

    #[test]
    fn a_separation_needs_an_alternate_but_no_function() {
        each(&[
            ("[/Separation /None]", 1),
            ("[/Separation (None) 7]", 1),
            ("[/Separation /X]", 3),
            ("[/Separation /X /Foo]", 3),
            ("[/Separation /X /DeviceCMYK]", 1),
            ("[/Separation /X /Pattern]", 3),
            ("[/Separation /X [/Indexed /G 1 <00>]]", 3),
            ("[/Separation /X [/Separation /Y /G]]", 3),
            ("[/Separation /All /G]", 1),
            ("[/Separation 7 /DeviceGray]", 1),
            // The second path reads an array of one by its bytes.
            ("[/Separation /X [(DeviceCMYK)]]", 1),
            ("[/Separation /X [/Foo]]", 3),
            ("[/Separation /X [[/G]]]", 3),
            ("[/Separation /X [/Pattern]]", 3),
        ]);
        let stream_of = |value: &str| format!("<< /Length 0 /A {value} >>");
        let alternate = "[/Separation /X 5 0 R]";
        assert_eq!(counted(alternate, &[&stream_of("/DeviceCMYK")]), Some(1));
        assert_eq!(counted(alternate, &[&stream_of("/Foo")]), Some(3));
        assert_eq!(counted(alternate, &[&stream_of("/Pattern")]), Some(3));
        assert_eq!(counted(alternate, &["6 0 R", "/DeviceRGB"]), Some(3));
        // None by a reference, and through two it's no name.
        assert_eq!(counted("[/Separation 5 0 R]", &["/None"]), Some(1));
        assert_eq!(counted("[/Separation 5 0 R]", &["6 0 R", "/None"]), Some(3));
    }

    #[test]
    fn a_stream_for_a_space_is_the_first_of_its_keys_that_names_one() {
        // The alternate's components are what tells them apart, against a
        // function of one output.
        let device_n = format!("[/DeviceN [/A] 5 0 R {F2_1}]");
        let with = |dict: &str| counted(&device_n, &[&format!("<< /Length 0 {dict} >>")]);
        assert_eq!(with("/Z /DeviceGray"), Some(1));
        assert_eq!(with("/Z /DeviceGray /A /DeviceCMYK"), Some(3));
        assert_eq!(with("/A /DeviceCMYK /Z /DeviceGray"), Some(3));
        assert_eq!(with("/A /DeviceCMYK /A /DeviceGray"), Some(1));
        assert_eq!(with("/A /DeviceGray /A /Foo"), Some(3));
        assert_eq!(with("/A /Foo /Z /DeviceGray"), Some(1));
        assert_eq!(with("/ /DeviceCMYK /Z /DeviceGray"), Some(1));
        // A value by reference is no name there.
        assert_eq!(with("/A 9 0 R /Z /DeviceGray"), Some(1));
    }

    #[test]
    fn a_device_n_needs_an_alternate_and_a_function_with_enough_outputs() {
        let f2 = |more: &str| format!("<< /FunctionType 2 /Domain [0 1] /N 1 {more} >>");
        for (function, want) in [
            (f2("/C0 [0 0 0 0] /C1 [1 1 1 1]"), 2),
            (f2("/C0 [0 0 0]"), 3),
            (f2(""), 3),
            (f2("/Range [0 1 0 1 0 1 0 1]"), 2),
            // A Range's count before /C0's.
            (f2("/Range [0 1] /C0 [0 0 0 0]"), 3),
            ("<< /FunctionType 1 /Domain [0 1] >>".to_string(), 3),
            ("/Identity".to_string(), 3),
            ("null".to_string(), 3),
            (
                format!(
                    "<< /FunctionType 3 /Domain [0 1] /Functions [{F2_4}] /Bounds [] /Encode [0 1] >>"
                ),
                2,
            ),
        ] {
            assert_eq!(
                counted(&device_n_2(&function), &[]),
                Some(want),
                "{function}"
            );
        }
        each(&[
            ("[/DeviceN [/A /B] /DeviceCMYK]", 3),
            ("[/DeviceN /A /DeviceGray]", 3),
        ]);
        let of = |names: &str, alternate: &str| format!("[/DeviceN {names} {alternate} {F2_1}]");
        assert_eq!(counted(&of("[]", "/DeviceGray"), &[]), Some(3));
        assert_eq!(counted(&of("/A", "/DeviceGray"), &[]), Some(3));
        assert_eq!(
            counted(&of("[/A /B /C /D /E /F /G /H /I]", "/G"), &[]),
            Some(9)
        );
        assert_eq!(counted(&of("[7 (x)]", "/DeviceGray"), &[]), Some(2));
        assert_eq!(counted(&of("[/A /B]", "/Foo"), &[]), Some(3));
        assert_eq!(counted(&of("[/A /B]", "/Pattern"), &[]), Some(3));
        assert_eq!(
            counted(&of("[/A /B]", "[/Indexed /G 1 <00>]"), &[]),
            Some(3)
        );
        // Names through one reference, and two.
        assert_eq!(counted(&of("5 0 R", "/G"), &["[/A /B]"]), Some(2));
        assert_eq!(counted(&of("5 0 R", "/G"), &["6 0 R", "[/A /B]"]), Some(3));
        // A function through two isn't read.
        let by_reference = "[/DeviceN [/A] /DeviceGray 5 0 R]";
        assert_eq!(counted(by_reference, &[F2_1]), Some(1));
        assert_eq!(counted(by_reference, &["6 0 R", F2_1]), Some(3));
        let more = format!("[/DeviceN [/A /B] /DeviceGray {F2_1} << /Subtype /NChannel >>]");
        assert_eq!(counted(&more, &[]), Some(2));
    }

    /// `[/DeviceN [/A /B] /DeviceGray 5 0 R]`, 5 a stream of this dictionary,
    /// with data never read.
    fn with_stream_function(dict: &str) -> Option<u32> {
        let stream = format!("<< /Length 0 {dict} >>");
        counted("[/DeviceN [/A /B] /DeviceGray 5 0 R]", &[&stream])
    }

    /// The same, 5 a function written with no stream.
    fn with_dict_function(dict: &str) -> Option<u32> {
        counted(
            "[/DeviceN [/A /B] /DeviceGray 5 0 R]",
            &[&format!("<< {dict} >>")],
        )
    }

    #[test]
    fn a_sampled_function_is_refused_by_its_dictionary() {
        let f0 = "/FunctionType 0 /Domain [0 1] /Range [0 1] ";
        for (more, want) in [
            ("/Size [2] /BitsPerSample 8", 2),
            ("/BitsPerSample 8", 3),
            ("/Size [] /BitsPerSample 8", 3),
            ("/Size [0] /BitsPerSample 8", 3),
            ("/Size [-1] /BitsPerSample 8", 3),
            ("/Size 2 /BitsPerSample 8", 3),
            ("/Size [2] /BitsPerSample 7", 3),
            ("/Size [2]", 3),
            ("/Size [2] /BitsPerSample true", 2),
            ("/Size [2] /BitsPerSample 8.9", 2),
            ("/Size [2.7] /BitsPerSample 8", 2),
            ("/Size [true] /BitsPerSample 8", 2),
            ("/Size [(2)] /BitsPerSample 8", 3),
            ("/Size [536870912] /BitsPerSample 32", 3),
            // 32 bits' worth, past the room to round it up to bytes.
            ("/Size [2147483647] /BitsPerSample 2", 3),
            ("/Size [536870911] /BitsPerSample 8", 2),
        ] {
            assert_eq!(
                with_stream_function(&format!("{f0}{more}")),
                Some(want),
                "{more}"
            );
        }
        for bits in [1, 2, 4, 8, 12, 16, 24, 32] {
            let more = format!("{f0}/Size [2] /BitsPerSample {bits}");
            assert_eq!(with_stream_function(&more), Some(2), "{bits}");
        }
        for bits in [0, 3, 5, 64, -8] {
            let more = format!("{f0}/Size [2] /BitsPerSample {bits}");
            assert_eq!(with_stream_function(&more), Some(3), "{bits}");
        }
        // Shorter than its inputs.
        let two = "/FunctionType 0 /Domain [0 1 0 1] /Range [0 1] /BitsPerSample 8 /Size";
        assert_eq!(with_stream_function(&format!("{two} [2 2]")), Some(2));
        assert_eq!(with_stream_function(&format!("{two} [2]")), Some(3));
        // Asks a stream and a /Range.
        assert_eq!(
            with_dict_function(&format!("{f0}/Size [2] /BitsPerSample 8")),
            Some(3)
        );
        let no_range = "/FunctionType 0 /Domain [0 1] /Size [2] /BitsPerSample 8";
        assert_eq!(with_stream_function(no_range), Some(3));
        // Any /FunctionType that reads as 0 is one.
        let untyped = "/Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8";
        assert_eq!(with_stream_function(untyped), Some(2));
        assert_eq!(
            with_stream_function(&format!("/FunctionType (2) {untyped}")),
            Some(2)
        );
        assert_eq!(
            with_stream_function(&format!("/FunctionType false {untyped}")),
            Some(2)
        );
        assert_eq!(with_dict_function(untyped), Some(3));
    }

    #[test]
    fn a_post_script_function_asks_a_stream_and_a_range() {
        let f4 = "/FunctionType 4 /Domain [0 1] /Range [0 1]";
        assert_eq!(with_stream_function(f4), Some(2));
        assert_eq!(with_dict_function(f4), Some(3));
        assert_eq!(
            with_stream_function("/FunctionType 4 /Domain [0 1]"),
            Some(3)
        );
    }

    #[test]
    fn a_function_type_and_domain_are_read_as_pdfium_reads_them() {
        for (dict, want) in [
            ("/FunctionType 2.5 /Domain [0 1] /N 1", 2),
            ("/FunctionType true /Domain [0 1] /N 1", 3),
            ("/Domain [0 1] /N 1", 3),
            ("/FunctionType 1 /Domain [0 1] /N 1", 3),
            ("/FunctionType 5 /Domain [0 1] /N 1", 3),
            ("/FunctionType 2 /Domain [0 1 0] /N 1", 2),
            ("/FunctionType 2 /Domain [0] /N 1", 3),
            ("/FunctionType 2 /Domain [] /N 1", 3),
            ("/FunctionType 2 /N 1", 3),
            ("/FunctionType 2 /Domain 7 /N 1", 3),
            ("/FunctionType 2 /Domain [0 1] /N 1.5", 2),
            ("/FunctionType 2 /Domain [0 1] /N true", 3),
            ("/FunctionType 2 /Domain [0 1]", 3),
        ] {
            assert_eq!(with_dict_function(dict), Some(want), "{dict}");
        }
        let by_reference = |dict: &str, then: &[&str]| {
            let function = format!("<< {dict} >>");
            let mut extra = vec![function.as_str()];
            extra.extend_from_slice(then);
            counted("[/DeviceN [/A /B] /DeviceGray 5 0 R]", &extra)
        };
        let domain = "/FunctionType 2 /N 1 /Domain 6 0 R";
        assert_eq!(by_reference(domain, &["[0 1]"]), Some(2));
        assert_eq!(by_reference(domain, &["7 0 R", "[0 1]"]), Some(3));
        // A Type 2's /N is written where it is: a reference is no number.
        let exponent = "/FunctionType 2 /Domain [0 1] /N 6 0 R";
        assert_eq!(by_reference(exponent, &["1"]), Some(3));
    }

    #[test]
    fn a_type_2_function_has_its_outputs_for_each_input() {
        let device_n = |function: &str| counted(&device_n_2(function), &[]);
        let inputs = "<< /FunctionType 2 /Domain [0 1 0 1] /N 1 /C0 [0 0] /C1 [1 1] >>";
        assert_eq!(device_n(inputs), Some(2));
        let empty = "<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [] >>";
        assert_eq!(device_n(empty), Some(3));
        let no_c0 = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] >>";
        assert_eq!(device_n(no_c0), Some(3));
    }

    #[test]
    fn a_stitching_function_is_its_subfunctions() {
        let stitched = |domain: &str, kids: &str, bounds: &str, encode: &str| {
            format!(
                "<< /FunctionType 3 /Domain {domain} /Functions {kids} /Bounds {bounds} /Encode {encode} >>"
            )
        };
        let f1 = "<< /FunctionType 2 /Domain [0 1] /N 1 >>";
        let f2_in = "<< /FunctionType 2 /Domain [0 1 0 1] /N 1 >>";
        let f3 = "<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0] >>";
        let dn = |function: &str, extra: &[&str]| {
            counted(&format!("[/DeviceN [/A /B] /DeviceGray {function}]"), extra)
        };
        let one = format!("[{f1}]");
        let two = format!("[{f1} {f1}]");
        assert_eq!(dn(&stitched("[0 1]", &one, "[]", "[0 1]"), &[]), Some(2));
        assert_eq!(
            dn(&stitched("[0 1]", &two, "[0.5]", "[0 1 0 1]"), &[]),
            Some(2)
        );
        // Two inputs.
        assert_eq!(
            dn(&stitched("[0 1 0 1]", &one, "[]", "[0 1]"), &[]),
            Some(3)
        );
        // No subfunction, or none with one input, or unalike.
        assert_eq!(dn(&stitched("[0 1]", "[]", "[]", "[]"), &[]), Some(3));
        assert_eq!(dn(&stitched("[0 1]", "7", "[]", "[0 1]"), &[]), Some(3));
        let kid = format!("[{f2_in}]");
        assert_eq!(dn(&stitched("[0 1]", &kid, "[]", "[0 1]"), &[]), Some(3));
        let unalike = format!("[{f1} {f3}]");
        assert_eq!(
            dn(&stitched("[0 1]", &unalike, "[0]", "[0 1 0 1]"), &[]),
            Some(3)
        );
        // /Bounds and /Encode long enough, and more is let by.
        assert_eq!(
            dn(&stitched("[0 1]", &two, "[]", "[0 1 0 1]"), &[]),
            Some(3)
        );
        assert_eq!(dn(&stitched("[0 1]", &two, "[0.5]", "[0 1]"), &[]), Some(3));
        assert_eq!(
            dn(&stitched("[0 1]", &two, "[0 0 0]", "[0 1 0 1 0 1]"), &[]),
            Some(2)
        );
        assert_eq!(dn(&stitched("[0 1]", &one, "7", "[0 1]"), &[]), Some(3));
        assert_eq!(dn(&stitched("[0 1]", &one, "[]", "7"), &[]), Some(3));
        // Its outputs are its subfunctions', whatever its /Range says.
        let ranged = format!(
            "<< /FunctionType 3 /Domain [0 1] /Range [0 1 0 1 0 1 0 1] /Functions [{f1}] /Bounds [] /Encode [0 1] >>"
        );
        let in_cmyk = counted(&device_n_2(&ranged), &[]);
        assert_eq!(in_cmyk, Some(3));
        // Itself among its subfunctions, directly and by another.
        let selfish = stitched("[0 1]", "[5 0 R]", "[]", "[0 1]");
        assert_eq!(dn("5 0 R", &[&selfish]), Some(3));
        let other = stitched("[0 1]", "[6 0 R]", "[]", "[0 1]");
        assert_eq!(dn("5 0 R", &[&other, &selfish]), Some(3));
        // A subfunction twice is no loop.
        let twice = stitched("[0 1]", "[6 0 R 6 0 R]", "[0]", "[0 1 0 1]");
        assert_eq!(dn("5 0 R", &[&twice, f1]), Some(2));
        // Through two, a subfunction isn't read.
        let by_reference = stitched("[0 1]", "[6 0 R]", "[]", "[0 1]");
        assert_eq!(dn("5 0 R", &[&by_reference, f1]), Some(2));
        assert_eq!(dn("5 0 R", &[&by_reference, "7 0 R", f1]), Some(3));
    }

    #[test]
    fn a_pattern_space_counts_what_it_underlies_and_its_name() {
        each(&[
            ("[/Pattern /DeviceRGB]", 4),
            ("[/Pattern /DeviceCMYK]", 5),
            ("[/Pattern /DeviceGray]", 2),
            ("[/Pattern /Foo]", 1),
            ("[/Pattern 7]", 1),
            ("[/Pattern null]", 1),
            ("[/Pattern 99 0 R]", 1),
            ("[/Pattern /Pattern]", 3),
            ("[/Pattern [/Pattern]]", 3),
            ("[/Pattern [/Pattern /G]]", 3),
            ("[/Pattern [/Indexed /G 1 <00>]]", 2),
            ("[/Pattern /DeviceRGB 7 7 7]", 4),
            ("[/Pattern [/RGB]]", 4),
            ("[/Pattern [/Lab 7]]", 1),
            ("[/Pattern [/Separation /X /G]]", 2),
        ]);
        let of = |n: usize| {
            let names = "/A ".repeat(n);
            format!("[/Pattern [/DeviceN [{names}] /DeviceGray {F2_1}]]")
        };
        assert_eq!(counted(&of(16), &[]), Some(17));
        assert_eq!(counted(&of(17), &[]), Some(3));
        // Itself, as the value's own and as what a value holds.
        assert_eq!(counted("5 0 R", &["[/Pattern 5 0 R]"]), Some(3));
        assert_eq!(counted("[/Pattern 5 0 R]", &["[/Pattern 5 0 R]"]), Some(1));
        assert_eq!(
            counted("[/Pattern 5 0 R]", &["<< /Length 0 /A /G >>"]),
            Some(1)
        );
    }

    #[test]
    fn a_loop_does_not_load_and_siblings_may_share() {
        let f = F2_1;
        let separation = format!("[/Separation /A 5 0 R {f}]");
        assert_eq!(counted("[5 0 R]", &["[5 0 R]"]), Some(3));
        assert_eq!(counted("5 0 R", &["[5 0 R]"]), Some(3));
        assert_eq!(
            counted("5 0 R", &["[/Indexed 6 0 R 1 <00>]", &separation]),
            Some(3)
        );
        // By arrays of one too, whichever path gets there first.
        let looped = ["[/Pattern 6 0 R]", "[5 0 R]"];
        assert_eq!(counted("5 0 R", &looped), Some(1));
        let two_patterns = ["[/Pattern 6 0 R]", "[/Pattern 5 0 R]"];
        assert_eq!(counted("5 0 R", &two_patterns), Some(3));
        // The same object twice on one path is a loop; on two, it's not.
        let diamond = "[/Indexed 5 0 R 1 <00>]";
        let extras = [
            separation.replace("5 0 R", "6 0 R"),
            "[/ICCBased 7 0 R]".to_string(),
            "<< /Length 0 /N 3 /Alternate 6 0 R >>".to_string(),
        ];
        let extras: Vec<&str> = extras.iter().map(String::as_str).collect();
        assert_eq!(counted(diamond, &extras), Some(1));
        let shared = "[/DeviceN [/A] 5 0 R << /FunctionType 3 /Domain [0 1] /Functions [6 0 R 6 0 R] /Bounds [0] /Encode [0 1 0 1] >>]";
        assert_eq!(counted(shared, &["/G", f]), Some(1));
    }

    /// A chain of `n` arrays of one, by reference, down to `/DeviceCMYK`.
    fn chain(n: u32) -> (Object, BTreeMap<ObjectId, Object>) {
        let mut objects = BTreeMap::new();
        for at in 0..n {
            objects.insert(
                ObjectId::new(5 + at, 0),
                Object::from(vec![Object::reference(6 + at)]),
            );
        }
        objects.insert(ObjectId::new(5 + n, 0), Object::from("DeviceCMYK"));
        (Object::reference(5), objects)
    }

    #[test]
    fn past_the_depth_a_chain_does_not_load() {
        let (value, objects) = chain(64);
        let resolve = |id: ObjectId| objects.get(&id);
        assert_eq!(components(resolve, &value, &Limits::DEFAULT), Ok(Some(4)));
        let mut small = Limits::DEFAULT;
        small.space_depth = 63;
        assert_eq!(components(resolve, &value, &small), Ok(Some(3)));
        assert!(super::counted(resolve, &value, &small).unwrap().capped);
        let (value, objects) = chain(65);
        let resolve = |id: ObjectId| objects.get(&id);
        assert_eq!(components(resolve, &value, &Limits::DEFAULT), Ok(Some(3)));
        let mut large = Limits::DEFAULT;
        large.space_depth = 65;
        assert_eq!(components(resolve, &value, &large), Ok(Some(4)));
        assert!(!super::counted(resolve, &value, &large).unwrap().capped);
    }

    /// A resolver over `objects` that counts what it's asked.
    struct Counting<'a> {
        objects: &'a BTreeMap<ObjectId, Object>,
        asked: &'a Cell<usize>,
    }

    impl<'a> Resolve<'a> for &Counting<'a> {
        fn resolve(&self, id: ObjectId) -> Option<&'a Object> {
            self.asked.set(self.asked.get() + 1);
            // What the budget's there to stop, stopped here if it doesn't.
            assert!(self.asked.get() <= 1 << 16, "asked without end");
            self.objects.get(&id)
        }
    }

    #[test]
    fn past_the_visits_a_branch_does_not_load() {
        let (value, objects) = chain(10);
        let asked = Cell::new(0);
        let counting = Counting {
            objects: &objects,
            asked: &asked,
        };
        let mut limits = Limits::DEFAULT;
        assert_eq!(components(&counting, &value, &limits), Ok(Some(4)));
        assert_eq!(asked.get(), 11);
        limits.space_visits = 11;
        assert_eq!(components(&counting, &value, &limits), Ok(Some(4)));
        limits.space_visits = 10;
        asked.set(0);
        assert_eq!(components(&counting, &value, &limits), Ok(Some(3)));
        assert_eq!(asked.get(), 10);
        limits.space_visits = 0;
        asked.set(0);
        assert_eq!(components(&counting, &value, &limits), Ok(Some(3)));
        assert_eq!(asked.get(), 0);
    }

    /// Stitching functions `levels` deep, each with `width` subfunctions, the
    /// same object, by reference: `width` to the `levels` to walk, were nothing
    /// to stop it. The value's object 5.
    fn stitched(levels: u32, width: usize) -> BTreeMap<ObjectId, Object> {
        let mut objects = BTreeMap::new();
        for level in 0..levels {
            let kids = format!("{} 0 R ", 6 + level).repeat(width);
            let (bounds, encode) = ("0 ".repeat(width - 1), "0 1 ".repeat(width));
            let text = format!(
                "<< /FunctionType 3 /Domain [0 1] /Functions [{kids}] /Bounds [{bounds}] /Encode [{encode}] >>"
            );
            objects.insert(ObjectId::new(5 + level, 0), value(&text));
        }
        objects.insert(ObjectId::new(5 + levels, 0), value(F2_1));
        objects
    }

    #[test]
    fn the_budget_is_spent_on_a_function_s_subfunctions() {
        let device_n = value("[/DeviceN [/A /B] /DeviceGray 5 0 R]");
        // Three levels of two: within the budget, it loads, and each is asked.
        let objects = stitched(3, 2);
        let asked = Cell::new(0);
        let counting = Counting {
            objects: &objects,
            asked: &asked,
        };
        assert_eq!(
            components(&counting, &device_n, &Limits::DEFAULT),
            Ok(Some(2))
        );
        assert_eq!(asked.get(), 1 + 2 + 4 + 8);
        // Thirty levels of two is past any budget, and stops at it.
        let objects = stitched(30, 2);
        let asked = Cell::new(0);
        let counting = Counting {
            objects: &objects,
            asked: &asked,
        };
        assert_eq!(
            components(&counting, &device_n, &Limits::DEFAULT),
            Ok(Some(3))
        );
        assert_eq!(asked.get(), Limits::DEFAULT.space_visits);
    }

    #[test]
    fn subfunctions_written_in_place_spend_it_too() {
        let kids = format!("{F2_1} ").repeat(100);
        let (bounds, encode) = ("0 ".repeat(99), "0 1 ".repeat(100));
        let function = format!(
            "<< /FunctionType 3 /Domain [0 1] /Functions [{kids}] /Bounds [{bounds}] /Encode [{encode}] >>"
        );
        let space = value(&format!("[/DeviceN [/A /B] /DeviceGray {function}]"));
        let nothing = |_: ObjectId| None;
        assert_eq!(components(nothing, &space, &Limits::DEFAULT), Ok(Some(2)));
        let mut limits = Limits::DEFAULT;
        limits.space_visits = 99;
        assert_eq!(components(nothing, &space, &limits), Ok(Some(3)));
    }

    /// A resolver that fails each lookup for `why`.
    #[derive(Clone, Copy)]
    struct Failing {
        why: Why,
    }

    impl<'a> Resolve<'a> for Failing {
        fn resolve(&self, _: ObjectId) -> Option<&'a Object> {
            None
        }

        fn try_resolve(&self, id: ObjectId) -> Result<Option<&'a Object>, Unreadable> {
            Err(Unreadable::new(id, self.why, "it fails"))
        }
    }

    #[test]
    fn a_failed_read_is_nothing_there_if_damaged_and_otherwise_the_error() {
        // Each place in a space where an object's asked for by reference.
        let places = [
            "5 0 R",
            "[5 0 R]",
            "[5 0 R /DeviceRGB]",
            "[/CalGray 5 0 R]",
            "[/CalGray << /WhitePoint 5 0 R >>]",
            "[/CalGray << /WhitePoint [5 0 R 1 1] >>]",
            "[/ICCBased 5 0 R]",
            "[/Indexed 5 0 R 1 <00>]",
            "[/Indexed /G 1 5 0 R]",
            "[/Separation 5 0 R]",
            "[/Separation /X 5 0 R]",
            "[/DeviceN 5 0 R /G]",
            "[/DeviceN [/A] 5 0 R]",
            "[/DeviceN [/A] /G 5 0 R]",
            "[/DeviceN [/A] /G << /FunctionType 5 0 R >>]",
            "[/DeviceN [/A] /G << /FunctionType 2 /Domain 5 0 R >>]",
            "[/DeviceN [/A] /G << /FunctionType 3 /Domain [0 1] /Functions 5 0 R >>]",
            "[/DeviceN [/A] /G << /FunctionType 3 /Domain [0 1] /Functions [5 0 R] /Bounds [] /Encode [0 1] >>]",
            "[/Pattern 5 0 R]",
        ];
        for text in places {
            let parsed = value(text);
            let want = counted(text, &[]);
            let damaged = Failing { why: Why::Damaged };
            let read = components(damaged, &parsed, &Limits::DEFAULT);
            assert_eq!(read, Ok(want), "{text}");
            // `Why::Filter` only a stream's data is, never a resolver's answer.
            for why in [Why::Capped, Why::Locked, Why::Source] {
                let read = components(Failing { why }, &parsed, &Limits::DEFAULT);
                let error = read.expect_err(text);
                assert_eq!((error.id, error.why), (ObjectId::new(5, 0), why), "{text}");
            }
        }
    }

    #[test]
    fn it_takes_a_reading_a_dyn_and_a_closure() {
        let objects = objects(&["/DeviceCMYK"]);
        let closure = |id: ObjectId| objects.get(&id);
        let dynamic: &dyn Resolve = &closure;
        let reference = Object::reference(5);
        assert_eq!(
            components(closure, &reference, &Limits::DEFAULT),
            Ok(Some(4))
        );
        assert_eq!(
            components(dynamic, &reference, &Limits::DEFAULT),
            Ok(Some(4))
        );
        // Nothing's copied that a caller could see, nor kept between calls.
        let seen = RefCell::new(Vec::new());
        let recording = |id: ObjectId| {
            seen.borrow_mut().push(id);
            objects.get(&id)
        };
        assert_eq!(
            components(recording, &reference, &Limits::DEFAULT),
            Ok(Some(4))
        );
        assert_eq!(
            components(recording, &reference, &Limits::DEFAULT),
            Ok(Some(4))
        );
        assert_eq!(*seen.borrow(), [ObjectId::new(5, 0), ObjectId::new(5, 0)]);
    }
}
