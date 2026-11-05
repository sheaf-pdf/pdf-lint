//! What reads a [`Space`]. It reads what a space holds whatever its sink, and what
//! only a check reads behind the sink's `wants`, on a budget of the check's own,
//! so what a sink wants can't change what's given. Its references are its own to
//! follow, each lookup a visit, so that a chain, a loop and each cap can be told.

use std::cell::Cell;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Entry, Object, ObjectId, Resolve, Span, Unreadable, Value, Why};

use crate::count::{Hop, Walk, float};
use crate::fault::Fault;
use crate::function;
use crate::limits::Limits;

use super::{
    Attributes, Cie, Colorant, ColorantSpace, DeviceN, Family, Icc, Indexed, Lookup, Process,
    Separation, Space, Subtype,
};

/// A value after its references.
enum Reached<'o> {
    /// What they lead to, itself no reference.
    At(&'o Object),
    /// Nothing: a reference to an object the file hasn't got.
    Nothing,
    /// A loop, a cap or a damaged object, each told.
    Stopped,
}

/// An array of names: each element's, by its position, and how many elements it
/// has, whatever they are.
#[derive(Default)]
struct Names<'o> {
    names: Vec<Option<&'o [u8]>>,
    elements: usize,
}

/// A resolver that notes the first object it finds damaged, for what walks a
/// function, which reads one as no object at all.
struct Noting<'a, R> {
    objects: &'a R,
    damaged: &'a Cell<Option<ObjectId>>,
}

impl<'o, R: Resolve<'o>> Resolve<'o> for Noting<'_, R> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        let read = self.objects.try_resolve(id);
        if read.as_ref().is_err_and(|e| e.why == Why::Damaged) && self.damaged.get().is_none() {
            self.damaged.set(Some(id));
        }
        read
    }
}

/// One space's reading: what's been spent, and what's on the way down.
pub(super) struct Reader<'l, R, S: ?Sized> {
    objects: R,
    limits: &'l Limits,
    sink: &'l mut S,
    /// What's been looked up for what a space holds, and whether the cap's told.
    visits: usize,
    visits_told: bool,
    /// What's been looked up for what only a check reads, and whether the cap's
    /// told. Apart, so that what a sink wants can't use up what a space holds.
    checks: usize,
    checks_told: bool,
    /// Whether a check is reading.
    checking: bool,
    /// The objects on the way down, by number: one seen again is a loop. At most
    /// a level longer than the depth lets it be.
    path: Vec<ObjectId>,
}

/// `name`'s family, its abbreviation among them, and whether it is one.
fn family_of(name: &[u8]) -> Option<(Family, bool)> {
    match name {
        b"G" => Some((Family::DeviceGray, true)),
        b"RGB" => Some((Family::DeviceRGB, true)),
        b"CMYK" => Some((Family::DeviceCMYK, true)),
        b"I" => Some((Family::Indexed, true)),
        _ => Family::of(name).map(|family| (family, false)),
    }
}

/// `dict`'s `key`, where it's anything but null, which is as if it weren't
/// written (7.3.9).
fn entry_of<'o>(dict: &'o Dict, key: &[u8]) -> Option<&'o Object> {
    dict.get(key).filter(|value| !value.is_null())
}

impl<'l, 'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized> Reader<'l, R, S> {
    pub(super) fn new(objects: R, limits: &'l Limits, sink: &'l mut S) -> Reader<'l, R, S> {
        Reader {
            objects,
            limits,
            sink,
            visits: 0,
            visits_told: false,
            checks: 0,
            checks_told: false,
            checking: false,
            path: Vec::new(),
        }
    }

    /// Tells a fault, if it's wanted, in the object it's reading.
    fn say(&mut self, at: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            let object = self.path.last().copied();
            self.sink.fault(Place::new(object, at), fault);
        }
    }

    /// One visit spent, or none left to spend, which is told once for what a
    /// space holds and once for what its checks read.
    fn spend(&mut self, at: Option<Span>) -> bool {
        let (spent, told) = if self.checking {
            (&mut self.checks, &mut self.checks_told)
        } else {
            (&mut self.visits, &mut self.visits_told)
        };
        if *spent < self.limits.space_visits {
            *spent += 1;
            return true;
        }
        let told = std::mem::replace(told, true);
        if !told {
            self.say(at, Fault::TooManyVisits);
        }
        false
    }

    /// `written` after its references, each put on the path.
    fn through(&mut self, written: &'o Object) -> Result<Reached<'o>, Unreadable> {
        let mut object = written;
        let mut followed = 0;
        while let Some(id) = object.as_reference() {
            followed += 1;
            if followed == 2 {
                // In the object that's a reference, which is on the path by now.
                self.say(object.at, Fault::Chained);
            }
            if self.path.contains(&id) {
                self.say(object.at, Fault::Loop);
                return Ok(Reached::Stopped);
            }
            if self.path.len() > self.limits.space_depth {
                self.say(object.at, Fault::TooDeep);
                return Ok(Reached::Stopped);
            }
            if !self.spend(object.at) {
                return Ok(Reached::Stopped);
            }
            match self.objects.try_resolve(id) {
                Ok(Some(next)) => {
                    self.path.push(id);
                    object = next;
                }
                Ok(None) => return Ok(Reached::Nothing),
                Err(unreadable) if unreadable.why == Why::Damaged => {
                    self.say(object.at, Fault::Damaged { id });
                    return Ok(Reached::Stopped);
                }
                Err(unreadable) => return Err(unreadable),
            }
        }
        Ok(Reached::At(object))
    }

    /// `read`, of what `written` leads to, with its references on the path, so a
    /// fault said of what it's given is said in the object it's written in; none
    /// where it leads nowhere, `nothing` told, of `written`, if it's a reference to
    /// nothing.
    fn entered<T>(
        &mut self,
        written: &'o Object,
        nothing: Option<Fault>,
        read: impl FnOnce(&mut Self, &'o Object) -> Result<Option<T>, Unreadable>,
    ) -> Result<Option<T>, Unreadable> {
        let mark = self.path.len();
        let read = match self.through(written) {
            Ok(Reached::At(object)) => read(self, object),
            Ok(Reached::Nothing) => {
                self.path.truncate(mark);
                if let Some(fault) = nothing {
                    self.say(written.at, fault);
                }
                Ok(None)
            }
            Ok(Reached::Stopped) => Ok(None),
            Err(unreadable) => Err(unreadable),
        };
        self.path.truncate(mark);
        read
    }

    /// What `written` leads to, `nothing` told if it's a reference to nothing. Its
    /// references are off the path again, so a fault said of what it's given is
    /// said of `written`; one said of what it leads to is said in [`Reader::entered`].
    fn value(
        &mut self,
        written: &'o Object,
        nothing: Option<Fault>,
    ) -> Result<Option<&'o Object>, Unreadable> {
        self.entered(written, nothing, |_, object| Ok(Some(object)))
    }

    /// `run`, for a sink that wants faults. What it reads comes off the check's
    /// own budget, and a failure to read ends it, unsaid, so that no sink's reading
    /// is an `Err` where `()`'s isn't.
    fn check(&mut self, run: impl FnOnce(&mut Self) -> Result<(), Unreadable>) {
        if !self.sink.wants() {
            return;
        }
        let mark = self.path.len();
        let checking = std::mem::replace(&mut self.checking, true);
        let _unsaid = run(self);
        self.checking = checking;
        self.path.truncate(mark);
    }

    /// The space `written` says, nested `depth` levels down.
    pub(super) fn space(
        &mut self,
        written: &'o Object,
        depth: usize,
    ) -> Result<Option<Space<'o>>, Unreadable> {
        if depth > self.limits.space_depth {
            self.say(written.at, Fault::TooDeep);
            return Ok(None);
        }
        self.entered(
            written,
            Some(Fault::NotASpace),
            |reader, object| match object.value() {
                Value::Name(name) => reader.named(name, &[], object.at, depth),
                Value::Array(parts) => {
                    let Some(first) = parts.first() else {
                        reader.say(object.at, Fault::NotASpace);
                        return Ok(None);
                    };
                    let family =
                        reader.entered(first, Some(Fault::NotASpace), |reader, family| {
                            if family.as_name().is_none() {
                                reader.say(family.at, Fault::NotASpace);
                            }
                            Ok(family.as_name())
                        })?;
                    let Some(name) = family else {
                        return Ok(None);
                    };
                    reader.named(name, parts, object.at, depth)
                }
                _ => {
                    reader.say(object.at, Fault::NotASpace);
                    Ok(None)
                }
            },
        )
    }

    /// The space of the family `name` says, with the array `parts` for its
    /// parameters, empty for a name alone.
    fn named(
        &mut self,
        name: &[u8],
        parts: &'o [Object],
        at: Option<Span>,
        depth: usize,
    ) -> Result<Option<Space<'o>>, Unreadable> {
        let Some((family, abbreviated)) = family_of(name) else {
            self.say(at, Fault::UnknownFamily);
            return Ok(None);
        };
        if abbreviated {
            self.say(at, Fault::Abbreviated);
        }
        let elements = match family {
            Family::DeviceGray | Family::DeviceRGB | Family::DeviceCMYK => 1,
            Family::Indexed | Family::Separation => 4,
            Family::DeviceN => 5,
            Family::CalGray | Family::CalRGB | Family::Lab | Family::ICCBased | Family::Pattern => {
                2
            }
        };
        if let Some(extra) = parts.get(elements) {
            self.say(extra.at, Fault::ExtraElements);
        }
        Ok(Some(match family {
            Family::DeviceGray => Space::DeviceGray,
            Family::DeviceRGB => Space::DeviceRGB,
            Family::DeviceCMYK => Space::DeviceCMYK,
            Family::CalGray => Space::CalGray(self.cie(family, parts, at)?),
            Family::CalRGB => Space::CalRGB(self.cie(family, parts, at)?),
            Family::Lab => Space::Lab(self.cie(family, parts, at)?),
            Family::ICCBased => Space::ICCBased(self.icc(parts, at, depth)?),
            Family::Indexed => Space::Indexed(self.indexed(parts, at, depth)?),
            Family::Separation => Space::Separation(self.separation(parts, at, depth)?),
            Family::DeviceN => Space::DeviceN(self.device_n(parts, at, depth)?),
            Family::Pattern => Space::Pattern(self.pattern(parts, depth)?),
        }))
    }

    /// `parts`' element, told as missing if it isn't there.
    fn element(
        &mut self,
        parts: &'o [Object],
        element: usize,
        at: Option<Span>,
    ) -> Option<&'o Object> {
        let part = parts.get(element);
        if part.is_none() {
            self.say(at, Fault::Missing { element });
        }
        part
    }

    /// The `count` numbers `written` is an array of, or `fault` told. One not
    /// required that's a reference to nothing or null isn't there.
    fn numbers(
        &mut self,
        written: &'o Object,
        count: usize,
        fault: Fault,
        required: bool,
    ) -> Result<Option<Vec<&'o Object>>, Unreadable> {
        let nothing = required.then_some(fault);
        self.entered(written, nothing, |reader, object| {
            if !required && object.is_null() {
                return Ok(None);
            }
            let Some(parts) = object.as_array().filter(|parts| parts.len() == count) else {
                reader.say(object.at, fault);
                return Ok(None);
            };
            let mut numbers = Vec::with_capacity(count);
            for part in parts {
                let Some(number) = reader.value(part, Some(fault))? else {
                    return Ok(None);
                };
                if number.as_number().is_none() {
                    reader.say(part.at, fault);
                    return Ok(None);
                }
                numbers.push(number);
            }
            Ok(Some(numbers))
        })
    }

    /// A CIE-based space's parameters, but for an ICCBased one's.
    fn cie(
        &mut self,
        family: Family,
        parts: &'o [Object],
        at: Option<Span>,
    ) -> Result<Cie<'o>, Unreadable> {
        let none = Cie {
            dict: None,
            white_point: None,
        };
        let Some(written) = self.element(parts, 1, at) else {
            return Ok(none);
        };
        let read = self.entered(written, Some(Fault::NotADictionary), |reader, object| {
            let dict = match object.value() {
                Value::Dict(dict) => dict,
                Value::Stream(stream) => {
                    reader.say(object.at, Fault::NotADictionary);
                    &stream.dict
                }
                _ => {
                    reader.say(object.at, Fault::NotADictionary);
                    return Ok(None);
                }
            };
            let white_point = reader.white_point(dict, object.at)?;
            reader.check(|reader| reader.check_cie(family, dict));
            Ok(Some(Cie {
                dict: Some(dict),
                white_point,
            }))
        })?;
        Ok(read.unwrap_or(none))
    }

    /// A `/WhitePoint`: held where it's three numbers, told unless the first and
    /// third are above 0 and the second is 1. Each is also asked as PDFium's
    /// accessor reads it, since a space with no fault told counts as PDFium's: a
    /// number can be above 0 and not as its 32-bit float.
    fn white_point(
        &mut self,
        dict: &'o Dict,
        at: Option<Span>,
    ) -> Result<Option<[f64; 3]>, Unreadable> {
        let Some(point) = entry_of(dict, b"WhitePoint") else {
            self.say(at, Fault::WhitePoint);
            return Ok(None);
        };
        let Some(numbers) = self.numbers(point, 3, Fault::WhitePoint, true)? else {
            return Ok(None);
        };
        let [x, y, z] = numbers.as_slice() else {
            return Ok(None);
        };
        let number = |object: &Object| object.as_number().unwrap_or(0.0);
        let held = [number(x), number(y), number(z)];
        let above_zero = |object: &Object| number(object) > 0.0 && float(object) > 0.0;
        if !(above_zero(x) && number(y) == 1.0 && above_zero(z)) {
            self.say(point.at, Fault::WhitePoint);
        }
        Ok(Some(held))
    }

    /// What's checked of a CIE-based dictionary and left in it.
    fn check_cie(&mut self, family: Family, dict: &'o Dict) -> Result<(), Unreadable> {
        let black = Fault::BlackPoint;
        if let Some(point) = entry_of(dict, b"BlackPoint") {
            if let Some(numbers) = self.numbers(point, 3, black, false)? {
                if numbers.iter().any(|n| n.as_number() < Some(0.0)) {
                    self.say(point.at, black);
                }
            }
        }
        match family {
            Family::CalGray => {
                if let Some(gamma) = entry_of(dict, b"Gamma") {
                    let number = self.value(gamma, None)?;
                    let holds =
                        |number: &Object| number.is_null() || number.as_number() > Some(0.0);
                    if !number.is_none_or(holds) {
                        self.say(gamma.at, Fault::Gamma);
                    }
                }
            }
            Family::CalRGB => {
                if let Some(gamma) = entry_of(dict, b"Gamma") {
                    self.numbers(gamma, 3, Fault::Gamma, false)?;
                }
                if let Some(matrix) = entry_of(dict, b"Matrix") {
                    self.numbers(matrix, 9, Fault::Matrix, false)?;
                }
            }
            Family::Lab => {
                if let Some(range) = entry_of(dict, b"Range") {
                    if let Some(numbers) = self.numbers(range, 4, Fault::LabRange, false)? {
                        let number = |at: usize| numbers[at].as_number();
                        if number(0) > number(1) || number(2) > number(3) {
                            self.say(range.at, Fault::LabRange);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// An ICCBased space's parameters, its profile not opened.
    fn icc(
        &mut self,
        parts: &'o [Object],
        at: Option<Span>,
        depth: usize,
    ) -> Result<Icc<'o>, Unreadable> {
        let none = Icc {
            id: None,
            stream: None,
            n: None,
            alternate: None,
        };
        let Some(written) = self.element(parts, 1, at) else {
            return Ok(none);
        };
        let read = self.entered(written, Some(Fault::IccNotStream), |reader, object| {
            let Some(stream) = object.as_stream() else {
                reader.say(object.at, Fault::IccNotStream);
                return Ok(None);
            };
            let n = reader.icc_components(&stream.dict, object.at)?;
            let mut alternate = None;
            if let Some(space) = entry_of(&stream.dict, b"Alternate") {
                alternate = reader.space(space, depth + 1)?;
                if let Some(alternate) = &alternate {
                    let pattern = alternate.family() == Family::Pattern;
                    if pattern || (n.is_some() && alternate.components() != n) {
                        reader.say(space.at, Fault::IccAlternate);
                    }
                }
            }
            if let Some(range) = entry_of(&stream.dict, b"Range") {
                reader.check(|reader| {
                    // With no count there's nothing to hold it to.
                    if let Some(n) = n {
                        let pairs = 2 * n as usize;
                        reader.numbers(range, pairs, Fault::IccRange, false)?;
                    }
                    Ok(())
                });
            }
            Ok(Some(Icc {
                id: written.as_reference().and(reader.path.last().copied()),
                stream: Some(stream),
                n,
                alternate: alternate.map(Box::new),
            }))
        })?;
        Ok(read.unwrap_or(none))
    }

    /// A profile stream's `/N`, or what's wrong with it told.
    fn icc_components(
        &mut self,
        dict: &'o Dict,
        at: Option<Span>,
    ) -> Result<Option<u32>, Unreadable> {
        let Some(n) = entry_of(dict, b"N") else {
            self.say(at, Fault::IccComponents);
            return Ok(None);
        };
        self.entered(
            n,
            Some(Fault::IccComponents),
            |reader, number| match number.as_integer() {
                Some(1) => Ok(Some(1)),
                Some(3) => Ok(Some(3)),
                Some(4) => Ok(Some(4)),
                _ => {
                    reader.say(number.at, Fault::IccComponents);
                    Ok(None)
                }
            },
        )
    }

    /// An Indexed space's parameters.
    fn indexed(
        &mut self,
        parts: &'o [Object],
        at: Option<Span>,
        depth: usize,
    ) -> Result<Indexed<'o>, Unreadable> {
        let base = match self.element(parts, 1, at) {
            Some(written) => {
                let base = self.space(written, depth + 1)?;
                if let Some(Family::Indexed | Family::Pattern) = base.as_ref().map(Space::family) {
                    self.say(written.at, Fault::BaseSpecial);
                }
                base
            }
            None => None,
        };
        let hival = match self.element(parts, 2, at) {
            Some(written) => self.hival(written)?,
            None => None,
        };
        let lookup = match self.element(parts, 3, at) {
            Some(written) => self.lookup(written)?,
            None => None,
        };
        if self.sink.wants() {
            self.check_lookup(parts.get(3), lookup, &base, hival);
        }
        Ok(Indexed {
            base: base.map(Box::new),
            hival,
            lookup,
        })
    }

    /// An Indexed space's high value.
    fn hival(&mut self, written: &'o Object) -> Result<Option<u8>, Unreadable> {
        self.entered(written, Some(Fault::Hival), |reader, number| {
            let hival = number.as_integer().and_then(|n| u8::try_from(n).ok());
            if hival.is_none() {
                reader.say(number.at, Fault::Hival);
            }
            Ok(hival)
        })
    }

    /// An Indexed space's table.
    fn lookup(&mut self, written: &'o Object) -> Result<Option<Lookup<'o>>, Unreadable> {
        self.entered(written, Some(Fault::LookupKind), |reader, table| {
            let lookup = match table.value() {
                Value::String(string, _) => Some(Lookup::String(string)),
                Value::Stream(stream) => Some(Lookup::Stream {
                    stream,
                    id: written.as_reference().and(reader.path.last().copied()),
                }),
                _ => None,
            };
            if lookup.is_none() {
                reader.say(table.at, Fault::LookupKind);
            }
            Ok(lookup)
        })
    }

    /// Whether a table, a string whose bytes a reader gave, is as long as its base
    /// and high value say.
    fn check_lookup(
        &mut self,
        written: Option<&'o Object>,
        lookup: Option<Lookup<'o>>,
        base: &Option<Space<'o>>,
        hival: Option<u8>,
    ) {
        let Some(Lookup::String(string)) = lookup else {
            return;
        };
        let (Some(bytes), Some(hival)) = (string.as_bytes(), hival) else {
            return;
        };
        let Some(components) = base.as_ref().and_then(Space::components) else {
            return;
        };
        let long = u64::from(components) * (u64::from(hival) + 1);
        if bytes.len() as u64 != long {
            self.say(written.and_then(|written| written.at), Fault::LookupLength);
        }
    }

    /// A Pattern space's underlying space, as a name alone has none.
    fn pattern(
        &mut self,
        parts: &'o [Object],
        depth: usize,
    ) -> Result<Option<Box<Space<'o>>>, Unreadable> {
        let Some(written) = parts.get(1) else {
            return Ok(None);
        };
        let under = self.space(written, depth + 1)?;
        if under.as_ref().map(Space::family) == Some(Family::Pattern) {
            self.say(written.at, Fault::UnderlyingPattern);
        }
        Ok(under.map(Box::new))
    }

    /// A Separation space's parameters.
    fn separation(
        &mut self,
        parts: &'o [Object],
        at: Option<Span>,
        depth: usize,
    ) -> Result<Separation<'o>, Unreadable> {
        let name = match self.element(parts, 1, at) {
            Some(written) => self.colorant(written, Fault::ColorantName)?,
            None => None,
        };
        let alternate = self.alternate(parts, at, depth)?;
        let outputs = alternate.as_deref().and_then(Space::components);
        let tint = self.tint(parts, at, Some(1), outputs, depth);
        Ok(Separation {
            name,
            alternate,
            tint,
        })
    }

    /// A name, through its references, or `fault` told.
    fn colorant(
        &mut self,
        written: &'o Object,
        fault: Fault,
    ) -> Result<Option<&'o [u8]>, Unreadable> {
        self.entered(written, Some(fault), |reader, name| {
            let bytes = name.as_name();
            if bytes.is_none() {
                reader.say(name.at, fault);
            }
            Ok(bytes)
        })
    }

    /// A Separation or DeviceN space's alternate, the third element of each.
    fn alternate(
        &mut self,
        parts: &'o [Object],
        at: Option<Span>,
        depth: usize,
    ) -> Result<Option<Box<Space<'o>>>, Unreadable> {
        let Some(written) = self.element(parts, 2, at) else {
            return Ok(None);
        };
        let alternate = self.space(written, depth + 1)?;
        if alternate.as_ref().is_some_and(|a| a.family().is_special()) {
            self.say(written.at, Fault::AlternateSpecial);
        }
        Ok(alternate.map(Box::new))
    }

    /// A Separation or DeviceN space's tint transform, as written, the fourth
    /// element of each: what's checked of it takes `inputs` and gives `outputs`,
    /// where they're known, a level below its space at `depth`.
    fn tint(
        &mut self,
        parts: &'o [Object],
        at: Option<Span>,
        inputs: Option<u32>,
        outputs: Option<u32>,
        depth: usize,
    ) -> Option<&'o Object> {
        let tint = self.element(parts, 3, at);
        if let Some(tint) = tint {
            self.check(|reader| reader.check_tint(tint, inputs, outputs, depth + 1));
        }
        tint
    }

    /// Whether a tint transform loads, as PDFium loads one, and takes and gives
    /// what its space says. It's walked from `depth`, the level [`components`]
    /// walks it from, so that this tells a cap on depth where that meets it.
    ///
    /// [`components`]: crate::components
    fn check_tint(
        &mut self,
        written: &'o Object,
        inputs: Option<u32>,
        outputs: Option<u32>,
        depth: usize,
    ) -> Result<(), Unreadable> {
        let no_function = Fault::TintNotFunction;
        self.entered(written, Some(no_function), |reader, object| {
            if !matches!(object.value(), Value::Dict(_) | Value::Stream(_)) {
                reader.say(object.at, no_function);
                return Ok(None::<()>);
            }
            let id = written.as_reference().and(reader.path.last().copied());
            let hop = Hop { object, id };
            let damaged = Cell::new(None);
            let noting = Noting {
                objects: &reader.objects,
                damaged: &damaged,
            };
            let mut walk = Walk::within(noting, reader.limits, reader.checks);
            let shape = function::shape(&mut walk, Some(hop), depth)?;
            let (spent, capped) = (walk.spent(), walk.capped());
            reader.checks = spent;
            if let Some(id) = damaged.get() {
                reader.say(object.at, Fault::Damaged { id });
            }
            if capped {
                if spent < reader.limits.space_visits {
                    reader.say(object.at, Fault::TooDeep);
                } else if !std::mem::replace(&mut reader.checks_told, true) {
                    reader.say(object.at, Fault::TooManyVisits);
                }
                return Ok(None);
            }
            let holds = shape.is_some_and(|shape| {
                inputs.is_none_or(|inputs| shape.inputs == inputs)
                    && outputs.is_none_or(|outputs| shape.outputs == outputs)
            });
            if !holds {
                reader.say(object.at, Fault::TintShape);
            }
            Ok(None)
        })?;
        Ok(())
    }

    /// A DeviceN space's parameters.
    fn device_n(
        &mut self,
        parts: &'o [Object],
        at: Option<Span>,
        depth: usize,
    ) -> Result<DeviceN<'o>, Unreadable> {
        let written = self.element(parts, 1, at);
        let read = match written {
            Some(written) => self.names(written, Fault::NamesNotArray, Fault::ColorantName)?,
            None => None,
        };
        if read.as_ref().is_some_and(|read| read.elements == 0) {
            self.say(written.and_then(|written| written.at), Fault::NamesEmpty);
        }
        let Names { names, elements } = read.unwrap_or_default();
        let components = u32::try_from(elements).ok().filter(|count| *count > 0);
        let alternate = self.alternate(parts, at, depth)?;
        let outputs = alternate.as_deref().and_then(Space::components);
        let tint = self.tint(parts, at, components, outputs, depth);
        let attributes = match parts.get(4) {
            Some(written) if !written.is_null() => self.attributes(written, &names, depth)?,
            _ => None,
        };
        if self.sink.wants() {
            let nchannel = attributes
                .as_ref()
                .is_some_and(|attributes| attributes.subtype == Some(Subtype::NChannel));
            self.check_names(parts.get(1), &names, nchannel);
        }
        Ok(DeviceN {
            names,
            components,
            alternate,
            tint,
            attributes,
        })
    }

    /// The names `written` is an array of, each through its references, each by
    /// its position. Each spends a visit, by reference or in place, so that an array
    /// of many, shared, can't make a space's memory the square of its file's.
    fn names(
        &mut self,
        written: &'o Object,
        not_array: Fault,
        not_name: Fault,
    ) -> Result<Option<Names<'o>>, Unreadable> {
        self.entered(written, Some(not_array), |reader, object| {
            let Some(parts) = object.as_array() else {
                reader.say(object.at, not_array);
                return Ok(None);
            };
            let mut names = Vec::new();
            for part in parts {
                if part.as_reference().is_none() && !reader.spend(part.at) {
                    break;
                }
                names.push(reader.colorant(part, not_name)?);
            }
            let elements = parts.len();
            Ok(Some(Names { names, elements }))
        })
    }

    /// What's checked of a DeviceN space's names alone.
    fn check_names(
        &mut self,
        written: Option<&'o Object>,
        names: &[Option<&'o [u8]>],
        nchannel: bool,
    ) {
        let at = written.and_then(|written| written.at);
        let names: Vec<&'o [u8]> = names.iter().flatten().copied().collect();
        let is = |of: Colorant| names.iter().any(|name| Colorant::of(name) == of);
        if is(Colorant::All) {
            self.say(at, Fault::AllInDeviceN);
        }
        if nchannel && is(Colorant::None) {
            self.say(at, Fault::NoneInNChannel);
        }
        let mut sorted = names;
        sorted.sort_unstable();
        let repeated = sorted
            .windows(2)
            .any(|pair| pair[0] == pair[1] && Colorant::of(pair[0]) != Colorant::None);
        if repeated {
            self.say(at, Fault::ColorantRepeated);
        }
    }

    /// A DeviceN space's attributes dictionary, read for what it holds; what its
    /// `/Colorants` say is checked.
    fn attributes(
        &mut self,
        written: &'o Object,
        names: &[Option<&'o [u8]>],
        depth: usize,
    ) -> Result<Option<Attributes<'o>>, Unreadable> {
        self.entered(written, None, |reader, object| {
            if object.is_null() {
                return Ok(None);
            }
            let Some(dict) = object.as_dict() else {
                reader.say(object.at, Fault::NotADictionary);
                return Ok(None);
            };
            let subtype = reader.subtype(dict)?;
            let held = reader.dictionary(dict, b"Colorants")?;
            let (colorants, colorants_in) = held.unzip();
            let colorants_in = colorants_in.flatten();
            let mixing_hints = reader.dictionary(dict, b"MixingHints")?.unzip().0;
            let process = match entry_of(dict, b"Process") {
                Some(process) => reader.process(process, depth)?,
                None => None,
            };
            if let Some(colorants) = colorants {
                reader.check(|reader| {
                    reader.enter(colorants_in);
                    for entry in colorants.last_entries() {
                        if !reader.spend(entry.value.at) {
                            break;
                        }
                        reader.separation_entry(entry, depth + 1)?;
                    }
                    Ok(())
                });
            }
            if subtype == Some(Subtype::NChannel) && reader.sink.wants() {
                let listed = process
                    .as_ref()
                    .map_or(&[][..], |p| p.components.as_slice());
                let undefined = names.iter().flatten().any(|name| {
                    Colorant::of(name) == Colorant::Spot
                        && !listed.contains(&Some(*name))
                        && colorants.is_none_or(|colorants| !colorants.has(name))
                });
                if undefined {
                    reader.say(object.at, Fault::ColorantUndefined);
                }
            }
            Ok(Some(Attributes {
                subtype,
                colorants,
                colorants_in,
                process,
                mixing_hints,
            }))
        })
    }

    /// An attributes dictionary's `/Subtype`.
    fn subtype(&mut self, dict: &'o Dict) -> Result<Option<Subtype>, Unreadable> {
        let Some(written) = entry_of(dict, b"Subtype") else {
            return Ok(None);
        };
        self.entered(written, None, |reader, name| {
            if name.is_null() {
                return Ok(None);
            }
            let subtype = match name.as_name() {
                Some(b"DeviceN") => Some(Subtype::DeviceN),
                Some(b"NChannel") => Some(Subtype::NChannel),
                _ => None,
            };
            if subtype.is_none() {
                reader.say(name.at, Fault::AttributesSubtype);
            }
            Ok(subtype)
        })
    }

    /// The dictionary an attributes dictionary has under `key`, and the object
    /// it's written in.
    fn dictionary(
        &mut self,
        dict: &'o Dict,
        key: &[u8],
    ) -> Result<Option<(&'o Dict, Option<ObjectId>)>, Unreadable> {
        let Some(written) = entry_of(dict, key) else {
            return Ok(None);
        };
        self.entered(written, None, |reader, object| {
            if object.is_null() {
                return Ok(None);
            }
            let held = object.as_dict();
            if held.is_none() {
                reader.say(object.at, Fault::NotADictionary);
            }
            Ok(held.map(|held| (held, reader.path.last().copied())))
        })
    }

    /// Puts the object what's read next is written in on the path, unless that's
    /// where it is already.
    fn enter(&mut self, object: Option<ObjectId>) {
        if object.is_some() && self.path.last().copied() != object {
            self.path.extend(object);
        }
    }

    /// A process dictionary.
    fn process(
        &mut self,
        written: &'o Object,
        depth: usize,
    ) -> Result<Option<Process<'o>>, Unreadable> {
        self.entered(written, None, |reader, object| {
            if object.is_null() {
                return Ok(None);
            }
            let Some(dict) = object.as_dict() else {
                reader.say(object.at, Fault::NotADictionary);
                return Ok(None);
            };
            let space = match entry_of(dict, b"ColorSpace") {
                Some(written) => {
                    let space = reader.space(written, depth + 1)?;
                    if space.as_ref().is_some_and(|space| {
                        space.family().is_special() || space.family() == Family::Lab
                    }) {
                        reader.say(written.at, Fault::ProcessSpace);
                    }
                    space
                }
                None => {
                    reader.say(object.at, Fault::ProcessSpace);
                    None
                }
            };
            let mut components = Vec::new();
            match entry_of(dict, b"Components") {
                Some(written) => {
                    let fault = Fault::ProcessComponents;
                    if let Some(read) = reader.names(written, fault, fault)? {
                        let wanted = space.as_ref().and_then(Space::components);
                        if wanted.is_some_and(|n| n as usize != read.elements) {
                            reader.say(written.at, fault);
                        }
                        components = read.names;
                    }
                }
                None => reader.say(object.at, Fault::ProcessComponents),
            }
            Ok(Some(Process {
                space: space.map(Box::new),
                components,
            }))
        })
    }

    /// The space an entry of `/Colorants` says, told unless it's a Separation of
    /// the name its key is.
    fn separation_entry(
        &mut self,
        entry: &'o Entry,
        depth: usize,
    ) -> Result<Option<Space<'o>>, Unreadable> {
        let space = self.space(&entry.value, depth)?;
        let holds = match &space {
            Some(Space::Separation(separation)) => separation.name == Some(entry.key.as_slice()),
            _ => false,
        };
        if !holds {
            self.say(entry.value.at, Fault::ColorantNotSeparation);
        }
        Ok(space)
    }

    /// Each of `colorants`' entries, written in `object`, under one budget.
    pub(super) fn separations(
        &mut self,
        colorants: &'o Dict,
        object: Option<ObjectId>,
    ) -> Result<Vec<ColorantSpace<'o>>, Unreadable> {
        self.enter(object);
        let mut spaces = Vec::new();
        for entry in colorants.last_entries() {
            if !self.spend(entry.value.at) {
                break;
            }
            let space = self.separation_entry(entry, 0)?;
            spaces.push(ColorantSpace {
                name: &entry.key,
                space,
            });
        }
        Ok(spaces)
    }
}
