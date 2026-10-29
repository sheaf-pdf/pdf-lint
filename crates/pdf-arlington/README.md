# pdf-arlington

The [Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model), the PDF Association's machine-readable grammar of ISO 32000-2, embedded in one Rust crate: its tables read, and the predicates in their cells evaluated over [pdf-objects](https://crates.io/crates/pdf-objects)' objects.

It's made for a checker. A predicate that can't be told comes to `Unknown`, saying what's in the way, and is never guessed; and where the embedded copy departs from upstream's, it says so, each change with the clause it follows.

## What it does

- **The model, embedded.** Upstream's `tsv/latest` at a pinned commit (`COMMIT`; `model/SOURCE.md` says from where), so nothing needs a checkout: a TSV table per object, a row per key or array element, saying its types, the PDF version it came in and any it was deprecated in, whether it's required, the values it may take and the tables its value links to. `Grammar::embedded()` reads it once, the first time it's asked for.
- **Corrected where it contradicts the spec.** The embedded copy has the cells corrected where the model contradicts ISO 32000-2 or the reference it cites (`model/corrections.tsv`, `grammar::corrections`), and the rows and tables added that a reference defines and the model lacks (`model/additions`, in upstream's format, `grammar::additions`), each with why. The files in `model/tsv/latest` stay upstream's, byte for byte, and the build stops when upstream has changed a corrected cell or has an added row itself.
- **A checkout instead.** `Grammar::load` reads a `tsv/latest` folder whole, upstream's own or a newer one, an error (`grammar::Error`) naming the file and line of a row that doesn't read; the caller holds the model it gives, as nothing in the crate chooses one. `Grammar::hash` names a model by its tables' bytes (XXH64), so two are alike when it is.
- **Each row's cells read.** A `Row` has upstream's 12 columns, the Type cell split into its types (`ArlType`), a cell with an item per type into those items, and the version predicates around a type, a value or a link taken off and kept beside it (`Wrapped`).
- **The predicates evaluated.** `predicate::parse` reads the language in the cells (INTERNAL_GRAMMAR.md's "Validation of predicates"): `fn:` functions, `&&` and `||`, comparisons, arithmetic, and keys, values and paths from the object, its parents or the trailer; an error (`predicate::Error`) says where one doesn't parse. `required`, `holds`, `reference` and the rest evaluate a cell on an object where it sits: a `FileCx` holds what's the same for every row of a file, its trailer and version and what resolves a reference, any of pdf-objects' `Resolve` (a closure, or a reader's or a document's own), so any reader's objects can be checked, and keeps what the predicates read of the file as a whole once read, shared across threads when its resolver is `Sync`; an object the resolver gives none for is null, whatever the reason, so over a reading by position one whose bytes the source failed to give is null in that row, and missing from any tree read then; a `Cx` it makes for each row gives the object, its number when it's an indirect object, and the way to it. The 34 functions of `SUPPORTED` are evaluated; any other comes to `Truth::Unknown`, named.
- **Versions and extensions.** `since` reads when a key came in, `gate` what the version predicates around a type, value or link make of it at a file's version; `extension::declared` gives the extensions a file declares, from its catalog and its XMP metadata, and `Grammar::extensions` every one a model names.

## An example

```rust
use pdf_arlington::pdf_objects::{Dict, Object, ObjectId, Version, dict};
use pdf_arlington::predicate;
use pdf_arlington::{At, FileCx, Grammar, Truth};

// The embedded model's row for a page's /Rotate: an integer since PDF
// 1.0, whose possible values are the multiples of 90.
let table = Grammar::embedded().table("PageObject").unwrap();
let row = table.row(At::Key(b"Rotate")).unwrap();
assert_eq!((row.r#type[0].item.name(), row.since_version.as_str()), ("integer", "1.0"));
let multiple = &row.possible_values[0][0].item;
assert_eq!(multiple, "fn:Eval((@Rotate mod 90)==0)");

// That predicate, evaluated on a page where the row sits. The file the
// page is in, and what a reference resolves to (here, nothing), are the
// caller's to give: once a file, every row it has evaluated in it.
let trailer = Dict::new();
let resolve = |_: ObjectId| None;
let file = FileCx::new(&trailer, Version::new(2, 0), &resolve);
let holds = |page: &Object| predicate::holds_fully(multiple, &file.at(page, At::Key(b"Rotate")));
assert_eq!(holds(&dict! { "Type" => "Page", "Rotate" => 180 }.into()), Truth::True);
assert_eq!(holds(&dict! { "Type" => "Page", "Rotate" => 45 }.into()), Truth::False);
```

A `FileCx`'s type names its resolver: here the closure's. `FileCx<'a>`, the default, is one over any resolver, a `dyn Resolve`, as a `Cx` reads through it, so a function taking `&FileCx` takes a reference to one over any resolver; one kept is kept by its resolver's type, or boxed (`Box<FileCx>`).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)), as the model is. Upstream's files in `model/` are its tables, `tsv/latest`, and its `LICENSE` and `NOTICE.txt`; `corrections.tsv`, `additions/` and `SOURCE.md` beside them are this crate's, under the same licence.
