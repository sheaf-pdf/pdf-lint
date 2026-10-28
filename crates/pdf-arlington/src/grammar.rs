//! The model's tables, read from its TSV files (upstream's README.md, "TSV
//! Data Overview"; INTERNAL_GRAMMAR.md, "TSV file rules" and "TSV Data
//! Fields"). The Type cell is split into its types here, and a cell with
//! an item per type (IndirectReference, PossibleValues, SpecialCase, Link)
//! into those items; the version predicates around a type, a value or a
//! link are taken off and kept beside it. What a predicate means is the
//! checker's to say.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdf_objects::Version;

use crate::xxh64::xxh64;

pub(crate) mod model {
    include!(concat!(env!("OUT_DIR"), "/model.rs"));
}

/// A cell the embedded model has corrected, where upstream's contradicts
/// ISO 32000-2: a line of `model/corrections.tsv`, which `build.rs` makes
/// to the embedded copy only, failing when upstream's cell isn't
/// `upstream` any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Correction {
    /// The table's name.
    pub table: &'static str,
    /// The row's key.
    pub key: &'static str,
    /// The column's name, as [`HEADER`] has it.
    pub column: &'static str,
    /// The cell as upstream has it.
    pub upstream: &'static str,
    /// The cell as corrected.
    pub corrected: &'static str,
    /// Why, with the clause of ISO 32000-2 it follows.
    pub reason: &'static str,
}

/// The corrections the embedded model has ([`Grammar::embedded`]). A
/// model [`Grammar::load`] reads is as its files have it.
pub fn corrections() -> &'static [Correction] {
    model::CORRECTIONS
}

/// A row the embedded model adds, where upstream lacks what a reference
/// defines: a row of a file in `model/additions`, in upstream's format,
/// which `build.rs` adds to the embedded copy only, failing when upstream
/// has the table, or the table's row, itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Addition {
    /// The table's name.
    pub table: &'static str,
    /// The row's key.
    pub key: &'static str,
    /// Whether the table is added too, upstream having none of its name.
    pub table_added: bool,
    /// The row's Note: why, with the reference it follows.
    pub note: &'static str,
}

/// The rows the embedded model adds ([`Grammar::embedded`]).
pub fn additions() -> &'static [Addition] {
    model::ADDITIONS
}

/// Every table's header: upstream's 12 columns, in their order.
pub const HEADER: [&str; 12] = [
    "Key",
    "Type",
    "SinceVersion",
    "DeprecatedIn",
    "Required",
    "IndirectReference",
    "Inheritable",
    "DefaultValue",
    "PossibleValues",
    "SpecialCase",
    "Link",
    "Note",
];

/// The model: its tables, by name.
pub struct Grammar {
    tables: BTreeMap<String, Table>,
    /// Its tables' files' XXH64 ([`Grammar::hash`]).
    hash: u64,
}

/// How many tables it has, and its hash: each [`Table`] shows its own
/// rows.
impl fmt::Debug for Grammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Grammar")
            .field("tables", &self.tables.len())
            .field("hash", &format_args!("{:016x}", self.hash))
            .finish_non_exhaustive()
    }
}

/// One object's table: `Catalog`, `AnnotSquare`, `ArrayOfAnnots`.
#[derive(Debug)]
pub struct Table {
    /// Its file's name less `.tsv`, which links name it by.
    pub name: String,
    /// Its rows, in its file's order.
    pub rows: Vec<Row>,
    /// The first of its repeating rows (`0*`, `1*`: INTERNAL_GRAMMAR.md,
    /// "Column 1 - Key"), which is also the first array element they
    /// stand for; `rows.len()` when it has none.
    repeats: usize,
}

/// Where in an object a value is: under a dictionary's key, or an array's
/// element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At<'a> {
    /// Under a dictionary's key.
    Key(&'a [u8]),
    /// An array's element, by its index.
    Index(usize),
}

/// One row: a key of a dictionary or an element of an array. The fields
/// are upstream's columns, in their order.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The line of its table's file it's on, the header being line 1.
    pub line: usize,
    /// A key; an array element's index (`0`); `n*`, an element of each
    /// repeating set; or `*`, any other key or element.
    pub key: String,
    /// The types its value may be, each maybe wrapped.
    pub r#type: Vec<Wrapped<ArlType>>,
    /// A version (`1.5`), or an `fn:Extension` or `fn:Eval` predicate.
    pub since_version: String,
    /// A version, or empty when it isn't deprecated.
    pub deprecated_in: String,
    /// `TRUE`, `FALSE`, or an `fn:IsRequired` predicate.
    pub required: String,
    /// Per type: `TRUE` (it must be an indirect reference), `FALSE`, or an
    /// `fn:MustBeDirect` or `fn:MustBeIndirect` predicate. A cell with one
    /// value, unbracketed, says it for every type.
    pub indirect_reference: Vec<String>,
    /// Inheritable: whether the object may take it from those above it, as
    /// a page from the page tree, a field from the field tree.
    pub inheritable: bool,
    /// As the cell has it: the checker reads no defaults.
    pub default_value: String,
    /// Per type, the values it may take, each maybe wrapped; none when
    /// any may be.
    pub possible_values: Vec<Vec<Wrapped<String>>>,
    /// Per type, the predicate its value meets; empty when there's none.
    pub special_case: Vec<String>,
    /// Per type, the tables its value may be, each maybe wrapped; none
    /// for a simple type.
    pub link: Vec<Vec<Wrapped<String>>>,
    /// Its Note, as written: prose.
    pub note: String,
}

/// Arlington's types (INTERNAL_GRAMMAR.md, "Column 2 - Type"): PDF's
/// objects, refined as the specification refines them (a date is a
/// string, a rectangle an array of four numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArlType {
    /// `array`.
    Array,
    /// `bitmask`: an integer whose bits are flags.
    Bitmask,
    /// `boolean`.
    Boolean,
    /// `date`: a string that's a date (ISO 32000-2, 7.9.4).
    Date,
    /// `dictionary`.
    Dictionary,
    /// `integer`.
    Integer,
    /// `matrix`: an array of six numbers (ISO 32000-2, 8.3.4).
    Matrix,
    /// `name`.
    Name,
    /// `name-tree`: a dictionary, a name tree's root (ISO 32000-2, 7.9.6).
    NameTree,
    /// `null`.
    Null,
    /// `number`: an integer or a real.
    Number,
    /// `number-tree`: a dictionary, a number tree's root (ISO 32000-2, 7.9.7).
    NumberTree,
    /// `rectangle`: an array of four numbers (ISO 32000-2, 7.9.5).
    Rectangle,
    /// `stream`.
    Stream,
    /// `string`: any string.
    String,
    /// `string-ascii`: a string of printable ASCII.
    StringAscii,
    /// `string-byte`: a string of bytes.
    StringByte,
    /// `string-text`: a text string (ISO 32000-2, 7.9.2.2).
    StringText,
}

/// A cell's item with the version predicates around it taken off,
/// outermost first: `fn:IsPDFVersion(1.7,fn:Extension(ADBE_Extn3,X))` is
/// `X` in `IsPDFVersion` of `1.7` and `Extension` of `ADBE_Extn3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped<T> {
    /// The version predicates around it, outermost first.
    pub wraps: Vec<Wrap>,
    /// The item inside them.
    pub item: T,
}

/// A version predicate around an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrap {
    /// Which predicate it is.
    pub func: WrapFn,
    /// Its first argument: a version (`1.5`, which [`version`] reads), or
    /// for `Extension` the extension's name.
    pub arg: String,
}

/// The predicates that wrap an item: those INTERNAL_GRAMMAR.md's "Column
/// 11 - Link" allows around a link, which wrap types and values too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WrapFn {
    /// `fn:SinceVersion(v,X)`: `X` from version `v` on.
    SinceVersion,
    /// `fn:BeforeVersion(v,X)`: `X` before version `v`.
    BeforeVersion,
    /// `fn:IsPDFVersion(v,X)`: `X` at version `v` only.
    IsPDFVersion,
    /// `fn:Deprecated(v,X)`: `X`, deprecated from version `v` on.
    Deprecated,
    /// `fn:Extension(e,X)`: `X` under the extension `e`.
    Extension,
}

/// Why a model doesn't read ([`Grammar::load`]): its folder, or a table
/// in it, can't be read, or a table breaks a rule of the model's files,
/// said after the table's file and line. The std error under one is a
/// field, said after `: `, not a source.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The folder, or a table in it, can't be read.
    #[error("reading {path}: {error}")]
    Read {
        /// The folder's path, or the table's.
        path: PathBuf,
        /// Why, as std says.
        error: io::Error,
    },
    /// The folder reads, but an entry of it doesn't.
    #[error("{0}")]
    Entry(io::Error),
    /// A table's file name isn't UTF-8, so it names no table.
    #[error("{path}: not a UTF-8 name")]
    Name {
        /// The table's path.
        path: PathBuf,
    },
    /// The folder has no `.tsv` file.
    #[error("{dir} has no tables")]
    Empty {
        /// The folder's path.
        dir: PathBuf,
    },
    /// A table breaks a rule of the model's files.
    #[error("{file}: {}{error}", on_line(.line))]
    Table {
        /// The table's file: its path, or for the embedded model its name
        /// (`Catalog.tsv`).
        file: PathBuf,
        /// The line it's broken on, the header being line 1, when it's a
        /// line's rule.
        line: Option<usize>,
        /// The rule.
        error: TableError,
    },
}

/// "line N: ", when `line` is known, for [`Error::Table`]'s text.
fn on_line(line: &Option<usize>) -> String {
    line.map(|n| format!("line {n}: ")).unwrap_or_default()
}

/// A rule of the model's files a table breaks (INTERNAL_GRAMMAR.md, "TSV
/// file rules" and "TSV Data Fields"), as [`Error::Table`] says it, after
/// the table's file and line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TableError {
    /// The file's last line has no newline after it.
    #[error("its last line isn't ended")]
    Unended,
    /// Its lines end in CR LF, where upstream's end in LF.
    #[error("CRLF line ends (.gitattributes keeps the model's bytes as they are)")]
    Crlf,
    /// Its first line isn't [`HEADER`].
    #[error("not upstream's header")]
    Header,
    /// A row for a key a row before it has.
    #[error("a second row for {key}")]
    SecondRow {
        /// The key.
        key: String,
    },
    /// No row after the header.
    #[error("no rows")]
    NoRows,
    /// A repeating row before one that isn't, or numbered other than by
    /// the element it starts at.
    #[error("repeating rows come last, numbered from the element they start at")]
    Repeating,
    /// A link to a table the model hasn't.
    #[error("links to {table}, which has no table")]
    Link {
        /// The table linked to.
        table: String,
    },
    /// A row without upstream's 12 fields.
    #[error("{count} fields, not 12")]
    Fields {
        /// How many it has.
        count: usize,
    },
    /// A row whose Key, Type, SinceVersion, Required or
    /// IndirectReference is empty.
    #[error("no {column}")]
    Missing {
        /// The column, as [`HEADER`] names it.
        column: &'static str,
    },
    /// An item whose version predicate around it doesn't read: one whose
    /// version isn't one, say.
    #[error("{item:?}: a version predicate that doesn't read")]
    Wrap {
        /// The item, as written.
        item: String,
    },
    /// A type that isn't one of [`ArlType`]'s.
    #[error("Type {name:?} isn't one of Arlington's")]
    Type {
        /// The type, as written.
        name: String,
    },
    /// An Inheritable that's neither `TRUE` nor `FALSE`.
    #[error("Inheritable is {cell:?}, not TRUE or FALSE")]
    Inheritable {
        /// The cell.
        cell: String,
    },
    /// A cell with an item per type whose item isn't one bracketed item.
    #[error("{column}: {group:?} isn't one bracketed item")]
    Group {
        /// The column, as [`HEADER`] names it.
        column: &'static str,
        /// The item, as written.
        group: String,
    },
    /// A cell with an item per type whose items aren't as many as the
    /// types.
    #[error("{column} has {items} items for {types} types")]
    Count {
        /// The column, as [`HEADER`] names it.
        column: &'static str,
        /// How many items it has.
        items: usize,
        /// How many types the row has.
        types: usize,
    },
    /// A type's list of values or links with an empty item.
    #[error("{list:?} has an empty item")]
    EmptyItem {
        /// The list, as written.
        list: String,
    },
    /// A `]` or `)` that closes nothing, or closes the other.
    #[error("{text:?}: a {found} out of place")]
    OutOfPlace {
        /// What it's in, as written.
        text: String,
        /// The `]` or `)`.
        found: char,
    },
    /// A bracket, parenthesis or quote that nothing closes.
    #[error("{text:?}: a bracket, parenthesis or quote left open")]
    Open {
        /// What it's in, as written.
        text: String,
    },
}

/// A table's file, to read: the table's name, what an error calls the
/// file, and its text.
struct File<'a> {
    name: String,
    label: PathBuf,
    text: std::borrow::Cow<'a, str>,
}

impl Grammar {
    /// The model this crate embeds: upstream's `tsv/latest` at
    /// [`COMMIT`](crate::COMMIT), with the [`corrections`] made and the
    /// [`additions`] added, read the first time it's asked for. Panics,
    /// naming the file and line, when it doesn't read, which its tests
    /// rule out.
    pub fn embedded() -> &'static Grammar {
        EMBEDDED.get_or_init(|| {
            Grammar::from_files(model::TABLES.iter().map(|&(name, text)| File {
                name: name.to_string(),
                label: format!("{name}.tsv").into(),
                text: text.into(),
            }))
            .unwrap_or_else(|e| panic!("the Arlington model doesn't read: {e}"))
        })
    }

    /// The tables in `dir`, a `.tsv` file each: a checkout of upstream's
    /// `tsv/latest`, newer than the embedded one, say. An [`Error`] names
    /// the file and line of a row that doesn't read, or that links to a
    /// table `dir` doesn't have.
    pub fn load(dir: &Path) -> Result<Grammar, Error> {
        let entries = std::fs::read_dir(dir).map_err(|error| Error::Read {
            path: dir.to_path_buf(),
            error,
        })?;
        let mut paths = Vec::new();
        for entry in entries {
            let path = entry.map_err(Error::Entry)?.path();
            if path.extension().is_some_and(|e| e == "tsv") {
                paths.push(path);
            }
        }
        paths.sort();
        let mut files = Vec::new();
        for path in &paths {
            let name = path.file_stem().and_then(|s| s.to_str());
            let name = name.ok_or_else(|| Error::Name { path: path.clone() })?;
            let text = std::fs::read_to_string(path).map_err(|error| Error::Read {
                path: path.clone(),
                error,
            })?;
            files.push(File {
                name: name.to_string(),
                label: path.clone(),
                text: text.into(),
            });
        }
        if files.is_empty() {
            return Err(Error::Empty {
                dir: dir.to_path_buf(),
            });
        }
        Grammar::from_files(files)
    }

    fn from_files<'a>(files: impl IntoIterator<Item = File<'a>>) -> Result<Grammar, Error> {
        let mut files: Vec<File> = files.into_iter().collect();
        files.sort_by_cached_key(|file| format!("{}.tsv", file.name));
        let mut bytes = Vec::new();
        for file in &files {
            bytes.extend_from_slice(format!("{}.tsv\n", file.name).as_bytes());
            bytes.extend_from_slice(file.text.as_bytes());
        }
        let hash = xxh64(&bytes, 0);
        let mut tables = BTreeMap::new();
        let mut labels = BTreeMap::new();
        for file in files {
            let table =
                Table::parse(&file.name, &file.text).map_err(|(line, error)| Error::Table {
                    file: file.label.clone(),
                    line,
                    error,
                })?;
            labels.insert(file.name.clone(), file.label);
            tables.insert(file.name, table);
        }
        for table in tables.values() {
            for row in &table.rows {
                if let Some(link) = row
                    .link
                    .iter()
                    .flatten()
                    .find(|l| !tables.contains_key(&l.item))
                {
                    return Err(Error::Table {
                        file: labels[&table.name].clone(),
                        line: Some(row.line),
                        error: TableError::Link {
                            table: link.item.clone(),
                        },
                    });
                }
            }
        }
        Ok(Grammar { tables, hash })
    }

    /// What names the model, as reports give it: XXH64, seed 0, of each
    /// table's file in byte order of the files' names, its name
    /// (`Catalog.tsv`), a newline, and its bytes. Two models are alike when
    /// it is, wherever each was read from. With xxHash's own command line,
    /// in a `tsv/latest` folder: `for f in $(LC_ALL=C ls *.tsv); do printf
    /// '%s\n' "$f"; cat "$f"; done | xxhsum -H1`. The embedded model's is
    /// of its tables with the [`corrections`] made and the [`additions`]
    /// added, so it isn't upstream's at the same commit, which
    /// [`Grammar::load`] reads as it is.
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// The table named `name`.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.get(name)
    }

    /// Every table, by name.
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.values()
    }
}

/// The embedded model, read once ([`Grammar::embedded`]).
static EMBEDDED: OnceLock<Grammar> = OnceLock::new();

impl Table {
    /// The table in `text`, a TSV file's whole: upstream's header, then a
    /// row a line, each line ended. An error is the rule it breaks, with
    /// the line when it's a line's.
    fn parse(name: &str, text: &str) -> Result<Table, (Option<usize>, TableError)> {
        let body = text.strip_suffix('\n').ok_or((None, TableError::Unended))?;
        let mut lines = body.split('\n');
        let header = lines.next().unwrap_or_default();
        if header.ends_with('\r') {
            return Err((Some(1), TableError::Crlf));
        }
        if header.split('\t').ne(HEADER) {
            return Err((Some(1), TableError::Header));
        }
        let mut rows = Vec::new();
        let mut keys = HashSet::new();
        for (i, text) in lines.enumerate() {
            let line = i + 2;
            let row = Row::parse(line, text).map_err(|error| (Some(line), error))?;
            if !keys.insert(row.key.clone()) {
                return Err((Some(line), TableError::SecondRow { key: row.key }));
            }
            rows.push(row);
        }
        if rows.is_empty() {
            return Err((None, TableError::NoRows));
        }
        let repeats = rows
            .iter()
            .position(|r| repeating(&r.key))
            .unwrap_or(rows.len());
        for (i, row) in rows.iter().enumerate().skip(repeats) {
            if row.key != format!("{i}*") {
                return Err((Some(row.line), TableError::Repeating));
            }
        }
        Ok(Table {
            name: name.to_string(),
            rows,
            repeats,
        })
    }

    /// The row for the value at `at`: the row with its key or index, else,
    /// for an array's element, the repeating row it falls on, else the
    /// wildcard `*`.
    pub fn row(&self, at: At) -> Option<&Row> {
        let named = |key: &[u8]| self.rows.iter().find(|r| r.key.as_bytes() == key);
        match at {
            At::Key(key) => named(key),
            At::Index(i) => named(i.to_string().as_bytes()).or_else(|| self.repeated(i)),
        }
        .or_else(|| named(b"*"))
    }

    /// The repeating rows, a set the array's elements after the other
    /// rows go through again and again; none when the table has none.
    pub fn repeating(&self) -> &[Row] {
        &self.rows[self.repeats..]
    }

    /// The repeating row element `i` falls on, if the table has them and
    /// they start at or before it.
    fn repeated(&self, i: usize) -> Option<&Row> {
        let set = self.rows.len() - self.repeats;
        (set > 0 && i >= self.repeats).then(|| &self.rows[self.repeats + (i - self.repeats) % set])
    }
}

/// Whether `key` is a repeating row's: a digit, then `*`.
fn repeating(key: &str) -> bool {
    key.strip_suffix('*')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

impl Row {
    /// The row on line `line`, from its text.
    fn parse(line: usize, text: &str) -> Result<Row, TableError> {
        let fields: Vec<&str> = text.split('\t').collect();
        let [
            key,
            ty,
            since,
            deprecated,
            required,
            indirect,
            inheritable,
            default,
            values,
            special,
            link,
            note,
        ] = fields[..]
        else {
            return Err(TableError::Fields {
                count: fields.len(),
            });
        };
        for (column, cell) in [
            ("Key", key),
            ("Type", ty),
            ("SinceVersion", since),
            ("Required", required),
            ("IndirectReference", indirect),
        ] {
            if cell.is_empty() {
                return Err(TableError::Missing { column });
            }
        }
        let r#type = split(ty, ';')?
            .into_iter()
            .map(|t| {
                let (wraps, name) = unwrap(t)?;
                let item = ArlType::named(name).ok_or_else(|| TableError::Type {
                    name: name.to_string(),
                })?;
                Ok(Wrapped { wraps, item })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let n = r#type.len();
        let inheritable = match inheritable {
            "TRUE" => true,
            "FALSE" => false,
            other => {
                return Err(TableError::Inheritable {
                    cell: other.to_string(),
                });
            }
        };
        let indirect_reference = if indirect.starts_with('[') {
            groups(indirect, n, "IndirectReference")?
                .into_iter()
                .map(str::to_string)
                .collect()
        } else {
            vec![indirect.to_string(); n]
        };
        let possible_values = groups(values, n, "PossibleValues")?;
        let special_case = groups(special, n, "SpecialCase")?;
        let link = groups(link, n, "Link")?;
        Ok(Row {
            line,
            key: key.to_string(),
            r#type,
            since_version: since.to_string(),
            deprecated_in: deprecated.to_string(),
            required: required.to_string(),
            indirect_reference,
            inheritable,
            default_value: default.to_string(),
            possible_values: possible_values
                .into_iter()
                .map(items)
                .collect::<Result<_, _>>()?,
            special_case: special_case.into_iter().map(str::to_string).collect(),
            link: link.into_iter().map(items).collect::<Result<_, _>>()?,
            note: note.to_string(),
        })
    }
}

impl ArlType {
    /// All 18, in the model's order (its names', alphabetical).
    pub const ALL: [ArlType; 18] = [
        ArlType::Array,
        ArlType::Bitmask,
        ArlType::Boolean,
        ArlType::Date,
        ArlType::Dictionary,
        ArlType::Integer,
        ArlType::Matrix,
        ArlType::Name,
        ArlType::NameTree,
        ArlType::Null,
        ArlType::Number,
        ArlType::NumberTree,
        ArlType::Rectangle,
        ArlType::Stream,
        ArlType::String,
        ArlType::StringAscii,
        ArlType::StringByte,
        ArlType::StringText,
    ];

    /// Its name in the model: `name-tree`, `string-text`.
    pub fn name(self) -> &'static str {
        match self {
            ArlType::Array => "array",
            ArlType::Bitmask => "bitmask",
            ArlType::Boolean => "boolean",
            ArlType::Date => "date",
            ArlType::Dictionary => "dictionary",
            ArlType::Integer => "integer",
            ArlType::Matrix => "matrix",
            ArlType::Name => "name",
            ArlType::NameTree => "name-tree",
            ArlType::Null => "null",
            ArlType::Number => "number",
            ArlType::NumberTree => "number-tree",
            ArlType::Rectangle => "rectangle",
            ArlType::Stream => "stream",
            ArlType::String => "string",
            ArlType::StringAscii => "string-ascii",
            ArlType::StringByte => "string-byte",
            ArlType::StringText => "string-text",
        }
    }

    /// The type the model names `name`.
    pub fn named(name: &str) -> Option<ArlType> {
        ArlType::ALL.into_iter().find(|t| t.name() == name)
    }
}

impl WrapFn {
    /// All 5, in the order [`WrapFn`] lists them.
    pub const ALL: [WrapFn; 5] = [
        WrapFn::SinceVersion,
        WrapFn::BeforeVersion,
        WrapFn::IsPDFVersion,
        WrapFn::Deprecated,
        WrapFn::Extension,
    ];

    /// Its name, less `fn:`.
    pub fn name(self) -> &'static str {
        match self {
            WrapFn::SinceVersion => "SinceVersion",
            WrapFn::BeforeVersion => "BeforeVersion",
            WrapFn::IsPDFVersion => "IsPDFVersion",
            WrapFn::Deprecated => "Deprecated",
            WrapFn::Extension => "Extension",
        }
    }
}

/// A cell that holds an item per type, `[a,b];[];[c]`, as its `n` items
/// with their brackets off (INTERNAL_GRAMMAR.md, "TSV Data Fields"); `n`
/// empty ones for an empty cell.
fn groups<'a>(cell: &'a str, n: usize, column: &'static str) -> Result<Vec<&'a str>, TableError> {
    if cell.is_empty() {
        return Ok(vec![""; n]);
    }
    let groups = split(cell, ';')?
        .into_iter()
        .map(|g| {
            let inner = g.strip_prefix('[').and_then(|g| g.strip_suffix(']'));
            inner
                .filter(|inner| balanced(inner))
                .ok_or_else(|| TableError::Group {
                    column,
                    group: g.to_string(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if groups.len() != n {
        return Err(TableError::Count {
            column,
            items: groups.len(),
            types: n,
        });
    }
    Ok(groups)
}

/// A type's list of values or links, `a,fn:SinceVersion(1.5,b)`, each
/// unwrapped; none for an empty one.
fn items(group: &str) -> Result<Vec<Wrapped<String>>, TableError> {
    if group.is_empty() {
        return Ok(Vec::new());
    }
    split(group, ',')?
        .into_iter()
        .map(|item| {
            let (wraps, item) = unwrap(item)?;
            if item.is_empty() {
                return Err(TableError::EmptyItem {
                    list: group.to_string(),
                });
            }
            let item = item.to_string();
            Ok(Wrapped { wraps, item })
        })
        .collect()
}

/// `item` with the version predicates around it taken off, outermost
/// first; an error for one that doesn't read, a version that isn't one
/// among them.
fn unwrap(mut item: &str) -> Result<(Vec<Wrap>, &str), TableError> {
    let mut wraps = Vec::new();
    while let Some((wrap, inner)) = wrapper(item) {
        wraps.push(wrap);
        item = inner;
    }
    let name = item.strip_prefix("fn:").and_then(|f| f.split_once('('));
    if let Some((name, _)) = name {
        if WrapFn::ALL.into_iter().any(|f| f.name() == name) {
            return Err(TableError::Wrap {
                item: item.to_string(),
            });
        }
    }
    Ok((wraps, item))
}

/// `fn:F(arg,inner)`, when `F` is a [`WrapFn`], `arg` its version (for
/// `Extension`, its name) and the call the whole of `item`, as `F` of
/// `arg`, and `inner`.
fn wrapper(item: &str) -> Option<(Wrap, &str)> {
    let (name, args) = item.strip_prefix("fn:")?.split_once('(')?;
    let func = WrapFn::ALL.into_iter().find(|f| f.name() == name)?;
    let (arg, inner) = args.strip_suffix(')')?.split_once(',')?;
    if arg.contains(['(', ')']) || !balanced(inner) {
        return None;
    }
    if func != WrapFn::Extension && version(arg).is_none() {
        return None;
    }
    let arg = arg.to_string();
    Some((Wrap { func, arg }, inner))
}

/// A version as the model writes one: `1.0` to `1.7`, or `2.0`
/// (INTERNAL_GRAMMAR.md, "Parameters to predicates").
pub fn version(text: &str) -> Option<Version> {
    match text.as_bytes() {
        [b'1', b'.', minor @ b'0'..=b'7'] => Some(Version::new(1, minor - b'0')),
        b"2.0" => Some(Version::new(2, 0)),
        _ => None,
    }
}

/// `s` split at each `sep` outside brackets, parentheses and single-quoted
/// strings; an error when those don't pair up.
pub(crate) fn split(s: &str, sep: char) -> Result<Vec<&str>, TableError> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut open = Vec::new();
    let mut quoted = false;
    for (i, c) in s.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            _ if quoted => {}
            '[' => open.push(']'),
            '(' => open.push(')'),
            ']' | ')' => {
                if open.pop() != Some(c) {
                    return Err(TableError::OutOfPlace {
                        text: s.to_string(),
                        found: c,
                    });
                }
            }
            _ if c == sep && open.is_empty() => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    if quoted || !open.is_empty() {
        return Err(TableError::Open {
            text: s.to_string(),
        });
    }
    parts.push(&s[start..]);
    Ok(parts)
}

/// Whether the brackets, parentheses and quotes in `s` pair up (a cell
/// holds no tab, so it splits nowhere).
fn balanced(s: &str) -> bool {
    split(s, '\t').is_ok()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::COMMIT;

    /// The model's folder in this crate, from which the build embedded it.
    fn dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("model")
    }

    #[test]
    fn every_table_reads() {
        let g = Grammar::embedded();
        let added: BTreeSet<&str> = (additions().iter())
            .filter(|a| a.table_added)
            .map(|a| a.table)
            .collect();
        assert_eq!(g.tables().count(), 613 + added.len());
        assert_eq!(model::TABLES.len(), 613 + added.len());
        for &(name, text) in model::TABLES {
            let mut lines = text.lines();
            assert_eq!(lines.next(), Some(HEADER.join("\t").as_str()), "{name}");
            for (i, line) in lines.enumerate() {
                assert_eq!(line.split('\t').count(), 12, "{name}: line {}", i + 2);
            }
        }
        let mut used = BTreeSet::new();
        for table in g.tables() {
            for row in &table.rows {
                let at = format!("{}: line {}", table.name, row.line);
                let n = row.r#type.len();
                assert!(n > 0, "{at}");
                assert_eq!(row.indirect_reference.len(), n, "{at}");
                assert_eq!(row.possible_values.len(), n, "{at}");
                assert_eq!(row.special_case.len(), n, "{at}");
                assert_eq!(row.link.len(), n, "{at}");
                used.extend(row.r#type.iter().map(|t| t.item));
                for link in row.link.iter().flatten() {
                    assert!(g.table(&link.item).is_some(), "{at}: {}", link.item);
                }
                // An array, dictionary or stream always links to its tables, a
                // simple type never (INTERNAL_GRAMMAR.md, "Column 11 - Link").
                for (ty, links) in row.r#type.iter().zip(&row.link) {
                    match ty.item {
                        ArlType::Array | ArlType::Dictionary | ArlType::Stream => {
                            assert!(!links.is_empty(), "{at}: {} links nowhere", ty.item.name())
                        }
                        ArlType::NameTree | ArlType::NumberTree => {}
                        simple => assert!(links.is_empty(), "{at}: {} links", simple.name()),
                    }
                }
            }
        }
        assert_eq!(
            used,
            BTreeSet::from(ArlType::ALL),
            "the model uses every type and no other"
        );
    }

    /// Each table's key, its first cell.
    fn key(line: &str) -> &str {
        line.split('\t').next().unwrap()
    }

    #[test]
    fn the_embedded_model_is_the_folders_with_its_corrections_and_additions() {
        assert!(!corrections().is_empty() && !additions().is_empty());
        let upstream: BTreeMap<String, String> = std::fs::read_dir(dir().join("tsv/latest"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .map(|path| {
                let name = path.file_stem().unwrap().to_str().unwrap().to_string();
                (name, std::fs::read_to_string(&path).unwrap())
            })
            .collect();
        let added: BTreeSet<&str> = (additions().iter())
            .filter(|a| a.table_added)
            .map(|a| a.table)
            .collect();
        let names: BTreeSet<&str> = model::TABLES.iter().map(|&(name, _)| name).collect();
        let expected: BTreeSet<&str> = upstream.keys().map(String::as_str).chain(added).collect();
        assert_eq!(names, expected);
        for &(name, text) in model::TABLES {
            let rows: Vec<&Addition> = additions().iter().filter(|a| a.table == name).collect();
            let (adds, rest): (Vec<&str>, Vec<&str>) = (text.lines())
                .skip(1)
                .partition(|line| rows.iter().any(|a| a.key == key(line)));
            assert_eq!(adds.len(), rows.len(), "{name}: its rows added");
            let Some(up) = upstream.get(name) else {
                assert!(rest.is_empty(), "{name}: rows of an added table not listed");
                continue;
            };
            // Upstream's rows, each correction made to its cell as it was.
            let header: Vec<&str> = up.lines().next().unwrap().split('\t').collect();
            let mut want: Vec<String> = up.lines().skip(1).map(String::from).collect();
            for c in corrections().iter().filter(|c| c.table == name) {
                let column = header.iter().position(|h| *h == c.column).unwrap();
                let line = want.iter_mut().find(|l| key(l) == c.key).unwrap();
                let mut cells: Vec<&str> = line.split('\t').collect();
                assert_eq!(cells[column], c.upstream, "{name}/{}", c.key);
                cells[column] = c.corrected;
                *line = cells.join("\t");
            }
            assert_eq!(rest, want, "{name}");
        }
        let loaded = Grammar::load(&dir().join("tsv/latest")).unwrap();
        assert_ne!(loaded.hash(), Grammar::embedded().hash());
    }

    /// The model's hashes, as xxHash 0.8.2's reference computes them (the
    /// command in [`Grammar::hash`]'s doc): over `model/tsv/latest`,
    /// upstream's tables as they are; and over the embedded ones, those
    /// with the corrections made. They move with the model and its
    /// corrections, and `model/SOURCE.md`'s steps say so.
    #[test]
    fn the_models_hashes_are_the_references() {
        let upstream = Grammar::load(&dir().join("tsv/latest")).unwrap();
        assert_eq!(format!("{:016x}", upstream.hash()), "c2bdcab22f823d8d");
        let embedded = Grammar::embedded();
        assert_eq!(format!("{:016x}", embedded.hash()), "fc22d1fadc1fd269");
    }

    #[test]
    fn the_embedded_commit_is_the_one_named() {
        let source = std::fs::read_to_string(dir().join("SOURCE.md")).unwrap();
        assert!(source.contains(COMMIT), "SOURCE.md doesn't name {COMMIT}");
    }

    #[test]
    fn rows_are_found_by_key_index_repeat_or_wildcard() {
        let g = Grammar::embedded();
        let key = |table: &str, at: At| g.table(table).unwrap().row(at).map(|r| r.key.as_str());
        assert_eq!(key("Catalog", At::Key(b"Pages")), Some("Pages"));
        assert_eq!(
            key("Catalog", At::Key(b"Nonesuch")),
            None,
            "Catalog has no wildcard"
        );
        assert_eq!(key("DocInfo", At::Key(b"Nonesuch")), Some("*"));
        // `0`, then the pairs `1*` and `2*` (INTERNAL_GRAMMAR.md, "Column 1 - Key").
        let inks: Vec<_> = (0..6)
            .map(|i| key("ArrayOfOPI2Inks", At::Index(i)))
            .collect();
        let expected = ["0", "1*", "2*", "1*", "2*", "1*"];
        assert_eq!(inks, expected.map(Some));
        assert_eq!(key("ArrayOfQuadPoints", At::Index(17)), Some("1*"));
        assert_eq!(key("ArrayOfAnnots", At::Index(4)), Some("*"));
    }

    #[test]
    fn version_predicates_come_off_types_values_and_links() {
        let g = Grammar::embedded();
        let row = |table: &str, key: &[u8]| g.table(table).unwrap().row(At::Key(key)).unwrap();
        let wrap = |func, arg: &str| Wrap {
            func,
            arg: arg.to_string(),
        };

        let n = row("Appearance", b"N");
        let types: Vec<_> = n.r#type.iter().map(|t| (t.item, t.wraps.clone())).collect();
        assert_eq!(
            types,
            [
                (ArlType::Dictionary, vec![wrap(WrapFn::SinceVersion, "1.3")]),
                (ArlType::Stream, vec![]),
            ]
        );
        assert_eq!(
            n.link[1]
                .iter()
                .map(|l| l.item.as_str())
                .collect::<Vec<_>>(),
            [
                "XObjectFormType1",
                "XObjectFormPS",
                "XObjectFormPSpassthrough"
            ]
        );

        let filter = row("XObjectImage", b"Filter");
        let flate = filter.possible_values[1]
            .iter()
            .find(|v| v.item == "FlateDecode")
            .unwrap();
        assert_eq!(flate.wraps, [wrap(WrapFn::SinceVersion, "1.2")]);
        assert!(
            filter.possible_values[0].is_empty(),
            "an array of filters has no values listed"
        );

        let annots = g.table("ArrayOfAnnots").unwrap().row(At::Index(0)).unwrap();
        let projection = annots.link[0]
            .iter()
            .find(|l| l.item == "AnnotProjection")
            .unwrap();
        assert_eq!(
            projection.wraps,
            [
                wrap(WrapFn::IsPDFVersion, "1.7"),
                wrap(WrapFn::Extension, "ADBE_Extn3")
            ]
        );
    }

    /// A table that breaks a rule doesn't load, and the error names its
    /// file, the line and the rule.
    #[test]
    fn a_broken_table_says_where() {
        let catalog = model::TABLES.iter().find(|t| t.0 == "Catalog").unwrap().1;
        let line = catalog.lines().count() + 1;
        for (what, row, why) in [
            (
                "a field short",
                "Bogus\tname\t1.0\t\tFALSE\tFALSE\tFALSE\t\t\t\t",
                "11 fields, not 12",
            ),
            (
                "a type that isn't",
                "Bogus\tdict\t1.0\t\tFALSE\tFALSE\tFALSE\t\t\t\t[]\t",
                "Type \"dict\" isn't one of Arlington's",
            ),
            (
                "a link for one type of two",
                "Bogus\tarray;dictionary\t1.0\t\tFALSE\tFALSE\tFALSE\t\t\t\t[ArrayOfAnnots]\t",
                "Link has 1 items for 2 types",
            ),
            (
                "a link to a missing table",
                "Bogus\tdictionary\t1.0\t\tFALSE\tFALSE\tFALSE\t\t\t\t[Nonesuch]\t",
                "links to Nonesuch, which has no table",
            ),
            (
                "a type's version that isn't one",
                "Bogus\tfn:SinceVersion(1.9,name)\t1.0\t\tFALSE\tFALSE\tFALSE\t\t\t\t[]\t",
                "\"fn:SinceVersion(1.9,name)\": a version predicate that doesn't read",
            ),
            (
                "a value's version that isn't one",
                "Bogus\tname\t1.0\t\tFALSE\tFALSE\tFALSE\t\t[A,fn:SinceVersion(3,B)]\t\t[]\t",
                "\"fn:SinceVersion(3,B)\": a version predicate that doesn't read",
            ),
        ] {
            let broken = format!("{catalog}{row}\n");
            let files = model::TABLES.iter().map(|&(name, text)| File {
                name: name.to_string(),
                label: format!("{name}.tsv").into(),
                text: if name == "Catalog" {
                    broken.as_str()
                } else {
                    text
                }
                .into(),
            });
            let Err(e) = Grammar::from_files(files) else {
                panic!("{what}: the model still read");
            };
            let e = format!("{e:#}");
            assert!(
                e.contains("Catalog.tsv")
                    && e.contains(&format!("line {line}:"))
                    && e.contains(why),
                "{what}: {e}"
            );
        }
    }

    /// A page's `Rotate` as a row's text, with `changed` cells (by
    /// [`HEADER`]'s names) in place of its own.
    fn rotate(changed: &[(&str, &str)]) -> String {
        let mut cells = [
            "Rotate", "integer", "1.0", "", "FALSE", "FALSE", "TRUE", "0", "", "", "", "",
        ];
        for &(column, cell) in changed {
            cells[HEADER.iter().position(|h| *h == column).unwrap()] = cell;
        }
        cells.join("\t")
    }

    /// What a model of one table, `T`, whose file `T.tsv` is `text`, says
    /// when it doesn't read; none when it does.
    fn one_table(text: &str) -> Option<String> {
        let file = File {
            name: "T".to_string(),
            label: "T.tsv".into(),
            text: text.into(),
        };
        Grammar::from_files([file]).err().map(|e| format!("{e:#}"))
    }

    /// Each rule a table breaks is said word for word, after its file and,
    /// for a rule of a line, the line.
    #[test]
    fn each_rule_a_table_breaks_is_said_word_for_word() {
        let header = HEADER.join("\t");
        let table = |rows: &[&str]| format!("{header}\n{}\n", rows.join("\n"));
        let row = rotate(&[]);
        assert_eq!(one_table(&table(&[&row])), None);
        let cell = |column, cell| table(&[&rotate(&[(column, cell)])]);
        let short: Vec<&str> = row.split('\t').take(11).collect();
        let cases = [
            (
                format!("{header}\n{row}"),
                "T.tsv: its last line isn't ended",
            ),
            (
                format!("{header}\r\n{row}\r\n"),
                "T.tsv: line 1: CRLF line ends (.gitattributes keeps the model's bytes as they are)",
            ),
            (
                format!("Key\tType\n{row}\n"),
                "T.tsv: line 1: not upstream's header",
            ),
            (
                table(&[&row, &row]),
                "T.tsv: line 3: a second row for Rotate",
            ),
            (format!("{header}\n"), "T.tsv: no rows"),
            (
                table(&[&rotate(&[("Key", "0*")]), &row]),
                "T.tsv: line 3: repeating rows come last, numbered from the element they start at",
            ),
            (
                table(&[&rotate(&[("Type", "dictionary"), ("Link", "[Nonesuch]")])]),
                "T.tsv: line 2: links to Nonesuch, which has no table",
            ),
            (
                table(&[&short.join("\t")]),
                "T.tsv: line 2: 11 fields, not 12",
            ),
            (cell("Type", ""), "T.tsv: line 2: no Type"),
            (
                cell("Type", "fn:SinceVersion(9.9,integer)"),
                "T.tsv: line 2: \"fn:SinceVersion(9.9,integer)\": a version predicate that doesn't read",
            ),
            (
                cell("Type", "int"),
                "T.tsv: line 2: Type \"int\" isn't one of Arlington's",
            ),
            (
                cell("Inheritable", "MAYBE"),
                "T.tsv: line 2: Inheritable is \"MAYBE\", not TRUE or FALSE",
            ),
            (
                cell("PossibleValues", "0"),
                "T.tsv: line 2: PossibleValues: \"0\" isn't one bracketed item",
            ),
            (
                cell("PossibleValues", "[0];[90]"),
                "T.tsv: line 2: PossibleValues has 2 items for 1 types",
            ),
            (
                cell("PossibleValues", "[0,,90]"),
                "T.tsv: line 2: \"0,,90\" has an empty item",
            ),
            (
                cell("PossibleValues", "[0)]"),
                "T.tsv: line 2: \"[0)]\": a ) out of place",
            ),
            (
                cell("PossibleValues", "[0"),
                "T.tsv: line 2: \"[0\": a bracket, parenthesis or quote left open",
            ),
        ];
        for (text, said) in cases {
            assert_eq!(one_table(&text).as_deref(), Some(said), "{text:?}");
        }
    }

    /// A folder that doesn't load says why, word for word: what std said
    /// reading it or a table, after the path; or the rule, after a table's
    /// path.
    #[test]
    fn a_folder_that_doesnt_load_says_why() {
        let dir = std::env::temp_dir().join(format!("pdf-arlington-load-{}", std::process::id()));
        let folder = |name: &str| {
            let folder = dir.join(name);
            std::fs::create_dir_all(&folder).unwrap();
            folder
        };
        let said = |dir: &Path| match Grammar::load(dir) {
            Ok(_) => panic!("{} loaded", dir.display()),
            Err(e) => format!("{e:#}"),
        };
        let missing = dir.join("missing");
        let why = std::fs::read_dir(&missing).unwrap_err();
        assert_eq!(
            said(&missing),
            format!("reading {}: {why}", missing.display())
        );
        let empty = folder("empty");
        std::fs::write(empty.join("README.md"), "no tables\n").unwrap();
        assert_eq!(said(&empty), format!("{} has no tables", empty.display()));
        let binary = folder("binary");
        let table = binary.join("T.tsv");
        std::fs::write(&table, b"\xff\n").unwrap();
        let why = std::fs::read_to_string(&table).unwrap_err();
        assert_eq!(said(&binary), format!("reading {}: {why}", table.display()));
        let broken = folder("broken");
        let table = broken.join("T.tsv");
        std::fs::write(&table, format!("{}\n", HEADER.join("\t"))).unwrap();
        assert_eq!(said(&broken), format!("{}: no rows", table.display()));
        // A name that isn't UTF-8, where the file system takes one.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let named = folder("named");
            let table = named.join(std::ffi::OsStr::from_bytes(b"\xff.tsv"));
            std::fs::write(&table, "").unwrap();
            assert_eq!(
                said(&named),
                format!("{}: not a UTF-8 name", table.display())
            );
        }
    }
}
