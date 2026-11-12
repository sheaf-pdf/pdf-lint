//! The fuzz body for the adapter, which a stable unit test runs: any bytes as
//! edits to a document of lopdf's, what's written of it read back and checked.
//! No fuzz target runs it. It's compiled only for tests.

use pdf_linearize::Limits;
use pdf_linearize::check::{self, Facts, Finding, Grade, Sink};

/// Told none, and how often it's asked.
#[derive(Default)]
struct Unwanted {
    told: usize,
    asked: std::cell::Cell<usize>,
}

impl Sink for Unwanted {
    fn finding(&mut self, _: Finding) {
        self.told += 1;
    }

    fn wants(&self) -> bool {
        self.asked.set(self.asked.get() + 1);
        false
    }
}

/// The same verdict whatever the sink, none told what doesn't want any, and
/// that verdict what the findings say. What's kept is given back.
fn judged_alike(facts: &Facts, limits: &Limits) -> (check::Verdict, Vec<Finding>) {
    let mut kept = Vec::new();
    let verdict = check::check(facts, limits, &mut kept);
    assert_eq!(check::check(facts, limits, &mut ()), verdict);
    let mut unwanted = Unwanted::default();
    assert_eq!(check::check(facts, limits, &mut unwanted), verdict);
    assert_eq!(unwanted.told, 0);
    let by = &mut kept.clone() as &mut dyn Sink;
    assert_eq!(check::check(facts, limits, by), verdict);
    let error = kept.iter().any(|finding| finding.grade() == Grade::Error);
    assert_eq!(verdict.holds, verdict.linearized && !error, "{kept:?}");
    (verdict, kept)
}

/// The objects [`lopdf_document`] starts from: a tree of three pages by two
/// nodes, what they inherit, share and have alone, and an outline and a page no
/// tree lists.
fn lopdf_start() -> ::lopdf::Document {
    use ::lopdf::{Dictionary, Document, Object, Stream, dictionary};

    let reference = |number: u32| Object::Reference((number, 0));
    let page = |parent: u32, more: Dictionary| {
        let mut dict = dictionary! { "Type" => "Page", "Parent" => reference(parent) };
        dict.extend(&more);
        Object::Dictionary(dict)
    };
    let mut doc = Document::new();
    doc.version = "1.7".to_string();
    let objects: [(u32, Object); 13] = [
        (
            1,
            Object::Dictionary(dictionary! {
                "Type" => "Catalog",
                "Pages" => reference(2),
                "Outlines" => reference(9),
                "Foo" => reference(12),
                "OpenAction" => vec![reference(4), Object::Name(b"Fit".to_vec())],
            }),
        ),
        (
            2,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![reference(3), reference(10)],
                "Count" => 3,
                "MediaBox" => vec![0.into(), 0.into(), 100.into(), reference(13)],
                "Resources" => reference(7),
                "Rotate" => 90,
            }),
        ),
        (3, page(2, dictionary! { "Contents" => reference(5) })),
        (
            4,
            page(
                10,
                dictionary! { "Contents" => reference(6), "X" => reference(8) },
            ),
        ),
        (
            5,
            Object::Stream(Stream::new(Dictionary::new(), b"q Q".to_vec())),
        ),
        (
            6,
            Object::Stream(Stream::new(Dictionary::new(), b"Q q".to_vec())),
        ),
        (
            7,
            Object::Dictionary(dictionary! { "Font" => Dictionary::new() }),
        ),
        (8, Object::Dictionary(dictionary! { "Mine" => true })),
        (9, Object::Dictionary(dictionary! { "Count" => 0 })),
        (
            10,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Parent" => reference(2),
                "Kids" => vec![reference(4), reference(11)],
                "Count" => 2,
            }),
        ),
        (11, page(10, dictionary! { "X" => reference(8) })),
        (12, page(10, dictionary! { "Mine" => true })),
        (13, Object::Integer(200)),
    ];
    doc.objects.extend(
        objects
            .into_iter()
            .map(|(number, object)| ((number, 0), object)),
    );
    doc.trailer.set("Root", reference(1));
    doc
}

/// The fuzz body for the lopdf adapter: any bytes as edits to the objects of a
/// document of lopdf's, so its reader isn't what's under test: refused, with
/// `out` as it was given, or written, its positions counted right, and what's
/// written saying it's linearized and holding, nothing said but what a page
/// doesn't have to push, its facts the same read whole beside its reading as of
/// its reading alone. It gives whether it wrote.
pub fn lopdf_document(data: &[u8]) -> bool {
    use ::lopdf::{Dictionary, Object};
    use pdf_structure::read::{self, Reading, WithReading};

    use pdf_linearize::family;

    const KEYS: [&str; 16] = [
        "Parent",
        "Kids",
        "Type",
        "MediaBox",
        "CropBox",
        "Resources",
        "Rotate",
        "Contents",
        "X",
        "Count",
        "First",
        "Next",
        "Foo",
        "UserUnit",
        "Pages",
        "Thumb",
    ];
    const OBJECTS: u32 = 16;
    let id = |byte: u8| (u32::from(byte) % OBJECTS + 1, 0);
    let value = |of: u8, byte: u8| match of % 11 {
        0 => Object::Null,
        1 => Object::Integer(i64::from(byte)),
        2 | 3 => Object::Reference(id(byte)),
        4 => Object::Name(if byte.is_multiple_of(2) {
            b"Page".to_vec()
        } else {
            b"Pages".to_vec()
        }),
        5 => Object::Array(vec![Object::Reference(id(byte))]),
        6 => Object::Array(Vec::new()),
        7 => Object::Array(vec![
            0.into(),
            0.into(),
            9.into(),
            Object::Reference(id(byte)),
        ]),
        8 => Object::Dictionary(Dictionary::new()),
        9 => Object::Reference((u32::from(byte) + 100, 0)),
        _ => Object::Real(f32::from(byte)),
    };
    let mut doc = lopdf_start();
    for &[what, at, key, byte] in data.as_chunks::<4>().0 {
        let at = id(at);
        let key = KEYS[usize::from(key) % KEYS.len()].as_bytes();
        match what % 4 {
            0 | 1 => {
                let dict = match doc.objects.get_mut(&at) {
                    Some(Object::Dictionary(dict)) => dict,
                    Some(Object::Stream(stream)) => &mut stream.dict,
                    _ => continue,
                };
                match what % 8 {
                    0 | 1 | 4 | 5 => dict.set(key.to_vec(), value(what >> 3, byte)),
                    _ => {
                        dict.remove(key);
                    }
                }
            }
            2 => {
                doc.objects.insert(at, value(what >> 2, byte));
            }
            _ => {
                doc.objects.remove(&at);
            }
        }
    }
    let mut out = b"before".to_vec();
    let options = crate::Options::DEFAULT;
    let Ok(layout) = crate::linearize(&mut out, &doc, &options) else {
        assert_eq!(out, b"before");
        return false;
    };
    assert_eq!(out.len() as u64 - 6, layout.len());
    let file = &out[6..];
    let reading = Reading::open(file, &read::Options::DEFAULT).expect("it reads");
    assert!(reading.facts().linearized);
    let read = family::facts(&reading, &Limits::DEFAULT).expect("it's in memory");
    // The same facts of it read whole beside its reading.
    let whole = WithReading::new(file, &read::Options::DEFAULT).expect("it reads");
    let beside = family::facts_with_reading(&whole, &Limits::DEFAULT).expect("it's in memory");
    assert_eq!(format!("{beside:?}"), format!("{read:?}"));
    let (verdict, findings) = judged_alike(&read, &Limits::DEFAULT);
    assert!(
        verdict.linearized && verdict.holds && verdict.hints_read,
        "{findings:?}"
    );
    assert!(
        (findings.iter()).all(|finding| matches!(finding, Finding::PageAttribute { .. })),
        "{findings:?}"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain generator: what's fuzzed only has to vary, and a seed gives the
    /// same inputs each time.
    fn generator(mut state: u64) -> impl FnMut() -> u64 {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    /// The lopdf adapter's body, on random edits: a good share of them written,
    /// so that what's held is what's been run.
    #[test]
    fn what_lopdf_s_adapter_writes_of_any_document_holds() {
        let mut random = generator(0x0F1E_2D3C_4B5A_6978);
        assert!(lopdf_document(&[]));
        let mut written = 0;
        for round in 0..6000 {
            let edits = 1 + round % 6;
            let data: Vec<u8> = (0..4 * edits).map(|_| (random() >> 11) as u8).collect();
            written += usize::from(lopdf_document(&data));
        }
        assert!(written > 300, "{written} of 6,000 were written");
    }
}
