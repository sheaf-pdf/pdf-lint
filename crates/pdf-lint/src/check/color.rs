//! Color spaces, read through pdf-color (ISO 32000-2, 8.6): each the walk
//! reached in the newest revision as one of the model's color space tables,
//! in the order it reached them, read again by [`Space::read`] under
//! pdf-color's caps, from its number where it has one, so that what's told
//! of it is in its object, else as it's written. What the read tells is
//! kept ([`Kept`]); none of it is found yet.

use std::mem;

use pdf_color::{Fault, Limits, Space};

use super::kept::Kept;
use super::{Objects, Walk};
use crate::object::Object;

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

/// What color spaces are read under: pdf-color's caps, which ISO 32000-2
/// doesn't set.
const LIMITS: Limits = Limits::DEFAULT;

impl Walk<'_> {
    /// Reads each color space the walk reached, in the order it reached
    /// them: see the module's documentation.
    pub(super) fn spaces(&mut self) {
        let spaces = mem::take(&mut self.spaces);
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
            let mut kept: Kept<Fault> = Kept::default();
            let _ = Space::read(Objects(self.source), value, &LIMITS, &mut kept);
        }
    }
}

#[cfg(test)]
mod tests {
    //! What's read of the color spaces the walk reaches; and the model's
    //! functions of colorants, a DeviceN space's and a halftone's, as a
    //! check of a file evaluates them.

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
}
