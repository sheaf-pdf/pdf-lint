//! Color spaces, read through pdf-color (ISO 32000-2, 8.6): each the walk
//! reached in the newest revision as one of the model's color space tables,
//! in the order it reached them, read again by [`Space::read`] under
//! pdf-color's caps, from its number where it has one, so that what's told
//! of it is in its object, else as it's written. What the read tells is
//! kept ([`Kept`]), each read run to its end.
//!
//! Of what it tells, what the walk can't say is a [`Rule::ColorSpace`]
//! finding, where the walk found nothing by the model's rules
//! ([`MODEL`](super::MODEL)) in that object at that key
//! ([`Walk::find_told`]): each kind once in each object, whichever space's
//! read tells it, at its first place there, its value pdf-color's words
//! with how many times it's told there, what the walk found among them; in
//! the fault's object, else the space's, else the one the space is written
//! in, held where that is. Its key is the dictionary's key the kind is
//! about, where it's one ([`kind_key`]), an element a space lacks at that
//! element, a family's name told at an array at `0` ([`of_the_name`]), as
//! the model's `missing` and `value` are at the key, the element or the
//! name, and any other at the entry or element whose value holds its bytes
//! ([`holding`]). Three more things the walk says where the key doesn't
//! match: what an object, as a whole, is of, which a read reaches through a
//! reference, the walk checks at the key holding the reference; a space of
//! a family that may not be where it is, the model links to the tables of
//! those that may alone, so the walk finds its family's name a value the
//! table it takes it for doesn't have ([`of_a_family`]); and what an array
//! at a key is as a whole, its count and its elements, the walk finds at or
//! inside the array, whose elements the model checks each
//! ([`of_the_elements`]). Its path and table are those of the dictionary or
//! array its key is written in, in place too, where the walk first visited
//! it as a color space table or one of what a space holds ([`PARTS`]), else
//! the object's, else the space's ([`Walk::path_and_table`]). An object
//! that doesn't read is never told, nor what a read tells at the bytes it
//! tells of one at ([`Faults`]): it's the reader's, and the model's
//! `unresolved`. What stops a space being read whole that isn't the file's
//! fault (an object that takes a password or is past a cap, a space nested
//! deeper or looking up more than is read) is counted as not checked, once
//! a space for each, where the space, or what it's written in, is held;
//! what was read before it is found all the same.

use std::collections::HashSet;
use std::mem::{self, Discriminant};

use pdf_color::{Fault, Limits, Space};
use pdf_objects::fault::{Place, Sink};

use super::content::times;
use super::kept::Kept;
use super::told::{Keyed, Told, holding};
use super::{Finding, Graphic, Made, Objects, REFERENCES, Rule, Walk};
use crate::object::{Object, ObjectId, Span};
use crate::{Unreadable, Why};

/// The model's color space tables: what the walk reaches as one is a color
/// space, read again after it.
pub(super) const SPACES: [&str; 11] = [
    "CalGrayColorSpace",
    "CalRGBColorSpace",
    "LabColorSpace",
    "ICCBasedColorSpace",
    "IndexedColorSpace",
    "SeparationColorSpace",
    "DeviceNColorSpace",
    "PatternColorSpace",
    "DeviceGrayColorSpace",
    "DeviceRGBColorSpace",
    "DeviceCMYKColorSpace",
];

/// The model's tables of what a color space holds: a profile stream, a
/// CIE-based space's dictionary, a DeviceN space's attributes, process,
/// colorants and mixing hints, and a tint transform. Where the walk first
/// visits an object as one of them, or as a space, is the path and table
/// of what's found in it.
pub(super) const PARTS: [&str; 12] = [
    "ICCProfileStream",
    "CalGrayDict",
    "CalRGBDict",
    "LabDict",
    "DeviceNDict",
    "DeviceNProcess",
    "DeviceNMixingHints",
    "ColorantsDict",
    "FunctionType0",
    "FunctionType2",
    "FunctionType3",
    "FunctionType4",
];

/// What color spaces are read under: pdf-color's caps, which ISO 32000-2
/// doesn't set.
const LIMITS: Limits = Limits::DEFAULT;

/// Why a space isn't read whole, other than for the file's faults: an
/// object of it takes a password the reader wasn't given.
const LOCKED: &str = "a color space with an object that takes a password";

/// Why a space isn't read whole: an object of it is past a cap the reader
/// reads under.
const CAPPED: &str = "a color space with an object past a cap it's read under";

/// Why a space isn't read whole: the source didn't give an object's bytes.
const UNGIVEN: &str = "a color space with an object whose bytes the source didn't give";

/// Why a space isn't read whole: an object of it doesn't read, for another
/// reason a reader gives.
const UNREAD: &str = "a color space with an object that doesn't read";

/// Why a space isn't read whole: it nests deeper than pdf-color reads one
/// ([`Limits::space_depth`]).
const DEEP: &str = "a color space nested deeper than it's read";

/// Why a space isn't read whole: it looks up more than pdf-color reads of
/// one ([`Limits::space_visits`]).
const VISITS: &str = "a color space with more looked up than is read";

/// Why a space isn't read whole, given what stopped its read.
fn unread(error: &Unreadable) -> &'static str {
    match error.why {
        Why::Locked => LOCKED,
        Why::Limit => CAPPED,
        Why::Source => UNGIVEN,
        _ => UNREAD,
    }
}

/// The key a kind of fault is about, where it's about one key of a
/// dictionary, which it's told at whether or not the key is written: a
/// fault of a key absent is told at its dictionary's bytes, where the
/// model's `missing` is at the key. None for the rest, told at the bytes
/// they're of ([`holding`]).
fn kind_key(fault: &Fault) -> Option<&'static str> {
    Some(match fault {
        Fault::WhitePoint => "WhitePoint",
        Fault::BlackPoint => "BlackPoint",
        Fault::Gamma => "Gamma",
        Fault::Matrix => "Matrix",
        Fault::LabRange | Fault::IccRange => "Range",
        Fault::IccComponents => "N",
        Fault::IccAlternate => "Alternate",
        Fault::ProcessSpace => "ColorSpace",
        Fault::ProcessComponents => "Components",
        Fault::ColorantUndefined => "Colorants",
        Fault::AttributesSubtype => "Subtype",
        _ => return None,
    })
}

/// What a space's read tells, kept ([`Kept`]), and where it tells of an
/// object that doesn't read ([`Fault::Damaged`]): what it tells at those
/// bytes is of that object, as a tint transform with a subfunction that
/// doesn't read doesn't load, and is the reader's.
#[derive(Default)]
struct Faults {
    kept: Kept<Fault>,
    damaged: HashSet<(Option<ObjectId>, Span)>,
}

impl Sink<Fault> for Faults {
    fn fault(&mut self, at: Place, fault: Fault) {
        if let (Fault::Damaged { .. }, Some(bytes)) = (fault, at.at) {
            self.damaged.insert((at.object, bytes));
        }
        self.kept.fault(at, fault);
    }
}

/// Whether `fault` is of a family's name, which a space that's an array
/// writes first: told at the array's bytes, it's at its element 0, where
/// the model holds the name.
fn of_the_name(fault: Fault) -> bool {
    matches!(
        fault,
        Fault::NotASpace | Fault::UnknownFamily | Fault::Abbreviated
    )
}

/// Whether `fault` is of what an array at its key is as a whole, its
/// count and its elements, each of which the model checks: a CIE-based
/// space's `/WhitePoint`, `/BlackPoint`, `/Gamma` and `/Matrix`, a Lab
/// `/Range`, a profile's `/Range` and a process dictionary's
/// `/Components`. Told at the array or at an element, it gives way to what
/// the walk finds there, which says it ([`Told::inside`]).
fn of_the_elements(fault: Fault) -> bool {
    matches!(
        fault,
        Fault::WhitePoint
            | Fault::BlackPoint
            | Fault::Gamma
            | Fault::Matrix
            | Fault::LabRange
            | Fault::IccRange
            | Fault::ProcessComponents
    )
}

/// Whether `fault` is of what family a space that another holds is: an
/// Indexed space's base, an alternate, a Pattern space's underlying space,
/// a process dictionary's space, a profile's alternate and a `/Colorants`
/// entry. Told at an array, which the model links to the tables of the
/// families that may be there alone, it gives way to the walk's finding at
/// the array's family's name, its element 0, which says it.
fn of_a_family(fault: Fault) -> bool {
    matches!(
        fault,
        Fault::BaseSpecial
            | Fault::AlternateSpecial
            | Fault::UnderlyingPattern
            | Fault::ProcessSpace
            | Fault::IccAlternate
            | Fault::ColorantNotSeparation
    )
}

impl<'a> Walk<'a> {
    /// Reads each color space the walk reached, in the order it reached
    /// them, and finds what each read tells that the walk can't say: see
    /// the module's documentation.
    pub(super) fn spaces(&mut self) {
        let spaces = mem::take(&mut self.spaces);
        // What each read tells, in order: found once all are read, against
        // the walk's findings.
        let mut tells: Vec<Told<Discriminant<Fault>>> = Vec::new();
        for space in &spaces {
            // Given by its number, the read starts in the space's object.
            let reference;
            let value = match space.id {
                Some(id) => {
                    reference = Object::from(id);
                    &reference
                }
                None => space.object,
            };
            let mut faults = Faults::default();
            let read = Space::read(Objects(self.source), value, &LIMITS, &mut faults);
            let mut stops: Vec<&str> = read.err().iter().map(unread).collect();
            for &(place, fault, n) in faults.kept.told() {
                let damaged =
                    (place.at).is_some_and(|at| faults.damaged.contains(&(place.object, at)));
                match fault {
                    Fault::TooDeep => stops.push(DEEP),
                    Fault::TooManyVisits => stops.push(VISITS),
                    // The reader's, which the walk says (`unresolved`), and
                    // what's told of it, at its bytes.
                    Fault::Damaged { .. } => {}
                    _ if damaged => {}
                    _ => {
                        let object = place.object.or(space.id).or(space.holder);
                        if let Some(made) = self.made(object) {
                            let told = self.space_told(space, object, made, place.at, fault, n);
                            tells.push(told);
                        }
                    }
                }
            }
            if self.made(space.id.or(space.holder)).is_some() {
                stops.sort_unstable();
                stops.dedup();
                for why in stops {
                    self.unchecked("-", why.to_string(), space.table, "-");
                }
            }
        }
        self.find_told(tells);
    }

    /// What `fault` is, told `n` times by `space`'s read, in `object`, made
    /// as `made`, at `at`: at the key [`kind_key`] gives it, an element
    /// absent at it, a family's name told at an array's bytes at `0`
    /// ([`of_the_name`]), and any other at the key [`holding`] gives, in
    /// its object, `-` where none holds it; its path and table those of
    /// what that key is written in, as the walk visited it as a space or
    /// one of [`PARTS`], else the space's ([`Walk::path_and_table`]).
    fn space_told(
        &self,
        space: &Graphic<'a>,
        object: Option<ObjectId>,
        made: Made,
        at: Option<Span>,
        fault: Fault,
        n: usize,
    ) -> Told<Discriminant<Fault>> {
        let written = self.written(object);
        let held = written
            .zip(at)
            .map(|(written, at)| (holding(written, at), written, at));
        // The value at the fault's bytes, where it's written whole there.
        let there = held.as_ref().and_then(|(held, written, at)| {
            let there = held.as_ref().map_or(*written, |&(_, value)| value);
            (there.at == Some(*at)).then_some(there)
        });
        let (key, keyed) = match (kind_key(&fault), fault) {
            (Some(key), _) => (key.to_string(), Keyed::Entry(key)),
            (None, Fault::Missing { element }) => (element.to_string(), Keyed::Element),
            _ if of_the_name(fault) && there.is_some_and(|o| o.as_array().is_some()) => {
                ("0".to_string(), Keyed::Element)
            }
            _ => match held.as_ref().and_then(|(held, ..)| held.as_ref()) {
                Some((key, _)) => (key.clone(), Keyed::Holding),
                None => ("-".to_string(), Keyed::Holding),
            },
        };
        let whole = object.filter(|_| {
            held.as_ref()
                .is_some_and(|(held, written, at)| held.is_none() && written.at == Some(*at))
        });
        let family = there.filter(|_| of_a_family(fault)).and_then(|there| {
            // Through its references, as the walk reaches the space.
            let (mut object, mut space) = (object, there);
            for _ in 0..REFERENCES {
                let Some(id) = space.as_reference() else {
                    break;
                };
                (object, space) = (Some(id), self.source.get(id).ok()?);
            }
            Some((object, space.as_array()?.first()?.at?))
        });
        let first = object.and_then(|id| self.spaces_at.get(&id));
        let (path, table) = self.path_and_table(object, written, at, keyed, first, space);
        let inside = match of_the_elements(fault) {
            true => self.value_at(object, at),
            false => None,
        };
        let finding = Finding {
            object,
            made,
            path,
            table: table.to_string(),
            key,
            rule: Rule::ColorSpace,
            value: times(&fault.to_string(), n),
            at,
            revision: None,
        };
        Told {
            finding,
            kind: mem::discriminant(&fault),
            whole,
            family,
            inside,
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    //! What's found of the color spaces the walk reaches, where, and as
    //! whose, and what's counted; and the model's functions of colorants, a
    //! DeviceN space's and a halftone's, as a check of a file evaluates
    //! them.

    use pdf_structure::fixture::{self, Body, Xref};

    use super::*;
    use crate::bytes::{Options, WithReading};
    use crate::check::{Hold, Made, Report, Rule, check};
    use crate::dict;
    use crate::object::Dict;

    /// Every table the walk names a color space by is the model's, so a
    /// space reached as one is read.
    #[test]
    fn each_space_table_is_the_model_s() {
        for name in SPACES {
            assert!(crate::grammar().table(name).is_some(), "{name}");
        }
    }

    fn r(n: u32) -> Object {
        Object::reference(n)
    }

    fn numbers(n: &[i64]) -> Object {
        let numbers: Vec<Object> = n.iter().map(|&n| Object::from(n)).collect();
        numbers.into()
    }

    fn names(names: &[&str]) -> Object {
        let names: Vec<Object> = names.iter().map(|&n| Object::from(n)).collect();
        names.into()
    }

    /// The catalog, object 1, its page tree, 2, and its page, 3, of
    /// `resources`, written at `version` and checked beside its reading,
    /// every object held as built.
    fn checked(version: &str, resources: Dict) -> Report {
        let page = dict! {
            "Type" => "Page",
            "Parent" => r(2),
            "MediaBox" => numbers(&[0, 0, 612, 792]),
            "Resources" => resources,
        };
        let objects: Vec<(u32, Body)> = vec![
            (1, dict! { "Type" => "Catalog", "Pages" => r(2) }.into()),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }.into(),
            ),
            (3, page.into()),
        ];
        let file = fixture::file(version, &objects, dict! { "Root" => r(1) }, Xref::Table);
        let read = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        check(
            &read,
            &Hold::All(Made::Built),
            &crate::check::Options::default(),
        )
    }

    /// A DeviceN space of `names` over DeviceCMYK, its attributes
    /// `attributes`.
    fn device_n(names: &[&str], attributes: Dict) -> Object {
        let tint = dict! {
            "FunctionType" => 2,
            "Domain" => numbers(&[0, 1]),
            "C0" => numbers(&[0, 0, 0, 0]),
            "C1" => numbers(&[1, 1, 1, 1]),
            "N" => 1,
        };
        let space = vec![
            Object::from("DeviceN"),
            self::names(names),
            Object::from("DeviceCMYK"),
            tint.into(),
            attributes.into(),
        ];
        space.into()
    }

    /// What a report says of a DeviceN space's attributes: the keys found
    /// missing, and why each row not made wasn't.
    type Asked = (Vec<String>, Vec<(String, String)>);

    /// [`Asked`] of the space `space`, written in a page's resources and
    /// checked at `version`.
    fn asked(version: &str, space: Object) -> Asked {
        let report = checked(version, dict! { "ColorSpace" => dict! { "CS0" => space } });
        let missing = (report.findings.iter())
            .filter(|f| f.table == "DeviceNDict")
            .inspect(|f| assert_eq!(f.rule, Rule::Missing, "{f}"))
            .map(|f| f.key.clone())
            .collect();
        let unmade = (report.unchecked.keys())
            .filter(|u| u.table == "DeviceNDict")
            .map(|u| (u.key.clone(), u.why.clone()))
            .collect();
        (missing, unmade)
    }

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    /// An NChannel space is asked `/Colorants` where it names a spot
    /// colorant and `/Process` where it names a process one: neither where
    /// its process dictionary lists the names (ISO 32000-2, 8.6.6.5); none
    /// counted not made. Without a `/Subtype`, what its names decide is
    /// made, and the rest counted, the `/Subtype` in the way, as before.
    #[test]
    fn a_device_n_space_s_names_decide_its_colorants_and_process() {
        let nchannel = |more: Dict| {
            let mut attributes = dict! { "Subtype" => "NChannel" };
            for (key, value) in more.iter() {
                attributes.set(key, value.clone());
            }
            attributes
        };
        let process = |space: &str, components: &[&str]| {
            dict! { "ColorSpace" => space, "Components" => names(components) }
        };
        let spot = |name: &str| {
            let tint = dict! {
                "FunctionType" => 2,
                "Domain" => numbers(&[0, 1]),
                "C0" => numbers(&[0, 0, 0, 0]),
                "C1" => numbers(&[0, 1, 1, 0]),
                "N" => 1,
            };
            let space = vec![
                Object::from("Separation"),
                Object::from(name),
                Object::from("DeviceCMYK"),
                tint.into(),
            ];
            Object::from(space)
        };
        let none = (Vec::new(), Vec::new());
        let spots = device_n(&["Gold", "Silver"], nchannel(dict! {}));
        assert_eq!(asked("1.7", spots), (keys(&["Colorants"]), Vec::new()));
        let cmyk = ["Cyan", "Magenta", "Yellow", "Black"];
        let over_cmyk = nchannel(dict! { "Process" => process("DeviceCMYK", &cmyk) });
        assert_eq!(asked("1.7", device_n(&cmyk, over_cmyk)), none);
        let rgb = ["Red", "Green", "Blue"];
        let over_rgb = nchannel(dict! { "Process" => process("DeviceRGB", &rgb) });
        assert_eq!(
            asked("1.7", device_n(&rgb, over_rgb)),
            none,
            "listed: no spot"
        );
        let colorants = dict! { "Colorants" => dict! { "Gold" => spot("Gold") } };
        let cyan_and_gold = device_n(&["Cyan", "Gold"], nchannel(colorants));
        assert_eq!(
            asked("1.7", cyan_and_gold),
            (keys(&["Process"]), Vec::new())
        );
        // No /Subtype: the process row made where no name is a process
        // colorant, and counted where one is; the colorants row counted.
        let subtype = |key: &str| (key.to_string(), "@Subtype".to_string());
        assert_eq!(
            asked("1.7", device_n(&rgb, dict! {})),
            (Vec::new(), vec![subtype("Colorants")])
        );
        assert_eq!(
            asked("1.7", device_n(&["Cyan"], dict! {})),
            (Vec::new(), vec![subtype("Process")])
        );
    }

    /// In a PDF 1.3 file, `/Colorants`' version predicate is vacant, which
    /// leaves the function alone: a DeviceN space naming a spot colorant is
    /// asked it, as the evaluator reads the model's cell, where ISO 32000-2
    /// asks it of an NChannel space alone (8.6.6.5, Table 70).
    #[test]
    fn a_pdf_1_3_device_n_space_naming_a_spot_is_asked_its_colorants() {
        assert_eq!(
            asked("1.3", device_n(&["Gold"], dict! {})),
            (keys(&["Colorants"]), Vec::new())
        );
        let cmyk = device_n(&["Cyan", "Black"], dict! {});
        assert_eq!(asked("1.3", cmyk), (Vec::new(), Vec::new()));
    }

    /// A Type 5 halftone's other keys each name a colorant, a primary
    /// component or a spot one, made; `All` and `None` name none, each a
    /// `special` finding (ISO 32000-2, 10.6.5.6).
    #[test]
    fn a_halftone_s_keys_name_colorants_but_all_and_none() {
        let screen = || {
            dict! {
                "Type" => "Halftone",
                "HalftoneType" => 1,
                "Frequency" => 60,
                "Angle" => 45,
                "SpotFunction" => "Round",
            }
        };
        let mut halftone = dict! { "Type" => "Halftone", "HalftoneType" => 5 };
        for key in ["Default", "Cyan", "Spot1", "All", "None"] {
            halftone.set(key, screen());
        }
        let state = dict! { "Type" => "ExtGState", "HT" => halftone };
        let report = checked("1.7", dict! { "ExtGState" => dict! { "GS0" => state } });
        let found: Vec<(&str, Rule)> = (report.findings.iter())
            .filter(|f| f.table == "HalftoneType5")
            .map(|f| (f.key.as_str(), f.rule))
            .collect();
        assert_eq!(
            found,
            [("All", Rule::Special), ("None", Rule::Special)],
            "{report}"
        );
        assert!(
            (report.unchecked.keys()).all(|u| u.why != "fn:KeyNameIsColorant"),
            "{report}"
        );
    }

    /// Every table the walk names what a color space holds by is the
    /// model's, so a fault in an object first visited as one has its path.
    #[test]
    fn each_part_table_is_the_model_s() {
        for name in PARTS {
            assert!(crate::grammar().table(name).is_some(), "{name}");
        }
    }

    /// A file at 1.7 of a catalog, object 1, its page tree, 2, and its
    /// page, 3, whose `/ColorSpace` resources hold `spaces` as written, and
    /// `objects` besides, each its number and as written.
    fn spaces(spaces: &str, objects: &[(u32, &str)]) -> Vec<u8> {
        let page = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /ColorSpace \
             << {spaces} >> >> >>"
        );
        let mut all: Vec<(u32, &[u8])> = vec![
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page.as_bytes()),
        ];
        all.extend(objects.iter().map(|&(n, body)| (n, body.as_bytes())));
        let size = all.iter().map(|&(n, _)| n).max().unwrap_or(0) + 1;
        fixture::assembled("1.7", &all, &format!("<< /Size {size} /Root 1 0 R >>"))
    }

    /// A stream of `entries` and `data`, as written.
    fn stream(entries: &str, data: &str) -> String {
        let length = data.len();
        format!("<< {entries} /Length {length} >>\nstream\n{data}\nendstream")
    }

    /// Every object held, as merged.
    const ALL: Hold = Hold::All(Made::Merged);

    /// `file` read and checked, holding what `hold` does.
    fn report(file: &[u8], hold: &Hold) -> Report {
        check(
            &crate::bytes::read(file),
            hold,
            &crate::check::Options::default(),
        )
    }

    /// A color space finding, as [`found`] gives it.
    type Found = (Option<u32>, String, String, String, String);

    /// Each color space finding of `file`'s check, holding what `hold`
    /// does: its object's number, path, table and key, value, and the bytes
    /// it's at in the file.
    fn found(file: &[u8], hold: &Hold) -> Vec<Found> {
        let report = report(file, hold);
        (report.findings.iter())
            .filter(|f| f.rule == Rule::ColorSpace)
            .map(|f| {
                let at = f.at.map(|at| &file[at.start..at.end]).unwrap_or_default();
                (
                    f.object.map(|id| id.number),
                    f.path.clone(),
                    format!("{}/{}", f.table, f.key),
                    f.value.clone(),
                    String::from_utf8_lossy(at).into_owned(),
                )
            })
            .collect()
    }

    /// A finding of [`found`]'s, in object `n`, at the path from the page's
    /// `/ColorSpace` (`CS0[3]`), of the row `table`, of `fault` told once,
    /// at `bytes`.
    fn told(n: u32, path: &str, table: &str, fault: Fault, bytes: &str) -> Found {
        (
            Some(n),
            format!("Root/Pages/Kids[0]/Resources/ColorSpace/{path}"),
            table.to_string(),
            fault.to_string(),
            bytes.to_string(),
        )
    }

    /// The model's findings of `file`'s check, every object held: each
    /// one's object's number, table and key, and rule.
    fn model(file: &[u8]) -> Vec<(Option<u32>, String, Rule)> {
        let report = report(file, &ALL);
        (report.findings.iter())
            .filter(|f| f.rule != Rule::ColorSpace)
            .map(|f| {
                (
                    f.object.map(|id| id.number),
                    format!("{}/{}", f.table, f.key),
                    f.rule,
                )
            })
            .collect()
    }

    /// A tint transform of one input and four outputs, in place.
    const TINT: &str = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 1 1 1] /N 1 >>";

    /// A tint transform of two inputs and four outputs, a PostScript
    /// function's stream, object 12.
    fn tint_of_two() -> (u32, String) {
        let entries = "/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1]";
        (12, stream(entries, "{ pop pop 0 0 0 0 }"))
    }

    /// A Separation space of `name` over DeviceCMYK, in place.
    fn separation(name: &str) -> String {
        format!("[/Separation /{name} /DeviceCMYK {TINT}]")
    }

    /// What pdf-color tells of a space that the walk can't say is found
    /// once, at its bytes, at its key and in its object: the array's
    /// number, the profile's or the function's, or, written in place, its
    /// holder's; its path and table those of what its key is written in, a
    /// dictionary written in place too, as the walk first visited it as a
    /// space or as what a space holds, else the space's.
    #[test]
    fn what_a_space_s_read_tells_that_the_walk_can_t_is_found_once_at_its_key() {
        let (two, tint) = tint_of_two();
        let gold = separation("Gold");
        let nchannel = |names: &str, attributes: &str| {
            format!("/CS0 [/DeviceN [{names}] /DeviceCMYK 12 0 R {attributes}]")
        };
        let undefined = format!("<< /Subtype /NChannel /Colorants << /Gold {gold} >> >>");
        let none = nchannel("/None /Gold", &undefined);
        let cases: Vec<(Vec<u8>, Vec<Found>)> = vec![
            // A lookup table too long, written in place; too short, of its
            // own; a stream's, whose data isn't read.
            (
                spaces("/CS0 [/Indexed /DeviceRGB 1 <000000FFFFFF00>]", &[]),
                vec![told(
                    3,
                    "CS0",
                    "IndexedColorSpace/3",
                    Fault::LookupLength,
                    "<000000FFFFFF00>",
                )],
            ),
            (
                spaces("/CS0 10 0 R", &[(10, "[/Indexed /DeviceRGB 1 <000000>]")]),
                vec![told(
                    10,
                    "CS0",
                    "IndexedColorSpace/3",
                    Fault::LookupLength,
                    "<000000>",
                )],
            ),
            (
                spaces(
                    "/CS0 10 0 R",
                    &[
                        (10, "[/Indexed /DeviceRGB 1 11 0 R]"),
                        (11, &stream("", "\x00\x00")),
                    ],
                ),
                vec![],
            ),
            // A profile's alternate of another count, in the profile.
            (
                spaces(
                    "/CS0 [/ICCBased 11 0 R]",
                    &[(11, &stream("/N 3 /Alternate /DeviceGray", ""))],
                ),
                vec![told(
                    11,
                    "CS0[1]",
                    "ICCProfileStream/Alternate",
                    Fault::IccAlternate,
                    "/DeviceGray",
                )],
            ),
            // A tint transform of three outputs over DeviceCMYK, of its own
            // and in place.
            (
                spaces(
                    "/CS0 [/Separation /Gold /DeviceCMYK 12 0 R]",
                    &[(
                        12,
                        "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >>",
                    )],
                ),
                vec![told(
                    12,
                    "CS0[3]",
                    "FunctionType2/-",
                    Fault::TintShape,
                    "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >>",
                )],
            ),
            (
                spaces(
                    "/CS0 [/Separation /Gold /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 \
                     [0] /C1 [1] /N 1 >>]",
                    &[],
                ),
                vec![told(
                    3,
                    "CS0",
                    "SeparationColorSpace/3",
                    Fault::TintShape,
                    "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>",
                )],
            ),
            // An Indexed space whose base is itself.
            (
                spaces("/CS0 10 0 R", &[(10, "[/Indexed 10 0 R 0 <00>]")]),
                vec![told(
                    10,
                    "CS0",
                    "IndexedColorSpace/1",
                    Fault::Loop,
                    "10 0 R",
                )],
            ),
            // A base by a reference to a reference, in the object that's
            // the reference.
            (
                spaces(
                    "/CS0 10 0 R",
                    &[
                        (10, "[/Indexed 11 0 R 0 <00>]"),
                        (11, "12 0 R"),
                        (12, "/DeviceGray"),
                    ],
                ),
                vec![told(
                    11,
                    "CS0",
                    "IndexedColorSpace/-",
                    Fault::Chained,
                    "12 0 R",
                )],
            ),
            // Elements past a family's: where the model lists none, told;
            // where the array fits another table better, which the model
            // finds of a type, the model's (below).
            (
                spaces("/CS0 [/Indexed /DeviceGray 0 <00> 1]", &[]),
                vec![told(
                    3,
                    "CS0",
                    "IndexedColorSpace/4",
                    Fault::ExtraElements,
                    "1",
                )],
            ),
            // A CalGray dictionary's black point and white point of four
            // numbers, which the model lists three of, at the dictionary
            // written in place in the space.
            (
                spaces(
                    "/CS0 [/CalGray << /WhitePoint [0.9505 1 1.089] /BlackPoint [0 0 0 0] >>] \
                     /CS1 [/CalGray << /WhitePoint [0.9505 1 1.089 1] >>]",
                    &[],
                ),
                vec![
                    told(
                        3,
                        "CS0[1]",
                        "CalGrayDict/BlackPoint",
                        Fault::BlackPoint,
                        "[0 0 0 0]",
                    ),
                    told(
                        3,
                        "CS1[1]",
                        "CalGrayDict/WhitePoint",
                        Fault::WhitePoint,
                        "[0.9505 1 1.089 1]",
                    ),
                ],
            ),
            // DeviceN spaces' names: a colorant twice, All, None in an
            // NChannel space.
            (
                spaces(
                    "/CS0 [/DeviceN [/Gold /Gold] /DeviceCMYK 12 0 R]",
                    &[(two, &tint)],
                ),
                vec![told(
                    3,
                    "CS0",
                    "DeviceNColorSpace/1",
                    Fault::ColorantRepeated,
                    "[/Gold /Gold]",
                )],
            ),
            (
                spaces(&format!("/CS0 [/DeviceN [/All] /DeviceCMYK {TINT}]"), &[]),
                vec![told(
                    3,
                    "CS0",
                    "DeviceNColorSpace/1",
                    Fault::AllInDeviceN,
                    "[/All]",
                )],
            ),
            (
                spaces(&none, &[(two, &tint)]),
                vec![told(
                    3,
                    "CS0",
                    "DeviceNColorSpace/1",
                    Fault::NoneInNChannel,
                    "[/None /Gold]",
                )],
            ),
            // An NChannel space's spot colorant /Colorants lacks.
            (
                spaces(&nchannel("/Gold /Silver", &undefined), &[(two, &tint)]),
                vec![told(
                    3,
                    "CS0[4]",
                    "DeviceNDict/Colorants",
                    Fault::ColorantUndefined,
                    &undefined,
                )],
            ),
            // A process dictionary's components, two for DeviceCMYK's four.
            (
                spaces(
                    &format!(
                        "/CS0 [/DeviceN [/Cyan] /DeviceCMYK {TINT} << /Subtype /NChannel \
                         /Process << /ColorSpace /DeviceCMYK /Components [/Cyan /Magenta] >> >>]"
                    ),
                    &[],
                ),
                vec![told(
                    3,
                    "CS0[4]/Process",
                    "DeviceNProcess/Components",
                    Fault::ProcessComponents,
                    "[/Cyan /Magenta]",
                )],
            ),
        ];
        for (i, (file, expected)) in cases.iter().enumerate() {
            assert_eq!(
                found(file, &ALL),
                *expected,
                "case {i}: {}",
                report(file, &ALL)
            );
        }
    }

    /// What the walk says of a space by the model's rules isn't told again,
    /// at the key it's at, in place or through a reference: a high value
    /// past 255, a family of no name, a profile without `/N` or with one
    /// that isn't 1, 3 or 4, a space of no elements or too few, a base
    /// that doesn't read, a profile that isn't a stream, an array that fits
    /// another table better, of a type that table doesn't allow.
    #[test]
    fn what_the_walk_says_of_a_space_is_found_by_it_alone() {
        let profile = |entries: &str| stream(entries, "");
        // Each file, and the object, table and key, and rule of what the
        // walk finds there.
        type Said<'a> = (Option<u32>, &'a str, Rule);
        let cases: Vec<(Vec<u8>, Said)> = vec![
            (
                spaces("/CS0 [/Indexed /DeviceRGB 256 <000000>]", &[]),
                (Some(3), "IndexedColorSpace/2", Rule::Value),
            ),
            (
                spaces("/CS0 [/Indexed /DeviceRGB 10 0 R <000000>]", &[(10, "256")]),
                (Some(3), "IndexedColorSpace/2", Rule::Value),
            ),
            (
                spaces("/CS0 [/Foo]", &[]),
                (Some(3), "CalGrayColorSpace/0", Rule::Value),
            ),
            (
                spaces("/CS0 [/ICCBased 11 0 R]", &[(11, &profile(""))]),
                (Some(11), "ICCProfileStream/N", Rule::Missing),
            ),
            (
                spaces("/CS0 [/ICCBased 11 0 R]", &[(11, &profile("/N 2"))]),
                (Some(11), "ICCProfileStream/N", Rule::Value),
            ),
            (
                spaces(
                    "/CS0 [/ICCBased 11 0 R]",
                    &[(11, &profile("/N 12 0 R")), (12, "2")],
                ),
                (Some(11), "ICCProfileStream/N", Rule::Value),
            ),
            (
                spaces("/CS0 [/ICCBased 11 0 R]", &[(11, "<< /N 3 >>")]),
                (Some(3), "ICCBasedColorSpace/1", Rule::Type),
            ),
            (
                spaces("/CS0 []", &[]),
                (Some(3), "CalGrayColorSpace/0", Rule::Missing),
            ),
            (
                spaces("/CS0 10 0 R", &[(10, "[]")]),
                (Some(10), "CalGrayColorSpace/0", Rule::Missing),
            ),
            (
                spaces("/CS0 [/Indexed /DeviceRGB 1]", &[]),
                (Some(3), "IndexedColorSpace/3", Rule::Missing),
            ),
            (
                spaces("/CS0 [/Indexed 11 0 R 0 <00>]", &[(11, "]")]),
                (Some(3), "IndexedColorSpace/1", Rule::Unresolved),
            ),
            // A tint transform's subfunction that doesn't read, which the
            // tint's read tells at the tint, the walk at the reference.
            (
                spaces(
                    "/CS0 [/Separation /Gold /DeviceCMYK << /FunctionType 3 /Domain [0 1] \
                     /Functions [11 0 R] /Bounds [] /Encode [0 1] >>]",
                    &[(11, "]")],
                ),
                (Some(3), "ArrayOfFunctions/0", Rule::Unresolved),
            ),
            (
                spaces("/CS0 [/DeviceGray 1]", &[]),
                (Some(3), "CalGrayColorSpace/1", Rule::Type),
            ),
        ];
        for (i, (file, said)) in cases.iter().enumerate() {
            let (object, key, rule) = *said;
            let said = (object, key.to_string(), rule);
            let report = report(file, &ALL);
            assert!(model(file).contains(&said), "case {i}: {report}");
            assert_eq!(found(file, &ALL), [], "case {i}: {report}");
        }

        // A space of a family that may not be where it is, an array in
        // place or through a reference: an Indexed space's base, an
        // alternate, a Pattern space's underlying space, a profile's
        // alternate, a /Colorants entry. The model links each to the tables
        // of the families that may be there alone, so the walk finds its
        // family's name, element 0, of the table it takes it for, a value
        // that table doesn't have.
        let one = "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>";
        let (two, tint) = tint_of_two();
        let families: Vec<(Vec<u8>, u32)> = vec![
            (spaces("/CS0 [/Indexed [/Pattern] 0 <00>]", &[]), 3),
            (
                spaces("/CS0 [/Indexed 11 0 R 0 <00>]", &[(11, "[/Pattern]")]),
                11,
            ),
            (
                spaces(
                    &format!("/CS0 [/Separation /Gold [/Indexed /DeviceGray 0 <00>] {one}]"),
                    &[],
                ),
                3,
            ),
            (spaces("/CS0 [/Pattern [/Pattern]]", &[]), 3),
            (
                spaces(
                    "/CS0 [/ICCBased 11 0 R]",
                    &[(11, &profile("/N 1 /Alternate [/Pattern]"))],
                ),
                11,
            ),
            (
                spaces(
                    "/CS0 [/DeviceN [/Gold /Silver] /DeviceCMYK 12 0 R << /Colorants << /Gold \
                     [/DeviceGray] >> >>]",
                    &[(two, &tint)],
                ),
                3,
            ),
        ];
        for (i, (file, n)) in families.iter().enumerate() {
            let report = report(file, &ALL);
            let named = |f: &Finding| {
                f.object == Some(ObjectId::new(*n, 0)) && f.rule == Rule::Value && f.key == "0"
            };
            assert!(report.findings.iter().any(named), "family {i}: {report}");
            assert_eq!(found(file, &ALL), [], "family {i}: {report}");
        }
    }

    /// A kind about one key of its dictionary is at that key, whether the
    /// key is absent, which the model finds `missing`, or written, which it
    /// doesn't say, the dictionary in place in the space or an object of
    /// its own, its path and table the dictionary's either way: a CIE-based
    /// space's `/WhitePoint`, a process dictionary's `/ColorSpace` and
    /// `/Components`, and an NChannel space's `/Colorants`; and a profile's
    /// `/N`, absent and written, the model's either way. A `/WhitePoint`
    /// the model finds a value of, an element 0, is the model's.
    #[test]
    fn a_kind_about_a_key_is_at_it_absent_or_written() {
        // Each dictionary in place, and as object 13.
        let both = |space: &dyn Fn(&str) -> String, dict: &str| {
            [
                spaces(&format!("/CS0 {}", space(dict)), &[]),
                spaces(&format!("/CS0 {}", space("13 0 R")), &[(13, dict)]),
            ]
        };
        let cal_rgb = |dict: &str| format!("[/CalRGB {dict}]");
        let absent = both(&cal_rgb, "<< /Gamma [1 1 1] >>");
        for (file, n) in absent.iter().zip([3, 13]) {
            let said = (Some(n), "CalRGBDict/WhitePoint".to_string(), Rule::Missing);
            assert!(model(file).contains(&said), "{}", report(file, &ALL));
            assert_eq!(found(file, &ALL), []);
        }
        for (file, n) in both(&cal_rgb, "<< /WhitePoint [0 1 1] >>")
            .iter()
            .zip([3, 13])
        {
            let said = (Some(n), "WhitepointArray/0".to_string(), Rule::Value);
            assert!(model(file).contains(&said), "{}", report(file, &ALL));
            assert_eq!(found(file, &ALL), []);
        }
        let [in_place, own] = both(&cal_rgb, "<< /WhitePoint [0.95 1 1.09 1] >>");
        let white = Fault::WhitePoint;
        let point = "[0.95 1 1.09 1]";
        assert_eq!(
            found(&in_place, &ALL),
            [told(3, "CS0[1]", "CalRGBDict/WhitePoint", white, point)]
        );
        assert_eq!(
            found(&own, &ALL),
            [told(13, "CS0[1]", "CalRGBDict/WhitePoint", white, point)]
        );

        let process = |dict: &str| {
            format!(
                "[/DeviceN [/Cyan] /DeviceCMYK {TINT} << /Subtype /NChannel /Process {dict} >>]"
            )
        };
        for (dict, key) in [
            (
                "<< /Components [/Cyan /Magenta /Yellow /Black] >>",
                "ColorSpace",
            ),
            ("<< /ColorSpace /DeviceCMYK >>", "Components"),
        ] {
            for file in both(&process, dict) {
                let report = report(&file, &ALL);
                let missing = |f: &Finding| f.rule == Rule::Missing && f.key == key;
                assert!(report.findings.iter().any(missing), "{report}");
                assert_eq!(found(&file, &ALL), [], "{report}");
            }
        }
        let components = "<< /ColorSpace /DeviceCMYK /Components [/Cyan] >>";
        let [in_place, own] = both(&process, components);
        let fault = Fault::ProcessComponents;
        assert_eq!(
            found(&in_place, &ALL),
            [told(
                3,
                "CS0[4]/Process",
                "DeviceNProcess/Components",
                fault,
                "[/Cyan]"
            )]
        );
        assert_eq!(
            found(&own, &ALL),
            [told(
                13,
                "CS0[4]/Process",
                "DeviceNProcess/Components",
                fault,
                "[/Cyan]"
            )]
        );
        // Lab, of three components, which no process dictionary may have:
        // the model links its /ColorSpace to the tables of the others
        // alone, and finds the family's name the table the walk takes it
        // for doesn't have.
        let lab = "<< /ColorSpace [/Lab << /WhitePoint [1 1 1] >>] /Components [/L /A /B] >>";
        for (file, n) in both(&process, lab).iter().zip([3, 13]) {
            let report = report(file, &ALL);
            let named = |f: &Finding| {
                f.object == Some(ObjectId::new(n, 0)) && f.rule == Rule::Value && f.key == "0"
            };
            assert!(report.findings.iter().any(named), "{report}");
            assert_eq!(found(file, &ALL), [], "{report}");
        }

        let gold = separation("Gold");
        let nchannel = |dict: &str| format!("[/DeviceN [/Gold /Silver] /DeviceCMYK 12 0 R {dict}]");
        let (two, tint) = tint_of_two();
        for (dict, told_at) in [
            ("<< /Subtype /NChannel >>".to_string(), None),
            (
                format!("<< /Subtype /NChannel /Colorants << /Gold {gold} >> >>"),
                Some("Colorants"),
            ),
        ] {
            let in_place = spaces(&format!("/CS0 {}", nchannel(&dict)), &[(two, &tint)]);
            let own = spaces(
                &format!("/CS0 {}", nchannel("13 0 R")),
                &[(two, &tint), (13, &dict)],
            );
            let undefined = Fault::ColorantUndefined;
            match told_at {
                None => {
                    for file in [in_place, own] {
                        let report = report(&file, &ALL);
                        let missing = |f: &Finding| f.rule == Rule::Missing && f.key == "Colorants";
                        assert!(report.findings.iter().any(missing), "{report}");
                        assert_eq!(found(&file, &ALL), [], "{report}");
                    }
                }
                Some(key) => {
                    let table = format!("DeviceNDict/{key}");
                    assert_eq!(
                        found(&in_place, &ALL),
                        [told(3, "CS0[4]", &table, undefined, &dict)]
                    );
                    assert_eq!(
                        found(&own, &ALL),
                        [told(13, "CS0[4]", &table, undefined, &dict)]
                    );
                }
            }
        }
    }

    /// A kind of what an array at its key is as a whole, its count and its
    /// elements, isn't told where the walk found something at or inside the
    /// array, whose elements the model checks each: a CalRGB space's
    /// `/WhitePoint` with a first element that's 0, a second that isn't 1,
    /// one short, one a string, one a reference to nothing; a
    /// `/BlackPoint` below 0; a `/Gamma` short; a `/Matrix` short or with a
    /// string; and a profile's `/Range` with a string. Each with the
    /// dictionary written in place in the space, an object of its own, and,
    /// the array an object of its own, through a reference to it.
    #[test]
    fn what_the_walk_finds_in_an_array_a_kind_is_of_says_it() {
        let white = "/WhitePoint [0.95 1 1.09]";
        for (key, array, with) in [
            ("WhitePoint", "[0 1 1]", ""),
            ("WhitePoint", "[0.95 2 1.09]", ""),
            ("WhitePoint", "[0.95 1]", ""),
            ("WhitePoint", "[0.95 1 (x)]", ""),
            ("WhitePoint", "[0.95 1 99 0 R]", ""),
            ("BlackPoint", "[-1 0 0]", white),
            ("Gamma", "[1 1]", white),
            ("Matrix", "[1 0 0 0 1 0 0 0]", white),
            ("Matrix", "[1 0 0 0 1 0 0 0 (x)]", white),
        ] {
            let entries = format!("{with} /{key} {array}");
            let by_reference = format!("{with} /{key} 14 0 R");
            let files = [
                (spaces(&format!("/CS0 [/CalRGB << {entries} >>]"), &[]), 3),
                (
                    spaces(
                        "/CS0 [/CalRGB 13 0 R]",
                        &[(13, &format!("<< {entries} >>"))],
                    ),
                    13,
                ),
                (
                    spaces(
                        "/CS0 [/CalRGB 13 0 R]",
                        &[(13, &format!("<< {by_reference} >>")), (14, array)],
                    ),
                    14,
                ),
            ];
            for (file, n) in files {
                let report = report(&file, &ALL);
                let inside = |f: &Finding| {
                    f.object == Some(ObjectId::new(n, 0)) && f.rule != Rule::ColorSpace
                };
                assert!(
                    report.findings.iter().any(inside),
                    "{key} {array}: {report}"
                );
                assert_eq!(found(&file, &ALL), [], "{key} {array}: {report}");
            }
        }
        let profile = spaces(
            "/CS0 [/ICCBased 11 0 R]",
            &[(11, &stream("/N 1 /Range [0 (x)]", "x"))],
        );
        let said = (Some(11), "ArrayOfNumbersGeneral/1".to_string(), Rule::Type);
        assert!(
            model(&profile).contains(&said),
            "{}",
            report(&profile, &ALL)
        );
        assert_eq!(found(&profile, &ALL), []);
    }

    /// What two spaces' reads tell in one object is told once, by the
    /// first: a space two pages' resources share, and an Indexed space's
    /// base the walk reaches alone too, each read again.
    #[test]
    fn what_two_reads_tell_in_one_object_is_told_once() {
        let page = |resources: &str| {
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources {resources} >>"
            )
        };
        let first = page("<< /ColorSpace << /CS0 [/Indexed 11 0 R 0 <000000>] /CS1 11 0 R >> >>");
        let second = page("<< /ColorSpace << /CS0 11 0 R >> >>");
        let profile = stream("/N 3 /Alternate /DeviceGray", "");
        let objects: [(u32, &[u8]); 6] = [
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"),
            (3, first.as_bytes()),
            (4, second.as_bytes()),
            (11, b"[/ICCBased 12 0 R]"),
            (12, profile.as_bytes()),
        ];
        let file = fixture::assembled("1.7", &objects, "<< /Size 13 /Root 1 0 R >>");
        assert_eq!(
            found(&file, &ALL),
            [told(
                12,
                "CS1[1]",
                "ICCProfileStream/Alternate",
                Fault::IccAlternate,
                "/DeviceGray"
            )]
        );
    }

    /// A file at 1.7 of `objects`, each written at its number as given, and
    /// `packed`, each its number and as written, in object stream 20, with
    /// cross-reference stream 21 listing them all, `trailer`'s entries
    /// among its own.
    pub(in crate::check) fn packed(
        objects: &[(u32, &str)],
        packed: &[(u32, &str)],
        trailer: &str,
    ) -> Vec<u8> {
        let mut file = b"%PDF-1.7\n".to_vec();
        let mut entries = std::collections::BTreeMap::new();
        for &(n, body) in objects {
            entries.insert(n, (1u8, file.len(), 0u16));
            file.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let (mut header, mut data) = (String::new(), String::new());
        for (i, &(m, body)) in packed.iter().enumerate() {
            header.push_str(&format!("{m} {} ", data.len()));
            data.push_str(body);
            data.push('\n');
            entries.insert(m, (2, 20, i as u16));
        }
        let (count, first) = (packed.len(), header.len());
        entries.insert(20, (1, file.len(), 0));
        let body = stream(
            &format!("/Type /ObjStm /N {count} /First {first}"),
            &format!("{header}{data}"),
        );
        file.extend_from_slice(format!("20 0 obj\n{body}\nendobj\n").as_bytes());
        let xref = file.len();
        entries.insert(21, (1, xref, 0));
        let rows: Vec<u8> = (0..22u32)
            .flat_map(|n| {
                let (kind, field, index) = entries.get(&n).copied().unwrap_or((0, 0, 0));
                let field = u32::try_from(field).unwrap().to_be_bytes();
                [&[kind][..], &field, &index.to_be_bytes()].concat()
            })
            .collect();
        let head = format!(
            "21 0 obj\n<< /Type /XRef /Size 22 /W [1 4 2] {trailer} /Length {} >>\nstream\n",
            rows.len()
        );
        file.extend_from_slice(head.as_bytes());
        file.extend_from_slice(&rows);
        file.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        file
    }

    /// The rows a check counts as not checked for a color space: why, its
    /// table, and how many.
    fn unread_spaces(report: &Report) -> Vec<(&str, &str, usize)> {
        (report.unchecked.iter())
            .filter(|(u, _)| u.column == "-" && u.key == "-" && u.why.starts_with("a color space"))
            .map(|(u, &n)| (u.why.as_str(), u.table.as_str(), n))
            .collect()
    }

    /// A space not read whole for what isn't the file's fault is counted as
    /// not checked, once, its table the space's, where the space or what
    /// it's written in is held, and what was read of it is found all the
    /// same: one whose base takes a password, in an object stream of a file
    /// the reader can't open, one nested 65 deep, and one looking up more
    /// than 4,096 objects and names.
    #[test]
    fn a_space_not_read_whole_for_what_isnt_the_file_s_fault_is_counted() {
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << \
                    /ColorSpace << /CS0 10 0 R >> >> >>";
        let objects = [
            (1, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page),
            (10, "[/Indexed 11 0 R 0 <000000> /x]"),
        ];
        let trailer = "/Root 1 0 R /Encrypt << /Filter /Standard /V 2 >>";
        let locked = packed(&objects, &[(11, "/DeviceRGB")], trailer);
        let checked = report(&locked, &ALL);
        assert_eq!(
            unread_spaces(&checked),
            [(LOCKED, "IndexedColorSpace", 1)],
            "{checked}"
        );
        // What was read before it.
        assert_eq!(
            found(&locked, &ALL),
            [told(
                10,
                "CS0",
                "IndexedColorSpace/4",
                Fault::ExtraElements,
                "/x"
            )]
        );
        let only = |n| Hold::Only([(ObjectId::new(n, 0), Made::Merged)].into());
        assert_eq!(unread_spaces(&report(&locked, &only(10))).len(), 1);
        assert_eq!(unread_spaces(&report(&locked, &only(3))), []);

        // Profiles 10 to 74, each the alternate of the one before, the
        // page's space 65 deep at the last's.
        let profiles: Vec<(u32, String)> = (10..75)
            .map(|n| {
                let alternate = match n {
                    74 => "/DeviceGray".to_string(),
                    n => format!("[/ICCBased {} 0 R]", n + 1),
                };
                (n, stream(&format!("/N 1 /Alternate {alternate}"), ""))
            })
            .collect();
        let profiles: Vec<(u32, &str)> = profiles.iter().map(|(n, s)| (*n, s.as_str())).collect();
        let deep = spaces("/CS0 [/ICCBased 10 0 R]", &profiles);
        let checked = report(&deep, &ALL);
        assert_eq!(
            unread_spaces(&checked),
            [(DEEP, "ICCBasedColorSpace", 1)],
            "{checked}"
        );
        assert_eq!(found(&deep, &ALL), []);

        // 4,097 names, written in place.
        let names: Vec<String> = (0..4097).map(|i| format!("/C{i}")).collect();
        let domain = vec!["0 1"; 4097].join(" ");
        let tint = stream(
            &format!("/FunctionType 4 /Domain [{domain}] /Range [0 1]"),
            "{ }",
        );
        let many = spaces(
            &format!("/CS0 [/DeviceN [{}] /DeviceGray 12 0 R]", names.join(" ")),
            &[(12, &tint)],
        );
        let checked = report(&many, &ALL);
        assert_eq!(
            unread_spaces(&checked),
            [(VISITS, "DeviceNColorSpace", 1)],
            "{checked}"
        );
        assert_eq!(found(&many, &ALL), []);
    }

    /// A space's finding is found where its object is held: a space of its
    /// own, held or not, whatever holds its page; one written in place,
    /// where its page is.
    #[test]
    fn a_space_s_finding_is_held_where_its_object_is() {
        let only = |n| Hold::Only([(ObjectId::new(n, 0), Made::Merged)].into());
        let own = spaces("/CS0 10 0 R", &[(10, "[/Indexed /DeviceRGB 1 <000000>]")]);
        let lookup = Fault::LookupLength;
        let short = told(10, "CS0", "IndexedColorSpace/3", lookup, "<000000>");
        assert_eq!(found(&own, &only(10)), [short]);
        assert_eq!(found(&own, &only(3)), []);
        let in_place = spaces("/CS0 [/Indexed /DeviceRGB 1 <000000>]", &[]);
        let short = told(3, "CS0", "IndexedColorSpace/3", lookup, "<000000>");
        assert_eq!(found(&in_place, &only(3)), [short]);
        assert_eq!(found(&in_place, &only(2)), []);
    }

    /// A color space finding is carried by a like one, whose count differs:
    /// by number, by path and from a source; one of a kind the earlier
    /// report hasn't is the file's own.
    #[test]
    fn a_color_space_finding_is_carried_whatever_its_count() {
        let (two, tint) = tint_of_two();
        let report = |colorants: &str, more: &str| {
            let options = crate::check::Options {
                paths: true,
                ..crate::check::Options::default()
            };
            let space = format!(
                "/CS0 [/DeviceN [/Gold /Silver] /DeviceCMYK 12 0 R << /Colorants << {colorants} \
                 >> >>{more}]"
            );
            let file = spaces(&space, &[(two, &tint)]);
            check(&crate::bytes::read(&file), &ALL, &options)
        };
        fn values(found: Vec<&Finding>) -> Vec<&str> {
            (found.into_iter())
                .filter(|f| f.rule == Rule::ColorSpace)
                .map(|f| f.value.as_str())
                .collect()
        }
        // /Colorants entries that are Separation spaces of other colorants.
        let separation = Fault::ColorantNotSeparation.to_string();
        let extra = Fault::ExtraElements.to_string();
        let (gold, silver) = (self::separation("Gold"), self::separation("Silver"));
        let before = report(&format!("/Gold {silver} /Silver {gold}"), " /x");
        let after = report(&format!("/Gold {silver} /Silver {silver}"), "");
        let twice = format!("{separation}, 2 times");
        assert_eq!(
            values(before.findings.iter().collect()),
            [extra.as_str(), &twice],
            "{before}"
        );
        assert_eq!(
            values(after.findings.iter().collect()),
            [separation.as_str()],
            "{after}"
        );
        assert_eq!(values(after.since(&before)), Vec::<&str>::new());
        assert_eq!(values(after.by_path(&before)), Vec::<&str>::new());
        assert_eq!(values(after.beyond(&[&before])), Vec::<&str>::new());
        assert_eq!(values(before.since(&after)), [extra.as_str()]);
        assert_eq!(values(before.by_path(&after)), [extra.as_str()]);
        assert_eq!(values(before.beyond(&[&after])), [extra.as_str()]);
    }
}
