//! pdf-color timed on inputs made here, so that a change to its count, its
//! typed reading or its reading of a resources dictionary can be judged
//! before and after it, by `cargo bench -p pdf-color`:
//!
//! - `count`: `counted` of a space of each kind a resources dictionary or
//!   an inline image holds: DeviceGray by name (`gray`), DeviceRGB in an
//!   array of one (`array-of-one`), CalRGB with its white point, gamma and
//!   matrix (`cal-rgb`), ICCBased by a profile's stream (`icc`), Indexed
//!   over DeviceRGB with its table (`indexed`), Separation of a spot
//!   colorant with an exponential tint transform (`separation`), an
//!   NChannel DeviceN space with its colorants, its process dictionary and
//!   a PostScript tint transform (`device-n`), Pattern over DeviceRGB
//!   (`pattern`), and DeviceN whose tint transform stitches eight
//!   functions, each by reference (`stitched`).
//! - `read`: `Space::read` of the same nine, with `()`, which looks for no
//!   fault (`none`), and with a `Vec` that keeps them, for which each is
//!   checked (`faults`).
//! - `resources`: a `/ColorSpace` dictionary of 8, 64 and 1,024 names, the
//!   nine in turn, found by `Spaces::read` and each name counted by
//!   `counted`, kept as a reader of content keeps them (pdf-content-streams'
//!   `ResCx::read`): each name's count, and the names a cap was met for.
//! - `colorants`: the 32 colorants of a DeviceN space, each by
//!   `Colorant::of`, tallied.
//!
//! Throughput is in spaces, but for `resources`, in names, and
//! `colorants`, in colorants. Each input is held to what it gives before
//! it's timed: a count to PDFium's for that space, with no cap met; a
//! typed space to its family, its components and how many faults it has,
//! none; a dictionary to each name's count, in order; the colorants to
//! their tally. The objects a space refers to are read by pdf-objects'
//! parser from text written here into a map, which is the resolver;
//! nothing is read from disk, and the inputs are the same on every run.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdf_color::{Colorant, Family, Fault, Limits, Space, Spaces, counted};
use pdf_objects::fault::Place;
use pdf_objects::parse::Parser;
use pdf_objects::{Object, ObjectId, Resolve, Unreadable};

/// The objects the spaces refer to, by number, as a writer lays them out:
/// an ICC profile's stream (5), a spot colorant's exponential tint
/// transform into DeviceCMYK (6), a PostScript tint transform of three
/// inputs into DeviceCMYK (7), and a stitching function (8) of eight
/// exponential functions into DeviceRGB (9 to 16). A dictionary with
/// `/Length` is a stream's; no stream's data is read, so none is placed.
const OBJECTS: &[(u32, &str)] = &[
    (
        5,
        "<< /N 3 /Alternate /DeviceRGB /Range [0 1 0 1 0 1] /Length 3144 >>",
    ),
    (
        6,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0.53 1 0] /N 1 >>",
    ),
    (
        7,
        "<< /FunctionType 4 /Domain [0 1 0 1 0 1] /Range [0 1 0 1 0 1 0 1] /Length 62 >>",
    ),
    (
        8,
        "<< /FunctionType 3 /Domain [0 1] \
        /Functions [9 0 R 10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R 16 0 R] \
        /Bounds [0.125 0.25 0.375 0.5 0.625 0.75 0.875] \
        /Encode [0 1 0 1 0 1 0 1 0 1 0 1 0 1 0 1] >>",
    ),
    (
        9,
        "<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0.9 0.95 1] /N 1 >>",
    ),
    (
        10,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0.9 0.95 1] /C1 [0.75 0.85 1] /N 1 >>",
    ),
    (
        11,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0.75 0.85 1] /C1 [0.6 0.75 0.95] /N 1 >>",
    ),
    (
        12,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0.6 0.75 0.95] /C1 [0.45 0.6 0.9] /N 1 >>",
    ),
    (
        13,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0.45 0.6 0.9] /C1 [0.3 0.45 0.8] /N 1 >>",
    ),
    (
        14,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0.3 0.45 0.8] /C1 [0.2 0.3 0.65] /N 1 >>",
    ),
    (
        15,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0.2 0.3 0.65] /C1 [0.1 0.15 0.5] /N 1 >>",
    ),
    (
        16,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0.1 0.15 0.5] /C1 [0 0 0.3] /N 2.2 >>",
    ),
];

/// A space of each kind, by the name its benchmarks carry, as written, and
/// what it gives: PDFium's count, and the typed space's family and
/// components, with no fault.
struct Case {
    name: &'static str,
    text: &'static str,
    count: u32,
    family: Family,
    components: u32,
}

/// The spaces timed, each written as a resources dictionary holds it.
const SPACES: &[Case] = &[
    Case {
        name: "gray",
        text: "/DeviceGray",
        count: 1,
        family: Family::DeviceGray,
        components: 1,
    },
    Case {
        name: "array-of-one",
        text: "[/DeviceRGB]",
        count: 3,
        family: Family::DeviceRGB,
        components: 3,
    },
    Case {
        name: "cal-rgb",
        text: "[/CalRGB << /WhitePoint [0.9505 1 1.089] /Gamma [2.2 2.2 2.2] \
            /Matrix [0.4124 0.2126 0.0193 0.3576 0.7152 0.1192 0.1805 0.0722 0.9505] >>]",
        count: 3,
        family: Family::CalRGB,
        components: 3,
    },
    Case {
        name: "icc",
        text: "[/ICCBased 5 0 R]",
        count: 3,
        family: Family::ICCBased,
        components: 3,
    },
    Case {
        name: "indexed",
        text: "[/Indexed /DeviceRGB 3 <FFFFFF FF8000 0080FF 000000>]",
        count: 1,
        family: Family::Indexed,
        components: 1,
    },
    Case {
        name: "separation",
        text: "[/Separation /Spot#20Orange /DeviceCMYK 6 0 R]",
        count: 1,
        family: Family::Separation,
        components: 1,
    },
    Case {
        name: "device-n",
        text: "[/DeviceN [/Cyan /Magenta /Spot#20Orange] /DeviceCMYK 7 0 R \
            << /Subtype /NChannel /Colorants << /Spot#20Orange [/Separation /Spot#20Orange /DeviceCMYK 6 0 R] >> \
            /Process << /ColorSpace /DeviceCMYK /Components [/Cyan /Magenta /Yellow /Black] >> >>]",
        count: 3,
        family: Family::DeviceN,
        components: 3,
    },
    Case {
        name: "pattern",
        text: "[/Pattern /DeviceRGB]",
        count: 4,
        family: Family::Pattern,
        components: 4,
    },
    Case {
        name: "stitched",
        text: "[/DeviceN [/Spot#20Blue] /DeviceRGB 8 0 R]",
        count: 1,
        family: Family::DeviceN,
        components: 1,
    },
];

/// `text` as one value, read by pdf-objects' parser.
fn value(text: &str) -> Object {
    let mut parser = Parser::at(text.as_bytes(), 0);
    (parser.value(&mut ())).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// `OBJECTS`, read, each by its number.
fn objects() -> BTreeMap<ObjectId, Object> {
    (OBJECTS.iter())
        .map(|&(number, text)| {
            let object = value(text);
            let stream = (object.as_dict())
                .filter(|dict| dict.has(b"Length"))
                .map(|dict| Object::stream(dict.clone()));
            (ObjectId::new(number, 0), stream.unwrap_or(object))
        })
        .collect()
}

/// `SPACES`, each read as written, in order.
fn spaces() -> Vec<Object> {
    SPACES.iter().map(|case| value(case.text)).collect()
}

fn count_group(c: &mut Criterion) {
    let (objects, spaces) = (objects(), spaces());
    let resolve = |id: ObjectId| objects.get(&id);
    let mut group = c.benchmark_group("count");
    group.throughput(Throughput::Elements(1));
    for (case, space) in SPACES.iter().zip(&spaces) {
        let name = case.name;
        let read = counted(resolve, space, &Limits::DEFAULT);
        let read = read.unwrap_or_else(|e| panic!("count/{name}: {e}"));
        let expected = (Some(case.count), false);
        assert_eq!(
            (read.count, read.capped),
            expected,
            "count/{name}: PDFium's count"
        );
        group.bench_function(name, |b| {
            b.iter(|| counted(resolve, black_box(space), &Limits::DEFAULT))
        });
    }
    group.finish();
}

/// Each departure `Space::read` tells, kept.
type Faults = Vec<(Place, Fault)>;

/// `space` read with `()` and with a `Vec`, held to `case`: its family, its
/// components and no fault.
fn held<'o>(objects: impl Resolve<'o> + Copy, space: &'o Object, case: &Case) {
    let name = case.name;
    let expected = (case.family, Some(case.components));
    let read = Space::read(objects, space, &Limits::DEFAULT, &mut ());
    let read = read.unwrap_or_else(|e| panic!("read/{name}/none: {e}"));
    let read = read.unwrap_or_else(|| panic!("read/{name}/none: a space"));
    let typed = (read.family(), read.components());
    assert_eq!(
        typed, expected,
        "read/{name}/none: its family and components"
    );
    let mut faults = Faults::new();
    let read = Space::read(objects, space, &Limits::DEFAULT, &mut faults);
    let read = read.unwrap_or_else(|e| panic!("read/{name}/faults: {e}"));
    let read = read.unwrap_or_else(|| panic!("read/{name}/faults: a space"));
    let typed = (read.family(), read.components());
    assert_eq!(
        typed, expected,
        "read/{name}/faults: its family and components"
    );
    assert_eq!(faults, [], "read/{name}/faults: none");
}

fn read_group(c: &mut Criterion) {
    let (objects, spaces) = (objects(), spaces());
    let resolve = |id: ObjectId| objects.get(&id);
    let mut group = c.benchmark_group("read");
    group.throughput(Throughput::Elements(1));
    for (case, space) in SPACES.iter().zip(&spaces) {
        let name = case.name;
        held(resolve, space, case);
        group.bench_function(format!("{name}/none"), |b| {
            b.iter(|| Space::read(resolve, black_box(space), &Limits::DEFAULT, &mut ()))
        });
        group.bench_function(format!("{name}/faults"), |b| {
            b.iter(|| {
                let mut faults = Faults::new();
                let read = Space::read(resolve, black_box(space), &Limits::DEFAULT, &mut faults);
                (read, faults)
            })
        });
    }
    group.finish();
}

/// The sizes of `/ColorSpace` dictionary timed, in names.
const NAMES: &[usize] = &[8, 64, 1024];

/// A resources dictionary whose `/ColorSpace` dictionary has `names` names,
/// `/CS0` on, each holding `SPACES`' next in turn.
fn resources(names: usize) -> Object {
    let mut text = String::from("<< /ProcSet [/PDF /ImageC] /ColorSpace <<");
    for (at, case) in (0..names).zip(SPACES.iter().cycle()) {
        text.push_str(&format!(" /CS{at} {}", case.text));
    }
    text.push_str(" >> >>");
    value(&text)
}

/// What a reader of content keeps of a resources dictionary's spaces: each
/// name's count, and the names a cap was met for.
type Kept = (Vec<(Vec<u8>, u32)>, Vec<Vec<u8>>);

/// The spaces `resources` names, each counted, kept as pdf-content-streams'
/// `ResCx::read` keeps them: room made for as many as the dictionary has
/// entries, each name's bytes copied.
fn kept<'o>(objects: impl Resolve<'o> + Copy, resources: &'o Object) -> Result<Kept, Unreadable> {
    let Some(spaces) = Spaces::read(objects, Some(resources))? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let named = spaces.iter();
    let room = named.size_hint().1.unwrap_or_default();
    let (mut counts, mut capped) = (Vec::with_capacity(room), Vec::new());
    for space in named {
        let read = counted(objects, space.value, &Limits::DEFAULT)?;
        if read.capped {
            capped.push(space.key.to_vec());
        }
        if let Some(count) = read.count {
            counts.push((space.key.to_vec(), count));
        }
    }
    Ok((counts, capped))
}

fn resources_group(c: &mut Criterion) {
    let objects = objects();
    let all: Vec<Object> = NAMES.iter().map(|&names| resources(names)).collect();
    let resolve = |id: ObjectId| objects.get(&id);
    let mut group = c.benchmark_group("resources");
    for (&names, resources) in NAMES.iter().zip(&all) {
        let (counts, capped) =
            kept(resolve, resources).unwrap_or_else(|e| panic!("resources/{names}: {e}"));
        let expected: Vec<(Vec<u8>, u32)> = (0..names)
            .zip(SPACES.iter().cycle())
            .map(|(at, case)| (format!("CS{at}").into_bytes(), case.count))
            .collect();
        assert!(
            counts == expected,
            "resources/{names}: each name's count, in order"
        );
        assert!(capped.is_empty(), "resources/{names}: no cap met");
        group.throughput(Throughput::Elements(names as u64));
        group.bench_function(names.to_string(), |b| {
            b.iter(|| kept(resolve, black_box(resources)))
        });
    }
    group.finish();
}

/// How many colorants the DeviceN space of `colorants` has.
const COLORANTS: usize = 32;

/// The number of the ink set's tint transform, after `OBJECTS`'.
const INK_TINT: u32 = 17;

/// A DeviceN space of 32 colorants, an ink set's: the four process
/// colorants, `None`, and 27 spot colorants, named as a swatch book names
/// them; and its tint transform into DeviceCMYK, a PostScript function of
/// 32 inputs.
fn ink_set() -> (Object, Object) {
    let mut names = String::from("/Cyan /Magenta /Yellow /Black /None");
    for spot in 0..COLORANTS - 5 {
        names.push_str(&format!(" /PANTONE#20{}#20C", 7401 + spot));
    }
    let space = value(&format!("[/DeviceN [{names}] /DeviceCMYK {INK_TINT} 0 R]"));
    let domain = "0 1 ".repeat(COLORANTS);
    let tint = format!(
        "<< /FunctionType 4 /Domain [{}] /Range [0 1 0 1 0 1 0 1] /Length 1210 >>",
        domain.trim_end()
    );
    let Some(tint) = value(&tint).as_dict().cloned() else {
        panic!("colorants/device-n-32: a tint transform's dictionary");
    };
    (space, Object::stream(tint))
}

/// How many of `colorants` are process colorants, `All`, `None` and spot
/// colorants, by what each name says of itself.
fn tally<'o>(colorants: impl Iterator<Item = &'o [u8]>) -> [usize; 4] {
    let mut tally = [0; 4];
    for name in colorants {
        let colorant = Colorant::of(name);
        let at = match colorant {
            _ if colorant.is_process() => 0,
            Colorant::All => 1,
            Colorant::None => 2,
            _ => 3,
        };
        tally[at] += 1;
    }
    tally
}

fn colorants_group(c: &mut Criterion) {
    let (ink_set, tint) = ink_set();
    let mut objects = objects();
    objects.insert(ObjectId::new(INK_TINT, 0), tint);
    let resolve = |id: ObjectId| objects.get(&id);
    let mut faults = Faults::new();
    let read = Space::read(resolve, &ink_set, &Limits::DEFAULT, &mut faults);
    let read = read.unwrap_or_else(|e| panic!("colorants/device-n-32: {e}"));
    let space = read.unwrap_or_else(|| panic!("colorants/device-n-32: a space"));
    let typed = (space.family(), space.components());
    let expected = (Family::DeviceN, Some(COLORANTS as u32));
    assert_eq!(
        typed, expected,
        "colorants/device-n-32: its family and components"
    );
    assert_eq!(faults, [], "colorants/device-n-32: no fault");
    let told = tally(space.colorants());
    let expected = [4, 0, 1, COLORANTS - 5];
    assert_eq!(
        told, expected,
        "colorants/device-n-32: process, All, None, spot"
    );
    let mut group = c.benchmark_group("colorants");
    group.throughput(Throughput::Elements(COLORANTS as u64));
    group.bench_function("device-n-32", |b| {
        b.iter(|| tally(black_box(&space).colorants()))
    });
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    count_group(&mut criterion);
    read_group(&mut criterion);
    resources_group(&mut criterion);
    colorants_group(&mut criterion);
    criterion.final_summary();
}
