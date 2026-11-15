# pdf-arlington

Embedded [Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model) grammar and predicate evaluator over [pdf-objects](https://crates.io/crates/pdf-objects).

The Arlington PDF Model is the PDF Association's machine-readable TSV specification of ISO 32000-2: one table per PDF object kind, and one row per dictionary key or array element describing allowed types, version availability, requiredness, allowed values, and child table links. `pdf-arlington` embeds a pinned snapshot of those tables, parses their `fn:` predicate expressions, and evaluates them against any `pdf-objects` object graph.

## Key Capabilities

- **Zero-setup embedded model (`Grammar::embedded()`):** Compiles in upstream's `tsv/latest` tables (at `COMMIT`) alongside audited spec fixes (`grammar::corrections()`) and missing legacy rows (`grammar::additions()`). Build scripts verify that upstream TSV files remain unmodified and that every patch still cleanly applies.
- **Load custom checkouts (`Grammar::load`):** Parse any local `tsv/latest` directory at runtime, identified by a SHA-256 content digest (`Grammar::digest`).
- **Typed TSV rows (`Table`, `Row`):** Splits multi-type cells (`ArlType`), per-type value/link lists, and version/extension wrapper predicates (`Wrapped`) into structured Rust types.
- **Three-valued predicate evaluation (`predicate`, `FileCx`, `Cx`, `Truth`):** Evaluates Arlington `fn:` predicates, boolean logic, comparisons, arithmetic, and `@Key` / `parent::` / `trailer::` path lookups over any `pdf_objects::Resolve` implementation. Predicates that cannot be determined (due to missing context or unsupported functions) return `Truth::Unknown` rather than guessing. Attaching `.grammar(model)` to `FileCx` caches parsed ASTs across files and threads.
- **PDF version & extension gating (`extension`):** Evaluates `fn:SinceVersion`, `fn:Deprecated`, and `fn:Extension` wrappers against declared catalog `/Extensions` and XMP conformance claims (`extension::declared`).

## Quickstart

```rust
use pdf_arlington::pdf_objects::{Dict, Object, ObjectId, Version, dict, resolve_fn};
use pdf_arlington::predicate;
use pdf_arlington::{At, FileCx, Grammar, Truth};

// Look up the Arlington row for a page's /Rotate entry: an integer since
// PDF 1.0 whose allowed values must be multiples of 90.
let table = Grammar::embedded().table("PageObject").unwrap();
let row = table.row(At::Key(b"Rotate")).unwrap();
assert_eq!((row.r#type[0].item.name(), row.since_version.as_str()), ("integer", "1.0"));
let multiple = &row.possible_values[0][0].item;
assert_eq!(multiple, "fn:Eval((@Rotate mod 90)==0)");

// Evaluate that predicate against two page dictionaries.
let trailer = Dict::new();
let resolve = resolve_fn(|_: ObjectId| None);
let file = FileCx::new(&trailer, Version::new(2, 0), &resolve).grammar(Grammar::embedded());
let holds = |page: &Object| predicate::holds_fully(multiple, &file.at(page, At::Key(b"Rotate")));
assert_eq!(holds(&dict! { "Type" => Object::name("Page"), "Rotate" => 180 }.into()), Truth::True);
assert_eq!(holds(&dict! { "Type" => Object::name("Page"), "Rotate" => 45 }.into()), Truth::False);
```

## License

Licensed under the Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)), matching the upstream Arlington PDF Model (`model/LICENSE` and `model/NOTICE.txt`).
