//! Holds `pdf-forms`'s `Rules::Pdfium` reading of the field tree to PDFium's
//! answers recorded at `chromium/8152` in `tests/pdfium/chromium-8152.txt`:
//! for each widget of a page, its field's type, flags, fully qualified name,
//! and a text field's value.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
use pdf_forms::pdf_objects::{Object, Resolve};
use pdf_forms::pdf_semantics::pages;
use pdf_forms::{Field, FieldKind, FieldValue, Form, Limits};
use pdf_structure::read::Recovery;
use pdf_structure::{Reading, read};

fn set() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium")
}

fn read_tsv<const N: usize>(name: &str) -> Vec<[String; N]> {
    fs::read_to_string(set().join(name))
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let cols: Vec<String> = l.splitn(N, '\t').map(str::to_owned).collect();
            cols.try_into().unwrap()
        })
        .collect()
}

/// Bytes as one word, as `oracle/pdfium.py`'s `esc` writes them (none of the
/// cases is long enough for its hash form).
fn esc(data: &[u8]) -> String {
    if data.is_empty() {
        return "%".to_string();
    }
    assert!(data.len() <= 64, "a case's text is past the oracle's LONG");
    let mut out = String::with_capacity(data.len());
    for &b in data {
        if (0x21..=0x7E).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// PDFium's `FPDF_FORMFIELD_*` number for a field.
fn type_number(field: &Field<'_>) -> u32 {
    match field.kind() {
        None => 0,
        Some(FieldKind::PushButton) => 1,
        Some(FieldKind::CheckBox) => 2,
        Some(FieldKind::Radio) => 3,
        Some(FieldKind::Combo) => 4,
        Some(FieldKind::List) => 5,
        Some(FieldKind::Text) => 6,
        Some(FieldKind::Signature) => 7,
        Some(_) => unreachable!("a kind PDFium has no number for"),
    }
}

fn forms_answer(bytes: &[u8]) -> String {
    let mut opts = read::Options::DEFAULT;
    opts.recovery = Recovery::Pdfium;
    let Ok(reading) = Reading::open(bytes, &opts) else {
        return "no-page".to_string();
    };
    let objects = &reading;
    let trailer = reading.trailer().dict();
    let Ok(Some(catalog)) = pdf_forms::pdf_semantics::catalog(objects, trailer) else {
        return "no-page".to_string();
    };
    let Ok(table) = pages::pages(
        objects,
        catalog,
        Rules::Pdfium,
        &pdf_forms::pdf_semantics::Limits::DEFAULT,
        &mut (),
    ) else {
        return "no-page".to_string();
    };
    let Some(page) = table.iter().next().and_then(|page| page.dict) else {
        return "no-page".to_string();
    };
    let form = Form::read(objects, catalog, Rules::Pdfium, &Limits::DEFAULT).unwrap();

    let follow = |object: &Object| match object.as_reference() {
        Some(id) => objects.resolve(id).cloned(),
        None => Some(object.clone()),
    };
    let annots = page.get("Annots").and_then(follow);
    let annots = annots.as_ref().and_then(Object::as_array).unwrap_or(&[]);

    let mut out = Vec::new();
    for (index, entry) in annots.iter().enumerate() {
        if follow(entry).is_none_or(|annot| annot.as_dict().is_none()) {
            continue;
        }
        // The field whose widget this annotation is, by its object number.
        let found = form.as_ref().and_then(|form| {
            let id = entry.as_reference()?;
            form.fields()
                .iter()
                .enumerate()
                .find(|(_, field)| field.widgets().iter().any(|widget| widget.id() == Some(id)))
        });
        let Some((at, field)) = found else {
            out.push(format!("idx {index} none"));
            continue;
        };
        let name = form.as_ref().unwrap().full_name(at).unwrap();
        let mut words = format!(
            "idx {index} type {} flags {} name {}",
            type_number(field),
            field.flags().bits(),
            esc(name.as_bytes())
        );
        if field.kind() == Some(FieldKind::Text) {
            let value = match field.value() {
                Some(FieldValue::Text(text)) => text.as_str(),
                _ => "",
            };
            words.push_str(&format!(" value {}", esc(value.as_bytes())));
        }
        out.push(words);
    }
    if out.is_empty() {
        "none".to_string()
    } else {
        out.join(" ; ")
    }
}

#[test]
fn pdfium_recorded_cases_agree() {
    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let differs: HashMap<String, String> = read_tsv::<2>("differ.tsv")
        .into_iter()
        .map(|[k, why]| (k, why))
        .collect();

    let cases = read_tsv::<3>("cases.tsv");
    assert!(!cases.is_empty(), "cases.tsv must not be empty");

    let mut wrong = Vec::new();
    for [name, kind, _files] in &cases {
        assert_eq!(kind, "forms", "{name}: unexpected case kind {kind}");
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let ours = forms_answer(&bytes);
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no recorded answer"));
        let matches = ours.split_whitespace().eq(pdfium.split_whitespace());
        if differs.contains_key(name) {
            if matches {
                wrong.push(format!(
                    "{name} is listed in differ.tsv but now matches PDFium: {ours}"
                ));
            }
        } else if !matches {
            wrong.push(format!("{name}:\n  pdfium: {pdfium}\n  ours:   {ours}"));
        }
    }

    for k in differs.keys() {
        assert!(
            recorded.contains_key(k),
            "differ.tsv names unknown case {k}"
        );
    }

    assert!(
        wrong.is_empty(),
        "{} case(s) disagreed with chromium/8152:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
