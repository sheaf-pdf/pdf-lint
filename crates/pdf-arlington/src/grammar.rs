//! The model's tables, read from its TSV files (upstream's README.md, "TSV
//! Data Overview"; INTERNAL_GRAMMAR.md, "TSV file rules" and "TSV Data
//! Fields"). The Type cell is split into its types here, and a cell with
//! an item per type (IndirectReference, PossibleValues, SpecialCase, Link)
//! into those items; the version predicates around a type, a value or a
//! link are taken off and kept beside it. What a predicate means is the
//! checker's to say.

use std::collections::{BTreeMap, HashSet, btree_map};
use std::fmt;
use std::io;
use std::iter::FusedIterator;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdf_objects::Version;
use sha2::{Digest as _, Sha256};

use crate::predicate::Parses;

pub(crate) mod model {
    include!(concat!(env!("OUT_DIR"), "/model.rs"));
}

/// A cell the embedded model has corrected, where upstream's contradicts
/// ISO 32000-2: a line of `model/corrections.tsv`, which `build.rs` makes
/// to the embedded copy only, failing when upstream's cell isn't
/// `upstream` any more.
///
/// Not exhaustive: read by its fields, and never built outside this crate,
/// so a field more is a minor release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
#[must_use]
pub fn corrections() -> &'static [Correction] {
    model::CORRECTIONS
}

/// A row the embedded model adds, where upstream lacks what a reference
/// defines: a row of a file in `model/additions`, in upstream's format,
/// which `build.rs` adds to the embedded copy only, failing when upstream
/// has the table, or the table's row, itself.
///
/// Not exhaustive: read by its fields, and never built outside this crate,
/// so a field more is a minor release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
#[must_use]
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

/// The model: its tables, by name; and, once a [`FileCx`](crate::FileCx)
/// given it asks, the predicates asked through it, each parsed once and kept
/// for as long as the model is ([`FileCx::grammar`](crate::FileCx::grammar)).
pub struct Grammar {
    tables: BTreeMap<String, Table>,
    /// Its tables' files' SHA-256 ([`Grammar::hash`]).
    hash: Digest,
    /// Its rows' predicates, each parsed the first time it's asked for:
    /// made the first time a [`FileCx`](crate::FileCx) given the model asks
    /// ([`Grammar::parses`]), so a caller that never gives one pays nothing.
    parses: OnceLock<Parses>,
}

/// How many tables it has, and its hash: each [`Table`] shows its own
/// rows.
impl fmt::Debug for Grammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Grammar")
            .field("tables", &self.tables.len())
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

/// Two models are equal when their digests are ([`Grammar::hash`]): when
/// their tables' files are the same, byte for byte, as SHA-256 is made so
/// that no two inputs that differ can be found with one digest. What either
/// keeps of the predicates asked through it is no part of what it is.
impl PartialEq for Grammar {
    fn eq(&self, other: &Grammar) -> bool {
        self.hash == other.hash
    }
}

impl Eq for Grammar {}

/// Its digest's hash, as models are equal by their digests. What it keeps
/// of its predicates is behind a lock, which clippy's `mutable_key_type`
/// reads as a key that can change; neither this nor `==` reads it, and a
/// map keyed by the digest ([`Grammar::hash`]) is keyed alike.
impl std::hash::Hash for Grammar {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

/// What names a model ([`Grammar::hash`]): the SHA-256 of its tables'
/// files, 32 bytes, which [`Display`](fmt::Display) and
/// [`Debug`](fmt::Debug) write as 64 lowercase hex digits, as `sha256sum`
/// does. Ordered by its bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    /// Its 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Its 64 lowercase hex digits.
impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// Its 64 lowercase hex digits, as [`Display`](fmt::Display) writes them.
impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// One object's table: `Catalog`, `AnnotSquare`, `ArrayOfAnnots`.
///
/// Not exhaustive: read by its fields, and never built outside this crate
/// (a model's loader makes it), so a field more is a minor release.
#[derive(Debug)]
#[non_exhaustive]
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
///
/// Exhaustive on purpose: a value in an object is under a key or at an
/// index, and nowhere else, so a match on both is checked by the compiler;
/// another would be a breaking release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At<'a> {
    /// Under a dictionary's key.
    Key(&'a [u8]),
    /// An array's element, by its index.
    Index(usize),
}

/// One row: a key of a dictionary or an element of an array. The fields
/// are upstream's columns, in their order.
///
/// Not exhaustive: read by its fields, and never built outside this crate
/// (a model's loader makes it), so a field more (where its line is in its
/// file's bytes, say, or a cell's parse) is a minor release.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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
///
/// Not exhaustive: these are the model's types, which upstream may add
/// to, so a match on them takes a wildcard, and a type more is a minor
/// release. [`ArlType::ALL`] lists every one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
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
///
/// Not exhaustive: read by its fields, and never built outside this crate
/// (a model's loader makes it), so a field more is a minor release.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Wrapped<T> {
    /// The version predicates around it, outermost first.
    pub wraps: Vec<Wrap>,
    /// The item inside them.
    pub item: T,
}

/// A version predicate around an item.
///
/// Not exhaustive: read by its fields, and never built outside this crate
/// (a model's loader makes it), so a field more is a minor release.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Wrap {
    /// Which predicate it is.
    pub func: WrapFn,
    /// Its first argument: a version (`1.5`, which [`version`] reads), or
    /// for `Extension` the extension's name.
    pub arg: String,
}

/// The predicates that wrap an item: those INTERNAL_GRAMMAR.md's "Column
/// 11 - Link" allows around a link, which wrap types and values too.
///
/// Not exhaustive: these are the model's, which upstream may add to, so a
/// match on them takes a wildcard, and a predicate more is a minor
/// release. [`WrapFn::ALL`] lists every one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
    /// A table's name holds a tab, a line feed or a carriage return,
    /// which cut the model's files into cells and lines: in the bytes
    /// [`Grammar::hash`] is of, each table's name on a line of its own
    /// before its text, such a name could read as a table's row, and two
    /// models could hash alike. Said before the table is read, by its
    /// path as [`Debug`](fmt::Debug) writes it, so the character shows.
    #[error(
        "{path:?}: a table's name can't hold a tab or a line break, which the model's hash would read as a table's text"
    )]
    Separator {
        /// The table's file, as [`Error::Table`] names it.
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
pub(crate) struct File<'a> {
    pub(crate) name: String,
    pub(crate) label: PathBuf,
    pub(crate) text: std::borrow::Cow<'a, str>,
}

impl Grammar {
    /// The model this crate embeds: upstream's `tsv/latest` at
    /// [`COMMIT`](crate::COMMIT), with the [`corrections`] made and the
    /// [`additions`] added, read the first time it's asked for. It's one
    /// for the process, so what it keeps of the predicates asked through it
    /// ([`FileCx::grammar`](crate::FileCx::grammar)) is kept for the
    /// process, and every file and thread shares it.
    ///
    /// # Panics
    ///
    /// Naming the file and line, when the embedded model doesn't read,
    /// which its tests rule out.
    #[must_use]
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
    /// `tsv/latest`, newer than the embedded one, say.
    ///
    /// # Errors
    ///
    /// An [`Error`] where the model doesn't read: `dir`, or a table in it,
    /// that can't be read, a table's name that isn't UTF-8, or no tables;
    /// the file and line of a row that doesn't read, or that links to a
    /// table `dir` doesn't have ([`Error::Table`]); and a table whose name
    /// holds a tab or a line break ([`Error::Separator`]), before it's
    /// read.
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
            if !names_a_table(name) {
                return Err(Error::Separator { path: path.clone() });
            }
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

    pub(crate) fn from_files<'a>(
        files: impl IntoIterator<Item = File<'a>>,
    ) -> Result<Grammar, Error> {
        let mut files: Vec<File> = files.into_iter().collect();
        files.sort_by_cached_key(|file| format!("{}.tsv", file.name));
        // The bytes `hash` says it's of, given a piece at a time.
        let mut sha = Sha256::new();
        for file in &files {
            if !names_a_table(&file.name) {
                return Err(Error::Separator {
                    path: file.label.clone(),
                });
            }
            sha.update(file.name.as_bytes());
            sha.update(b".tsv\n");
            sha.update(file.text.as_bytes());
        }
        let hash = Digest(sha.finalize().into());
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
        Ok(Grammar {
            tables,
            hash,
            parses: OnceLock::new(),
        })
    }

    /// Its rows' predicates, each parsed the first time it's asked for, on
    /// whichever thread asks: made the first time it's asked for.
    pub(crate) fn parses(&self) -> &Parses {
        self.parses.get_or_init(|| Parses::of(self))
    }

    /// What names the model, as reports give it: the SHA-256 of each
    /// table's file in byte order of the files' names, its name
    /// (`Catalog.tsv`), a newline, and its bytes. Two models are alike when
    /// it is, wherever each was read from, and equal (`==`); no table's
    /// name holds a tab or a line break ([`Error::Separator`]), so no two
    /// models give it the same bytes. With coreutils, in a `tsv/latest`
    /// folder: `for f in $(LC_ALL=C ls *.tsv); do printf '%s\n' "$f"; cat
    /// "$f"; done | sha256sum`. The embedded model's is of its tables with
    /// the [`corrections`] made and the [`additions`] added, so it isn't
    /// upstream's at the same commit, which [`Grammar::load`] reads as it
    /// is.
    #[must_use]
    pub const fn hash(&self) -> Digest {
        self.hash
    }

    /// The table named `name`.
    #[must_use]
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.get(name)
    }

    /// Every table, in order of name.
    pub fn tables(&self) -> Tables<'_> {
        Tables {
            tables: self.tables.values(),
        }
    }
}

/// A model's tables, in order of name: what [`Grammar::tables`] gives. It
/// knows how many are left, walks from either end, and a `count`,
/// `for_each` or `fold` over it goes through them in one pass.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Clone)]
pub struct Tables<'a> {
    tables: btree_map::Values<'a, String, Table>,
}

/// How many are left; not the tables, each of which shows its rows.
impl fmt::Debug for Tables<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tables")
            .field("left", &self.tables.len())
            .finish()
    }
}

impl<'a> Iterator for Tables<'a> {
    type Item = &'a Table;

    #[inline]
    fn next(&mut self) -> Option<&'a Table> {
        self.tables.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.tables.size_hint()
    }

    /// The last, from the map's end, not by walking to it.
    #[inline]
    fn last(self) -> Option<&'a Table> {
        self.tables.last()
    }

    /// One pass over the map, as `count`, `for_each` and `sum` fold.
    #[inline]
    fn fold<B, F: FnMut(B, &'a Table) -> B>(self, init: B, f: F) -> B {
        self.tables.fold(init, f)
    }
}

impl<'a> DoubleEndedIterator for Tables<'a> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a Table> {
        self.tables.next_back()
    }

    /// One pass over the map from its end, as `rev` folds.
    #[inline]
    fn rfold<B, F: FnMut(B, &'a Table) -> B>(self, init: B, f: F) -> B {
        self.tables.rfold(init, f)
    }
}

impl ExactSizeIterator for Tables<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.tables.len()
    }
}

impl FusedIterator for Tables<'_> {}

/// The embedded model, read once ([`Grammar::embedded`]).
static EMBEDDED: OnceLock<Grammar> = OnceLock::new();

/// Whether `name` can name a table: it holds no tab, line feed or carriage
/// return ([`Error::Separator`]).
fn names_a_table(name: &str) -> bool {
    !name.contains(['\t', '\n', '\r'])
}

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
    #[must_use]
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
    #[must_use]
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
    /// Every one, once, in the model's order (its names', alphabetical): a
    /// slice, not an array of a fixed length, so a type can come without a
    /// breaking release.
    pub const ALL: &'static [ArlType] = &[
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
    #[must_use]
    pub const fn name(self) -> &'static str {
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
    #[must_use]
    pub fn named(name: &str) -> Option<ArlType> {
        ArlType::ALL.iter().copied().find(|t| t.name() == name)
    }
}

impl WrapFn {
    /// Every one, once, in the order [`WrapFn`] lists them: a slice, not an
    /// array of a fixed length, so a predicate can come without a breaking
    /// release.
    pub const ALL: &'static [WrapFn] = &[
        WrapFn::SinceVersion,
        WrapFn::BeforeVersion,
        WrapFn::IsPDFVersion,
        WrapFn::Deprecated,
        WrapFn::Extension,
    ];

    /// Its name, less `fn:`.
    #[must_use]
    pub const fn name(self) -> &'static str {
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
    if let Some((name, _)) = name
        && WrapFn::ALL.iter().copied().any(|f| f.name() == name)
    {
        return Err(TableError::Wrap {
            item: item.to_string(),
        });
    }
    Ok((wraps, item))
}

/// `fn:F(arg,inner)`, when `F` is a [`WrapFn`], `arg` its version (for
/// `Extension`, its name) and the call the whole of `item`, as `F` of
/// `arg`, and `inner`.
fn wrapper(item: &str) -> Option<(Wrap, &str)> {
    let (name, args) = item.strip_prefix("fn:")?.split_once('(')?;
    let func = WrapFn::ALL.iter().copied().find(|f| f.name() == name)?;
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
#[must_use]
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
            ArlType::ALL.iter().copied().collect::<BTreeSet<_>>(),
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

    /// The model's hashes, as coreutils' `sha256sum` computes them over the
    /// command in [`Grammar::hash`]'s doc: over `model/tsv/latest`,
    /// upstream's tables as they are; and over the embedded ones, those
    /// with the corrections made and the additions added (upstream's
    /// folder with the build's corrected and added tables over it). They
    /// move with the model, its corrections and its additions, and
    /// `model/SOURCE.md`'s steps say so.
    #[test]
    fn the_models_hashes_are_the_references() {
        let upstream = Grammar::load(&dir().join("tsv/latest")).unwrap();
        assert_eq!(
            upstream.hash().to_string(),
            "3493c24c602b0637685e453bbb01cc16f1aee2aba8cbfaa0bf336426b17d2c1b"
        );
        let embedded = Grammar::embedded();
        assert_eq!(
            embedded.hash().to_string(),
            "2bf6c3ab3fb4783fec94c0d6bdec719117128719259e4f994b8574e0d48f83bc"
        );
    }

    /// A table's text, `T` of [`rotate`]'s row with `note` its Note, so
    /// that each table of a model built of them is another.
    fn noted(note: &str) -> String {
        format!("{}\n{}\n", HEADER.join("\t"), rotate(&[("Note", note)]))
    }

    /// A model's digest is the SHA-256 of the bytes its doc says, built
    /// here apart from the loader: each table's name, `.tsv`, a newline
    /// and its text, in byte order of `<name>.tsv`, so `B` before `a-b`
    /// before `a` (`-` is before `.`), as `LC_ALL=C ls` lists them; read
    /// from a folder, and from the same files in memory in any order.
    #[test]
    fn a_models_digest_is_sha256_of_its_tables_files_framed() {
        let tables = [
            ("a", noted("one")),
            ("a-b", noted("two")),
            ("B", noted("three")),
        ];
        let mut framed = Vec::new();
        for name in ["B", "a-b", "a"] {
            let (_, text) = tables.iter().find(|(n, _)| *n == name).unwrap();
            framed.extend_from_slice(format!("{name}.tsv\n{text}").as_bytes());
        }
        let want: [u8; 32] = Sha256::digest(&framed).into();
        let dir = tempfile::Builder::new()
            .prefix("pdf-arlington-digest-")
            .tempdir()
            .unwrap();
        for (name, text) in &tables {
            std::fs::write(dir.path().join(format!("{name}.tsv")), text).unwrap();
        }
        let loaded = Grammar::load(dir.path()).unwrap();
        assert_eq!(loaded.hash().as_bytes(), &want);
        let in_memory = |order: &[usize]| {
            let files = order.iter().map(|&i| File {
                name: tables[i].0.to_string(),
                label: format!("{}.tsv", tables[i].0).into(),
                text: tables[i].1.as_str().into(),
            });
            Grammar::from_files(files).unwrap().hash()
        };
        for order in [[0, 1, 2], [2, 1, 0], [1, 0, 2]] {
            assert_eq!(in_memory(&order), loaded.hash(), "{order:?}");
        }
        let hex: String = want.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(loaded.hash().to_string(), hex);
    }

    /// A digest is written as 64 lowercase hex digits, by `Display` and
    /// `Debug` alike, and its bytes are what it's compared, ordered and
    /// hashed by.
    #[test]
    fn a_digest_is_written_in_64_lowercase_hex_digits() {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(0x4B) ^ 0xA0;
        }
        let digest = Digest(bytes);
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().any(|b| b.is_ascii_alphabetic()));
        assert_eq!(digest.to_string(), hex);
        assert_eq!(format!("{digest:?}"), hex);
        assert_eq!(digest.as_bytes(), &bytes);
        let copy = digest;
        assert_eq!(copy, digest, "Copy");
        let (low, high) = (Digest([0; 32]), Digest([0xFF; 32]));
        let mut last = [0; 32];
        last[31] = 1;
        let mut first = [0; 32];
        first[0] = 1;
        assert!(low < Digest(last) && Digest(last) < Digest(first) && Digest(first) < high);
        let set: HashSet<Digest> = [low, high, low].into_iter().collect();
        assert_eq!(set.len(), 2);
        let model = Grammar::embedded();
        let debug = format!("{model:?}");
        assert!(
            debug.contains(&format!("hash: {}", model.hash())),
            "{debug}"
        );
    }

    /// A table whose name holds a tab, a line feed or a carriage return
    /// doesn't load: from a folder, before its file is read (here a
    /// folder, which doesn't read), and from files in memory, each said by
    /// its path's `Debug`.
    #[test]
    fn a_tables_name_with_a_tab_or_a_line_break_is_refused() {
        let why = "a table's name can't hold a tab or a line break, \
                   which the model's hash would read as a table's text";
        for name in ["A\tB", "A\nB", "A\rB", "\t", "A\n"] {
            let file = |name: &str| File {
                name: name.to_string(),
                label: format!("{name}.tsv").into(),
                text: noted("").into(),
            };
            let Err(e) = Grammar::from_files([file("Z"), file(name)]) else {
                panic!("{name:?} loaded");
            };
            let label = PathBuf::from(format!("{name}.tsv"));
            assert_eq!(e.to_string(), format!("{label:?}: {why}"), "{name:?}");
            assert!(matches!(e, Error::Separator { path } if path == label));
        }
        // A name of a folder's file, where the file system takes one.
        #[cfg(target_os = "linux")]
        {
            let dir = tempfile::Builder::new()
                .prefix("pdf-arlington-names-")
                .tempdir()
                .unwrap();
            for (i, name) in ["A\tB", "A\nB", "A\rB"].into_iter().enumerate() {
                let folder = dir.path().join(i.to_string());
                std::fs::create_dir(&folder).unwrap();
                std::fs::write(folder.join("Z.tsv"), noted("")).unwrap();
                let table = folder.join(format!("{name}.tsv"));
                std::fs::create_dir(&table).unwrap();
                let e = Grammar::load(&folder).unwrap_err();
                assert_eq!(e.to_string(), format!("{table:?}: {why}"), "{name:?}");
                std::fs::remove_dir(&table).unwrap();
                std::fs::write(&table, noted("")).unwrap();
                let e = Grammar::load(&folder).unwrap_err();
                assert_eq!(e.to_string(), format!("{table:?}: {why}"), "{name:?}");
            }
        }
    }

    /// Models are equal, and hash alike, by their digests: the embedded
    /// model and a load of its tables written out are one, upstream's
    /// tables another, and upstream's with one Note changed a third, of as
    /// many tables.
    #[test]
    // What a model keeps of its predicates is behind a lock, which the
    // lint reads as a key that can change; its hash and equality read the
    // digest alone, which can't.
    #[allow(clippy::mutable_key_type)]
    fn models_are_equal_by_their_digests() {
        let embedded = Grammar::embedded();
        assert_eq!(embedded, embedded);
        let temp = tempfile::Builder::new()
            .prefix("pdf-arlington-equal-")
            .tempdir()
            .unwrap();
        let written = temp.path().join("embedded");
        std::fs::create_dir(&written).unwrap();
        for &(name, text) in model::TABLES {
            std::fs::write(written.join(format!("{name}.tsv")), text).unwrap();
        }
        let copy = Grammar::load(&written).unwrap();
        assert_eq!(copy, *embedded);
        let upstream = Grammar::load(&dir().join("tsv/latest")).unwrap();
        assert_ne!(upstream, *embedded);
        let edited = temp.path().join("edited");
        std::fs::create_dir(&edited).unwrap();
        for entry in std::fs::read_dir(dir().join("tsv/latest")).unwrap() {
            let path = entry.unwrap().path();
            let mut text = std::fs::read_to_string(&path).unwrap();
            if path.file_name().unwrap() == "Catalog.tsv" {
                // The last row's Note, its last cell, a word longer.
                assert_eq!(text.pop(), Some('\n'));
                text.push_str(" edited\n");
            }
            std::fs::write(edited.join(path.file_name().unwrap()), text).unwrap();
        }
        let edited = Grammar::load(&edited).unwrap();
        assert_eq!(edited.tables().count(), upstream.tables().count());
        assert_ne!(edited, upstream);
        let models: HashSet<&Grammar> = [embedded, &copy, &upstream, &edited].into_iter().collect();
        assert_eq!(models.len(), 3);
    }

    /// A model keeps its rows' predicates parsed only once a file given it
    /// asks one: a file given none, or given it but asking nothing, makes
    /// nothing of it.
    #[test]
    fn a_models_parses_are_made_the_first_time_a_file_given_it_asks() {
        use crate::predicate::{self, FileCx, Truth};
        use pdf_objects::{Dict, Object, ObjectId, dict, resolve_fn};

        let model = Grammar::load(&dir().join("tsv/latest")).unwrap();
        let rotate = model.table("PageObject").unwrap().row(At::Key(b"Rotate"));
        let text = &rotate.unwrap().possible_values[0][0].item;
        let (trailer, page) = (Dict::new(), Object::from(dict! { "Rotate" => 90 }));
        let resolve = resolve_fn(|_: ObjectId| None);
        let holds =
            |file: &FileCx| predicate::holds_fully(text, &file.at(&page, At::Key(b"Rotate")));
        let alone = FileCx::new(&trailer, Version::new(2, 0), &resolve);
        assert_eq!(holds(&alone), Truth::True);
        let given = FileCx::new(&trailer, Version::new(2, 0), &resolve).grammar(&model);
        assert!(model.parses.get().is_none(), "made before it's asked");
        assert_eq!(holds(&given), Truth::True);
        assert!(model.parses.get().is_some(), "not made when asked");
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

    /// The tables walked by `next` and by `fold` alike, in order of name;
    /// from the back by `next_back` and by `rfold`, and from both ends at
    /// once; each walk knowing how many are left, and giving none after
    /// its last.
    #[test]
    fn the_tables_are_walked_alike_every_way() {
        let g = Grammar::embedded();
        let names: Vec<&str> = g.tables.keys().map(String::as_str).collect();
        let n = names.len();
        let backwards: Vec<&str> = names.iter().rev().copied().collect();
        let name = |table: &'static Table| table.name.as_str();
        let push = |mut all: Vec<&'static str>, table: &'static Table| {
            all.push(name(table));
            all
        };
        let mut tables = g.tables();
        assert_eq!((tables.len(), tables.size_hint()), (n, (n, Some(n))));
        assert_eq!(format!("{tables:?}"), format!("Tables {{ left: {n} }}"));
        // By `next` alone, as `from_fn` asks for each.
        let by_next: Vec<&str> = std::iter::from_fn(|| tables.next()).map(name).collect();
        assert_eq!(by_next, names);
        assert_eq!((tables.len(), tables.size_hint()), (0, (0, Some(0))));
        assert!(tables.next().is_none() && tables.next_back().is_none());
        assert_eq!(format!("{tables:?}"), "Tables { left: 0 }");
        assert_eq!(g.tables().fold(Vec::new(), push), names);
        let mut back = g.tables();
        let by_next_back: Vec<&str> = std::iter::from_fn(|| back.next_back()).map(name).collect();
        assert_eq!(by_next_back, backwards);
        assert_eq!(g.tables().rfold(Vec::new(), push), backwards);
        assert_eq!(g.tables().rev().fold(Vec::new(), push), backwards);
        assert_eq!(g.tables().count(), n);
        assert_eq!(g.tables().last().map(name), names.last().copied());
        assert_eq!(g.tables().nth(2).map(name), Some(names[2]));
        // From both ends at once, a clone going on alone.
        let mut both = g.tables();
        assert_eq!(both.next().map(name), Some(names[0]));
        assert_eq!(both.next_back().map(name), Some(names[n - 1]));
        assert_eq!(both.len(), n - 2);
        let rest = both.clone();
        assert_eq!(both.fold(Vec::new(), push), names[1..n - 1]);
        assert_eq!(rest.len(), n - 2);
        for (table, name) in g.tables().zip(&names) {
            assert_eq!(table.name, *name);
            assert!(std::ptr::eq(table, g.table(name).unwrap()));
        }
    }

    /// [`ArlType::ALL`] and [`WrapFn::ALL`] hold every variant once, in the
    /// order its enum lists them, each at its own discriminant's place, and
    /// end at the enum's last, which a match the compiler holds whole says:
    /// so a variant more doesn't build here until it's said where it goes.
    /// Each reads back from its name, the types in their names' order.
    #[test]
    fn each_all_holds_every_variant_once_in_order() {
        for (i, &t) in ArlType::ALL.iter().enumerate() {
            assert_eq!(t as usize, i, "{t:?}");
            assert_eq!(ArlType::named(t.name()), Some(t), "{t:?}");
        }
        let last = |t: ArlType| match t {
            ArlType::StringText => true,
            ArlType::Array
            | ArlType::Bitmask
            | ArlType::Boolean
            | ArlType::Date
            | ArlType::Dictionary
            | ArlType::Integer
            | ArlType::Matrix
            | ArlType::Name
            | ArlType::NameTree
            | ArlType::Null
            | ArlType::Number
            | ArlType::NumberTree
            | ArlType::Rectangle
            | ArlType::Stream
            | ArlType::String
            | ArlType::StringAscii
            | ArlType::StringByte => false,
        };
        assert!(ArlType::ALL.last().copied().is_some_and(last));
        assert!(ArlType::ALL.is_sorted_by_key(|t| t.name()));
        assert_eq!(ArlType::named("nonesuch"), None);
        for (i, &f) in WrapFn::ALL.iter().enumerate() {
            assert_eq!(f as usize, i, "{f:?}");
            let read = WrapFn::ALL.iter().copied().find(|g| g.name() == f.name());
            assert_eq!(read, Some(f), "{f:?}");
        }
        let last = |f: WrapFn| match f {
            WrapFn::Extension => true,
            WrapFn::SinceVersion
            | WrapFn::BeforeVersion
            | WrapFn::IsPDFVersion
            | WrapFn::Deprecated => false,
        };
        assert!(WrapFn::ALL.last().copied().is_some_and(last));
    }

    /// What's `const` makes a constant: a type's name and a wrapping
    /// predicate's, a comparison's symbol and an operator's, and the trees'
    /// default caps.
    #[test]
    fn const_fns_make_constants() {
        use crate::predicate::{Arith, Cmp, Limits};
        const NAMES: [&str; 2] = [ArlType::NameTree.name(), WrapFn::IsPDFVersion.name()];
        const SYMBOLS: [&str; 2] = [Cmp::Le.symbol(), Arith::Mod.symbol()];
        const CAPS: Limits = Limits::DEFAULT;
        assert_eq!(NAMES, ["name-tree", "IsPDFVersion"]);
        assert_eq!(SYMBOLS, ["<=", " mod "]);
        assert_eq!(CAPS, Limits::default());
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
        let dir = tempfile::Builder::new()
            .prefix("pdf-arlington-load-")
            .tempdir()
            .unwrap();
        let folder = |name: &str| {
            let folder = dir.path().join(name);
            std::fs::create_dir_all(&folder).unwrap();
            folder
        };
        let said = |dir: &Path| match Grammar::load(dir) {
            Ok(_) => panic!("{} loaded", dir.display()),
            Err(e) => format!("{e:#}"),
        };
        let missing = dir.path().join("missing");
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
