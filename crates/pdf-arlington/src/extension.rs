//! The extensions a file declares, as the model names them
//! (INTERNAL_GRAMMAR.md, `fn:Extension`): those of the catalog's
//! extensions dictionary (ISO 32000-2, 7.12), and the subsets its XMP
//! metadata says it conforms to. The checker holds a file to what's under
//! the extensions it declares, as TestGrammar does to those named with its
//! `--extensions`; it can't tell any other. Those a model names are
//! [`Grammar::extensions`].

use std::collections::BTreeSet;

use pdf_objects::{Dict, Object, ObjectId, Value};

use crate::grammar::{Grammar, WrapFn};

/// The model's extensions that ISO publishes, by the number its extensions
/// dictionary entries give as `/ExtensionLevel` under `/ISO_` (ISO 32000-2,
/// 7.12.3): ISO 21812-1 (print product metadata), ISO 19593-1
/// (processing steps), and the technical specifications.
pub const ISO: &[(&str, i64)] = &[
    ("ISO_19593", 19593),
    ("ISO_21812", 21812),
    ("ISO_TS_24064", 24064),
    ("ISO_TS_24654", 24654),
    ("ISO_TS_32001", 32001),
    ("ISO_TS_32003", 32003),
    ("ISO_TS_32004", 32004),
    ("ISO_TS_32007", 32007),
];

/// The extensions a file declares, as the model names them, from `root`,
/// its trailer's `/Root` as written, and `metadata`, the data of its
/// catalog's metadata stream (ISO 32000-2, 14.3.2); `resolve` gives the
/// object a reference names, as its own, so a reader needn't keep what
/// it reads:
///
/// - `ADBE_Extn3`, from an `/ADBE` entry at `/BaseVersion /1.7` and an
///   `/ExtensionLevel` of 3 or more, Adobe's levels each having those
///   before it;
/// - `ETSI_PAdES`, from an `/ESIC` entry, ETSI's (EN 319 142-1);
/// - an ISO extension of [`ISO`], from an `/ISO_` entry with its number;
/// - `ISO_19005_3` (PDF/A-3), from XMP metadata whose `pdfaid:part` is 3,
///   and `PDF_VT2` (PDF/VT-2), from its `pdfvtid:GTS_PDFVTVersion`.
///
/// An entry may be one developer extensions dictionary or, from PDF 2.0,
/// an array of them. Metadata a reader doesn't have in plain view, given
/// as none, declares nothing.
///
/// `resolve` gives an object of its own, not a borrow, as a
/// [`Resolve`](pdf_objects::Resolve) does for a
/// [`FileCx`](crate::predicate::FileCx): what's declared is read once,
/// before a check, from a few objects, which a reader on demand then
/// needn't keep, where a check reads every object, many more than once. A
/// `Resolve` gives one by a clone: `&|id| resolver.resolve(id).cloned()`.
pub fn declared(
    root: Option<&Object>,
    metadata: Option<&[u8]>,
    resolve: &dyn Fn(ObjectId) -> Option<Object>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let get = |o: &Object| -> Option<Object> {
        let mut o = o.clone();
        for _ in 0..8 {
            match *o.value() {
                Value::Ref(id) => o = resolve(id)?,
                _ => return Some(o),
            }
        }
        None
    };
    let catalog = root.and_then(get).and_then(|o| o.as_dict().cloned());
    let extensions = (catalog.as_ref())
        .and_then(|c| c.get(b"Extensions"))
        .and_then(get)
        .and_then(|o| o.as_dict().cloned());
    for (prefix, entry) in extensions.iter().flat_map(Dict::iter) {
        let entries = match get(entry).map(|o| o.into_value()) {
            Some(Value::Array(entries)) => entries.iter().filter_map(get).collect(),
            Some(value) => vec![Object::new(value)],
            None => Vec::new(),
        };
        for entry in entries {
            let Some(entry) = entry.as_dict() else {
                continue;
            };
            let level = (entry.get(b"ExtensionLevel"))
                .and_then(get)
                .and_then(|o| o.as_integer());
            let base = (entry.get(b"BaseVersion")).and_then(get);
            let base = base.as_ref().and_then(|o| o.as_name());
            match prefix {
                b"ADBE" if base == Some(b"1.7") && level.is_some_and(|l| l >= 3) => {
                    names.insert("ADBE_Extn3".to_string());
                }
                b"ESIC" => {
                    names.insert("ETSI_PAdES".to_string());
                }
                b"ISO_" => {
                    let iso = ISO.iter().find(|&&(_, n)| Some(n) == level);
                    names.extend(iso.map(|(name, _)| name.to_string()));
                }
                _ => {}
            }
        }
    }
    if let Some(xmp) = metadata {
        if property(xmp, b"pdfaid:part").as_deref() == Some(&b"3"[..]) {
            names.insert("ISO_19005_3".to_string());
        }
        if property(xmp, b"pdfvtid:GTS_PDFVTVersion").as_deref() == Some(&b"PDF/VT-2"[..]) {
            names.insert("PDF_VT2".to_string());
        }
    }
    names
}

impl Grammar {
    /// Every extension the model names, in its cells (`fn:Extension(X`)
    /// and around its types, values and links: what a file can be checked
    /// under.
    pub fn extensions(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for table in self.tables() {
            for row in &table.rows {
                let values = row.possible_values.iter().flatten();
                let cells = [&row.since_version, &row.required]
                    .into_iter()
                    .chain(&row.special_case)
                    .chain(&row.indirect_reference)
                    .chain(values.clone().map(|value| &value.item));
                for cell in cells {
                    names.extend(called(cell));
                }
                let wraps = (row.r#type.iter().flat_map(|t| &t.wraps))
                    .chain(row.link.iter().flatten().flat_map(|l| &l.wraps))
                    .chain(values.flat_map(|v| &v.wraps));
                for wrap in wraps.filter(|w| w.func == WrapFn::Extension) {
                    names.insert(wrap.arg.clone());
                }
            }
        }
        names
    }
}

/// The extensions `text` calls `fn:Extension` of.
fn called(text: &str) -> impl Iterator<Item = String> + '_ {
    text.match_indices("fn:Extension(").map(move |(i, call)| {
        (text[i + call.len()..].chars())
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect()
    })
}

/// The value of the XMP property `name` in `xmp`, written as an attribute
/// (`name="3"`) or an element (`<name>3</name>`), its white-space trimmed:
/// the first written.
fn property(xmp: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    let trim = |v: &[u8]| v.trim_ascii().to_vec();
    let mut at = 0;
    while let Some(i) = find(&xmp[at..], name).map(|i| at + i) {
        at = i + name.len();
        let rest = &xmp[at..];
        let after = rest.trim_ascii_start();
        if let Some(value) = after.strip_prefix(b"=") {
            let value = value.trim_ascii_start();
            let Some((&quote, value)) = value.split_first() else {
                continue;
            };
            if quote == b'"' || quote == b'\'' {
                let end = value.iter().position(|&b| b == quote)?;
                return Some(trim(&value[..end]));
            }
        } else if let Some(value) = rest.strip_prefix(b">") {
            let preceded = i > 0 && xmp[i - 1] == b'<';
            if preceded {
                let end = find(value, b"<")?;
                return Some(trim(&value[..end]));
            }
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::dict;

    use super::*;

    /// A file's objects, its trailer's `/Root` and its metadata.
    struct File {
        objects: BTreeMap<ObjectId, Object>,
        root: Object,
        metadata: Option<Vec<u8>>,
    }

    /// A file whose catalog, object 1, has `extensions`, and `xmp` as
    /// metadata.
    fn declaring(extensions: Option<Dict>, xmp: Option<&str>) -> File {
        let mut catalog = dict! { "Type" => "Catalog" };
        if let Some(extensions) = extensions {
            catalog.set("Extensions", extensions);
        }
        File {
            objects: BTreeMap::from([((1, 0), catalog.into())]),
            root: Object::reference(1),
            metadata: xmp.map(|x| x.as_bytes().to_vec()),
        }
    }

    fn names(file: &File) -> Vec<String> {
        let resolve = |id: ObjectId| file.objects.get(&id).cloned();
        (declared(Some(&file.root), file.metadata.as_deref(), &resolve).into_iter()).collect()
    }

    fn developer(base: &str, level: i64) -> Object {
        dict! { "BaseVersion" => base, "ExtensionLevel" => level }.into()
    }

    #[test]
    fn a_file_declares_the_extensions_its_catalog_and_metadata_name() {
        assert_eq!(names(&declaring(None, None)), Vec::<String>::new());
        // Adobe's level 3 and later, at 1.7; ETSI's; ISO's by number, in an
        // array, the PDF 2.0 way.
        let extensions = dict! {
            "ADBE" => developer("1.7", 8),
            "ESIC" => developer("1.7", 1),
            "ISO_" => vec![developer("2.0", 32001), developer("2.0", 32003)],
            "XYZ" => developer("1.7", 3),
        };
        assert_eq!(
            names(&declaring(Some(extensions), None)),
            ["ADBE_Extn3", "ETSI_PAdES", "ISO_TS_32001", "ISO_TS_32003"]
        );
        // Adobe's earlier levels, or another base version, aren't level 3.
        for adbe in [developer("1.7", 1), developer("1.6", 3)] {
            let extensions = dict! { "ADBE" => adbe };
            assert_eq!(
                names(&declaring(Some(extensions), None)),
                Vec::<String>::new()
            );
        }
        // PDF/A-3 and PDF/VT-2, from XMP, as an attribute or an element.
        let attribute = r#"<rdf:Description pdfaid:part="3" pdfaid:conformance="B"/>"#;
        let element = "<pdfaid:part> 3 </pdfaid:part><pdfvtid:GTS_PDFVTVersion>PDF/VT-2</pdfvtid:GTS_PDFVTVersion>";
        assert_eq!(names(&declaring(None, Some(attribute))), ["ISO_19005_3"]);
        assert_eq!(
            names(&declaring(None, Some(element))),
            ["ISO_19005_3", "PDF_VT2"]
        );
        let quoted = r#"<rdf:Description pdfaid:part = '3'/>"#;
        assert_eq!(names(&declaring(None, Some(quoted))), ["ISO_19005_3"]);
        let a2 = r#"<rdf:Description pdfaid:part='2'/>"#;
        assert_eq!(names(&declaring(None, Some(a2))), Vec::<String>::new());
    }

    #[test]
    fn every_extension_the_model_names_is_found_and_isos_by_number() {
        // Every `fn:Extension(X` in the embedded model's files, as written.
        let written: BTreeSet<String> = (crate::grammar::model::TABLES.iter())
            .flat_map(|&(_, text)| called(text))
            .collect();
        assert_eq!(Grammar::embedded().extensions(), written);
        // ISO's, but PDF/A-3's, declared in XMP metadata, by number.
        let iso: BTreeSet<String> = (written.into_iter())
            .filter(|n| n.starts_with("ISO_") && n != "ISO_19005_3")
            .collect();
        let known: BTreeSet<String> = ISO.iter().map(|(name, _)| name.to_string()).collect();
        assert_eq!(iso, known);
    }
}
