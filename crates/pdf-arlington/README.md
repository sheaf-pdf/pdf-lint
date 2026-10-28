# pdf-arlington

The [Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model), the PDF Association's machine-readable grammar of ISO 32000-2, embedded in one Rust crate: its tables read, and the predicates in their cells evaluated over [pdf-objects](https://crates.io/crates/pdf-objects)' objects.

It's made for a checker. A predicate that can't be told comes to `Unknown`, saying what's in the way, and is never guessed; and where the embedded copy departs from upstream's, it says so, each change with the clause it follows.

## What it does

- **The model, embedded.** Upstream's `tsv/latest` at a pinned commit (`COMMIT`; `model/SOURCE.md` says from where), so nothing needs a checkout: a TSV table per object, a row per key or array element, saying its types, the PDF version it came in and any it was deprecated in, whether it's required, the values it may take and the tables its value links to. `grammar()` reads it once.
- **Corrected where it contradicts the spec.** The embedded copy has the cells corrected where the model contradicts ISO 32000-2 or the reference it cites (`model/corrections.tsv`, `grammar::corrections`), and the rows and tables added that a reference defines and the model lacks (`model/additions`, in upstream's format, `grammar::additions`), each with why. The files in `model/tsv/latest` stay upstream's, byte for byte, and the build stops when upstream has changed a corrected cell or has an added row itself.
- **A checkout instead.** `Grammar::load` reads a `tsv/latest` folder whole, upstream's own or a newer one, an error naming the file and line of a row that doesn't read; `use_model` makes it the model `grammar()` gives. `Grammar::hash` names a model by its tables' bytes (XXH64), so two are alike when it is.
- **Each row's cells read.** A `Row` has upstream's 12 columns, the Type cell split into its types (`ArlType`), a cell with an item per type into those items, and the version predicates around a type, a value or a link taken off and kept beside it (`Wrapped`).
- **The predicates evaluated.** `predicate::parse` reads the language in the cells (INTERNAL_GRAMMAR.md's "Validation of predicates"): `fn:` functions, `&&` and `||`, comparisons, arithmetic, and keys, values and paths from the object, its parents or the trailer. `required`, `holds`, `reference` and the rest evaluate a cell on an object where it sits: a `Cx` gives the object, the way to it, the trailer and the file's version, and resolves a reference through a closure, so any reader's objects can be checked. The 34 functions of `SUPPORTED` are evaluated; any other comes to `Truth::Unknown`, named.
- **Versions and extensions.** `since` reads when a key came in, `gate` what the version predicates around a type, value or link make of it at a file's version; `extension::declared` gives the extensions a file declares, from its catalog and its XMP metadata, and `extension::named` every one the model names.

## An example

```rust
use std::collections::BTreeSet;

use pdf_arlington::pdf_objects::{Dict, Object, ObjectId, Version, dict};
use pdf_arlington::predicate::{self, Fields, Structure, Trees};
use pdf_arlington::{At, Cx, Truth, grammar};

// The embedded model's row for a page's /Rotate: an integer since PDF
// 1.0, whose possible values are the multiples of 90.
let row = grammar().table("PageObject").and_then(|t| t.row(At::Key(b"Rotate"))).unwrap();
assert_eq!((row.r#type[0].item.name(), row.since_version.as_str()), ("integer", "1.0"));
let multiple = &row.possible_values[0][0].item;
assert_eq!(multiple, "fn:Eval((@Rotate mod 90)==0)");

// That predicate, evaluated on a page where the row sits. What the page
// is in, and what a reference resolves to (here, nothing), are the
// caller's to give.
let holds = |page: &Object| {
    let (trailer, extensions) = (Dict::new(), BTreeSet::new());
    let (trees, structure, fields) = (Trees::default(), Structure::default(), Fields::default());
    let cx = Cx {
        container: page,
        at: At::Key(b"Rotate"),
        parents: &[],
        trailer: &trailer,
        resolve: &|_: ObjectId| None,
        version: Version::new(2, 0),
        defaults: None,
        size: None,
        trees: &trees,
        structure: &structure,
        fields: &fields,
        extensions: &extensions,
    };
    predicate::holds_fully(multiple, &cx)
};
assert_eq!(holds(&dict! { "Type" => "Page", "Rotate" => 180 }.into()), Truth::True);
assert_eq!(holds(&dict! { "Type" => "Page", "Rotate" => 45 }.into()), Truth::False);
```

## License

Licensed under the Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)), as the model is. Upstream's files in `model/` are its tables, `tsv/latest`, and its `LICENSE` and `NOTICE.txt`; `corrections.tsv`, `additions/` and `SOURCE.md` beside them are this crate's, under the same licence.
