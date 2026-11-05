//! Each family well-formed, each fault by its clause, and what a sink can't
//! change.

use std::cell::RefCell;
use std::collections::BTreeMap;

use pdf_objects::fault::Place;
use pdf_objects::parse::Parser;
use pdf_objects::{ObjectId, Unreadable, Why};
use pdf_structure::bytes::{Options, Reading};

use super::*;
use crate::fault::Fault;
use crate::{components, counted};

/// A white point, as a CIE-based dictionary has it.
const CAL: &str = "<< /WhitePoint [0.95 1 1.08] >>";
/// A function of one input and one output.
const TINT_1: &str = "<< /FunctionType 2 /Domain [0 1] /N 1 >>";
/// A function of one input and four outputs.
const TINT_4: &str = "<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0 0] /C1 [1 1 1 1] >>";
/// A function of two inputs and one output, as a stream: object 5 wherever it's
/// asked for.
const TINT_2_1: &str = "<< /Length 0 /FunctionType 4 /Domain [0 1 0 1] /Range [0 1] >>";

/// `text` as one value.
fn value(text: &str) -> Object {
    let mut parser = Parser::at(text.as_bytes(), 0);
    (parser.value(&mut ())).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// A value and its objects, kept for the test's length.
#[derive(Clone, Copy)]
struct File {
    value: &'static Object,
    objects: &'static BTreeMap<ObjectId, Object>,
}

/// `text`, with `extra` for its objects, the first of them object 5, a dictionary
/// with `/Length` a stream.
fn file(text: &str, extra: &[&str]) -> File {
    let objects = (extra.iter().zip(5..))
        .map(|(text, n)| {
            let object = value(text);
            let stream = (object.as_dict())
                .filter(|dict| dict.has(b"Length"))
                .map(|dict| Object::stream(dict.clone()));
            (ObjectId::new(n, 0), stream.unwrap_or(object))
        })
        .collect();
    File {
        value: Box::leak(Box::new(value(text))),
        objects: Box::leak(Box::new(objects)),
    }
}

/// A resolver that lists what it's asked and makes these numbers fail, each as
/// [`Why`] says.
#[derive(Clone, Copy)]
struct Asked<'c> {
    file: File,
    asked: &'c RefCell<Vec<u32>>,
    failing: &'c [(u32, Why)],
}

impl Resolve<'static> for Asked<'_> {
    fn resolve(&self, id: ObjectId) -> Option<&'static Object> {
        self.try_resolve(id).ok().flatten()
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'static Object>, Unreadable> {
        self.asked.borrow_mut().push(id.number);
        if let Some((_, why)) = self.failing.iter().find(|(n, _)| *n == id.number) {
            return Err(Unreadable::new(id, *why, "made to fail"));
        }
        Ok(self.file.objects.get(&id))
    }
}

/// A sink that wants no faults, and that's told none.
struct Unwanted;

impl Sink<Fault> for Unwanted {
    fn fault(&mut self, _: Place, fault: Fault) {
        panic!("told {fault:?}, wanting none");
    }

    fn wants(&self) -> bool {
        false
    }
}

/// The space read of `file` under `limits`, and its faults, the same by `()`, by a
/// sink that wants none and through a `dyn`, and no more looked up than the caps
/// allow.
fn read_under(file: File, limits: &Limits) -> (Option<Space<'static>>, Vec<Fault>) {
    let resolve = |id: ObjectId| file.objects.get(&id);
    let mut kept = Vec::new();
    let space = Space::read(resolve, file.value, limits, &mut kept).unwrap();
    assert_eq!(
        Space::read(resolve, file.value, limits, &mut ()).unwrap(),
        space
    );
    assert_eq!(
        Space::read(resolve, file.value, limits, &mut Unwanted).unwrap(),
        space
    );
    let dynamic: &mut dyn Sink<Fault> = &mut ();
    assert_eq!(
        Space::read(resolve, file.value, limits, dynamic).unwrap(),
        space
    );
    (space, kept.into_iter().map(|(_, fault)| fault).collect())
}

/// [`read_under`] the default limits.
fn read(text: &str, extra: &[&str]) -> (Option<Space<'static>>, Vec<Fault>) {
    read_under(file(text, extra), &Limits::DEFAULT)
}

/// What's looked up, in order, of `file`, by `()` and by a sink that keeps faults.
fn lookups(file: File) -> (Vec<u32>, Vec<u32>) {
    let asked = RefCell::new(Vec::new());
    let resolver = Asked {
        file,
        asked: &asked,
        failing: &[],
    };
    Space::read(resolver, file.value, &Limits::DEFAULT, &mut ()).unwrap();
    let plain = asked.replace(Vec::new());
    let mut kept = Vec::new();
    Space::read(resolver, file.value, &Limits::DEFAULT, &mut kept).unwrap();
    (plain, asked.take())
}

#[test]
fn each_family_well_formed_tells_nothing_and_counts_as_pdfium() {
    const NCHANNEL: &str = "[/DeviceN [/Cyan /Spot] /DeviceCMYK 5 0 R << \
        /Subtype /NChannel \
        /Colorants << /Spot [/Separation /Spot /DeviceGray << /FunctionType 2 /Domain [0 1] /N 1 >>] >> \
        /Process << /ColorSpace /DeviceCMYK /Components [/Cyan /Magenta /Yellow /Black] >> \
        /MixingHints << >> >>]";
    const TO_CMYK: &str =
        "<< /Length 0 /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1] >>";
    let icc = "<< /Length 0 /N 3 /Alternate /DeviceRGB /Range [0 1 0 1 0 1] >>";
    let cases: &[(&str, &[&str], Family, u32)] = &[
        ("/DeviceGray", &[], Family::DeviceGray, 1),
        ("/DeviceRGB", &[], Family::DeviceRGB, 3),
        ("/DeviceCMYK", &[], Family::DeviceCMYK, 4),
        ("[/DeviceRGB]", &[], Family::DeviceRGB, 3),
        ("5 0 R", &["/DeviceCMYK"], Family::DeviceCMYK, 4),
        ("[5 0 R]", &["/DeviceCMYK"], Family::DeviceCMYK, 4),
        ("/Pattern", &[], Family::Pattern, 1),
        ("[/Pattern]", &[], Family::Pattern, 1),
        ("[/Pattern /DeviceRGB]", &[], Family::Pattern, 4),
        ("[/Pattern 5 0 R]", &["/DeviceCMYK"], Family::Pattern, 5),
        ("[/CalGray 5 0 R]", &[CAL], Family::CalGray, 1),
        (
            "[/CalGray << /WhitePoint [1 1 1] /BlackPoint [0 0 0.5] /Gamma 2.2 >>]",
            &[],
            Family::CalGray,
            1,
        ),
        (
            "[/CalRGB << /WhitePoint [1 1 1] /Gamma [1 1 1] /Matrix [1 0 0 0 1 0 0 0 1] >>]",
            &[],
            Family::CalRGB,
            3,
        ),
        (
            "[/Lab << /WhitePoint [1 1 1] /Range [-128 127 -128 127] >>]",
            &[],
            Family::Lab,
            3,
        ),
        ("[/ICCBased 5 0 R]", &[icc], Family::ICCBased, 3),
        (
            "[/Indexed /DeviceRGB 1 <000000FFFFFF>]",
            &[],
            Family::Indexed,
            1,
        ),
        (
            "[/Indexed 5 0 R 0 6 0 R]",
            &["/DeviceGray", "<< /Length 0 >>"],
            Family::Indexed,
            1,
        ),
        (
            "[/Indexed [/Separation /A /DeviceGray 5 0 R] 1 <00FF>]",
            &[TINT_1],
            Family::Indexed,
            1,
        ),
        (
            "[/Separation /Gold /DeviceCMYK 5 0 R]",
            &[TINT_4],
            Family::Separation,
            1,
        ),
        (
            "[/Separation /All /DeviceGray 5 0 R]",
            &[TINT_1],
            Family::Separation,
            1,
        ),
        (
            "[/Separation /None /DeviceGray 5 0 R]",
            &[TINT_1],
            Family::Separation,
            1,
        ),
        (
            "[/DeviceN [/A /B] /DeviceGray 5 0 R]",
            &[TINT_2_1],
            Family::DeviceN,
            2,
        ),
        (
            "[/DeviceN [/A /None /None] /DeviceGray 5 0 R]",
            &["<< /Length 0 /FunctionType 4 /Domain [0 1 0 1 0 1] /Range [0 1] >>"],
            Family::DeviceN,
            3,
        ),
        (
            "[/Pattern [/DeviceN [/A /B] /DeviceGray 5 0 R]]",
            &[TINT_2_1],
            Family::Pattern,
            3,
        ),
        (NCHANNEL, &[TO_CMYK], Family::DeviceN, 2),
    ];
    for (text, extra, family, count) in cases {
        let (space, faults) = read(text, extra);
        let space = space.unwrap_or_else(|| panic!("{text}: no space"));
        assert_eq!(faults, [], "{text}");
        assert_eq!(space.family(), *family, "{text}");
        assert_eq!(space.components(), Some(*count), "{text}");
        let file = file(text, extra);
        let resolve = |id: ObjectId| file.objects.get(&id);
        let counted = components(resolve, file.value, &Limits::DEFAULT);
        assert_eq!(counted, Ok(Some(*count)), "{text}");
    }
}

#[test]
fn each_fault_by_its_clause() {
    let tint = "<< /FunctionType 2 /Domain [0 1] /N 1 >>";
    let cases: &[(&str, &[&str], Fault)] = &[
        // 8.6.3.
        ("7", &[], Fault::NotASpace),
        ("<< >>", &[], Fault::NotASpace),
        ("[]", &[], Fault::NotASpace),
        ("[7]", &[], Fault::NotASpace),
        ("[(CalGray) << >>]", &[], Fault::NotASpace),
        ("[[/DeviceRGB]]", &[], Fault::NotASpace),
        ("9 0 R", &[], Fault::NotASpace),
        ("5 0 R", &["<< /Length 0 >>"], Fault::NotASpace),
        ("/Foo", &[], Fault::UnknownFamily),
        ("[/CalGrayX << >>]", &[], Fault::UnknownFamily),
        ("[/CalCMYK << >>]", &[], Fault::UnknownFamily),
        ("[/DeviceGray 1]", &[], Fault::ExtraElements),
        ("[/CalGray << >> 1]", &[], Fault::ExtraElements),
        ("[/Pattern /DeviceGray 1]", &[], Fault::ExtraElements),
        ("[/Indexed /G 0 <00> 1]", &[], Fault::ExtraElements),
        ("[/Separation /A /G 5 0 R 1]", &[tint], Fault::ExtraElements),
        (
            "[/DeviceN [/A] /G 5 0 R << >> 1]",
            &[tint],
            Fault::ExtraElements,
        ),
        // 8.9.7.
        ("/G", &[], Fault::Abbreviated),
        ("/RGB", &[], Fault::Abbreviated),
        ("/CMYK", &[], Fault::Abbreviated),
        ("[/I /G 0 <00>]", &[], Fault::Abbreviated),
        ("[/Pattern /G]", &[], Fault::Abbreviated),
        // Each family's parameters.
        ("/CalGray", &[], Fault::Missing { element: 1 }),
        ("[/ICCBased]", &[], Fault::Missing { element: 1 }),
        ("/Indexed", &[], Fault::Missing { element: 1 }),
        ("[/Indexed /G]", &[], Fault::Missing { element: 2 }),
        ("[/Indexed /G 0]", &[], Fault::Missing { element: 3 }),
        ("[/Separation]", &[], Fault::Missing { element: 1 }),
        ("[/Separation /A]", &[], Fault::Missing { element: 2 }),
        ("[/Separation /A /G]", &[], Fault::Missing { element: 3 }),
        ("[/DeviceN]", &[], Fault::Missing { element: 1 }),
        ("[/DeviceN [/A]]", &[], Fault::Missing { element: 2 }),
        ("[/DeviceN [/A] /G]", &[], Fault::Missing { element: 3 }),
        // Tables 62 to 64.
        ("[/CalGray << >>]", &[], Fault::WhitePoint),
        ("[/CalRGB << /WhitePoint 7 >>]", &[], Fault::WhitePoint),
        ("[/Lab << /WhitePoint [1 1] >>]", &[], Fault::WhitePoint),
        ("[/Lab << /WhitePoint [1 1 1 1] >>]", &[], Fault::WhitePoint),
        (
            "[/CalGray << /WhitePoint [1 true 1] >>]",
            &[],
            Fault::WhitePoint,
        ),
        (
            "[/CalGray << /WhitePoint [0 1 1] >>]",
            &[],
            Fault::WhitePoint,
        ),
        (
            "[/CalGray << /WhitePoint [1 1 -1] >>]",
            &[],
            Fault::WhitePoint,
        ),
        (
            "[/CalGray << /WhitePoint [1 0.5 1] >>]",
            &[],
            Fault::WhitePoint,
        ),
        (
            "[/CalGray << /WhitePoint [1 1.0000001 1] >>]",
            &[],
            Fault::WhitePoint,
        ),
        // Above 0, but not as PDFium's 32-bit float.
        (
            "[/CalGray << /WhitePoint [0.00000000000000000000000000000000000000000000000000001 1 1] >>]",
            &[],
            Fault::WhitePoint,
        ),
        (
            "[/CalGray << /WhitePoint [99999999999 1 1] >>]",
            &[],
            Fault::WhitePoint,
        ),
        (
            "[/CalGray << /WhitePoint [1 1 1] /BlackPoint [0 0] >>]",
            &[],
            Fault::BlackPoint,
        ),
        (
            "[/Lab << /WhitePoint [1 1 1] /BlackPoint [0 -1 0] >>]",
            &[],
            Fault::BlackPoint,
        ),
        (
            "[/CalRGB << /WhitePoint [1 1 1] /BlackPoint 7 >>]",
            &[],
            Fault::BlackPoint,
        ),
        (
            "[/CalGray << /WhitePoint [1 1 1] /Gamma 0 >>]",
            &[],
            Fault::Gamma,
        ),
        (
            "[/CalGray << /WhitePoint [1 1 1] /Gamma [1] >>]",
            &[],
            Fault::Gamma,
        ),
        (
            "[/CalRGB << /WhitePoint [1 1 1] /Gamma 2 >>]",
            &[],
            Fault::Gamma,
        ),
        (
            "[/CalRGB << /WhitePoint [1 1 1] /Matrix [1 0 0] >>]",
            &[],
            Fault::Matrix,
        ),
        (
            "[/Lab << /WhitePoint [1 1 1] /Range [0 1 0] >>]",
            &[],
            Fault::LabRange,
        ),
        (
            "[/Lab << /WhitePoint [1 1 1] /Range [1 0 0 1] >>]",
            &[],
            Fault::LabRange,
        ),
        (
            "[/Lab << /WhitePoint [1 1 1] /Range [0 1 1 0] >>]",
            &[],
            Fault::LabRange,
        ),
        // Table 65.
        ("[/ICCBased 7]", &[], Fault::IccNotStream),
        ("[/ICCBased << /N 3 >>]", &[], Fault::IccNotStream),
        ("[/ICCBased 9 0 R]", &[], Fault::IccNotStream),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 >>"],
            Fault::IccComponents,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N 2 >>"],
            Fault::IccComponents,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N 3.0 >>"],
            Fault::IccComponents,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N true >>"],
            Fault::IccComponents,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N 3 /Alternate /Pattern >>"],
            Fault::IccAlternate,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N 3 /Alternate /DeviceGray >>"],
            Fault::IccAlternate,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N 3 /Range [0 1] >>"],
            Fault::IccRange,
        ),
        // 8.6.6.3.
        ("[/Indexed /Pattern 0 <00>]", &[], Fault::BaseSpecial),
        (
            "[/Indexed [/Indexed /G 0 <00>] 0 <00>]",
            &[],
            Fault::BaseSpecial,
        ),
        ("[/Indexed /G 256 <00>]", &[], Fault::Hival),
        ("[/Indexed /G -1 <00>]", &[], Fault::Hival),
        ("[/Indexed /G 1.0 <00>]", &[], Fault::Hival),
        ("[/Indexed /G (1) <00>]", &[], Fault::Hival),
        ("[/Indexed /G 0 7]", &[], Fault::LookupKind),
        ("[/Indexed /G 0 << >>]", &[], Fault::LookupKind),
        ("[/Indexed /G 0 9 0 R]", &[], Fault::LookupKind),
        (
            "[/Indexed /DeviceRGB 1 <0000000000>]",
            &[],
            Fault::LookupLength,
        ),
        (
            "[/Indexed /DeviceRGB 1 <00000000000000>]",
            &[],
            Fault::LookupLength,
        ),
        (
            "[/Indexed /DeviceCMYK 0 <000000>]",
            &[],
            Fault::LookupLength,
        ),
        // 8.6.6.4, 8.6.6.5.
        (
            "[/Separation (None) /G 5 0 R]",
            &[tint],
            Fault::ColorantName,
        ),
        ("[/Separation 7 /G 5 0 R]", &[tint], Fault::ColorantName),
        ("[/DeviceN [/A 7] /G 5 0 R]", &[tint], Fault::ColorantName),
        (
            "[/DeviceN [/A 9 0 R] /G 5 0 R]",
            &[tint],
            Fault::ColorantName,
        ),
        (
            "[/Separation /A /Pattern 5 0 R]",
            &[tint],
            Fault::AlternateSpecial,
        ),
        (
            "[/Separation /A [/Indexed /G 0 <00>] 5 0 R]",
            &[tint],
            Fault::AlternateSpecial,
        ),
        (
            "[/DeviceN [/A] [/Separation /B /G 5 0 R] 5 0 R]",
            &[tint],
            Fault::AlternateSpecial,
        ),
        ("[/Separation /A /G 7]", &[], Fault::TintNotFunction),
        ("[/Separation /A /G 9 0 R]", &[], Fault::TintNotFunction),
        ("[/DeviceN [/A] /G /Identity]", &[], Fault::TintNotFunction),
        ("[/Separation /A /G << >>]", &[], Fault::TintShape),
        (
            "[/Separation /A /DeviceCMYK 5 0 R]",
            &[tint],
            Fault::TintShape,
        ),
        ("[/DeviceN [/A /B] /G 5 0 R]", &[tint], Fault::TintShape),
        ("[/DeviceN 7 /G 5 0 R]", &[tint], Fault::NamesNotArray),
        ("[/DeviceN /A /G 5 0 R]", &[tint], Fault::NamesNotArray),
        ("[/DeviceN 9 0 R /G 5 0 R]", &[tint], Fault::NamesNotArray),
        ("[/DeviceN [] /G 5 0 R]", &[tint], Fault::NamesEmpty),
        (
            "[/DeviceN [/A /A] /G 5 0 R]",
            &[tint],
            Fault::ColorantRepeated,
        ),
        ("[/DeviceN [/All] /G 5 0 R]", &[tint], Fault::AllInDeviceN),
        (
            "[/DeviceN [/None] /G 5 0 R << /Subtype /NChannel >>]",
            &[tint],
            Fault::NoneInNChannel,
        ),
        // Tables 70 and 71.
        (
            "[/DeviceN [/A] /G 5 0 R << /Subtype /X >>]",
            &[tint],
            Fault::AttributesSubtype,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Subtype 7 >>]",
            &[tint],
            Fault::AttributesSubtype,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Colorants << /A /DeviceGray >> >>]",
            &[tint],
            Fault::ColorantNotSeparation,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Colorants << /A [/Separation /B /G 5 0 R] >> >>]",
            &[tint],
            Fault::ColorantNotSeparation,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Subtype /NChannel >>]",
            &[tint],
            Fault::ColorantUndefined,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Process << /Components [/A] >> >>]",
            &[tint],
            Fault::ProcessSpace,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Process << /ColorSpace [/Lab << /WhitePoint [1 1 1] >>] /Components [/A /B /C] >> >>]",
            &[tint],
            Fault::ProcessSpace,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Process << /ColorSpace /Pattern /Components [/A] >> >>]",
            &[tint],
            Fault::ProcessSpace,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Process << /ColorSpace /DeviceGray >> >>]",
            &[tint],
            Fault::ProcessComponents,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Process << /ColorSpace /DeviceGray /Components 7 >> >>]",
            &[tint],
            Fault::ProcessComponents,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Process << /ColorSpace /DeviceRGB /Components [/A] >> >>]",
            &[tint],
            Fault::ProcessComponents,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Process << /ColorSpace /DeviceGray /Components [7] >> >>]",
            &[tint],
            Fault::ProcessComponents,
        ),
        // 8.7.3.3.
        ("[/Pattern /Pattern]", &[], Fault::UnderlyingPattern),
        ("[/Pattern [/Pattern /G]]", &[], Fault::UnderlyingPattern),
        // A parameter that's no dictionary.
        ("[/CalGray 7]", &[], Fault::NotADictionary),
        ("[/CalGray 9 0 R]", &[], Fault::NotADictionary),
        (
            "[/CalGray 5 0 R]",
            &["<< /Length 0 /WhitePoint [1 1 1] >>"],
            Fault::NotADictionary,
        ),
        ("[/DeviceN [/A] /G 5 0 R 7]", &[tint], Fault::NotADictionary),
        (
            "[/DeviceN [/A] /G 5 0 R << /Colorants 7 >>]",
            &[tint],
            Fault::NotADictionary,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /Process 7 >>]",
            &[tint],
            Fault::NotADictionary,
        ),
        (
            "[/DeviceN [/A] /G 5 0 R << /MixingHints 7 >>]",
            &[tint],
            Fault::NotADictionary,
        ),
        // A reference to a reference.
        ("5 0 R", &["6 0 R", "/DeviceGray"], Fault::Chained),
        (
            "[/Indexed 5 0 R 0 <00>]",
            &["6 0 R", "/DeviceGray"],
            Fault::Chained,
        ),
        (
            "[/Pattern 5 0 R]",
            &["6 0 R", "/DeviceGray"],
            Fault::Chained,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N 6 0 R >>", "7 0 R", "3"],
            Fault::Chained,
        ),
        ("[/CalGray 5 0 R]", &["6 0 R", CAL], Fault::Chained),
        (
            "[/CalGray << /WhitePoint 5 0 R >>]",
            &["6 0 R", "[1 1 1]"],
            Fault::Chained,
        ),
        (
            "[/DeviceN [5 0 R] /G 7 0 R]",
            &["6 0 R", "/A", tint],
            Fault::Chained,
        ),
        ("[5 0 R]", &["6 0 R", "/DeviceGray"], Fault::Chained),
        (
            "[/Separation /A /G 5 0 R]",
            &["6 0 R", tint],
            Fault::Chained,
        ),
        // Caps and loops.
        ("5 0 R", &["5 0 R"], Fault::Loop),
        ("5 0 R", &["6 0 R", "5 0 R"], Fault::Loop),
        ("5 0 R", &["[/Pattern 5 0 R]"], Fault::Loop),
        ("5 0 R", &["[/Indexed 5 0 R 0 <00>]"], Fault::Loop),
        (
            "5 0 R",
            &["[/Separation /A 5 0 R 6 0 R]", tint],
            Fault::Loop,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N 1 /Alternate [/ICCBased 5 0 R] >>"],
            Fault::Loop,
        ),
    ];
    for (text, extra, fault) in cases {
        let (_, faults) = read(text, extra);
        assert!(faults.contains(fault), "{text}: {faults:?}, not {fault:?}");
        // Each says what it is, whole.
        assert!(!fault.to_string().is_empty());
    }
}

/// What it reads where it's the reader's own cap, or where there's a damaged
/// object: each told by what it is.
#[test]
fn caps_and_damage_are_told() {
    let mut limits = Limits::DEFAULT;
    limits.space_depth = 1;
    let deep = file("[/Pattern [/Pattern [/Pattern /DeviceGray]]]", &[]);
    assert!(read_under(deep, &limits).1.contains(&Fault::TooDeep));

    // A chain as long as the depth, and one past it.
    let chain = file("5 0 R", &["6 0 R", "7 0 R", "/DeviceGray"]);
    limits.space_depth = 2;
    assert_eq!(read_under(chain, &limits).0, Some(Space::DeviceGray));
    limits.space_depth = 1;
    let (space, faults) = read_under(chain, &limits);
    assert_eq!(space, None);
    assert!(faults.contains(&Fault::TooDeep), "{faults:?}");

    let mut limits = Limits::DEFAULT;
    limits.space_visits = 1;
    let (space, faults) = read_under(chain, &limits);
    assert_eq!(space, None);
    assert_eq!(faults, [Fault::Chained, Fault::TooManyVisits]);

    // Told once, however many are stopped by it.
    let names = file(
        "[/DeviceN [5 0 R 5 0 R 5 0 R] /DeviceGray 6 0 R]",
        &["/A", TINT_1],
    );
    let (_, faults) = read_under(names, &limits);
    let told = faults.iter().filter(|f| **f == Fault::TooManyVisits);
    assert_eq!(told.count(), 1, "{faults:?}");

    // A function's walk is on the checks' budget, and told.
    let shared = file(
        "[/Separation /A /DeviceGray 5 0 R]",
        &[
            "<< /FunctionType 3 /Domain [0 1] /Functions [6 0 R 6 0 R] /Bounds [0.5] /Encode [0 1 0 1] >>",
            TINT_1,
        ],
    );
    limits.space_visits = 2;
    let (space, faults) = read_under(shared, &limits);
    assert_eq!(faults, [Fault::TooManyVisits]);
    assert_eq!(space.unwrap().family(), Family::Separation);
    let mut limits = Limits::DEFAULT;
    limits.space_depth = 0;
    let (_, faults) = read_under(shared, &limits);
    assert!(faults.contains(&Fault::TooDeep), "{faults:?}");
}

#[test]
fn damaged_is_told_as_nothing_there() {
    let file = file("[/Pattern 5 0 R]", &[]);
    let (asked, failing) = (RefCell::new(Vec::new()), [(5, Why::Damaged)]);
    let resolver = Asked {
        file,
        asked: &asked,
        failing: &failing,
    };
    let mut kept = Vec::new();
    let space = Space::read(resolver, file.value, &Limits::DEFAULT, &mut kept).unwrap();
    assert_eq!(space, Some(Space::Pattern(None)));
    let faults: Vec<Fault> = kept.into_iter().map(|(_, fault)| fault).collect();
    assert_eq!(
        faults,
        [Fault::Damaged {
            id: ObjectId::new(5, 0)
        }]
    );
}

/// What's not a space's to hold, a name alone or an empty array, holds nothing,
/// and what's told about the rest of it goes on being told.
#[test]
fn reading_goes_on_past_what_is_wrong() {
    let (space, faults) = read("/CalGray", &[]);
    let Some(Space::CalGray(cie)) = space else {
        panic!("not CalGray");
    };
    assert_eq!((cie.dict, cie.white_point), (None, None));
    assert_eq!(faults, [Fault::Missing { element: 1 }]);

    let (space, _) = read("[/CalGray << /WhitePoint [0 7 -1] >>]", &[]);
    let Some(Space::CalGray(cie)) = space else {
        panic!("not CalGray");
    };
    assert_eq!(cie.white_point, Some([0.0, 7.0, -1.0]));

    let (space, faults) = read("[/Indexed /Foo 999 7]", &[]);
    let Some(Space::Indexed(indexed)) = space else {
        panic!("not Indexed");
    };
    assert_eq!(
        (indexed.base, indexed.hival, indexed.lookup),
        (None, None, None)
    );
    assert_eq!(
        faults,
        [Fault::UnknownFamily, Fault::Hival, Fault::LookupKind]
    );

    // A stream's dictionary is held where PDFium reads it.
    let (space, _) = read("[/Lab 5 0 R]", &["<< /Length 0 /WhitePoint [1 1 1] >>"]);
    let Some(Space::Lab(cie)) = space else {
        panic!("not Lab");
    };
    assert_eq!(cie.white_point, Some([1.0, 1.0, 1.0]));
}

/// A fault in a space another holds is told, whichever holds it.
#[test]
fn a_fault_goes_on_past_a_nested_space() {
    let bad = "[/CalGray << >>]";
    let tint = TINT_1;
    let holders = [
        format!("[/Pattern {bad}]"),
        format!("[/Indexed {bad} 0 <00>]"),
        format!("[/Separation /A {bad} 5 0 R]"),
        format!("[/DeviceN [/A] {bad} 5 0 R]"),
        format!("[/DeviceN [/A] /G 5 0 R << /Process << /ColorSpace {bad} /Components [/A] >> >>]"),
        format!("[/DeviceN [/A] /G 5 0 R << /Colorants << /A [/Separation /A {bad} 5 0 R] >> >>]"),
    ];
    for holder in &holders {
        let (_, faults) = read(holder, &[tint]);
        assert!(faults.contains(&Fault::WhitePoint), "{holder}: {faults:?}");
    }
    let icc = format!("<< /Length 0 /N 1 /Alternate {bad} >>");
    let (_, faults) = read("[/ICCBased 5 0 R]", &[&icc]);
    assert!(faults.contains(&Fault::WhitePoint), "{faults:?}");
}

#[test]
fn a_colorant_by_reference_is_a_colorant() {
    let (space, faults) = read("[/DeviceN [5 0 R /B] /DeviceGray 6 0 R]", &["/A", TINT_2_1]);
    let space = space.unwrap();
    assert_eq!(faults, []);
    assert_eq!(space.colorants().collect::<Vec<_>>(), [b"A", b"B"]);
    assert_eq!(space.colorants().count(), 2);
    assert_eq!(space.components(), Some(2));

    let (space, _) = read("[/Separation 5 0 R /DeviceGray 6 0 R]", &["/A", TINT_1]);
    assert_eq!(space.unwrap().colorants().collect::<Vec<_>>(), [b"A"]);

    // An element that's no name has no name, and is counted, as PDFium counts it.
    let (space, faults) = read("[/DeviceN [/A 7 9 0 R] /DeviceGray 5 0 R]", &[TINT_1]);
    let space = space.unwrap();
    assert_eq!(space.colorants().collect::<Vec<_>>(), [b"A"]);
    assert_eq!(space.components(), Some(3));
    let named = faults.iter().filter(|f| **f == Fault::ColorantName);
    assert_eq!(named.count(), 2);

    // None for any other family.
    let (space, _) = read("/DeviceGray", &[]);
    assert_eq!(space.unwrap().colorants().next(), None);
}

#[test]
fn a_name_says_what_colorant_it_is() {
    let cases: [(&[u8], Colorant, bool); 8] = [
        (b"Cyan", Colorant::Cyan, true),
        (b"Magenta", Colorant::Magenta, true),
        (b"Yellow", Colorant::Yellow, true),
        (b"Black", Colorant::Black, true),
        (b"All", Colorant::All, false),
        (b"None", Colorant::None, false),
        (b"PANTONE 131", Colorant::Spot, false),
        (b"cyan", Colorant::Spot, false),
    ];
    for (name, colorant, process) in cases {
        assert_eq!(Colorant::of(name), colorant);
        assert_eq!(colorant.is_process(), process);
    }
}

#[test]
fn a_family_says_what_it_is() {
    let all = [
        (Family::DeviceGray, (true, false, false)),
        (Family::DeviceRGB, (true, false, false)),
        (Family::DeviceCMYK, (true, false, false)),
        (Family::CalGray, (false, true, false)),
        (Family::CalRGB, (false, true, false)),
        (Family::Lab, (false, true, false)),
        (Family::ICCBased, (false, true, false)),
        (Family::Indexed, (false, false, true)),
        (Family::Pattern, (false, false, true)),
        (Family::Separation, (false, false, true)),
        (Family::DeviceN, (false, false, true)),
    ];
    for (family, (device, cie, special)) in all {
        assert_eq!(Family::of(family.name().as_bytes()), Some(family));
        assert_eq!(family.is_device(), device);
        assert_eq!(family.is_cie(), cie);
        assert_eq!(family.is_special(), special);
    }
    for name in ["I", "G", "RGB", "CMYK", "CalCMYK", "Devi", ""] {
        assert_eq!(Family::of(name.as_bytes()), None, "{name}");
    }
}

#[test]
fn what_a_space_holds_is_read_through_its_references() {
    let (space, faults) = read(
        "[/Indexed 5 0 R 6 0 R 7 0 R]",
        &["/DeviceRGB", "1", "<000000FFFFFF>"],
    );
    assert_eq!(faults, []);
    let space = space.unwrap();
    assert_eq!(space.base(), Some(&Space::DeviceRGB));
    assert_eq!(space.alternate(), None);
    let Space::Indexed(indexed) = &space else {
        panic!("not Indexed");
    };
    assert_eq!(indexed.hival, Some(1));
    let Some(Lookup::String(table)) = indexed.lookup else {
        panic!("no string");
    };
    assert_eq!(table.as_bytes().map(<[u8]>::len), Some(6));

    let (space, faults) = read(
        "[/ICCBased 5 0 R]",
        &[
            "<< /Length 0 /N 6 0 R /Alternate 7 0 R >>",
            "4",
            "/DeviceCMYK",
        ],
    );
    assert_eq!(faults, []);
    let space = space.unwrap();
    assert_eq!(space.components(), Some(4));
    assert_eq!(space.alternate(), Some(&Space::DeviceCMYK));
    let Space::ICCBased(icc) = &space else {
        panic!("not ICCBased");
    };
    assert!(icc.stream.is_some());

    let (space, faults) = read("[/Pattern 5 0 R]", &["/DeviceCMYK"]);
    assert_eq!(faults, []);
    assert_eq!(space.unwrap().base(), Some(&Space::DeviceCMYK));

    let tint = TINT_1;
    let (space, faults) = read("[/Separation /A 5 0 R 6 0 R]", &["/DeviceGray", tint]);
    assert_eq!(faults, []);
    let space = space.unwrap();
    assert_eq!(space.alternate(), Some(&Space::DeviceGray));
    let Space::Separation(separation) = &space else {
        panic!("not Separation");
    };
    assert_eq!(
        separation.tint.and_then(Object::as_reference),
        Some(ObjectId::new(6, 0))
    );

    // The attributes, each through its reference.
    let (space, faults) = read(
        "[/DeviceN [/Cyan /Spot] /DeviceGray 5 0 R 6 0 R]",
        &[
            TINT_2_1,
            "<< /Subtype 7 0 R /Colorants 8 0 R /Process 9 0 R /MixingHints 10 0 R >>",
            "/NChannel",
            "<< /Spot 11 0 R >>",
            "<< /ColorSpace 12 0 R /Components 13 0 R >>",
            "<< >>",
            "[/Separation /Spot /DeviceGray 14 0 R]",
            "/DeviceCMYK",
            "[15 0 R /Magenta /Yellow /Black]",
            TINT_1,
            "/Cyan",
        ],
    );
    assert_eq!(faults, []);
    let space = space.unwrap();
    let Space::DeviceN(device_n) = &space else {
        panic!("not DeviceN");
    };
    let attributes = device_n.attributes.as_ref().unwrap();
    assert_eq!(attributes.subtype, Some(Subtype::NChannel));
    assert!(attributes.colorants.is_some_and(|c| c.has(b"Spot")));
    assert!(attributes.mixing_hints.is_some());
    let process = attributes.process.as_ref().unwrap();
    assert_eq!(process.space.as_deref(), Some(&Space::DeviceCMYK));
    assert_eq!(process.components[0], Some(&b"Cyan"[..]));
    assert_eq!(process.components.len(), 4);
}

/// What `()` looks up is what a [`Space`] holds, and what only a check reads is a
/// sink's to pay for.
#[test]
fn nothing_only_a_check_reads_is_looked_up_for_unit() {
    type Case<'c> = (&'c str, &'c [&'c str], &'c [u32], &'c [u32]);
    let cases: &[Case] = &[
        (
            "[/CalRGB 5 0 R]",
            &[
                "<< /WhitePoint 6 0 R /BlackPoint 7 0 R /Gamma 8 0 R /Matrix 9 0 R >>",
                "[1 1 1]",
                "[0 0 0]",
                "[1 1 1]",
                "[1 0 0 0 1 0 0 0 1]",
            ],
            &[5, 6],
            &[5, 6, 7, 8, 9],
        ),
        (
            "[/CalGray 5 0 R]",
            &["<< /WhitePoint [6 0 R 1 1] /Gamma 7 0 R >>", "1", "2"],
            &[5, 6],
            &[5, 6, 7],
        ),
        (
            "[/Lab 5 0 R]",
            &["<< /WhitePoint [1 1 1] /Range 6 0 R >>", "[0 1 0 1]"],
            &[5],
            &[5, 6],
        ),
        (
            "[/ICCBased 5 0 R]",
            &[
                "<< /Length 0 /N 6 0 R /Alternate 7 0 R /Range 8 0 R >>",
                "1",
                "/DeviceGray",
                "[0 1]",
            ],
            &[5, 6, 7],
            &[5, 6, 7, 8],
        ),
        (
            "[/Indexed 5 0 R 6 0 R 7 0 R]",
            &["/DeviceGray", "0", "<00>"],
            &[5, 6, 7],
            &[5, 6, 7],
        ),
        (
            "[/Separation 5 0 R 6 0 R 7 0 R]",
            &["/A", "/DeviceGray", TINT_1],
            &[5, 6],
            &[5, 6, 7],
        ),
        (
            "[/DeviceN 5 0 R 6 0 R 7 0 R 8 0 R]",
            &[
                "[9 0 R]",
                "/DeviceGray",
                TINT_1,
                "<< /Colorants 10 0 R /Process 11 0 R /MixingHints 12 0 R >>",
                "/A",
                "<< /A 13 0 R >>",
                "<< /ColorSpace 14 0 R /Components 15 0 R >>",
                "<< >>",
                "[/Separation /A /DeviceGray 16 0 R]",
                "/DeviceGray",
                "[/A]",
                TINT_1,
            ],
            &[5, 9, 6, 8, 10, 12, 11, 14, 15],
            &[5, 9, 6, 7, 8, 10, 12, 11, 14, 15, 13, 16],
        ),
    ];
    for (text, extra, plain, wanting) in cases {
        let (by_unit, by_sink) = lookups(file(text, extra));
        assert_eq!(&by_unit, plain, "{text}: by ()");
        assert_eq!(&by_sink, wanting, "{text}: by a sink");
    }
}

/// A read that fails in a check isn't an `Err`, under any sink; one that fails in
/// what a space holds is, under every sink; a damaged one is told only to a sink
/// that wants it.
#[test]
fn a_failing_read_in_a_check_reads_alike_under_every_sink() {
    let tint_ref =
        "<< /FunctionType 3 /Domain [0 1] /Functions [6 0 R] /Bounds [] /Encode [0 1] >>";
    let cases: &[(&str, &[&str], u32)] = &[
        (
            "[/CalGray << /WhitePoint [1 1 1] /BlackPoint 5 0 R >>]",
            &["[0 0 0]"],
            5,
        ),
        (
            "[/CalGray << /WhitePoint [1 1 1] /Gamma 5 0 R >>]",
            &["2"],
            5,
        ),
        (
            "[/CalRGB << /WhitePoint [1 1 1] /Matrix 5 0 R >>]",
            &["[1 0 0 0 1 0 0 0 1]"],
            5,
        ),
        (
            "[/Lab << /WhitePoint [1 1 1] /Range 5 0 R >>]",
            &["[0 1 0 1]"],
            5,
        ),
        (
            "[/ICCBased 5 0 R]",
            &["<< /Length 0 /N 1 /Range 6 0 R >>", "[0 1]"],
            6,
        ),
        ("[/Separation /A /DeviceGray 5 0 R]", &[TINT_1], 5),
        ("[/Separation /A /DeviceGray 5 0 R]", &[tint_ref, TINT_1], 6),
        (
            "[/DeviceN [/A] /DeviceGray 5 0 R << /Colorants << /A 6 0 R >> >>]",
            &[TINT_1, "[/Separation /A /DeviceGray 5 0 R]"],
            6,
        ),
    ];
    for (text, extra, failing) in cases {
        let file = file(text, extra);
        let plain = lookups(file).0;
        assert!(
            !plain.contains(failing),
            "{text}: {failing} isn't a check's"
        );
        for why in [Why::Limit, Why::Locked, Why::Source, Why::Damaged] {
            let asked = RefCell::new(Vec::new());
            let fails = [(*failing, why)];
            let resolver = Asked {
                file,
                asked: &asked,
                failing: &fails,
            };
            let limits = &Limits::DEFAULT;
            let by_unit = Space::read(resolver, file.value, limits, &mut ());
            let by_none = Space::read(resolver, file.value, limits, &mut Unwanted);
            let mut kept = Vec::new();
            let by_sink = Space::read(resolver, file.value, limits, &mut kept);
            assert!(by_unit.is_ok(), "{text}: {by_unit:?}");
            assert_eq!(by_none, by_unit, "{text}");
            assert_eq!(by_sink, by_unit, "{text}");
            // Said only if it's damaged, and then as damaged.
            let id = ObjectId::new(*failing, 0);
            let said = kept.iter().any(|(_, f)| *f == Fault::Damaged { id });
            assert_eq!(said, why == Why::Damaged, "{text} by {why:?}: {kept:?}");
            if why != Why::Damaged {
                assert_eq!(kept.len(), 0, "{text} by {why:?}: {kept:?}");
            }
        }
    }

    // What a space holds fails the same under every sink.
    let file = file("[/CalGray 5 0 R]", &["<< /WhitePoint 6 0 R >>", "[1 1 1]"]);
    for failing in [5, 6] {
        for why in [Why::Limit, Why::Locked, Why::Source] {
            let asked = RefCell::new(Vec::new());
            let fails = [(failing, why)];
            let resolver = Asked {
                file,
                asked: &asked,
                failing: &fails,
            };
            let limits = &Limits::DEFAULT;
            let by_unit = Space::read(resolver, file.value, limits, &mut ());
            let error = by_unit.clone().unwrap_err();
            assert_eq!((error.id, error.why), (ObjectId::new(failing, 0), why));
            let by_none = Space::read(resolver, file.value, limits, &mut Unwanted);
            let mut kept = Vec::new();
            let by_sink = Space::read(resolver, file.value, limits, &mut kept);
            assert_eq!(by_none, by_unit);
            assert_eq!(by_sink, by_unit);
        }
    }
}

/// A check's lookups come off a budget of their own, so a sink that wants faults
/// reads what `()` reads under the smallest cap.
#[test]
fn a_sink_spends_what_it_reads_on_its_own_budget() {
    let file = file(
        "[/DeviceN [/A] /DeviceGray 5 0 R 6 0 R]",
        &[
            "<< /FunctionType 3 /Domain [0 1] /Functions [7 0 R 7 0 R 7 0 R] /Bounds [0.3 0.6] /Encode [0 1 0 1 0 1] >>",
            "<< /MixingHints 8 0 R >>",
            TINT_1,
            "<< >>",
        ],
    );
    let mut limits = Limits::DEFAULT;
    limits.space_visits = 3;
    let (space, faults) = read_under(file, &limits);
    let Some(Space::DeviceN(device_n)) = space else {
        panic!("not DeviceN");
    };
    assert!(device_n.attributes.unwrap().mixing_hints.is_some());
    assert_eq!(faults, [Fault::TooManyVisits], "the tint's, and once");
}

/// A shared array of names is spent by its elements, whether they're in place or
/// by reference, so what's kept can't grow as the square of the file.
#[test]
fn names_spend_visits_so_a_space_stays_small() {
    let many: Vec<String> = (0..64).map(|n| format!("/N{n}")).collect();
    let text = format!("[/DeviceN [{}] /DeviceGray 5 0 R]", many.join(" "));
    let mut limits = Limits::DEFAULT;
    limits.space_visits = 10;
    let (space, faults) = read_under(file(&text, &[TINT_1]), &limits);
    let Some(Space::DeviceN(device_n)) = space else {
        panic!("not DeviceN");
    };
    assert_eq!(device_n.names.len(), 10);
    assert_eq!(
        device_n.components,
        Some(64),
        "the array's, as PDFium counts"
    );
    assert!(faults.contains(&Fault::TooManyVisits), "{faults:?}");
}

#[test]
fn a_fault_says_where_it_is() {
    let file = file(
        "[/Pattern 5 0 R]",
        &["[/CalGray << /WhitePoint 6 0 R >>]", "7"],
    );
    let resolve = |id: ObjectId| file.objects.get(&id);
    let mut kept = Vec::new();
    Space::read(resolve, file.value, &Limits::DEFAULT, &mut kept).unwrap();
    let (place, fault) = kept[0];
    assert_eq!(fault, Fault::WhitePoint);
    assert_eq!(
        place.object,
        Some(ObjectId::new(6, 0)),
        "in what it's written in"
    );
    assert!(place.at.is_some());

    let direct = value("/Foo");
    let nothing = |_: ObjectId| None;
    let mut kept = Vec::new();
    Space::read(nothing, &direct, &Limits::DEFAULT, &mut kept).unwrap();
    assert_eq!(kept[0].0.object, None, "written in place");
    assert_eq!(kept[0].0, Place::new(None, direct.at));
}

/// A file of `objects`, the first of them object 5, its page's resources object
/// 3's, and where each object is written, by number.
fn laid_out(objects: &[&str]) -> (Vec<u8>, BTreeMap<u32, std::ops::Range<u32>>) {
    let head = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    ];
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] >>";
    let mut out = b"%PDF-1.7\n".to_vec();
    let (mut offsets, mut at) = (Vec::new(), BTreeMap::new());
    let bodies = head.iter().chain([&page, &"null"]).chain(objects);
    for (body, number) in bodies.zip(1..) {
        offsets.push(out.len());
        out.extend(format!("{number} 0 obj\n").as_bytes());
        let start = out.len() as u32;
        out.extend(body.as_bytes());
        at.insert(number, start..out.len() as u32);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let size = offsets.len() + 1;
    let trailer = format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n");
    out.extend(trailer.as_bytes());
    (out, at)
}

/// Each `(fault, object)` told of object 5, read as a space from a file of
/// `objects`, each span checked to be in the object its place names.
fn told_in(objects: &[&str]) -> Vec<(Fault, u32)> {
    let (bytes, at) = laid_out(objects);
    let reading = Reading::open(&bytes, &Options::DEFAULT).unwrap();
    let start = Object::reference(5);
    let space_at: &Object = Box::leak(Box::new(start));
    let mut faults = Vec::new();
    Space::read(&reading, space_at, &Limits::DEFAULT, &mut faults).unwrap();
    faults_placed(&faults, &at)
}

/// Each fault of `faults` and the object it's told in, each span in that object.
fn faults_placed(
    faults: &[(Place, Fault)],
    at: &BTreeMap<u32, std::ops::Range<u32>>,
) -> Vec<(Fault, u32)> {
    let placed = faults.iter().map(|(place, fault)| {
        let object = place
            .object
            .unwrap_or_else(|| panic!("{fault:?} in none"))
            .number;
        let span = place
            .at
            .unwrap_or_else(|| panic!("{fault:?} written nowhere"));
        let within = &at[&object];
        assert!(
            within.start <= span.start as u32 && span.end as u32 <= within.end,
            "{fault:?}: {span:?} isn't in object {object}, at {within:?}"
        );
        (*fault, object)
    });
    placed.collect()
}

#[test]
fn a_fault_of_what_a_reference_led_to_is_in_that_object() {
    let separation = "[/Separation 6 0 R /DeviceGray << /FunctionType 2 /Domain [0 1] /N 1 >>]";
    let cases: [(&[&str], (Fault, u32)); 11] = [
        (
            &[
                "[/ICCBased 6 0 R]",
                "<< /N 7 0 R /Length 0 >>\nstream\n\nendstream",
                "2",
            ],
            (Fault::IccComponents, 7),
        ),
        (
            &["[/Indexed /DeviceGray 6 0 R <00>]", "500"],
            (Fault::Hival, 6),
        ),
        (
            &["[/Indexed /DeviceGray 0 6 0 R]", "7"],
            (Fault::LookupKind, 6),
        ),
        (&["[6 0 R /DeviceGray]", "7"], (Fault::NotASpace, 6)),
        (&["6 0 R", "7"], (Fault::NotASpace, 6)),
        (&["6 0 R", "7 0 R", "/DeviceGray"], (Fault::Chained, 5)),
        (&[separation, "7"], (Fault::ColorantName, 6)),
        (
            &[
                "[/DeviceN [6 0 R] /DeviceGray << /FunctionType 2 /Domain [0 1] /N 1 >>]",
                "7",
            ],
            (Fault::ColorantName, 6),
        ),
        (
            &[
                "[/DeviceN [/A] /DeviceGray << /FunctionType 2 /Domain [0 1] /N 1 >> << /Subtype 6 0 R >>]",
                "7",
            ],
            (Fault::AttributesSubtype, 6),
        ),
        (
            &[
                "[/DeviceN [/A] /DeviceGray << /FunctionType 2 /Domain [0 1] /N 1 >> << /MixingHints 6 0 R >>]",
                "7",
            ],
            (Fault::NotADictionary, 6),
        ),
        (
            &[
                "[/DeviceN [/A] /DeviceGray << /FunctionType 2 /Domain [0 1] /N 1 >> << /Colorants 6 0 R >>]",
                "<< /A /DeviceGray >>",
            ],
            (Fault::ColorantNotSeparation, 6),
        ),
    ];
    for (objects, want) in cases {
        let told = told_in(objects);
        assert!(told.contains(&want), "{objects:?}: {told:?}");
    }

    // A reference to nothing is told of the reference, in what it's written in,
    // through a chain too.
    let told = told_in(&["[/Indexed /DeviceGray 6 0 R <00>]", "7 0 R", "99 0 R"]);
    assert!(told.contains(&(Fault::Chained, 6)), "{told:?}");
    assert!(told.contains(&(Fault::Hival, 5)), "{told:?}");
}

#[test]
fn separations_are_told_in_the_object_colorants_is_in() {
    let tint = "<< /FunctionType 2 /Domain [0 1] /N 1 >>";
    let in_place =
        format!("[/DeviceN [/A] /DeviceGray {tint} << /Colorants << /A /DeviceGray >> >>]");
    let by_reference = format!("[/DeviceN [/A] /DeviceGray {tint} << /Colorants 6 0 R >>]");
    let attributes = format!("[/DeviceN [/A] /DeviceGray {tint} 6 0 R]");
    let cases: [(&[&str], u32); 3] = [
        (&[&in_place], 5),
        (&[&by_reference, "<< /A /DeviceGray >>"], 6),
        (&[&attributes, "<< /Colorants << /A /DeviceGray >> >>"], 6),
    ];
    for (objects, want) in cases {
        let (bytes, at) = laid_out(objects);
        let reading = Reading::open(&bytes, &Options::DEFAULT).unwrap();
        let value: &Object = Box::leak(Box::new(Object::reference(5)));
        let mut faults = Vec::new();
        let space = Space::read(&reading, value, &Limits::DEFAULT, &mut faults).unwrap();
        let by_read = faults_placed(&faults, &at);
        assert_eq!(
            by_read,
            [(Fault::ColorantNotSeparation, want)],
            "{objects:?}"
        );

        let Some(Space::DeviceN(device_n)) = space else {
            panic!("not DeviceN");
        };
        let mut faults = Vec::new();
        let attributes = device_n.attributes.unwrap();
        attributes
            .separations(&reading, &Limits::DEFAULT, &mut faults)
            .unwrap();
        assert_eq!(faults_placed(&faults, &at), by_read, "{objects:?}");
    }
}

#[test]
fn a_stream_held_can_have_its_data_asked_for() {
    let objects = [
        "[/Indexed 6 0 R 0 7 0 R]",
        "[/ICCBased 8 0 R]",
        "9 0 R",
        "<< /N 1 /Length 1 >>\nstream\np\nendstream",
        "<< /Length 1 >>\nstream\nq\nendstream",
    ];
    let (bytes, _) = laid_out(&objects);
    let reading = Reading::open(&bytes, &Options::DEFAULT).unwrap();
    let value: &Object = Box::leak(Box::new(Object::reference(5)));
    let space = Space::read(&reading, value, &Limits::DEFAULT, &mut ());
    let Ok(Some(Space::Indexed(indexed))) = space else {
        panic!("not Indexed");
    };
    let Some(Lookup::Stream { stream, id, .. }) = indexed.lookup else {
        panic!("no stream");
    };
    assert_eq!(id, Some(ObjectId::new(9, 0)), "the object a chain ended in");
    assert_eq!(reading.decoded(id.unwrap()).unwrap(), b"q");
    assert!(stream.dict.has(b"Length"));
    let Some(Space::ICCBased(icc)) = indexed.base.as_deref() else {
        panic!("no ICCBased");
    };
    assert_eq!(icc.id, Some(ObjectId::new(8, 0)));
    assert_eq!(reading.decoded(icc.id.unwrap()).unwrap(), b"p");

    // A stream that has no number has none to say.
    let file = file("[/ICCBased 5 0 R]", &["<< /Length 0 /N 1 >>"]);
    let (space, _) = read_under(file, &Limits::DEFAULT);
    let Some(Space::ICCBased(icc)) = space else {
        panic!("not ICCBased");
    };
    assert_eq!(icc.id, Some(ObjectId::new(5, 0)));
    let built = Object::from(vec![
        Object::from("ICCBased"),
        Object::stream(pdf_objects::dict! { "N" => 1 }),
    ]);
    let built: &Object = Box::leak(Box::new(built));
    let nothing = |_: ObjectId| None;
    let space = Space::read(nothing, built, &Limits::DEFAULT, &mut ());
    let Ok(Some(Space::ICCBased(icc))) = space else {
        panic!("not ICCBased");
    };
    assert_eq!((icc.id, icc.n), (None, Some(1)));
}

#[test]
fn a_name_keeps_its_position_among_a_space_s_colorants() {
    let (space, faults) = read("[/DeviceN [/A 7 /B] /DeviceRGB 5 0 R]", &[TINT_1]);
    assert!(faults.contains(&Fault::ColorantName), "{faults:?}");
    let space = space.unwrap();
    let Space::DeviceN(device_n) = &space else {
        panic!("not DeviceN");
    };
    let held = [Some(&b"A"[..]), None, Some(&b"B"[..])];
    assert_eq!(device_n.names, held);
    assert_eq!(device_n.components, Some(3));
    let named: Vec<&[u8]> = space.colorants().collect();
    assert_eq!(named, [&b"A"[..], &b"B"[..]]);

    let attributes = "<< /Process << /ColorSpace /DeviceRGB /Components [/R 7 /B] >> >>";
    let text = format!("[/DeviceN [/R] /DeviceGray 5 0 R {attributes}]");
    let (space, _) = read(&text, &[TINT_1]);
    let Some(Space::DeviceN(device_n)) = space else {
        panic!("not DeviceN");
    };
    let process = device_n.attributes.unwrap().process.unwrap();
    assert_eq!(process.components, [Some(&b"R"[..]), None, Some(&b"B"[..])]);
}

/// A DeviceN space whose tint is `n` stitching functions by reference, each holding
/// the next, and then a function of one output.
fn stitched_by_reference(n: u32) -> File {
    let kids: Vec<String> = (0..n)
        .map(|at| {
            let kid = 6 + at;
            format!("<< /FunctionType 3 /Domain [0 1] /Functions [{kid} 0 R] /Bounds [] /Encode [0 1] >>")
        })
        .chain([TINT_1.to_string()])
        .collect();
    let kids: Vec<&str> = kids.iter().map(String::as_str).collect();
    file("[/DeviceN [/A] /DeviceGray 5 0 R]", &kids)
}

#[test]
fn a_tint_past_the_depth_is_told_where_components_meets_it() {
    let at = stitched_by_reference(63);
    let resolve = |id: ObjectId| at.objects.get(&id);
    let (space, faults) = read_under(at, &Limits::DEFAULT);
    assert_eq!(faults, []);
    let read = counted(resolve, at.value, &Limits::DEFAULT).unwrap();
    assert_eq!((read.count, read.capped), (Some(1), false));
    assert_eq!(space.unwrap().components(), Some(1));

    let past = stitched_by_reference(64);
    let resolve = |id: ObjectId| past.objects.get(&id);
    let (space, faults) = read_under(past, &Limits::DEFAULT);
    assert_eq!(faults, [Fault::TooDeep]);
    let read = counted(resolve, past.value, &Limits::DEFAULT).unwrap();
    assert_eq!((read.count, read.capped), (Some(3), true));
    assert_eq!(space.unwrap().components(), Some(1));
}

/// The promise's condition, as each reading spends its own: `components` spends one
/// budget on an alternate and a tint, where a typed space's checks have their own.
#[test]
fn each_reading_spends_its_own_visits() {
    let at = |kids: usize| {
        let functions = "7 0 R ".repeat(kids);
        let (bounds, encode) = ("0 ".repeat(kids - 1), "0 1 ".repeat(kids));
        let wide = format!(
            "<< /FunctionType 3 /Domain [0 1] /Functions [{functions}] /Bounds [{bounds}] /Encode [{encode}] >>"
        );
        file(
            "[/DeviceN [/A] 6 0 R 5 0 R]",
            &[&wide, "/DeviceGray", TINT_1],
        )
    };
    let under = at(4094);
    let resolve = |id: ObjectId| under.objects.get(&id);
    let read = counted(resolve, under.value, &Limits::DEFAULT).unwrap();
    assert_eq!((read.count, read.capped), (Some(1), false));
    let (_, faults) = read_under(under, &Limits::DEFAULT);
    assert_eq!(faults, []);

    let met = at(4095);
    let resolve = |id: ObjectId| met.objects.get(&id);
    let read = counted(resolve, met.value, &Limits::DEFAULT).unwrap();
    assert_eq!((read.count, read.capped), (Some(3), true));
    let (space, faults) = read_under(met, &Limits::DEFAULT);
    assert_eq!(faults, [], "no cap met by this one");
    assert_eq!(space.unwrap().components(), Some(1));

    let past = at(4096);
    let (_, faults) = read_under(past, &Limits::DEFAULT);
    assert_eq!(faults, [Fault::TooManyVisits]);
}

#[test]
fn separations_are_read_on_asking() {
    let text = "[/DeviceN [/A] /DeviceGray 5 0 R << /Colorants 6 0 R >>]";
    let extra = [
        TINT_1,
        "<< /A [/Separation /A /DeviceGray 5 0 R] /B 7 0 R /C /DeviceGray /A [/Separation /A /DeviceRGB 5 0 R] >>",
        "[/Separation /B /DeviceGray 5 0 R]",
    ];
    let file = file(text, &extra);
    let resolve = |id: ObjectId| file.objects.get(&id);
    let (space, _) = read_under(file, &Limits::DEFAULT);
    let Some(Space::DeviceN(device_n)) = space else {
        panic!("not DeviceN");
    };
    let attributes = device_n.attributes.unwrap();
    let mut kept = Vec::new();
    let read = attributes.separations(resolve, &Limits::DEFAULT, &mut kept);
    let read = read.unwrap();
    let keys: Vec<&[u8]> = read.iter().map(|entry| entry.name).collect();
    assert_eq!(keys, [&b"B"[..], b"C", b"A"], "by each key's last entry");
    assert_eq!(
        read[0].space.as_ref().map(Space::family),
        Some(Family::Separation)
    );
    assert_eq!(read[1].space, Some(Space::DeviceGray));
    let faults: Vec<Fault> = kept.into_iter().map(|(_, fault)| fault).collect();
    assert!(faults.contains(&Fault::ColorantNotSeparation), "{faults:?}");
    assert!(
        faults.contains(&Fault::TintShape),
        "the last A's: {faults:?}"
    );

    // One budget for the whole of it.
    let mut limits = Limits::DEFAULT;
    limits.space_visits = 1;
    let read = attributes.separations(resolve, &limits, &mut ()).unwrap();
    assert_eq!(read.len(), 1);

    // None written, none read; a failure is the `Err`.
    let asked = RefCell::new(Vec::new());
    let fails = [(7, Why::Source)];
    let resolver = Asked {
        file,
        asked: &asked,
        failing: &fails,
    };
    let error = attributes.separations(resolver, &Limits::DEFAULT, &mut ());
    assert_eq!(error.unwrap_err().why, Why::Source);
    let empty = Attributes {
        colorants_in: None,
        subtype: None,
        colorants: None,
        process: None,
        mixing_hints: None,
    };
    assert_eq!(
        empty.separations(resolve, &Limits::DEFAULT, &mut ()),
        Ok(vec![])
    );
}

/// The promise's one carve-out: past 16 underlying components there's no fault to
/// tell, as no clause refuses it, though PDFium does.
#[test]
fn a_pattern_over_more_than_sixteen_is_the_promise_s_exception() {
    let names: Vec<String> = (0..17).map(|n| format!("/N{n}")).collect();
    let text = format!(
        "[/Pattern [/DeviceN [{}] /DeviceGray 5 0 R]]",
        names.join(" ")
    );
    let inputs = "0 1 ".repeat(17);
    let function = format!("<< /Length 0 /FunctionType 4 /Domain [{inputs}] /Range [0 1] >>");
    let (space, faults) = read(&text, &[&function]);
    assert_eq!(faults, []);
    assert_eq!(space.unwrap().components(), Some(18));
    let file = file(&text, &[&function]);
    let resolve = |id: ObjectId| file.objects.get(&id);
    assert_eq!(
        components(resolve, file.value, &Limits::DEFAULT),
        Ok(Some(3))
    );
}
