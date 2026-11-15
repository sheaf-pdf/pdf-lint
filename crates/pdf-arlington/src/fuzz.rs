//! What the fuzz target checks, which unit tests check too: the promises the
//! loader makes of any tables, and the evaluator of any predicate over any
//! objects. It's compiled only for tests and under cargo-fuzz's
//! `--cfg fuzzing`.
//!
//! Neither body asks anything through [`Grammar::embedded`]: what that
//! model keeps of the predicates asked through it is kept for the process,
//! first come, so texts a body asked there would take the room its rows'
//! own predicates have, and the crate's tests of that room read it. Each
//! body asks through a model of its own, made for its input.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use pdf_objects::parse::Parser;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Unreadable, Value, Version, Why, resolve_fn};
use sha2::{Digest as _, Sha256};

use crate::grammar::{ArlType, At, Digest, Error, File, Grammar, Row, Table, Wrapped, version};
use crate::predicate::{
    self, Cx, DEPTH, Expr, FileCx, Latin, Limits, Reference, SUPPORTED, Since, Truth,
};

/// How many pieces of an input are read as tables: more than the embedded
/// model has.
const TABLES: usize = 1024;

/// How many values of an input are read as objects.
const OBJECTS: usize = 256;

/// How many failed starts an input may have, each a byte on from the last.
const STARTS: usize = 512;

/// How many keys or elements of a container a predicate is asked at.
const PLACES: usize = 12;

/// How many texts fill the predicates' model's memo, when the input says
/// to: as many as it has room for, a model of one table's.
const ROOM: usize = 32;

/// The versions a predicate's file may be at.
const VERSIONS: [(u8, u8); 9] = [
    (1, 0),
    (1, 1),
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
    (1, 7),
    (2, 0),
];

/// Extensions a predicate's file may be checked under: some the model
/// names, and one it doesn't.
const EXTENSIONS: [&str; 7] = [
    "ADBE_Extn3",
    "ISO_21812",
    "ISO_19005_3",
    "ETSI_PAdES",
    "ISO_TS_32001",
    "ISO_TS_32004",
    "Nonesuch",
];

/// The table of the predicates' model: a few keys the model's predicates
/// read, with defaults, and any other.
const TABLE: &str = "Key\tType\tSinceVersion\tDeprecatedIn\tRequired\tIndirectReference\t\
                     Inheritable\tDefaultValue\tPossibleValues\tSpecialCase\tLink\tNote\n\
                     Type\tname\t1.0\t\tFALSE\tFALSE\tFALSE\t\t\t\t\t\n\
                     Subtype\tname\t1.0\t\tFALSE\tFALSE\tFALSE\t\t\t\t\t\n\
                     S\tname\t1.0\t\tFALSE\tFALSE\tFALSE\tGoTo\t\t\t\t\n\
                     N\tinteger\t1.0\t\tFALSE\tFALSE\tFALSE\t1\t\t\t\t\n\
                     ImageMask\tboolean\t1.0\t\tFALSE\tFALSE\tFALSE\tfalse\t\t\t\t\n\
                     Rotate\tinteger\t1.0\t\tFALSE\tFALSE\tFALSE\t0\t\t\t\t\n\
                     Flags\tbitmask\t1.0\t\tFALSE\tFALSE\tFALSE\t0\t\t\t\t\n\
                     Filter\tname;array\t1.0\t\tFALSE\tFALSE\tFALSE\t[];[]\t\t\t\t\n\
                     *\tstring\t1.0\t\tFALSE\tFALSE\tFALSE\t\t\t\t\t\n";

/// `data` as tables' files: split at each `\0`, each piece's first line its
/// table's name and the rest its file's text, each read as UTF-8, a byte
/// that isn't read as U+FFFD; the first piece of a name alone, as a folder
/// holds one file of a name.
fn pieces(data: &[u8]) -> Vec<(String, String)> {
    let mut names = BTreeSet::new();
    let mut pieces = Vec::new();
    for piece in data.split(|&b| b == 0).take(TABLES) {
        let (name, text) = match piece.iter().position(|&b| b == b'\n') {
            Some(at) => (&piece[..at], &piece[at + 1..]),
            None => (piece, &[][..]),
        };
        let name = String::from_utf8_lossy(name).into_owned();
        if names.insert(name.clone()) {
            pieces.push((name, String::from_utf8_lossy(text).into_owned()));
        }
    }
    pieces
}

/// The loader's files of `pieces`, in their order, each called as the
/// embedded model's are: its name and `.tsv`.
fn files(pieces: &[(String, String)]) -> impl DoubleEndedIterator<Item = File<'_>> {
    pieces.iter().map(|(name, text)| File {
        name: name.clone(),
        label: format!("{name}.tsv").into(),
        text: Cow::Borrowed(text),
    })
}

/// What [`Grammar::hash`] says it's of, built here: each piece's name,
/// `.tsv`, a newline and its text, in byte order of `<name>.tsv`, as
/// `LC_ALL=C ls` lists a folder's files (so `a-b.tsv` before `a.tsv`, where
/// `a` comes before `a-b`).
fn framing(pieces: &[(String, String)]) -> Vec<u8> {
    let mut files: Vec<(String, &str)> = (pieces.iter())
        .map(|(name, text)| (format!("{name}.tsv"), text.as_str()))
        .collect();
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut bytes = Vec::new();
    for (file, text) in files {
        bytes.extend_from_slice(file.as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(text.as_bytes());
    }
    bytes
}

/// The fuzz target's body for the model's tables: `data` as tables' files
/// ([`pieces`]), loaded as a folder of them is, and held to the loader's
/// promises: it never panics; a model that doesn't load says which file,
/// and a line within it where it gives one; a table whose name holds a tab
/// or a line break is refused, the first by `<name>.tsv`, and no other
/// table is refused so; one that loads loads the same from its files in
/// any order, the same tables of the same rows; its hash is of the framing
/// [`Grammar::hash`] gives, built here from the pieces; every row is where
/// [`Table::row`] says, at its key and at each index;
/// every predicate in its cells parses or says why not, and one that
/// parses prints back as a text that parses to it, nested no deeper than
/// the parser goes; and every row's cells, asked as a checker asks them
/// over an object of its table's keys, say the same through a [`FileCx`]
/// given the model and one given none, and asked again.
pub fn tables(data: &[u8]) {
    loaded(data, &mut |_| {});
}

/// [`tables`], telling `asked` each predicate it asks through the model it
/// loads, and giving that model's hash.
fn loaded(data: &[u8], asked: &mut dyn FnMut(&str)) -> Option<Digest> {
    let pieces = pieces(data);
    let read = Grammar::from_files(files(&pieces));
    let backwards = Grammar::from_files(files(&pieces).rev());
    match (&read, &backwards) {
        (Ok(model), Ok(other)) => alike(model, other),
        (Err(error), Err(other)) => assert_eq!(error.to_string(), other.to_string()),
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => {
            panic!("the files' order decides whether they load: {error}")
        }
    }
    if refuses_a_name(&read, &pieces) {
        return None;
    }
    let model = match read {
        Ok(model) => model,
        Err(error) => {
            says_where(&error, &pieces);
            return None;
        }
    };
    let framed: [u8; 32] = Sha256::digest(framing(&pieces)).into();
    assert_eq!(
        model.hash().as_bytes(),
        &framed,
        "the hash isn't of the framing"
    );
    for table in model.tables() {
        found(table);
    }
    for text in texts(&model) {
        prints_back(text);
    }
    evaluated(&model, asked);
    Some(model.hash())
}

/// Whether two models are alike: of one hash, and the same tables of the
/// same rows.
fn alike(model: &Grammar, other: &Grammar) {
    assert_eq!(model.hash(), other.hash(), "one model, two hashes");
    let each = |model: &Grammar| -> Vec<(String, Vec<Row>, usize)> {
        let tables = model.tables();
        tables
            .map(|t| (t.name.clone(), t.rows.clone(), t.repeating().len()))
            .collect()
    };
    assert!(each(model) == each(other), "one model, two readings");
}

/// Whether `read` refused a table's name, held to the loader's promise: it
/// refuses the first piece whose name holds a tab, a line feed or a
/// carriage return, by the bytes of `<name>.tsv`, where any does, naming
/// its file, and refuses no name where none does.
fn refuses_a_name(read: &Result<Grammar, Error>, pieces: &[(String, String)]) -> bool {
    let first = (pieces.iter())
        .filter(|(name, _)| name.contains(['\t', '\n', '\r']))
        .map(|(name, _)| format!("{name}.tsv"))
        .min();
    match (first, read) {
        (Some(first), Err(Error::Separator { path })) => {
            assert_eq!(path.to_str(), Some(first.as_str()), "another name refused");
            true
        }
        (Some(first), _) => panic!("{first:?} isn't refused, a name the hash can't frame"),
        (None, Err(Error::Separator { path })) => panic!("{path:?} is refused, a name it frames"),
        (None, _) => false,
    }
}

/// That `error`, of a load in memory, is a table's, naming its piece's file,
/// and a line within it when it gives one, in its words as in its fields.
fn says_where(error: &Error, pieces: &[(String, String)]) {
    let Error::Table { file, line, .. } = error else {
        panic!("a load in memory read a folder: {error}");
    };
    let label = file.to_str().expect("a piece's name is UTF-8");
    let piece = pieces
        .iter()
        .find(|(name, _)| format!("{name}.tsv") == label);
    let (_, text) = piece.unwrap_or_else(|| panic!("{label} is no piece's: {error}"));
    let mut said = format!("{label}: ");
    if let Some(line) = line {
        let lines = text.split_terminator('\n').count();
        assert!(
            (1..=lines).contains(line),
            "line {line} of {lines}: {error}"
        );
        said.push_str(&format!("line {line}: "));
    }
    let words = error.to_string();
    assert!(words.starts_with(&said), "{words:?} isn't after {said:?}");
}

/// Whether a key is a repeating row's: digits, then `*`.
fn repeating(key: &str) -> bool {
    key.strip_suffix('*')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// That each row of `table` is where [`Table::row`] says: at its key; and
/// at each index from 0 to past its repeating rows, the row of that index's
/// key, else the repeating row it falls on, else the wildcard `*`.
fn found(table: &Table) {
    let rows = &table.rows;
    for row in rows {
        let at = table.row(At::Key(row.key.as_bytes()));
        let at_key = at.is_some_and(|at| std::ptr::eq(at, row));
        assert!(at_key, "{}: {} isn't found at its key", table.name, row.key);
    }
    let first = (rows.iter().position(|r| repeating(&r.key))).unwrap_or(rows.len());
    let set = &rows[first..];
    assert!(std::ptr::eq(table.repeating(), set), "{}", table.name);
    let wild = rows.iter().find(|r| r.key == "*");
    for i in 0..=rows.len() + set.len() {
        let named = rows.iter().find(|r| r.key == i.to_string());
        let repeated = (!set.is_empty() && i >= first).then(|| &set[(i - first) % set.len()]);
        let row = named.or(repeated).or(wild);
        let at = table.row(At::Index(i));
        let same = match (at, row) {
            (Some(at), Some(row)) => std::ptr::eq(at, row),
            (at, row) => at.is_none() && row.is_none(),
        };
        assert!(same, "{}: element {i} at the wrong row", table.name);
    }
}

/// Every predicate a checker asks of `model`'s rows' cells, as the model
/// splits them: each SinceVersion but a version, each Required and
/// IndirectReference but `TRUE` and `FALSE`, each SpecialCase, and each
/// PossibleValues item, the version predicates around it taken off.
fn texts(model: &Grammar) -> BTreeSet<&str> {
    let rows = model.tables().flat_map(|table| &table.rows);
    rows.flat_map(texts_of).collect()
}

/// The predicates of `row`'s cells, as [`texts`] has them.
fn texts_of(row: &Row) -> impl Iterator<Item = &str> {
    let asked = |cell: &&String| !matches!(cell.as_str(), "TRUE" | "FALSE");
    let since = Some(&row.since_version).filter(|cell| version(cell).is_none());
    let values = row.possible_values.iter().flatten();
    (since.into_iter())
        .chain(Some(&row.required).filter(asked))
        .chain(row.indirect_reference.iter().filter(asked))
        .chain(&row.special_case)
        .chain(values.map(|value| &value.item))
        .map(String::as_str)
}

/// How many terms deep `e` nests, as the parser counts them: a bracket, a
/// call and a value each one, inside those around it.
fn terms(e: &Expr) -> usize {
    match e {
        Expr::Group(inner) => 1 + terms(inner),
        Expr::Call(_, args) => 1 + args.iter().map(terms).max().unwrap_or(0),
        Expr::And(items) | Expr::Or(items) => items.iter().map(terms).max().unwrap_or(0),
        Expr::Cmp(_, a, b) => terms(a).max(terms(b)),
        Expr::Arith(first, rest) => (rest.iter()).fold(terms(first), |n, (_, e)| n.max(terms(e))),
        _ => 1,
    }
}

/// `text` parsed: an error that says the predicate first, or an expression
/// that prints back as a text that parses to it, nested no deeper than the
/// parser goes.
fn prints_back(text: &str) {
    match predicate::parse(text) {
        Ok(expr) => {
            let printed = expr.to_string();
            let again = predicate::parse(&printed);
            assert!(
                again.as_ref() == Ok(&expr),
                "{text:?} prints as {printed:?}"
            );
            assert!(terms(&expr) <= DEPTH, "{text:?} nests past the cap");
        }
        Err(error) => {
            let words = error.to_string();
            let said = format!("{text:?}: ");
            assert!(words.starts_with(&said), "{words:?} isn't after {said:?}");
        }
    }
}

/// What one of the evaluator's functions said.
#[derive(Debug, PartialEq)]
enum Said {
    /// [`predicate::required`], [`predicate::holds`] or
    /// [`predicate::holds_fully`].
    Truth(Truth),
    /// [`predicate::reference`].
    Reference(Reference),
    /// [`FileCx::since`] or [`predicate::since`].
    Since(Since),
    /// [`predicate::required_value`].
    Value(Option<(Truth, String)>),
    /// [`predicate::ignored`].
    Ignored(Option<Truth>),
}

impl Said {
    /// That where it can't be told, it says why.
    fn reasoned(&self) {
        let why = match self {
            Said::Truth(Truth::Unknown(why))
            | Said::Reference(Reference::Unknown(why))
            | Said::Since(Since::Unknown(why))
            | Said::Value(Some((Truth::Unknown(why), _)))
            | Said::Ignored(Some(Truth::Unknown(why))) => why,
            _ => return,
        };
        assert!(!why.is_empty(), "unknown, for no reason: {self:?}");
    }
}

/// A value of `row`'s first type: a name or an integer its first
/// PossibleValues item when that's one, else one of the type's own; a
/// stream a dictionary with a `/Length`, as [`values`] reads one.
fn of_type(row: &Row) -> Object {
    let item = row.possible_values.first().and_then(|items| items.first());
    let constant = item
        .map(|item| item.item.as_str())
        .filter(|v| !v.starts_with("fn:"));
    let numbers =
        |numbers: &[i64]| Object::from(numbers.iter().map(|&n| n.into()).collect::<Vec<_>>());
    match row.r#type.first().map(|t| t.item) {
        Some(ArlType::Name) => Object::name(constant.unwrap_or("X")),
        Some(ArlType::Integer | ArlType::Bitmask) => {
            Object::from(constant.and_then(|v| v.parse().ok()).unwrap_or(1_i64))
        }
        Some(ArlType::Number) => Object::from(constant.and_then(|v| v.parse().ok()).unwrap_or(1.5)),
        Some(ArlType::Boolean) => Object::from(constant != Some("false")),
        Some(ArlType::Array) => numbers(&[0, 1]),
        Some(ArlType::Matrix) => numbers(&[1, 0, 0, 1, 0, 0]),
        Some(ArlType::Rectangle) => numbers(&[0, 0, 612, 792]),
        Some(ArlType::Dictionary | ArlType::NameTree | ArlType::NumberTree) => Dict::new().into(),
        Some(ArlType::Stream) => Object::from(Dict::from_iter([("Length", 0)])),
        Some(ArlType::Date) => Object::bytes("D:20261114000000Z"),
        Some(ArlType::String | ArlType::StringAscii | ArlType::StringByte) => Object::bytes("a"),
        Some(ArlType::StringText) => Object::text("a"),
        Some(ArlType::Null) | None => Object::null(),
    }
}

/// The element a row of an array is, a repeating row's first.
fn index(row: &Row) -> Option<usize> {
    let n = row.key.strip_suffix('*').unwrap_or(&row.key);
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .then(|| n.parse().ok())
        .flatten()
}

/// An object of `table`'s kind: an array of an element for each row, when
/// it has rows of elements or is named as the model names an array's
/// (`ArrayOf`), else a dictionary of each of its keys but `*`, each value of
/// its row's first type.
fn made(table: &Table) -> Object {
    let array = table.name.starts_with("ArrayOf");
    if array || table.rows.iter().any(|row| index(row).is_some()) {
        return Object::from(table.rows.iter().map(of_type).collect::<Vec<_>>());
    }
    let keys = table.rows.iter().filter(|row| row.key != "*");
    keys.map(|row| (row.key.as_bytes(), of_type(row)))
        .collect::<Dict>()
        .into()
}

/// What each of the evaluator's functions says of each text of `row`'s
/// cells, as a checker asks them, at `at` in `container`: its SinceVersion
/// through the file; its Required and each IndirectReference, with no
/// defaults; each SpecialCase but an empty one, which asks nothing, by
/// [`predicate::holds`], [`predicate::holds_fully`] and
/// [`predicate::ignored`], and each PossibleValues item by
/// [`predicate::required_value`] and [`predicate::holds_fully`], with the
/// table's defaults.
fn said_of(row: &Row, file: &FileCx, container: &Object, at: At, table: &Table) -> Vec<Said> {
    let cx = file.at(container, at);
    let with = cx.defaults(Some(table));
    let mut said = vec![
        Said::Since(file.since(&row.since_version)),
        Said::Truth(predicate::required(&row.required, &cx)),
    ];
    for text in &row.indirect_reference {
        said.push(Said::Reference(predicate::reference(text, &cx)));
    }
    for text in row.special_case.iter().filter(|text| !text.is_empty()) {
        said.push(Said::Truth(predicate::holds(text, &with)));
        said.push(Said::Truth(predicate::holds_fully(text, &with)));
        said.push(Said::Ignored(predicate::ignored(text, &with)));
    }
    for item in row.possible_values.iter().flatten() {
        said.push(Said::Value(predicate::required_value(&item.item, &with)));
        said.push(Said::Truth(predicate::holds_fully(&item.item, &with)));
    }
    said
}

/// Every row of every table of `model` asked as [`said_of`] asks it, in an
/// object of its table [`made`], at its key, or in an array at the element
/// made of it, at version 2.0 under every extension the model names: the
/// same through a [`FileCx`] given the model, asked twice, and through one
/// given none, and every unknown with its reason.
fn evaluated(model: &Grammar, asked: &mut dyn FnMut(&str)) {
    for text in texts(model) {
        asked(text);
    }
    let trailer = Dict::new();
    let nothing = resolve_fn(|_: ObjectId| None::<&Object>);
    let extensions = model.extensions();
    let v2 = Version::new(2, 0);
    let given = (FileCx::new(&trailer, v2, &nothing).extensions(&extensions)).grammar(model);
    let alone = FileCx::new(&trailer, v2, &nothing).extensions(&extensions);
    for table in model.tables() {
        let container = made(table);
        for (i, row) in table.rows.iter().enumerate() {
            let at = match container.as_array() {
                Some(_) => At::Index(i),
                None => At::Key(row.key.as_bytes()),
            };
            let first = said_of(row, &given, &container, at, table);
            let again = said_of(row, &given, &container, at, table);
            assert!(again == first, "{} {}: asked twice", table.name, row.key);
            let without = said_of(row, &alone, &container, at, table);
            assert!(
                without == first,
                "{} {}: given the model",
                table.name,
                row.key
            );
            first.iter().for_each(Said::reasoned);
        }
    }
}

/// `data` as a series of values, the `n`th of them object `n`, read by
/// pdf-objects' parser, a dictionary with a `/Length` as a stream with no
/// data, as no value's a stream by itself; a value that doesn't read is
/// passed a byte at a time.
fn values(data: &[u8]) -> BTreeMap<ObjectId, Object> {
    let mut objects = BTreeMap::new();
    let (mut at, mut failed) = (0, 0);
    let mut next = 1;
    while at < data.len() && objects.len() < OBJECTS && failed < STARTS {
        let mut parser = Parser::at(data, at);
        match parser.value(&mut ()) {
            Ok(value) => {
                let stream = (value.as_dict())
                    .filter(|dict| dict.has(b"Length"))
                    .map(|dict| {
                        let mut stream = Object::stream(dict.clone());
                        if let Some(span) = value.at() {
                            stream = stream.with_span(span);
                        }
                        stream
                    });
                objects.insert(ObjectId::new(next, 0), stream.unwrap_or(value));
                next += 1;
                at = parser.offset();
            }
            Err(_) => {
                failed += 1;
                at += 1;
            }
        }
    }
    objects
}

/// A resolver over an input's objects that fails each number `fails`
/// marks, object `n` by bit `n % 8`, for `why`, as a reader fails one it
/// can't read; none when there's no `why`.
#[derive(Clone, Copy)]
struct Failing<'o> {
    objects: &'o BTreeMap<ObjectId, Object>,
    why: Option<Why>,
    fails: u8,
}

impl<'o> Resolve<'o> for Failing<'o> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.try_resolve(id).ok().flatten()
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        match self.why {
            Some(why) if (self.fails >> (id.number % 8)) & 1 == 1 => {
                Err(Unreadable::new(id, why, "made to fail"))
            }
            _ => Ok(self.objects.get(&id)),
        }
    }
}

/// The keys and elements of `container` a predicate is asked at: each of a
/// dictionary's or a stream's keys, or each of an array's elements and one
/// past them, to [`PLACES`]; and a key it hasn't.
fn places(container: &Object) -> Vec<At<'_>> {
    let dict = (container.as_dict()).or_else(|| container.as_stream().map(|s| &s.dict));
    let mut places: Vec<At> = match (dict, container.as_array()) {
        (Some(dict), _) => dict
            .iter()
            .map(|(key, _)| At::Key(key))
            .take(PLACES)
            .collect(),
        (None, Some(array)) => (0..=array.len().min(PLACES)).map(At::Index).collect(),
        (None, None) => vec![At::Index(0)],
    };
    places.push(At::Key(b"Nonesuch"));
    places
}

/// The row at `at` in `container`, through `parents`, with `defaults`, the
/// container object `id` where that's given.
fn placed<'c, 'a>(
    file: &'c FileCx<'a>,
    container: &'c Object,
    at: At<'c>,
    parents: &'c [&'c Object],
    defaults: Option<&'c Table>,
    id: Option<ObjectId>,
) -> Cx<'c, 'a> {
    let cx = file.at(container, at).parents(parents).defaults(defaults);
    match id {
        Some(id) => cx.id(id),
        None => cx,
    }
}

/// What each of the evaluator's functions says of `text` in `cx`.
fn said(text: &str, cx: &Cx) -> [Said; 6] {
    [
        Said::Truth(predicate::required(text, cx)),
        Said::Truth(predicate::holds(text, cx)),
        Said::Truth(predicate::holds_fully(text, cx)),
        Said::Reference(predicate::reference(text, cx)),
        Said::Value(predicate::required_value(text, cx)),
        Said::Ignored(predicate::ignored(text, cx)),
    ]
}

/// The fuzz target's body for the evaluator: `data` as a predicate's text,
/// to its first `\0`, read as UTF-8, a byte that isn't read as U+FFFD; then
/// three bytes; then a series of values ([`values`]), object 1 the
/// container and object 2 the trailer. The first byte marks the objects a
/// resolver fails to read, object `n` by bit `n % 8`; the second's low
/// three bits say why (none, damaged, capped, locked, or the source), its
/// next four the file's version, its top bit that the file's length is
/// known; the third's low two bits how many of objects 3, 4 and 5 are the
/// container's parents, nearest first, and its next bits, in turn, that
/// the model's defaults stand in, that the container is object 1, that the
/// file's pages are its page dictionaries, that its font descriptors' fonts
/// are said to have Latin characters by their numbers' oddness, that the
/// model's memo is full before the text's asked, and that the file's
/// checked under a few extensions.
///
/// The promises: it never panics or overflows the stack, a text nested past
/// the parser's cap being an error; the text parses as [`tables`] holds a
/// cell to; and every one of the evaluator's functions, at every key and
/// element of the container, says the same through a [`FileCx`] given the
/// body's own model and one given none, asked twice of each, what a
/// [`FileCx`] keeps read once; and every unknown says why. Where the text
/// is one `fn:InNameTree` call, a key the tree holds over a resolver that
/// gives every object is never said to be missing over the input's, which
/// fails what the input marks, where the row's own value, and the
/// references its entry goes through, read alike over both: a part of the
/// tree, or of the way to it, that doesn't read leaves a key it may hold
/// unknown. The row's own value that doesn't read is null, as an absent
/// row is, which the function says is in no tree. And where the text is
/// one call of a function that reads a tree (a name tree, the form's
/// fields, the structure tree's content items), under smaller caps than
/// the default, false only where the default's answer is false too, as a
/// cap met leaves the tree not read whole; for a name tree, true never
/// where the default's is false either. A field or a structure element
/// reached by two paths is read by the first, so under a cap past which
/// one path isn't read, a name or a page another path gives may be found
/// where the default doesn't find it.
pub fn predicates(data: &[u8]) {
    asking(data, &mut |_| {});
}

/// [`predicates`], telling `asked` each text it asks through its model.
fn asking(data: &[u8], asked: &mut dyn FnMut(&str)) {
    let (text, rest) = match data.iter().position(|&b| b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &[][..]),
    };
    let text = String::from_utf8_lossy(text);
    let byte = |i: usize| rest.get(i).copied().unwrap_or(0);
    let (fails, how, picks) = (byte(0), byte(1), byte(2));
    let objects = values(rest.get(3..).unwrap_or_default());
    prints_back(&text);

    let model = Grammar::from_files([File {
        name: "Fuzzed".to_string(),
        label: "Fuzzed.tsv".into(),
        text: TABLE.into(),
    }])
    .expect("the body's own table loads");
    let table = model.table("Fuzzed").expect("its one table");
    let empty = Object::from(Dict::new());
    let container = objects.get(&ObjectId::new(1, 0)).unwrap_or(&empty);
    let no_trailer = Dict::new();
    let trailer = (objects.get(&ObjectId::new(2, 0)))
        .and_then(Object::as_dict)
        .unwrap_or(&no_trailer);
    let why = [
        None,
        Some(Why::Damaged),
        Some(Why::Capped),
        Some(Why::Locked),
        Some(Why::Source),
    ];
    let resolve = Failing {
        objects: &objects,
        why: why[usize::from(how & 7) % why.len()],
        fails,
    };
    let (major, minor) = VERSIONS[usize::from((how >> 3) & 15) % VERSIONS.len()];
    let version = Version::new(major, minor);
    let size = (how & 0x80 != 0).then_some(data.len());
    let extensions: BTreeSet<String> = match picks & 0x80 {
        0 => BTreeSet::new(),
        _ => EXTENSIONS.iter().map(|e| e.to_string()).collect(),
    };
    let pages: Vec<&Dict> = (objects.values())
        .filter_map(|o| o.as_dict().or_else(|| o.as_stream().map(|s| &s.dict)))
        .filter(|dict| dict.has_type(b"Page"))
        .collect();
    let mut latin = Latin::default();
    for (id, object) in &objects {
        latin.insert(Some(*id), None, id.number % 2 == 1);
        latin.insert(None, object.at(), id.number % 2 == 1);
    }
    let file = |resolve, given: bool| {
        let file = FileCx::new(trailer, version, resolve)
            .size(size)
            .extensions(&extensions);
        let file = if picks & 0x10 != 0 {
            file.pages(&pages)
        } else {
            file
        };
        let file = if picks & 0x20 != 0 {
            file.latin(&latin)
        } else {
            file
        };
        if given { file.grammar(&model) } else { file }
    };
    let (given, alone) = (file(resolve, true), file(resolve, false));
    let parents: Vec<&Object> = (3..3 + u32::from(picks & 3))
        .filter_map(|n| objects.get(&ObjectId::new(n, 0)))
        .collect();
    let defaults = (picks & 4 != 0).then_some(table);
    let id = (picks & 8 != 0).then_some(ObjectId::new(1, 0));
    let cx = |file, at| placed(file, container, at, &parents, defaults, id);

    if picks & 0x40 != 0 {
        let nowhere = placed(&given, &empty, At::Key(b"Filler"), &[], None, None);
        for n in 0..ROOM {
            let filler = format!("@Filler=={n}");
            asked(&filler);
            let truth = predicate::holds(&filler, &nowhere);
            assert_eq!(truth, Truth::Unknown("@Filler".to_string()), "{filler}");
        }
    }
    asked(&text);
    let since = Said::Since(predicate::since(&text, &extensions));
    for file in [&given, &alone, &given, &alone] {
        assert!(
            Said::Since(file.since(&text)) == since,
            "{text:?}: its since"
        );
    }
    since.reasoned();
    for at in places(container) {
        let first = said(&text, &cx(&given, at));
        for file in [&alone, &given, &alone] {
            assert!(said(&text, &cx(file, at)) == first, "{text:?} at {at:?}");
        }
        first.iter().for_each(Said::reasoned);
    }

    let call = sole_call(&text);
    let function = match &call {
        Some(Expr::Call(f, _)) => f.as_str(),
        _ => "",
    };
    let of_a_path =
        matches!(&call, Some(Expr::Call(_, args)) if matches!(&args[..], [Expr::Path(_)]));
    if function == "fn:InNameTree" && of_a_path {
        let all = Failing {
            why: None,
            ..resolve
        };
        let whole = file(all, false);
        for at in places(container) {
            let over_all = placed(&whole, container, at, &parents, defaults, id);
            let over_some = placed(&alone, container, at, &parents, defaults, id);
            let own = |cx: &Cx| predicate::value_at(container, at, cx).map(std::ptr::from_ref);
            if predicate::holds(&text, &over_all) == Truth::True
                && own(&over_all) == own(&over_some)
            {
                assert!(
                    predicate::holds(&text, &over_some) != Truth::False,
                    "{text:?} at {at:?}: a key the tree holds said to be missing where some reads fail"
                );
            }
        }
    }

    if TREES.contains(&function) {
        for (depth, nodes) in SMALL {
            let mut limits = Limits::DEFAULT;
            (limits.tree_depth, limits.tree_nodes) = (depth, nodes);
            (limits.structure_depth, limits.structure_nodes) = (depth, nodes);
            let small = file(resolve, false).limits(limits);
            for at in places(container) {
                let by_default = predicate::holds(&text, &cx(&alone, at));
                let under = placed(&small, container, at, &parents, defaults, id);
                let under = predicate::holds(&text, &under);
                assert!(
                    under != Truth::False || by_default == Truth::False,
                    "{text:?} at {at:?}: false under {limits:?}, {by_default:?} by default"
                );
                assert!(
                    function != "fn:InNameTree"
                        || under != Truth::True
                        || by_default != Truth::False,
                    "{text:?} at {at:?}: a key held under {limits:?} is missing by default"
                );
            }
        }
    }
}

/// The functions that read one of a file's trees, under its [`Limits`].
const TREES: [&str; 4] = [
    "fn:InNameTree",
    "fn:IsFullyQualifiedFieldName",
    "fn:ImageIsStructContentItem",
    "fn:PageContainsStructContentItems",
];

/// The caps the predicates body reads the trees under beside the default:
/// each depth and nodes cap, for all three walks.
const SMALL: [(usize, usize); 4] = [(0, 0), (1, 2), (2, 4), (3, 8)];

/// The call `text` is, in brackets or `fn:Eval` or neither, as the model's
/// cells write one; none where it's something else, or doesn't parse.
fn sole_call(text: &str) -> Option<Expr> {
    let mut e = predicate::parse(text).ok()?;
    loop {
        e = match e {
            Expr::Group(inner) => *inner,
            Expr::Call(f, mut args) if f == "fn:Eval" && args.len() == 1 => args.remove(0),
            call @ Expr::Call(..) => return Some(call),
            _ => return None,
        };
    }
}

/// A seed of the predicates' body: its text, the three bytes after it,
/// and its objects.
fn seed(text: &str, bytes: [u8; 3], objects: &str) -> Vec<u8> {
    let mut seed = text.as_bytes().to_vec();
    seed.push(0);
    seed.extend_from_slice(&bytes);
    seed.extend_from_slice(objects.as_bytes());
    seed
}

/// `object` written as a value, each stream in it a dictionary with a
/// `/Length`, as [`values`] reads one back.
fn written(object: &Object) -> String {
    let mut object = object.clone();
    unstreamed(&mut object);
    let mut out = Vec::new();
    pdf_objects::serialize::write(&mut out, &object).expect("a made object writes");
    String::from_utf8(out).expect("a made object is ASCII")
}

/// `object` with each stream in it its dictionary.
fn unstreamed(object: &mut Object) {
    let value = object.value_mut();
    match value {
        Value::Stream(stream) => *value = Value::Dict(stream.dict.clone()),
        Value::Array(items) => items.iter_mut().for_each(unstreamed),
        Value::Dict(dict) => dict.iter_mut().for_each(|(_, v)| unstreamed(v)),
        _ => {}
    }
}

/// The objects a seed's predicate is asked over, after its container: a
/// trailer, a catalog naming an encryption dictionary's crypt filters, and
/// an empty page tree.
const PLAIN: &str = "<< /Root 3 0 R /Size 6 /Encrypt 4 0 R /ID [<00> <00>] >> \
                     << /Type /Catalog /Pages 5 0 R >> \
                     << /Filter /Standard /V 4 /CF << /StdCF << /CFM /AESV2 >> >> \
                        /StmF /StdCF /StrF /StdCF >> \
                     << /Type /Pages /Kids [] /Count 0 >>";

/// A go-to action whose destination is named in a name tree with a kid.
const NAMES: &str = "<< /S /GoTo /D (b) >> \
                     << /Root 3 0 R /Size 9 >> \
                     << /Type /Catalog /Names 4 0 R /Pages 7 0 R >> \
                     << /Dests 5 0 R >> \
                     << /Kids [6 0 R] >> \
                     << /Limits [(a) (c)] /Names [(a) [8 0 R /Fit] (b) [8 0 R /XYZ 0 0 0]] >> \
                     << /Type /Pages /Kids [8 0 R] /Count 1 >> \
                     << /Type /Page /Parent 7 0 R >>";

/// A hide action naming a field of a field tree, and the tree.
const FIELDS: &str = "<< /S /Hide /T (top.kid) >> \
                      << /Root 3 0 R /Size 7 >> \
                      << /Type /Catalog /AcroForm << /Fields [4 0 R 6 0 R] >> >> \
                      << /T (top) /Kids [5 0 R] >> \
                      << /T (kid) /Parent 4 0 R /FT /Tx >> \
                      << /T (other) /FT /Btn >>";

/// A page, which a structure tree's content items name, by an object
/// reference and as a marked-content item's page, in a tagged file.
const STRUCTURE: &str = "<< /Type /Page /Parent 4 0 R /StructParents 0 >> \
     << /Root 3 0 R /Size 8 >> \
     << /Type /Catalog /Pages 4 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >> \
     << /Type /Pages /Kids [1 0 R] /Count 1 >> \
     << /Type /StructTreeRoot /K [6 0 R] /ParentTree << /Nums [0 [6 0 R]] >> >> \
     << /S /Figure /P 5 0 R /Pg 1 0 R /K [0 << /Type /OBJR /Obj 1 0 R >> \
        << /Type /OBJR /Obj 7 0 R >> << /Type /MCR /Pg 1 0 R /MCID 1 >>] >> \
     << /Type /XObject /Subtype /Image /Length 0 /StructParent 1 >>";

/// A DeviceN space's attributes, the space their parent, of a spot and a
/// process colorant.
const DEVICE_N: &str = "<< /Subtype /NChannel /Colorants << /Spot 4 0 R >> \
        /Process << /ColorSpace /DeviceCMYK /Components [/Cyan /Magenta /Yellow /Black] >> >> \
     << /Root 5 0 R /Size 7 >> \
     [/DeviceN [/Cyan /Spot /None] /DeviceCMYK 6 0 R 1 0 R] \
     [/Separation /Spot /DeviceCMYK 6 0 R] \
     << /Type /Catalog >> \
     << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 1 1 1] /N 1 >>";

/// A go-to action whose destination is named in a name tree each part of
/// which is a reference: the catalog, 3, its `/Names`, 4, the tree's root,
/// 5, its `/Kids`, 6, a leaf, 7, its `/Names`, 8, and the key `b`, 9.
const NAMES_REFERRED: &str = "<< /S /GoTo /D (b) >> \
                              << /Root 3 0 R /Size 10 >> \
                              << /Type /Catalog /Names 4 0 R >> \
                              << /Dests 5 0 R >> \
                              << /Kids 6 0 R >> \
                              [7 0 R] \
                              << /Limits [(a) (b)] /Names 8 0 R >> \
                              [(a) 0 9 0 R 0] \
                              (b)";

/// A target, of an embedded file named in a name tree, and its target,
/// which names a page by number and an annotation of it.
const TARGET: &str = "<< /R /C /N (doc) /P 0 /A 0 /T << /R /P >> >> \
     << /Root 3 0 R /Size 7 >> \
     << /Type /Catalog /Pages 4 0 R /Names << /EmbeddedFiles << /Names [(doc) 6 0 R] >> \
        /Dests << /Names [(p) [5 0 R /Fit]] >> >> >> \
     << /Type /Pages /Kids [5 0 R] /Count 1 >> \
     << /Type /Page /Parent 4 0 R /Annots [<< /Subtype /FileAttachment /NM (a) >>] >> \
     << /Type /Filespec /F (doc.pdf) >>";

/// The tables of the embedded model the loader's seeds are, each with its
/// Link cells emptied, so that it loads alone.
const SEED_TABLES: [&str; 5] = [
    "Catalog",
    "PageObject",
    "FontDescriptorType1",
    "ActionGoTo",
    "DestXYZStructArray",
];

/// The embedded model's table `name`, as its file is, with its Link cells
/// emptied.
fn unlinked(name: &str) -> String {
    let (_, text) = (crate::grammar::model::TABLES.iter())
        .find(|(table, _)| *table == name)
        .unwrap_or_else(|| panic!("the embedded model has {name}"));
    let mut lines = text.lines();
    let mut unlinked = format!("{}\n", lines.next().unwrap_or_default());
    for line in lines {
        let mut cells: Vec<&str> = line.split('\t').collect();
        if let Some(link) = cells.get_mut(10) {
            *link = "";
        }
        unlinked.push_str(&cells.join("\t"));
        unlinked.push('\n');
    }
    unlinked
}

/// `value` with the version predicates around it put back, as its cell
/// has it.
fn rewrapped(value: &Wrapped<String>) -> String {
    let wraps = value.wraps.iter().rev();
    wraps.fold(value.item.clone(), |inner, wrap| {
        format!("fn:{}({},{inner})", wrap.func.name(), wrap.arg)
    })
}

/// The inputs the fuzzer begins from, and the tests run on, each named: for
/// the loader, a few of the embedded model's tables, alone and together;
/// for the evaluator, one for each function it supports, its text the
/// shortest of the embedded model's cells that call it (`fn:InNameTree`'s
/// the go-to action's), over a small object graph that gives it something
/// to read; and over a resolver that fails a name tree's kid, and one by
/// one each reference on the way to a tree and in it.
pub fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut seeds = Vec::new();
    let mut together = Vec::new();
    for name in SEED_TABLES {
        let piece = format!("{name}\n{}", unlinked(name));
        seeds.push((format!("tables-{name}"), piece.clone().into_bytes()));
        together.push(piece);
    }
    seeds.push((
        "tables-together".to_string(),
        together.join("\0").into_bytes(),
    ));

    let model = Grammar::embedded();
    let mut cells: Vec<(String, &Table)> = Vec::new();
    for table in model.tables() {
        for row in &table.rows {
            cells.extend(texts_of(row).map(|text| (text.to_string(), table)));
            // A value as its cell has it, with the version predicates around
            // it, which the loader takes off: so `fn:Deprecated` is called.
            let wrapped = row.possible_values.iter().flatten();
            cells.extend(
                wrapped
                    .filter(|v| !v.wraps.is_empty())
                    .map(|v| (rewrapped(v), table)),
            );
        }
    }
    // Version 2.0, its length known; every pick but the memo's and the
    // parents'.
    let how = (8 << 3) | 0x80;
    let picks = 4 | 8 | 0x10 | 0x20 | 0x80;
    for &f in SUPPORTED {
        let call = format!("{f}(");
        let preferred = (f == "fn:InNameTree").then(|| {
            let goto = model.table("ActionGoTo").and_then(|t| t.row(At::Key(b"D")));
            let items = goto
                .into_iter()
                .flat_map(|row| row.possible_values.iter().flatten());
            items
                .map(|item| item.item.as_str())
                .find(|item| item.contains(&call))
        });
        let shortest = (cells.iter())
            .filter(|(text, _)| text.contains(&call))
            .min_by_key(|(text, _)| (text.len(), text.as_str()));
        let (text, objects, parents) = match (preferred.flatten(), shortest) {
            (Some(text), _) => (text, NAMES.to_string(), 0),
            (None, Some((text, table))) => match f {
                "fn:IsFullyQualifiedFieldName" => (text.as_str(), FIELDS.to_string(), 0),
                "fn:ImageIsStructContentItem"
                | "fn:PageContainsStructContentItems"
                | "fn:IsPDFTagged" => (text.as_str(), STRUCTURE.to_string(), 0),
                "fn:HasSpotColorants" | "fn:HasProcessColorants" => {
                    (text.as_str(), DEVICE_N.to_string(), 1)
                }
                "fn:NumberOfPages" | "fn:PageProperty" => (text.as_str(), TARGET.to_string(), 0),
                _ => (
                    text.as_str(),
                    format!("{} {PLAIN}", written(&made(table))),
                    0,
                ),
            },
            (None, None) => panic!("no cell of the embedded model calls {f}"),
        };
        let name = format!("predicate-{}", &f["fn:".len()..]);
        seeds.push((name, seed(text, [0, how, picks | parents], &objects)));
    }
    // The name tree's kid, object 6, damaged.
    let goto = "fn:Eval(fn:InNameTree(trailer::Catalog::Names::Dests))";
    let failing = seed(goto, [1 << 6, how | 1, picks], NAMES);
    seeds.push(("predicate-InNameTree-damaged".to_string(), failing));
    // Each reference on the way to a tree, and in it, failing, one at a
    // time, for each reason (1 to 4: damaged, capped, locked, the source).
    let parts = [
        ("catalog", 3, 1),
        ("names", 4, 3),
        ("root", 5, 2),
        ("kids", 6, 4),
        ("leaf", 7, 1),
        ("leaf-names", 8, 3),
        ("key", 9, 2),
    ];
    for (part, n, why) in parts {
        let failing = seed(goto, [1 << (n % 8), how | why, picks], NAMES_REFERRED);
        seeds.push((format!("predicate-InNameTree-{part}"), failing));
    }
    seeds
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tokens a mutation puts in: the tables' and the predicates'.
    const PIECES: [&[u8]; 44] = [
        b"\t",
        b"\n",
        b"\0",
        b"\r",
        b"[",
        b"]",
        b";",
        b",",
        b"(",
        b")",
        b"'",
        b"*",
        b"0*",
        b"1*",
        b"TRUE",
        b"FALSE",
        b"integer",
        b"name;array",
        b"dictionary",
        b"1.7",
        b"2.0",
        b"fn:Eval(",
        b"fn:Not(",
        b"fn:SinceVersion(1.5,",
        b"fn:Extension(ADBE_Extn3,",
        b"fn:InNameTree(",
        b"fn:IsPresent(",
        b"@Type",
        b"@*",
        b"parent::",
        b"trailer::Catalog::",
        b" && ",
        b" || ",
        b"==",
        b"!=",
        b"<=",
        b" mod ",
        b" * ",
        b"<<",
        b">>",
        b"1 0 R",
        b"3 0 R",
        b"(a)",
        b"/Names",
    ];

    /// A mutation of `seed`, from `random`, as a fuzzer might make it.
    fn mutated(seed: &[u8], random: &mut impl FnMut() -> usize) -> Vec<u8> {
        let mut data = seed.to_vec();
        for _ in 0..1 + random() % 4 {
            let at = random() % (data.len() + 1);
            let end = match random() % 3 {
                0 => at,
                _ => data.len().min(at + 1 + random() % 8),
            };
            let piece: &[u8] = match random() % 3 {
                1 => &[],
                _ => PIECES[random() % PIECES.len()],
            };
            data.splice(at..end, piece.iter().copied());
        }
        data
    }

    fn generator(mut state: u64) -> impl FnMut() -> usize {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize
        }
    }

    /// The seeds, and mutations of each.
    fn mutated_seeds() -> Vec<Vec<u8>> {
        let mut random = generator(0x5EED);
        let mut inputs = Vec::new();
        for (name, seed) in fixtures() {
            let times = if name.starts_with("tables-") { 10 } else { 50 };
            for _ in 0..times {
                inputs.push(mutated(&seed, &mut random));
            }
            inputs.push(seed);
        }
        inputs
    }

    /// A random table's groups of PossibleValues, one a type.
    const POSSIBLE: [&str; 6] = [
        "",
        "A,fn:SinceVersion(1.5,B)",
        "fn:Eval(@N>0)",
        "fn:RequiredValue(@S==A,1),2",
        "fn:Eval(fn:InNameTree(trailer::Catalog::Names::Dests))",
        "fn:Deprecated(1.4,A)",
    ];
    /// A random table's SpecialCases, one a type.
    const SPECIAL: [&str; 6] = [
        "",
        "fn:Ignore(@S==A)",
        "fn:IsMeaningful(@Type==A)",
        "fn:Eval(fn:ArrayLength(Kids)>0)",
        "fn:Not(fn:IsPresent(S))",
        "fn:Eval(fn:BitsClear(2,32) && fn:Nonesuch())",
    ];
    /// A random table's groups of Links, one a type.
    const LINKS: [&str; 5] = ["", "A", "B", "fn:SinceVersion(1.5,A)", "Nonesuch"];

    /// A random table's SinceVersions.
    const SINCE: [&str; 5] = [
        "1.0",
        "1.5",
        "2.0",
        "fn:Extension(ADBE_Extn3)",
        "fn:Eval(fn:Extension(ISO_TS_32001,2.0) || 1.7)",
    ];
    /// A random table's Requireds.
    const REQUIRED: [&str; 5] = [
        "TRUE",
        "FALSE",
        "fn:IsRequired(@Type==A)",
        "fn:IsRequired(fn:SinceVersion(1.5))",
        "fn:IsRequired(fn:IsPresent(N) && @N>1)",
    ];
    /// A random table's IndirectReferences.
    const INDIRECT: [&str; 4] = [
        "TRUE",
        "FALSE",
        "fn:MustBeIndirect()",
        "fn:MustBeDirect(fn:IsPresent(N))",
    ];
    /// A random table's types.
    const TYPES: [&str; 10] = [
        "name",
        "integer",
        "array",
        "dictionary",
        "string",
        "number",
        "boolean",
        "stream",
        "name-tree",
        "fn:SinceVersion(1.5,bitmask)",
    ];

    /// What a mutation of a random table puts in a cell's place.
    const BROKEN: [&str; 8] = [
        "",
        "[",
        "]",
        "nonesuch",
        "fn:Nonesuch(",
        "[a];[b]",
        "\r",
        "MAYBE",
    ];

    /// A random table's text: upstream's header, and rows of a dictionary's
    /// keys or an array's elements, a repeating set among them, each of
    /// mostly well-formed cells, one in a few broken.
    fn random_table(random: &mut impl FnMut() -> usize) -> String {
        let mut keys: Vec<String> = match random() % 3 {
            0 => (0..random() % 4).map(|i| i.to_string()).collect(),
            _ => ["Type", "S", "N", "Kids"]
                .into_iter()
                .filter(|_| random().is_multiple_of(2))
                .map(String::from)
                .collect(),
        };
        let repeats = random() % 3;
        if keys.first().is_some_and(|k| k == "0") || keys.is_empty() {
            let at = keys.len();
            keys.extend((at..at + repeats).map(|i| format!("{i}*")));
        }
        if random().is_multiple_of(3) {
            keys.push("*".to_string());
        }
        let mut text = crate::grammar::HEADER.join("\t");
        text.push('\n');
        for key in keys {
            let n = 1 + random() % 2;
            let types: Vec<&str> = (0..n).map(|_| TYPES[random() % TYPES.len()]).collect();
            let mut groups = |items: &[&str]| -> String {
                if random().is_multiple_of(3) {
                    return String::new();
                }
                let each: Vec<String> = (0..n)
                    .map(|_| format!("[{}]", items[random() % items.len()]))
                    .collect();
                each.join(";")
            };
            let (values, special, links) = (groups(&POSSIBLE), groups(&SPECIAL), groups(&LINKS));
            let mut cells = vec![
                key,
                types.join(";"),
                SINCE[random() % SINCE.len()].to_string(),
                String::new(),
                REQUIRED[random() % REQUIRED.len()].to_string(),
                INDIRECT[random() % INDIRECT.len()].to_string(),
                ["FALSE", "TRUE"][random() % 2].to_string(),
                String::new(),
                values,
                special,
                links,
                String::new(),
            ];
            if random().is_multiple_of(8) {
                let at = random() % cells.len();
                cells[at] = BROKEN[random() % BROKEN.len()].to_string();
            }
            text.push_str(&cells.join("\t"));
            text.push('\n');
        }
        text
    }

    /// Random tables: a few pieces, each a name and a random table's text,
    /// a name now and then one no table can have.
    fn random_tables() -> Vec<Vec<u8>> {
        let mut random = generator(0x7AB1E5);
        let names = ["A", "B", "a", "a-b", "A", ""];
        let unnamed = ["A\tB", "a\r"];
        let mut inputs = Vec::new();
        for _ in 0..1_000 {
            let pieces: Vec<String> = (0..1 + random() % 3)
                .map(|_| {
                    let name = match random() % 32 {
                        0 => unnamed[random() % unnamed.len()],
                        _ => names[random() % names.len()],
                    };
                    format!("{name}\n{}", random_table(&mut random))
                })
                .collect();
            inputs.push(pieces.join("\0").into_bytes());
        }
        inputs
    }

    /// The words random predicates are made of.
    const WORDS: [&str; 24] = [
        "@Type",
        "Type",
        "@*",
        "*",
        "N",
        "@N",
        "@S",
        "parent::@S",
        "parent::parent::Kids",
        "trailer::Catalog::Names::Dests",
        "trailer::Encrypt::CF",
        "Kids::0",
        "1",
        "-2",
        "0",
        "1.5",
        "2.0",
        "'a'",
        "true",
        "null",
        "Page",
        "GoTo",
        "ADBE_Extn3",
        "Nonesuch",
    ];

    /// The operators random predicates are made of, with the space they're
    /// written with.
    const COMPARISONS: [&str; 6] = ["==", "!=", "<", "<=", ">", ">="];
    const ARITHMETIC: [&str; 4] = ["+", " - ", " * ", " mod "];

    /// A random predicate's term: a bracketed predicate, a call of a
    /// supported function or another, or a word.
    fn term(random: &mut dyn FnMut() -> usize, depth: usize) -> String {
        match random() % if depth > 2 { 2 } else { 6 } {
            0 | 1 => WORDS[random() % WORDS.len()].to_string(),
            2 => format!("({})", predicate(random, depth + 1)),
            _ => {
                let f = match random() % 8 {
                    0 => "fn:Nonesuch",
                    _ => SUPPORTED[random() % SUPPORTED.len()],
                };
                let args: Vec<String> = (0..random() % 3)
                    .map(|_| predicate(random, depth + 1))
                    .collect();
                format!("{f}({})", args.join(","))
            }
        }
    }

    /// Random terms joined by arithmetic, mostly one alone.
    fn arith(random: &mut dyn FnMut() -> usize, depth: usize) -> String {
        let mut text = term(random, depth);
        if random().is_multiple_of(4) {
            text.push_str(ARITHMETIC[random() % ARITHMETIC.len()]);
            text.push_str(&term(random, depth));
        }
        text
    }

    /// Random arithmetic, compared one time in three.
    fn compared(random: &mut dyn FnMut() -> usize, depth: usize) -> String {
        let mut text = arith(random, depth);
        if random().is_multiple_of(3) {
            text.push_str(COMPARISONS[random() % COMPARISONS.len()]);
            text.push_str(&arith(random, depth));
        }
        text
    }

    /// A random predicate: comparisons, one in three of them two or three
    /// joined by `&&` or by `||`.
    fn predicate(random: &mut dyn FnMut() -> usize, depth: usize) -> String {
        let join = [" && ", " || "][random() % 2];
        let count = if random().is_multiple_of(3) {
            2 + random() % 2
        } else {
            1
        };
        let items: Vec<String> = (0..count).map(|_| compared(random, depth)).collect();
        items.join(join)
    }

    /// The values random objects are made of.
    const VALUES: [&str; 16] = [
        "<< /Type /Page >>",
        "<< /S /GoTo /D (a) >>",
        "<< /Root 3 0 R >>",
        "<< /Names 4 0 R >>",
        "<< /Dests 5 0 R >>",
        "<< /Names [(a) 1 0 R] >>",
        "<< /Kids [5 0 R 6 0 R] >>",
        "[1 2 3]",
        "[/A /B]",
        "1 0 R",
        "3 0 R",
        "7",
        "(a)",
        "/A",
        "null",
        "<< /Length 0 >>",
    ];

    /// Random predicates of the language's tokens, over random objects,
    /// with random bytes between.
    fn random_predicates() -> Vec<Vec<u8>> {
        let mut random = generator(0xB17E5);
        let mut inputs = Vec::new();
        for _ in 0..1_000 {
            let mut text = predicate(&mut random, 0);
            // One in eight broken, a byte of it taken out or put in.
            if random().is_multiple_of(8) && !text.is_empty() {
                let at = random() % text.len();
                if text.is_char_boundary(at) && text.is_char_boundary(at + 1) {
                    match random() % 2 {
                        0 => _ = text.remove(at),
                        _ => text.insert(at, ['(', ')', ',', '\'', ':'][random() % 5]),
                    }
                }
            }
            let mut input = text.into_bytes();
            input.push(0);
            input.extend((0..3).map(|_| random() as u8));
            for _ in 0..random() % 8 {
                input.extend_from_slice(VALUES[random() % VALUES.len()].as_bytes());
                input.push(b' ');
            }
            inputs.push(input);
        }
        inputs
    }

    /// The crate's sources, whose texts other tests ask through the
    /// embedded model.
    const SOURCES: [&str; 3] = [
        include_str!("grammar.rs"),
        include_str!("predicate.rs"),
        include_str!("extension.rs"),
    ];

    /// That the embedded model's memo holds none of the texts `asked`, the
    /// bodies', but those its rows or the crate's sources hold, which other
    /// tests ask through it; and how many of them neither holds.
    fn none_kept(asked: &BTreeSet<String>) -> usize {
        let model = Grammar::embedded();
        let rows = texts(model);
        let theirs: Vec<&String> = (asked.iter())
            .filter(|text| !rows.contains(text.as_str()))
            .collect();
        let kept = model.parses().kept();
        let held: Vec<&&String> = (theirs.iter())
            .filter(|text| kept.contains(text.as_str()))
            .filter(|text| !SOURCES.iter().any(|source| source.contains(text.as_str())))
            .collect();
        assert!(held.is_empty(), "the embedded model keeps {held:?}");
        theirs.len()
    }

    /// Every input, through both bodies, telling `asked` what they ask.
    fn through_both(inputs: Vec<Vec<u8>>, asked: &mut BTreeSet<String>) -> usize {
        let mut loads = 0;
        let mut ask = |text: &str| {
            asked.insert(text.to_string());
        };
        for input in &inputs {
            loads += usize::from(loaded(input, &mut ask).is_some());
            asking(input, &mut ask);
        }
        loads
    }

    #[test]
    fn mutated_seeds_keep_every_promise() {
        let mut asked = BTreeSet::new();
        through_both(mutated_seeds(), &mut asked);
        none_kept(&asked);
    }

    #[test]
    fn random_tables_keep_every_promise() {
        let mut asked = BTreeSet::new();
        let loads = through_both(random_tables(), &mut asked);
        assert!(loads > 100, "random tables load: {loads}");
        none_kept(&asked);
    }

    #[test]
    fn random_predicates_keep_every_promise() {
        let inputs = random_predicates();
        let parses = (inputs.iter())
            .map(|input| input.split(|&b| b == 0).next().unwrap_or_default())
            .filter(|text| predicate::parse(std::str::from_utf8(text).unwrap()).is_ok())
            .count();
        assert!(parses > 500, "random predicates parse: {parses}");
        let mut asked = BTreeSet::new();
        through_both(inputs, &mut asked);
        none_kept(&asked);
    }

    /// The embedded model's tables, as an input of a piece each, keep every
    /// promise, and load to the embedded model's hash.
    #[test]
    fn the_embedded_tables_keep_every_promise() {
        let tables = crate::grammar::model::TABLES;
        let pieces: Vec<String> = (tables.iter())
            .map(|(name, text)| format!("{name}\n{text}"))
            .collect();
        assert_eq!(pieces.len(), 614);
        let hash = loaded(pieces.join("\0").as_bytes(), &mut |_| {});
        assert_eq!(hash, Some(Grammar::embedded().hash()));
    }

    /// Every seed keeps every promise, the loader's load, and each
    /// predicate's text calls its function.
    #[test]
    fn the_seeds_keep_every_promise() {
        let seeds = fixtures();
        for (name, seed) in &seeds {
            let hash = loaded(seed, &mut |_| {});
            assert_eq!(hash.is_some(), name.starts_with("tables-"), "{name}");
            predicates(seed);
        }
        for f in SUPPORTED {
            let name = format!("predicate-{}", &f["fn:".len()..]);
            let (_, seed) = (seeds.iter()).find(|(n, _)| *n == name).unwrap();
            let text = seed.split(|&b| b == 0).next().unwrap();
            let call = format!("{f}(");
            assert!(std::str::from_utf8(text).unwrap().contains(&call), "{name}");
        }
    }

    /// The bodies leave the embedded model's memo holding none of the texts
    /// they ask, as each test above holds of its own inputs: they ask
    /// through models of their own. Here over the seeds, and over texts of
    /// their own as the fuzzer makes them, that no other test asks: each the
    /// predicates' body's, and the tables' body's as a table's cell.
    #[test]
    fn the_bodies_keep_none_of_their_texts_in_the_embedded_model() {
        let mut random = generator(0x3E30);
        let mut inputs: Vec<Vec<u8>> = fixtures().into_iter().map(|(_, seed)| seed).collect();
        for n in 0..100 {
            let text = format!("fn:Eval(@Nonesuch{n}==1) && {}", predicate(&mut random, 1));
            let cells = [
                "Key",
                "integer",
                "1.0",
                "",
                "FALSE",
                "FALSE",
                "FALSE",
                "",
                "",
                &format!("[{text}]"),
                "",
                "",
            ];
            let table = format!(
                "A\n{}\n{}\n",
                crate::grammar::HEADER.join("\t"),
                cells.join("\t")
            );
            inputs.push(seed(&text, [0, 0, 0x7F], "<< /Key 1 >>"));
            inputs.push(table.into_bytes());
        }
        let mut asked = BTreeSet::new();
        assert_eq!(through_both(inputs, &mut asked), 100 + 6, "the tables load");
        let theirs = none_kept(&asked);
        assert!(theirs >= 200, "{theirs} texts of the bodies' own");
    }

    /// `PDF_ARLINGTON_FUZZ_SEEDS` names a folder: writes the seeds there, for
    /// the fuzzer to begin from (`just fuzz arlington`).
    #[test]
    #[ignore = "needs PDF_ARLINGTON_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir = std::env::var_os("PDF_ARLINGTON_FUZZ_SEEDS")
            .expect("PDF_ARLINGTON_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (name, seed) in fixtures() {
            std::fs::write(dir.join(name), seed).unwrap();
        }
    }
}
