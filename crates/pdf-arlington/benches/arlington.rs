//! pdf-arlington timed on the model it embeds and on objects made here, so
//! that a change to its parser, its tables' lookup, its evaluators or its
//! reading of a name tree can be judged before and after it, by `cargo
//! bench -p pdf-arlington`:
//!
//! - `parse`: `predicate::parse` of every distinct predicate of the
//!   embedded model's cells, each SinceVersion, Required,
//!   IndirectReference, SpecialCase and PossibleValues item that calls a
//!   function (`model`), in bytes.
//! - `rows`: `Table::row` at every row's key or index, and at a key no row
//!   has, in the model's five largest tables, each by its name, in rows
//!   asked.
//! - `tables`: a walk over every table `Grammar::tables` gives, their rows
//!   summed, by `next` alone (`next`), in one pass (`fold`), and collected
//!   into a vector first (`collect`), in tables walked.
//! - `evaluate`: every row of `PageObject`, `Catalog`, `FontType1` and
//!   `XObjectImage`, in an object of that table's kind with a value of its
//!   first type for each key, through `required`, `reference` (each
//!   IndirectReference), `holds_fully` (each SpecialCase, and each
//!   PossibleValues item that calls a function) and `FileCx::since`: with
//!   the model given to the `FileCx`, which keeps the predicates parsed
//!   (`<table>/given`), and without, which parses each as it's asked
//!   (`<table>/alone`), in answers.
//! - `trees`: `fn:InNameTree`, as `ActionGoTo`'s `/D` asks it of a string,
//!   over a `/Dests` name tree of 10,000 keys in 100 leaves: a file's first
//!   ask, which reads the tree (`first`), in keys read; and every key asked
//!   after it, of the tree the file keeps (`kept`), in keys asked.
//!
//! Each input is held to what it gives before it's timed: the predicates to
//! how many there are and how many parse; each row asked to the row found;
//! the tables to their count, and each walk to their rows; the answers to
//! their tally, the same with the model given and without; each key of the
//! tree to true, and a key in none to false. The objects are made here,
//! into a map that's the resolver, which gives every object it's asked for,
//! so that no read fails and no cap is met; nothing is read from disk, and
//! the inputs are the same on every run.

use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdf_arlington::pdf_objects::{Dict, Object, ObjectId, Version, dict};
use pdf_arlington::predicate::{self, Reference};
use pdf_arlington::{ArlType, At, FileCx, Grammar, Row, Since, Table, Truth};

/// How many distinct predicates the embedded model's cells hold, and how
/// many of them parse.
const PREDICATES: (usize, usize) = (474, 474);

/// Every distinct predicate of `model`'s cells, as its rows split them:
/// each SinceVersion, Required, IndirectReference, SpecialCase and
/// PossibleValues item that calls a function, the version predicates
/// around a value taken off.
fn predicates(model: &Grammar) -> BTreeSet<&str> {
    let mut texts = BTreeSet::new();
    for row in model.tables().flat_map(|table| &table.rows) {
        let values = row.possible_values.iter().flatten();
        let cells = [&row.since_version, &row.required]
            .into_iter()
            .chain(&row.indirect_reference)
            .chain(&row.special_case)
            .chain(values.map(|value| &value.item));
        let calls = cells.map(String::as_str).filter(|t| t.starts_with("fn:"));
        texts.extend(calls);
    }
    texts
}

/// How many of `texts` parse.
fn parsed(texts: &[&str]) -> usize {
    texts
        .iter()
        .filter(|text| predicate::parse(text).is_ok())
        .count()
}

fn parse_group(c: &mut Criterion) {
    let texts: Vec<&str> = predicates(Grammar::embedded()).into_iter().collect();
    assert_eq!(
        (texts.len(), parsed(&texts)),
        PREDICATES,
        "parse/model: the predicates, and how many parse"
    );
    let bytes: usize = texts.iter().map(|text| text.len()).sum();
    let mut group = c.benchmark_group("parse");
    group.throughput(Throughput::Bytes(bytes as u64));
    group.bench_function("model", |b| b.iter(|| parsed(black_box(&texts))));
    group.finish();
}

/// The model's five largest tables, by their rows, and how many rows each
/// has: every other table has as many as the last at most.
const LARGEST: [(&str, usize); 5] = [
    ("StructureAttributesDict", 53),
    ("AnnotLine", 39),
    ("AnnotFreeText", 36),
    ("AnnotPolyLine", 35),
    ("AnnotPolygon", 35),
];

/// A key no table of the model has a row of its own for.
const NO_ROW: &[u8] = b"NoSuchKey";

/// The element an array's row is, a repeating row's first; none for a
/// dictionary's.
fn index(row: &Row) -> Option<usize> {
    let n = row.key.strip_suffix('*').unwrap_or(&row.key);
    let digits = !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    digits.then(|| n.parse().ok()).flatten()
}

/// Where in its object a row's value is: its index, or its key.
fn at(row: &Row) -> At<'_> {
    match index(row) {
        Some(i) => At::Index(i),
        None => At::Key(row.key.as_bytes()),
    }
}

/// How many of `asks` find a row of `table`.
fn found(table: &Table, asks: &[At]) -> usize {
    asks.iter().filter(|&&at| table.row(at).is_some()).count()
}

fn rows_group(c: &mut Criterion) {
    let model = Grammar::embedded();
    let most = LARGEST[LARGEST.len() - 1].1;
    for table in model.tables() {
        let named = LARGEST.iter().any(|&(name, _)| name == table.name);
        assert!(
            named || table.rows.len() <= most,
            "rows: {} has more rows than the five",
            table.name
        );
    }
    let mut group = c.benchmark_group("rows");
    for (name, count) in LARGEST {
        let table = model
            .table(name)
            .unwrap_or_else(|| panic!("rows/{name}: the table"));
        assert_eq!(table.rows.len(), count, "rows/{name}: its rows");
        let mut asks: Vec<At> = table.rows.iter().map(at).collect();
        for (row, &ask) in table.rows.iter().zip(&asks) {
            let itself = table.row(ask).is_some_and(|f| std::ptr::eq(f, row));
            assert!(itself, "rows/{name}: {} finds itself", row.key);
        }
        asks.push(At::Key(NO_ROW));
        let wild = table.rows.iter().find(|row| row.key == "*");
        let none = table.row(At::Key(NO_ROW));
        let as_wild = match (none, wild) {
            (Some(f), Some(w)) => std::ptr::eq(f, w),
            (None, None) => true,
            _ => false,
        };
        assert!(as_wild, "rows/{name}: a key of no row finds the wildcard's");
        let expected = count + usize::from(wild.is_some());
        assert_eq!(found(table, &asks), expected, "rows/{name}: the rows found");
        group.throughput(Throughput::Elements(asks.len() as u64));
        group.bench_function(name, |b| b.iter(|| found(table, black_box(&asks))));
    }
    group.finish();
}

/// How many tables the embedded model has, and how many rows they hold.
const TABLES: (usize, usize) = (614, 3_988);

/// The rows of each table `tables` gives, summed by asking for each in
/// turn, `next` by `next`, as a `for` does.
fn rows_by_next(tables: impl Iterator<Item = &'static Table>) -> usize {
    let mut rows = 0;
    for table in tables {
        rows += table.rows.len();
    }
    rows
}

/// The rows of each table `tables` gives, summed in one pass, as `sum`,
/// `count` and `for_each` go.
fn rows_by_fold(tables: impl Iterator<Item = &'static Table>) -> usize {
    tables.fold(0, |rows, table| rows + table.rows.len())
}

/// The rows of each table `tables` gives, collected into a vector, as
/// `collect` sizes it, then summed.
fn rows_by_collect(tables: impl Iterator<Item = &'static Table>) -> usize {
    let rows: Vec<usize> = tables.map(|table| table.rows.len()).collect();
    rows.iter().sum()
}

/// A walk over a model's tables, which sums their rows.
type Walk = fn(&'static Grammar) -> usize;

fn tables_group(c: &mut Criterion) {
    let model = Grammar::embedded();
    let walks: [(&str, Walk); 3] = [
        ("next", |model| rows_by_next(model.tables())),
        ("fold", |model| rows_by_fold(model.tables())),
        ("collect", |model| rows_by_collect(model.tables())),
    ];
    assert_eq!(
        model.tables().count(),
        TABLES.0,
        "tables: the model's tables"
    );
    for (name, walk) in walks {
        assert_eq!(walk(model), TABLES.1, "tables/{name}: their rows");
    }
    let mut group = c.benchmark_group("tables");
    group.throughput(Throughput::Elements(TABLES.0 as u64));
    for (name, walk) in walks {
        group.bench_function(name, |b| b.iter(|| walk(black_box(model))));
    }
    group.finish();
}

/// The tables evaluated, whether an object of each is a stream, and what
/// their rows' answers come to ([`Tally`]).
const EVALUATED: [(&str, bool, Tally); 4] = [
    ("PageObject", false, [9, 28, 1, 0, 4, 31, 0, 34, 0, 0]),
    ("Catalog", false, [2, 30, 0, 1, 4, 28, 0, 32, 0, 0]),
    ("FontType1", false, [7, 4, 0, 0, 1, 10, 0, 10, 0, 0]),
    ("XObjectImage", true, [13, 35, 2, 0, 3, 38, 0, 30, 4, 0]),
];

/// How many answers were of each kind: a [`Truth`] true, false and
/// unknown (Required and the predicates); a [`Reference`] direct,
/// indirect, either and unknown; and a [`Since`] a version, an extension
/// and unknown.
type Tally = [usize; 10];

fn truth(answer: &Truth) -> usize {
    match answer {
        Truth::True => 0,
        Truth::False => 1,
        Truth::Unknown(_) => 2,
    }
}

fn reference(answer: &Reference) -> usize {
    match answer {
        Reference::Direct => 3,
        Reference::Indirect => 4,
        Reference::Either => 5,
        Reference::Unknown(_) => 6,
    }
}

fn since(answer: &Since) -> usize {
    match answer {
        Since::In(_) => 7,
        Since::Extension(_) => 8,
        Since::Unknown(_) => 9,
    }
}

/// A value of `row`'s first type: the first of its possible values that
/// isn't a predicate, where it's a name or a number, else one of the
/// type's own.
fn value(row: &Row) -> Object {
    let given = (row.possible_values.first().into_iter().flatten())
        .map(|value| value.item.as_str())
        .find(|item| !item.starts_with("fn:"));
    let Some(ty) = row.r#type.first().map(|ty| ty.item) else {
        return Object::null();
    };
    match ty {
        ArlType::Array => Object::from(Vec::new()),
        ArlType::Bitmask | ArlType::Integer => {
            Object::from(given.and_then(|item| item.parse::<i64>().ok()).unwrap_or(0))
        }
        ArlType::Boolean => Object::from(true),
        ArlType::Date => Object::bytes(*b"D:20261114120000Z"),
        ArlType::Dictionary | ArlType::NameTree | ArlType::NumberTree => Dict::new().into(),
        ArlType::Matrix => numbers(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        ArlType::Name => Object::name(given.unwrap_or("X")),
        ArlType::Null => Object::null(),
        ArlType::Number => Object::from(given.and_then(|item| item.parse().ok()).unwrap_or(1.0)),
        ArlType::Rectangle => numbers(&[0.0, 0.0, 612.0, 792.0]),
        ArlType::Stream => Object::stream(Dict::new()),
        ArlType::StringText => Object::text("text"),
        _ => Object::bytes(*b"bytes"),
    }
}

/// An array of `numbers`.
fn numbers(numbers: &[f64]) -> Object {
    numbers
        .iter()
        .map(|&n| Object::from(n))
        .collect::<Vec<_>>()
        .into()
}

/// An object of `table`'s kind, a dictionary or a stream's, with a value of
/// its first type for each of its rows' keys but the wildcard.
fn object(table: &Table, stream: bool) -> Object {
    let keys = table.rows.iter().filter(|row| row.key != "*");
    let dict: Dict = keys.map(|row| (row.key.as_bytes(), value(row))).collect();
    match stream {
        true => Object::stream(dict),
        false => dict.into(),
    }
}

/// What every row of `table` gives in `container`, through `file`.
fn evaluated(file: &FileCx, table: &Table, container: &Object) -> Tally {
    let mut tally = [0; 10];
    for row in &table.rows {
        let cx = file.at(container, at(row));
        tally[truth(&predicate::required(&row.required, &cx))] += 1;
        for cell in &row.indirect_reference {
            tally[reference(&predicate::reference(cell, &cx))] += 1;
        }
        for text in row.special_case.iter().filter(|text| !text.is_empty()) {
            tally[truth(&predicate::holds_fully(text, &cx))] += 1;
        }
        let values = row.possible_values.iter().flatten();
        for value in values.filter(|value| value.item.starts_with("fn:")) {
            tally[truth(&predicate::holds_fully(&value.item, &cx))] += 1;
        }
        tally[since(&file.since(&row.since_version))] += 1;
    }
    tally
}

/// A file of nothing but a catalog, object 1, which every object it's
/// asked for resolves in.
fn catalog() -> (Dict, BTreeMap<ObjectId, Object>) {
    let trailer = dict! { "Root" => Object::reference(1) };
    let catalog = dict! { "Type" => Object::name("Catalog") }.into();
    (trailer, BTreeMap::from([(ObjectId::new(1, 0), catalog)]))
}

fn evaluate_group(c: &mut Criterion) {
    let model = Grammar::embedded();
    let (trailer, objects) = catalog();
    let resolve = &objects;
    let version = Version::new(2, 0);
    let mut group = c.benchmark_group("evaluate");
    for (name, stream, expected) in EVALUATED {
        let table = model
            .table(name)
            .unwrap_or_else(|| panic!("evaluate/{name}: the table"));
        let container = object(table, stream);
        let given = FileCx::new(&trailer, version, resolve).grammar(model);
        let alone = FileCx::new(&trailer, version, resolve);
        let tally = evaluated(&given, table, &container);
        assert_eq!(tally, expected, "evaluate/{name}: the answers' tally");
        let tally = evaluated(&alone, table, &container);
        assert_eq!(tally, expected, "evaluate/{name}: without the model");
        group.throughput(Throughput::Elements(expected.iter().sum::<usize>() as u64));
        group.bench_function(format!("{name}/given"), |b| {
            b.iter(|| evaluated(&given, table, black_box(&container)))
        });
        group.bench_function(format!("{name}/alone"), |b| {
            b.iter(|| evaluated(&alone, table, black_box(&container)))
        });
    }
    group.finish();
}

/// How many keys the name tree holds, and in how many leaves.
const KEYS: usize = 10_000;
const LEAVES: usize = 100;

/// The `n`th key of the tree.
fn key(n: usize) -> Vec<u8> {
    format!("dest-{n:05}").into_bytes()
}

/// A file whose catalog's `/Names` has a `/Dests` tree of [`KEYS`] keys in
/// [`LEAVES`] leaves under one root, each key's destination a page, as a
/// writer lays them out: the catalog (1), its `/Names` (2), the root (3),
/// the leaves (4 on), and the page.
fn dests() -> (Dict, BTreeMap<ObjectId, Object>) {
    let trailer = dict! { "Root" => Object::reference(1) };
    let first = 4;
    let page = first + LEAVES as u32;
    let kids: Vec<Object> = (0..LEAVES as u32)
        .map(|leaf| Object::reference(first + leaf))
        .collect();
    let mut objects = BTreeMap::from([
        (
            ObjectId::new(1, 0),
            dict! { "Type" => Object::name("Catalog"), "Names" => Object::reference(2) }.into(),
        ),
        (
            ObjectId::new(2, 0),
            dict! { "Dests" => Object::reference(3) }.into(),
        ),
        (ObjectId::new(3, 0), dict! { "Kids" => kids }.into()),
        (
            ObjectId::new(page, 0),
            dict! { "Type" => Object::name("Page") }.into(),
        ),
    ]);
    let per = KEYS / LEAVES;
    for leaf in 0..LEAVES {
        let keys = leaf * per..(leaf + 1) * per;
        let limits = vec![
            Object::bytes(key(keys.start)),
            Object::bytes(key(keys.end - 1)),
        ];
        let mut names = Vec::with_capacity(2 * per);
        for n in keys {
            names.push(Object::bytes(key(n)));
            let fit = vec![Object::reference(page), Object::name("Fit")];
            names.push(fit.into());
        }
        let node = dict! { "Limits" => limits, "Names" => names };
        objects.insert(ObjectId::new(first + leaf as u32, 0), node.into());
    }
    (trailer, objects)
}

/// A go-to action to the destination named `key`.
fn goto(key: Vec<u8>) -> Object {
    dict! { "Type" => Object::name("Action"), "S" => Object::name("GoTo"), "D" => Object::bytes(key) }.into()
}

/// How many of `actions` name a destination in the tree, by `cell`.
fn named(file: &FileCx, cell: &str, actions: &[Object]) -> usize {
    let held = |action| predicate::holds_fully(cell, &file.at(action, At::Key(b"D")));
    actions
        .iter()
        .filter(|&action| held(action) == Truth::True)
        .count()
}

fn trees_group(c: &mut Criterion) {
    let model = Grammar::embedded();
    let row = model
        .table("ActionGoTo")
        .and_then(|table| table.row(At::Key(b"D")))
        .unwrap_or_else(|| panic!("trees: ActionGoTo's /D"));
    let cell = (row.possible_values.get(2).and_then(|values| values.first()))
        .map(|value| value.item.as_str())
        .unwrap_or_else(|| panic!("trees: /D's string's possible value"));
    assert_eq!(
        cell, "fn:Eval(fn:InNameTree(trailer::Catalog::Names::Dests))",
        "trees: /D's string's possible value"
    );
    let (trailer, objects) = dests();
    let resolve = &objects;
    let version = Version::new(2, 0);
    let actions: Vec<Object> = (0..KEYS).map(|n| goto(key(n))).collect();
    let file = FileCx::new(&trailer, version, resolve).grammar(model);
    assert_eq!(named(&file, cell, &actions), KEYS, "trees: every key");
    let missing = [goto(b"dest-none".to_vec())];
    assert_eq!(named(&file, cell, &missing), 0, "trees: a key in none");
    let mut group = c.benchmark_group("trees");
    group.throughput(Throughput::Elements(KEYS as u64));
    group.bench_function("first", |b| {
        b.iter(|| {
            let file = FileCx::new(&trailer, version, resolve).grammar(model);
            named(&file, cell, black_box(&actions[..1]))
        })
    });
    group.bench_function("kept", |b| {
        b.iter(|| named(&file, cell, black_box(&actions)))
    });
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    parse_group(&mut criterion);
    rows_group(&mut criterion);
    tables_group(&mut criterion);
    evaluate_group(&mut criterion);
    trees_group(&mut criterion);
    criterion.final_summary();
}
