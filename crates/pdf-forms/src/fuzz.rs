//! In-process fuzzing entrypoint exercised by unit tests and
//! `fuzz/fuzz_targets/forms.rs`.
//!
//! Verifies on arbitrary byte inputs:
//! 1. Zero panics across `Form::read`, `Form::read_with`, `Form::full_name`,
//!    `Form::font`, and `Widget::annot`, under small caps.
//! 2. Sink invariance: `&mut ()` and `Vec` fault sinks read the same form
//!    under both `Rules::Iso` and `Rules::Pdfium`.
//! 3. The tree is a tree: every field but a root has a parent before it that
//!    lists it once, and fields and widgets together stay within
//!    `Limits::fields`.
//! 4. `Debug` formatting never panics on a `Form`, `Field`, `Widget`, or
//!    `Fault`.
//!
//! `over` checks the same of the form a parsed PDF file's catalog names.

use std::collections::BTreeMap;

use pdf_objects::fault::Place;
use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Value};

use crate::{Fault, Form, Limits};

/// Built-in seed inputs: a catalog, then objects 1, 2, … split on `0xFF`.
pub const SEEDS: &[&[u8]] = &[
    b"",
    b"<< /AcroForm << /Fields [1 0 R 3 0 R] /DA (/Helv 0 Tf 0 g) /Q 1 /DR << /Font << /Helv 4 0 R >> >> >> >>\xff<< /T (address) /FT /Tx /Ff 4096 /Kids [2 0 R] >>\xff<< /T (city) /Parent 1 0 R /V (Trenton) /MaxLen 20 /Subtype /Widget /Rect [72 700 200 716] /MK << /BC [0] >> >>\xff<< /T (agree) /FT /Btn /V /Yes /Kids [5 0 R 6 0 R] >>\xff<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\xff<< /Parent 3 0 R /Subtype /Widget /Rect [72 680 82 690] /AS /Yes >>\xff<< /Parent 3 0 R /Subtype /Widget /Rect [92 680 102 690] /AS /Off >>",
    b"<< /AcroForm 1 0 R >>\xff<< /Fields [2 0 R 2 0 R << /T (direct) /FT /Ch /V [(a) (b) /c] >>] /NeedAppearances true /SigFlags 3 /XFA [] >>\xff<< /T (loop) /Kids [2 0 R 3 0 R] >>\xff<< /T (kid) /Parent 9 0 R /FT /Sig /V << /Type /Sig >> /Kids 7 >>",
    b"<< /AcroForm << /Fields [1 0 R] >> >>\xff<< /T (a) /Kids [2 0 R] >>\xff<< /T (b) /Kids [3 0 R] >>\xff<< /T (c) /Kids [4 0 R] >>\xff<< /T (d) /Kids [5 0 R] >>\xff<< /T (e) /FT /Tx /V 6 0 R /DV 3.5 /Ff 1.0 >>\xff<< /Length 2 >>\nstream\nhi",
];

/// Reads the form of a catalog and objects split on `0xFF`, and checks what
/// the module's docs list.
pub fn check(data: &[u8]) {
    let mut parts = data.split(|&b| b == 0xFF);
    let mut parse_limits = pdf_objects::Limits::DEFAULT;
    parse_limits.nesting = 16;
    parse_limits.values = 512;
    let parse = |bytes: &[u8]| {
        Parser::at(bytes, 0)
            .rules(Rules::Pdfium)
            .limits(parse_limits)
            .value(&mut ())
            .ok()
    };

    let catalog: Dict = parts
        .next()
        .and_then(parse)
        .and_then(|object| match object.into_value() {
            Value::Dict(dict) => Some(dict),
            _ => None,
        })
        .unwrap_or_default();
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    for (index, chunk) in parts.take(24).enumerate() {
        if let Some(object) = parse(chunk) {
            objects.insert(ObjectId::new(index as u32 + 1, 0), object);
        }
    }

    read(&objects, &catalog);
}

/// Reads the form the catalog of a parsed PDF file names, and checks what the
/// module's docs list.
pub fn over<'o, R: Resolve<'o> + Copy>(objects: R, trailer: &'o Dict) {
    if let Ok(Some(catalog)) = pdf_semantics::catalog(objects, trailer) {
        read(objects, catalog);
    }
}

fn read<'o, R: Resolve<'o> + Copy>(objects: R, catalog: &'o Dict) {
    let mut limits = Limits::DEFAULT;
    limits.hops = 8;
    limits.fields = 48;
    limits.depth = 4;
    limits.items = 8;

    for rules in [Rules::Iso, Rules::Pdfium] {
        let quiet = Form::read(objects, catalog, rules, &limits);
        let mut faults: Vec<(Place, Fault)> = Vec::new();
        let told = Form::read_with(objects, catalog, rules, &limits, &mut faults);
        assert_eq!(format!("{quiet:?}"), format!("{told:?}"));
        let _ = format!("{faults:?}");
        for (_, fault) in &faults {
            let _ = fault.to_string();
        }

        let Ok(Some(form)) = quiet else { continue };
        let Ok(Some(told)) = told else {
            panic!("a sink changed whether the form was read")
        };
        let fields = form.fields();
        assert_eq!(fields.len(), told.fields().len());
        let widgets: usize = fields.iter().map(|field| field.widgets().len()).sum();
        assert!(fields.len() + widgets <= limits.fields);
        for (index, field) in fields.iter().enumerate() {
            assert_eq!(format!("{field:?}"), format!("{:?}", told.fields()[index]));
            match field.parent() {
                Some(parent) => {
                    assert!(parent < index);
                    let listed = fields[parent].kids().iter().filter(|&&k| k == index);
                    assert_eq!(listed.count(), 1);
                }
                None => assert!(form.roots().contains(&index)),
            }
            assert!(form.full_name(index).is_some());
            let _ = field.kind();
            for widget in field.widgets() {
                let _ = format!("{widget:?}");
                let _ = widget.annot(objects, rules, &limits);
            }
        }
        assert!(form.full_name(fields.len()).is_none());
        for name in form.font_names() {
            let _ = form.font(objects, name, &limits);
        }
        let _ = (form.has_xfa(), form.quadding(), form.sig_flags());
    }
}

#[cfg(test)]
mod tests {
    use super::SEEDS;

    /// `PDF_FORMS_FUZZ_SEEDS` names a folder: writes the seed inputs there for
    /// `just fuzz forms` to begin from.
    #[test]
    #[ignore = "needs PDF_FORMS_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_FORMS_FUZZ_SEEDS").expect("PDF_FORMS_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in SEEDS.iter().enumerate() {
            std::fs::write(dir.join(format!("seed-{i}")), seed).unwrap();
        }
    }
}
