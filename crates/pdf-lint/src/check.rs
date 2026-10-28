// Parts of this file are adapted from TestGrammar, the Arlington PDF
// Model's checker (pdf-association/arlington-pdf-model, `TestGrammar/src`,
// Copyright 2020-2022 PDF Association, Inc.), under the Apache License,
// Version 2.0: `Walk::score` (from `CParsePDF::recommended_link_for_object`),
// `Walk::array_rows` (from `CParsePDF::parse_object`'s order), `is_date`
// (from `is_valid_pdf_date_string`) and `Class::of` (from
// `is_second_class_pdf_name` and `is_third_class_pdf_name`). Each one's
// doc says how it differs from TestGrammar. The notices of upstream's
// NOTICE.txt (pdf-arlington's `model/NOTICE.txt`) that pertain to them:
//
//     Copyright 2020 PDF Association, Inc. https://www.pdfa.org
//
//     This material is based upon work supported by the Defense Advanced
//     Research Projects Agency (DARPA) under Contract No. HR001119C0079.
//     Any opinions, findings and conclusions or recommendations expressed
//     in this material are those of the author(s) and do not necessarily
//     reflect the views of the Defense Advanced Research Projects Agency
//     (DARPA). Approved for public release.

//! The walk: from the trailer as the file writes it, every object it
//! reaches, each checked against the table it's reached as. Links are
//! followed as TestGrammar follows them (upstream's
//! `TestGrammar/src/ParseObjects.cpp`, Apache-2.0): an object reached as
//! an annotation is held to the annotation table its keys fit best, and an
//! object is checked once, as the first table it's reached as (reached
//! again as another, it's counted: [`Report::two_contexts`]). But a
//! `_Universal` table, which fits anything (the model's link where it says
//! only "a dictionary", as for a structure element's `/OBJR /Obj`), comes
//! last: an object reached as one is checked as it only when the walk
//! reaches it as no other table. TestGrammar checks an object as the first
//! table it reaches it as, whichever, so a widget its structure tree
//! reaches before its page is checked against no annotation's table.
//!
//! Unlike TestGrammar (unless [`Options::one_table`]), a field and its
//! widget merged in one dictionary (ISO 32000-2 12.5.6.19), which the
//! model splits between two tables, is checked as both at once, and so is
//! what both link a key of theirs to, as the triggers of its `/AA`: each
//! key against the row of each table that lists it, unlisted only if
//! neither does ([`Report::two_tables`]). And a key of an object reached
//! again as another table, which it isn't checked as, is unlisted only if
//! that table doesn't list it either.
//!
//! A file is checked under the extensions it declares, as the model
//! names them ([`extension::declared`](crate::extension::declared)): what's
//! under another is as if it weren't in the model, as in TestGrammar
//! without that extension in its `--extensions` ([`Report::extensions`]).
//!
//! Only the objects held ([`Hold`]: what the caller wrote) give findings
//! and counts; the rest are walked so the held ones get their tables.
//!
//! The rules here: a value's type ([`Rule::Type`]); a required key or
//! element ([`Rule::Missing`]), found through `/Parent` for an inheritable
//! key (and a field's `/DA` in the form, its document-wide default); a key
//! from a later version than the file's ([`Rule::TooNew`]); a key, type or
//! value deprecated by the file's version, when asked for
//! ([`Rule::Deprecated`], [`Options::deprecations`], else counted only:
//! [`Report::deprecated`]); a value
//! its row doesn't list ([`Rule::Value`]); a value direct where it must be
//! an indirect reference, or the reverse ([`Rule::Indirect`],
//! [`Rule::Direct`]); a SpecialCase that comes to false
//! ([`Rule::Special`]); a reference to no object ([`Rule::Unresolved`]); a
//! malformed name or number tree ([`Rule::Tree`]); a held object the
//! walk never reached ([`Rule::Unreached`]); in an object built whole (or
//! as [`Options::unlisted`] says), a key the model doesn't list that isn't
//! exempt ([`Rule::Unlisted`]); a
//! key written twice ([`Rule::Duplicate`]); and what the reader had to
//! choose how to read ([`Rule::Reader`]). A check whose predicate can't be
//! told, or uses a function the evaluator doesn't support, or reads a
//! string's bytes the reader didn't give, is counted as not made
//! ([`Report::unchecked`]), and every key the model doesn't list is
//! counted with what kind of name it is ([`Report::unlisted`]); in an
//! object merged over a source's, by default never failed, as TestGrammar
//! reports it as information: it may be the source's.
//!
//! Each finding says where its bytes were written ([`Finding::at`]), when
//! the reader knows: a key and its value, an array's element, a dictionary
//! that lacks a key, an object nothing reaches.
//!
//! [`Report::beyond`] says which findings a writer answers for, given the
//! reports of the files it merged objects from, and
//! [`Allowances`](crate::Allowances) which of those are known and held.
//!
//! Every input is walked in bounded work: from a queue, never recursion on
//! the file (the way to an object, as long as the file makes it, is
//! dropped a step at a time too); each indirect object once; direct
//! objects nested at most [`NESTING`] deep in one; `/Parent` chains at most
//! the page tree's depth.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt::{self, Write as _};
use std::num::ParseIntError;
use std::rc::Rc;

use crate::grammar::{ArlType, At, Row, Table, WrapFn, Wrapped, grammar, version};
use crate::object::{Dict, Entry, Object, ObjectId, Span, Str, Value};
use crate::predicate::{self, Cx, FileCx, Gate, Reference, Since, TOLERANCE, Truth};
use crate::source::{Source, Warning};
use crate::text;
use crate::version::Version;

/// How deeply direct objects may nest in an indirect one before the walk
/// stops going in. A real file nests a few.
pub const NESTING: usize = 64;

/// How many `/Parent`s up an inheritable key is looked for: PDFium's
/// `kMaxPageLevel`.
const PAGE_TREE_DEPTH: usize = 1024;

/// How many references a value goes through to its object, as one chain.
const REFERENCES: usize = 64;

/// How many of the objects on the walk's way to one `parent::` may reach
/// back through: the model's deepest is `parent::parent::parent::`.
const PARENTS: usize = 8;

/// How many steps a finding's path shows: its last ones.
const PATH: usize = 24;

/// How many characters of a finding's value its display shows.
const SHOWN: usize = 80;

/// How many bytes of a name or string a finding's value shows.
const TOKEN: usize = 80;

/// How a writer made an object it wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Made {
    /// Built whole: every key in it is the writer's.
    Built,
    /// Emitted over a source's dictionary, whose keys it may keep.
    Merged,
}

/// The objects held to the grammar: what a writer wrote. The trailer as
/// written is always held, as [`Made::Merged`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    /// Every object in the file.
    All(Made),
    /// These.
    Only(BTreeMap<ObjectId, Made>),
}

impl Hold {
    /// The objects a hold file names: a line each, its object's number,
    /// generation and how it was made (`12 0 built`, `3 0 merged`), and
    /// `#` lines and blank ones besides. A [`HoldError`] names the line
    /// that doesn't read, or that names an object a second time.
    pub fn parse(text: &str) -> Result<Hold, HoldError> {
        let mut objects = BTreeMap::new();
        for (i, line) in text.lines().enumerate() {
            let line_number = i + 1;
            let words: Vec<&str> = line.split_whitespace().collect();
            let (n, g, made) = match words[..] {
                [] => continue,
                [first, ..] if first.starts_with('#') => continue,
                [n, g, made] => (n, g, made),
                _ => return Err(HoldError::Words { line: line_number }),
            };
            let n: u32 = n.parse().map_err(|error| HoldError::Number {
                line: line_number,
                number: n.to_string(),
                error,
            })?;
            let g: u16 = g.parse().map_err(|error| HoldError::Generation {
                line: line_number,
                generation: g.to_string(),
                error,
            })?;
            let made = match made {
                "built" => Made::Built,
                "merged" => Made::Merged,
                other => {
                    return Err(HoldError::Made {
                        line: line_number,
                        made: other.to_string(),
                    });
                }
            };
            if objects.insert((n, g), made).is_some() {
                return Err(HoldError::Again {
                    line: line_number,
                    id: (n, g),
                });
            }
        }
        Ok(Hold::Only(objects))
    }
}

/// Why a hold file's text doesn't read ([`Hold::parse`]): the rule a line
/// breaks, after the line. The std error under one is a field, said after
/// `: `, not a source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HoldError {
    /// A line of other than three words, neither blank nor a `#` line.
    #[error("line {line}: not a number, a generation and built or merged")]
    Words {
        /// Its line.
        line: usize,
    },
    /// An object number that isn't one: not digits, or over 32 bits.
    #[error("line {line}: {number:?} isn't an object number: {error}")]
    Number {
        /// Its line.
        line: usize,
        /// The number, as written.
        number: String,
        /// Why, as std says.
        error: ParseIntError,
    },
    /// A generation that isn't one: not digits, or over 16 bits.
    #[error("line {line}: {generation:?} isn't a generation: {error}")]
    Generation {
        /// Its line.
        line: usize,
        /// The generation, as written.
        generation: String,
        /// Why, as std says.
        error: ParseIntError,
    },
    /// A third word that's neither `built` nor `merged`.
    #[error("line {line}: {made:?} isn't built or merged")]
    Made {
        /// Its line.
        line: usize,
        /// The word.
        made: String,
    },
    /// An object a line before holds.
    #[error("line {line}: {n} {g} is held a second time", n = .id.0, g = .id.1)]
    Again {
        /// Its line.
        line: usize,
        /// The object.
        id: ObjectId,
    },
}

/// What a check is told besides the file and what it holds.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Check at this version, not the file's (its header's, or its
    /// catalog's `/Version` when later: [`file_version`]). A file a writer
    /// emitted objects over, checked at the version of the file it wrote
    /// them in, has the faults its objects have there
    /// ([`Report::beyond`]), as a key that came in at a version between
    /// the two, or one a key then requires.
    pub version: Option<Version>,
    /// Keys never found unlisted in a built object: a writer's own private
    /// keys.
    pub exempt: Vec<Exempt>,
    /// Record the path of every held value the walk reaches
    /// ([`Report::paths`]), as [`Report::by_path`] needs of a baseline.
    pub paths: bool,
    /// Check each object as one table, as TestGrammar does: a field and
    /// its widget merged in one dictionary as whichever of the two fits
    /// it best, and a key of an object reached as two tables unlisted
    /// when the first doesn't list it.
    pub one_table: bool,
    /// Which held objects a key the model doesn't list is a finding in
    /// ([`Rule::Unlisted`]); in any, it's counted ([`Report::unlisted`]).
    pub unlisted: UnlistedIn,
    /// Find what the version checked at deprecates, a key, a type or a
    /// value ([`Rule::Deprecated`]); else it's counted only
    /// ([`Report::deprecated`]).
    pub deprecations: bool,
    /// Extensions to check the file under, as the model names them
    /// (`ETSI_PAdES`), besides those it declares
    /// ([`extension::declared`](crate::extension::declared)).
    pub extensions: BTreeSet<String>,
}

/// Which held objects a key the model doesn't list is a finding in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UnlistedIn {
    /// Built objects only: a merged object's may be its source's.
    #[default]
    Built,
    /// Every held object, built or merged: with a baseline, or an update
    /// set against the revision before it, a merged object's that the
    /// source had is carried, and the rest are the writer's.
    All,
    /// None.
    Never,
}

impl UnlistedIn {
    /// As the command line names it: `built`, `all` or `never`.
    pub fn name(self) -> &'static str {
        match self {
            UnlistedIn::Built => "built",
            UnlistedIn::All => "all",
            UnlistedIn::Never => "never",
        }
    }

    /// The one `name` names.
    pub fn named(name: &str) -> Option<UnlistedIn> {
        [UnlistedIn::Built, UnlistedIn::All, UnlistedIn::Never]
            .into_iter()
            .find(|u| u.name() == name)
    }
}

/// A key a writer writes on purpose that the model doesn't list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exempt {
    pub name: Vec<u8>,
    /// Whether any key it begins is exempt too.
    pub prefix: bool,
}

impl Exempt {
    /// The keys an exempt file names: a line each, a name as a PDF file
    /// writes one, its `#xx` escapes decoded (`/MyKey`), or with a `*`
    /// after it for every key it begins (`/My*`; a name that ends in a
    /// star is written `/My#2A`); and `#` lines and blank ones besides.
    /// An [`ExemptError`] names the line that doesn't read, or that names
    /// a key a second time.
    pub fn parse(text: &str) -> Result<Vec<Exempt>, ExemptError> {
        use pdf_tokenizer::decode::How;
        use pdf_tokenizer::{Kind, Tokenizer};
        let mut keys = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line_number = i + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (written, prefix) = match line.strip_suffix('*') {
                Some(written) => (written, true),
                None => (line, false),
            };
            let tokens: Vec<_> = Tokenizer::new(written.as_bytes()).collect();
            let token = match tokens[..] {
                [token] if matches!(token.kind, Kind::Name) => token,
                _ => {
                    return Err(ExemptError::Name {
                        line: line_number,
                        written: line.to_string(),
                    });
                }
            };
            for piece in token.pieces().into_iter().flatten() {
                match piece.how {
                    How::StrayHash => {
                        return Err(ExemptError::Hash {
                            line: line_number,
                            written: line.to_string(),
                        });
                    }
                    How::NulEscape => {
                        return Err(ExemptError::Nul {
                            line: line_number,
                            written: line.to_string(),
                        });
                    }
                    _ => {}
                }
            }
            let name: Vec<u8> = token.pieces().into_iter().flat_map(|p| p.bytes()).collect();
            if name.is_empty() {
                return Err(ExemptError::Empty { line: line_number });
            }
            let key = Exempt { name, prefix };
            if keys.contains(&key) {
                return Err(ExemptError::Again {
                    line: line_number,
                    written: line.to_string(),
                });
            }
            keys.push(key);
        }
        Ok(keys)
    }
}

/// Why an exempt file's text doesn't read ([`Exempt::parse`]): the rule a
/// line breaks, after the line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExemptError {
    /// A line that isn't a name, or a name and a `*`.
    #[error("line {line}: {written:?} isn't a name, as /MyKey, or /My*")]
    Name {
        /// Its line.
        line: usize,
        /// The line as written, its white space at either end off.
        written: String,
    },
    /// A name with a `#` without two hex digits after it.
    #[error("line {line}: {written:?} has a # without two hex digits after it")]
    Hash {
        /// Its line.
        line: usize,
        /// The line as written, its white space at either end off.
        written: String,
    },
    /// A name with `#00`, NUL, which a name may not hold (ISO 32000-2,
    /// 7.3.5).
    #[error("line {line}: {written:?} has #00, which a name may not hold")]
    Nul {
        /// Its line.
        line: usize,
        /// The line as written, its white space at either end off.
        written: String,
    },
    /// A name of no bytes: `/`, or `/*`.
    #[error("line {line}: an empty name")]
    Empty {
        /// Its line.
        line: usize,
    },
    /// A key, or a prefix, a line before exempts.
    #[error("line {line}: {written:?} is exempt a second time")]
    Again {
        /// Its line.
        line: usize,
        /// The line as written, its white space at either end off.
        written: String,
    },
}

/// What a finding breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Rule {
    /// A value of none of the types its row allows at the file's version.
    Type,
    /// A required key or element that's absent, and for an inheritable
    /// key, on no `/Parent` either.
    Missing,
    /// A key or element whose SinceVersion is later than the file's
    /// version.
    TooNew,
    /// A key or element whose DeprecatedIn is the file's version or
    /// earlier; or a type or value its row allows at the file's version
    /// only as deprecated (`fn:Deprecated`). Found with
    /// [`Options::deprecations`], else counted only.
    Deprecated,
    /// A value none of its row's PossibleValues for its type is, of those
    /// the file's version lets in.
    Value,
    /// A direct value where its row's IndirectReference asks for an
    /// indirect reference.
    Indirect,
    /// An indirect reference where its row's IndirectReference asks for a
    /// direct value.
    Direct,
    /// A value its row's SpecialCase comes to false for.
    Special,
    /// A held object the walk never reached, and not under a key the
    /// model doesn't list.
    Unreached,
    /// A reference to an object the file hasn't, or a trailer the file's
    /// newest cross-reference section doesn't give.
    Unresolved,
    /// A name or number tree's node that breaks ISO 32000-2 7.9.6 or
    /// 7.9.7.
    Tree,
    /// A key or element of a [`Made::Built`] object that the model
    /// doesn't list ([`Unlisted`]), and isn't a second- or third-class
    /// name nor exempt ([`Options::exempt`]), in a table that says what
    /// the object is (not a `_Universal` one, nor the plain `Stream` one
    /// such a table links a stream to): the writer builds the object whole,
    /// so it's a key PDF doesn't define, misspelled or misplaced. In a
    /// merged object too, or in none, as [`Options::unlisted`] says.
    Unlisted,
    /// A dictionary's key written again later in it (ISO 32000-2, 7.3.7:
    /// "no two entries in the same dictionary should have the same key"):
    /// the earlier entry, which readers leave for the later.
    Duplicate,
    /// Something the reader had to choose how to read: a stream whose
    /// `/Length` doesn't end at `endstream`, a lone CR after `stream`, an
    /// object without `endobj`, a name's stray `#` or `#00`, a
    /// cross-reference table's rows not 20 bytes, a file whose last line
    /// isn't `%%EOF` alone, and the like ([`Warning`]).
    Reader,
}

/// A rule a held object breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Finding {
    /// The indirect object it's in: none for a table's trailer, or the
    /// file as a whole.
    pub object: Option<ObjectId>,
    pub made: Made,
    /// Where the walk reached it from the trailer:
    /// `Root/Pages/Kids[0]/Annots[2]`, `trailer` for the trailer, `-` for
    /// an object it never reached.
    pub path: String,
    /// The table and key of the row it breaks; for an unreached object,
    /// those of the object that references it, `-` for none; `-` and `-`
    /// for the reader's.
    pub table: String,
    pub key: String,
    pub rule: Rule,
    /// The value as written (its first entries, names and strings cut
    /// short), and what's wrong with it; its display shows the first 80
    /// characters (`SHOWN`).
    pub value: String,
    /// Where its bytes were written, when the reader knows: a key and its
    /// value, an array's element, the dictionary that lacks a key, the
    /// object nothing reaches.
    pub at: Option<Span>,
}

/// A key or element of held objects that the model doesn't list: no row
/// has it, and its table has no wildcard.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unlisted {
    pub table: String,
    pub key: String,
    pub class: Class,
}

/// What kind of name a key the model doesn't list is (ISO 32000-2 Annex
/// E), as TestGrammar tells (`is_second_class_pdf_name` and
/// `is_third_class_pdf_name`, `TestGrammar/src/Utils.cpp`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// A second-class name: a registered prefix of four or five letters,
    /// digits, `_` or `-`, then `_` or `:` (`ABCD_x`).
    Second,
    /// A third-class name, free for anyone's use: `XX` first.
    Third,
    /// A key the model has under this extension, which the file isn't
    /// checked under ([`Report::extensions`]).
    Extension(String),
    /// Any other: a first-class name the model doesn't define, an array's
    /// element past its table's rows, or a writer's own key.
    Other,
}

/// A check a predicate that can't be told stood in the way of.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unchecked {
    /// The column whose cell it is.
    pub column: &'static str,
    /// What's in the way ([`Truth::Unknown`]'s): a function
    /// (`fn:IsMeaningful`), a value that can't be read (`@Type`), or a
    /// string's bytes the reader didn't give.
    pub why: String,
    pub table: String,
    pub key: String,
}

/// What a check of a file found in its held objects.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Held objects checked, by table; tree nodes as `name-tree` and
    /// `number-tree`.
    pub checked: BTreeMap<String, usize>,
    /// Keys and elements of held objects checked against their rows.
    pub keys: usize,
    /// Checks not made, for a predicate that can't be told.
    pub unchecked: BTreeMap<Unchecked, usize>,
    /// Keys and elements of held objects the model doesn't list.
    pub unlisted: BTreeMap<Unlisted, usize>,
    /// Keys and elements of held objects that the version checked at
    /// deprecates, or deprecates the type or value of, by table and key:
    /// findings too with [`Options::deprecations`].
    pub deprecated: BTreeMap<(String, String), usize>,
    /// Keys and elements of held objects that the model says readers
    /// ignore there, by table and key: a SpecialCase's `fn:Ignore` whose
    /// condition holds, or `fn:IsMeaningful` whose condition doesn't.
    pub ignored: BTreeMap<(String, String), usize>,
    /// Held objects reached through a type with no table to check them as.
    pub no_table: usize,
    /// Held objects two tables fit best, the first taken.
    pub ties: usize,
    /// Direct objects in held ones nested past [`NESTING`], not walked.
    pub truncated: usize,
    /// Held objects reached again as another table, kept as the first;
    /// a key the first doesn't list is unlisted only if none of them does
    /// (unless [`Options::one_table`]).
    pub two_contexts: usize,
    /// The extensions the file was checked under, as the model names
    /// them: those it declares, and [`Options::extensions`]. What's under
    /// another is as if it weren't in the model, as in TestGrammar
    /// without `--extensions`.
    pub extensions: BTreeSet<String>,
    /// Held objects checked as two tables at once, and counted in
    /// [`Report::checked`] as each: a field and its widget merged in one
    /// dictionary, and what both tables link a key of theirs to, as its
    /// `/AA` (unless [`Options::one_table`]).
    pub two_tables: usize,
    /// The cross-reference stream the trailer is, when it's one: a finding
    /// in the trailer is in it, or in no object.
    pub trailer: Option<ObjectId>,
    /// With [`Options::paths`], the path of every held value the walk
    /// reached, as a finding's is written.
    pub paths: BTreeSet<String>,
}

/// The objects of `source` that `hold` names, held to the model: each
/// reached from the trailer as the file's newest cross-reference section
/// writes it, at the file's version unless `options` says another. A
/// trailer the reader couldn't read so is a finding, and the walk starts
/// from what it made of the file instead.
pub fn check(source: &dyn Source, hold: &Hold, options: &Options) -> Report {
    let version = options.version.unwrap_or_else(|| file_version(source));
    let written = source.trailer();
    let mut findings = Vec::new();
    if let Some(why) = &written.unwritten {
        findings.push(Finding {
            object: None,
            made: Made::Merged,
            path: "trailer".to_string(),
            table: "FileTrailer".to_string(),
            key: "-".to_string(),
            rule: Rule::Unresolved,
            value: why.clone(),
            at: None,
        });
    }
    let object = &written.object;
    let (trailer, name) = match object.value() {
        Value::Stream(s) => (&s.dict, "XRefStream"),
        Value::Dict(d) => (d, "FileTrailer"),
        _ => (&Dict::EMPTY, "FileTrailer"),
    };
    let table = grammar()
        .table(name)
        .expect("the model has the trailers' tables");
    let id = written.stream;
    let mut extensions = crate::extension::declared(source);
    extensions.extend(options.extensions.iter().cloned());
    let checked_under = extensions.clone();
    // What every predicate of the file is evaluated in, once.
    let resolve = |id: ObjectId| source.get(id).ok();
    let file = FileCx::new(trailer, version, &resolve)
        .size(source.size())
        .extensions(&extensions);
    let mut walk = Walk {
        source,
        trailer,
        trailer_id: id,
        version,
        hold,
        exempt: &options.exempt,
        paths: options.paths,
        queue: VecDeque::new(),
        later: VecDeque::new(),
        waiting: HashSet::new(),
        reached: HashMap::new(),
        referrers: HashMap::new(),
        under_unlisted: HashSet::new(),
        one_table: options.one_table,
        unlisted_findings: options.unlisted,
        extensions: &extensions,
        deprecations: options.deprecations,
        field_keys: match options.one_table {
            true => Vec::new(),
            false => field_keys(),
        },
        also_of: HashMap::new(),
        again: HashMap::new(),
        unlisted_in: Vec::new(),
        file,
        report: Report {
            findings,
            trailer: id,
            extensions: checked_under,
            ..Report::default()
        },
    };
    if let Some(id) = id {
        walk.reached.insert(id, Some(table.name.as_str()));
    }
    let trail = Rc::new(Trail {
        up: None,
        step: Step::Trailer,
        object,
    });
    walk.queue.push_back(Visit {
        object,
        holder: id,
        table,
        trail,
        depth: 0,
        loose: false,
        also: None,
    });
    loop {
        while let Some(visit) = walk.queue.pop_front() {
            walk.visit(visit);
        }
        let Some(visit) = walk.later.pop_front() else {
            break;
        };
        if visit.holder.is_some_and(|id| walk.waiting.remove(&id)) {
            walk.visit(visit);
        }
    }
    walk.excuse();
    walk.unreached();
    walk.read_as(source.warnings());
    walk.report
}

impl Report {
    /// The held objects checked: those [`Report::checked`] counts, each
    /// once though it counts one checked as two tables as each.
    pub fn objects(&self) -> usize {
        self.checked.values().sum::<usize>() - self.two_tables
    }

    /// The findings a writer answers for, given `sources`, the reports of
    /// the files it emitted objects over: every finding in a
    /// [`Made::Built`] object, and those in [`Made::Merged`] objects and
    /// the trailer that no source has one like, of the same table, key,
    /// rule and value, the value's references' numbers aside, which a
    /// rewrite renumbers (and a reader's finding's numbers aside, its
    /// offsets); for a key that came in later than the file's version, any
    /// value, as its value isn't what's wrong. A merged object keeps the
    /// keys of the source dictionary it's emitted over, faults and all, and
    /// a fault the source has is the file's, carried; a built object is the
    /// writer's whole, and never excused. The sources are best checked at
    /// this file's version ([`Options::version`]). A like finding in any
    /// object of a source carries it, not only in the one the merged object
    /// was emitted over, which no [`Hold`] records: a fault written into a
    /// merged object is missed when some object of the same table in a
    /// source has it.
    ///
    /// The trailer is written anew by every save, in the file's form: its
    /// `/Size`, `/Prev`, `/Index` and `/W` are the save's own. A key too new
    /// for the file's version in it is carried when a source's trailer has
    /// one too: what's too new is the form, as a cross-reference stream
    /// below PDF 1.5 is, which an incremental update goes on writing.
    pub fn beyond<'a>(&'a self, sources: &[&Report]) -> Vec<&'a Finding> {
        let carried: HashSet<_> = (sources.iter())
            .flat_map(|source| &source.findings)
            .map(like)
            .collect();
        let too_new = |report: &Report, f: &Finding| f.rule == Rule::TooNew && report.in_trailer(f);
        let form = (sources.iter()).any(|s| s.findings.iter().any(|f| too_new(s, f)));
        (self.findings.iter())
            .filter(|f| {
                f.made == Made::Built || !(carried.contains(&like(f)) || (form && too_new(self, f)))
            })
            .collect()
    }

    /// The findings an incremental update answers for, given `before`, the
    /// report of the revision it updates, best checked at this file's
    /// version ([`Options::version`]): every finding in a [`Made::Built`]
    /// object, and in a [`Made::Merged`] object or the trailer those that
    /// the object of the same number, or the trailer, had no like finding
    /// in before, alike as [`Report::beyond`] has them. An update keeps its
    /// objects' numbers, so a fault is carried only from the object it was
    /// in.
    pub fn since<'a>(&'a self, before: &Report) -> Vec<&'a Finding> {
        let place = |report: &Report, f: &Finding| match report.in_trailer(f) {
            true => None,
            false => f.object,
        };
        let carried: HashSet<_> = (before.findings.iter())
            .map(|f| (place(before, f), like(f)))
            .collect();
        (self.findings.iter())
            .filter(|f| f.made == Made::Built || !carried.contains(&(place(self, f), like(f))))
            .collect()
    }

    /// The findings a rewrite answers for, given `baseline`, the report of
    /// the file it rewrote, checked with [`Options::paths`] and best at this
    /// file's version: every finding in a [`Made::Built`] object, and in a
    /// [`Made::Merged`] object or the trailer those the baseline had no like
    /// finding at the same path for, alike as [`Report::beyond`] has them;
    /// or, where the baseline reached nothing at that path, as where a page
    /// moved, no like finding anywhere. A rewrite renumbers objects, but
    /// most stay where they were.
    pub fn by_path<'a>(&'a self, baseline: &Report) -> Vec<&'a Finding> {
        let at_path: HashSet<_> = (baseline.findings.iter())
            .map(|f| (f.path.as_str(), like(f)))
            .collect();
        let anywhere: HashSet<_> = baseline.findings.iter().map(like).collect();
        (self.findings.iter())
            .filter(|f| {
                let carried = match baseline.paths.contains(&f.path) {
                    true => at_path.contains(&(f.path.as_str(), like(f))),
                    false => anywhere.contains(&like(f)),
                };
                f.made == Made::Built || !carried
            })
            .collect()
    }

    /// Whether `finding` is in the trailer as written: in its
    /// cross-reference stream, or in no object.
    fn in_trailer(&self, finding: &Finding) -> bool {
        finding.object.is_none() || finding.object == self.trailer
    }
}

/// What a finding is, for [`Report::beyond`], [`Report::since`] and
/// [`Report::by_path`]: its table, key, rule and value, the value's
/// references' numbers aside, which a rewrite renumbers, and a reader's
/// finding's numbers aside, its offsets; for a key that came in later than
/// the file's version, any value, as its value isn't what's wrong; and for
/// an object nothing reaches, the object alone, as which object's
/// reference to it is named follows the objects' numbers and keys' order.
fn like(f: &Finding) -> (String, String, Rule, String) {
    let value = match f.rule {
        Rule::TooNew => String::new(),
        Rule::Reader => f.value.chars().filter(|c| !c.is_ascii_digit()).collect(),
        _ => numberless(&f.value),
    };
    match f.rule {
        Rule::Unreached => ("-".to_string(), "-".to_string(), f.rule, value),
        _ => (f.table.clone(), f.key.clone(), f.rule, value),
    }
}

/// The file's version: its header's (1.4 when it has none that reads), or
/// its catalog's `/Version` when that's later.
pub fn file_version(source: &dyn Source) -> Version {
    let header = source.header_version().unwrap_or(Version::new(1, 4));
    let catalog = (source.trailer().dict().get(b"Root"))
        .and_then(|root| root.as_reference())
        .and_then(|id| source.get(id).ok())
        .and_then(|catalog| {
            catalog
                .as_dict()?
                .get(b"Version")?
                .as_name()
                .map(<[u8]>::to_vec)
        })
        .and_then(|name| Version::parse(&name));
    header.max(catalog.unwrap_or(header))
}

/// A walk of one file.
struct Walk<'a> {
    source: &'a dyn Source,
    /// The trailer as written (`trailer::`).
    trailer: &'a Dict,
    /// The cross-reference stream the trailer is, when it is one.
    trailer_id: Option<ObjectId>,
    version: Version,
    hold: &'a Hold,
    exempt: &'a [Exempt],
    /// Whether to record each held value's path.
    paths: bool,
    queue: VecDeque<Visit<'a>>,
    /// Indirect objects reached as a `_Universal` table, which fits
    /// anything, to visit as that once the queue is empty, unless one is
    /// reached as another table first; and which of them haven't been.
    later: VecDeque<Visit<'a>>,
    waiting: HashSet<ObjectId>,
    /// Each indirect object reached, with the table it was given; none for
    /// one with no table, a value of a simple type.
    reached: HashMap<ObjectId, Option<&'static str>>,
    /// The table and key each object was first seen referenced under in an
    /// object walked.
    referrers: HashMap<ObjectId, (&'static str, String)>,
    /// The objects referenced under keys the model doesn't list.
    under_unlisted: HashSet<ObjectId>,
    /// [`Options::one_table`].
    one_table: bool,
    /// [`Options::unlisted`].
    unlisted_findings: UnlistedIn,
    /// [`Options::deprecations`].
    deprecations: bool,
    /// [`Report::extensions`].
    extensions: &'a BTreeSet<String>,
    /// The keys only field tables list, not the widget's: a widget with
    /// one of them, or `/T`, is a field too.
    field_keys: Vec<&'static [u8]>,
    /// The other table each indirect object checked as two is checked as.
    also_of: HashMap<ObjectId, &'static str>,
    /// The tables each indirect object was reached again as, other than
    /// those it's checked as.
    again: HashMap<ObjectId, Vec<&'static Table>>,
    /// Each held indirect object's own keys found unlisted: the object,
    /// the key, its count, and its finding, if one was made.
    unlisted_in: Vec<(ObjectId, Vec<u8>, Unlisted, Option<usize>)>,
    /// What the predicates are evaluated in: the file's parts, and the name
    /// trees' keys, the structure tree's content items and the form's
    /// fields' names, as they read them.
    file: FileCx<'a>,
    report: Report,
}

/// A dictionary, stream or array to check, as a table.
struct Visit<'a> {
    object: &'a Object,
    /// The indirect object it is, or is in: none for a table's trailer.
    holder: Option<ObjectId>,
    table: &'static Table,
    trail: Rc<Trail<'a>>,
    /// How deeply it's nested in its holder, direct.
    depth: usize,
    /// Whether its table says only what kind of object it is: a
    /// `_Universal` one, or the plain `Stream` one a `_Universal` one links
    /// a stream to. No key is one such a table doesn't list.
    loose: bool,
    /// The other table a dictionary or stream is checked as too: a
    /// field's or its widget's, the two merged ([`Walk::merged`]), or what
    /// the other of its holder's two tables links its key to. Each key is
    /// checked against the row of each table that lists it, and is
    /// unlisted only if neither does.
    also: Option<&'static Table>,
}

impl<'a> Visit<'a> {
    /// The number of the indirect object it is, not one it's nested in:
    /// its holder's when it's that object; none for a direct object, and
    /// for a trailer that isn't a cross-reference stream.
    fn id(&self) -> Option<ObjectId> {
        self.holder.filter(|_| self.depth == 0)
    }

    /// The same visit as `table`, with this one's table the other.
    fn as_(&self, table: &'static Table) -> Visit<'a> {
        Visit {
            table,
            also: Some(self.table),
            trail: self.trail.clone(),
            ..*self
        }
    }
}

/// The walk's way to an object: each step's object, and how it was
/// reached from the one before.
struct Trail<'a> {
    up: Option<Rc<Trail<'a>>>,
    step: Step<'a>,
    object: &'a Object,
}

impl Drop for Trail<'_> {
    /// Unlinks the way back a step at a time, as far as no other trail
    /// shares it: dropped whole, a way as long as the file makes it (an
    /// outline's `/Next` siblings, a thread's beads) would drop
    /// recursively, a step deeper each, and overflow the stack.
    fn drop(&mut self) {
        let mut up = self.up.take();
        while let Some(trail) = up {
            up = match Rc::try_unwrap(trail) {
                Ok(mut trail) => trail.up.take(),
                Err(_) => None,
            };
        }
    }
}

/// How an object was reached from the one before it.
#[derive(Clone)]
enum Step<'a> {
    Trailer,
    Key(&'a [u8]),
    Index(usize),
    /// A name or number tree's entry.
    Entry(String),
}

/// A value's type among its row's.
enum Typed {
    /// The type its row's `i`th is; and when the row allows it at the
    /// file's version only as deprecated, from which version.
    Is(usize, Option<Version>),
    /// Of a type its row allows only at other versions, or under an
    /// extension.
    Excluded(ArlType),
    Not,
}

/// What a value's row's PossibleValues for its type make of it.
enum Values {
    /// One of them; or any, none being listed.
    Allowed,
    /// One of them only as deprecated (`fn:Deprecated`), from this
    /// version.
    Deprecated(Version),
    /// None of them: those the file's version lets in, as the model
    /// writes them.
    Not(Vec<&'static str>),
    /// Not the value an `fn:RequiredValue` whose condition holds requires:
    /// that value, and the item, as the model writes it.
    Required(String, &'static str),
    /// It can't be told, for one of them that's a predicate that can't be
    /// ([`Truth::Unknown`]'s).
    Unknown(String),
}

/// Where an entry's key and value were written, the two together.
fn entry_at(entry: &Entry) -> Option<Span> {
    match (entry.key_at, entry.value.at) {
        (Some(key), Some(value)) => Some(Span::new(key.stream, key.start, value.end)),
        (key, value) => value.or(key),
    }
}

impl<'a> Walk<'a> {
    /// How the object `holder` was made, when it's held: the trailer as
    /// [`Made::Merged`].
    fn made(&self, holder: Option<ObjectId>) -> Option<Made> {
        match holder {
            None => Some(Made::Merged),
            Some(id) if Some(id) == self.trailer_id => Some(Made::Merged),
            Some(id) => match self.hold {
                Hold::All(made) => Some(*made),
                Hold::Only(objects) => objects.get(&id).copied(),
            },
        }
    }

    /// Checks `v`'s keys or elements, and when it's held, that it has those
    /// its table requires and no key twice.
    fn visit(&mut self, v: Visit<'a>) {
        let made = self.made(v.holder);
        let also = v.also.map(|table| v.as_(table));
        if made.is_some() {
            *self.report.checked.entry(v.table.name.clone()).or_default() += 1;
            if let Some(also) = &also {
                *self
                    .report
                    .checked
                    .entry(also.table.name.clone())
                    .or_default() += 1;
                self.report.two_tables += 1;
            }
            if self.paths {
                self.report.paths.insert(v.trail.path());
            }
        }
        let parents = v.trail.parents();
        if let Some(dict) = v.object.as_dict() {
            for entry in dict.last_entries() {
                let at = At::Key(&entry.key);
                let row = v.table.row(at);
                let also = also.as_ref().map(|also| (also, also.table.row(at)));
                let (raw, written) = (&entry.value, entry_at(entry));
                self.entry(&v, at, row, raw, written, made, &parents, also);
            }
            if let Some(made) = made {
                for twice in dict.duplicates() {
                    let key = String::from_utf8_lossy(&twice.key).into_owned();
                    let value = format!("{} (its key written again later)", render(&twice.value));
                    let table = &v.table.name;
                    self.find(
                        &v,
                        made,
                        Rule::Duplicate,
                        table,
                        &key,
                        value,
                        entry_at(twice),
                    );
                }
                let first = self.report.findings.len();
                self.required_keys(&v, dict, made, &parents);
                if let Some(also) = &also {
                    self.required_keys(also, dict, made, &parents);
                    self.once(first);
                }
            }
        } else if let Value::Array(array) = v.object.value() {
            let rows = self.array_rows(v.table, array);
            for (i, (value, row)) in array.iter().zip(rows).enumerate() {
                let at = At::Index(i);
                self.entry(&v, at, row, value, value.at, made, &parents, None);
            }
            if let Some(made) = made {
                self.required_elements(&v, array.len(), made, &parents);
            }
        }
    }

    /// The rows `array`'s elements fall on in `table`, as TestGrammar's
    /// walk assigns them (`CParsePDF::parse_object`, ParseObjects.cpp): in
    /// order, round and round from the first row with a `*` (a repeating
    /// set's, or the wildcard); and where some rows aren't required, an
    /// element at or past the first of those that isn't of its type starts
    /// again from the first row with a `*`. None for an element past every
    /// row, which TestGrammar notes and checks no further.
    fn array_rows(&self, table: &'static Table, array: &[Object]) -> Vec<Option<&'static Row>> {
        let rows = &table.rows;
        let wildcard = rows
            .last()
            .filter(|row| row.key == "*")
            .map(|_| rows.len() - 1);
        let repeat = rows.iter().position(|row| row.key.contains('*'));
        let optional = rows.iter().position(|row| row.required != "TRUE");
        let mut next = 0;
        let mut assigned = Vec::with_capacity(array.len());
        for (i, element) in array.iter().enumerate() {
            let at = match (wildcard, repeat, optional) {
                (Some(wildcard), _, _) if i >= wildcard => wildcard,
                (_, Some(repeat), None) if next >= repeat && next < rows.len() => next,
                (_, Some(repeat), None) if next >= repeat => repeat,
                (_, Some(repeat), Some(optional)) if next >= optional => {
                    let fits = match &rows[optional].r#type[..] {
                        [ty] if ty.wraps.is_empty() => (self.resolve(element))
                            .is_ok_and(|(_, value)| self.matches(value, ty.item)),
                        _ => false,
                    };
                    if fits { next } else { repeat }
                }
                _ => next,
            };
            next = at + 1;
            assigned.push(rows.get(at));
        }
        assigned
    }

    /// The entry `raw` at `at` in `v`'s object, on `row` of its table, and
    /// on `also`'s row of the other table it's checked as, if any, written
    /// at `written`: checked against each row that lists it (a finding both
    /// make made once, as the first's), and followed to the tables they
    /// link to.
    #[allow(clippy::too_many_arguments)]
    fn entry(
        &mut self,
        v: &Visit<'a>,
        at: At<'a>,
        row: Option<&'static Row>,
        raw: &'a Object,
        written: Option<Span>,
        made: Option<Made>,
        parents: &[&'a Object],
        also: Option<(&Visit<'a>, Option<&'static Row>)>,
    ) {
        let key = at_text(at);
        let step = match at {
            At::Key(key) => Step::Key(key),
            At::Index(i) => Step::Index(i),
        };
        let (id, value) = match self.resolve(raw) {
            Ok(found) => found,
            Err(gone) => {
                if let Some(made) = made {
                    let value = format!("{} {} R", gone.0, gone.1);
                    let table = &v.table.name;
                    self.find(v, made, Rule::Unresolved, table, &key, value, written);
                }
                return;
            }
        };
        let (row, extension) = walked(v.table, row, self.extensions);
        let also = (also)
            .and_then(|(also, row)| Some((also, walked(also.table, row, self.extensions).0?)));
        let (v, row, also) = match (row, also) {
            (Some(row), also) => (v, row, also),
            (None, Some((other, row))) => (other, row, None),
            (None, None) => {
                // Metadata streams may be anywhere from PDF 1.4 (ISO 32000-2
                // 14.3.2), and associated files from 2.0 (14.13), which
                // TestGrammar checks as such where the model has no row.
                let table = match (at, value.value()) {
                    (At::Key(b"Metadata"), Value::Stream(_)) => "Metadata",
                    (At::Key(b"AF"), Value::Array(_)) => "ArrayOfFileSpecifications",
                    _ => {
                        if let Some(made) = made {
                            self.unlisted(v, at, key, extension, value, written, made);
                        }
                        each_reference(raw, |to| {
                            self.under_unlisted.insert(to);
                        });
                        return;
                    }
                };
                if made.is_some() {
                    self.report.keys += 1;
                }
                self.refer(raw, v.table, key);
                let table = grammar().table(table).expect("the model has the table");
                self.reach(v, step, (id, value), &[table], parents, &[]);
                return;
            }
        };
        self.refer(raw, v.table, key.clone());
        // A dictionary's null entry is no entry (ISO 32000-2, 7.3.7), unless
        // its row allows null.
        let null = |row: &Row| row.r#type.iter().any(|t| t.item == ArlType::Null);
        if matches!((at, value.value()), (At::Key(_), Value::Null))
            && !null(row)
            && also.is_none_or(|(_, row)| !null(row))
        {
            return;
        }
        if made.is_some() {
            self.report.keys += 1;
        }
        let first = self.report.findings.len();
        let found = (raw, value);
        let typed = self.against(v, at, row, found, &key, written, made, parents);
        let also = also.map(|(also, row)| {
            let typed = self.against(also, at, row, found, &key, written, made, parents);
            (also, row, typed)
        });
        self.once(first);
        let found = (id, value);
        match (typed, also) {
            (Some(i), Some((_, other, Some(j)))) => {
                let links = match other.r#type[j].item {
                    ArlType::Array | ArlType::Dictionary | ArlType::Stream => {
                        self.links(&other.link[j])
                    }
                    _ => Vec::new(),
                };
                self.follow(v, step, found, row, i, parents, &links);
            }
            (Some(i), _) => self.follow(v, step, found, row, i, parents, &[]),
            (None, Some((also, other, Some(j)))) => {
                self.follow(also, step, found, other, j, parents, &[]);
            }
            (None, _) => {}
        }
    }

    /// The key or element at `at` of `v`'s object, `value`, that no row
    /// lists, counted by its kind of name, `extension`'s when an
    /// extension's row has it; in a built object (or as
    /// [`Options::unlisted`] says), unless `v`'s table says only what kind
    /// of object it is, or the key is exempt, a finding, but a name that's
    /// second or third class.
    #[allow(clippy::too_many_arguments)]
    fn unlisted(
        &mut self,
        v: &Visit<'a>,
        at: At<'a>,
        key: String,
        extension: Option<String>,
        value: &Object,
        written: Option<Span>,
        made: Made,
    ) {
        let class = match (extension, at) {
            (Some(name), _) => Class::Extension(name),
            (None, At::Key(key)) => Class::of(key),
            (None, At::Index(_)) => Class::Other,
        };
        let exempt = matches!(at, At::Key(key) if self.exempt(key));
        let mut finding = None;
        let held = match self.unlisted_findings {
            UnlistedIn::Built => made == Made::Built,
            UnlistedIn::All => true,
            UnlistedIn::Never => false,
        };
        if held && matches!(class, Class::Other | Class::Extension(_)) && !exempt && !v.loose {
            finding = Some(self.report.findings.len());
            let value = render(value);
            let table = &v.table.name;
            self.find(v, made, Rule::Unlisted, table, &key, value, written);
        }
        let unlisted = Unlisted {
            table: v.table.name.clone(),
            key,
            class,
        };
        // An indirect object's own key: unlisted only if no other table
        // it's reached as lists it either, which the walk's end tells.
        if let (At::Key(name), Some(id), 0, false) = (at, v.holder, v.depth, self.one_table) {
            (self.unlisted_in).push((id, name.to_vec(), unlisted.clone(), finding));
        }
        *self.report.unlisted.entry(unlisted).or_default() += 1;
    }

    /// `value` (`raw`, its references followed) at `at` in `v`'s object,
    /// written at `written`, checked against `row` when the object is
    /// held: its versions, its type, and the rules on a value of it. Its
    /// type among the row's, when it's of one the file's version lets in,
    /// to follow.
    #[allow(clippy::too_many_arguments)]
    fn against(
        &mut self,
        v: &Visit<'a>,
        at: At<'a>,
        row: &'static Row,
        (raw, value): (&'a Object, &'a Object),
        key: &str,
        written: Option<Span>,
        made: Option<Made>,
        parents: &[&'a Object],
    ) -> Option<usize> {
        let Some(made) = made else {
            return match self.typed(row, value) {
                Typed::Is(i, _) => Some(i),
                _ => None,
            };
        };
        self.versions(v, made, row, key, value, written);
        match self.typed(row, value) {
            Typed::Is(i, deprecated) => {
                let ty = row.r#type[i].item;
                if unknown_bytes(value, ty) {
                    let why = "the string's bytes".to_string();
                    self.unchecked("Type", why, &v.table.name, key);
                }
                if let Some(from) = deprecated {
                    let value = format!("{} ({} deprecated in {from})", render(value), ty.name());
                    let table = &v.table.name;
                    self.deprecated(v, made, table, key, value, written);
                }
                self.rules(v, at, made, row, i, (raw, value), written, parents);
                Some(i)
            }
            Typed::Excluded(ty) => {
                let value = format!(
                    "{} ({} isn't allowed at {})",
                    render(value),
                    ty.name(),
                    self.version
                );
                self.find(v, made, Rule::Type, &v.table.name, key, value, written);
                None
            }
            Typed::Not => {
                let value = render(value);
                self.find(v, made, Rule::Type, &v.table.name, key, value, written);
                None
            }
        }
    }

    /// Leaves out each finding from `first` on that one before it, from
    /// `first` on, makes but for its table: a rule of both tables an
    /// object is checked as, broken once.
    fn once(&mut self, first: usize) {
        let findings = &mut self.report.findings;
        let mut i = first + 1;
        while i < findings.len() {
            if findings[first..i].iter().any(|f| same(f, &findings[i])) {
                findings.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Whether `row`'s key or element, with `value` in a held object `v`,
    /// came in by the file's version (its SinceVersion; an extension's row
    /// never gets here) and isn't deprecated by it (its DeprecatedIn), as
    /// TestGrammar's `ArlVersion` reads the two (ArlVersion.cpp).
    fn versions(
        &mut self,
        v: &Visit,
        made: Made,
        row: &Row,
        key: &str,
        value: &Object,
        written: Option<Span>,
    ) {
        match predicate::since(&row.since_version, self.extensions) {
            Since::In(since) if since > self.version => {
                let value = format!("{} (since {since})", render(value));
                self.find(v, made, Rule::TooNew, &v.table.name, key, value, written);
            }
            Since::In(_) | Since::Extension(_) => {}
            Since::Unknown(why) => self.unchecked("SinceVersion", why, &v.table.name, key),
        }
        if let Some(from) = version(&row.deprecated_in)
            && from <= self.version
        {
            let value = format!("{} (deprecated in {from})", render(value));
            self.deprecated(v, made, &v.table.name, key, value, written);
        }
    }

    /// The rules on `raw` (`value`, its references followed) at `at` in
    /// the held object `v`, of its row's `i`th type: what its
    /// IndirectReference, PossibleValues and SpecialCase ask (TestGrammar's
    /// `check_everything`, ParseObjects.cpp, which asks only for indirect
    /// references; here direct values too).
    #[allow(clippy::too_many_arguments)]
    fn rules(
        &mut self,
        v: &Visit<'a>,
        at: At<'a>,
        made: Made,
        row: &'static Row,
        i: usize,
        (raw, value): (&'a Object, &'a Object),
        written: Option<Span>,
        parents: &[&'a Object],
    ) {
        let key = at_text(at);
        let table = &v.table.name;
        // A tree's IndirectReference is its values' (INTERNAL_GRAMMAR.md,
        // "Column 6 - IndirectReference"), which the model asks of none
        // but under an extension (a test says so): unread.
        if !matches!(row.r#type[i].item, ArlType::NameTree | ArlType::NumberTree) {
            let asked = self.with_cx((v.id(), v.object), at, parents, None, |cx| {
                predicate::reference(&row.indirect_reference[i], cx)
            });
            let referenced = matches!(raw.value(), Value::Ref(_));
            match asked {
                Reference::Indirect if !referenced && !value.is_null() => {
                    self.find(v, made, Rule::Indirect, table, &key, render(value), written);
                }
                Reference::Direct if referenced => {
                    self.find(v, made, Rule::Direct, table, &key, render(raw), written);
                }
                Reference::Unknown(why) => self.unchecked("IndirectReference", why, table, &key),
                _ => {}
            }
        }
        match self.with_cx((v.id(), v.object), at, parents, Some(v.table), |cx| {
            self.values(row, i, value, cx)
        }) {
            Values::Allowed => {}
            Values::Deprecated(from) => {
                let value = format!("{} (deprecated in {from})", render(value));
                self.deprecated(v, made, table, &key, value, written);
            }
            Values::Not(listed) => {
                let value = format!("{} (not one of {})", render(value), listed.join(", "));
                self.find(v, made, Rule::Value, table, &key, value, written);
            }
            Values::Required(required, item) => {
                let value = format!("{} (not {required}, as {item} requires)", render(value));
                self.find(v, made, Rule::Value, table, &key, value, written);
            }
            Values::Unknown(why) => self.unchecked("PossibleValues", why, table, &key),
        }
        let special = &row.special_case[i];
        if !special.is_empty() {
            let (holds, ignored) =
                self.with_cx((v.id(), v.object), at, parents, Some(v.table), |cx| {
                    (
                        predicate::holds_fully(special, cx),
                        predicate::ignored(special, cx),
                    )
                });
            if ignored == Some(Truth::True) {
                *self
                    .report
                    .ignored
                    .entry((table.clone(), key.clone()))
                    .or_default() += 1;
            }
            match holds {
                Truth::True => {}
                Truth::False => {
                    let value = format!("{} ({special})", render(value));
                    self.find(v, made, Rule::Special, table, &key, value, written);
                }
                Truth::Unknown(why) => self.unchecked("SpecialCase", why, table, &key),
            }
        }
    }

    /// Notes that `raw`, when it's a reference, was seen under `key` of
    /// an object walked as `table`.
    fn refer(&mut self, raw: &Object, table: &'static Table, key: String) {
        if let Value::Ref(to) = *raw.value() {
            (self.referrers.entry(to)).or_insert_with(|| (table.name.as_str(), key));
        }
    }

    /// Goes on from `found`, the entry at `step` of `v`'s object (and the
    /// number of the object it's in), of its row's `i`th type: to the
    /// table of its links it fits best (and of `also`, the other table's
    /// row's links, if another), or through its tree.
    #[allow(clippy::too_many_arguments)]
    fn follow(
        &mut self,
        v: &Visit<'a>,
        step: Step<'a>,
        found: (Option<ObjectId>, &'a Object),
        row: &'static Row,
        i: usize,
        parents: &[&'a Object],
        also: &[&'static Table],
    ) {
        match row.r#type[i].item {
            ArlType::NameTree | ArlType::NumberTree => self.tree(v, step, found, row, i, parents),
            ArlType::Array | ArlType::Dictionary | ArlType::Stream => {
                let links = self.links(&row.link[i]);
                self.reach(v, step, found, &links, parents, also)
            }
            _ => {
                if let (Some(id), _) = found {
                    self.reached.entry(id).or_insert(None);
                }
            }
        }
    }

    /// The tables of a type's links that their version predicates let in
    /// at the file's version; all of them when those let in none, as
    /// TestGrammar's walk always takes all (its `get_full_linkset`).
    fn links(&self, links: &'static [Wrapped<String>]) -> Vec<&'static Table> {
        let table = |l: &Wrapped<String>| {
            grammar()
                .table(&l.item)
                .expect("the loader checks that every link names a table")
        };
        let allowed: Vec<&Table> = (links.iter())
            .filter(|l| predicate::gate(&l.wraps, self.version, self.extensions) != Gate::Excluded)
            .map(table)
            .collect();
        if allowed.is_empty() {
            links.iter().map(table).collect()
        } else {
            allowed
        }
    }

    /// Goes on to `found`, a dictionary, stream or array at `step` from
    /// `from`'s object (with its number, when it's an indirect object), as
    /// the table of `links` it fits best: queued, unless it was reached
    /// already or is nested past [`NESTING`]. An indirect object reached as
    /// a `_Universal` table waits for the queue to empty, and reached as
    /// another table meanwhile, is queued as that. Unless
    /// [`Options::one_table`], it's checked as two tables at once when the
    /// table of `also` it fits best is another, or when it's a field and
    /// its widget merged ([`Walk::merged`]).
    fn reach(
        &mut self,
        from: &Visit<'a>,
        step: Step<'a>,
        (id, value): (Option<ObjectId>, &'a Object),
        links: &[&'static Table],
        parents: &[&'a Object],
        also: &[&'static Table],
    ) {
        let held = self.made(id.or(from.holder)).is_some();
        let first = id.and_then(|id| self.reached.get(&id).copied().flatten());
        let waiting = id.is_some_and(|id| self.waiting.contains(&id));
        if first.is_some() && !held && !waiting {
            return;
        }
        let mut up = Vec::with_capacity(PARENTS);
        up.push(from.object);
        up.extend(parents.iter().take(PARENTS - 1));
        let Some((table, tie)) = self.choose((id, value), links, &up) else {
            if held && id.is_none_or(|id| !self.reached.contains_key(&id)) {
                self.report.no_table += 1;
            }
            if let Some(id) = id {
                self.reached.entry(id).or_insert(None);
            }
            return;
        };
        if let (Some(first), Some(id)) = (first, id) {
            // Reached as a `_Universal` table and not visited yet, it's this
            // one's instead. TestGrammar visits it as the first table, and
            // doesn't count `_Universal` ones, which fit anything.
            let instead = waiting && !universal(&table.name);
            if !instead {
                if first != table.name
                    && self.also_of.get(&id) != Some(&table.name.as_str())
                    && !universal(first)
                    && !universal(&table.name)
                {
                    self.report.two_contexts += 1;
                    if !self.one_table {
                        let again = self.again.entry(id).or_default();
                        if !again.iter().any(|t| t.name == table.name) {
                            again.push(table);
                        }
                    }
                }
                return;
            }
            self.waiting.remove(&id);
        }
        // Only a dictionary or stream is checked as two tables at once.
        let dict = matches!(value.value(), Value::Dict(_) | Value::Stream(_));
        let also = match dict && !self.one_table && !universal(&table.name) {
            false => None,
            true => (self.choose((id, value), also, &up))
                .map(|(also, _)| also)
                .filter(|also| also.name != table.name && !universal(&also.name))
                .or_else(|| self.merged((id, value), table, &up)),
        };
        if tie && held {
            self.report.ties += 1;
        }
        let trail = Rc::new(Trail {
            up: Some(from.trail.clone()),
            step,
            object: value,
        });
        let loose =
            universal(&table.name) || (universal(&from.table.name) && table.name == "Stream");
        let visit = match id {
            Some(id) => {
                self.reached.insert(id, Some(table.name.as_str()));
                if let Some(also) = also {
                    self.also_of.insert(id, also.name.as_str());
                }
                Visit {
                    object: value,
                    holder: Some(id),
                    table,
                    trail,
                    depth: 0,
                    loose,
                    also,
                }
            }
            None if from.depth < NESTING => Visit {
                object: value,
                holder: from.holder,
                table,
                trail,
                depth: from.depth + 1,
                loose,
                also,
            },
            None => {
                if held {
                    self.report.truncated += 1;
                }
                return;
            }
        };
        match id {
            Some(id) if universal(&table.name) => {
                self.waiting.insert(id);
                self.later.push_back(visit);
            }
            _ => self.queue.push_back(visit),
        }
    }

    /// The other table `object`, reached as `table`, is checked as too
    /// when it's a field and its widget merged in one dictionary (ISO
    /// 32000-2 12.5.6.19; and a dictionary without `/T` under a field is
    /// another of the field's, which differs from it only in how it looks,
    /// 12.7.4.2). A widget is a field too when it has `/T`, its partial
    /// name, or a key only field tables list ([`Walk::field_keys`]): as the
    /// field table [`Walk::field_table`] says. A field whose `/Subtype` is
    /// `/Widget` is a widget too. The model's widget table lists the keys
    /// all fields have, but not a field type's (`/FT`, `/V`, `/DA`...); a
    /// field's tables, none of a widget's. `id` is the indirect object
    /// `object` is, when it's one.
    fn merged(
        &self,
        (id, object): (Option<ObjectId>, &'a Object),
        table: &'static Table,
        parents: &[&'a Object],
    ) -> Option<&'static Table> {
        let Value::Dict(dict) = object.value() else {
            return None;
        };
        if table.name == "AnnotWidget" {
            let field = self.present(dict.get(b"T"))
                || (self.field_keys.iter()).any(|key| self.present(dict.get(key)));
            return field.then(|| self.field_table((id, object), dict, parents));
        }
        let subtype = (dict.get(b"Subtype").and_then(|o| self.resolve(o).ok()))
            .and_then(|(_, o)| o.as_name());
        (FIELDS.contains(&table.name.as_str()) && subtype == Some(b"Widget")).then(|| {
            grammar()
                .table("AnnotWidget")
                .expect("the model has the widget's table")
        })
    }

    /// The field table `object`, `dict`, is checked as: the one its field
    /// type, `/FT`, says, and a button's flags, `/Ff` (ISO 32000-2 Tables
    /// 226 and 227: bit 17 a push button, else bit 16 a radio button, else
    /// a checkbox), each its own or inherited; else the field table it
    /// fits best. `id` is the indirect object it is, when it's one.
    fn field_table(
        &self,
        (id, object): (Option<ObjectId>, &'a Object),
        dict: &'a Dict,
        parents: &[&'a Object],
    ) -> &'static Table {
        let ff = (self.inheritable(dict, b"Ff").and_then(Object::as_integer))
            .and_then(|ff| {
                (u32::try_from(ff).ok()).or_else(|| i32::try_from(ff).ok().map(i32::cast_unsigned))
            })
            .unwrap_or(0);
        let name = match self.inheritable(dict, b"FT").and_then(Object::as_name) {
            Some(b"Tx") => Some("FieldTx"),
            Some(b"Ch") => Some("FieldChoice"),
            Some(b"Sig") => Some("FieldSig"),
            Some(b"Btn") if ff & 1 << 16 != 0 => Some("FieldBtnPush"),
            Some(b"Btn") if ff & 1 << 15 != 0 => Some("FieldBtnRadio"),
            Some(b"Btn") => Some("FieldBtnCheckbox"),
            _ => None,
        };
        let table = |name| {
            grammar()
                .table(name)
                .expect("the model has the field tables")
        };
        match name {
            Some(name) => table(name),
            None => {
                let fields: Vec<&'static Table> = FIELDS.into_iter().map(table).collect();
                (self.choose((id, object), &fields, parents)).map_or(table("Field"), |(t, _)| t)
            }
        }
    }

    /// The table of `links` that `object` fits best, and whether another
    /// fit as well, the first of them taken; none when there are none.
    /// TestGrammar's `recommended_link_for_object` (ParseObjects.cpp),
    /// ported: see [`Walk::score`]. `id` is the indirect object `object`
    /// is, when it's one.
    fn choose(
        &self,
        (id, object): (Option<ObjectId>, &'a Object),
        links: &[&'static Table],
        parents: &[&'a Object],
    ) -> Option<(&'static Table, bool)> {
        // Where TestGrammar scores an array against a dictionary's table
        // too, and one as long as the table has rows took it (a structure
        // parent tree's array of 19, a structure element's 19 rows), only
        // the tables of the object's kind are scored, when there are any.
        let fitting: Vec<&'static Table> = (links.iter().copied())
            .filter(|table| fits(object, table))
            .collect();
        let links = if fitting.is_empty() { links } else { &fitting };
        if let [only] = links {
            return Some((only, false));
        }
        let mut best: Option<(&'static Table, i32)> = None;
        let mut tie = false;
        for &table in links {
            let score = self.score((id, object), table, parents);
            match best {
                Some((_, least)) if score > least => {}
                Some((_, least)) if score == least => tie = true,
                _ => {
                    best = Some((table, score));
                    tie = false;
                }
            }
        }
        best.map(|(table, _)| (table, tie))
    }

    /// How well `object` fits `table`, lower better (like golf): ported
    /// from `CParsePDF::recommended_link_for_object` in TestGrammar's
    /// `src/ParseObjects.cpp`, Copyright 2020-2022 PDF Association, Inc.,
    /// Apache-2.0 (pdf-arlington's `model/NOTICE.txt`), its weights kept.
    /// Each row's entry scores: a disambiguating key's (`Type`, `Subtype`,
    /// `S`, `Parent`, `TransformMethod`) value of an allowed type and
    /// value -80, an array's first element's -60 required or -20,
    /// another's -10 required or -4; a value it doesn't allow +10, +7 or
    /// +5, a type it doesn't +20, +3 or +1; a deprecated key's +8 more.
    /// All required entries good, -8 for each entry found; and -10 times
    /// the share of rows found. An array as long as the table has rows -20.
    ///
    /// Where its code departs from what its comments say, this follows
    /// the comments: the array bonus goes to tables without repeating or
    /// wildcard rows ("but not for wildcards or repeating sets", where its
    /// code tests that the last row has a `*`); and a key is deprecated
    /// when the file's version is at or past its DeprecatedIn (its
    /// `IsDeprecated` tests the reverse). A required entry that's absent
    /// scores nothing, as in its code, which works out whether an entry is
    /// required only for entries present and so never reaches the +12 its
    /// comment gives one: a key required and inherited (a field's `/DA`,
    /// from the form's) is absent from the right table's object. And a
    /// value it allows meets its row's SpecialCase too, where TestGrammar
    /// reads PossibleValues alone: button fields differ only in their
    /// `/Ff` bits, which the model checks in SpecialCase, and would tie,
    /// the push button taken. Both are read as the rules read them: what
    /// can't be told counts as allowed.
    fn score(
        &self,
        (id, object): (Option<ObjectId>, &'a Object),
        table: &'static Table,
        parents: &[&'a Object],
    ) -> i32 {
        let rows = &table.rows;
        let (dict, array) = match object.value() {
            Value::Dict(d) => (Some(d), None),
            Value::Stream(s) => (Some(&s.dict), None),
            Value::Array(a) => (None, Some(a)),
            _ => return 0,
        };
        let mut score = 0;
        if let Some(array) = array
            && array.len() == rows.len()
            && rows.last().is_some_and(|row| !row.key.contains('*'))
        {
            score -= 20;
        }
        let mut found = 0;
        let mut bad = false;
        for row in rows {
            let (at, entry) = match (dict, array) {
                (Some(dict), _) => {
                    let key = row.key.as_bytes();
                    (At::Key(key), dict.get(key))
                }
                (_, Some(array)) => match index(&row.key) {
                    Some(i) => (At::Index(i), array.get(i)),
                    None => continue,
                },
                (None, None) => return 0,
            };
            let entry = (entry.and_then(|e| self.resolve(e).ok()))
                .map(|(_, e)| e)
                .filter(|e| array.is_some() || !e.is_null());
            let Some(entry) = entry else {
                continue;
            };
            found += 1;
            let required = self
                .with_cx((id, object), at, parents, None, |cx| self.required(row, cx))
                == Truth::True;
            let decisive = matches!(row.key.as_str(), "Type" | "Subtype" | "S");
            let first = array.is_some() && row.key == "0";
            let typed = row.r#type.iter().position(|t| {
                (t.wraps.iter())
                    .all(|w| w.func != WrapFn::Extension || self.extensions.contains(&w.arg))
                    && self.matches(entry, t.item)
            });
            let Some(i) = typed else {
                score += if decisive {
                    20
                } else if first {
                    3
                } else {
                    1
                };
                bad |= required;
                continue;
            };
            let special = &row.special_case[i];
            let allowed = self.with_cx((id, object), at, parents, Some(table), |cx| {
                !matches!(self.values(row, i, entry, cx), Values::Not(_))
            }) && (special.is_empty()
                || self.with_cx((id, object), at, parents, Some(table), |cx| {
                    predicate::holds_fully(special, cx) != Truth::False
                }));
            if allowed {
                score += if decisive || matches!(row.key.as_str(), "Parent" | "TransformMethod") {
                    -80
                } else if first && required {
                    -60
                } else if first {
                    -20
                } else if required {
                    -10
                } else {
                    -4
                };
            } else {
                score += if decisive {
                    10
                } else if first {
                    7
                } else {
                    5
                };
                bad |= required;
            }
            if version(&row.deprecated_in).is_some_and(|v| self.version >= v) {
                score += 8;
            }
        }
        if !bad {
            score -= 8 * found;
        }
        score + (-10.0 * f64::from(found) / rows.len() as f64) as i32
    }

    /// Walks the name or number tree `root` (with its number, when it's an
    /// indirect object), the value of `row`'s key at `step` from `from`'s
    /// object, of the row's `i`th type: each node through its `/Kids`, its
    /// `/Names` or `/Nums` pairs' values as the table of the type's links
    /// they fit best, as TestGrammar's `parse_name_tree` and
    /// `parse_number_tree` do. A node that breaks ISO 32000-2 7.9.6 or
    /// 7.9.7 is a [`Rule::Tree`] finding.
    fn tree(
        &mut self,
        from: &Visit<'a>,
        step: Step<'a>,
        (id, root): (Option<ObjectId>, &'a Object),
        row: &'static Row,
        i: usize,
        parents: &[&'a Object],
    ) {
        let ty = row.r#type[i].item;
        let links = self.links(&row.link[i]);
        let (entries, names): (&[u8], _) = match ty {
            ArlType::NameTree => (b"Names", true),
            _ => (b"Nums", false),
        };
        let mut up = Vec::with_capacity(PARENTS);
        up.push(from.object);
        up.extend(parents.iter().take(PARENTS - 1));
        let depth = if id.is_some() { 0 } else { from.depth + 1 };
        let mut nodes = vec![(root, id, depth, true)];
        while let Some((node, node_id, depth, is_root)) = nodes.pop() {
            if let Some(n) = node_id {
                if self.reached.get(&n).copied().flatten().is_some() {
                    continue;
                }
                self.reached.insert(n, Some(ty.name()));
            }
            let Value::Dict(dict) = node.value() else {
                continue;
            };
            let here = Visit {
                object: node,
                holder: node_id.or(from.holder),
                table: from.table,
                trail: Rc::new(Trail {
                    up: Some(from.trail.clone()),
                    step: step.clone(),
                    object: node,
                }),
                depth,
                loose: from.loose,
                also: None,
            };
            let made = self.made(here.holder);
            // What's wrong with the node, as a tree finding unless said.
            let mut wrong = Vec::new();
            let gone = |what: &str, (n, g): ObjectId| {
                (
                    Rule::Unresolved,
                    format!("its {what} is {n} {g} R, no object"),
                )
            };
            if made.is_some() {
                *self
                    .report
                    .checked
                    .entry(ty.name().to_string())
                    .or_default() += 1;
                let has = |k: &[u8]| dict.get(k).is_some_and(|o| !o.is_null());
                let entries_name = String::from_utf8_lossy(entries);
                let (kids, leaves, limits) = (has(b"Kids"), has(entries), has(b"Limits"));
                if is_root && !kids && !leaves {
                    let what = format!("a root with neither /Kids nor /{entries_name}");
                    wrong.push((Rule::Tree, what));
                }
                if kids && leaves {
                    let what = format!("a node with both /Kids and /{entries_name}");
                    wrong.push((Rule::Tree, what));
                }
                if is_root && limits {
                    wrong.push((Rule::Tree, "a root with /Limits".to_string()));
                }
                if !is_root && !limits {
                    let what = "a node under the root without /Limits".to_string();
                    wrong.push((Rule::Tree, what));
                }
            }
            if let Some(raw) = dict.get(entries) {
                self.refer(raw, from.table, row.key.clone());
                match self.resolve(raw).map(|(_, o)| o.value()) {
                    Ok(Value::Array(pairs)) => {
                        for (k, pair) in pairs.chunks(2).enumerate() {
                            let (label, value) = match pair {
                                [label, value] => (label, value),
                                _ => {
                                    let what = format!("its last key, [{}], has no value", 2 * k);
                                    wrong.push((Rule::Tree, what));
                                    continue;
                                }
                            };
                            let label = match self.resolve(label).map(|(_, o)| o.value()) {
                                Ok(Value::String(s, _)) if names => {
                                    s.text().unwrap_or_else(|| "…".to_string())
                                }
                                Ok(Value::Integer(n)) if !names => n.to_string(),
                                _ => {
                                    let kind = if names { "a string" } else { "an integer" };
                                    let what = format!("its key [{}] isn't {kind}", 2 * k);
                                    wrong.push((Rule::Tree, what));
                                    continue;
                                }
                            };
                            self.refer(value, from.table, row.key.clone());
                            match self.resolve(value) {
                                Ok((vid, v))
                                    if matches!(
                                        v.value(),
                                        Value::Dict(_) | Value::Stream(_) | Value::Array(_)
                                    ) =>
                                {
                                    self.reach(
                                        &here,
                                        Step::Entry(label),
                                        (vid, v),
                                        &links,
                                        &up,
                                        &[],
                                    )
                                }
                                Ok((vid, v)) => {
                                    if !links.is_empty() {
                                        let what =
                                            format!("its {label} is {}, of no table", render(v));
                                        wrong.push((Rule::Tree, what));
                                    }
                                    if let Some(vid) = vid {
                                        self.reached.entry(vid).or_insert(None);
                                    }
                                }
                                Err(to) => wrong.push(gone(&label, to)),
                            }
                        }
                    }
                    Ok(Value::Null) => {}
                    Ok(_) => {
                        let what =
                            format!("its /{} isn't an array", String::from_utf8_lossy(entries));
                        wrong.push((Rule::Tree, what));
                    }
                    Err(to) => {
                        wrong.push(gone(&format!("/{}", String::from_utf8_lossy(entries)), to))
                    }
                }
            }
            if let Some(raw) = dict.get(b"Kids") {
                self.refer(raw, from.table, row.key.clone());
                match self.resolve(raw).map(|(_, o)| o.value()) {
                    Ok(Value::Array(kids)) => {
                        for (i, kid) in kids.iter().enumerate() {
                            self.refer(kid, from.table, row.key.clone());
                            match self.resolve(kid) {
                                Ok((kid_id, kid)) if matches!(kid.value(), Value::Dict(_)) => {
                                    let depth = if kid_id.is_some() { 0 } else { depth + 1 };
                                    if depth <= NESTING {
                                        nodes.push((kid, kid_id, depth, false));
                                    } else if made.is_some() {
                                        self.report.truncated += 1;
                                    }
                                }
                                Ok((_, other)) => {
                                    let what = format!(
                                        "its /Kids[{i}] is {}, not a dictionary",
                                        render(other)
                                    );
                                    wrong.push((Rule::Tree, what));
                                }
                                Err(to) => wrong.push(gone(&format!("/Kids[{i}]"), to)),
                            }
                        }
                    }
                    Ok(Value::Null) => {}
                    Ok(_) => wrong.push((Rule::Tree, "its /Kids isn't an array".to_string())),
                    Err(to) => wrong.push(gone("/Kids", to)),
                }
            }
            if let Some(made) = made {
                for (rule, what) in wrong {
                    let table = &from.table.name;
                    self.find(&here, made, rule, table, &row.key, what, node.at);
                }
            }
        }
    }

    /// Whether `v`'s dictionary has each key its table requires, for an
    /// inheritable one on a `/Parent` if not on it.
    fn required_keys(&mut self, v: &Visit<'a>, dict: &'a Dict, made: Made, parents: &[&'a Object]) {
        for row in &v.table.rows {
            // A wildcard is never required (INTERNAL_GRAMMAR.md, "Column 5
            // - Required").
            let key = row.key.as_bytes();
            if row.key.ends_with('*') || self.present(dict.get(key)) {
                continue;
            }
            match self.with_cx((v.id(), v.object), At::Key(key), parents, None, |cx| {
                self.required(row, cx)
            }) {
                Truth::True if row.inheritable && self.inherited(dict, key) => {}
                Truth::True => {
                    let (table, at) = (&v.table.name, v.object.at);
                    let absent = "absent".to_string();
                    self.find(v, made, Rule::Missing, table, &row.key, absent, at);
                }
                Truth::False => {}
                Truth::Unknown(why) => self.unchecked("Required", why, &v.table.name, &row.key),
            }
        }
    }

    /// Whether `v`'s array, `n` long, has each element its table requires:
    /// those before its repeating rows, and of its repeating rows, those
    /// of the last set it starts (or the first).
    fn required_elements(&mut self, v: &Visit<'a>, n: usize, made: Made, parents: &[&'a Object]) {
        let set = v.table.repeating().len();
        let first = v.table.rows.len() - set;
        for (p, row) in v.table.rows.iter().enumerate() {
            let i = if p < first {
                match row.key.parse::<usize>() {
                    Ok(i) => i,
                    // A wildcard is never required.
                    Err(_) => continue,
                }
            } else {
                let sets = n.saturating_sub(first).div_ceil(set).max(1);
                first + (sets - 1) * set + (p - first)
            };
            if i < n {
                continue;
            }
            match self.with_cx((v.id(), v.object), At::Index(i), parents, None, |cx| {
                self.required(row, cx)
            }) {
                Truth::True => {
                    let (table, at) = (&v.table.name, v.object.at);
                    let absent = "absent".to_string();
                    self.find(v, made, Rule::Missing, table, &row.key, absent, at);
                }
                Truth::False => {}
                Truth::Unknown(why) => self.unchecked("Required", why, &v.table.name, &row.key),
            }
        }
    }

    /// Whether `row`'s key is required where `cx` is: never before its
    /// SinceVersion, nor an extension's key (TestGrammar's
    /// `IsValidForPDFVersion`); else as its Required cell says.
    fn required(&self, row: &Row, cx: &Cx) -> Truth {
        match predicate::since(&row.since_version, self.extensions) {
            Since::In(v) if self.version >= v => predicate::required(&row.required, cx),
            _ => Truth::False,
        }
    }

    /// Whether an entry is there: not absent, and not null, which a
    /// dictionary's entry may be (ISO 32000-2, 7.3.7) through a reference
    /// too. A reference to no object is there, found unresolved.
    fn present(&self, entry: Option<&Object>) -> bool {
        match entry.map(|e| self.resolve(e)) {
            None => false,
            Some(Ok((_, o))) => !o.is_null(),
            Some(Err(_)) => true,
        }
    }

    /// Whether a `/Parent` up from `dict` has `key`. Or, for `/DA`, which
    /// only fields inherit, whether the interactive form dictionary has it:
    /// its `/DA` is the document-wide default (ISO 32000-2, 12.7.4, Table
    /// 224), where TestGrammar looks up the `/Parent`s alone.
    fn inherited(&self, dict: &'a Dict, key: &[u8]) -> bool {
        self.up(dict, |parent| self.present(parent.get(key)).then_some(()))
            .is_some()
            || key == b"DA" && (self.form()).is_some_and(|form| self.present(form.get(key)))
    }

    /// `dict`'s `key`, its references followed, else the nearest
    /// `/Parent`'s up from it that has it; null is none.
    fn inheritable(&self, dict: &'a Dict, key: &[u8]) -> Option<&'a Object> {
        let value = |dict: &'a Dict| {
            (dict.get(key).and_then(|o| self.resolve(o).ok()))
                .map(|(_, o)| o)
                .filter(|o| !o.is_null())
        };
        value(dict).or_else(|| self.up(dict, value))
    }

    /// The first of `f` of the `/Parent`s up from `dict` that's some: each
    /// once, and no more of them than the page tree's depth.
    fn up<T>(&self, dict: &'a Dict, mut f: impl FnMut(&'a Dict) -> Option<T>) -> Option<T> {
        let mut seen = HashSet::new();
        let mut at = dict;
        for _ in 0..PAGE_TREE_DEPTH {
            let (id, parent) = match at.get(b"Parent").map(|p| self.resolve(p)) {
                Some(Ok((id, parent))) => match parent.value() {
                    Value::Dict(parent) => (id, parent),
                    _ => break,
                },
                _ => break,
            };
            if let Some(id) = id
                && !seen.insert(id)
            {
                break;
            }
            if let Some(found) = f(parent) {
                return Some(found);
            }
            at = parent;
        }
        None
    }

    /// The interactive form dictionary: the catalog's `/AcroForm`.
    fn form(&self) -> Option<&'a Dict> {
        let dict = |o: &'a Object| self.resolve(o).ok()?.1.as_dict();
        let catalog = dict(self.trailer.get(b"Root")?)?;
        dict(catalog.get(b"AcroForm")?)
    }

    /// `value`'s type among `row`'s: the first it is that the file's
    /// version lets in, and not as deprecated if one is.
    fn typed(&self, row: &Row, value: &Object) -> Typed {
        let (mut deprecated, mut excluded) = (None, None);
        for (i, ty) in row.r#type.iter().enumerate() {
            if !self.matches(value, ty.item) {
                continue;
            }
            match predicate::gate(&ty.wraps, self.version, self.extensions) {
                Gate::Allowed => return Typed::Is(i, None),
                Gate::AllowedDeprecated(from) => {
                    deprecated.get_or_insert(Typed::Is(i, Some(from)));
                }
                Gate::Excluded => {
                    excluded.get_or_insert(Typed::Excluded(ty.item));
                }
            }
        }
        deprecated.or(excluded).unwrap_or(Typed::Not)
    }

    /// Whether `o` is of the type `ty`, as TestGrammar's `ArlVersion` tells
    /// (ArlVersion.cpp): an integer is an integer, a bitmask or a number,
    /// a string any string or a date, an array an array, a rectangle or a
    /// matrix, and a dictionary a dictionary or a tree. Refined as the
    /// model's types are (INTERNAL_GRAMMAR.md, "Column 2 - Type"): a
    /// rectangle is four numbers and a matrix six (ISO 32000-2 7.9.5 and
    /// 8.3.4), a string-ascii printable ASCII, and a date reads as
    /// [`is_date`] reads one. A string whose bytes the reader didn't give
    /// is taken to be what it may be ([`unknown_bytes`]).
    fn matches(&self, o: &Object, ty: ArlType) -> bool {
        use ArlType as T;
        match (o.value(), ty) {
            (Value::Integer(_), T::Integer | T::Bitmask | T::Number)
            | (Value::Real(_), T::Number)
            | (Value::Bool(_), T::Boolean)
            | (Value::Name(_), T::Name)
            | (Value::Null, T::Null)
            | (Value::Stream(_), T::Stream)
            | (Value::String(..), T::String | T::StringByte | T::StringText)
            | (Value::Array(_), T::Array)
            | (Value::Dict(_), T::Dictionary | T::NameTree | T::NumberTree) => true,
            (Value::String(s, _), T::StringAscii) => {
                (s.as_bytes()).is_none_or(|b| b.iter().all(|b| (0x20..=0x7E).contains(b)))
            }
            (Value::String(s, _), T::Date) => s.text().is_none_or(|t| is_date(&t)),
            (Value::Array(a), T::Rectangle) => self.numbers(a, 4),
            (Value::Array(a), T::Matrix) => self.numbers(a, 6),
            _ => false,
        }
    }

    /// Whether `a` is `n` numbers.
    fn numbers(&self, a: &[Object], n: usize) -> bool {
        a.len() == n
            && (a.iter()).all(|e| {
                (self.resolve(e))
                    .is_ok_and(|(_, o)| matches!(o.value(), Value::Integer(_) | Value::Real(_)))
            })
    }

    /// What `row`'s PossibleValues for its `i`th type make of `value`,
    /// of those the file's version lets in: each a constant
    /// ([`is_value`]), the wildcard `*` for any name, or a predicate
    /// ([`predicate::holds_fully`]). A predicate that can't be told makes
    /// them all unknown, met or not; so does ciphertext.
    ///
    /// `fn:RequiredValue(condition,value)` is its value, listed, when the
    /// condition doesn't hold, and the value required, whatever else is
    /// listed, when it does ([`predicate::required_value`]): the first
    /// one that holds says, and when one can't be told, a value other
    /// than its own can't be either. TestGrammar's `ReducePVRow` takes
    /// whichever item a value matches first, so that a later one lets
    /// through a value an earlier one's condition rules out;
    /// INTERNAL_GRAMMAR.md's "must be" is followed here.
    fn values(&self, row: &'static Row, i: usize, value: &Object, cx: &Cx) -> Values {
        let items = &row.possible_values[i];
        if items.is_empty() {
            return Values::Allowed;
        }
        if matches!(value.value(), Value::String(Str::Encrypted(_), _)) {
            return Values::Unknown("the string's bytes".to_string());
        }
        let (mut listed, mut met, mut unknown) = (Vec::new(), None, None);
        let mut required = None;
        for item in items {
            let gate = predicate::gate(&item.wraps, self.version, self.extensions);
            if gate == Gate::Excluded {
                continue;
            }
            listed.push(item.item.as_str());
            let truth = if let Some((holds, needed)) = predicate::required_value(&item.item, cx) {
                let is = is_value(&needed, value);
                match holds {
                    Truth::True if required.is_none() => {
                        required = Some((is, needed, item.item.as_str()));
                    }
                    Truth::Unknown(why) if !is => {
                        unknown.get_or_insert(why);
                    }
                    _ => {}
                }
                if is { Truth::True } else { Truth::False }
            } else if item.item.starts_with("fn:") {
                predicate::holds_fully(&item.item, cx)
            } else if is_value(&item.item, value) {
                Truth::True
            } else {
                Truth::False
            };
            match (truth, gate) {
                (Truth::True, Gate::AllowedDeprecated(from)) => {
                    met.get_or_insert(Values::Deprecated(from));
                }
                (Truth::True, _) => met = Some(Values::Allowed),
                (Truth::False, _) => {}
                (Truth::Unknown(why), _) => {
                    unknown.get_or_insert(why);
                }
            }
        }
        match (required, unknown, met) {
            (Some((false, needed, item)), _, _) => Values::Required(needed, item),
            (_, Some(why), _) => Values::Unknown(why),
            (_, None, Some(met)) => met,
            (_, None, None) => Values::Not(listed),
        }
    }

    /// `f` of the predicates' view of the row at `at` in `container`, the
    /// indirect object `id` when it's one, with `defaults`, the
    /// container's table for a SpecialCase or PossibleValues.
    fn with_cx<R>(
        &self,
        (id, container): (Option<ObjectId>, &Object),
        at: At,
        parents: &[&Object],
        defaults: Option<&'static Table>,
        f: impl FnOnce(&Cx) -> R,
    ) -> R {
        let row = self.file.at(container, at).parents(parents);
        let row = row.defaults(defaults);
        f(&id.map_or(row, |id| row.id(id)))
    }

    /// `raw` with its references followed, at most [`REFERENCES`]: the
    /// number of the object it's in, if it's an indirect object, and it.
    /// The number of one the file hasn't, or of the last of a chain too
    /// long, is the error.
    fn resolve<'o>(&self, raw: &'o Object) -> Result<(Option<ObjectId>, &'o Object), ObjectId>
    where
        'a: 'o,
    {
        let (mut id, mut at) = (None, raw);
        for _ in 0..REFERENCES {
            let Value::Ref(to) = *at.value() else {
                return Ok((id, at));
            };
            at = self.source.get(to).map_err(|_| to)?;
            id = Some(to);
        }
        Err(id.unwrap_or((0, 0)))
    }

    /// Whether `key` is exempt from being found unlisted.
    fn exempt(&self, key: &[u8]) -> bool {
        (self.exempt.iter()).any(|e| match e.prefix {
            true => key.starts_with(&e.name),
            false => key == e.name.as_slice(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn find(
        &mut self,
        visit: &Visit,
        made: Made,
        rule: Rule,
        table: &str,
        key: &str,
        value: String,
        at: Option<Span>,
    ) {
        self.report.findings.push(Finding {
            object: visit.holder,
            made,
            path: visit.trail.path(),
            table: table.to_string(),
            key: key.to_string(),
            rule,
            value,
            at,
        });
    }

    /// What the version checked at deprecates at `key` of `v`'s held
    /// object, `value` and why: counted, and with [`Options::deprecations`]
    /// a finding.
    fn deprecated(
        &mut self,
        v: &Visit,
        made: Made,
        table: &str,
        key: &str,
        value: String,
        written: Option<Span>,
    ) {
        let at = (table.to_string(), key.to_string());
        *self.report.deprecated.entry(at).or_default() += 1;
        if self.deprecations {
            self.find(v, made, Rule::Deprecated, table, key, value, written);
        }
    }

    fn unchecked(&mut self, column: &'static str, why: String, table: &str, key: &str) {
        let unchecked = Unchecked {
            column,
            why,
            table: table.to_string(),
            key: key.to_string(),
        };
        *self.report.unchecked.entry(unchecked).or_default() += 1;
    }

    /// Leaves out of the unlisted keys, and their findings, each held
    /// indirect object's own that a table it was reached again as lists:
    /// it's both, and the first its keys were checked against.
    fn excuse(&mut self) {
        let mut gone = HashSet::new();
        for (id, key, unlisted, finding) in std::mem::take(&mut self.unlisted_in) {
            let Some(again) = self.again.get(&id) else {
                continue;
            };
            let at = At::Key(&key);
            if !(again.iter()).any(|t| walked(t, t.row(at), self.extensions).0.is_some()) {
                continue;
            }
            if let Some(n) = self.report.unlisted.get_mut(&unlisted) {
                *n -= 1;
                if *n == 0 {
                    self.report.unlisted.remove(&unlisted);
                }
            }
            gone.extend(finding);
        }
        let mut i = 0;
        self.report.findings.retain(|_| {
            i += 1;
            !gone.contains(&(i - 1))
        });
    }

    /// The held objects the walk never reached, each a [`Rule::Unreached`]
    /// finding but those referenced under a key the model doesn't list,
    /// and the file's own structures (object and cross-reference streams,
    /// a linearization dictionary). Its table and key are those it was
    /// referenced under in an object walked, else the key of a held object
    /// unreached too, else `-`. One the file hasn't, or that doesn't read,
    /// is [`Rule::Unresolved`].
    fn unreached(&mut self) {
        let held: Vec<(ObjectId, Made)> = match self.hold {
            Hold::All(made) => (self.source.ids().into_iter())
                .map(|id| (id, *made))
                .collect(),
            Hold::Only(objects) => objects.iter().map(|(&id, &made)| (id, made)).collect(),
        };
        let mut left = Vec::new();
        for (id, made) in held {
            if self.reached.contains_key(&id) || self.under_unlisted.contains(&id) {
                continue;
            }
            match self.source.get(id) {
                Ok(object) if left_out(object) => {}
                Ok(object) => left.push((id, made, object)),
                Err(why) => self.report.findings.push(Finding {
                    object: Some(id),
                    made,
                    path: "-".to_string(),
                    table: "-".to_string(),
                    key: "-".to_string(),
                    rule: Rule::Unresolved,
                    value: why.to_string(),
                    at: None,
                }),
            }
        }
        let mut among = HashMap::new();
        for &(_, _, object) in &left {
            let entries: Vec<(String, &Object)> = if let Some(d) = object.as_dict() {
                (d.iter())
                    .map(|(k, v)| (String::from_utf8_lossy(k).into_owned(), v))
                    .collect()
            } else if let Some(a) = object.as_array() {
                a.iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v))
                    .collect()
            } else {
                Vec::new()
            };
            for (key, value) in entries {
                each_reference(value, |to| {
                    among.entry(to).or_insert_with(|| key.clone());
                });
            }
        }
        for (id, made, object) in left {
            let (table, key) = match self.referrers.get(&id) {
                Some((table, key)) => (table.to_string(), key.clone()),
                None => (
                    "-".to_string(),
                    among.get(&id).cloned().unwrap_or("-".to_string()),
                ),
            };
            self.report.findings.push(Finding {
                object: Some(id),
                made,
                path: "-".to_string(),
                table,
                key,
                rule: Rule::Unreached,
                value: render(object),
                at: object.at,
            });
        }
    }

    /// The reader's warnings in held objects, and those of the file as a
    /// whole as the trailer's, each a [`Rule::Reader`] finding.
    fn read_as(&mut self, warnings: &[Warning]) {
        for warning in warnings {
            let Some(made) = self.made(warning.object) else {
                continue;
            };
            self.report.findings.push(Finding {
                object: warning.object,
                made,
                path: "-".to_string(),
                table: "-".to_string(),
                key: "-".to_string(),
                rule: Rule::Reader,
                value: warning.message.clone(),
                at: warning.at,
            });
        }
    }
}

impl<'a> Trail<'a> {
    /// The objects the walk came through to this one, nearest first, as
    /// many as `parent::` reaches.
    fn parents(&self) -> Vec<&'a Object> {
        let mut parents = Vec::with_capacity(PARENTS);
        let mut at = self.up.as_deref();
        while let Some(trail) = at
            && parents.len() < PARENTS
        {
            parents.push(trail.object);
            at = trail.up.as_deref();
        }
        parents
    }

    /// The way from the trailer, its last [`PATH`] steps:
    /// `Root/Pages/Kids[0]/Annots[2]`, a tree's entries in braces.
    fn path(&self) -> String {
        let mut steps = Vec::new();
        let mut at = Some(self);
        while let Some(trail) = at
            && steps.len() <= PATH
        {
            steps.push(&trail.step);
            at = trail.up.as_deref();
        }
        let mut path = String::new();
        if steps.len() > PATH {
            steps.truncate(PATH);
            path.push('…');
        }
        for step in steps.into_iter().rev() {
            match step {
                Step::Trailer => {}
                Step::Key(key) => {
                    if !path.is_empty() {
                        path.push('/');
                    }
                    path.push_str(&String::from_utf8_lossy(key));
                }
                Step::Index(i) => {
                    let _ = write!(path, "[{i}]");
                }
                Step::Entry(entry) => {
                    let _ = write!(path, "{{{entry}}}");
                }
            }
        }
        if path.is_empty() {
            "trailer".to_string()
        } else {
            path
        }
    }
}

/// Whether a value of type `ty` is a string whose bytes the reader didn't
/// give, so that whether it's of the type can't be told: a string-ascii
/// read as text, and a string-ascii or date read as ciphertext.
fn unknown_bytes(value: &Object, ty: ArlType) -> bool {
    matches!(
        (value.value(), ty),
        (Value::String(Str::Text(_), _), ArlType::StringAscii)
            | (
                Value::String(Str::Encrypted(_), _),
                ArlType::StringAscii | ArlType::Date
            )
    )
}

/// Whether `object` is one of the file's own structures, which nothing
/// reaches: an object stream, a cross-reference stream, a linearization
/// dictionary.
fn left_out(object: &Object) -> bool {
    match object.value() {
        Value::Stream(s) => {
            matches!(
                s.dict.get(b"Type").and_then(Object::as_name),
                Some(b"ObjStm" | b"XRef")
            )
        }
        Value::Dict(d) => d.has(b"Linearized"),
        _ => false,
    }
}

/// The field tables, as a widget's `/Parent` links them.
const FIELDS: [&str; 7] = [
    "FieldTx",
    "FieldBtnPush",
    "FieldBtnCheckbox",
    "FieldBtnRadio",
    "FieldChoice",
    "FieldSig",
    "Field",
];

/// The keys only field tables list, not the widget's ([`Walk::merged`]).
fn field_keys() -> Vec<&'static [u8]> {
    let table = |name| {
        grammar()
            .table(name)
            .expect("the model has the field tables")
    };
    let widget = table("AnnotWidget");
    let mut keys: Vec<&'static [u8]> = (FIELDS.into_iter().map(table))
        .flat_map(|t| &t.rows)
        .map(|row| row.key.as_bytes())
        .filter(|&key| key != b"*" && !widget.rows.iter().any(|row| row.key.as_bytes() == key))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// `table`'s `row` as the walk takes it, under `extensions`: the row of
/// another extension is no row (TestGrammar without it in its
/// `--extensions`), but the table's wildcard, if it has one; and that
/// extension's name.
fn walked(
    table: &'static Table,
    row: Option<&'static Row>,
    extensions: &BTreeSet<String>,
) -> (Option<&'static Row>, Option<String>) {
    let since = |row: &Row| predicate::since(&row.since_version, extensions);
    match row.map(|row| (row, since(row))) {
        Some((_, Since::Extension(name))) => {
            let wildcard = (table.row(At::Key(b"*")))
                .filter(|row| row.key == "*" && !matches!(since(row), Since::Extension(_)));
            (wildcard, Some(name))
        }
        row => (row.map(|(row, _)| row), None),
    }
}

/// Whether two findings are the same but for their tables.
fn same(a: &Finding, b: &Finding) -> bool {
    (a.object, a.made, &a.path, &a.key, a.rule, &a.value, a.at)
        == (b.object, b.made, &b.path, &b.key, b.rule, &b.value, b.at)
}

/// Whether `table` is one of the model's `_Universal` tables, which fit
/// anything.
fn universal(table: &str) -> bool {
    table.starts_with("_Universal")
}

/// Whether `object` can be of `table`'s kind: an array of a table of
/// elements (its keys indices, repeating rows' `1*` or the wildcard), a
/// dictionary or stream of a table of keys; a table of the wildcard alone,
/// either.
fn fits(object: &Object, table: &Table) -> bool {
    let element = |row: &Row| {
        let index = row.key.strip_suffix('*').unwrap_or(&row.key);
        !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())
    };
    let keyed = |row: &Row| row.key != "*" && !element(row);
    match object.value() {
        Value::Array(_) => !table.rows.iter().any(keyed),
        Value::Dict(_) | Value::Stream(_) => !table.rows.iter().any(element),
        _ => true,
    }
}

/// The array index a row's key names: `0`, or a repeating row's `1*`;
/// none for the wildcard (TestGrammar's `key_to_array_index`).
fn index(key: &str) -> Option<usize> {
    let digits = key.bytes().take_while(u8::is_ascii_digit).count();
    key[..digits].parse().ok()
}

/// A key or element as findings name it.
fn at_text(at: At) -> String {
    match at {
        At::Key(key) => String::from_utf8_lossy(key).into_owned(),
        At::Index(i) => i.to_string(),
    }
}

/// Whether `value` is the PossibleValues constant `text`
/// (INTERNAL_GRAMMAR.md, "PDF Object conventions" and "Column 9 -
/// PossibleValues"): a name, or any name for `*`; a string in single
/// quotes; a number, near enough; `true` or `false`; an array in square
/// brackets, its elements apart by spaces (an image mask's `[0 1]`). A
/// null, a dictionary or a stream is any.
fn is_value(text: &str, value: &Object) -> bool {
    match value.value() {
        Value::Array(elements) => {
            let inner = text.strip_prefix('[').and_then(|t| t.strip_suffix(']'));
            inner.is_some_and(|inner| {
                let items: Vec<&str> = inner.split_ascii_whitespace().collect();
                items.len() == elements.len()
                    && (items.iter().zip(elements)).all(|(item, e)| is_value(item, e))
            })
        }
        Value::Name(name) => text == "*" || name.as_slice() == text.as_bytes(),
        Value::String(s, _) => {
            let quoted = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\''));
            quoted.is_some_and(|quoted| s.text().as_deref() == Some(quoted))
        }
        Value::Integer(_) | Value::Real(_) => {
            let n = value.as_number().unwrap_or(f64::NAN);
            text.parse::<f64>()
                .is_ok_and(|t| (t - n).abs() <= TOLERANCE)
        }
        Value::Bool(b) => text == if *b { "true" } else { "false" },
        Value::Null | Value::Dict(_) | Value::Stream(_) | Value::Ref(_) => true,
    }
}

/// Each reference in `o`, and in the direct objects it holds, no deeper
/// than [`NESTING`].
pub(crate) fn each_reference(o: &Object, mut f: impl FnMut(ObjectId)) {
    let mut stack = vec![(o, 0)];
    while let Some((o, depth)) = stack.pop() {
        match o.value() {
            Value::Ref(id) => f(*id),
            _ if depth >= NESTING => {}
            Value::Array(a) => stack.extend(a.iter().map(|e| (e, depth + 1))),
            Value::Dict(d) => stack.extend(d.iter().map(|(_, v)| (v, depth + 1))),
            Value::Stream(s) => stack.extend(s.dict.iter().map(|(_, v)| (v, depth + 1))),
            _ => {}
        }
    }
}

/// Whether `text` is a date (ISO 32000-2, 7.9.4) as TestGrammar's
/// `is_valid_pdf_date_string` reads one (`TestGrammar/src/Utils.cpp`,
/// Copyright 2020-2022 PDF Association, Inc., Apache-2.0), ported: `D:`
/// and a year, then as many of month, day, hour, minute and second as
/// follow, two digits each and in range; then an offset's `Z`, `+` or
/// `-`, its hours, an apostrophe and its minutes, each if there. Its
/// expression is anchored only at the start, so anything after is let be.
pub fn is_date(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("D:") else {
        return false;
    };
    let b = rest.as_bytes();
    let two = |at: usize| {
        (b.get(at..at + 2))
            .filter(|d| d.iter().all(u8::is_ascii_digit))
            .map(|d| u32::from(d[0] - b'0') * 10 + u32::from(d[1] - b'0'))
    };
    if !b
        .get(..4)
        .is_some_and(|year| year.iter().all(u8::is_ascii_digit))
    {
        return false;
    }
    let mut at = 4;
    for range in [1..=12, 1..=31, 0..=23, 0..=59, 0..=59] {
        let Some(n) = two(at) else {
            break;
        };
        if !range.contains(&n) {
            return false;
        }
        at += 2;
    }
    if b.get(at).is_some_and(|c| b"Z+-".contains(c)) {
        at += 1;
    }
    if let Some(hours) = two(at) {
        if hours > 23 {
            return false;
        }
        at += 2;
    }
    if b.get(at) == Some(&b'\'') {
        at += 1;
    }
    two(at).is_none_or(|minutes| minutes <= 59)
}

/// `value` with each reference in it, `12 0 R`, as `R`.
pub fn numberless(value: &str) -> String {
    let b = value.as_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let mut out = String::with_capacity(value.len());
    let (mut at, mut kept) = (0, 0);
    while at < b.len() {
        let starts = b[at].is_ascii_digit() && (at == 0 || !b[at - 1].is_ascii_alphanumeric());
        let number = if starts { digits(at) } else { 0 };
        let generation = match b.get(at + number) {
            Some(b' ') if number > 0 => digits(at + number + 1),
            _ => 0,
        };
        let r = at + number + 1 + generation;
        let reference = generation > 0
            && b.get(r..r + 2) == Some(b" R")
            && b.get(r + 2).is_none_or(|c| !c.is_ascii_alphanumeric());
        if reference {
            out.push_str(&value[kept..at]);
            out.push('R');
            at = r + 2;
            kept = at;
        } else {
            at += 1;
        }
    }
    out.push_str(&value[kept..]);
    out
}

/// `o` as a finding has it: its first entries, two deep, names and
/// strings (as text) their first [`TOKEN`] bytes.
/// A finding's value: `o`, its first entries and elements, names and
/// strings cut short. A dictionary's keys are in order, and a stream's
/// encoding (`/Length`, `/Filter`, `/DecodeParms`) left out, so that a
/// value is the same after a rewrite that sorts keys or encodes streams
/// anew, as a finding's must be to be carried ([`Report::beyond`]). Its
/// bytes as written are where its span is.
fn render(o: &Object) -> String {
    let mut out = String::new();
    write_value(&mut out, o, 2);
    out
}

fn write_value(out: &mut String, o: &Object, depth: usize) {
    match o.value() {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => {
            let _ = write!(out, "{b}");
        }
        Value::Integer(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Real(r) => {
            let _ = write!(out, "{r}");
        }
        Value::Name(name) => {
            out.push('/');
            out.push_str(&String::from_utf8_lossy(&name[..name.len().min(TOKEN)]));
        }
        Value::String(s, _) => {
            let text = match s {
                Str::Bytes(bytes) => text::decode(&bytes[..bytes.len().min(TOKEN)]).text,
                Str::Text(text) => text.chars().take(TOKEN).collect(),
                Str::Encrypted(_) => "ciphertext".to_string(),
            };
            let _ = write!(out, "({text})");
        }
        Value::Ref((n, g)) => {
            let _ = write!(out, "{n} {g} R");
        }
        Value::Array(_) | Value::Dict(_) | Value::Stream(_) if depth == 0 => out.push('…'),
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().take(6).enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_value(out, e, depth - 1);
            }
            if a.len() > 6 {
                out.push_str(" …");
            }
            out.push(']');
        }
        Value::Dict(d) => write_dictionary(out, d, depth, false),
        Value::Stream(s) => {
            out.push_str("stream ");
            write_dictionary(out, &s.dict, depth, true);
        }
    }
}

fn write_dictionary(out: &mut String, d: &Dict, depth: usize, stream: bool) {
    let encoding = |key: &[u8]| stream && matches!(key, b"Length" | b"Filter" | b"DecodeParms");
    let mut entries: Vec<_> = d.iter().filter(|(key, _)| !encoding(key)).collect();
    entries.sort_by_key(|&(key, _)| key);
    out.push_str("<<");
    for &(key, value) in entries.iter().take(4) {
        let key = &key[..key.len().min(TOKEN)];
        let _ = write!(out, " /{} ", String::from_utf8_lossy(key));
        write_value(out, value, depth - 1);
    }
    if entries.len() > 4 {
        out.push_str(" …");
    }
    out.push_str(" >>");
}

impl Made {
    pub fn name(self) -> &'static str {
        match self {
            Made::Built => "built",
            Made::Merged => "merged",
        }
    }
}

impl Rule {
    pub const ALL: [Rule; 14] = [
        Rule::Type,
        Rule::Missing,
        Rule::TooNew,
        Rule::Deprecated,
        Rule::Value,
        Rule::Indirect,
        Rule::Direct,
        Rule::Special,
        Rule::Unreached,
        Rule::Unresolved,
        Rule::Tree,
        Rule::Unlisted,
        Rule::Duplicate,
        Rule::Reader,
    ];

    /// Its name, as findings and allowances write it.
    pub fn name(self) -> &'static str {
        match self {
            Rule::Type => "type",
            Rule::Missing => "missing",
            Rule::TooNew => "too-new",
            Rule::Deprecated => "deprecated",
            Rule::Value => "value",
            Rule::Indirect => "indirect",
            Rule::Direct => "direct",
            Rule::Special => "special",
            Rule::Unreached => "unreached",
            Rule::Unresolved => "unresolved",
            Rule::Tree => "tree",
            Rule::Unlisted => "unlisted",
            Rule::Duplicate => "duplicate",
            Rule::Reader => "reader",
        }
    }

    /// The rule named `name`.
    pub fn named(name: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|r| r.name() == name)
    }
}

impl Class {
    /// The kind of name `key` is: [`Class::Second`] or [`Class::Third`] by
    /// TestGrammar's expressions for them (`^([a-zA-Z0-9_\-]{4,5}(_|:))`
    /// and `^XX`), else [`Class::Other`].
    pub fn of(key: &[u8]) -> Class {
        let prefix = |n: usize| {
            key.len() > n
                && (key[..n].iter()).all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                && matches!(key[n], b'_' | b':')
        };
        if prefix(4) || prefix(5) {
            Class::Second
        } else if key.starts_with(b"XX") {
            Class::Third
        } else {
            Class::Other
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Class::Second => f.write_str("second-class"),
            Class::Third => f.write_str("third-class"),
            Class::Extension(name) => write!(f, "{name}'s"),
            Class::Other => f.write_str("other"),
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let object = match self.object {
            Some((n, g)) => format!("{n} {g} R"),
            None => "the trailer".to_string(),
        };
        let value: String = match self.value.char_indices().nth(SHOWN - 1) {
            Some((cut, _)) if self.value.chars().count() > SHOWN => {
                format!("{}…", &self.value[..cut])
            }
            _ => self.value.clone(),
        };
        write!(
            f,
            "{}: {}/{} at {}, in {object} ({}): {value}",
            self.rule,
            self.table,
            self.key,
            self.path,
            self.made.name(),
        )?;
        match self.at {
            Some(Span {
                stream: None,
                start,
                end,
                ..
            }) => write!(f, " [bytes {start}..{end}]"),
            Some(Span {
                stream: Some((n, g)),
                start,
                end,
                ..
            }) => write!(f, " [bytes {start}..{end} of {n} {g} R's data]"),
            None => Ok(()),
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let objects = self.objects();
        let unchecked: usize = self.unchecked.values().sum();
        let unlisted: usize = self.unlisted.values().sum();
        let ignored: usize = self.ignored.values().sum();
        let deprecated: usize = self.deprecated.values().sum();
        writeln!(
            f,
            "Arlington: {objects} held objects checked as {} tables, {} keys; {} findings; \
             {unchecked} checks not made for predicates that can't be told; {unlisted} keys \
             the model doesn't list; {deprecated} deprecated; {ignored} that readers ignore; \
             {} with no table, {} ties, \
             {} truncated, {} in two contexts, {} as two tables",
            self.checked.len(),
            self.keys,
            self.findings.len(),
            self.no_table,
            self.ties,
            self.truncated,
            self.two_contexts,
            self.two_tables
        )?;
        if !self.extensions.is_empty() {
            let names: Vec<&str> = self.extensions.iter().map(String::as_str).collect();
            writeln!(f, "  under extensions: {}", names.join(", "))?;
        }
        for (table, n) in &self.checked {
            writeln!(f, "  checked as {table}: {n}")?;
        }
        let mut by_why: BTreeMap<(&str, &str), (usize, Vec<String>)> = BTreeMap::new();
        for (u, n) in &self.unchecked {
            let (count, keys) = by_why.entry((u.column, &u.why)).or_default();
            *count += n;
            keys.push(format!("{}/{}", u.table, u.key));
        }
        for ((column, why), (n, keys)) in by_why {
            let more = if keys.len() > 3 { ", …" } else { "" };
            let some = keys[..keys.len().min(3)].join(", ");
            writeln!(f, "  not checked, {column} {why}: {n} ({some}{more})")?;
        }
        for (u, n) in &self.unlisted {
            writeln!(f, "  unlisted: {}/{} ({}): {n}", u.table, u.key, u.class)?;
        }
        for ((table, key), n) in &self.deprecated {
            writeln!(f, "  deprecated: {table}/{key}: {n}")?;
        }
        for ((table, key), n) in &self.ignored {
            writeln!(f, "  ignored: {table}/{key}: {n}")?;
        }
        for finding in &self.findings {
            writeln!(f, "- {finding}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! The walk: a clean file reaches its tables with no findings, written
    //! with a table and with a cross-reference stream; each rule's finding
    //! on a fault planted in one, and where its bytes are; links chosen as
    //! TestGrammar chooses them; only held objects found in; and every walk
    //! bounded, on cycles and on deep nesting.

    use pdf_structure::write::{self, Body, Xref};

    use super::*;
    use crate::bytes;
    use crate::dict;
    use crate::source::Document;

    /// `objects`, the catalog object 1, written at `version` with a
    /// section of `kind`.
    fn written(version: &str, objects: &[(u32, Body)], kind: Xref) -> Vec<u8> {
        written_with(version, objects, kind, Dict::new())
    }

    /// [`written`], with `trailer`'s entries in the trailer too.
    fn written_with(version: &str, objects: &[(u32, Body)], kind: Xref, trailer: Dict) -> Vec<u8> {
        let mut trailer = trailer;
        trailer.set("Root", r(1));
        write::file(version, objects, trailer, kind)
    }

    /// `bytes` read and checked, holding what `hold` does.
    fn checked_as(bytes: &[u8], hold: &Hold) -> Report {
        check(&bytes::read(bytes), hold, &Options::default())
    }

    /// `objects` written at 1.7 with a table and checked, every object held
    /// as built.
    fn checked(objects: &[(u32, Body)]) -> Report {
        let bytes = written("1.7", objects, Xref::Table);
        checked_as(&bytes, &Hold::All(Made::Built))
    }

    fn r(n: u32) -> Object {
        Object::reference(n)
    }

    fn numbers(n: &[i64]) -> Object {
        n.iter()
            .map(|&n| Object::from(n))
            .collect::<Vec<_>>()
            .into()
    }

    fn text(s: &str) -> Object {
        Object::string(s)
    }

    /// Where the last `startxref` points, in `bytes`.
    fn startxref(bytes: &[u8]) -> usize {
        let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
        let digits: String = (bytes[at..].iter())
            .skip_while(|b| b.is_ascii_whitespace())
            .take_while(|b| b.is_ascii_digit())
            .map(|&b| b as char)
            .collect();
        digits.parse().unwrap()
    }

    fn find(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap()
    }

    /// The tables `checked` has, and how many of each.
    fn tables(checked: &[(&str, usize)]) -> BTreeMap<String, usize> {
        (checked.iter())
            .map(|&(table, n)| (table.to_string(), n))
            .collect()
    }

    /// A clean file: a catalog, its page tree, a page with a Text
    /// annotation, and the annotation's appearance stream with its
    /// resources.
    fn clean() -> Vec<(u32, Body)> {
        let form = dict! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => numbers(&[0, 0, 20, 20]),
            "Resources" => dict! { "ProcSet" => vec![Object::from("PDF")] },
        };
        vec![
            (1, dict! { "Type" => "Catalog", "Pages" => r(2) }.into()),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }.into(),
            ),
            (
                3,
                dict! {
                    "Type" => "Page",
                    "Parent" => r(2),
                    "MediaBox" => numbers(&[0, 0, 612, 792]),
                    "Resources" => dict! {},
                    "Annots" => vec![r(4)],
                }
                .into(),
            ),
            (
                4,
                dict! {
                    "Type" => "Annot",
                    "Subtype" => "Text",
                    "Rect" => numbers(&[100, 100, 120, 120]),
                    "Contents" => text("A note"),
                    "AP" => dict! { "N" => r(5) },
                }
                .into(),
            ),
            (5, Body::stream(form, b"0 0 m 20 20 l S")),
        ]
    }

    /// `objects` with object `n`'s dictionary as `f` changes it.
    fn changed(
        mut objects: Vec<(u32, Body)>,
        n: u32,
        f: impl FnOnce(&mut Dict),
    ) -> Vec<(u32, Body)> {
        match objects.iter_mut().find(|(m, _)| *m == n).map(|(_, b)| b) {
            Some(Body::Object(o)) => match o.value_mut() {
                Value::Dict(d) => f(d),
                _ => panic!("no dictionary {n}"),
            },
            Some(Body::Stream(d, _)) => f(d),
            _ => panic!("no dictionary {n}"),
        }
        objects
    }

    /// A finding in object `n` (built), as the tests expect one, not
    /// saying where its bytes are, which [`placeless`] leaves out.
    fn finding(n: u32, path: &str, (table, key): (&str, &str), rule: Rule, value: &str) -> Finding {
        Finding {
            object: Some((n, 0)),
            made: Made::Built,
            path: path.to_string(),
            table: table.to_string(),
            key: key.to_string(),
            rule,
            value: value.to_string(),
            at: None,
        }
    }

    /// The report's findings without where their bytes are, which the
    /// tests of the rules compare apart.
    fn placeless(report: &Report) -> Vec<Finding> {
        (report.findings.iter())
            .map(|f| Finding {
                at: None,
                ..f.clone()
            })
            .collect()
    }

    fn unlisted(table: &str, key: &str, class: Class) -> Unlisted {
        Unlisted {
            table: table.to_string(),
            key: key.to_string(),
            class,
        }
    }

    /// Each finding's object, table, key, rule and value.
    fn found(report: &Report) -> Vec<(Option<u32>, &str, &str, Rule, &str)> {
        (report.findings.iter())
            .map(|f| {
                let object = f.object.map(|(n, _)| n);
                (
                    object,
                    f.table.as_str(),
                    f.key.as_str(),
                    f.rule,
                    f.value.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn a_clean_file_reaches_its_tables_with_no_findings() {
        let common = [
            ("Catalog", 1),
            ("PageTreeNodeRoot", 1),
            ("ArrayOfPageTreeNodeKids", 1),
            ("PageObject", 1),
            ("Resource", 2),
            ("ArrayOfAnnots", 1),
            ("AnnotText", 1),
            ("Appearance", 1),
            ("XObjectFormType1", 1),
            ("ArrayOfNamesForProcSet", 1),
        ];
        // Each key and element: the trailer's two, the catalog's 2, the
        // page tree's 3 and its kid, the page's 5 and its annotation, the
        // annotation's 5 and its appearance, the form's 5, its resources' 1
        // and its ProcSet's 1. A cross-reference stream has 4 more keys than
        // a table's trailer, and its /W's 3 elements and /Index's 2.
        for (kind, trailer, keys) in [
            (Xref::Table, &[("FileTrailer", 1)][..], 27),
            (
                Xref::Stream,
                &[
                    ("XRefStream", 1),
                    ("ArrayOfXRefWIntegers", 1),
                    ("ArrayOfXRefIndexIntegers", 1),
                ][..],
                36,
            ),
        ] {
            let bytes = written("1.7", &clean(), kind);
            let report = checked_as(&bytes, &Hold::All(Made::Built));
            assert_eq!(report.findings, [], "{kind:?}:\n{report}");
            assert_eq!(report.unlisted, BTreeMap::new(), "{kind:?}:\n{report}");
            let mut expected = tables(&common);
            expected.extend(tables(trailer));
            assert_eq!(report.checked, expected, "{kind:?}:\n{report}");
            assert_eq!(report.keys, keys, "{kind:?}:\n{report}");
            let counts = (
                report.ties,
                report.no_table,
                report.truncated,
                report.two_contexts,
            );
            assert_eq!(counts, (0, 0, 0, 0), "{report}");
        }
    }

    #[test]
    fn a_value_of_a_type_its_row_doesnt_allow_is_found() {
        let bogus = changed(clean(), 4, |annot| annot.set("Rect", "Bogus"));
        assert_eq!(
            placeless(&checked(&bogus)),
            [finding(
                4,
                "Root/Pages/Kids[0]/Annots[0]",
                ("AnnotText", "Rect"),
                Rule::Type,
                "/Bogus"
            )]
        );

        // A rectangle is four numbers, and a date a date.
        let refined = changed(clean(), 4, |annot| annot.set("Rect", numbers(&[1, 2, 3])));
        let refined = changed(refined, 3, |page| {
            page.set("LastModified", text("D:20261324"))
        });
        let report = checked(&refined);
        assert_eq!(
            found(&report),
            [
                (
                    Some(3),
                    "PageObject",
                    "LastModified",
                    Rule::Type,
                    "(D:20261324)"
                ),
                (Some(4), "AnnotText", "Rect", Rule::Type, "[1 2 3]"),
            ],
            "{report}"
        );

        // A type its row allows only from a version later than the file's.
        let states = changed(clean(), 4, |annot| {
            annot.set("AP", dict! { "N" => dict! { "Off" => r(5) } });
            annot.set("AS", "Off");
        });
        let bytes = written("1.2", &states, Xref::Table);
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        let types: Vec<_> = (found(&report).into_iter())
            .filter(|f| f.3 == Rule::Type)
            .collect();
        assert_eq!(
            types,
            [(
                Some(4),
                "Appearance",
                "N",
                Rule::Type,
                "<< /Off 5 0 R >> (dictionary isn't allowed at 1.2)"
            )],
            "{report}"
        );
    }

    #[test]
    fn a_cross_reference_streams_keys_are_checked_as_the_trailer() {
        let mut bytes = written("1.7", &clean(), Xref::Stream);
        let start = startxref(&bytes);
        let dict = start + find(&bytes[start..], b"<<") + 2;
        bytes.splice(dict..dict, b" /Prev /Bogus".iter().copied());
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        let mut expected = finding(6, "trailer", ("XRefStream", "Prev"), Rule::Type, "/Bogus");
        expected.made = Made::Merged;
        assert_eq!(placeless(&report), [expected], "{report}");
    }

    #[test]
    fn a_trailer_that_doesnt_read_is_found_and_the_file_walked_as_read() {
        // `startxref` pointing past the file: the reader finds the objects
        // by scanning, and the trailer after the last `trailer`.
        let open_yes = changed(clean(), 4, |annot| annot.set("Open", text("yes")));
        let mut bytes = written("1.7", &open_yes, Xref::Table);
        let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
        let end = at + bytes[at..].iter().position(|b| b.is_ascii_digit()).unwrap();
        let digits = bytes[end..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        bytes.splice(end..end + digits, b"99999999".iter().copied());
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        let rules: Vec<_> = (report.findings.iter())
            .map(|f| (f.object, f.table.as_str(), f.key.as_str(), f.rule))
            .collect();
        assert_eq!(
            rules,
            [
                (None, "FileTrailer", "-", Rule::Unresolved),
                (Some((4, 0)), "AnnotText", "Open", Rule::Type),
            ],
            "{report}"
        );
        for table in ["FileTrailer", "Catalog", "PageObject", "AnnotText"] {
            assert_eq!(report.checked.get(table), Some(&1), "{table}: {report}");
        }

        // A cross-reference stream its own section doesn't list, past its
        // `/Size` and `/Index`, as some writers leave it: read at
        // `startxref` as written, and said so.
        let mut bytes = written("1.7", &open_yes, Xref::Stream);
        let start = startxref(&bytes);
        let dict = start..start + find(&bytes[start..], b">>");
        let text = String::from_utf8(bytes[dict.clone()].to_vec()).unwrap();
        let fewer = text
            .replace("/Size 7", "/Size 6")
            .replace("/Index [0 7]", "/Index [0 6]");
        assert_ne!(fewer, text);
        bytes.splice(
            dict,
            format!("{fewer:<width$}", width = text.len()).into_bytes(),
        );
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        let rules: Vec<_> = (report.findings.iter())
            .map(|f| (f.object, f.table.as_str(), f.key.as_str(), f.rule))
            .collect();
        assert_eq!(
            rules,
            [
                (Some((4, 0)), "AnnotText", "Open", Rule::Type),
                (Some((6, 0)), "-", "-", Rule::Reader),
            ],
            "{report}"
        );
        assert_eq!(report.findings[1].made, Made::Merged, "the trailer's");
        assert_eq!(report.trailer, Some((6, 0)), "{report}");
        assert_eq!(report.checked.get("XRefStream"), Some(&1), "{report}");
    }

    #[test]
    fn a_required_key_absent_is_found_unless_inherited() {
        let no_rect = changed(clean(), 4, |annot| {
            annot.remove(b"Rect");
        });
        assert_eq!(
            placeless(&checked(&no_rect)),
            [finding(
                4,
                "Root/Pages/Kids[0]/Annots[0]",
                ("AnnotText", "Rect"),
                Rule::Missing,
                "absent"
            )]
        );

        // A page's MediaBox is inheritable: its parent's will do.
        let no_box = changed(clean(), 3, |page| {
            page.remove(b"MediaBox");
        });
        assert_eq!(
            placeless(&checked(&no_box)),
            [finding(
                3,
                "Root/Pages/Kids[0]",
                ("PageObject", "MediaBox"),
                Rule::Missing,
                "absent"
            )]
        );
        let inherited = changed(no_box, 2, |pages| {
            pages.set("MediaBox", numbers(&[0, 0, 612, 792]))
        });
        let report = checked(&inherited);
        assert_eq!(report.findings, [], "{report}");

        // A text field's /DA is inheritable: its parent's will do, and so
        // will the form's, the document-wide default.
        let field = |form: Dict, parent: Dict| {
            let mut file = changed(clean(), 1, |catalog| {
                let mut form = form;
                form.set("Fields", vec![r(7)]);
                catalog.set("AcroForm", form);
            });
            let mut parent = parent;
            parent.set("FT", "Tx");
            parent.set("T", text("p"));
            parent.set("Kids", vec![r(6)]);
            file.extend([
                (
                    6,
                    dict! { "FT" => "Tx", "T" => text("t"), "Parent" => r(7) }.into(),
                ),
                (7, parent.into()),
            ]);
            checked(&file)
        };
        let report = field(dict! {}, dict! {});
        assert_eq!(
            found(&report),
            [
                (Some(7), "FieldTx", "DA", Rule::Missing, "absent"),
                (Some(6), "FieldTx", "DA", Rule::Missing, "absent"),
            ],
            "{report}"
        );
        let da = || dict! { "DA" => text("/Helv 0 Tf 0 g") };
        for report in [field(da(), dict! {}), field(dict! {}, da())] {
            assert_eq!(report.findings, [], "{report}");
        }

        // An array shorter than its table's required rows.
        let line = changed(clean(), 4, |annot| {
            annot.set("Subtype", "Line");
            annot.set("L", numbers(&[1, 2, 3]));
        });
        let report = checked(&line);
        assert_eq!(
            found(&report),
            [(Some(4), "ArrayOf_4Numbers", "3", Rule::Missing, "absent")],
            "{report}"
        );
    }

    #[test]
    fn an_object_is_checked_as_the_table_it_fits_best() {
        let square = changed(clean(), 4, |annot| annot.set("Subtype", "Square"));
        let report = checked(&square);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.checked.get("AnnotSquare"), Some(&1), "{report}");
        assert_eq!(report.checked.get("AnnotText"), None, "{report}");
        assert_eq!(report.ties, 0, "{report}");

        // Without its Subtype, a square's keys fit a circle's table as well:
        // the two have the same rows. The first listed is taken, and counted.
        let ambiguous = changed(clean(), 4, |annot| {
            annot.remove(b"Subtype");
            annot.set("IC", numbers(&[1, 0, 0]));
            annot.set("RD", numbers(&[1, 1, 1, 1]));
        });
        let report = checked(&ambiguous);
        assert_eq!(report.ties, 1, "{report}");
        assert_eq!(report.checked.get("AnnotSquare"), Some(&1), "{report}");

        // A button field is a push button, a checkbox or a radio button by
        // its /Ff's bits alone.
        for (flags, table) in [
            (1 << 16, "FieldBtnPush"),
            (0, "FieldBtnCheckbox"),
            (1 << 15, "FieldBtnRadio"),
        ] {
            let field = dict! { "FT" => "Btn", "T" => text("b"), "Ff" => flags };
            let mut form = changed(clean(), 1, |catalog| {
                catalog.set("AcroForm", dict! { "Fields" => vec![r(6)] })
            });
            form.push((6, field.into()));
            let report = checked(&form);
            assert_eq!(report.checked.get(table), Some(&1), "{report}");
            assert_eq!(report.ties, 0, "{report}");
        }

        // A structure parent tree's array is an array's table, even as long
        // as a structure element's table has rows (19). The page has marked
        // content of the tree's, so its entry in the parent tree.
        let tree = changed(clean(), 1, |catalog| catalog.set("StructTreeRoot", r(6)));
        let mut tree = changed(tree, 3, |page| page.set("StructParents", 0));
        tree.extend([
            (
                6,
                dict! {
                    "Type" => "StructTreeRoot",
                    "K" => r(7),
                    "ParentTree" => dict! { "Nums" => vec![Object::from(0), r(8)] },
                }
                .into(),
            ),
            (
                7,
                dict! {
                    "Type" => "StructElem",
                    "S" => "P",
                    "P" => r(6),
                    "Pg" => r(3),
                    "K" => 0,
                }
                .into(),
            ),
            (8, Object::from(vec![r(7); 19]).into()),
        ]);
        let report = checked(&tree);
        assert_eq!(report.findings, [], "{report}");
        let tables = ["ArrayOfStructElemParentTree", "StructElem"];
        for table in tables {
            assert_eq!(report.checked.get(table), Some(&1), "{table}: {report}");
        }
        assert_eq!(report.ties, 0, "{report}");

        // One dictionary reached as a page's resources and as a square's
        // border style keeps the first, counted.
        let shared = changed(square, 3, |page| page.set("Resources", r(6)));
        let mut shared = changed(shared, 4, |annot| annot.set("BS", r(6)));
        shared.push((6, dict! {}.into()));
        let report = checked(&shared);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.two_contexts, 1, "{report}");
        assert_eq!(report.checked.get("BorderStyle"), None, "{report}");
    }

    #[test]
    fn an_object_a_universal_table_reaches_first_is_checked_as_another_that_reaches_it() {
        // The catalog's structure tree comes before its pages, and its
        // element marks the annotation (`/OBJR`), whose `/Obj` the model
        // links to `_UniversalDictionary` alone: the walk reaches the
        // annotation there first.
        let mut tree = clean();
        tree[0].1 = dict! {
            "Type" => "Catalog", "StructTreeRoot" => r(6), "Pages" => r(2),
        }
        .into();
        tree.extend([
            (6, dict! { "Type" => "StructTreeRoot", "K" => r(7) }.into()),
            (
                7,
                dict! {
                    "Type" => "StructElem", "S" => "Annot", "P" => r(6), "Pg" => r(3),
                    "K" => dict! { "Type" => "OBJR", "Obj" => r(4), "Pg" => r(3) },
                }
                .into(),
            ),
        ]);
        let tree = changed(tree, 4, |annot| annot.set("Open", text("yes")));
        let report = checked(&tree);
        assert_eq!(report.checked.get("ObjectReference"), Some(&1), "{report}");
        assert_eq!(report.checked.get("AnnotText"), Some(&1), "{report}");
        assert_eq!(report.checked.get("_UniversalDictionary"), None, "{report}");
        assert_eq!(
            found(&report),
            [(Some(4), "AnnotText", "Open", Rule::Type, "(yes)")],
            "checked as the annotation it is: {report}"
        );

        // Reached as nothing else, it's checked as that table, last.
        let alone = changed(tree, 3, |page| {
            page.remove(b"Annots");
        });
        let report = checked(&alone);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.checked.get("AnnotText"), None, "{report}");
        assert_eq!(
            report.checked.get("_UniversalDictionary"),
            Some(&2),
            "{report}"
        );
    }

    /// `objects` written at 1.7 with a table and checked as one table an
    /// object, every object held as built.
    fn checked_one_table(objects: &[(u32, Body)]) -> Report {
        let bytes = written("1.7", objects, Xref::Table);
        let options = Options {
            one_table: true,
            ..Options::default()
        };
        check(&bytes::read(&bytes), &Hold::All(Made::Built), &options)
    }

    /// [`clean`] with an interactive form whose fields are objects 6 on,
    /// `roots` its top ones, and `widgets` on the page too; and the
    /// form's `/DA` when `da`.
    fn form(fields: Vec<Dict>, roots: &[u32], widgets: &[u32], da: bool) -> Vec<(u32, Body)> {
        let objects = changed(clean(), 1, |catalog| {
            let mut form = dict! { "Fields" => roots.iter().map(|&n| r(n)).collect::<Vec<_>>() };
            if da {
                form.set("DA", text("/Helv 0 Tf 0 g"));
            }
            catalog.set("AcroForm", form);
        });
        let mut objects = changed(objects, 3, |page| {
            let annots = [r(4)].into_iter().chain(widgets.iter().map(|&n| r(n)));
            page.set("Annots", annots.collect::<Vec<_>>());
        });
        objects.extend((6..).zip(fields).map(|(n, field)| (n, field.into())));
        objects
    }

    /// A widget's keys, on the page, with `more`.
    fn widget(more: Dict) -> Dict {
        let mut widget = dict! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => numbers(&[10, 10, 110, 30]),
            "P" => r(3),
        };
        for (key, value) in more.iter() {
            widget.set(key, value.clone());
        }
        widget
    }

    fn script(js: &str) -> Object {
        dict! { "S" => "JavaScript", "JS" => text(js) }.into()
    }

    #[test]
    fn a_field_and_its_widget_merged_are_checked_as_both() {
        // A text field with its one widget, one dictionary (ISO 32000-2
        // 12.5.6.19), and its triggers a widget's (/E) and a field's (/K).
        let merged = widget(dict! {
            "FT" => "Tx",
            "T" => text("name"),
            "DA" => text("/Helv 12 Tf 0 g"),
            "V" => text("Ann"),
            "MaxLen" => -1,
            "AA" => dict! { "E" => script("enter()"), "K" => script("key()") },
        });
        let objects = form(vec![merged.clone()], &[6], &[6], false);
        let report = checked(&objects);
        assert_eq!(
            found(&report),
            [(
                Some(6),
                "FieldTx",
                "MaxLen",
                Rule::Special,
                "-1 (fn:Eval(@MaxLen>=0))"
            )],
            "{report}"
        );
        assert_eq!(report.unlisted, BTreeMap::new(), "{report}");
        for table in [
            "AnnotWidget",
            "FieldTx",
            "AddActionWidgetAnnotation",
            "AddActionFormField",
        ] {
            assert_eq!(report.checked.get(table), Some(&1), "{table}: {report}");
        }
        assert_eq!(report.checked.get("ActionECMAScript"), Some(&2), "{report}");
        assert_eq!((report.two_tables, report.two_contexts), (2, 0), "{report}");
        let objects_checked = report.objects();

        // As one table, as TestGrammar checks it, it's the widget alone: the
        // field's keys and triggers are unlisted, and its rules unchecked.
        let report = checked_one_table(&objects);
        let unlisted: Vec<(&str, &str)> = (report.findings.iter())
            .map(|f| {
                assert_eq!(f.rule, Rule::Unlisted, "{report}");
                (f.table.as_str(), f.key.as_str())
            })
            .collect();
        assert_eq!(
            unlisted,
            [
                ("AnnotWidget", "FT"),
                ("AnnotWidget", "DA"),
                ("AnnotWidget", "V"),
                ("AnnotWidget", "MaxLen"),
                ("AddActionWidgetAnnotation", "K"),
            ],
            "{report}"
        );
        assert_eq!(report.checked.get("FieldTx"), None, "{report}");
        assert_eq!(report.two_tables, 0, "{report}");
        // The same objects, each once, but the script the field's trigger
        // has.
        assert_eq!(report.objects() + 1, objects_checked, "{report}");

        // A rule both tables have, broken, is found once, as the first's.
        let mut name = merged.clone();
        name.set("T", 5);
        name.remove(b"MaxLen");
        let report = checked(&form(vec![name], &[6], &[6], false));
        assert_eq!(
            found(&report),
            [(Some(6), "AnnotWidget", "T", Rule::Type, "5")],
            "{report}"
        );

        // Each table's required keys: a text field's /DA, absent here and
        // from the form, as a field's.
        let mut plain = merged.clone();
        plain.remove(b"DA");
        plain.remove(b"MaxLen");
        let report = checked(&form(vec![plain.clone()], &[6], &[6], false));
        assert_eq!(
            found(&report),
            [(Some(6), "FieldTx", "DA", Rule::Missing, "absent")],
            "{report}"
        );
        assert_eq!(checked(&form(vec![plain], &[6], &[6], true)).findings, []);
    }

    #[test]
    fn a_merged_fields_table_is_its_type_and_flags_own_or_inherited() {
        // A radio button group, and a button of it with its widget: /FT and
        // /Ff from the group.
        let group = dict! {
            "FT" => "Btn",
            "Ff" => 1 << 15,
            "T" => text("group"),
            "Kids" => vec![r(7)],
        };
        let button = widget(dict! { "Parent" => r(6), "T" => text("a") });
        let report = checked(&form(vec![group.clone(), button.clone()], &[6], &[7], true));
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.checked.get("FieldBtnRadio"), Some(&2), "{report}");
        assert_eq!(report.checked.get("AnnotWidget"), Some(&1), "{report}");
        assert_eq!(report.two_tables, 1, "{report}");

        // A widget with neither /T nor a key only fields have is one of its
        // field's widgets, and only that (12.7.4.2).
        let mut only = button;
        only.remove(b"T");
        let report = checked(&form(vec![group, only], &[6], &[7], true));
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.checked.get("FieldBtnRadio"), Some(&1), "{report}");
        assert_eq!(report.two_tables, 0, "{report}");

        // A field reached as a field's table first, here as a widget's
        // /Parent, and a widget by its /Subtype, is checked as the widget
        // too.
        let field = widget(dict! {
            "FT" => "Tx",
            "T" => text("text"),
            "DA" => text("/Helv 0 Tf 0 g"),
            "Kids" => vec![r(7)],
        });
        let kid = widget(dict! { "Parent" => r(6) });
        let report = checked(&form(vec![field, kid], &[], &[7], false));
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.checked.get("FieldTx"), Some(&1), "{report}");
        assert_eq!(report.checked.get("AnnotWidget"), Some(&2), "{report}");
        assert_eq!(report.two_tables, 1, "{report}");
    }

    #[test]
    fn a_hide_action_names_a_field_by_its_fully_qualified_name() {
        // The embedded model's correction: a hide action's /T is a field's
        // fully qualified name, `group.box` here, where the model asks a
        // partial one.
        let group = dict! { "T" => text("group"), "Kids" => vec![r(7)] };
        let hide = |name: &str| dict! { "S" => "Hide", "T" => text(name) };
        let field = |name: &str| {
            widget(dict! {
                "Parent" => r(6),
                "T" => text("box"),
                "FT" => "Btn",
                "A" => hide(name),
            })
        };
        let objects = form(vec![group.clone(), field("group.box")], &[6], &[7], true);
        assert_eq!(checked(&objects).findings, []);
        let objects = form(vec![group, field("box")], &[6], &[7], true);
        assert_eq!(
            found(&checked(&objects)),
            [(
                Some(7),
                "ActionHide",
                "T",
                Rule::Special,
                "(box) (fn:Eval(fn:IsFullyQualifiedFieldName(@T)))"
            )]
        );
    }

    #[test]
    fn the_embedded_models_additions_list_a_fields_dr_and_a_forms_encodings() {
        // A text field with its own /DR, as PDF 1.4 defined, obsolete from
        // 1.5; and the form's /DR with an /Encoding resource of named
        // encodings, as Acrobat writes one.
        let field = widget(dict! {
            "FT" => "Tx",
            "T" => text("name"),
            "DR" => dict! { "Font" => dict! { "Helv" => r(7) } },
        });
        let objects = form(vec![field], &[6], &[6], true);
        let mut objects = changed(objects, 1, |catalog| {
            let mut form = catalog.get(b"AcroForm").unwrap().as_dict().unwrap().clone();
            let encodings = dict! { "PDFDocEncoding" => dict! {
                "Type" => "Encoding",
                "Differences" => vec![Object::from(24), Object::from("breve")],
            } };
            form.set(
                "DR",
                dict! { "Font" => dict! { "Helv" => r(7) }, "Encoding" => encodings },
            );
            catalog.set("AcroForm", form);
        });
        objects.push((
            7,
            dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" }.into(),
        ));
        let at_version = |version: &str| {
            let bytes = written(version, &objects, Xref::Table);
            checked_as(&bytes, &Hold::All(Made::Built))
        };
        let report = at_version("1.4");
        assert_eq!(report.unlisted, BTreeMap::new(), "{report}");
        assert_eq!(report.checked.get("EncodingMap"), Some(&1), "{report}");
        // The resources' /Encoding itself is deprecated from 1.2, as the
        // model has it; and a field's /DR from 1.5.
        let at = |table: &str, key: &str| (table.to_string(), key.to_string());
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(
            report.deprecated,
            BTreeMap::from([(at("Resource", "Encoding"), 1)]),
            "{report}"
        );
        let report = at_version("1.6");
        assert_eq!(
            report.deprecated.get(&at("FieldTx", "DR")),
            Some(&1),
            "{report}"
        );
    }

    #[test]
    fn a_key_another_table_reaching_its_object_lists_isnt_unlisted() {
        // One dictionary is a page's resources, reached first, and a
        // square's border style, which lists its /W.
        let square = changed(clean(), 4, |annot| annot.set("Subtype", "Square"));
        let shared = changed(square, 3, |page| page.set("Resources", r(6)));
        let mut shared = changed(shared, 4, |annot| annot.set("BS", r(6)));
        shared.push((6, dict! { "W" => 1 }.into()));
        let report = checked(&shared);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.unlisted, BTreeMap::new(), "{report}");
        assert_eq!(report.two_contexts, 1, "{report}");
        assert_eq!(report.checked.get("BorderStyle"), None, "{report}");

        let report = checked_one_table(&shared);
        assert_eq!(
            found(&report),
            [(Some(6), "Resource", "W", Rule::Unlisted, "1")],
            "{report}"
        );
        assert_eq!(
            report.unlisted,
            BTreeMap::from([(unlisted("Resource", "W", Class::Other), 1)]),
            "{report}"
        );
    }

    #[test]
    fn only_held_objects_are_found_in_but_all_are_walked() {
        let no_box = changed(clean(), 3, |page| {
            page.remove(b"MediaBox");
        });
        let bytes = written("1.7", &no_box, Xref::Table);
        let all = checked_as(&bytes, &Hold::All(Made::Built));
        assert_eq!(all.findings.len(), 1, "{all}");

        let annotation = BTreeMap::from([((4, 0), Made::Merged), ((5, 0), Made::Built)]);
        let report = checked_as(&bytes, &Hold::Only(annotation));
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(
            report.checked,
            tables(&[
                ("FileTrailer", 1),
                ("AnnotText", 1),
                ("Appearance", 1),
                ("XObjectFormType1", 1),
                ("Resource", 1),
                ("ArrayOfNamesForProcSet", 1),
            ]),
            "the page isn't held, and its annotation is still an annotation"
        );

        let bogus = changed(no_box, 4, |annot| annot.set("Rect", "Bogus"));
        let bytes = written("1.7", &bogus, Xref::Table);
        let report = checked_as(
            &bytes,
            &Hold::Only(BTreeMap::from([((4, 0), Made::Merged)])),
        );
        let held: Vec<_> = (report.findings.iter())
            .map(|f| (f.object, f.made, f.rule))
            .collect();
        assert_eq!(held, [(Some((4, 0)), Made::Merged, Rule::Type)], "{report}");
    }

    #[test]
    fn a_held_object_nothing_reaches_is_found() {
        let mut objects = changed(clean(), 1, |catalog| catalog.set("XXGenerators", r(7)));
        objects.extend([
            (
                6,
                dict! { "Type" => "Annot", "Subtype" => "Text", "Popup" => r(8) }.into(),
            ),
            (7, dict! { "Mine" => 1 }.into()),
            (8, dict! { "Subtype" => "Popup" }.into()),
        ]);
        let report = checked(&objects);
        let unreached = |n: u32, key: &str| (n, "-", "-", Rule::Unreached, key.to_string());
        let seen: Vec<_> = (report.findings.iter())
            .map(|f| {
                let n = f.object.unwrap().0;
                (n, f.path.as_str(), f.table.as_str(), f.rule, f.key.clone())
            })
            .collect();
        assert_eq!(
            seen,
            [unreached(6, "-"), unreached(8, "Popup")],
            "an object under a key the model doesn't list isn't one: {report}"
        );
        assert_eq!(
            report.unlisted,
            BTreeMap::from([(unlisted("Catalog", "XXGenerators", Class::Third), 1)])
        );

        // A reference to no object is found where it's written.
        let dangling = changed(clean(), 3, |page| page.set("Annots", vec![r(4), r(9)]));
        assert_eq!(
            placeless(&checked(&dangling)),
            [finding(
                3,
                "Root/Pages/Kids[0]/Annots",
                ("ArrayOfAnnots", "1"),
                Rule::Unresolved,
                "9 0 R"
            )]
        );
    }

    #[test]
    fn name_and_number_trees_are_walked_and_a_malformed_node_is_found() {
        let dest = || Object::from(vec![r(3), Object::from("Fit")]);
        let leaf = dict! {
            "Names" => vec![text("a"), dest(), text("b"), dest()],
            "Limits" => vec![text("a"), text("b")],
        };
        let labels = dict! { "Nums" => vec![Object::from(0), dict! { "S" => "D" }.into()] };
        let mut objects = changed(clean(), 1, |catalog| {
            catalog.set("Names", dict! { "Dests" => dict! { "Kids" => vec![r(6)] } });
            catalog.set("PageLabels", labels);
        });
        objects.push((6, leaf.into()));
        let report = checked(&objects);
        assert_eq!(report.findings, [], "{report}");
        for (table, n) in [
            ("name-tree", 2),
            ("number-tree", 1),
            ("Dest0Array", 2),
            ("PageLabel", 1),
        ] {
            assert_eq!(report.checked.get(table), Some(&n), "{table}: {report}");
        }

        let broken = changed(objects, 6, |leaf| {
            leaf.remove(b"Limits");
            leaf.set("Names", vec![text("a"), dest(), Object::from(3)]);
        });
        let report = checked(&broken);
        assert_eq!(
            found(&report),
            [
                (
                    Some(6),
                    "Name",
                    "Dests",
                    Rule::Tree,
                    "a node under the root without /Limits"
                ),
                (
                    Some(6),
                    "Name",
                    "Dests",
                    Rule::Tree,
                    "its last key, [2], has no value"
                ),
            ],
            "{report}"
        );
    }

    #[test]
    fn every_walk_ends_on_cycles_and_deep_nesting() {
        // A /Parent cycle, up which the inheritable MediaBox is looked for.
        let cycle = changed(clean(), 3, |page| {
            page.remove(b"MediaBox");
        });
        let cycle = changed(cycle, 2, |pages| pages.set("Parent", r(3)));
        let report = checked(&cycle);
        let missing: Vec<_> = (found(&report).into_iter())
            .filter(|f| f.3 == Rule::Missing)
            .map(|f| (f.1, f.2))
            .collect();
        assert_eq!(missing, [("PageObject", "MediaBox")], "{report}");

        // An array that holds itself, and one nested past the cap, as a page
        // piece's private data.
        let mut deep = Object::from(Vec::new());
        for _ in 0..NESTING + 6 {
            deep = vec![deep].into();
        }
        let piece = dict! { "Writer" => dict! {
            "LastModified" => text("D:20261024"),
            "Private" => r(6),
        } };
        let mut objects = changed(clean(), 3, |page| {
            page.set("LastModified", text("D:20261024"));
            page.set("PieceInfo", piece);
        });
        objects.push((6, Object::from(vec![r(6), deep]).into()));
        let report = checked(&objects);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.truncated, 1, "{report}");
        assert_eq!(
            report.checked.get("_UniversalArray"),
            Some(&(NESTING + 1)),
            "{report}"
        );
    }

    /// An outline of 50,000 items, each reached from the one before by its
    /// `/Next`: the walk's way to the last is 50,000 steps long, and it's
    /// dropped without recursion, on a thread with an eighth of a test
    /// thread's stack, which a recursive drop of that many steps overflows.
    #[test]
    fn a_way_as_long_as_the_file_makes_it_drops_on_a_small_stack() {
        const ITEMS: u32 = 50_000;
        const STACK: usize = 256 << 10;
        let first = 7;
        let last = first + ITEMS - 1;
        let mut objects = changed(clean(), 1, |catalog| catalog.set("Outlines", r(6)));
        objects.push((
            6,
            dict! {
                "Type" => "Outlines", "First" => r(first), "Last" => r(last),
                "Count" => i64::from(ITEMS),
            }
            .into(),
        ));
        for n in first..=last {
            let mut item = dict! { "Title" => text("An item"), "Parent" => r(6) };
            if n > first {
                item.set("Prev", r(n - 1));
            }
            if n < last {
                item.set("Next", r(n + 1));
            }
            objects.push((n, item.into()));
        }
        let bytes = written("1.7", &objects, Xref::Table);
        let report = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || checked_as(&bytes, &Hold::All(Made::Built)))
            .unwrap()
            .join()
            .expect("the walk ends");
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(
            report.checked.get("OutlineItem"),
            Some(&(ITEMS as usize)),
            "{report}"
        );
    }

    #[test]
    fn a_date_reads_as_test_grammar_reads_one() {
        for (date, valid) in [
            ("D:2026", true),
            ("D:20261024", true),
            ("D:20261024153000Z", true),
            ("D:20261024153000+05'30'", true),
            ("D:20261024153000-08'00", true),
            ("D:2026102415", true),
            ("D:202610241530007", true),
            ("D:20261324", false),
            ("D:20261000", false),
            ("D:20261024246000", false),
            ("D:20261024153000+24'00'", false),
            ("D:20261024153000+05'60'", false),
            ("D:2026102415300099", false),
            ("D:26", false),
            ("20261024", false),
        ] {
            assert_eq!(is_date(date), valid, "{date}");
        }
    }

    #[test]
    fn a_structure_trees_arrays_are_read_as_test_grammar_reads_them() {
        let structure = |attributes: Vec<Object>| {
            let mut objects = changed(clean(), 1, |catalog| catalog.set("StructTreeRoot", r(6)));
            let reference = dict! { "Type" => "OBJR", "Obj" => r(4) };
            objects.extend([
                (6, dict! { "Type" => "StructTreeRoot", "K" => r(7) }.into()),
                (
                    7,
                    dict! {
                        "Type" => "StructElem",
                        "S" => "Form",
                        "P" => r(6),
                        "Pg" => r(3),
                        "A" => attributes,
                        "K" => vec![Object::from(reference)],
                    }
                    .into(),
                ),
            ]);
            objects
        };
        // An attribute object's revision number may be left out: an element
        // that isn't one is the next attribute object.
        let table = || Object::from(dict! { "O" => "Table" });
        for attributes in [
            vec![table(), table()],
            vec![table(), Object::from(3), table()],
        ] {
            let report = checked(&structure(attributes));
            assert_eq!(report.findings, [], "{report}");
        }

        // An object reference in a /K array needs its own /Pg only when
        // its structure element has none: `parent::Pg` goes past the array
        // to the element.
        let report = checked(&structure(vec![table()]));
        assert!(
            !(report.unchecked.keys()).any(|u| u.key == "Pg"),
            "{report}"
        );
        let report = checked(&changed(structure(vec![table()]), 7, |element| {
            element.remove(b"Pg");
        }));
        assert_eq!(
            found(&report),
            [(Some(7), "ObjectReference", "Pg", Rule::Missing, "absent")],
            "{report}"
        );
    }

    /// A page the structure tree puts marked content on, and an image it
    /// points at, are content items, and need their entries in the
    /// parent tree.
    #[test]
    fn content_items_need_their_parent_tree_entries() {
        let tagged = |entries: bool| {
            let mut image = dict! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            };
            let objects = changed(clean(), 1, |catalog| catalog.set("StructTreeRoot", r(6)));
            let mut objects = changed(objects, 3, |page| {
                page.set("Resources", dict! { "XObject" => dict! { "Im" => r(8) } });
                if entries {
                    page.set("StructParents", 0);
                }
            });
            if entries {
                image.set("StructParent", 1);
            }
            let reference = dict! { "Type" => "OBJR", "Obj" => r(8) };
            objects.extend([
                (6, dict! { "Type" => "StructTreeRoot", "K" => r(7) }.into()),
                (
                    7,
                    dict! {
                        "Type" => "StructElem",
                        "S" => "Figure",
                        "P" => r(6),
                        "Pg" => r(3),
                        "K" => vec![Object::from(0), Object::from(reference)],
                    }
                    .into(),
                ),
                (8, Body::stream(image, &[0])),
            ]);
            let report = checked(&objects);
            (found(&report).into_iter())
                .filter(|f| matches!(f.2, "StructParents" | "StructParent"))
                .map(|f| (f.0, f.1.to_string(), f.3))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            tagged(false),
            [
                (Some(3), "PageObject".to_string(), Rule::Missing),
                (Some(8), "XObjectImage".to_string(), Rule::Missing)
            ]
        );
        assert_eq!(tagged(true), []);
    }

    /// A page written directly in the page tree has no number of its own:
    /// a content item on the node it's in isn't on it.
    #[test]
    fn a_page_written_directly_isnt_the_page_tree_node_its_in() {
        let mut objects = clean();
        let at = objects.iter().position(|(n, _)| *n == 3).unwrap();
        let Body::Object(page) = objects.remove(at).1 else {
            panic!("page 3 is a dictionary");
        };
        let objects = changed(objects, 2, |pages| pages.set("Kids", vec![page]));
        let mut objects = changed(objects, 1, |catalog| catalog.set("StructTreeRoot", r(6)));
        let element =
            dict! { "Type" => "StructElem", "S" => "P", "P" => r(6), "Pg" => r(2), "K" => 0 };
        objects.extend([
            (6, dict! { "Type" => "StructTreeRoot", "K" => r(7) }.into()),
            (7, element.into()),
        ]);
        let report = checked(&objects);
        assert_eq!(report.checked.get("PageObject"), Some(&1), "{report}");
        assert!(
            !(found(&report).iter()).any(|f| f.2 == "StructParents"),
            "{report}"
        );
        assert!(
            !(report.unchecked.keys()).any(|u| u.key == "StructParents"),
            "{report}"
        );
    }

    /// Two objects an object stream's header starts at one offset, page 3
    /// in the page tree and 4, which a structure element's `/Pg` names:
    /// read from the same bytes, they have one span, and their numbers
    /// tell them apart, so only 4 needs a `/StructParents`.
    #[test]
    fn objects_an_object_stream_starts_at_one_offset_are_told_apart_by_their_numbers() {
        let page = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>";
        let data = [b"3 0 4 0 ".as_slice(), page].concat();
        let head = format!(
            "<< /Type /ObjStm /N 2 /First 8 /Length {} >>\nstream\n",
            data.len()
        );
        let objects: [(u32, Vec<u8>); 5] = [
            (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R >>".to_vec(),
            ),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (5, b"<< /Type /StructTreeRoot /K 6 0 R >>".to_vec()),
            (
                6,
                b"<< /Type /StructElem /S /P /P 5 0 R /Pg 4 0 R /K 0 >>".to_vec(),
            ),
            (10, [head.as_bytes(), &data, b"\nendstream"].concat()),
        ];
        let mut file = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        // Each object's cross-reference entry: its type, and two fields.
        let mut rows = [(0u8, 0usize, 0u16); 12];
        rows[0] = (0, 0, 65535);
        for (n, body) in &objects {
            rows[*n as usize] = (1, file.len(), 0);
            file.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
            file.extend_from_slice(body);
            file.extend_from_slice(b"\nendobj\n");
        }
        (rows[3], rows[4]) = ((2, 10, 0), (2, 10, 1));
        let xref = file.len();
        rows[11] = (1, xref, 0);
        let rows: Vec<u8> = (rows.iter())
            .flat_map(|&(kind, field, index)| {
                let field = u32::try_from(field).unwrap().to_be_bytes();
                [&[kind][..], &field, &index.to_be_bytes()].concat()
            })
            .collect();
        let dict = format!(
            "11 0 obj\n<< /Type /XRef /Size 12 /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n",
            rows.len()
        );
        file.extend_from_slice(dict.as_bytes());
        file.extend_from_slice(&rows);
        file.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        let doc = bytes::read(&file);
        assert_eq!(doc.get((3, 0)).unwrap().at, doc.get((4, 0)).unwrap().at);
        let report = check(&doc, &Hold::All(Made::Built), &Options::default());
        assert_eq!(report.checked.get("PageObject"), Some(&2), "{report}");
        let parents: Vec<_> = (found(&report).into_iter())
            .filter(|f| f.2 == "StructParents")
            .map(|f| (f.0, f.1, f.3))
            .collect();
        assert_eq!(
            parents,
            [(Some(4), "PageObject", Rule::Missing)],
            "{report}"
        );
    }

    /// A reader that doesn't say where its objects were written: the
    /// structure tree's content items, and the pages and images checked,
    /// are known by their numbers.
    #[test]
    fn content_items_are_known_by_their_numbers_where_nothing_is_placed() {
        let objects = changed(clean(), 1, |catalog| catalog.set("StructTreeRoot", r(6)));
        let mut objects = changed(objects, 3, |page| {
            page.set("Resources", dict! { "XObject" => dict! { "Im" => r(8) } });
        });
        let image = dict! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 1,
            "Height" => 1,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
        };
        let reference = dict! { "Type" => "OBJR", "Obj" => r(8) };
        objects.extend([
            (6, dict! { "Type" => "StructTreeRoot", "K" => r(7) }.into()),
            (
                7,
                dict! {
                    "Type" => "StructElem",
                    "S" => "Figure",
                    "P" => r(6),
                    "Pg" => r(3),
                    "K" => vec![Object::from(0), Object::from(reference)],
                }
                .into(),
            ),
            (8, Body::stream(image, &[0])),
        ]);
        let mut doc = bytes::read(&written("1.7", &objects, Xref::Table));
        let content = |doc: &Document| {
            let report = check(doc, &Hold::All(Made::Built), &Options::default());
            let found = (found(&report).into_iter())
                .filter(|f| matches!(f.2, "StructParents" | "StructParent"))
                .map(|f| (f.0, f.1.to_string(), f.3))
                .collect::<Vec<_>>();
            let unchecked = (report.unchecked.keys())
                .filter(|u| u.column == "Required")
                .count();
            (found, unchecked)
        };
        let placed = content(&doc);
        for object in doc.objects.values_mut().flatten() {
            object.at = None;
        }
        assert_eq!(content(&doc), placed);
        assert_eq!(
            placed.0,
            [
                (Some(3), "PageObject".to_string(), Rule::Missing),
                (Some(8), "XObjectImage".to_string(), Rule::Missing)
            ]
        );
    }

    /// A SpecialCase's or PossibleValues' `@Key` the object lacks takes the
    /// key's DefaultValue; Required's doesn't, and what it asks of a key
    /// that's absent stays unknown.
    #[test]
    fn special_cases_and_possible_values_take_an_absent_keys_default() {
        let image = dict! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 1,
            "Height" => 1,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
        };
        let objects = changed(clean(), 5, |form| {
            form.set("Resources", dict! { "XObject" => dict! { "Im1" => r(6) } });
        });
        let objects = changed(objects, 1, |catalog| {
            catalog.set("Collection", dict! { "Type" => "Collection" });
            catalog.set(
                "OCProperties",
                dict! { "OCGs" => vec![r(7)], "D" => dict! {} },
            );
        });
        let mut objects = objects;
        objects.push((6, Body::stream(image, b"\x00")));
        let zoom = dict! { "Zoom" => dict! { "min" => 1.0 } };
        let group = dict! { "Type" => "OCG", "Name" => text("L"), "Usage" => zoom };
        objects.push((7, group.into()));
        let report = checked_as(
            &written("2.0", &objects, Xref::Table),
            &Hold::All(Made::Built),
        );
        let unchecked = |column, why: &str, table: &str, key: &str| Unchecked {
            column,
            why: why.to_string(),
            table: table.to_string(),
            key: key.to_string(),
        };
        // The image's /ImageMask is false, so it may have a /ColorSpace.
        let mask = unchecked("SpecialCase", "@ImageMask", "XObjectImage", "ColorSpace");
        assert_eq!(report.unchecked.get(&mask), None, "{report}");
        assert!(
            !(report.findings.iter()).any(|f| f.table == "XObjectImage"),
            "{report}"
        );
        // Whether /Navigator is required, when /View is C: the default D
        // doesn't say.
        let navigator = unchecked("Required", "@View", "Collection", "Navigator");
        assert_eq!(report.unchecked.get(&navigator), Some(&1), "{report}");
        // Whether /min is at most /max: the default 999999.9 says.
        let min = unchecked("PossibleValues", "@max", "OptContentZoom", "min");
        assert_eq!(report.unchecked.get(&min), None, "{report}");
        let zoomed = |report: &Report| {
            (report.findings.iter())
                .filter(|f| f.table == "OptContentZoom")
                .map(|f| (f.key.clone(), f.rule))
                .collect::<Vec<_>>()
        };
        assert_eq!(zoomed(&report), []);
        let past = changed(objects.clone(), 7, |group| {
            group.set("Usage", dict! { "Zoom" => dict! { "min" => 1_000_000.0 } });
        });
        let report = checked_as(&written("2.0", &past, Xref::Table), &Hold::All(Made::Built));
        assert_eq!(
            zoomed(&report),
            [("min".to_string(), Rule::Value)],
            "{report}"
        );
    }

    /// What the reader knows of the file reaches the predicates: its
    /// length, and which strings were written as hex strings.
    #[test]
    fn the_file_size_and_hex_strings_reach_the_predicates() {
        // A /Prev past the file's end.
        let past = dict! { "Prev" => 999_999 };
        let report = checked_as(
            &written_with("1.7", &clean(), Xref::Table, past),
            &Hold::All(Made::Built),
        );
        assert!(
            (report.findings.iter()).any(|f| (f.table.as_str(), f.key.as_str(), f.rule)
                == ("FileTrailer", "Prev", Rule::Value)),
            "{report}"
        );
        // A signature's /Contents, which must be a hex string.
        let signed = |hex: bool| {
            let signature = dict! {
                "Type" => "Sig",
                "Filter" => "Adobe.PPKLite",
                "ByteRange" => numbers(&[0, 1, 2, 3]),
                "Contents" => text("xxxx"),
            };
            let objects = changed(clean(), 1, |catalog| {
                catalog.set("Perms", dict! { "DocMDP" => r(6) });
            });
            let mut objects = objects;
            objects.push((6, signature.into()));
            let mut bytes = written("1.7", &objects, Xref::Table);
            if hex {
                // As long, so that every offset stays as it was.
                let at = (bytes.windows(6)).position(|w| w == b"(xxxx)").unwrap();
                bytes[at..at + 6].copy_from_slice(b"<abcd>");
            }
            let report = checked_as(&bytes, &Hold::All(Made::Built));
            (report.findings.iter()).any(|f| {
                (f.table.as_str(), f.key.as_str(), f.rule)
                    == ("Signature", "Contents", Rule::Special)
            })
        };
        assert!(signed(false), "a literal string");
        assert!(!signed(true), "a hex string");
    }

    /// `value` as PDF syntax.
    fn syntax(value: &Object) -> Vec<u8> {
        let mut out = Vec::new();
        write::value(&mut out, value);
        out
    }

    /// `clean()`'s appearance stream written with `/Length 6 0 R`, object 6
    /// `length`, and `more` beside, at 1.7 with a table.
    fn length_referenced(length: Object, more: &[(u32, Object)]) -> Vec<u8> {
        let raw = b"<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
            /Resources << >> /Length 6 0 R >>\nstream\n0 0 m 20 20 l S\nendstream"
            .to_vec();
        let mut bodies: Vec<(u32, Vec<u8>)> = (clean().into_iter())
            .filter_map(|(n, body)| match body {
                Body::Object(o) => Some((n, syntax(&o))),
                Body::Stream(..) => None,
            })
            .collect();
        bodies.push((5, raw));
        bodies.push((6, syntax(&length)));
        bodies.extend(more.iter().map(|(n, o)| (*n, syntax(o))));
        bodies.sort_by_key(|(n, _)| *n);
        let size = bodies.last().map_or(1, |(n, _)| n + 1);
        let bodies: Vec<(u32, &[u8])> = bodies.iter().map(|(n, b)| (*n, b.as_slice())).collect();
        write::assembled("1.7", &bodies, &format!("<< /Size {size} /Root 1 0 R >>"))
    }

    #[test]
    fn a_streams_referenced_length_is_reached() {
        let bytes = length_referenced(Object::from(15), &[]);
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.checked.get("XObjectFormType1"), Some(&1), "{report}");

        // A length that isn't an integer isn't one, whether or not another
        // integer is left unreached.
        let unreached = |report: &Report| {
            (report.findings.iter())
                .filter(|f| f.rule == Rule::Unreached)
                .map(|f| f.object.unwrap().0)
                .collect::<Vec<_>>()
        };
        let real = length_referenced(Object::from(15.0), &[]);
        let report = checked_as(&real, &Hold::All(Made::Built));
        assert_eq!(unreached(&report), [6], "{report}");
        let and_an_integer = length_referenced(Object::from(15.0), &[(7, Object::from(3))]);
        let report = checked_as(&and_an_integer, &Hold::All(Made::Built));
        assert_eq!(unreached(&report), [6, 7], "{report}");
    }

    #[test]
    fn a_file_is_checked_under_the_extensions_it_declares() {
        // A page's /Tabs /A, which PDF 1.7 has only under Adobe's
        // extension level 3; and the catalog's /AF, only under PDF/A-3.
        let tabbed = changed(clean(), 3, |page| page.set("Tabs", "A"));
        let both = changed(tabbed.clone(), 1, |catalog| {
            let file = dict! {
                "Type" => "Filespec",
                "F" => text("data.csv"),
                "UF" => text("data.csv"),
                "AFRelationship" => "Data",
                "EF" => dict! { "F" => r(7) },
            };
            catalog.set("AF", vec![Object::from(file)]);
        });
        let mut both = both;
        let embedded = dict! {
            "Type" => "EmbeddedFile",
            "Subtype" => "text#2Fcsv",
            "Params" => dict! { "ModDate" => text("D:20261025120000Z") },
        };
        both.push((7, Body::stream(embedded, b"a,b")));
        let findings = |objects: &[(u32, Body)], options: &Options| {
            let report = check(
                &bytes::read(&written("1.7", objects, Xref::Table)),
                &Hold::All(Made::Built),
                options,
            );
            let found: Vec<(String, Rule)> = (report.findings.iter())
                .map(|f| (format!("{}/{}", f.table, f.key), f.rule))
                .collect();
            (found, report.extensions)
        };
        let none = Options::default();
        // Undeclared, every key of the associated file is later than 1.7,
        // and /Tabs /A isn't a value 1.7 has.
        let (found, under) = findings(&both, &none);
        assert_eq!(under, BTreeSet::new());
        let tabs = ("PageObject/Tabs".to_string(), Rule::Value);
        assert!(found.contains(&tabs), "{found:?}");
        let af = ("Catalog/AF".to_string(), Rule::TooNew);
        assert!(found.contains(&af), "{found:?}");
        assert!(
            (found.iter()).all(|f| f.1 == Rule::TooNew || *f == tabs),
            "{found:?}"
        );
        // Declared: Adobe's in the catalog's extensions dictionary, PDF/A-3
        // in its XMP metadata.
        let declared = changed(both, 1, |catalog| {
            let adbe = dict! { "BaseVersion" => "1.7", "ExtensionLevel" => 3 };
            catalog.set("Extensions", dict! { "ADBE" => adbe });
            catalog.set("Metadata", r(6));
        });
        let mut declared = declared;
        let xmp = br#"<x:xmpmeta><rdf:Description pdfaid:part="3"/></x:xmpmeta>"#;
        let metadata = dict! { "Type" => "Metadata", "Subtype" => "XML" };
        declared.push((6, Body::stream(metadata, xmp)));
        let names = |names: &[&str]| names.iter().map(|n| n.to_string()).collect();
        assert_eq!(
            findings(&declared, &none),
            (vec![], names(&["ADBE_Extn3", "ISO_19005_3"]))
        );
        // Or named, besides.
        let named = Options {
            extensions: names(&["ADBE_Extn3"]),
            ..Options::default()
        };
        assert_eq!(findings(&tabbed, &named), (vec![], names(&["ADBE_Extn3"])));
    }

    #[test]
    fn a_key_later_than_the_files_version_is_found() {
        let opacity = changed(clean(), 4, |annot| annot.set("CA", 0.5));
        let old = written("1.3", &opacity, Xref::Table);
        let report = checked_as(&old, &Hold::All(Made::Built));
        assert_eq!(
            placeless(&report),
            [finding(
                4,
                "Root/Pages/Kids[0]/Annots[0]",
                ("AnnotText", "CA"),
                Rule::TooNew,
                "0.5 (since 1.4)"
            )]
        );

        let report = checked_as(
            &written("1.4", &opacity, Xref::Table),
            &Hold::All(Made::Built),
        );
        assert_eq!(report.findings, [], "{report}");
        // The catalog's /Version, when later, is the file's.
        let later = changed(opacity, 1, |catalog| catalog.set("Version", "1.4"));
        let report = checked_as(
            &written("1.3", &later, Xref::Table),
            &Hold::All(Made::Built),
        );
        assert_eq!(report.findings, [], "{report}");
    }

    #[test]
    fn a_value_its_row_doesnt_list_is_found() {
        let bogus = changed(clean(), 4, |annot| {
            annot.set("Name", "Bogus");
            annot.set("CA", 1.5);
        });
        let report = checked(&bogus);
        assert_eq!(
            found(&report),
            [
                (
                    Some(4),
                    "AnnotText",
                    "Name",
                    Rule::Value,
                    "/Bogus (not one of Comment, Key, Note, Help, NewParagraph, Paragraph, Insert)"
                ),
                (
                    Some(4),
                    "AnnotText",
                    "CA",
                    Rule::Value,
                    "1.5 (not one of fn:Eval((@CA>=0.0) && (@CA<=1.0)))"
                ),
            ],
            "{report}"
        );

        // A stamp's name may be any, its row's `*` says.
        let stamp = changed(clean(), 4, |annot| {
            annot.set("Subtype", "Stamp");
            annot.set("Name", "WriterReviewed");
        });
        let report = checked(&stamp);
        assert_eq!(report.findings, [], "{report}");

        // A page's tab order /A is an extension's at 1.7 and PDF's from 2.0.
        let tabs = changed(clean(), 3, |page| page.set("Tabs", "A"));
        let report = checked(&tabs);
        assert_eq!(
            found(&report),
            [(
                Some(3),
                "PageObject",
                "Tabs",
                Rule::Value,
                "/A (not one of R, C, S)"
            )],
            "{report}"
        );
        let id = dict! { "ID" => vec![text("0123456789abcdef"), text("0123456789abcdef")] };
        let bytes = written_with("2.0", &tabs, Xref::Table, id);
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        let values: Vec<_> = (found(&report).into_iter())
            .filter(|f| f.3 == Rule::Value)
            .collect();
        assert_eq!(values, [], "{report}");

        // An image's mask's /Decode is one of two arrays.
        for (decode, values) in [
            (numbers(&[1, 0]), vec![]),
            (
                numbers(&[0, 2]),
                vec![(
                    Some(7),
                    "XObjectImageMask",
                    "Decode",
                    Rule::Value,
                    "[0 2] (not one of [0 1], [1 0])",
                )],
            ),
        ] {
            let image = dict! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
                "Mask" => r(7),
            };
            let mask = dict! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ImageMask" => true,
                "Decode" => decode,
            };
            let mut objects = changed(clean(), 3, |page| {
                page.set("Resources", dict! { "XObject" => dict! { "Im0" => r(6) } })
            });
            objects.push((6, Body::stream(image, &[0])));
            objects.push((7, Body::stream(mask, &[0])));
            let report = checked(&objects);
            assert_eq!(found(&report), values, "{report}");
            assert_eq!(report.checked.get("XObjectImageMask"), Some(&1), "{report}");
        }

        // An image's bits per component: one value, by fn:RequiredValue,
        // when its filter and /ImageMask ask (a DCT image's 8), else one
        // of those listed. Without a /Filter, whether one asks can't be
        // told, and nor can a value no condition is sure to allow.
        let bits = |filter: Option<&str>, bits: i64| {
            let mut image = dict! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => bits,
                "ImageMask" => false,
            };
            if let Some(filter) = filter {
                image.set("Filter", filter);
            } else {
                image.remove(b"ImageMask");
            }
            let mut objects = changed(clean(), 3, |page| {
                page.set("Resources", dict! { "XObject" => dict! { "Im0" => r(6) } })
            });
            objects.push((6, Body::stream(image, &[0])));
            checked(&objects)
        };
        let report = bits(None, 3);
        assert_eq!(report.findings, [], "{report}");
        let unchecked = Unchecked {
            column: "PossibleValues",
            why: "@Filter".to_string(),
            table: "XObjectImage".to_string(),
            key: "BitsPerComponent".to_string(),
        };
        assert_eq!(report.unchecked.get(&unchecked), Some(&1), "{report}");
        assert_eq!(bits(Some("DCTDecode"), 8).findings, []);
        let required = "fn:RequiredValue((@Filter==RunLengthDecode) || (@Filter==DCTDecode),8)";
        assert_eq!(
            found(&bits(Some("DCTDecode"), 4)),
            [(
                Some(6),
                "XObjectImage",
                "BitsPerComponent",
                Rule::Value,
                &*format!("4 (not 8, as {required} requires)")
            )],
            "a value listed besides isn't enough"
        );
        assert_eq!(bits(Some("FlateDecode"), 4).findings, []);
        let report = bits(Some("FlateDecode"), 3);
        assert_eq!(
            found(&report).iter().map(|f| f.3).collect::<Vec<_>>(),
            [Rule::Value],
            "{report}"
        );

        // The document information dictionary's /Trapped.
        let mut objects = clean();
        objects.push((6, dict! { "Trapped" => "Maybe" }.into()));
        let bytes = written_with("1.7", &objects, Xref::Table, dict! { "Info" => r(6) });
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        assert_eq!(
            found(&report),
            [(
                Some(6),
                "DocInfo",
                "Trapped",
                Rule::Value,
                "/Maybe (not one of True, False, Unknown)"
            )],
            "{report}"
        );
    }

    #[test]
    fn a_deprecated_key_type_or_value_is_counted_and_found_when_asked() {
        let with = |bytes: &[u8], deprecations: bool| {
            let options = Options {
                deprecations,
                ..Options::default()
            };
            check(&bytes::read(bytes), &Hold::All(Made::Built), &options)
        };
        let blend = |bm: Object| {
            changed(clean(), 5, |form| {
                form.set(
                    "Resources",
                    dict! { "ExtGState" => dict! { "G0" => dict! { "BM" => bm } } },
                )
            })
        };
        // PDF 2.0 asks every file for an /ID.
        let id = || dict! { "ID" => vec![text("0123456789abcdef"), text("0123456789abcdef")] };
        let mut objects = blend("Compatible".into());
        objects.push((6, dict! { "Title" => text("Mine") }.into()));
        let mut trailer = id();
        trailer.set("Info", r(6));
        let bytes = written_with("2.0", &objects, Xref::Table, trailer);
        let report = with(&bytes, false);
        assert_eq!(report.findings, [], "counted only: {report}");
        let at = |table: &str, key: &str| (table.to_string(), key.to_string());
        assert_eq!(
            report.deprecated,
            BTreeMap::from([
                (at("DocInfo", "Title"), 1),
                (at("GraphicsStateParameter", "BM"), 1)
            ]),
            "{report}"
        );
        let report = with(&bytes, true);
        assert_eq!(
            found(&report),
            [
                (
                    Some(6),
                    "DocInfo",
                    "Title",
                    Rule::Deprecated,
                    "(Mine) (deprecated in 2.0)"
                ),
                (
                    Some(5),
                    "GraphicsStateParameter",
                    "BM",
                    Rule::Deprecated,
                    "/Compatible (deprecated in 1.4)"
                ),
            ],
            "{report}"
        );

        // A blend mode as an array is deprecated from 2.0, as a type, and so
        // is each element of one, as a row.
        let array = blend(vec![Object::from("Normal")].into());
        let bytes = written_with("2.0", &array, Xref::Table, id());
        let report = with(&bytes, true);
        assert_eq!(
            found(&report),
            [
                (
                    Some(5),
                    "GraphicsStateParameter",
                    "BM",
                    Rule::Deprecated,
                    "[/Normal] (array deprecated in 2.0)"
                ),
                (
                    Some(5),
                    "ArrayOfBlendModes",
                    "0",
                    Rule::Deprecated,
                    "/Normal (deprecated in 2.0)"
                ),
            ],
            "{report}"
        );
        let report = with(&written("1.7", &array, Xref::Table), true);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.deprecated, BTreeMap::new(), "{report}");
    }

    #[test]
    fn a_value_direct_where_it_must_be_indirect_or_the_reverse_is_found() {
        // An annotation's popup is an indirect reference.
        let popup = dict! {
            "Type" => "Annot",
            "Subtype" => "Popup",
            "Rect" => numbers(&[0, 0, 10, 10]),
        };
        let direct = changed(clean(), 4, |annot| annot.set("Popup", popup));
        let report = checked(&direct);
        assert_eq!(
            found(&report),
            [(
                Some(4),
                "AnnotText",
                "Popup",
                Rule::Indirect,
                "<< /Rect [0 0 10 10] /Subtype /Popup /Type /Annot >>"
            )],
            "{report}"
        );

        // An encrypted file's /ID is direct, in a table's trailer and in a
        // cross-reference stream; and from PDF 2.0, a cross-reference
        // stream's /Encrypt is too (pdf-issues #246). Whether the file's
        // encrypted is its trailer's /Encrypt: nothing here is decrypted.
        let encrypt = dict! {
            "Filter" => "Standard", "V" => 5, "R" => 6, "Length" => 256, "P" => -4,
            "O" => text("o"), "U" => text("u"), "OE" => text("oe"), "UE" => text("ue"),
            "Perms" => text("perms"), "StrF" => "StdCF", "StmF" => "StdCF",
            "CF" => dict! { "StdCF" => dict! { "CFM" => "AESV3", "Length" => 32 } },
        };
        let id = || Object::from(vec![text("0123456789abcdef"), text("0123456789abcdef")]);
        for kind in [Xref::Table, Xref::Stream] {
            for referenced in [false, true] {
                let mut objects = clean();
                objects.push((6, encrypt.clone().into()));
                let mut trailer = dict! { "Encrypt" => r(6) };
                if referenced {
                    objects.push((7, id().into()));
                    trailer.set("ID", r(7));
                } else {
                    trailer.set("ID", id());
                }
                let bytes = written_with("2.0", &objects, kind, trailer);
                let report = checked_as(&bytes, &Hold::All(Made::Built));
                let mut direct: Vec<_> = (found(&report).into_iter())
                    .filter(|f| f.3 == Rule::Direct)
                    .map(|f| (f.1, f.2, numberless(f.4)))
                    .collect();
                direct.sort();
                let r = || "R".to_string();
                let expected = match (referenced, kind) {
                    (false, Xref::Table) => vec![],
                    (true, Xref::Table) => vec![("FileTrailer", "ID", r())],
                    (false, Xref::Stream) => vec![("XRefStream", "Encrypt", r())],
                    (true, Xref::Stream) => {
                        vec![("XRefStream", "Encrypt", r()), ("XRefStream", "ID", r())]
                    }
                };
                assert_eq!(direct, expected, "{kind:?}: {report}");
                let unread = (report.unchecked.keys()).any(|u| u.column == "IndirectReference");
                assert!(!unread, "{report}");
            }
        }
    }

    #[test]
    fn a_key_the_model_doesnt_list_is_counted_by_its_kind_of_name() {
        let keys = changed(clean(), 4, |annot| {
            annot.set("WriterAnnot", 1);
            annot.set("ABCD_x", 1);
            annot.set("XXMine", 1);
        });
        // A writer's own key, exempt.
        let options = Options {
            exempt: vec![Exempt {
                name: b"WriterAnnot".to_vec(),
                prefix: false,
            }],
            ..Options::default()
        };
        let checked_with =
            |bytes: &[u8], made: Made| check(&bytes::read(bytes), &Hold::All(made), &options);
        // The trailer's /DocChecksum is OpenOffice's extension's key.
        let trailer = dict! { "DocChecksum" => "Sum" };
        let bytes = written_with("1.7", &keys, Xref::Table, trailer);
        let report = checked_with(&bytes, Made::Built);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(
            report.unlisted,
            BTreeMap::from([
                (unlisted("AnnotText", "ABCD_x", Class::Second), 1),
                (unlisted("AnnotText", "WriterAnnot", Class::Other), 1),
                (unlisted("AnnotText", "XXMine", Class::Third), 1),
                (
                    unlisted(
                        "FileTrailer",
                        "DocChecksum",
                        Class::Extension("OpenOffice".to_string())
                    ),
                    1
                ),
            ]),
            "{report}"
        );

        // Any other key the model doesn't list in an object built whole is
        // a finding: misspelled, or a key no longer exempt. Merged over a
        // source's, it may be the source's, and is counted only. A page's
        // content, the plain stream table, is said to be a stream of its
        // own: a key it doesn't list is one too.
        let mut misspelled = changed(keys, 4, |annot| {
            annot.set("Contnets", text("A note"));
            annot.set("WriterPart", 1);
        });
        misspelled = changed(misspelled, 3, |page| page.set("Contents", r(6)));
        misspelled.push((6, Body::stream(dict! { "Subtype" => "Form" }, b"q Q")));
        let bytes = written("1.7", &misspelled, Xref::Table);
        let report = checked_with(&bytes, Made::Built);
        assert_eq!(
            found(&report),
            [
                (Some(6), "Stream", "Subtype", Rule::Unlisted, "/Form"),
                (Some(4), "AnnotText", "Contnets", Rule::Unlisted, "(A note)"),
                (Some(4), "AnnotText", "WriterPart", Rule::Unlisted, "1"),
            ],
            "{report}"
        );
        let report = checked_with(&bytes, Made::Merged);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.unlisted.len(), 6, "{report}");

        // Unless asked to find them in every held object, merged too, or in
        // none; counted all the same.
        let unlisted_in = |unlisted: UnlistedIn, made: Made| {
            let options = Options {
                unlisted,
                ..options.clone()
            };
            check(&bytes::read(&bytes), &Hold::All(made), &options)
        };
        let report = unlisted_in(UnlistedIn::All, Made::Merged);
        let made: Vec<(&str, Made)> = (report.findings.iter())
            .map(|f| (f.key.as_str(), f.made))
            .collect();
        assert_eq!(
            made,
            [
                ("Subtype", Made::Merged),
                ("Contnets", Made::Merged),
                ("WriterPart", Made::Merged),
            ],
            "{report}"
        );
        let report = unlisted_in(UnlistedIn::Never, Made::Built);
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.unlisted.len(), 6, "{report}");
    }

    #[test]
    fn a_special_case_false_is_found_and_one_that_cant_be_told_is_counted() {
        // A page's private data without the page's /LastModified.
        let piece = dict! { "Writer" => dict! {
            "LastModified" => text("D:20261024"),
            "Private" => 1,
        } };
        let objects = changed(clean(), 3, |page| page.set("PieceInfo", piece));
        // An annotation's reply type, which is only meaningful in a reply
        // (fn:IsMeaningful(fn:IsPresent(IRT))): this one is one.
        let objects = changed(objects, 4, |annot| {
            annot.set("RT", "R");
            annot.set("IRT", r(4));
        });
        let report = checked(&objects);
        assert_eq!(
            found(&report),
            [
                (
                    Some(3),
                    "PageObject",
                    "PieceInfo",
                    Rule::Special,
                    "<< /Writer << /LastModified (D:20261024) /Private 1 >> >> \
                     (fn:Eval(fn:IsPresent(LastModified)))"
                ),
                // Its Required says so too.
                (
                    Some(3),
                    "PageObject",
                    "LastModified",
                    Rule::Missing,
                    "absent"
                ),
            ],
            "{report}"
        );
        let rt = ("AnnotText".to_string(), "RT".to_string());
        assert!(
            !(report.unchecked.keys()).any(|u| u.key == "RT"),
            "{report}"
        );
        assert_eq!(report.ignored.get(&rt), None, "{report}");
        // Outside a reply, readers ignore it: no finding, and counted so.
        let objects = changed(objects, 4, |annot| {
            annot.remove(b"IRT");
        });
        let report = checked(&objects);
        assert_eq!(report.ignored.get(&rt), Some(&1), "{report}");
        assert!(!found(&report).iter().any(|f| f.2 == "RT"), "{report}");
    }

    #[test]
    fn what_the_walk_leaves_unread_the_model_never_asks() {
        for table in grammar().tables() {
            for row in &table.rows {
                let at = format!("{}.tsv line {}", table.name, row.line);
                // Deprecation is read on keys, types and values: never on links.
                let deprecated = (row.link.iter().flatten())
                    .any(|l| l.wraps.iter().any(|w| w.func == WrapFn::Deprecated));
                assert!(!deprecated, "{at}: a deprecated link");
                // A tree's IndirectReference is its values', which the walk
                // doesn't read: none asks it but under an extension.
                for (ty, cell) in row.r#type.iter().zip(&row.indirect_reference) {
                    if matches!(ty.item, ArlType::NameTree | ArlType::NumberTree) && cell != "FALSE"
                    {
                        let since = predicate::since(&row.since_version, &BTreeSet::new());
                        assert!(matches!(since, Since::Extension(_)), "{at}: {cell}");
                    }
                }
            }
        }
    }

    #[test]
    fn beyond_leaves_out_what_a_source_has_in_merged_objects_only() {
        let at = |n: u32, made: Made, rule: Rule, value: &str| Finding {
            object: Some((n, 0)),
            made,
            path: "Root/Pages/Kids[0]/Annots[0]".to_string(),
            table: "AnnotText".to_string(),
            key: "Popup".to_string(),
            rule,
            value: value.to_string(),
            at: None,
        };
        let merged = at(4, Made::Merged, Rule::Type, "/Bogus");
        let built = at(5, Made::Built, Rule::Type, "/Bogus");
        let renumbered = at(4, Made::Merged, Rule::Type, "<< /Kids [3 0 R 14 0 R] >>");
        let other = at(4, Made::Merged, Rule::Type, "/Other");
        let missing = at(4, Made::Merged, Rule::Missing, "/Bogus");
        let written = Report {
            findings: vec![
                merged.clone(),
                built.clone(),
                renumbered.clone(),
                other.clone(),
                missing.clone(),
            ],
            ..Report::default()
        };
        let one = Report {
            findings: vec![at(9, Made::Merged, Rule::Type, "/Bogus")],
            ..Report::default()
        };
        let two = Report {
            findings: vec![at(9, Made::Built, Rule::Type, "<< /Kids [30 0 R 7 0 R] >>")],
            ..Report::default()
        };
        assert_eq!(
            written.beyond(&[]),
            [&merged, &built, &renumbered, &other, &missing]
        );
        assert_eq!(
            written.beyond(&[&one]),
            [&built, &renumbered, &other, &missing],
            "a built object's finding is never carried"
        );
        assert_eq!(
            written.beyond(&[&one, &two]),
            [&built, &other, &missing],
            "a reference's numbers aside"
        );

        // A reader's finding, its offsets aside.
        let reader = |n: u32, value: &str| Finding {
            rule: Rule::Reader,
            table: "-".to_string(),
            key: "-".to_string(),
            ..at(n, Made::Merged, Rule::Reader, value)
        };
        let written = Report {
            findings: vec![reader(4, "its /Length, 99, doesn't end at endstream")],
            ..Report::default()
        };
        let source = Report {
            findings: vec![reader(9, "its /Length, 7, doesn't end at endstream")],
            ..Report::default()
        };
        assert_eq!(written.beyond(&[&source]), Vec::<&Finding>::new());

        for (value, numberless_value) in [
            ("3 0 R", "R"),
            ("<< /Kids [3 0 R 14 0 R] >>", "<< /Kids [R R] >>"),
            (
                "its /Kids[0] is 12 0 R, no object",
                "its /Kids[0] is R, no object",
            ),
            ("[1 2 3]", "[1 2 3]"),
            ("/A1 0 R", "/A1 0 R"),
            ("1 0 Rx", "1 0 Rx"),
            ("(é 2 0 R)", "(é R)"),
        ] {
            assert_eq!(numberless(value), numberless_value, "{value}");
        }
    }

    /// A value is rendered the same however its keys were ordered or its
    /// stream encoded, as a rewrite may do either.
    #[test]
    fn a_value_is_the_same_after_its_keys_are_sorted_and_its_stream_encoded_anew() {
        let written = Object::stream(dict! {
            "Subtype" => "Form",
            "Length" => 87,
            "BBox" => numbers(&[0, 0, 1, 1]),
            "Filter" => "FlateDecode",
            "DecodeParms" => dict! { "Predictor" => 12 },
        });
        let rewritten = Object::stream(dict! {
            "BBox" => numbers(&[0, 0, 1, 1]),
            "Length" => 54,
            "Subtype" => "Form",
        });
        assert_eq!(
            render(&written),
            "stream << /BBox [0 0 1 1] /Subtype /Form >>"
        );
        assert_eq!(render(&rewritten), render(&written));
        // A dictionary's /Length, which isn't an encoding, is kept.
        let dict = Object::from(dict! { "Length" => 3, "A" => 1 });
        assert_eq!(render(&dict), "<< /A 1 /Length 3 >>");
    }

    /// A rewrite's findings are carried from the same path in the baseline,
    /// or from anywhere in it where it reached nothing at that path.
    #[test]
    fn by_path_carries_a_finding_from_its_path_or_where_the_baseline_has_none_from_anywhere() {
        let missing = |n: u32, made: Made, path: &str| Finding {
            object: Some((n, 0)),
            made,
            path: path.to_string(),
            table: "PageObject".to_string(),
            key: "MediaBox".to_string(),
            rule: Rule::Missing,
            value: "absent".to_string(),
            at: None,
        };
        let stayed = missing(10, Made::Merged, "Root/Pages/Kids[0]");
        let written = missing(11, Made::Merged, "Root/Pages/Kids[1]");
        let moved = missing(12, Made::Merged, "Root/Pages/Kids[2]");
        let built = missing(13, Made::Built, "Root/Pages/Kids[0]");
        let rewrite = Report {
            findings: vec![
                stayed.clone(),
                written.clone(),
                moved.clone(),
                built.clone(),
            ],
            ..Report::default()
        };
        let baseline = Report {
            findings: vec![missing(3, Made::Merged, "Root/Pages/Kids[0]")],
            paths: [
                "trailer",
                "Root",
                "Root/Pages",
                "Root/Pages/Kids[0]",
                "Root/Pages/Kids[1]",
            ]
            .map(String::from)
            .into(),
            ..Report::default()
        };
        assert_eq!(
            rewrite.by_path(&baseline),
            [&written, &built],
            "Kids[1] had none, though Kids[0] had one; the baseline has no Kids[2]"
        );
    }

    #[test]
    fn paths_are_recorded_when_asked() {
        let bytes = written("1.7", &clean(), Xref::Table);
        let doc = bytes::read(&bytes);
        let hold = Hold::All(Made::Merged);
        assert!(check(&doc, &hold, &Options::default()).paths.is_empty());
        let options = Options {
            paths: true,
            ..Options::default()
        };
        let paths = check(&doc, &hold, &options).paths;
        for path in [
            "trailer",
            "Root",
            "Root/Pages",
            "Root/Pages/Kids[0]",
            "Root/Pages/Kids[0]/Annots[0]",
            "Root/Pages/Kids[0]/Annots[0]/AP/N",
        ] {
            assert!(paths.contains(path), "{path}: {paths:?}");
        }
    }

    /// An update's findings are carried from the object of the same number
    /// before it, or the trailer from the trailer, and from nowhere else.
    #[test]
    fn since_carries_a_finding_from_the_same_object_only() {
        let missing = |n: Option<u32>, made: Made| Finding {
            object: n.map(|n| (n, 0)),
            made,
            path: "Root/Pages/Kids[0]".to_string(),
            table: "PageObject".to_string(),
            key: "MediaBox".to_string(),
            rule: Rule::Missing,
            value: "absent".to_string(),
            at: None,
        };
        let page = missing(Some(4), Made::Merged);
        let built = missing(Some(5), Made::Built);
        let other = missing(Some(6), Made::Merged);
        let in_stream = Finding {
            table: "XRefStream".to_string(),
            key: "Info".to_string(),
            ..missing(Some(20), Made::Merged)
        };
        let updated = Report {
            findings: vec![
                page.clone(),
                built.clone(),
                other.clone(),
                in_stream.clone(),
            ],
            trailer: Some((20, 0)),
            ..Report::default()
        };
        let before = Report {
            findings: vec![
                missing(Some(4), Made::Merged),
                missing(Some(5), Made::Merged),
                // The trailer before, a table's.
                Finding {
                    table: "XRefStream".to_string(),
                    key: "Info".to_string(),
                    ..missing(None, Made::Merged)
                },
            ],
            ..Report::default()
        };
        assert_eq!(
            updated.since(&before),
            [&built, &other],
            "4's is carried, and the trailer's; a built object's never; 6 had none"
        );
        assert_eq!(
            updated.since(&Report::default()),
            updated.findings.iter().collect::<Vec<_>>()
        );

        // An object nothing reaches is alike whichever reference to it is
        // named.
        let unreached = |key: &str| Finding {
            path: "-".to_string(),
            table: "-".to_string(),
            key: key.to_string(),
            rule: Rule::Unreached,
            value: "<< /Type /Encoding >>".to_string(),
            ..missing(Some(7), Made::Merged)
        };
        let updated = Report {
            findings: vec![unreached("Encoding")],
            ..Report::default()
        };
        let before = Report {
            findings: vec![unreached("PDFDocEncoding")],
            ..Report::default()
        };
        assert_eq!(updated.since(&before), Vec::<&Finding>::new());
    }

    /// The carried rule matches a merged object's finding against every
    /// object of the sources, not the one it was emitted over: a page
    /// written without its `/MediaBox` is carried when any page of a source
    /// lacks one.
    #[test]
    fn a_merged_objects_finding_is_carried_from_any_object_of_a_source() {
        let missing = |n: u32, made: Made| Finding {
            object: Some((n, 0)),
            made,
            path: format!("Root/Pages/Kids[{n}]"),
            table: "PageObject".to_string(),
            key: "MediaBox".to_string(),
            rule: Rule::Missing,
            value: "absent".to_string(),
            at: None,
        };
        let written = Report {
            findings: vec![missing(3, Made::Merged), missing(4, Made::Built)],
            ..Report::default()
        };
        let another_page = Report {
            findings: vec![missing(9, Made::Merged)],
            ..Report::default()
        };
        assert_eq!(written.beyond(&[&another_page]), [&missing(4, Made::Built)]);
    }

    #[test]
    fn a_source_carries_a_key_too_new_whatever_its_value_and_a_trailers_form() {
        let at =
            |object: Option<u32>, made: Made, (table, key): (&str, &str), value: &str| Finding {
                object: object.map(|n| (n, 0)),
                made,
                path: "-".to_string(),
                table: table.to_string(),
                key: key.to_string(),
                rule: Rule::TooNew,
                value: value.to_string(),
                at: None,
            };
        let info = at(
            Some(3),
            Made::Merged,
            ("PageObject", "PieceInfo"),
            "<< … 9 >>",
        );
        let built = at(
            Some(8),
            Made::Built,
            ("PageObject", "PieceInfo"),
            "<< … 9 >>",
        );
        let size = at(
            Some(20),
            Made::Merged,
            ("XRefStream", "Size"),
            "21 (since 1.5)",
        );
        let index = at(
            Some(20),
            Made::Merged,
            ("ArrayOfXRefIndexIntegers", "2"),
            "7",
        );
        let written = Report {
            findings: vec![info.clone(), built.clone(), size.clone(), index.clone()],
            trailer: Some((20, 0)),
            ..Report::default()
        };
        let page = at(
            Some(3),
            Made::Merged,
            ("PageObject", "PieceInfo"),
            "<< … 4 >>",
        );
        let pages = Report {
            findings: vec![
                page.clone(),
                at(Some(3), Made::Merged, ("XRefStream", "W"), ""),
            ],
            trailer: Some((12, 0)),
            ..Report::default()
        };
        assert_eq!(
            written.beyond(&[&pages]),
            [&built, &size, &index],
            "a page's key, whatever its value; and not the trailer's, from no trailer"
        );
        let stream = Report {
            findings: vec![page, at(Some(12), Made::Merged, ("XRefStream", "W"), "")],
            trailer: Some((12, 0)),
            ..Report::default()
        };
        assert_eq!(
            written.beyond(&[&stream]),
            [&built],
            "the trailer's form, which the source's trailer is too"
        );
        let table = Report {
            findings: vec![at(None, Made::Merged, ("FileTrailer", "XRefStm"), "9")],
            ..Report::default()
        };
        assert_eq!(written.beyond(&[&table]), [&info, &built]);
    }

    #[test]
    fn a_file_is_checked_at_the_version_asked() {
        // Before 1.3 a page's /PieceInfo is too new; from 1.3 it asks for
        // the page's /LastModified too.
        let piece = changed(clean(), 3, |page| {
            page.set("PieceInfo", dict! { "Mine" => dict! {} })
        });
        let doc = bytes::read(&written("1.2", &piece, Xref::Table));
        assert_eq!(file_version(&doc), Version::new(1, 2));
        let hold = Hold::All(Made::Built);
        let as_written = check(&doc, &hold, &Options::default());
        let options = Options {
            version: Some(Version::new(1, 4)),
            ..Options::default()
        };
        let at_1_4 = check(&doc, &hold, &options);
        let page = |report: &Report| {
            (report.findings.iter())
                .filter(|f| f.table == "PageObject")
                .map(|f| (f.key.clone(), f.rule))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            page(&as_written),
            [
                ("PieceInfo".to_string(), Rule::TooNew),
                ("PieceInfo".to_string(), Rule::Special)
            ],
            "{as_written}"
        );
        assert_eq!(
            page(&at_1_4),
            [
                ("PieceInfo".to_string(), Rule::Special),
                ("LastModified".to_string(), Rule::Missing)
            ],
            "{at_1_4}"
        );
    }

    /// Every PDF under `PDF_LINT_CORPUS` read, opened when it's encrypted
    /// by the empty password, and checked, every object held as merged: no panic, and
    /// every finding's place inside its file or its decoded data.
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files"]
    fn a_corpus_checks() {
        use std::path::PathBuf;
        use std::time::Instant;

        let dir = std::env::var_os("PDF_LINT_CORPUS").expect("PDF_LINT_CORPUS names a folder");
        let mut folders = vec![PathBuf::from(dir)];
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
                {
                    files.push(path);
                }
            }
        }
        files.sort();
        let (mut objects, mut keys, mut placed, mut unplaced) = (0, 0, 0, 0);
        let mut rules: BTreeMap<Rule, usize> = BTreeMap::new();
        let mut unchecked = 0;
        let start = Instant::now();
        for path in &files {
            let data = std::fs::read(path).unwrap();
            let doc = std::panic::catch_unwind(|| bytes::read_opened(&data, &[]))
                .unwrap_or_else(|_| panic!("{} panicked reading", path.display()));
            let report = std::panic::catch_unwind(|| {
                check(&doc, &Hold::All(Made::Merged), &Options::default())
            })
            .unwrap_or_else(|_| panic!("{} panicked checking", path.display()));
            objects += report.objects();
            keys += report.keys;
            unchecked += report.unchecked.values().sum::<usize>();
            for f in &report.findings {
                *rules.entry(f.rule).or_default() += 1;
                match f.at {
                    Some(at) => {
                        let within = match at.stream {
                            None => data.len(),
                            Some(stream) => doc.decoded[&stream].len(),
                        };
                        let inside = at.start <= at.end && at.end <= within;
                        assert!(inside, "{}: {f}", path.display());
                        placed += 1;
                    }
                    None => unplaced += 1,
                }
            }
        }
        println!(
            "{} files in {:.1} s: {objects} objects checked, {keys} keys, {unchecked} checks not made",
            files.len(),
            start.elapsed().as_secs_f64()
        );
        println!("findings: {placed} with their bytes, {unplaced} without");
        for (rule, n) in rules {
            println!("  {rule}: {n}");
        }
    }

    #[test]
    fn a_hold_file_names_objects_and_how_each_was_made() {
        let hold = Hold::parse("# held\n4 0 merged\n\n5 0 built\n").unwrap();
        let expected = BTreeMap::from([((4, 0), Made::Merged), ((5, 0), Made::Built)]);
        assert_eq!(hold, Hold::Only(expected));
        for (text, why) in [
            ("4 0", "line 1: not a number"),
            ("4 x built", "line 1: \"x\" isn't a generation"),
            ("4 0 made", "line 1: \"made\" isn't built or merged"),
            ("4 0 built\n4 0 merged", "line 2: 4 0 is held a second time"),
        ] {
            let e = Hold::parse(text).unwrap_err();
            assert!(format!("{e:#}").contains(why), "{text:?}: {e:#}");
        }
    }

    #[test]
    fn an_exempt_file_names_keys_and_the_beginnings_of_keys() {
        let exempt = Exempt::parse("# mine\n/WriterAnnot\n\n  /Writer*  \n/A#20B\n/Star#2A\n");
        let key = |name: &[u8], prefix| Exempt {
            name: name.to_vec(),
            prefix,
        };
        assert_eq!(
            exempt.unwrap(),
            [
                key(b"WriterAnnot", false),
                key(b"Writer", true),
                key(b"A B", false),
                key(b"Star*", false),
            ]
        );
        for (text, why) in [
            ("WriterAnnot", "line 1: \"WriterAnnot\" isn't a name"),
            ("/Writer Annot", "line 1: \"/Writer Annot\" isn't a name"),
            ("*", "line 1: \"*\" isn't a name"),
            ("/", "line 1: an empty name"),
            ("/*", "line 1: an empty name"),
            ("/A#G1", "line 1: \"/A#G1\" has a # without two hex digits"),
            ("/A#00", "line 1: \"/A#00\" has #00"),
            ("/A\n/A#41*\n/A", "line 3: \"/A\" is exempt a second time"),
        ] {
            let e = Exempt::parse(text).unwrap_err();
            assert!(format!("{e:#}").contains(why), "{text:?}: {e:#}");
        }
    }

    /// A hold file's line that doesn't read is said word for word: its
    /// number and why, a number's or a generation's after what std said
    /// parsing it.
    #[test]
    fn a_hold_files_line_that_doesnt_read_says_why() {
        let number = |text: &str| text.parse::<u32>().unwrap_err();
        let generation = |text: &str| text.parse::<u16>().unwrap_err();
        let cases = [
            (
                "# held\n\n4 0",
                "line 3: not a number, a generation and built or merged".to_string(),
            ),
            (
                "x 0 built",
                format!("line 1: \"x\" isn't an object number: {}", number("x")),
            ),
            (
                "99999999999 0 built",
                format!(
                    "line 1: \"99999999999\" isn't an object number: {}",
                    number("99999999999")
                ),
            ),
            (
                "4 y built",
                format!("line 1: \"y\" isn't a generation: {}", generation("y")),
            ),
            (
                "4 70000 built",
                format!(
                    "line 1: \"70000\" isn't a generation: {}",
                    generation("70000")
                ),
            ),
            (
                "4 0 made",
                "line 1: \"made\" isn't built or merged".to_string(),
            ),
            (
                "4 0 built\n4 0 merged",
                "line 2: 4 0 is held a second time".to_string(),
            ),
        ];
        for (text, said) in cases {
            let e = Hold::parse(text).unwrap_err();
            assert_eq!(format!("{e:#}"), said, "{text:?}");
        }
    }

    /// An exempt file's line that doesn't read is said word for word: its
    /// number, the line as written, and why.
    #[test]
    fn an_exempt_files_line_that_doesnt_read_says_why() {
        let cases = [
            (
                "# mine\n\nWriterAnnot",
                "line 3: \"WriterAnnot\" isn't a name, as /MyKey, or /My*",
            ),
            (
                "/A#G1",
                "line 1: \"/A#G1\" has a # without two hex digits after it",
            ),
            (
                "/A#00",
                "line 1: \"/A#00\" has #00, which a name may not hold",
            ),
            ("/", "line 1: an empty name"),
            ("/A\n/A", "line 2: \"/A\" is exempt a second time"),
        ];
        for (text, said) in cases {
            let e = Exempt::parse(text).unwrap_err();
            assert_eq!(format!("{e:#}"), said, "{text:?}");
        }
    }

    /// The bytes each finding says it's at, as text.
    fn bytes_at<'a>(bytes: &'a [u8], finding: &Finding) -> &'a str {
        let at = finding
            .at
            .unwrap_or_else(|| panic!("{finding} says nowhere"));
        std::str::from_utf8(&bytes[at.start..at.end]).unwrap()
    }

    /// Each finding says where in the file its bytes are: a key and its
    /// value, an array's element, the dictionary that lacks a key, the
    /// object nothing reaches.
    #[test]
    fn every_finding_says_where_its_bytes_were_written() {
        let faults = changed(clean(), 4, |annot| {
            annot.set("Rect", numbers(&[1, 2, 3]));
            annot.set("Contnets", text("A note"));
        });
        let faults = changed(faults, 3, |page| {
            page.remove(b"MediaBox");
            page.set("Annots", vec![r(4), r(9)]);
        });
        let mut faults = faults;
        faults.push((6, dict! { "Mine" => 1 }.into()));
        let bytes = written("1.7", &faults, Xref::Table);
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        let at: Vec<(Rule, &str, &str)> = (report.findings.iter())
            .map(|f| (f.rule, f.key.as_str(), bytes_at(&bytes, f)))
            .collect();
        assert_eq!(
            at,
            [
                (
                    Rule::Missing,
                    "MediaBox",
                    "<< /Type /Page /Parent 2 0 R /Resources << >> /Annots [4 0 R 9 0 R] >>"
                ),
                (Rule::Unresolved, "1", "9 0 R"),
                (Rule::Type, "Rect", "/Rect [1 2 3]"),
                (Rule::Unlisted, "Contnets", "/Contnets (A note)"),
                (Rule::Unreached, "-", "<< /Mine 1 >>"),
            ],
            "{report}"
        );
        assert!(report.to_string().contains("[bytes "), "{report}");
    }

    #[test]
    fn a_key_written_twice_is_found_where_it_was_first_written() {
        let bytes = write::assembled(
            "1.7",
            &[
                (1, b"<< /Type /Catalog /Pages 2 0 R /Type /Catalog >>"),
                (2, b"<< /Type /Pages /Kids [] /Count 0 >>"),
            ],
            "<< /Size 3 /Root 1 0 R >>",
        );
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        let duplicates: Vec<_> = (report.findings.iter())
            .filter(|f| f.rule == Rule::Duplicate)
            .map(|f| {
                (
                    f.table.as_str(),
                    f.key.as_str(),
                    f.value.as_str(),
                    bytes_at(&bytes, f),
                )
            })
            .collect();
        assert_eq!(
            duplicates,
            [(
                "Catalog",
                "Type",
                "/Catalog (its key written again later)",
                "/Type /Catalog"
            )],
            "{report}"
        );
        let at = report.findings[0].at.unwrap();
        assert_eq!(
            at.start,
            find(&bytes, b"/Type"),
            "the first, not the second"
        );
    }

    #[test]
    fn what_the_reader_had_to_choose_is_found_in_held_objects_only() {
        let bytes = write::assembled(
            "1.7",
            &[
                (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
                (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
                (
                    3,
                    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> \
                      /Contents [4 0 R 5 0 R] >>",
                ),
                (4, b"<< /Length 3 >>stream\rq Q\nendstream"),
                (5, b"<< /Length 3 >>stream q Q\nendstream"),
            ],
            "<< /Size 6 /Root 1 0 R >>",
        );
        let report = checked_as(&bytes, &Hold::All(Made::Built));
        let read: Vec<_> = (report.findings.iter())
            .filter(|f| f.rule == Rule::Reader)
            .map(|f| (f.object, f.value.as_str(), bytes_at(&bytes, f)))
            .collect();
        assert_eq!(
            read,
            [
                (
                    Some((4, 0)),
                    "a lone CR after `stream`, where ISO 32000-2 wants CR LF or LF",
                    "\r"
                ),
                (Some((5, 0)), "no end of line after `stream`", "stream"),
                (
                    Some((5, 0)),
                    "its /Length, 3, doesn't end at endstream",
                    "3"
                ),
            ],
            "{report}"
        );
        let other = Hold::Only(BTreeMap::from([((3, 0), Made::Built)]));
        let report = checked_as(&bytes, &other);
        assert!(
            report.findings.iter().all(|f| f.rule != Rule::Reader),
            "{report}"
        );
    }

    /// What depends on a string's bytes, when the reader gave text or
    /// ciphertext instead, is counted as a check not made.
    #[test]
    fn a_strings_bytes_unknown_are_checked_where_they_can_be() {
        let link = |uri: Str, date: Str| {
            let trailer = dict! { "Size" => 5, "Root" => r(1) };
            let mut doc = Document::new(Version::new(1, 7), trailer);
            doc.insert(1, dict! { "Type" => "Catalog", "Pages" => r(2) });
            doc.insert(
                2,
                dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 },
            );
            doc.insert(
                3,
                dict! {
                    "Type" => "Page",
                    "Parent" => r(2),
                    "MediaBox" => numbers(&[0, 0, 1, 1]),
                    "Resources" => dict! {},
                    "LastModified" => Object::new(Value::String(date, None)),
                    "Annots" => vec![r(4)],
                },
            );
            doc.insert(
                4,
                dict! {
                    "Type" => "Annot",
                    "Subtype" => "Link",
                    "Rect" => numbers(&[0, 0, 1, 1]),
                    "A" => dict! { "S" => "URI", "URI" => Object::new(Value::String(uri, None)) },
                },
            );
            check(&doc, &Hold::All(Made::Built), &Options::default())
        };
        let bytes = |b: &str| Str::Bytes(b.as_bytes().to_vec());
        let report = link(bytes("https://x"), bytes("D:20261024"));
        assert_eq!(report.findings, [], "{report}");
        let report = link(Str::Bytes(b"https://\xE9".to_vec()), bytes("D:20261324"));
        let types: Vec<_> = (found(&report).into_iter()).map(|f| (f.2, f.3)).collect();
        assert_eq!(
            types,
            [("LastModified", Rule::Type), ("URI", Rule::Type)],
            "{report}"
        );

        let report = link(
            Str::Text("https://x".to_string()),
            Str::Encrypted(b"\x01".to_vec()),
        );
        assert_eq!(report.findings, [], "{report}");
        let not_made = |table: &str, key: &str| Unchecked {
            column: "Type",
            why: "the string's bytes".to_string(),
            table: table.to_string(),
            key: key.to_string(),
        };
        assert_eq!(
            report.unchecked.get(&not_made("ActionURI", "URI")),
            Some(&1),
            "{report}"
        );
        assert_eq!(
            report
                .unchecked
                .get(&not_made("PageObject", "LastModified")),
            Some(&1),
            "{report}"
        );
        // Text is a date's text, which can be told.
        let report = link(bytes("https://x"), Str::Text("D:20261324".to_string()));
        assert_eq!(found(&report).len(), 1, "{report}");
    }
}
