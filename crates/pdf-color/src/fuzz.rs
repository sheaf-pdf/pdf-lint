//! What the fuzz target checks, which unit tests check too: the promises the
//! count, a resources dictionary's names and a typed space make, on any input.
//! It's compiled only for tests and under cargo-fuzz's `--cfg fuzzing`.

use std::cell::Cell;
use std::collections::BTreeMap;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::parse::Parser;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Unreadable, Why};

use crate::count::counted;
use crate::{Family, Fault, Limits, Space, Spaces, components};

/// How many values of an input are read as objects.
const OBJECTS: usize = 4096;

/// How many of them are each counted, and each read as resources, as a run of
/// the target shouldn't take the square of its input.
const VALUES: usize = 32;

/// How many failed starts an input may have, each a byte on from the last.
const STARTS: usize = 512;

/// How many numbers of a file are read, from 1 on, whatever its `/Size` says.
const NUMBERS: u32 = 256;

/// `data` as a series of values, the `n`th of them object `n`, read by
/// pdf-objects' parser, a dictionary with a `/Length` as a stream with no data,
/// as no value's a stream by itself; a value that doesn't read is passed a byte
/// at a time.
fn values(data: &[u8]) -> BTreeMap<ObjectId, Object> {
    let mut objects = BTreeMap::new();
    let (mut at, mut failed) = (0, 0);
    let mut next = 1;
    while at < data.len() && objects.len() < OBJECTS && failed < STARTS {
        let mut parser = Parser::at(data, at);
        match parser.value(&mut ()) {
            Ok(value) => {
                let stream = (value.as_dict())
                    .filter(|dict| dict.has(b"Length"))
                    .map(|dict| Object::stream(dict.clone()));
                objects.insert(ObjectId::new(next, 0), stream.unwrap_or(value));
                next += 1;
                at = parser.offset();
            }
            Err(_) => {
                failed += 1;
                at += 1;
            }
        }
    }
    objects
}

/// A resolver that counts what it's asked.
#[derive(Clone, Copy)]
struct Counting<'c, R> {
    objects: R,
    asked: &'c Cell<usize>,
}

impl<'o, R: Resolve<'o>> Resolve<'o> for Counting<'_, R> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.asked.set(self.asked.get() + 1);
        self.objects.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        self.asked.set(self.asked.get() + 1);
        self.objects.try_resolve(id)
    }
}

/// A resolver that fails every lookup for `why`, and one that has nothing.
#[derive(Clone, Copy)]
struct Failing {
    why: Option<Why>,
}

impl<'o> Resolve<'o> for Failing {
    fn resolve(&self, _: ObjectId) -> Option<&'o Object> {
        None
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        match self.why {
            Some(why) => Err(Unreadable::new(id, why, "made to fail")),
            None => Ok(None),
        }
    }
}

/// Limits of these two caps.
fn limits(space_depth: usize, space_visits: usize) -> Limits {
    let mut limits = Limits::DEFAULT;
    limits.space_depth = space_depth;
    limits.space_visits = space_visits;
    limits
}

/// `value`, counted over `objects`, a closure over `data`'s values so nothing to
/// fail, and held to its promises: a count from 1 to the most it could be, within
/// every cap; none only for a reference to nothing; no looks past a cap; what
/// a cap met says nothing of, and what no cap met is the same under larger and
/// smaller caps; a failing resolver's reasons kept.
fn a_value<'o, R: Resolve<'o> + Copy>(objects: R, value: &'o Object, most: u32) {
    let asked = Cell::new(0);
    let counting = Counting {
        objects,
        asked: &asked,
    };
    let read = counted(counting, value, &Limits::DEFAULT).expect("nothing fails");
    let (count, capped) = (read.count, read.capped);
    assert!(asked.get() <= Limits::DEFAULT.space_visits);
    match count {
        Some(count) => assert!((1..=most.max(17)).contains(&count), "{count} of {value:?}"),
        None => {
            let id = value
                .as_reference()
                .expect("only a reference leads to nothing");
            assert_eq!(objects.resolve(id), None, "{value:?}");
        }
    }
    assert_eq!(components(objects, value, &Limits::DEFAULT), Ok(count));
    if !capped {
        let larger = limits(256, Limits::DEFAULT.space_visits * 4);
        assert_eq!(components(objects, value, &larger), Ok(count), "{value:?}");
    }

    for (depth, visits) in [(0, 0), (1, 1), (2, 8), (3, 64)] {
        let asked = Cell::new(0);
        let counting = Counting {
            objects,
            asked: &asked,
        };
        let small = limits(depth, visits);
        let read = counted(counting, value, &small).expect("nothing fails");
        let (under, met) = (read.count, read.capped);
        assert!(
            asked.get() <= visits,
            "{} of {visits} on {value:?}",
            asked.get()
        );
        if !met {
            assert_eq!(under, count, "{value:?} under {small:?}");
        }
    }

    // What a resolver can't read: `Why::Damaged` as nothing there, as a
    // resolver with nothing has it; any other, the `Err`, if it's asked at all.
    let nothing = components(Failing { why: None }, value, &Limits::DEFAULT);
    let damaged = Failing {
        why: Some(Why::Damaged),
    };
    assert_eq!(
        components(damaged, value, &Limits::DEFAULT),
        nothing,
        "{value:?}"
    );
    // Each other kind a resolver gives: `Why::Filter` and `Why::Detached` only a
    // stream's data is.
    for why in [Why::Capped, Why::Locked, Why::Source] {
        let failing = Failing { why: Some(why) };
        let asked = Cell::new(0);
        let counting = Counting {
            objects: failing,
            asked: &asked,
        };
        let read = components(counting, value, &Limits::DEFAULT);
        match (read, asked.get()) {
            (Err(error), 0) => panic!("an error with nothing asked: {error:?}"),
            (Err(error), _) => assert_eq!(error.why, why),
            (Ok(_), 0) => {}
            (Ok(_), _) => panic!("a failed read let by: {value:?}"),
        }
    }
}

/// A resolver over `objects` that fails, for `why`, each number `every` divides.
#[derive(Clone, Copy)]
struct FailingSome<R> {
    objects: R,
    every: u32,
    why: Why,
}

impl<'o, R: Resolve<'o>> Resolve<'o> for FailingSome<R> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.try_resolve(id).ok().flatten()
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        if id.number.is_multiple_of(self.every) {
            return Err(Unreadable::new(id, self.why, "made to fail"));
        }
        self.objects.try_resolve(id)
    }
}

/// A sink that wants nothing, and that's told nothing.
struct Unwanted;

impl Sink<Fault> for Unwanted {
    fn fault(&mut self, _: Place, fault: Fault) {
        panic!("told {fault:?}, wanting none");
    }

    fn wants(&self) -> bool {
        false
    }
}

/// `value` as a typed space, over `objects`, and held to its promises: the same
/// whatever the sink, what `()` looks up within its cap and a sink's within twice
/// that; a space or none with a fault told; and, the one between the two readings,
/// a space with no fault told counts by [`Space::components`] as [`components`]
/// does, where no cap ended a branch of that, but for a Pattern space over more
/// than 16. And failing reads, of some objects or all, alike under every sink.
fn a_space<'o, R: Resolve<'o> + Copy>(objects: R, value: &'o Object) {
    let defaults = Limits::DEFAULT;
    let asked = Cell::new(0);
    let counting = Counting {
        objects,
        asked: &asked,
    };
    let plain = Space::read(counting, value, &defaults, &mut ()).expect("nothing fails");
    assert!(asked.get() <= defaults.space_visits, "{value:?}");
    asked.set(0);
    let mut faults = Vec::new();
    let told = Space::read(counting, value, &defaults, &mut faults).expect("nothing fails");
    assert!(asked.get() <= 2 * defaults.space_visits, "{value:?}");
    assert_eq!(told, plain, "{value:?}");
    let unwanted = Space::read(objects, value, &defaults, &mut Unwanted);
    assert_eq!(unwanted.as_ref(), Ok(&plain), "{value:?}");
    let by_dyn: &mut dyn Sink<Fault> = &mut ();
    assert_eq!(
        Space::read(objects, value, &defaults, by_dyn),
        Ok(plain.clone())
    );

    if faults.is_empty() {
        let space = plain.as_ref().expect("no space without a fault");
        let read = counted(objects, value, &defaults).expect("nothing fails");
        let (count, capped) = (read.count, read.capped);
        let over = |under: &Space<'o>| under.components().is_some_and(|n| n > 16);
        if !capped && !matches!(space, Space::Pattern(Some(under)) if over(under)) {
            assert!(space.components().is_some(), "{value:?}");
            assert_eq!(space.components(), count, "the promise: {value:?}");
        }
    }
    if let Some(space) = &plain {
        let family = space.family();
        assert!(family.is_device() ^ family.is_cie() ^ family.is_special());
        match space {
            Space::Separation(_) => assert!(space.colorants().count() <= 1),
            Space::DeviceN(device_n) => {
                let named = device_n.names.iter().flatten().count();
                assert_eq!(space.colorants().count(), named);
                let most = device_n.components.map_or(0, |n| n as usize);
                assert!(device_n.names.len() <= most, "{value:?}");
            }
            _ => assert_eq!(space.colorants().count(), 0),
        }
    }

    for (depth, visits) in [(0, 0), (1, 1), (2, 8), (3, 64)] {
        let small = limits(depth, visits);
        asked.set(0);
        let under = Space::read(counting, value, &small, &mut ()).expect("nothing fails");
        assert!(
            asked.get() <= visits,
            "{} of {visits} on {value:?}",
            asked.get()
        );
        let mut told = Vec::new();
        asked.set(0);
        let by_sink = Space::read(counting, value, &small, &mut told).expect("nothing fails");
        assert!(
            asked.get() <= 2 * visits,
            "{} of 2 x {visits} on {value:?}",
            asked.get()
        );
        assert_eq!(by_sink, under, "{value:?} under {small:?}");
    }

    // Each kind a resolver gives: `Why::Filter` and `Why::Detached` only a stream's
    // data is.
    for why in [Why::Damaged, Why::Capped, Why::Locked, Why::Source] {
        for every in [1, 2, 3] {
            let failing = FailingSome {
                objects,
                every,
                why,
            };
            let plain = Space::read(failing, value, &defaults, &mut ());
            let mut faults = Vec::new();
            let told = Space::read(failing, value, &defaults, &mut faults);
            assert_eq!(told, plain, "{value:?} by {why:?}");
            let unwanted = Space::read(failing, value, &defaults, &mut Unwanted);
            assert_eq!(unwanted, plain, "{value:?} by {why:?}");
            if let Err(error) = plain {
                assert!(why != Why::Damaged, "{error:?}");
                assert_eq!(error.why, why);
            }
        }
    }
}

/// What a resources dictionary names, each with what it counts as, or why a
/// lookup failed; or why looking for the dictionary did.
pub type Names<'o> = Result<Option<Vec<(&'o [u8], Result<Option<u32>, Why>)>>, Why>;

/// `resources` read for what it names: the same by [`Spaces::iter`] as by
/// [`Spaces::get`], each once and none with an empty key, and at most four
/// objects looked up to find them; and the first of them counted by
/// [`Spaces::components`], the same names in the same order, each with what
/// [`counted`] gives for its value, or a failure for the same reason, its
/// upper bound [`Spaces::iter`]'s.
fn a_resources<'o, R: Resolve<'o> + Copy>(objects: R, resources: Option<&'o Object>) -> Names<'o> {
    let asked = Cell::new(0);
    let counting = Counting {
        objects,
        asked: &asked,
    };
    let spaces = Spaces::read(counting, resources).map_err(|unreadable| unreadable.why);
    assert!(asked.get() <= 4);
    let Some(spaces) = spaces? else {
        return Ok(None);
    };
    for (at, named) in spaces.iter().enumerate() {
        assert_eq!(spaces.get(named.key), Some(named), "{:?}", named.key);
        assert!(!named.key.is_empty());
        let first = spaces.iter().position(|other| other.key == named.key);
        assert_eq!(first, Some(at));
    }
    assert_eq!(spaces.get(b""), None);

    let each = spaces.components(objects, &Limits::DEFAULT);
    assert_eq!(each.size_hint().1, spaces.iter().size_hint().1);
    let mut names = spaces.iter().take(VALUES);
    let mut counts = Vec::new();
    for (named, read) in each.take(VALUES) {
        assert_eq!(Some(named), names.next(), "{:?}", named.key);
        let read = read.map_err(|unreadable| unreadable.why);
        let alone = counted(objects, named.value, &Limits::DEFAULT);
        let alone = alone.map_err(|unreadable| unreadable.why);
        assert_eq!(read, alone, "{:?}", named.key);
        counts.push((named.key, read.map(|read| read.count)));
    }
    assert_eq!(names.next(), None, "a name not counted");
    Ok(Some(counts))
}

/// The fuzz target's body for what's held in memory: `data` as objects
/// ([`values`]), the first of them each counted, each read as a typed space, and
/// each read as resources, its names counted as they're given.
pub fn counts(data: &[u8]) {
    let objects = values(data);
    let resolve = |id: ObjectId| objects.get(&id);
    let longest = objects.values().map(longest_array).max().unwrap_or(0);
    let most = u32::try_from(longest).unwrap_or(u32::MAX);
    for value in objects.values().take(VALUES) {
        a_value(resolve, value, most);
        a_space(resolve, value);
        a_resources(resolve, Some(value)).expect("nothing fails");
    }
    assert_eq!(a_resources(resolve, None), Ok(None));
}

/// The longest array in `value`.
fn longest_array(value: &Object) -> usize {
    match value.value() {
        pdf_objects::Value::Array(parts) => {
            let inner = parts.iter().map(longest_array).max().unwrap_or(0);
            inner.max(parts.len())
        }
        pdf_objects::Value::Dict(dict) => dict
            .iter()
            .map(|(_, v)| longest_array(v))
            .max()
            .unwrap_or(0),
        _ => 0,
    }
}

/// A typed space's family and count, and what's told of it. A fault's place isn't
/// here: it's what a reader has and a document needn't.
#[derive(Debug, PartialEq)]
pub struct Typed {
    /// Its family and count, if it's a space.
    pub space: Option<(Family, Option<u32>)>,
    /// What's told of it.
    pub faults: Vec<Fault>,
}

/// What a file's objects count as: for each number, its object, if it reads, as
/// a value and as resources, and what it holds under `/Resources`.
#[derive(Debug, PartialEq)]
pub struct Counted<'o> {
    /// The object's number.
    pub number: u32,
    /// What it counts as a value, or why a lookup of it failed.
    pub count: Result<Option<u32>, Why>,
    /// What it says as a typed space, or why a lookup of it failed.
    pub space: Result<Typed, Why>,
    /// What it names, read as resources.
    pub spaces: Names<'o>,
    /// What it names under its `/Resources`.
    pub named: Names<'o>,
}

/// `object` as a typed space, as [`Counted::space`] has it.
fn typed<'o, R: Resolve<'o> + Copy>(objects: R, object: &'o Object) -> Result<Typed, Why> {
    let mut faults = Vec::new();
    let space = Space::read(objects, object, &Limits::DEFAULT, &mut faults);
    let space = space.map_err(|unreadable| unreadable.why)?;
    Ok(Typed {
        space: space.map(|space| (space.family(), space.components())),
        faults: faults.into_iter().map(|(_, fault)| fault).collect(),
    })
}

/// Every object of a file numbered up to its `trailer`'s `/Size`, to [`NUMBERS`],
/// counted and typed, and its resources read: what each gave, for a caller to set
/// beside the same file through another resolver.
pub fn over<'o, R: Resolve<'o> + Copy>(objects: R, trailer: &'o Dict) -> Vec<Counted<'o>> {
    let size = trailer
        .get(b"Size")
        .and_then(Object::as_integer)
        .unwrap_or(0);
    let top = u32::try_from(size).unwrap_or(0).min(NUMBERS);
    let mut counted = Vec::new();
    for number in 1..top {
        match objects.try_resolve(ObjectId::new(number, 0)) {
            Ok(Some(object)) => {
                let holds = object.as_dict().and_then(|dict| dict.get(b"Resources"));
                counted.push(Counted {
                    number,
                    count: components(objects, object, &Limits::DEFAULT).map_err(|e| e.why),
                    space: typed(objects, object),
                    spaces: a_resources(objects, Some(object)),
                    named: a_resources(objects, holds),
                });
            }
            Ok(None) => {}
            Err(unreadable) => counted.push(Counted {
                number,
                count: Err(unreadable.why),
                space: Err(unreadable.why),
                spaces: Ok(None),
                named: Ok(None),
            }),
        }
    }
    counted
}

/// Objects the fuzzer begins from, and the tests run on: a series of values, the
/// `n`th of them object `n`, each family well-formed and otherwise, as a file too
/// where its first is a trailer.
pub fn fixtures() -> Vec<Vec<u8>> {
    let series = [
        // Every family, as a resources dictionary names them.
        "<< /Root 2 0 R /Size 20 >> \
         << /Pages 3 0 R >> \
         << /Kids [4 0 R] /Count 1 >> \
         << /Type /Page /Parent 3 0 R /Resources 5 0 R >> \
         << /ColorSpace 6 0 R >> \
         << /A /DeviceGray /B [/CalRGB << /WhitePoint [0.9 1 1.1] >>] /C [/Lab << /WhitePoint [1 1 1] >>] \
            /D [/ICCBased 7 0 R] /E [/Indexed /DeviceRGB 1 <000000FFFFFF>] \
            /F [/Separation /Spot /DeviceCMYK 8 0 R] /G [/DeviceN [/A /B] /DeviceGray 9 0 R] \
            /H [/Pattern /DeviceRGB] /I 10 0 R /J [[/G]] /K [/I /RGB 1 (ab)] >> \
         << /Length 0 /N 3 /Alternate /DeviceRGB >> \
         << /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0 0] /C1 [1 1 1 1] >> \
         << /FunctionType 3 /Domain [0 1] /Functions [11 0 R 12 0 R] /Bounds [0.5] /Encode [0 1 0 1] >> \
         /DeviceCMYK \
         << /FunctionType 2 /Domain [0 1] /N 1 >> \
         << /Length 0 /FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8 >>",
        // Loops: of references, of arrays of one, of bases, of subfunctions, of
        // resources.
        "1 0 R \
         [2 0 R] \
         [/Pattern 3 0 R] \
         [/Indexed 5 0 R 1 <00>] \
         [/Separation /A 4 0 R] \
         [/DeviceN [/A] /G 7 0 R] \
         << /FunctionType 3 /Domain [0 1] /Functions [7 0 R] /Bounds [] /Encode [0 1] >> \
         9 0 R \
         8 0 R \
         << /ColorSpace 10 0 R >>",
        // Deep: a chain of arrays of one, and of stitching functions.
        "2 0 R 3 0 R 4 0 R 5 0 R [6 0 R] [7 0 R] [8 0 R] [9 0 R] [[[[[[[[/G]]]]]]]] \
         [/DeviceN [/A] /G 11 0 R] \
         << /FunctionType 3 /Domain [0 1] /Functions [12 0 R 12 0 R] /Bounds [0] /Encode [0 1 0 1] >> \
         << /FunctionType 3 /Domain [0 1] /Functions [13 0 R 13 0 R] /Bounds [0] /Encode [0 1 0 1] >> \
         << /FunctionType 3 /Domain [0 1] /Functions [14 0 R 14 0 R] /Bounds [0] /Encode [0 1 0 1] >> \
         << /FunctionType 2 /Domain [0 1] /N 1 >>",
        // What a typed space reads and checks: each parameter by reference, an
        // NChannel space's attributes, and each of them at fault.
        "<< /Root 2 0 R /Size 24 >> \
         << /Pages 3 0 R >> \
         << /Kids [4 0 R] /Count 1 >> \
         << /Type /Page /Parent 3 0 R /Resources << /ColorSpace 5 0 R >> >> \
         << /A 6 0 R /B [/DeviceN 7 0 R 8 0 R 9 0 R 10 0 R] /C [/CalRGB 11 0 R] /D [/Lab 12 0 R] \
            /E [/Indexed /CalGray 256 (short)] /F [/ICCBased 13 0 R] /G [/DeviceN [/All /A /A] 14 0 R 9 0 R << /Subtype /NChannel >>] \
            /H [/Separation /Cyan /Pattern 15 0 R] /I [/DeviceN [/None] /DeviceCMYK 9 0 R << /Subtype /NChannel /Colorants << /X /DeviceGray >> >>] \
            /J [/Pattern /Pattern] /K /RGB >> \
         [/Separation 16 0 R /DeviceCMYK 15 0 R] \
         [16 0 R /Black /None] \
         /DeviceGray \
         << /Length 0 /FunctionType 4 /Domain [0 1 0 1 0 1] /Range [0 1] >> \
         << /Subtype /NChannel /Colorants 17 0 R /Process 18 0 R /MixingHints << /Solidities << /Default 1 >> >> >> \
         << /WhitePoint [19 0 R 1 1] /BlackPoint [0 0 -1] /Gamma [1 1] /Matrix [1 0 0] >> \
         << /WhitePoint [1 1 1] /Range [1 0 0 1] >> \
         << /Length 0 /N 3 /Alternate [/ICCBased 13 0 R] /Range [0 1] >> \
         [/DeviceGray 1] \
         << /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0 0] /C1 [1 1 1 1] >> \
         /Spot \
         << /Spot [/Separation /Spot /DeviceGray 20 0 R] /Other [/Separation /Wrong /DeviceGray 20 0 R] >> \
         << /ColorSpace /DeviceCMYK /Components [/Cyan /Magenta /Yellow 21 0 R] >> \
         22 0 R \
         << /FunctionType 2 /Domain [0 1] /N 1 >> \
         /Black \
         0.9",
        // What PDFium's accessors read, a shape each.
        "<< /Length 0 /N 4.7 >> [/ICCBased 1 0 R] << /Length 0 /N true >> [/ICCBased 3 0 R] \
         [/CalGray << /WhitePoint [1 0.99999999 1] >>] [/CalGrayX << /WhitePoint [1 1 1] >>] \
         [/CalGray 7] [(Pattern) /RGB] [/DeviceCMYK [/A] /G << /FunctionType 2.5 /Domain [0 1 0] /N 1 >>] \
         << /Length 0 /Domain [0 1] /Range [0 1] /Size [536870912] /BitsPerSample 32 >> \
         [/DeviceN [/A] /G 10 0 R] [/Pattern [/DeviceN [/A /B /C /D /E /F /G /H /I /J /K /L /M /N /O /P /Q] /G 12 0 R]] \
         << /FunctionType 2 /Domain [0 1] /N 1 >>",
    ];
    series.iter().map(|text| text.as_bytes().to_vec()).collect()
}

#[cfg(test)]
mod tests {
    use pdf_fixtures::fixture::{self, Body, Xref};
    use pdf_objects::dict;
    use pdf_structure::bytes::{Options, Reading};

    use super::*;

    /// A mutation of `seed`, from `random`, as a fuzzer might make it.
    fn mutated(seed: &[u8], random: &mut impl FnMut() -> usize) -> Vec<u8> {
        const PIECES: [&[u8]; 28] = [
            b"[",
            b"]",
            b"<<",
            b">>",
            b"/ColorSpace",
            b"/Resources",
            b"/DeviceN",
            b"/Separation",
            b"/Indexed",
            b"/Pattern",
            b"/ICCBased",
            b"/CalGray",
            b"/Lab",
            b"/WhitePoint",
            b"/FunctionType",
            b"/Functions",
            b"/Domain",
            b"/Range",
            b"/Size",
            b"/N",
            b"1 0 R",
            b"2 0 R",
            b"3 0 R",
            b"null",
            b"true",
            b"(a)",
            b"7",
            b"1.5",
        ];
        let mut data = seed.to_vec();
        for _ in 0..1 + random() % 4 {
            let at = random() % (data.len() + 1);
            let end = match random() % 3 {
                0 => at,
                _ => data.len().min(at + 1 + random() % 8),
            };
            let piece: &[u8] = match random() % 3 {
                1 => &[],
                _ => PIECES[random() % PIECES.len()],
            };
            data.splice(at..end, piece.iter().copied());
        }
        data
    }

    fn generator(mut state: u64) -> impl FnMut() -> usize {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize
        }
    }

    #[test]
    fn mutated_seeds_keep_every_promise() {
        let mut random = generator(0x5EED);
        for seed in fixtures() {
            counts(&seed);
            for _ in 0..200 {
                counts(&mutated(&seed, &mut random));
            }
        }
    }

    #[test]
    fn random_bytes_keep_every_promise() {
        let mut random = generator(0xB17E5);
        for _ in 0..2_000 {
            let data: Vec<u8> = (0..random() % 64).map(|_| random() as u8).collect();
            counts(&data);
        }
    }

    /// A seed's objects, the first its trailer, as a file; a seed that's no
    /// such thing as it is.
    fn file_of(seed: &[u8]) -> Vec<u8> {
        let objects = values(seed);
        let Some(trailer) = objects.get(&ObjectId::new(1, 0)).and_then(Object::as_dict) else {
            return seed.to_vec();
        };
        let bodies: Vec<(u32, Body)> = (objects.iter())
            .skip(1)
            .map(|(id, object)| {
                let body = match object.as_stream() {
                    Some(stream) => Body::stream(stream.dict.clone(), b""),
                    None => Body::Object(object.clone()),
                };
                (id.number, body)
            })
            .collect();
        let root = trailer.get(b"Root").cloned().unwrap_or_else(Object::null);
        fixture::file("1.7", &bodies, dict! { "Root" => root }, Xref::Table)
    }

    /// What the target does with `data` as a file, under the default options
    /// where it has caps of its own: [`over`] through a reading and through the
    /// document it makes, the same; how many names it counted.
    fn a_file(data: &[u8]) -> usize {
        let Ok(reading) = Reading::open(data, &Options::DEFAULT) else {
            return 0;
        };
        let read = over(&reading, reading.trailer().dict());
        let document = reading
            .to_document()
            .expect("bytes in memory make a document");
        assert_eq!(over(&document, document.trailer.dict()), read);
        let named = read
            .iter()
            .filter_map(|counted| counted.named.as_ref().ok()?.as_ref());
        named.map(Vec::len).sum()
    }

    #[test]
    fn files_read_alike_through_a_reading_and_its_document() {
        let mut named = 0;
        let mut random = generator(0xF11E5);
        for seed in fixtures() {
            let file = file_of(&seed);
            named += a_file(&file);
            for _ in 0..50 {
                a_file(&mutated(&file, &mut random));
            }
        }
        assert!(named > 10, "the seeds' files name spaces: {named}");
    }

    /// `PDF_COLOR_FUZZ_SEEDS` names a folder: writes the seeds there, each also
    /// as a file, for the fuzzer to begin from (`just fuzz color`).
    #[test]
    #[ignore = "needs PDF_COLOR_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_COLOR_FUZZ_SEEDS").expect("PDF_COLOR_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in fixtures().into_iter().enumerate() {
            std::fs::write(dir.join(format!("file-{i}")), file_of(&seed)).unwrap();
            std::fs::write(dir.join(format!("objects-{i}")), seed).unwrap();
        }
    }
}
