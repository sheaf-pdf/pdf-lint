//! The predicates in the model's cells, parsed, and evaluated where the
//! checked objects need them. The language is INTERNAL_GRAMMAR.md's
//! "Validation of predicates (declarative functions)": `fn:` functions,
//! fully bracketed `&&` and `||`, six comparisons, arithmetic, and keys,
//! `@` values and `::` paths from the object, its `parent::` or the
//! `trailer::`. What each function means is its "Predicates (declarative
//! functions)"; "Parameters to predicates" says any part of a predicate
//! may be indeterminable: a key the object lacks or has null, or a value
//! of a type the comparison can't use.
//!
//! [`SUPPORTED`] are the functions evaluated, all INTERNAL_GRAMMAR.md's
//! but `fn:IsFullyQualifiedFieldName`, which the embedded model's
//! corrections call. Any other comes to [`Truth::Unknown`], named: never
//! guessed, so that a checker can count a check an unknown predicate
//! stands in the way of as not made. Arithmetic is evaluated on numbers,
//! integers kept integers.
//!
//! A key the object lacks, or has null, is indeterminable, but for a
//! SpecialCase's or PossibleValues' `@Key` with a DefaultValue, which
//! stands in for it ([`Cx::defaults`]): "if both `/A` and `/B` are
//! optional and both had a DefaultValue [...] then this predicate would
//! always be determinable" ("Parameters to predicates"). As readers do:
//! qpdf takes an image without `/ImageMask` as no mask. Required and
//! IndirectReference take none: the model's author holds that a default
//! in a Required cell would hide a fault in the specification (upstream
//! issue #30). TestGrammar's `ReduceSCRow` takes defaults, and its
//! `ReducePVRow` doesn't.
//!
//! The version predicates, and `fn:Extension` with a statement, say when
//! the statement applies. Where it doesn't, it's vacant, and `&&` and `||`
//! go on without it, as TestGrammar's reduction does (its
//! `CPDFFile::ProcessPredicate`, Apache-2.0): `fn:IsPDFVersion(1.1,X) &&
//! fn:SinceVersion(1.7,Y)` is `Y` at 1.7 and asks nothing at 1.3. Each
//! column's reader says what a vacant predicate comes to there.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::num::ParseIntError;
use std::rc::Rc;

use pdf_objects::{Dict, Form, Object, ObjectId, Str, Value, Version};

use crate::grammar::{At, Table, Wrap, WrapFn, split, version};

/// The functions [`required`], [`holds`] and [`reference()`] evaluate;
/// `fn:RequiredValue` through [`required_value`]. Not `fn:IsFieldName`:
/// the model asks it, a partial field name, of a hide action's `/T`,
/// which ISO 32000-2 makes a fully qualified one, so that it would fail
/// every hide action naming a field below the top. The embedded model's
/// corrections put `fn:IsFullyQualifiedFieldName` in its place, which
/// this crate defines, not INTERNAL_GRAMMAR.md.
pub const SUPPORTED: &[&str] = &[
    "fn:AlwaysUnencrypted",
    "fn:ArrayLength",
    "fn:ArraySortAscending",
    "fn:BeforeVersion",
    "fn:BitClear",
    "fn:BitSet",
    "fn:BitsClear",
    "fn:BitsSet",
    "fn:Contains",
    "fn:Deprecated",
    "fn:Eval",
    "fn:Extension",
    "fn:FileSize",
    "fn:Ignore",
    "fn:ImageIsStructContentItem",
    "fn:InKeyMap",
    "fn:InNameTree",
    "fn:IsFullyQualifiedFieldName",
    "fn:IsHexString",
    "fn:IsMeaningful",
    "fn:IsPDFTagged",
    "fn:IsPDFVersion",
    "fn:IsPresent",
    "fn:IsRequired",
    "fn:MustBeDirect",
    "fn:MustBeIndirect",
    "fn:Not",
    "fn:NotStandard14Font",
    "fn:PageContainsStructContentItems",
    "fn:RectHeight",
    "fn:RectWidth",
    "fn:RequiredValue",
    "fn:SinceVersion",
    "fn:StringLength",
];

/// The standard 14 fonts' names (ISO 32000-2, 9.6.2.2), which
/// `fn:NotStandard14Font` reads.
pub const STANDARD_14: [&str; 14] = [
    "Courier",
    "Courier-Bold",
    "Courier-Oblique",
    "Courier-BoldOblique",
    "Helvetica",
    "Helvetica-Bold",
    "Helvetica-Oblique",
    "Helvetica-BoldOblique",
    "Times-Roman",
    "Times-Bold",
    "Times-Italic",
    "Times-BoldItalic",
    "Symbol",
    "ZapfDingbats",
];

/// How deep a predicate may nest, in brackets and calls. The model's
/// deepest is 5 brackets deep (the census test prints it).
const DEPTH: usize = 32;

/// How many references a value may go through to its object.
const HOPS: usize = 32;

/// How deep a name tree's `/Kids`, the structure tree, or the form's
/// fields, may go, and how many nodes each may have, before the rest of
/// it is left unread.
const TREE_DEPTH: usize = 32;
const TREE_NODES: usize = 100_000;

/// How near two numbers are equal: TestGrammar's `ArlNumberTolerance`,
/// half the fifth digit old PDF specifications recommended.
pub const TOLERANCE: f64 = 0.000005;

const AND: &str = " && ";
const OR: &str = " || ";

/// A predicate, or a part of one.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Expr {
    /// `fn:Name(a,b)`: the function, `fn:` and all, and its arguments,
    /// none, one or two.
    Call(String, Vec<Expr>),
    /// `(e)`.
    Group(Box<Expr>),
    /// `a && b && c`.
    And(Vec<Expr>),
    /// `a || b || c`.
    Or(Vec<Expr>),
    /// `a==b`, `a<=b`.
    Cmp(Cmp, Box<Expr>, Box<Expr>),
    /// `a+b`, `a - b`, `a * b`, `a mod b`, as written: evaluated with `*`
    /// and `mod` before `+` and `-`, each left to right.
    Arith(Box<Expr>, Vec<(Arith, Expr)>),
    /// A key, a path, or the value at one: `Key`, `A::B`, `@Key`, `*`. A
    /// bare word compared with a value is a name (`@Type==Template`).
    Path(Path),
    /// An integer: `3`, `-1`.
    Int(i64),
    /// A real as written: `1.5` is also a PDF version.
    Real(String),
    /// `'text'`.
    Str(String),
    /// `true` or `false`.
    Bool(bool),
    /// `null`.
    Null,
}

/// A comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    /// `==`.
    Eq,
    /// `!=`.
    Ne,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
}

/// An arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `mod`, the remainder.
    Mod,
}

/// A path through objects: keys and array indices between `::`, the
/// last maybe `@`, the value there (INTERNAL_GRAMMAR.md, "Validation of
/// predicates").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    /// Where it starts.
    pub base: Base,
    /// The keys and indices it goes through. `*` (or a repeating row's
    /// `1*`) first is the row's own value, as in `@*`; `*` last after
    /// others is any entry.
    pub keys: Vec<String>,
    /// Whether it's the value at the path (`@Key`) rather than the key.
    pub value: bool,
    /// As written.
    pub text: String,
}

/// Where a path starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// The object the row is in.
    Here,
    /// The object `n` steps back on the walk's way to it: `parent::` is 1.
    Parent(usize),
    /// The trailer. `trailer::Catalog` is `trailer::Root`, which `keys`
    /// has.
    Trailer,
}

/// What a predicate comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Truth {
    /// It holds.
    True,
    /// It doesn't.
    False,
    /// It can't be told, for the first thing in the way: a function the
    /// evaluator doesn't support (`fn:BitsClear`), or a value it can't read
    /// (`@Type`, which the object lacks, with no default to stand in).
    Unknown(String),
}

/// What a predicate is evaluated in: the row's place, the walk's way to
/// it, and the file.
pub struct Cx<'a> {
    /// The dictionary, array or stream the row's value is in. An indirect
    /// object is the one `resolve` gives, not a copy:
    /// `fn:ImageIsStructContentItem` and `fn:PageContainsStructContentItems`
    /// know the objects the structure tree points at by where `resolve`
    /// keeps them, and a copy is elsewhere.
    pub container: &'a Object,
    /// Where in it: the key or element the row matched, which `@*` and
    /// `@1*` read.
    pub at: At<'a>,
    /// The objects the walk came through to `container`, nearest first
    /// (`parent::`).
    pub parents: &'a [&'a Object],
    /// The trailer as written (`trailer::`).
    pub trailer: &'a Dict,
    /// An indirect object; none for one the file hasn't, which is null
    /// (ISO 32000-2, 7.3.10).
    pub resolve: &'a dyn Fn(ObjectId) -> Option<&'a Object>,
    /// The file's version: its header's, or its catalog's `/Version` when
    /// later.
    pub version: Version,
    /// The container's table, whose DefaultValues stand in for a key the
    /// container lacks or has null, read as `@Key` (`@ImageMask`): set
    /// for SpecialCase and PossibleValues, none for the other columns.
    pub defaults: Option<&'a Table>,
    /// The file's length in bytes, when the reader knows (`fn:FileSize`).
    pub size: Option<usize>,
    /// The file's name trees' keys, as `fn:InNameTree` reads them: each
    /// tree once.
    pub trees: &'a Trees,
    /// The structure tree's content items, as `fn:ImageIsStructContentItem`
    /// and `fn:PageContainsStructContentItems` read them: once.
    pub structure: &'a Structure,
    /// The form's fields' fully qualified names, as
    /// `fn:IsFullyQualifiedFieldName` reads them: once.
    pub fields: &'a Fields,
    /// The extensions the file is checked under, as the model names them
    /// (`fn:Extension`).
    pub extensions: &'a BTreeSet<String>,
}

/// Its parts but `resolve`, a closure, which can't be shown.
impl fmt::Debug for Cx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cx")
            .field("container", &self.container)
            .field("at", &self.at)
            .field("parents", &self.parents)
            .field("trailer", &self.trailer)
            .field("version", &self.version)
            .field("defaults", &self.defaults.map(|table| &table.name))
            .field("size", &self.size)
            .field("trees", &self.trees)
            .field("structure", &self.structure)
            .field("fields", &self.fields)
            .field("extensions", &self.extensions)
            .finish_non_exhaustive()
    }
}

/// The interactive form's fields' fully qualified names, read once a file.
#[derive(Debug, Default)]
pub struct Fields(RefCell<Option<Rc<Names>>>);

/// Fields' fully qualified names, as text, and whether they're all of
/// them: every field read, and every partial name a string whose text
/// the reader gave.
#[derive(Debug)]
struct Names {
    names: HashSet<String>,
    whole: bool,
}

/// The structure tree's content items, read once a file. It knows the
/// objects it found by where `resolve` keeps them, so it serves one file,
/// while its objects are kept.
#[derive(Debug, Default)]
pub struct Structure(RefCell<Option<Rc<Items>>>);

/// What the structure tree says of its content items (ISO 32000-2,
/// 14.7.5): the objects its object references (OBJR) point at, the pages
/// its marked-content items are on, and whether each is all of them,
/// every node read and every item placed.
#[derive(Debug)]
struct Items {
    objects: HashSet<usize>,
    pages: HashSet<usize>,
    whole_objects: bool,
    whole_pages: bool,
}

/// The keys of a file's name trees, read once each, by where `resolve`
/// keeps the tree's root: so it serves one file, while its objects are
/// kept.
#[derive(Debug, Default)]
pub struct Trees(RefCell<HashMap<usize, Rc<Tree>>>);

/// A name tree's keys, and whether they're all of them: every node read,
/// and every key a string whose bytes the reader gave.
#[derive(Debug)]
struct Tree {
    keys: HashSet<Vec<u8>>,
    whole: bool,
}

/// What a type's IndirectReference cell asks of its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// That it be direct.
    Direct,
    /// That it be an indirect reference.
    Indirect,
    /// Nothing.
    Either,
    /// It can't be told, as [`Truth::Unknown`].
    Unknown(String),
}

/// When a key came in: its SinceVersion cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Since {
    /// In this version.
    In(Version),
    /// With this extension, and in no version, the file not being checked
    /// under it (not among the extensions [`since`] is given).
    Extension(String),
    /// A cell the reader doesn't read.
    Unknown(String),
}

/// What the version predicates around a type, a value or a link make of
/// it at a file's version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Allowed at the file's version, under its extensions.
    Allowed,
    /// Allowed, and deprecated from this version, which the file's is at
    /// or past.
    AllowedDeprecated(Version),
    /// Not at the file's version, or not under its extensions: as if the
    /// model hadn't it.
    Excluded,
}

/// The Required cell (INTERNAL_GRAMMAR.md, "Column 5 - Required"): `TRUE`,
/// `FALSE`, or `fn:IsRequired(cond)`, required when `cond` is true and
/// optional when it's false, or when nothing in it applies at the file's
/// version.
pub fn required(cell: &str, cx: &Cx) -> Truth {
    match cell {
        "TRUE" => return Truth::True,
        "FALSE" => return Truth::False,
        _ => {}
    }
    let Ok(Expr::Call(f, args)) = parse(cell) else {
        return Truth::Unknown(cell.to_string());
    };
    match (f.as_str(), &args[..]) {
        ("fn:IsRequired", [cond]) => truth(cond, cx).truth(false),
        _ => Truth::Unknown(cell.to_string()),
    }
}

/// Whether a value meets a predicate: its SpecialCase (INTERNAL_GRAMMAR.md,
/// "Column 10 - SpecialCase": true valid, false invalid), or an `fn:Eval`
/// among its PossibleValues. Met when nothing in it applies at the file's
/// version.
pub fn holds(text: &str, cx: &Cx) -> Truth {
    match parse(text) {
        Ok(expr) => truth(&expr, cx).truth(true),
        Err(_) => Truth::Unknown(text.to_string()),
    }
}

/// What a type's IndirectReference cell asks (INTERNAL_GRAMMAR.md, "Column
/// 6 - IndirectReference"): `TRUE` an indirect reference, `FALSE` nothing,
/// `fn:MustBeDirect()` and `fn:MustBeIndirect()` that it be direct or
/// indirect, and with a condition, that when it holds; when it doesn't, or
/// nothing in it applies at the file's version, nothing.
pub fn reference(cell: &str, cx: &Cx) -> Reference {
    match cell {
        "TRUE" => return Reference::Indirect,
        "FALSE" => return Reference::Either,
        _ => {}
    }
    let Ok(Expr::Call(f, args)) = parse(cell) else {
        return Reference::Unknown(cell.to_string());
    };
    let must = match f.as_str() {
        "fn:MustBeDirect" => Reference::Direct,
        "fn:MustBeIndirect" => Reference::Indirect,
        _ => return Reference::Unknown(cell.to_string()),
    };
    let applies = match &args[..] {
        [] => Tri::Is(true),
        [cond] => truth(cond, cx),
        _ => return Reference::Unknown(cell.to_string()),
    };
    match applies {
        Tri::Is(true) => must,
        Tri::Is(false) | Tri::Vacant => Reference::Either,
        Tri::Unknown(why) => Reference::Unknown(why),
    }
}

/// The SinceVersion cell read (INTERNAL_GRAMMAR.md, "Column 3 -
/// SinceVersion"), under `extensions`: a version; `fn:Extension(X)` or
/// `fn:Extension(X,v)`, an extension's, in every version or from `v` when
/// X is one of `extensions`; or `fn:Eval(fn:Extension(X,v) || w)`, an
/// extension's from `v` and standard from `w`, the earlier of the two
/// when X is one of `extensions`, else `w` (TestGrammar without X in its
/// `--extensions`).
pub fn since(cell: &str, extensions: &BTreeSet<String>) -> Since {
    if let Some(v) = version(cell) {
        return Since::In(v);
    }
    let unread = || Since::Unknown(cell.to_string());
    let Ok(Expr::Call(f, args)) = parse(cell) else {
        return unread();
    };
    let under = |name: &Path| extensions.contains(&name.text);
    match (f.as_str(), &args[..]) {
        ("fn:Extension", [Expr::Path(name)]) if under(name) => Since::In(Version::new(1, 0)),
        ("fn:Extension", [Expr::Path(name)]) => Since::Extension(name.text.clone()),
        ("fn:Extension", [Expr::Path(name), Expr::Real(v)]) => match version(v) {
            Some(v) if under(name) => Since::In(v),
            Some(_) => Since::Extension(name.text.clone()),
            None => unread(),
        },
        ("fn:Eval", [Expr::Or(items)]) => match &items[..] {
            [Expr::Call(g, inner), Expr::Real(w)] if g == "fn:Extension" => {
                let Some(w) = version(w) else {
                    return unread();
                };
                match &inner[..] {
                    [Expr::Path(name), Expr::Real(v)] if under(name) => {
                        version(v).map_or_else(unread, |v| Since::In(v.min(w)))
                    }
                    _ => Since::In(w),
                }
            }
            _ => unread(),
        },
        _ => unread(),
    }
}

/// `wraps` at the file's version: `fn:SinceVersion`, `fn:BeforeVersion`
/// and `fn:IsPDFVersion` let the item in at their versions only;
/// `fn:Deprecated(v,X)` lets it in at every version, deprecated from `v`
/// on (INTERNAL_GRAMMAR.md, "Predicates (declarative functions)":
/// deprecation "is allowed/permitted but is strongly recommended
/// against"); and `fn:Extension` lets it in under one of `extensions`
/// only.
pub fn gate(wraps: &[Wrap], file: Version, extensions: &BTreeSet<String>) -> Gate {
    let mut deprecated: Option<Version> = None;
    for wrap in wraps {
        // The loader takes no wrap whose version doesn't read.
        let allowed = match (wrap.func, version(&wrap.arg)) {
            (WrapFn::SinceVersion, Some(v)) => file >= v,
            (WrapFn::BeforeVersion, Some(v)) => file < v,
            (WrapFn::IsPDFVersion, Some(v)) => file == v,
            (WrapFn::Deprecated, Some(v)) => {
                if file >= v {
                    deprecated = Some(deprecated.map_or(v, |d| d.min(v)));
                }
                true
            }
            (WrapFn::Extension, _) => extensions.contains(&wrap.arg),
            (_, None) => false,
        };
        if !allowed {
            return Gate::Excluded;
        }
    }
    deprecated.map_or(Gate::Allowed, Gate::AllowedDeprecated)
}

/// [`holds`], when every function in `text` is [`SUPPORTED`]: else
/// unknown, by the first that isn't, even where the rest would decide
/// it. TestGrammar takes a predicate it hasn't wholly implemented as no
/// verdict either way (its `WasFullyImplemented`, which ParseObjects.cpp's
/// `check_everything` reads); the checker's rules read SpecialCase and
/// PossibleValues so.
pub fn holds_fully(text: &str, cx: &Cx) -> Truth {
    match parse(text) {
        Ok(expr) => match unsupported(&expr) {
            Some(f) => Truth::Unknown(f.to_string()),
            None => truth(&expr, cx).truth(true),
        },
        Err(_) => Truth::Unknown(text.to_string()),
    }
}

/// A PossibleValues item `fn:RequiredValue(condition,value)`
/// (INTERNAL_GRAMMAR.md: the value "must by value when condition
/// evaluates to true"): whether the condition holds, read as
/// [`holds_fully`] reads a predicate, and the value, as the model writes
/// it. None for any other item.
pub fn required_value(text: &str, cx: &Cx) -> Option<(Truth, String)> {
    let Ok(Expr::Call(f, args)) = parse(text) else {
        return None;
    };
    let [condition, value] = &args[..] else {
        return None;
    };
    (f == "fn:RequiredValue").then(|| {
        let holds = match unsupported(condition) {
            Some(g) => Truth::Unknown(g.to_string()),
            None => truth(condition, cx).truth(false),
        };
        (holds, value.to_string())
    })
}

/// Whether a SpecialCase that's a note to readers, `fn:Ignore` or
/// `fn:IsMeaningful`, maybe under a version predicate, says they ignore
/// the row: `fn:Ignore` when its condition holds, or always;
/// `fn:IsMeaningful` when its condition doesn't. False where the note
/// doesn't apply at the file's version; none for any other SpecialCase.
pub fn ignored(text: &str, cx: &Cx) -> Option<Truth> {
    let expr = parse(text).ok()?;
    note(&expr, cx).map(|ignored| ignored.truth(false))
}

/// [`ignored`] of `e`: vacant where the note doesn't apply.
fn note(e: &Expr, cx: &Cx) -> Option<Tri> {
    let Expr::Call(f, args) = e else {
        return None;
    };
    // As [`holds_fully`] reads a predicate: unknown for one that calls a
    // function the evaluator doesn't support, whatever the rest says.
    let fully = |condition: &Expr| match unsupported(condition) {
        Some(g) => Tri::Unknown(g.to_string()),
        None => truth(condition, cx),
    };
    match (f.as_str(), &args[..]) {
        ("fn:Ignore", []) => Some(Tri::Is(true)),
        ("fn:Ignore", [condition]) => Some(fully(condition)),
        ("fn:IsMeaningful", [condition]) => Some(match fully(condition) {
            Tri::Is(meaningful) => Tri::Is(!meaningful),
            other => other,
        }),
        (g @ ("fn:SinceVersion" | "fn:BeforeVersion" | "fn:IsPDFVersion"), [v, inner]) => {
            let v = version_of(v)?;
            let applies = match g {
                "fn:SinceVersion" => cx.version >= v,
                "fn:BeforeVersion" => cx.version < v,
                _ => cx.version == v,
            };
            let ignored = note(inner, cx)?;
            Some(if applies { ignored } else { Tri::Vacant })
        }
        _ => None,
    }
}

/// The first function in `e` that isn't [`SUPPORTED`].
fn unsupported(e: &Expr) -> Option<&str> {
    match e {
        Expr::Call(f, _) if !SUPPORTED.contains(&f.as_str()) => Some(f),
        // A note to readers, whose condition never decides it.
        Expr::Call(f, _) if matches!(f.as_str(), "fn:Ignore" | "fn:IsMeaningful") => None,
        Expr::Call(_, args) | Expr::And(args) | Expr::Or(args) => args.iter().find_map(unsupported),
        Expr::Group(inner) => unsupported(inner),
        Expr::Cmp(_, a, b) => unsupported(a).or_else(|| unsupported(b)),
        Expr::Arith(first, rest) => {
            unsupported(first).or_else(|| rest.iter().find_map(|(_, e)| unsupported(e)))
        }
        Expr::Path(_)
        | Expr::Int(_)
        | Expr::Real(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Null => None,
    }
}

/// `text`, the whole of it, parsed. An [`Error`] says the predicate, then
/// what stopped it and where.
pub fn parse(text: &str) -> Result<Expr, Error> {
    let mut p = Parser {
        text,
        at: 0,
        depth: 0,
    };
    let expr = p.expr()?;
    if p.at != text.len() {
        return Err(Error::After {
            predicate: text.to_string(),
            rest: p.rest().to_string(),
        });
    }
    Ok(expr)
}

/// Why a predicate doesn't parse ([`parse`]): each says the predicate as
/// written, then what stopped the parser and where, a byte offset in it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Text after a whole predicate.
    #[error("{predicate:?}: {rest:?} after a whole predicate")]
    After {
        /// The predicate.
        predicate: String,
        /// The text after it.
        rest: String,
    },
    /// Not the bracket a call or a group wants.
    #[error("{predicate:?}: {token:?} expected at {at}, not {rest:?}")]
    Expected {
        /// The predicate.
        predicate: String,
        /// What was wanted: `(` or `)`.
        token: &'static str,
        /// Where.
        at: usize,
        /// The text from there.
        rest: String,
    },
    /// `&&` and `||` joining one list, which takes brackets
    /// (INTERNAL_GRAMMAR.md, "Validation of predicates").
    #[error("{predicate:?}: && and || mixed without brackets at {at}")]
    Mixed {
        /// The predicate.
        predicate: String,
        /// Where the other one comes.
        at: usize,
    },
    /// Terms nested deeper than the parser goes.
    #[error("{predicate:?}: nested deeper than {DEPTH} at {at}")]
    Nested {
        /// The predicate.
        predicate: String,
        /// Where the term too deep starts.
        at: usize,
    },
    /// A string that doesn't end.
    #[error("{predicate:?}: a string left open")]
    StringOpen {
        /// The predicate.
        predicate: String,
    },
    /// A string with nothing in it, which a predicate never has.
    #[error("{predicate:?}: an empty string at {at}")]
    EmptyString {
        /// The predicate.
        predicate: String,
        /// Where its text would be, past the opening quote.
        at: usize,
    },
    /// No term where one should be.
    #[error("{predicate:?}: a term expected at {at}, not {rest:?}")]
    Term {
        /// The predicate.
        predicate: String,
        /// Where.
        at: usize,
        /// The text from there.
        rest: String,
    },
    /// A `fn:` without a function's CamelCase name after it.
    #[error("{predicate:?}: fn: and a CamelCase name expected at {at}")]
    Call {
        /// The predicate.
        predicate: String,
        /// Where the name should be.
        at: usize,
    },
    /// An integer outside 64 bits.
    #[error("{predicate:?}: {word} is out of range: {error}")]
    Range {
        /// The predicate.
        predicate: String,
        /// The integer, as written.
        word: String,
        /// Why, as std says.
        error: ParseIntError,
    },
    /// A path that names no object: `trailer` alone.
    #[error("{predicate:?}: {path:?}: a path to nothing")]
    Nothing {
        /// The predicate.
        predicate: String,
        /// The path, as written.
        path: String,
    },
    /// A path through what isn't a key or an index.
    #[error("{predicate:?}: {path:?}: {key:?} isn't a key")]
    Key {
        /// The predicate.
        predicate: String,
        /// The path, as written.
        path: String,
        /// What isn't a key.
        key: String,
    },
}

/// [`Truth`] as evaluation carries it, with one more: vacant, a statement
/// that doesn't apply, which `&&` and `||` go on without.
enum Tri {
    Is(bool),
    Vacant,
    Unknown(String),
}

impl Tri {
    /// As a [`Truth`], `vacant` what a vacant one comes to.
    fn truth(self, vacant: bool) -> Truth {
        match self {
            Tri::Is(true) => Truth::True,
            Tri::Is(false) => Truth::False,
            Tri::Vacant if vacant => Truth::True,
            Tri::Vacant => Truth::False,
            Tri::Unknown(why) => Truth::Unknown(why),
        }
    }
}

/// What `e` comes to as a condition.
fn truth(e: &Expr, cx: &Cx) -> Tri {
    match e {
        Expr::Group(inner) => truth(inner, cx),
        Expr::And(items) => join(items, false, cx),
        Expr::Or(items) => join(items, true, cx),
        Expr::Cmp(op, a, b) => compare(*op, a, b, cx),
        Expr::Call(f, args) => call(f, args, cx),
        // `Key` means the key is present (INTERNAL_GRAMMAR.md, "Validation
        // of predicates").
        Expr::Path(p) if !p.value => present(p, cx),
        Expr::Bool(b) => Tri::Is(*b),
        Expr::Arith(..)
        | Expr::Path(_)
        | Expr::Int(_)
        | Expr::Real(_)
        | Expr::Str(_)
        | Expr::Null => Tri::Unknown(e.to_string()),
    }
}

/// `items` joined by `||` (`or`) or `&&`: decided by the first item that
/// decides it, true for `||` and false for `&&`; else unknown when one
/// is, by the first that is. Vacant items are left out, and when all are,
/// it's vacant.
fn join(items: &[Expr], or: bool, cx: &Cx) -> Tri {
    let mut unknown = None;
    let mut vacant = true;
    for item in items {
        match truth(item, cx) {
            Tri::Is(b) if b == or => return Tri::Is(or),
            Tri::Is(_) => vacant = false,
            Tri::Vacant => {}
            Tri::Unknown(why) => {
                unknown.get_or_insert(why);
            }
        }
    }
    match unknown {
        Some(why) => Tri::Unknown(why),
        None if vacant => Tri::Vacant,
        None => Tri::Is(!or),
    }
}

/// A function's truth: each in [`SUPPORTED`] as INTERNAL_GRAMMAR.md's
/// "Predicates (declarative functions)" defines it, any other unknown.
fn call(f: &str, args: &[Expr], cx: &Cx) -> Tri {
    match (f, args) {
        // From `v` on; or the statement, which applies from `v` on.
        ("fn:SinceVersion", [v, statement @ ..]) => {
            at_version(f, v, statement, cx, |file, v| file >= v)
        }
        // Before `v`; or the statement, which applies before `v` only.
        ("fn:BeforeVersion", [v, statement @ ..]) => {
            at_version(f, v, statement, cx, |file, v| file < v)
        }
        // At `v`; or the statement, which applies at `v` only.
        ("fn:IsPDFVersion", [v, statement @ ..]) => {
            at_version(f, v, statement, cx, |file, v| file == v)
        }
        // Under an extension the file is checked under, true, and the
        // statement under it applies; under another, false, and the
        // statement applies nowhere (TestGrammar without the extension in
        // its `--extensions`).
        ("fn:Extension", [Expr::Path(name)]) => Tri::Is(cx.extensions.contains(&name.text)),
        ("fn:Extension", [Expr::Path(name), statement]) => {
            match cx.extensions.contains(&name.text) {
                true => truth(statement, cx),
                false => Tri::Vacant,
            }
        }
        // The statement, deprecated from `v`, and so still allowed.
        ("fn:Deprecated", [v, statement]) if version_of(v).is_some() => truth(statement, cx),
        // The expression; the condition the key is required under; the
        // condition it must be direct or indirect under, always without
        // one.
        ("fn:Eval" | "fn:IsRequired", [e]) => truth(e, cx),
        ("fn:MustBeDirect" | "fn:MustBeIndirect", []) => Tri::Is(true),
        ("fn:MustBeDirect" | "fn:MustBeIndirect", [cond]) => truth(cond, cx),
        ("fn:Not", [e]) => match truth(e, cx) {
            Tri::Is(b) => Tri::Is(!b),
            other => other,
        },
        // With a condition: when the key is present, the condition; else
        // it asks nothing.
        ("fn:IsPresent", [k]) => is_present(k, cx),
        ("fn:IsPresent", [k, cond]) => match is_present(k, cx) {
            Tri::Is(true) => truth(cond, cx),
            Tri::Is(false) => Tri::Vacant,
            other => other,
        },
        ("fn:NotStandard14Font", []) => Tri::Is(not_standard_14(cx)),
        // A note to readers, which asks nothing of the value: the row is
        // ignored when the condition holds, or always, or only meaningful
        // when it holds ([`ignored`] says which).
        ("fn:Ignore", [] | [_]) | ("fn:IsMeaningful", [_]) => Tri::Is(true),
        ("fn:IsPDFTagged", []) => tagged(cx),
        ("fn:Contains", [k, v]) => contains(f, k, v, cx),
        ("fn:ArraySortAscending", [k, step]) => sorted(f, k, step, cx),
        ("fn:AlwaysUnencrypted", []) => unencrypted(f, cx),
        ("fn:IsHexString", []) => hex(f, cx),
        ("fn:InNameTree", [Expr::Path(tree)]) => in_name_tree(f, tree, cx),
        ("fn:ImageIsStructContentItem", []) => {
            let items = items(cx);
            content_item(f, &items.objects, items.whole_objects, cx)
        }
        ("fn:PageContainsStructContentItems", []) => {
            let items = items(cx);
            content_item(f, &items.pages, items.whole_pages, cx)
        }
        ("fn:InKeyMap", [Expr::Path(map)]) => in_key_map(f, map, cx),
        ("fn:IsFullyQualifiedFieldName", [Expr::Path(p)]) if p.value => field_name(f, p, cx),
        // A bit of the row's own value, or a run of them, clear or set.
        ("fn:BitClear" | "fn:BitSet", [bit]) => bits(f, bit, bit, cx),
        ("fn:BitsClear" | "fn:BitsSet", [low, high]) => bits(f, low, high, cx),
        _ => Tri::Unknown(f.to_string()),
    }
}

/// Whether the row's container is a Type 1 font (`/Type /Font /Subtype
/// /Type1`) whose `/BaseFont` isn't one of the standard 14 fonts' names
/// (ISO 32000-2, 9.6.2.2), as INTERNAL_GRAMMAR.md's `fn:NotStandard14Font`
/// asks.
fn not_standard_14(cx: &Cx) -> bool {
    let name = |key: &[u8]| value_at(cx.container, At::Key(key), cx)?.as_name();
    name(b"Type") == Some(b"Font")
        && name(b"Subtype") == Some(b"Type1")
        && !name(b"BaseFont").is_some_and(|base| STANDARD_14.iter().any(|f| f.as_bytes() == base))
}

/// Whether bits `low` to `high` of the row's own value, a bitmask whose
/// lowest bit is 1, are all clear (`f` `fn:BitClear` or `fn:BitsClear`)
/// or all set: false for a value that doesn't fit in 32 bits, which each
/// asserts it does, a negative one fitting as its two's complement.
fn bits(f: &str, low: &Expr, high: &Expr, cx: &Cx) -> Tri {
    let unknown = || Tri::Unknown(f.to_string());
    let (&Expr::Int(low), &Expr::Int(high)) = (low, high) else {
        return unknown();
    };
    let (Ok(low), Ok(high)) = (u32::try_from(low), u32::try_from(high)) else {
        return unknown();
    };
    if !(1 <= low && low <= high && high <= 32) {
        return unknown();
    }
    let Some(value) = value_at(cx.container, cx.at, cx).and_then(Object::as_integer) else {
        return unknown();
    };
    let value = u32::try_from(value)
        .ok()
        .or_else(|| i32::try_from(value).ok().map(|v| v as u32));
    let Some(value) = value else {
        return Tri::Is(false);
    };
    let mask = (u32::MAX >> (32 - (high - low + 1))) << (low - 1);
    Tri::Is(match f {
        "fn:BitClear" | "fn:BitsClear" => value & mask == 0,
        _ => value & mask == mask,
    })
}

/// A version predicate `f` of `v`: whether the file's version `applies`,
/// or with a statement, the statement where it applies.
fn at_version(
    f: &str,
    v: &Expr,
    statement: &[Expr],
    cx: &Cx,
    applies: fn(Version, Version) -> bool,
) -> Tri {
    let Some(v) = version_of(v) else {
        return Tri::Unknown(f.to_string());
    };
    match statement {
        [] => Tri::Is(applies(cx.version, v)),
        [statement] if applies(cx.version, v) => truth(statement, cx),
        [_] => Tri::Vacant,
        _ => Tri::Unknown(f.to_string()),
    }
}

/// `fn:IsPresent`'s first argument: a key or path, or an array's index
/// (INTERNAL_GRAMMAR.md, "Column 1 - Key": an annotation's color's
/// `fn:IsPresent(2)`), whether the object has it; else an expression, the
/// form that asks the row be present when the expression is true. For a
/// present row, the only kind a SpecialCase is read for, that comes to the
/// expression (TestGrammar reduces it so).
fn is_present(k: &Expr, cx: &Cx) -> Tri {
    match named(k) {
        Some(p) => present(&p, cx),
        None => truth(k, cx),
    }
}

/// `k` as a function's argument that names a key: a key or path, or an
/// array's index (`fn:StringLength(0)`).
fn named(k: &Expr) -> Option<Path> {
    match k {
        Expr::Path(p) if !p.value => Some(p.clone()),
        Expr::Int(i) if *i >= 0 => Some(Path {
            base: Base::Here,
            keys: vec![i.to_string()],
            value: false,
            text: i.to_string(),
        }),
        _ => None,
    }
}

/// Whether the object has what `p` names, null being absent (ISO 32000-2,
/// 7.3.7). A path ending in `*` asks for any entry of the dictionary or
/// array before it, and not of a stream's dictionary: the rows that ask
/// (an annotation's AS, `AP::N::*`) mean a dictionary of appearance
/// states, which INTERNAL_GRAMMAR.md's "Proposals for future predicates"
/// would write `fn:IsDictionary(AP::N)`.
fn present(p: &Path, cx: &Cx) -> Tri {
    let found = match p.keys.split_last() {
        Some((last, before)) if last == "*" && !before.is_empty() => {
            walk(p, before, cx).map(|o| o.is_some_and(|o| has_entry(o, cx)))
        }
        _ => walk(p, &p.keys, cx).map(|o| o.is_some()),
    };
    match found {
        Ok(found) => Tri::Is(found),
        Err(why) => Tri::Unknown(why),
    }
}

/// The object at the end of `keys` from `p`'s base: none when one of them
/// isn't there; an error, `p` as written, for a `*` past the start, which
/// only presence reads, and for a step that can't be told ([`step`]).
fn walk<'a>(p: &Path, keys: &[String], cx: &Cx<'a>) -> Result<Option<&'a Object>, String> {
    let (mut at, keys) = match (p.base, keys) {
        (Base::Here, [first, rest @ ..]) if wild(first) => {
            (value_at(cx.container, cx.at, cx), rest)
        }
        (Base::Here, _) => (Some(cx.container), keys),
        (Base::Parent(n), _) => (parent(n, keys, cx), keys),
        (Base::Trailer, [first, rest @ ..]) => {
            let entry = cx.trailer.get(first.as_bytes());
            (entry.and_then(|o| deref(o, cx)), rest)
        }
        (Base::Trailer, []) => return Err(p.text.clone()),
    };
    for key in keys {
        if wild(key) {
            return Err(p.text.clone());
        }
        at = match at {
            Some(o) => step(o, key, cx).ok_or_else(|| p.text.clone())?,
            None => None,
        };
    }
    Ok(at)
}

/// The object `n` steps back on the walk's way to the container
/// (`parent::`), and past any arrays there when the path goes on with a
/// key, not an index: an array holds no keys, and what a key means there
/// is what holds the array. An object reference in a structure element's
/// `/K` array has the array as its parent, but its `parent::Pg` is the
/// element's (ISO 32000-2, 14.7.5.3: "the page on which the object is
/// shown, if not the structure element's /Pg"), while a DeviceN
/// colourspace's attributes' `parent::1` indexes the array itself.
fn parent<'a>(n: usize, keys: &[String], cx: &Cx<'a>) -> Option<&'a Object> {
    let mut at = n.checked_sub(1)?;
    let by_key = keys
        .first()
        .is_some_and(|k| !wild(k) && k.parse::<usize>().is_err());
    while by_key && matches!(cx.parents.get(at)?.value(), Value::Array(_)) {
        at += 1;
    }
    cx.parents.get(at).copied()
}

/// Whether `key` is `*` or a repeating row's `1*`.
fn wild(key: &str) -> bool {
    key.ends_with('*')
}

/// `o`'s entry under `key`, an array's element when it's an index: none
/// when `o` is null or hasn't it. A name into an array, or any key into a
/// value that isn't an array, a dictionary or a stream, can't be told
/// (INTERNAL_GRAMMAR.md, "Parameters to predicates"): itself none.
fn step<'a>(o: &'a Object, key: &str, cx: &Cx<'a>) -> Option<Option<&'a Object>> {
    let at = match deref(o, cx).map(|o| o.value()) {
        None => return Some(None),
        Some(Value::Array(_)) => At::Index(key.parse().ok()?),
        Some(Value::Dict(_) | Value::Stream(_)) => At::Key(key.as_bytes()),
        Some(_) => return None,
    };
    Some(value_at(o, at, cx))
}

/// `o`'s entry at `at`, a stream's from its dictionary; none for null.
fn value_at<'a>(o: &'a Object, at: At, cx: &Cx<'a>) -> Option<&'a Object> {
    let entry = match (deref(o, cx)?.value(), at) {
        (Value::Dict(d), At::Key(key)) => d.get(key),
        (Value::Stream(s), At::Key(key)) => s.dict.get(key),
        (Value::Array(a), At::Index(i)) => a.get(i),
        _ => None,
    }?;
    deref(entry, cx)
}

/// `o` with its references followed, at most [`HOPS`]: none for null, or
/// for a reference to an object the file hasn't, which is null (ISO
/// 32000-2, 7.3.10).
fn deref<'a>(mut o: &'a Object, cx: &Cx<'a>) -> Option<&'a Object> {
    for _ in 0..HOPS {
        match o.value() {
            Value::Ref(id) => o = (cx.resolve)(*id)?,
            Value::Null => return None,
            _ => return Some(o),
        }
    }
    None
}

/// Whether `o` is a dictionary or array with an entry that isn't null.
fn has_entry<'a>(o: &'a Object, cx: &Cx<'a>) -> bool {
    match o.value() {
        Value::Dict(d) => d.iter().any(|(_, v)| deref(v, cx).is_some()),
        Value::Array(a) => a.iter().any(|v| deref(v, cx).is_some()),
        _ => false,
    }
}

/// A value a comparison reads.
enum Scalar {
    Int(i64),
    Real(f64),
    Name(Vec<u8>),
    Str(String),
    Bool(bool),
}

impl Scalar {
    fn number(&self) -> Option<f64> {
        match *self {
            Scalar::Int(i) => Some(i as f64),
            Scalar::Real(r) => Some(r),
            _ => None,
        }
    }
}

/// `a op b`: names, strings and booleans compared for equality, numbers
/// also in order, an integer standing for a number (INTERNAL_GRAMMAR.md,
/// "Validation of predicates"); unknown for anything else, or a side that
/// can't be read.
fn compare(op: Cmp, a: &Expr, b: &Expr, cx: &Cx) -> Tri {
    let (x, y) = match (operand(a, cx), operand(b, cx)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(why), _) | (_, Err(why)) => return Tri::Unknown(why),
    };
    let order = match (&x, &y) {
        (Scalar::Int(x), Scalar::Int(y)) => Some(x.cmp(y)),
        _ => (x.number().zip(y.number())).and_then(|(x, y)| x.partial_cmp(&y)),
    };
    let holds = match op {
        Cmp::Eq => equal(&x, &y),
        Cmp::Ne => equal(&x, &y).map(|equal| !equal),
        _ => order.map(|o| op.holds(o)),
    };
    holds.map_or_else(|| Tri::Unknown(format!("{a}{}{b}", op.symbol())), Tri::Is)
}

/// Whether `x` is `y`: names, strings and booleans alike, and numbers
/// within [`TOLERANCE`], an integer standing for a number; none for two
/// that don't compare.
fn equal(x: &Scalar, y: &Scalar) -> Option<bool> {
    match (x, y) {
        (Scalar::Int(x), Scalar::Int(y)) => Some(x == y),
        (Scalar::Name(x), Scalar::Name(y)) => Some(x == y),
        (Scalar::Str(x), Scalar::Str(y)) => Some(x == y),
        (Scalar::Bool(x), Scalar::Bool(y)) => Some(x == y),
        _ => (x.number().zip(y.number())).map(|(x, y)| (x - y).abs() <= TOLERANCE),
    }
}

/// What `e` comes to as a comparison's side, or what's in the way.
fn operand(e: &Expr, cx: &Cx) -> Result<Scalar, String> {
    match e {
        Expr::Group(inner) => operand(inner, cx),
        Expr::Int(i) => Ok(Scalar::Int(*i)),
        Expr::Real(r) => r.parse().map(Scalar::Real).map_err(|_| r.clone()),
        Expr::Str(s) => Ok(Scalar::Str(s.clone())),
        Expr::Bool(b) => Ok(Scalar::Bool(*b)),
        // A bare word compared is a name: `@Type==Template`.
        Expr::Path(p) if !p.value => match (p.base, &p.keys[..]) {
            (Base::Here, [name]) => Ok(Scalar::Name(name.as_bytes().to_vec())),
            _ => Err(p.text.clone()),
        },
        Expr::Path(p) => match walk(p, &p.keys, cx)? {
            Some(o) => scalar(o),
            None => default(p, cx),
        }
        .ok_or_else(|| p.text.clone()),
        Expr::Call(f, args) => match (f.as_str(), &args[..]) {
            ("fn:Eval", [e]) => operand(e, cx),
            // The rectangle's width and height, its corners any two
            // opposite ones (ISO 32000-2, 7.9.5).
            ("fn:RectWidth", [k]) => {
                rect(k, cx).map(|[x0, _, x1, _]| Scalar::Real((x1 - x0).abs()))
            }
            ("fn:RectHeight", [k]) => {
                rect(k, cx).map(|[_, y0, _, y1]| Scalar::Real((y1 - y0).abs()))
            }
            ("fn:ArrayLength", [k]) => array(k, cx).map(|a| Scalar::Int(a.len() as i64)),
            ("fn:StringLength", [k]) => string_length(k, cx),
            ("fn:FileSize", []) => (cx.size)
                .and_then(|n| i64::try_from(n).ok())
                .map(Scalar::Int)
                .ok_or_else(|| f.clone()),
            _ => Err(f.clone()),
        },
        Expr::Arith(first, rest) => arithmetic(e, first, rest, cx),
        Expr::And(_) | Expr::Or(_) | Expr::Cmp(..) | Expr::Null => Err(e.to_string()),
    }
}

/// `o` as a comparison reads it: none for an array, a dictionary or a
/// stream.
fn scalar(o: &Object) -> Option<Scalar> {
    Some(match o.value() {
        Value::Integer(i) => Scalar::Int(*i),
        Value::Real(r) => Scalar::Real(*r),
        Value::Name(name) => Scalar::Name(name.clone()),
        Value::String(s, _) => Scalar::Str(s.text()?),
        Value::Bool(b) => Scalar::Bool(*b),
        _ => return None,
    })
}

/// The rectangle at `k`, a key or path: four numbers.
fn rect(k: &Expr, cx: &Cx) -> Result<[f64; 4], String> {
    let numbers: Option<Vec<f64>> = (array(k, cx)?.iter())
        .map(|o| deref(o, cx).and_then(|o| scalar(o)?.number()))
        .collect();
    numbers
        .and_then(|n| <[f64; 4]>::try_from(n).ok())
        .ok_or_else(|| k.to_string())
}

/// The array at `k`, a key or path.
fn array<'a>(k: &Expr, cx: &Cx<'a>) -> Result<&'a [Object], String> {
    match k {
        Expr::Path(p) if !p.value => match walk(p, &p.keys, cx)?.map(|o| o.value()) {
            Some(Value::Array(a)) => Ok(a),
            _ => Err(p.text.clone()),
        },
        Expr::Call(f, _) => Err(f.clone()),
        _ => Err(k.to_string()),
    }
}

/// The length in bytes of the string at `k`, a key, path or index; what
/// stands in the way for a string whose bytes the reader didn't give.
fn string_length(k: &Expr, cx: &Cx) -> Result<Scalar, String> {
    let p = named(k).ok_or_else(|| k.to_string())?;
    match walk(&p, &p.keys, cx)?.map(|o| o.value()) {
        Some(Value::String(Str::Bytes(bytes), _)) => Ok(Scalar::Int(bytes.len() as i64)),
        Some(Value::String(..)) => Err("the string's bytes".to_string()),
        _ => Err(p.text.clone()),
    }
}

/// Whether the value at `k` is `v`, or an array holding it
/// (`fn:Contains`, which the model asks of a stream's `/Filter`, a name or
/// an array of them). An array's elements that aren't simple values hold
/// nothing, as in TestGrammar's `fn_Contains`.
fn contains(f: &str, k: &Expr, v: &Expr, cx: &Cx) -> Tri {
    let Expr::Path(p) = k else {
        return Tri::Unknown(f.to_string());
    };
    let want = match operand(v, cx) {
        Ok(want) => want,
        Err(why) => return Tri::Unknown(why),
    };
    let found = match walk(p, &p.keys, cx) {
        Ok(Some(o)) => o,
        Ok(None) => {
            return match default(p, cx) {
                Some(d) => Tri::Is(equal(&d, &want) == Some(true)),
                None => Tri::Unknown(p.text.clone()),
            };
        }
        Err(why) => return Tri::Unknown(why),
    };
    let held = |o: &Object| scalar(o).and_then(|s| equal(&s, &want)) == Some(true);
    Tri::Is(match found.value() {
        Value::Array(a) => a.iter().any(|e| deref(e, cx).is_some_and(held)),
        _ => held(found),
    })
}

/// Whether every `step`-th element of the array at `k`, from the first,
/// is a number no less than the one before (`fn:ArraySortAscending`):
/// false for one that isn't a number, and an empty array sorted.
fn sorted(f: &str, k: &Expr, step: &Expr, cx: &Cx) -> Tri {
    let step = match step {
        Expr::Int(n) if *n >= 1 => usize::try_from(*n).ok(),
        _ => None,
    };
    let Some(step) = step else {
        return Tri::Unknown(f.to_string());
    };
    let elements = match array(k, cx) {
        Ok(elements) => elements,
        Err(why) => return Tri::Unknown(why),
    };
    let numbers: Option<Vec<f64>> = (elements.iter().step_by(step))
        .map(|o| deref(o, cx).and_then(|o| scalar(o)?.number()))
        .collect();
    Tri::Is(numbers.is_some_and(|n| n.windows(2).all(|w| w[0] <= w[1])))
}

/// Whether the row's own value is a string the file wrote unencrypted
/// (`fn:AlwaysUnencrypted`). The rows that ask are strings no reader
/// decrypts (the trailer's `/ID`, the encryption dictionary's), so any
/// string as read is as written, but ciphertext the reader didn't
/// decrypt, which can't be told; anything else isn't a string.
fn unencrypted(f: &str, cx: &Cx) -> Tri {
    match value_at(cx.container, cx.at, cx).map(|o| o.value()) {
        Some(Value::String(Str::Encrypted(_), _)) => Tri::Unknown("the string's bytes".to_string()),
        Some(Value::String(..)) => Tri::Is(true),
        Some(_) => Tri::Is(false),
        None => Tri::Unknown(f.to_string()),
    }
}

/// Whether the row's own value is a string written as a hex string
/// (`fn:IsHexString`), by its [`Form`]: unknown when it has none, as a
/// string built without choosing, or read by a reader that doesn't say.
fn hex(f: &str, cx: &Cx) -> Tri {
    match value_at(cx.container, cx.at, cx).map(|o| o.value()) {
        Some(Value::String(_, Some(form))) => Tri::Is(*form == Form::Hex),
        Some(Value::String(_, None)) | None => Tri::Unknown(f.to_string()),
        Some(_) => Tri::Is(false),
    }
}

/// Whether the row's own value, a string, is a key of the name tree at
/// `tree` (`fn:InNameTree`): false when the row is absent, or the file has
/// no such tree. A row that isn't a string isn't what the function asks
/// of (INTERNAL_GRAMMAR.md: "which must be a PDF string"), and can't be
/// told; nor can one whose bytes the reader didn't give, or one the tree's
/// readable keys don't hold when some of it doesn't read.
fn in_name_tree(f: &str, tree: &Path, cx: &Cx) -> Tri {
    let key = match value_at(cx.container, cx.at, cx).map(|o| o.value()) {
        None => return Tri::Is(false),
        Some(Value::String(Str::Bytes(key), _)) => key,
        Some(Value::String(..)) => return Tri::Unknown("the string's bytes".to_string()),
        Some(_) => return Tri::Unknown(f.to_string()),
    };
    let root = match walk(tree, &tree.keys, cx) {
        Ok(Some(root)) => root,
        Ok(None) => return Tri::Is(false),
        Err(why) => return Tri::Unknown(why),
    };
    let address = std::ptr::from_ref(root) as usize;
    let known = cx.trees.0.borrow().get(&address).cloned();
    let tree = known.unwrap_or_else(|| {
        let read = Rc::new(name_tree(root, cx));
        cx.trees.0.borrow_mut().insert(address, Rc::clone(&read));
        read
    });
    match (tree.keys.contains(key), tree.whole) {
        (true, _) => Tri::Is(true),
        (false, true) => Tri::Is(false),
        (false, false) => Tri::Unknown(f.to_string()),
    }
}

/// The keys of the name tree at `root` (ISO 32000-2, 7.9.6): each node's
/// `/Names` pairs' first, through its `/Kids`, each node once, as deep and
/// as many as [`TREE_DEPTH`] and [`TREE_NODES`] let.
fn name_tree<'a>(root: &'a Object, cx: &Cx<'a>) -> Tree {
    let mut tree = Tree {
        keys: HashSet::new(),
        whole: true,
    };
    let mut seen = HashSet::new();
    let mut nodes = vec![(root, 0)];
    while let Some((node, depth)) = nodes.pop() {
        if depth > TREE_DEPTH || seen.len() >= TREE_NODES {
            tree.whole = false;
            continue;
        }
        if !seen.insert(std::ptr::from_ref(node) as usize) {
            continue;
        }
        let Value::Dict(node) = node.value() else {
            tree.whole = false;
            continue;
        };
        let array = |key: &[u8]| node.get(key).and_then(|o| deref(o, cx)).map(|o| o.value());
        match array(b"Names") {
            Some(Value::Array(names)) => {
                for key in names.iter().step_by(2) {
                    match deref(key, cx).map(|o| o.value()) {
                        Some(Value::String(Str::Bytes(key), _)) => {
                            tree.keys.insert(key.clone());
                        }
                        _ => tree.whole = false,
                    }
                }
            }
            None => {}
            Some(_) => tree.whole = false,
        }
        match array(b"Kids") {
            Some(Value::Array(kids)) => {
                for kid in kids {
                    match deref(kid, cx) {
                        Some(kid) => nodes.push((kid, depth + 1)),
                        None => tree.whole = false,
                    }
                }
            }
            None => {}
            Some(_) => tree.whole = false,
        }
    }
    tree
}

/// Whether the string at `p` is the fully qualified name of a field of
/// the file's interactive form (`fn:IsFullyQualifiedFieldName`, which
/// this crate defines for a hide action's `/T`, ISO 32000-2 12.6.4: "the
/// fully qualified field name of an interactive form field"), as text,
/// however each is encoded. A value that isn't a string isn't what the
/// function asks of, and can't be told; nor can one whose text the
/// reader didn't give, or one the form's readable fields don't name when
/// some of it doesn't read. A file without a form has no fields.
fn field_name(f: &str, p: &Path, cx: &Cx) -> Tri {
    let name = match walk(p, &p.keys, cx).map(|o| o.map(|o| o.value())) {
        Ok(Some(Value::String(s, _))) => match s.text() {
            Some(name) => name,
            None => return Tri::Unknown("the string's bytes".to_string()),
        },
        Ok(Some(_)) => return Tri::Unknown(f.to_string()),
        Ok(None) => return Tri::Unknown(p.text.clone()),
        Err(why) => return Tri::Unknown(why),
    };
    let known = cx.fields.0.borrow().clone();
    let fields = known.unwrap_or_else(|| {
        let read = Rc::new(field_names(cx));
        *cx.fields.0.borrow_mut() = Some(Rc::clone(&read));
        read
    });
    match (fields.names.contains(&name), fields.whole) {
        (true, _) => Tri::Is(true),
        (false, true) => Tri::Is(false),
        (false, false) => Tri::Unknown(f.to_string()),
    }
}

/// The fully qualified names of the fields of the catalog's `/AcroForm`
/// (ISO 32000-2, 12.7.4.2): from each field in its `/Fields` down through
/// `/Kids`, each one's partial name, `/T`, after its parent's and a
/// period; one without `/T` has its parent's ("different representations
/// of the same underlying field"), and one at the top without it, none.
/// Each node once, as deep and as many as [`TREE_DEPTH`] and
/// [`TREE_NODES`] let.
fn field_names<'a>(cx: &Cx<'a>) -> Names {
    let mut names = Names {
        names: HashSet::new(),
        whole: true,
    };
    let fields = (cx.trailer.get(b"Root").and_then(|o| deref(o, cx)))
        .and_then(|catalog| value_at(catalog, At::Key(b"AcroForm"), cx))
        .and_then(|form| value_at(form, At::Key(b"Fields"), cx));
    let Some(fields) = fields else {
        return names;
    };
    let mut seen = HashSet::new();
    // Each node, its parent's name, and its depth.
    let mut nodes: Vec<(&'a Object, Option<Rc<str>>, usize)> = vec![(fields, None, 0)];
    while let Some((node, parent, depth)) = nodes.pop() {
        if depth > TREE_DEPTH || seen.len() >= TREE_NODES {
            names.whole = false;
            continue;
        }
        if !seen.insert(std::ptr::from_ref(node) as usize) {
            continue;
        }
        let field = match node.value() {
            Value::Array(kids) => {
                for kid in kids {
                    match deref(kid, cx) {
                        Some(kid) => nodes.push((kid, parent.clone(), depth + 1)),
                        None => names.whole = false,
                    }
                }
                continue;
            }
            Value::Dict(field) => field,
            _ => {
                names.whole = false;
                continue;
            }
        };
        let name = match field
            .get(b"T")
            .and_then(|t| deref(t, cx))
            .map(|t| t.value())
        {
            None => parent,
            Some(Value::String(t, _)) => match t.text() {
                Some(t) => Some(match parent {
                    Some(parent) => format!("{parent}.{t}").into(),
                    None => t.into(),
                }),
                // Neither it nor the fields under it can be named.
                None => {
                    names.whole = false;
                    continue;
                }
            },
            Some(_) => {
                names.whole = false;
                continue;
            }
        };
        if let Some(name) = &name {
            names.names.insert(name.to_string());
        }
        if let Some(kids) = field.get(b"Kids").and_then(|k| deref(k, cx)) {
            nodes.push((kids, name, depth + 1));
        }
    }
    names
}

/// Whether the row's container is one of `found`, an object the
/// structure tree points at or a page it puts marked content on: false
/// when it isn't and `whole` says they're all there are, else unknown.
fn content_item(f: &str, found: &HashSet<usize>, whole: bool, cx: &Cx) -> Tri {
    match (
        found.contains(&(std::ptr::from_ref(cx.container) as usize)),
        whole,
    ) {
        (true, _) => Tri::Is(true),
        (false, true) => Tri::Is(false),
        (false, false) => Tri::Unknown(f.to_string()),
    }
}

/// The structure tree's content items ([`Items`]), read the first time
/// they're asked for.
fn items(cx: &Cx) -> Rc<Items> {
    let known = cx.structure.0.borrow().clone();
    known.unwrap_or_else(|| {
        let read = Rc::new(structure(cx));
        *cx.structure.0.borrow_mut() = Some(Rc::clone(&read));
        read
    })
}

/// The content items of the structure tree from the catalog's
/// `/StructTreeRoot` (ISO 32000-2, 14.7.2 and 14.7.5): a structure
/// element's kids, through its `/K`, are elements, marked-content
/// identifiers (an integer, on the element's `/Pg`), marked-content
/// references (MCR, on their own `/Pg`, else the element's; one in a
/// form XObject's stream, `/Stm`, isn't on the page) and object references
/// (OBJR, to their `/Obj`). An MCR or OBJR is told by its `/Type`, or
/// without one by its `/MCID` or `/Obj` where an element would have `/S`.
/// A file without a structure tree has no content items.
fn structure<'a>(cx: &Cx<'a>) -> Items {
    let mut items = Items {
        objects: HashSet::new(),
        pages: HashSet::new(),
        whole_objects: true,
        whole_pages: true,
    };
    let address = |o: &Object| std::ptr::from_ref(o) as usize;
    let Some(root) = (cx.trailer.get(b"Root").and_then(|o| deref(o, cx)))
        .and_then(|catalog| value_at(catalog, At::Key(b"StructTreeRoot"), cx))
    else {
        return items;
    };
    let mut seen = HashSet::new();
    // Each node, the page of the element it's a kid of, and its depth.
    let mut nodes: Vec<(&'a Object, Option<usize>, usize)> = Vec::new();
    if let Some(kids) = value_at(root, At::Key(b"K"), cx) {
        nodes.push((kids, None, 0));
    }
    while let Some((node, page, depth)) = nodes.pop() {
        if depth > TREE_DEPTH || seen.len() >= TREE_NODES {
            items.whole_objects = false;
            items.whole_pages = false;
            continue;
        }
        match node.value() {
            Value::Array(kids) => {
                for kid in kids {
                    if let Some(kid) = deref(kid, cx) {
                        nodes.push((kid, page, depth + 1));
                    }
                }
            }
            // A marked-content identifier, on the element's page.
            Value::Integer(_) => match page {
                Some(page) => {
                    items.pages.insert(page);
                }
                None => items.whole_pages = false,
            },
            Value::Dict(d) => {
                if !seen.insert(address(node)) {
                    continue;
                }
                let name = |key: &[u8]| d.get(key).and_then(|o| deref(o, cx)?.as_name());
                let has = |key: &[u8]| d.get(key).and_then(|o| deref(o, cx)).is_some();
                let own_page = d.get(b"Pg").and_then(|o| deref(o, cx)).map(address);
                let element = !has(b"S");
                if name(b"Type") == Some(b"MCR") || (element && has(b"MCID")) {
                    if has(b"Stm") {
                        continue;
                    }
                    match own_page.or(page) {
                        Some(page) => {
                            items.pages.insert(page);
                        }
                        None => items.whole_pages = false,
                    }
                } else if name(b"Type") == Some(b"OBJR") || (element && has(b"Obj")) {
                    match d.get(b"Obj").and_then(|o| deref(o, cx)) {
                        Some(object) => {
                            items.objects.insert(address(object));
                        }
                        None => items.whole_objects = false,
                    }
                } else if let Some(kids) = d.get(b"K").and_then(|o| deref(o, cx)) {
                    nodes.push((kids, own_page, depth + 1));
                }
            }
            _ => {}
        }
    }
    items
}

/// Whether the row's own value, a name, is a key of the dictionary at
/// `map` (`fn:InKeyMap`, which the model asks of a destination's name in
/// the catalog's `/Dests`): false when the row is absent, or the file has
/// no such dictionary; and a row that isn't a name can't be told.
fn in_key_map(f: &str, map: &Path, cx: &Cx) -> Tri {
    let key = match value_at(cx.container, cx.at, cx).map(|o| o.value()) {
        None => return Tri::Is(false),
        Some(Value::Name(key)) => key,
        Some(_) => return Tri::Unknown(f.to_string()),
    };
    match walk(map, &map.keys, cx).map(|o| o.map(|o| o.value())) {
        Ok(Some(Value::Dict(map))) => Tri::Is(map.get(key).and_then(|o| deref(o, cx)).is_some()),
        Ok(None) => Tri::Is(false),
        Ok(Some(_)) => Tri::Unknown(f.to_string()),
        Err(why) => Tri::Unknown(why),
    }
}

/// Whether the file is a Tagged PDF (`fn:IsPDFTagged`): its catalog's
/// `/MarkInfo /Marked` true, which is false when absent (ISO 32000-2,
/// 14.7.1).
fn tagged(cx: &Cx) -> Tri {
    let marked = Path {
        base: Base::Trailer,
        keys: ["Root", "MarkInfo", "Marked"].map(String::from).to_vec(),
        value: true,
        text: "trailer::Catalog::MarkInfo::@Marked".to_string(),
    };
    match walk(&marked, &marked.keys, cx) {
        Ok(o) => Tri::Is(o.is_some_and(|o| matches!(o.value(), Value::Bool(true)))),
        Err(why) => Tri::Unknown(why),
    }
}

/// The DefaultValue that stands in for the key `p` names when the
/// container lacks it ([`Cx::defaults`]): a constant, the one of a key
/// whose types have one between them. None for a path or `@*`, or for a
/// default that's an array, another key's value, or a predicate
/// (INTERNAL_GRAMMAR.md, "Column 8 - DefaultValue"), as for a key with no
/// default.
fn default(p: &Path, cx: &Cx) -> Option<Scalar> {
    let table = cx.defaults?;
    let ([key], Base::Here) = (&p.keys[..], p.base) else {
        return None;
    };
    if wild(key) {
        return None;
    }
    let at = match key.parse() {
        Ok(i) => At::Index(i),
        Err(_) => At::Key(key.as_bytes()),
    };
    // Not a wildcard row's, which has no default of the key's own.
    let row = table.row(at).filter(|row| row.key == *key)?;
    let text = match row.r#type.len() {
        1 => row.default_value.as_str(),
        // `[];[None]`: a default for each type, or none.
        _ => {
            let parts = split(&row.default_value, ';').ok()?;
            let mut given = (parts.iter())
                .map(|part| part.strip_prefix('[')?.strip_suffix(']'))
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .filter(|part| !part.is_empty());
            let first = given.next()?;
            if !given.all(|part| part == first) {
                return None;
            }
            first
        }
    };
    match parse(text).ok()? {
        Expr::Int(i) => Some(Scalar::Int(i)),
        Expr::Real(r) => r.parse().ok().map(Scalar::Real),
        Expr::Bool(b) => Some(Scalar::Bool(b)),
        Expr::Str(s) => Some(Scalar::Str(s)),
        Expr::Path(name) if !name.value && name.base == Base::Here => match &name.keys[..] {
            [name] if !wild(name) => Some(Scalar::Name(name.as_bytes().to_vec())),
            _ => None,
        },
        _ => None,
    }
}

/// A number arithmetic reads and makes.
#[derive(Debug, Clone, Copy)]
enum Num {
    Int(i64),
    Real(f64),
}

/// `whole`, `first` and the operators and operands after it, as a
/// number: `*` and `mod` before `+` and `-`, each left to right. Integers
/// stay integers and a real makes a real, as TestGrammar's
/// `ProcessPredicate` types them. `mod` takes integers, a real that's
/// whole standing for one, and its remainder has the sign of what's
/// divided, as there. What's in the way is the error: an operand that
/// can't be read, or isn't a number; or `whole`, for an integer that
/// overflows, a `mod` of a fraction, or of 0.
fn arithmetic(
    whole: &Expr,
    first: &Expr,
    rest: &[(Arith, Expr)],
    cx: &Cx,
) -> Result<Scalar, String> {
    let number = |e: &Expr| match operand(e, cx)? {
        Scalar::Int(i) => Ok(Num::Int(i)),
        Scalar::Real(r) => Ok(Num::Real(r)),
        _ => Err(e.to_string()),
    };
    let apply = |op: Arith, a: Num, b: Num| -> Result<Num, String> {
        let integer = |n: Num| match n {
            Num::Int(i) => Some(i),
            // Within i64, exactly: `as` saturates past it.
            Num::Real(r) if r.fract() == 0.0 && r.abs() < 9.0e15 => Some(r as i64),
            Num::Real(_) => None,
        };
        let real = |n: Num| match n {
            Num::Int(i) => i as f64,
            Num::Real(r) => r,
        };
        let done = match (op, a, b) {
            (Arith::Mod, a, b) => integer(a)
                .zip(integer(b))
                .and_then(|(a, b)| a.checked_rem(b))
                .map(Num::Int),
            (Arith::Add, Num::Int(a), Num::Int(b)) => a.checked_add(b).map(Num::Int),
            (Arith::Sub, Num::Int(a), Num::Int(b)) => a.checked_sub(b).map(Num::Int),
            (Arith::Mul, Num::Int(a), Num::Int(b)) => a.checked_mul(b).map(Num::Int),
            (Arith::Add, a, b) => Some(Num::Real(real(a) + real(b))),
            (Arith::Sub, a, b) => Some(Num::Real(real(a) - real(b))),
            (Arith::Mul, a, b) => Some(Num::Real(real(a) * real(b))),
        };
        done.ok_or_else(|| whole.to_string())
    };
    // Each term's sign, and its value with `*` and `mod` applied.
    let mut terms = vec![(Arith::Add, number(first)?)];
    for (op, e) in rest {
        let n = number(e)?;
        match op {
            Arith::Mul | Arith::Mod => {
                let (_, term) = terms.last_mut().expect("never empty");
                *term = apply(*op, *term, n)?;
            }
            Arith::Add | Arith::Sub => terms.push((*op, n)),
        }
    }
    let (_, mut total) = terms[0];
    for &(op, n) in &terms[1..] {
        total = apply(op, total, n)?;
    }
    Ok(match total {
        Num::Int(i) => Scalar::Int(i),
        Num::Real(r) => Scalar::Real(r),
    })
}

/// `e`'s version, when it's a version as the model writes one.
fn version_of(e: &Expr) -> Option<Version> {
    match e {
        Expr::Real(text) => version(text),
        _ => None,
    }
}

impl Cmp {
    /// Every comparison, the longer of those that start alike first.
    const ALL: [Cmp; 6] = [Cmp::Eq, Cmp::Ne, Cmp::Le, Cmp::Ge, Cmp::Lt, Cmp::Gt];

    /// As the model writes it: `==`, `<=`.
    pub fn symbol(self) -> &'static str {
        match self {
            Cmp::Eq => "==",
            Cmp::Ne => "!=",
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
        }
    }

    /// Whether it holds of two values in `order`.
    fn holds(self, order: Ordering) -> bool {
        match self {
            Cmp::Eq => order.is_eq(),
            Cmp::Ne => order.is_ne(),
            Cmp::Lt => order.is_lt(),
            Cmp::Le => order.is_le(),
            Cmp::Gt => order.is_gt(),
            Cmp::Ge => order.is_ge(),
        }
    }
}

impl Arith {
    const ALL: [Arith; 4] = [Arith::Add, Arith::Sub, Arith::Mul, Arith::Mod];

    /// As the model writes it: `-` and `*` spaced, to tell them from a
    /// negative number and a wildcard, and `mod` (INTERNAL_GRAMMAR.md,
    /// "Predicates (declarative functions)").
    pub fn symbol(self) -> &'static str {
        match self {
            Arith::Add => "+",
            Arith::Sub => " - ",
            Arith::Mul => " * ",
            Arith::Mod => " mod ",
        }
    }
}

/// A predicate as the model writes it, which every one in the model
/// prints back as.
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let join = |f: &mut fmt::Formatter, items: &[Expr], sep: &str| -> fmt::Result {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    f.write_str(sep)?;
                }
                write!(f, "{item}")?;
            }
            Ok(())
        };
        match self {
            Expr::Call(name, args) => {
                write!(f, "{name}(")?;
                join(f, args, ",")?;
                f.write_str(")")
            }
            Expr::Group(inner) => write!(f, "({inner})"),
            Expr::And(items) => join(f, items, AND),
            Expr::Or(items) => join(f, items, OR),
            Expr::Cmp(op, a, b) => write!(f, "{a}{}{b}", op.symbol()),
            Expr::Arith(first, rest) => {
                write!(f, "{first}")?;
                for (op, e) in rest {
                    write!(f, "{}{e}", op.symbol())?;
                }
                Ok(())
            }
            Expr::Path(p) => f.write_str(&p.text),
            Expr::Int(i) => write!(f, "{i}"),
            Expr::Real(text) => f.write_str(text),
            Expr::Str(s) => write!(f, "'{s}'"),
            Expr::Bool(b) => write!(f, "{b}"),
            Expr::Null => f.write_str("null"),
        }
    }
}

/// A recursive descent over a predicate's text, as INTERNAL_GRAMMAR.md's
/// "Predicates (declarative functions)" spaces it: a space each side of
/// `&&`, `||`, `-`, `*` and `mod`, and nowhere else.
struct Parser<'s> {
    text: &'s str,
    at: usize,
    /// How many terms the parser is inside.
    depth: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.text[self.at..]
    }

    fn eat(&mut self, token: &str) -> bool {
        let ate = self.rest().starts_with(token);
        if ate {
            self.at += token.len();
        }
        ate
    }

    /// The one of `ops` the text goes on with.
    fn peek<T: Copy, const N: usize>(
        &self,
        ops: [T; N],
        symbol: fn(T) -> &'static str,
    ) -> Option<T> {
        ops.into_iter()
            .find(|&op| self.rest().starts_with(symbol(op)))
    }

    fn expect(&mut self, token: &'static str) -> Result<(), Error> {
        if !self.eat(token) {
            return Err(Error::Expected {
                predicate: self.text.to_string(),
                token,
                at: self.at,
                rest: self.rest().to_string(),
            });
        }
        Ok(())
    }

    /// Comparisons joined by `&&` or by `||`: mixing them takes brackets
    /// (INTERNAL_GRAMMAR.md, "Validation of predicates").
    fn expr(&mut self) -> Result<Expr, Error> {
        let first = self.comparison()?;
        let (op, other, joined): (_, _, fn(Vec<Expr>) -> Expr) = match self.rest() {
            rest if rest.starts_with(AND) => (AND, OR, Expr::And),
            rest if rest.starts_with(OR) => (OR, AND, Expr::Or),
            _ => return Ok(first),
        };
        let mut items = vec![first];
        while self.eat(op) {
            items.push(self.comparison()?);
        }
        if self.rest().starts_with(other) {
            return Err(Error::Mixed {
                predicate: self.text.to_string(),
                at: self.at,
            });
        }
        Ok(joined(items))
    }

    /// Arithmetic, maybe compared.
    fn comparison(&mut self) -> Result<Expr, Error> {
        let left = self.arith()?;
        let Some(op) = self.peek(Cmp::ALL, Cmp::symbol) else {
            return Ok(left);
        };
        self.at += op.symbol().len();
        let right = self.arith()?;
        Ok(Expr::Cmp(op, Box::new(left), Box::new(right)))
    }

    /// Terms joined by arithmetic.
    fn arith(&mut self) -> Result<Expr, Error> {
        let first = self.term()?;
        let mut rest = Vec::new();
        while let Some(op) = self.peek(Arith::ALL, Arith::symbol) {
            self.at += op.symbol().len();
            rest.push((op, self.term()?));
        }
        Ok(if rest.is_empty() {
            first
        } else {
            Expr::Arith(Box::new(first), rest)
        })
    }

    /// A bracketed expression, a call, a string or a word, inside no more
    /// than [`DEPTH`] others.
    fn term(&mut self) -> Result<Expr, Error> {
        if self.depth >= DEPTH {
            return Err(Error::Nested {
                predicate: self.text.to_string(),
                at: self.at,
            });
        }
        self.depth += 1;
        let term = self.term_within();
        self.depth -= 1;
        term
    }

    fn term_within(&mut self) -> Result<Expr, Error> {
        if self.eat("(") {
            let inner = self.expr()?;
            self.expect(")")?;
            return Ok(Expr::Group(Box::new(inner)));
        }
        if self.eat("fn:") {
            return self.call();
        }
        let text = self.text;
        if self.eat("'") {
            // Never empty, and without escapes (TestGrammar's `ArlString`).
            let rest = &text[self.at..];
            let end = rest.find('\'').ok_or_else(|| Error::StringOpen {
                predicate: text.to_string(),
            })?;
            if end == 0 {
                return Err(Error::EmptyString {
                    predicate: text.to_string(),
                    at: self.at,
                });
            }
            self.at += end + 1;
            return Ok(Expr::Str(rest[..end].to_string()));
        }
        let rest = &text[self.at..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || "_.-*@:".contains(c)))
            .unwrap_or(rest.len());
        if len == 0 {
            return Err(Error::Term {
                predicate: text.to_string(),
                at: self.at,
                rest: rest.to_string(),
            });
        }
        self.at += len;
        word(text, &rest[..len])
    }

    /// `Name(...)`, after a call's `fn:`: none, one or two arguments
    /// (INTERNAL_GRAMMAR.md, "Validation of predicates").
    fn call(&mut self) -> Result<Expr, Error> {
        let text = self.text;
        let rest = &text[self.at..];
        let len = (rest.find(|c: char| !c.is_ascii_alphanumeric())).unwrap_or(rest.len());
        let name = &rest[..len];
        if !name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return Err(Error::Call {
                predicate: text.to_string(),
                at: self.at,
            });
        }
        self.at += len;
        self.expect("(")?;
        let mut args = Vec::new();
        if !self.eat(")") {
            args.push(self.expr()?);
            if self.eat(",") {
                args.push(self.expr()?);
            }
            self.expect(")")?;
        }
        Ok(Expr::Call(format!("fn:{name}"), args))
    }
}

/// A word of `predicate`: `true`, `false` or `null`, an integer, a real,
/// or else a path.
fn word(predicate: &str, word: &str) -> Result<Expr, Error> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let unsigned = word.strip_prefix('-').unwrap_or(word);
    let real = unsigned
        .split_once('.')
        .is_some_and(|(whole, fraction)| digits(whole) && digits(fraction));
    Ok(match word {
        "true" => Expr::Bool(true),
        "false" => Expr::Bool(false),
        "null" => Expr::Null,
        _ if digits(unsigned) => Expr::Int(word.parse().map_err(|error| Error::Range {
            predicate: predicate.to_string(),
            word: word.to_string(),
            error,
        })?),
        _ if real => Expr::Real(word.to_string()),
        _ => Expr::Path(path(predicate, word)?),
    })
}

/// `A::B::@C`, a word of `predicate`, as a path: `parent::`s or a
/// `trailer::` first, then keys, which only `parent` (the object itself)
/// may go without.
fn path(predicate: &str, text: &str) -> Result<Path, Error> {
    let mut keys: Vec<&str> = text.split("::").collect();
    let mut value = false;
    if let Some(last) = keys.last_mut() {
        if let Some(key) = last.strip_prefix('@') {
            *last = key;
            value = true;
        }
    }
    let parents = keys.iter().take_while(|&&k| k == "parent").count();
    let (base, skip) = match keys[0] {
        _ if parents > 0 => (Base::Parent(parents), parents),
        "trailer" => (Base::Trailer, 1),
        _ => (Base::Here, 0),
    };
    let mut keys: Vec<String> = keys[skip..].iter().map(|k| k.to_string()).collect();
    if base == Base::Trailer && keys.first().is_some_and(|k| k == "Catalog") {
        keys[0] = "Root".to_string();
    }
    let parent = matches!(base, Base::Parent(_));
    if keys.is_empty() && !parent {
        return Err(Error::Nothing {
            predicate: predicate.to_string(),
            path: text.to_string(),
        });
    }
    if let Some(key) = keys.iter().find(|k| !key_like(k)) {
        return Err(Error::Key {
            predicate: predicate.to_string(),
            path: text.to_string(),
            key: key.clone(),
        });
    }
    Ok(Path {
        base,
        keys,
        value,
        text: text.to_string(),
    })
}

/// Whether a path may go through `key`: a key or an index, `*`, or a
/// repeating row's `1*` (INTERNAL_GRAMMAR.md, "Column 1 - Key"); never
/// `parent` or `trailer` past the path's start.
fn key_like(key: &str) -> bool {
    let plain =
        !key.is_empty() && (key.bytes()).all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b));
    let repeat = (key.strip_suffix('*')).is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()));
    !matches!(key, "parent" | "trailer") && (plain || repeat)
}

#[cfg(test)]
mod tests {
    //! The predicate language: every predicate in the model parses and
    //! prints back as written, with a census of its functions; and each
    //! supported function means what INTERNAL_GRAMMAR.md says, on the
    //! model's own cells where it has them.

    use std::collections::BTreeMap;

    use pdf_objects::{Dict, dict};

    use super::*;
    use crate::grammar::{self, Grammar, Row, model, split};

    /// A version the model names.
    fn v(text: &str) -> Version {
        version(text).unwrap()
    }

    /// Every predicate in the model, read from its tables as they are:
    /// where it is (table, line and column) and its text. A cell is split
    /// into its types' groups and those into their items
    /// (INTERNAL_GRAMMAR.md, "TSV Data Fields"); Key holds none, and Note
    /// is prose.
    fn predicates() -> Vec<(String, String)> {
        let mut found = Vec::new();
        for &(name, text) in model::TABLES {
            for (i, line) in text.lines().enumerate().skip(1) {
                let cells: Vec<&str> = line.split('\t').collect();
                for (column, cell) in grammar::HEADER.iter().zip(&cells).take(11).skip(1) {
                    for group in split(cell, ';').unwrap() {
                        let inner = group.strip_prefix('[').and_then(|g| g.strip_suffix(']'));
                        for item in split(inner.unwrap_or(group), ',').unwrap() {
                            if item.contains("fn:") {
                                found
                                    .push((format!("{name}:{} {column}", i + 1), item.to_string()));
                            }
                        }
                    }
                }
            }
        }
        found
    }

    /// Every call in `e`, by function, counted into `census`.
    fn calls(e: &Expr, census: &mut BTreeMap<String, usize>) {
        match e {
            Expr::Call(name, args) => {
                *census.entry(name.clone()).or_default() += 1;
                args.iter().for_each(|a| calls(a, census));
            }
            Expr::Group(inner) => calls(inner, census),
            Expr::And(items) | Expr::Or(items) => items.iter().for_each(|i| calls(i, census)),
            Expr::Cmp(_, a, b) => {
                calls(a, census);
                calls(b, census);
            }
            Expr::Arith(first, rest) => {
                calls(first, census);
                rest.iter().for_each(|(_, e)| calls(e, census));
            }
            Expr::Path(_)
            | Expr::Int(_)
            | Expr::Real(_)
            | Expr::Str(_)
            | Expr::Bool(_)
            | Expr::Null => {}
        }
    }

    /// How deeply `text` nests its brackets.
    fn nesting(text: &str) -> usize {
        let (mut depth, mut deepest, mut quoted) = (0usize, 0, false);
        for c in text.chars() {
            match c {
                '\'' => quoted = !quoted,
                '(' if !quoted => {
                    depth += 1;
                    deepest = deepest.max(depth);
                }
                ')' if !quoted => depth -= 1,
                _ => {}
            }
        }
        deepest
    }

    #[test]
    fn every_predicate_in_the_model_parses() {
        let all = predicates();
        let mut census = BTreeMap::new();
        let mut failed = Vec::new();
        for (at, text) in &all {
            match parse(text) {
                Ok(e) => {
                    assert_eq!(e.to_string(), *text, "{at}: it prints back as written");
                    calls(&e, &mut census);
                }
                Err(e) => failed.push(format!("{at}: {e:#}")),
            }
        }
        assert!(
            failed.is_empty(),
            "{} of {} predicates don't parse:\n{}",
            failed.len(),
            all.len(),
            failed.join("\n")
        );
        // Every `fn:` in the model's cells is a call in one of them.
        let mut written = 0;
        for &(_, text) in model::TABLES {
            for line in text.lines().skip(1) {
                let cells = line.split('\t').take(11).skip(1);
                written += cells.map(|cell| cell.matches("fn:").count()).sum::<usize>();
            }
        }
        assert_eq!(census.values().sum::<usize>(), written);
        for &name in SUPPORTED {
            assert!(
                census.contains_key(name),
                "{name} is supported but the model has none"
            );
        }

        let deepest = all.iter().map(|(_, text)| nesting(text)).max().unwrap();
        println!(
            "Arlington predicates: {} in the model's cells, {written} calls of {} functions, nested at most {deepest} deep",
            all.len(),
            census.len()
        );
        let mut by_count: Vec<_> = census.iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (name, n) in by_count {
            let supported = if SUPPORTED.contains(&name.as_str()) {
                "supported"
            } else {
                "unsupported: unknown, counted"
            };
            println!("  {n:>5}  {name:<34} {supported}");
        }
    }

    #[test]
    fn every_since_required_and_indirect_reference_cell_has_a_form_its_reader_reads() {
        let mut since_in = BTreeMap::new();
        let mut extensions = 0;
        for table in Grammar::embedded().tables() {
            for row in &table.rows {
                let at = format!("{}:{}", table.name, row.line);
                match since(&row.since_version, &none()) {
                    Since::In(v) => *since_in.entry(v).or_insert(0) += 1,
                    Since::Extension(_) => extensions += 1,
                    Since::Unknown(cell) => panic!("{at}: SinceVersion {cell:?} doesn't read"),
                }
                if !matches!(row.required.as_str(), "TRUE" | "FALSE") {
                    let call = parse(&row.required).unwrap();
                    assert!(
                        matches!(&call, Expr::Call(f, args) if f == "fn:IsRequired" && args.len() == 1),
                        "{at}: Required {call}"
                    );
                }
                for cell in &row.indirect_reference {
                    if !matches!(cell.as_str(), "TRUE" | "FALSE") {
                        let call = parse(cell).unwrap();
                        let Expr::Call(f, args) = &call else {
                            panic!("{at}: IndirectReference {call}");
                        };
                        assert!(
                            matches!(f.as_str(), "fn:MustBeDirect" | "fn:MustBeIndirect")
                                && args.len() <= 1,
                            "{at}: IndirectReference {call}"
                        );
                    }
                }
            }
        }
        let by_version: Vec<String> = (since_in.iter()).map(|(v, n)| format!("{v} {n}")).collect();
        println!(
            "SinceVersion: {extensions} keys an extension's; by version, {}",
            by_version.join(", ")
        );
    }

    /// The trailer and objects of a file, for predicates to read; its
    /// length, when it says.
    struct File {
        trailer: Dict,
        objects: BTreeMap<ObjectId, Object>,
        size: Option<usize>,
        /// The extensions it's checked under.
        extensions: BTreeSet<String>,
    }

    /// No extensions.
    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    impl File {
        fn new(trailer: Dict, objects: impl IntoIterator<Item = (u32, Object)>) -> File {
            let objects = objects.into_iter().map(|(n, o)| ((n, 0), o)).collect();
            File {
                trailer,
                objects,
                size: None,
                extensions: BTreeSet::new(),
            }
        }

        /// What `f` says of the row at `at` in `container`, reached through
        /// `parents`, in this file at `version`.
        fn eval<T>(
            &self,
            container: &Object,
            at: At,
            parents: &[&Object],
            version: &str,
            f: impl FnOnce(&Cx) -> T,
        ) -> T {
            self.eval_with(None, container, at, parents, version, f)
        }

        /// [`File::eval`] with `defaults`, as for a SpecialCase.
        fn eval_with<T>(
            &self,
            defaults: Option<&Table>,
            container: &Object,
            at: At,
            parents: &[&Object],
            version: &str,
            f: impl FnOnce(&Cx) -> T,
        ) -> T {
            let resolve = |id: ObjectId| self.objects.get(&id);
            let trees = Trees::default();
            let structure = Structure::default();
            let fields = Fields::default();
            f(&Cx {
                container,
                at,
                parents,
                trailer: &self.trailer,
                resolve: &resolve,
                version: v(version),
                defaults,
                size: self.size,
                trees: &trees,
                structure: &structure,
                fields: &fields,
                extensions: &self.extensions,
            })
        }
    }

    /// A file with nothing but a catalog, object 1.
    fn bare() -> File {
        let catalog = dict! { "Type" => "Catalog" }.into();
        File::new(dict! { "Root" => Object::reference(1) }, [(1, catalog)])
    }

    /// The model's row `key` of `table`.
    fn row(table: &str, key: &[u8]) -> &'static Row {
        let table = Grammar::embedded().table(table).unwrap();
        table.row(At::Key(key)).unwrap()
    }

    /// `text` held of `container` at `version` in `file`.
    fn holds_in(file: &File, container: &Object, version: &str, text: &str) -> Truth {
        file.eval(container, At::Key(b"X"), &[], version, |cx| holds(text, cx))
    }

    /// `cell` as Required of `container` at `version` in `file`.
    fn required_in(file: &File, container: &Object, version: &str, cell: &str) -> Truth {
        file.eval(container, At::Key(b"X"), &[], version, |cx| {
            required(cell, cx)
        })
    }

    fn unknown(why: &str) -> Truth {
        Truth::Unknown(why.to_string())
    }

    fn integers(n: impl IntoIterator<Item = i64>) -> Object {
        n.into_iter().map(Object::from).collect::<Vec<_>>().into()
    }

    #[test]
    fn is_present_reads_keys_paths_from_the_trailer_and_any_entry() {
        let catalog = dict! { "Type" => "Catalog", "PieceInfo" => dict! {} }.into();
        let file = File::new(dict! { "Root" => Object::reference(1) }, [(1, catalog)]);
        let here = dict! {
            "Null" => Object::null(),
            "Gone" => Object::reference(9),
            "Kept" => 1,
        }
        .into();
        let present = |text: &str| holds_in(&file, &here, "1.7", text);
        assert_eq!(
            present("fn:IsPresent(trailer::Catalog::PieceInfo)"),
            Truth::True
        );
        assert_eq!(
            present("fn:IsPresent(trailer::Root::PieceInfo)"),
            Truth::True,
            "Root is Catalog"
        );
        assert_eq!(
            present("fn:IsPresent(trailer::Info::ModDate)"),
            Truth::False
        );
        assert_eq!(present("fn:IsPresent(Kept)"), Truth::True);
        assert_eq!(
            present("fn:IsPresent(Null)"),
            Truth::False,
            "null is absent"
        );
        assert_eq!(
            present("fn:IsPresent(Gone)"),
            Truth::False,
            "a reference to no object is null"
        );
        assert_eq!(present("fn:Not(fn:IsPresent(Gone))"), Truth::True);

        // An annotation's color has a third element if it has a second: an
        // array's element by its index.
        let colors = Grammar::embedded().table("ArrayOf_4NumbersColorAnnotation");
        let second = &colors.unwrap().row(At::Index(1)).unwrap().special_case[0];
        for (color, met) in [(vec![0, 1], Truth::False), (vec![0, 1, 0], Truth::True)] {
            let color = integers(color);
            let is = file.eval(&color, At::Index(1), &[], "1.7", |cx| holds(second, cx));
            assert_eq!(is, met, "{second}: {color:?}");
        }

        // An annotation's AS is required when its appearances are
        // dictionaries of states, not when they're one stream each.
        let appearance = || Object::stream(dict! { "Subtype" => "Form" });
        let annot = |n: Object| Object::from(dict! { "AP" => dict! { "N" => n } });
        let states = annot(dict! { "Off" => appearance(), "On" => appearance() }.into());
        let one = annot(appearance());
        let empty = annot(dict! { "Off" => Object::null() }.into());
        let as_ = &row("AnnotSquare", b"AS").required;
        assert_eq!(required_in(&file, &states, "1.7", as_), Truth::True);
        assert_eq!(required_in(&file, &one, "1.7", as_), Truth::False);
        assert_eq!(
            required_in(&file, &empty, "1.7", as_),
            Truth::False,
            "a null entry is none"
        );

        // The trailer's Info, from 1.4 with a catalog's PieceInfo: its
        // ModDate. Without PieceInfo, or before 1.4, it asks nothing.
        let info = &row("FileTrailer", b"Info").special_case[0];
        let with = |info_dict: Dict, catalog: Dict| {
            File::new(
                dict! { "Root" => Object::reference(1), "Info" => Object::reference(2) },
                [(1, catalog.into()), (2, info_dict.into())],
            )
        };
        let pieces = || dict! { "PieceInfo" => dict! {} };
        let dated = || dict! { "ModDate" => Object::string("D:20261024") };
        for (file, version, met) in [
            (with(dated(), pieces()), "1.4", Truth::True),
            (with(dict! {}, pieces()), "1.4", Truth::False),
            (with(dict! {}, pieces()), "1.3", Truth::True),
            (with(dict! {}, dict! {}), "1.7", Truth::True),
        ] {
            let trailer = Object::from(file.trailer.clone());
            assert_eq!(
                holds_in(&file, &trailer, version, info),
                met,
                "at {version}"
            );
        }
    }

    #[test]
    fn the_trailers_info_is_required_from_1_4_when_the_catalog_has_piece_info() {
        let info = &row("FileTrailer", b"Info").required;
        let pieces = dict! { "Type" => "Catalog", "PieceInfo" => dict! {} }.into();
        let file = File::new(dict! { "Root" => Object::reference(1) }, [(1, pieces)]);
        let trailer = Object::from(file.trailer.clone());
        assert_eq!(required_in(&file, &trailer, "1.3", info), Truth::False);
        assert_eq!(required_in(&file, &trailer, "1.4", info), Truth::True);
        let bare = bare();
        let trailer = Object::from(bare.trailer.clone());
        assert_eq!(required_in(&bare, &trailer, "1.4", info), Truth::False);
    }

    #[test]
    fn an_annotations_appearance_is_required_from_2_0_unless_its_rect_is_a_point() {
        let ap = &row("AnnotSquare", b"AP").required;
        let file = bare();
        let square = |rect: [i64; 4]| {
            Object::from(dict! { "Subtype" => "Square", "Rect" => integers(rect) })
        };
        let wide = square([0, 0, 100, 50]);
        assert_eq!(required_in(&file, &wide, "1.7", ap), Truth::False);
        assert_eq!(required_in(&file, &wide, "2.0", ap), Truth::True);
        let point = square([5, 5, 5, 5]);
        assert_eq!(required_in(&file, &point, "2.0", ap), Truth::False);
        let zero_width = square([5, 0, 5, 10]);
        assert_eq!(
            required_in(&file, &zero_width, "2.0", ap),
            Truth::True,
            "it has a height"
        );
        let turned = square([100, 50, 0, 0]);
        assert_eq!(
            required_in(&file, &turned, "2.0", ap),
            Truth::True,
            "any two opposite corners"
        );
        let unboxed = Object::from(dict! { "Subtype" => "Square" });
        assert_eq!(required_in(&file, &unboxed, "2.0", ap), unknown("Rect"));
    }

    #[test]
    fn a_pages_parent_is_required_unless_it_is_a_template() {
        let parent = row("PageObject", b"Parent");
        let file = bare();
        let page = |ty: &str| Object::from(dict! { "Type" => ty });
        assert_eq!(
            required_in(&file, &page("Page"), "1.7", &parent.required),
            Truth::True
        );
        assert_eq!(
            required_in(&file, &page("Template"), "1.7", &parent.required),
            Truth::False
        );
        let untyped = Object::from(dict! {});
        assert_eq!(
            required_in(&file, &untyped, "1.7", &parent.required),
            unknown("@Type")
        );
        // And a template's shall not be present: fn:Not(fn:IsPresent(cond)).
        let special = &parent.special_case[0];
        assert_eq!(holds_in(&file, &page("Page"), "1.7", special), Truth::True);
        assert_eq!(
            holds_in(&file, &page("Template"), "1.7", special),
            Truth::False
        );
    }

    #[test]
    fn version_predicates_hold_at_their_edges() {
        let file = bare();
        let x = Object::from(dict! { "X" => 1 });
        let req = |version: &str, cell: &str| required_in(&file, &x, version, cell);
        for (version, is) in [("1.6", false), ("1.7", true), ("2.0", false)] {
            assert_eq!(
                req(version, "fn:IsRequired(fn:IsPDFVersion(1.7))") == Truth::True,
                is,
                "{version}"
            );
        }
        for (version, is) in [("1.2", true), ("1.3", false)] {
            assert_eq!(
                req(version, "fn:IsRequired(fn:BeforeVersion(1.3))") == Truth::True,
                is,
                "{version}"
            );
        }
        for (version, is) in [("1.4", false), ("1.5", true)] {
            assert_eq!(
                req(version, "fn:IsRequired(fn:SinceVersion(1.5))") == Truth::True,
                is,
                "{version}"
            );
        }
        // A statement where it doesn't apply asks nothing: no requirement,
        // and nothing to meet.
        let statement = "fn:IsRequired(fn:SinceVersion(2.0,fn:IsPresent(X)))";
        assert_eq!(req("1.7", statement), Truth::False);
        assert_eq!(req("2.0", statement), Truth::True);
        let before = "fn:BeforeVersion(2.0,fn:Not(fn:IsPresent(X)))";
        assert_eq!(holds_in(&file, &x, "1.7", before), Truth::False);
        assert_eq!(holds_in(&file, &x, "2.0", before), Truth::True);
        // An annotation's F: the bits each version leaves clear, none before
        // 1.3. ToggleNoView, bit 9, came in 1.5.
        let f = &row("AnnotSquare", b"F").special_case[0];
        let met = |version: &str, flags: i64| {
            let annot = Object::from(dict! { "F" => flags });
            file.eval(&annot, At::Key(b"F"), &[], version, |cx| holds(f, cx))
        };
        for (version, toggle) in [
            ("1.2", true),
            ("1.3", false),
            ("1.4", false),
            ("1.5", true),
            ("2.0", true),
        ] {
            assert_eq!(met(version, 4), Truth::True, "{version}: Print");
            let toggle = if toggle { Truth::True } else { Truth::False };
            assert_eq!(met(version, 1 << 8), toggle, "{version}: ToggleNoView");
        }
    }

    #[test]
    fn an_extension_and_then_a_version_is_since_the_version() {
        assert_eq!(since("1.5", &none()), Since::In(v("1.5")));
        assert_eq!(
            since(&row("CollectionSplit", b"Direction").since_version, &none()),
            Since::In(v("2.0"))
        );
        assert_eq!(
            since("fn:Eval(fn:Extension(X,1.7) || 2.0)", &none()),
            Since::In(v("2.0"))
        );
        assert_eq!(
            since(&row("FileTrailer", b"AuthCode").since_version, &none()),
            Since::Extension("ISO_TS_32004".to_string())
        );
        assert_eq!(
            since("fn:Extension(AAPL)", &none()),
            Since::Extension("AAPL".to_string())
        );
        assert_eq!(
            since("2.1", &none()),
            Since::Unknown("2.1".to_string()),
            "not a version the model has"
        );
        // No extension is allowed: false, and a statement under one asks
        // nothing.
        let file = bare();
        let x = Object::from(dict! {});
        assert_eq!(
            required_in(&file, &x, "2.0", "fn:IsRequired(fn:Extension(PDF_VT2))"),
            Truth::False
        );
        let under = "fn:Extension(ETSI_PAdES,fn:IsPresent(Cert))";
        assert_eq!(holds_in(&file, &x, "2.0", under), Truth::True);

        // Under extensions the file is checked under, a key is theirs from
        // their version, or from the earlier of theirs and the standard's;
        // a predicate under one is true and its statement applies.
        let claimed = |names: &[&str]| names.iter().map(|n| n.to_string()).collect();
        let x_and_aapl: BTreeSet<String> = claimed(&["X", "AAPL"]);
        let either = "fn:Eval(fn:Extension(X,1.7) || 2.0)";
        assert_eq!(since(either, &x_and_aapl), Since::In(v("1.7")));
        assert_eq!(
            since("fn:Extension(AAPL)", &x_and_aapl),
            Since::In(v("1.0"))
        );
        assert_eq!(
            since("fn:Extension(AAPL,1.6)", &x_and_aapl),
            Since::In(v("1.6"))
        );
        assert_eq!(
            since("fn:Extension(Y,1.6)", &x_and_aapl),
            Since::Extension("Y".to_string())
        );
        let file = File {
            extensions: claimed(&["PDF_VT2", "ETSI_PAdES"]),
            ..bare()
        };
        assert_eq!(
            required_in(&file, &x, "2.0", "fn:IsRequired(fn:Extension(PDF_VT2))"),
            Truth::True
        );
        assert_eq!(holds_in(&file, &x, "2.0", under), Truth::False);
    }

    #[test]
    fn deprecated_allows_what_it_marks() {
        let values = &row("EncryptionStandard", b"V").possible_values[0];
        let two = values.iter().find(|w| w.item == "2").unwrap();
        assert_eq!(
            gate(&two.wraps, v("1.3"), &none()),
            Gate::Excluded,
            "since 1.4"
        );
        assert_eq!(gate(&two.wraps, v("1.7"), &none()), Gate::Allowed);
        assert_eq!(
            gate(&two.wraps, v("2.0"), &none()),
            Gate::AllowedDeprecated(v("2.0"))
        );
        let zero = values.iter().find(|w| w.item == "0").unwrap();
        assert_eq!(gate(&zero.wraps, v("1.0"), &none()), Gate::Allowed);
        assert_eq!(
            gate(&zero.wraps, v("1.7"), &none()),
            Gate::AllowedDeprecated(v("1.1")),
            "deprecated in 1.1"
        );

        let wrap = |func, arg: &str| Wrap {
            func,
            arg: arg.to_string(),
        };
        let from = [wrap(WrapFn::SinceVersion, "1.4")];
        assert_eq!(gate(&from, v("1.3"), &none()), Gate::Excluded);
        assert_eq!(gate(&from, v("1.4"), &none()), Gate::Allowed);
        let before = [wrap(WrapFn::BeforeVersion, "1.3")];
        assert_eq!(gate(&before, v("1.2"), &none()), Gate::Allowed);
        assert_eq!(gate(&before, v("1.3"), &none()), Gate::Excluded);
        let only = [wrap(WrapFn::IsPDFVersion, "1.2")];
        assert_eq!(gate(&only, v("1.1"), &none()), Gate::Excluded);
        assert_eq!(gate(&only, v("1.2"), &none()), Gate::Allowed);
        assert_eq!(gate(&only, v("1.3"), &none()), Gate::Excluded);
        assert_eq!(gate(&[], v("1.0"), &none()), Gate::Allowed);
        let twice = [
            wrap(WrapFn::Deprecated, "1.7"),
            wrap(WrapFn::Deprecated, "1.4"),
        ];
        assert_eq!(
            gate(&twice, v("1.5"), &none()),
            Gate::AllowedDeprecated(v("1.4"))
        );
        assert_eq!(
            gate(&twice, v("2.0"), &none()),
            Gate::AllowedDeprecated(v("1.4"))
        );
        // An extension's link, at its version or not, is no version's.
        let annots = Grammar::embedded()
            .table("ArrayOfAnnots")
            .unwrap()
            .row(At::Index(0))
            .unwrap();
        let projection = annots.link[0]
            .iter()
            .find(|l| l.item == "AnnotProjection")
            .unwrap();
        assert_eq!(gate(&projection.wraps, v("1.7"), &none()), Gate::Excluded);
        // Unless it's checked under the extension, at the link's version.
        let adbe = BTreeSet::from(["ADBE_Extn3".to_string()]);
        assert_eq!(gate(&projection.wraps, v("1.7"), &adbe), Gate::Allowed);
        assert_eq!(gate(&projection.wraps, v("1.6"), &adbe), Gate::Excluded);
    }

    #[test]
    fn the_trailers_id_must_be_direct_when_the_file_is_encrypted() {
        let id = &row("FileTrailer", b"ID").indirect_reference[0];
        let file = bare();
        let reads = |trailer: Dict, version: &str, cell: &str| {
            let trailer = Object::from(trailer);
            file.eval(&trailer, At::Key(b"ID"), &[], version, |cx| {
                reference(cell, cx)
            })
        };
        let encrypted = || dict! { "Root" => Object::reference(1), "Encrypt" => dict! {} };
        assert_eq!(reads(encrypted(), "1.7", id), Reference::Direct);
        assert_eq!(reads(file.trailer.clone(), "1.7", id), Reference::Either);
        let info = &row("FileTrailer", b"Info").indirect_reference[0];
        assert_eq!(
            reads(file.trailer.clone(), "1.7", info),
            Reference::Indirect
        );
        assert_eq!(reads(file.trailer.clone(), "2.0", info), Reference::Either);
        let size = &row("FileTrailer", b"Size").indirect_reference[0];
        assert_eq!(reads(file.trailer.clone(), "1.7", size), Reference::Direct);
        let root = &row("FileTrailer", b"Root").indirect_reference[0];
        assert_eq!(
            reads(file.trailer.clone(), "1.7", root),
            Reference::Indirect
        );
        let encrypt = &row("FileTrailer", b"Encrypt").indirect_reference[0];
        assert_eq!(
            reads(file.trailer.clone(), "1.7", encrypt),
            Reference::Either
        );
    }

    #[test]
    fn unknown_goes_through_and_and_or_until_something_decides() {
        let file = bare();
        let x = |n: i64| Object::from(dict! { "X" => n });
        let is = |n, text: &str| holds_in(&file, &x(n), "1.7", text);
        assert_eq!(
            is(2, "fn:Eval(fn:IsEncryptedWrapper() && (@X==1))"),
            Truth::False
        );
        assert_eq!(
            is(2, "fn:Eval((@X==1) && fn:IsEncryptedWrapper())"),
            Truth::False
        );
        assert_eq!(
            is(1, "fn:Eval(fn:IsEncryptedWrapper() && (@X==1))"),
            unknown("fn:IsEncryptedWrapper")
        );
        assert_eq!(
            is(1, "fn:Eval(fn:IsEncryptedWrapper() || (@X==1))"),
            Truth::True
        );
        assert_eq!(
            is(2, "fn:Eval(fn:IsEncryptedWrapper() || (@X==1))"),
            unknown("fn:IsEncryptedWrapper")
        );
        assert_eq!(
            is(1, "fn:Eval(fn:IsMeaningful() && fn:IsEncryptedWrapper())"),
            unknown("fn:IsMeaningful"),
            "the first"
        );
        assert_eq!(
            is(1, "fn:Not(fn:IsEncryptedWrapper())"),
            unknown("fn:IsEncryptedWrapper")
        );
    }

    #[test]
    fn a_rule_holds_fully_only_when_every_function_in_it_is_supported() {
        let file = bare();
        let x = |n: i64| Object::from(dict! { "X" => n });
        let fully =
            |n, text: &str| file.eval(&x(n), At::Key(b"X"), &[], "1.7", |cx| holds_fully(text, cx));
        let decided = "fn:Eval(fn:IsEncryptedWrapper() || (@X==1))";
        assert_eq!(holds_in(&file, &x(1), "1.7", decided), Truth::True);
        assert_eq!(fully(1, decided), unknown("fn:IsEncryptedWrapper"));
        assert_eq!(
            fully(1, "fn:Eval((@X==1) && fn:Not(fn:FontHasLatinChars()))"),
            unknown("fn:FontHasLatinChars"),
            "however deep"
        );
        assert_eq!(fully(1, "fn:Eval((@X>=0) && (@X<=1))"), Truth::True);
        assert_eq!(fully(2, "fn:Eval((@X>=0) && (@X<=1))"), Truth::False);
        assert_eq!(fully(2, "fn:Eval((@X mod 2)==0)"), Truth::True);
        assert_eq!(fully(3, "fn:Eval((@X mod 2)==0)"), Truth::False);
    }

    #[test]
    fn an_unsupported_function_is_unknown() {
        let file = bare();
        let x = Object::from(dict! { "A" => 1 });
        assert_eq!(
            holds_in(&file, &x, "1.7", "fn:Eval(fn:IsEncryptedWrapper())"),
            unknown("fn:IsEncryptedWrapper")
        );
    }

    #[test]
    fn arithmetic_comes_to_a_number_or_what_stands_in_its_way() {
        let file = bare();
        let x = Object::from(dict! {
            "A" => 1,
            "B" => 2,
            "K" => integers([1, 2, 3, 4]),
            "Half" => 0.5,
            "Whole" => 180.0,
            "Big" => i64::MAX,
            "N" => "Name",
        });
        let is = |text: &str| holds_in(&file, &x, "1.7", text);
        assert_eq!(is("fn:Eval((@A+@B)==3)"), Truth::True);
        assert_eq!(is("fn:Eval((@A - @B)<0)"), Truth::True);
        assert_eq!(is("fn:Eval((@A - @B)==-1)"), Truth::True);
        assert_eq!(is("fn:Eval((fn:ArrayLength(K) mod 2)==0)"), Truth::True);
        assert_eq!(is("fn:Eval((fn:ArrayLength(K) mod 3)==0)"), Truth::False);
        // `*` and `mod` before `+` and `-`: 1+6, not 3 * 3.
        assert_eq!(is("fn:Eval((@A+@B * 3)==7)"), Truth::True);
        assert_eq!(
            is("fn:Eval((@B - @A - @A)==0)"),
            Truth::True,
            "left to right"
        );
        // A real makes a real; an integer stands for a number.
        assert_eq!(is("fn:Eval((@Half * 2)==1.0)"), Truth::True);
        assert_eq!(is("fn:Eval((@Half+@A)==1.5)"), Truth::True);
        assert_eq!(
            is("fn:Eval((@Whole mod 90)==0)"),
            Truth::True,
            "a whole real"
        );

        // A page's /Rotate, a multiple of 90, negative ones too.
        let rotated = |r: i64| {
            let page = Object::from(dict! { "Rotate" => r });
            holds_in(&file, &page, "1.7", "fn:Eval((@Rotate mod 90)==0)")
        };
        assert_eq!(rotated(180), Truth::True);
        assert_eq!(rotated(-90), Truth::True);
        assert_eq!(rotated(45), Truth::False);
        // A Type 1 font's /Widths, one for each code from /FirstChar to
        // /LastChar.
        let widths = "fn:Eval(fn:ArrayLength(Widths)==(1+(@LastChar - @FirstChar)))";
        let font = |last: i64| {
            let font = dict! { "Widths" => integers([500, 600, 700]), "FirstChar" => 32, "LastChar" => last };
            holds_in(&file, &font.into(), "1.7", widths)
        };
        assert_eq!(font(34), Truth::True);
        assert_eq!(font(35), Truth::False);

        // What can't be read, or isn't a number, stands in the way; and an
        // overflow, a fraction's mod or a mod of 0, the whole.
        assert_eq!(is("fn:Eval((@Gone+1)==2)"), unknown("@Gone"));
        assert_eq!(is("fn:Eval((@N+1)==2)"), unknown("@N"));
        assert_eq!(is("fn:Eval((@Big * 2)==0)"), unknown("@Big * 2"));
        assert_eq!(is("fn:Eval((@Big+@A)==0)"), unknown("@Big+@A"));
        assert_eq!(is("fn:Eval((@Half mod 2)==0)"), unknown("@Half mod 2"));
        assert_eq!(is("fn:Eval((@A mod 0)==0)"), unknown("@A mod 0"));
        // A number is no condition.
        assert_eq!(is("fn:Eval(@A+@B)"), unknown("@A+@B"));
    }

    #[test]
    fn functions_read_strings_arrays_and_the_file() {
        let mut file = bare();
        let bytes = |b: &[u8]| Object::new(Value::String(Str::Bytes(b.to_vec()), None));
        let x = Object::from(dict! {
            "ID" => vec![bytes(&[7; 16]), bytes(b"0123")],
            "Panose" => bytes(&[0; 12]),
            "Text" => Object::new(Value::String(Str::Text("A".to_string()), None)),
            "Cipher" => Object::new(Value::String(Str::Encrypted(vec![1, 2]), None)),
            "N" => 1,
            "Prev" => 500,
            "Index" => integers([0, 3, 5, 1, 9, 2]),
            "Down" => integers([5, 1, 0, 3]),
            "Even" => integers([0, 1, 0, 2]),
            "Mixed" => vec![Object::from(0), Object::from(1), Object::from("A"), Object::from(2)],
            "Empty" => integers([]),
            "Filter" => "DCTDecode",
            "Filters" => vec![Object::from("FlateDecode"), Object::from("DCTDecode")],
        });
        let is = |file: &File, text: &str| holds_in(file, &x, "1.7", text);
        // A string's length in bytes, by key or by an array's index.
        let id = x.as_dict().unwrap().get(b"ID").unwrap().clone();
        let element = |i| {
            file.eval(&id, At::Index(i), &[], "1.7", |cx| {
                holds(&format!("fn:Eval(fn:StringLength({i})>=16)"), cx)
            })
        };
        assert_eq!(element(0), Truth::True);
        assert_eq!(element(1), Truth::False);
        assert_eq!(
            is(&file, "fn:Eval(fn:StringLength(Panose)==12)"),
            Truth::True
        );
        assert_eq!(
            is(&file, "fn:Eval(fn:StringLength(Text)==1)"),
            unknown("the string's bytes")
        );
        assert_eq!(
            is(&file, "fn:Eval(fn:StringLength(Cipher)==2)"),
            unknown("the string's bytes")
        );
        assert_eq!(is(&file, "fn:Eval(fn:StringLength(N)==1)"), unknown("N"));
        // The file's length, when the reader knows it.
        let prev = "fn:Eval((@Prev>=0) && (@Prev<=fn:FileSize()))";
        assert_eq!(is(&file, prev), unknown("fn:FileSize"));
        file.size = Some(1000);
        assert_eq!(is(&file, prev), Truth::True);
        file.size = Some(100);
        assert_eq!(is(&file, prev), Truth::False);
        // Every second element, from the first, no less than the one before.
        assert_eq!(is(&file, "fn:ArraySortAscending(Index,2)"), Truth::True);
        assert_eq!(is(&file, "fn:ArraySortAscending(Down,2)"), Truth::False);
        assert_eq!(
            is(&file, "fn:ArraySortAscending(Even,2)"),
            Truth::True,
            "equal"
        );
        assert_eq!(
            is(&file, "fn:ArraySortAscending(Mixed,2)"),
            Truth::False,
            "a name"
        );
        assert_eq!(is(&file, "fn:ArraySortAscending(Empty,2)"), Truth::True);
        assert_eq!(is(&file, "fn:ArraySortAscending(Gone,2)"), unknown("Gone"));
        assert_eq!(
            is(&file, "fn:ArraySortAscending(Index,0)"),
            unknown("fn:ArraySortAscending")
        );
        // A name, or an array holding it.
        assert_eq!(is(&file, "fn:Contains(@Filter,DCTDecode)"), Truth::True);
        assert_eq!(is(&file, "fn:Contains(@Filters,DCTDecode)"), Truth::True);
        assert_eq!(is(&file, "fn:Contains(@Filter,FlateDecode)"), Truth::False);
        assert_eq!(is(&file, "fn:Contains(@Gone,DCTDecode)"), unknown("@Gone"));
        // The file's catalog's /MarkInfo /Marked.
        let tagged = |marked: Option<bool>| {
            let mut catalog = dict! { "Type" => "Catalog" };
            if let Some(marked) = marked {
                catalog.set("MarkInfo", dict! { "Marked" => marked });
            }
            let file = File::new(
                dict! { "Root" => Object::reference(1) },
                [(1, catalog.into())],
            );
            holds_in(&file, &x, "1.7", "fn:IsPDFTagged()")
        };
        assert_eq!(tagged(Some(true)), Truth::True);
        assert_eq!(tagged(Some(false)), Truth::False);
        assert_eq!(tagged(None), Truth::False);
    }

    #[test]
    fn functions_read_the_rows_own_string() {
        let file = bare();
        let row = |value: Value| Object::from(dict! { "X" => Object::new(value) });
        let sig = |form| row(Value::String(Str::Bytes(b"sig".to_vec()), form));
        let string = sig(None);
        let cipher = row(Value::String(Str::Encrypted(b"sig".to_vec()), None));
        let number = row(Value::Integer(1));
        let is = |file: &File, x: &Object, text: &str| holds_in(file, x, "1.7", text);
        // As written, when the reader says.
        assert_eq!(
            is(&file, &string, "fn:IsHexString()"),
            unknown("fn:IsHexString")
        );
        let hex = sig(Some(Form::Hex));
        assert_eq!(is(&file, &hex, "fn:IsHexString()"), Truth::True);
        let literal = sig(Some(Form::Literal));
        assert_eq!(is(&file, &literal, "fn:IsHexString()"), Truth::False);
        let cipher_hex = row(Value::String(
            Str::Encrypted(b"sig".to_vec()),
            Some(Form::Hex),
        ));
        assert_eq!(is(&file, &cipher_hex, "fn:IsHexString()"), Truth::True);
        assert_eq!(is(&file, &number, "fn:IsHexString()"), Truth::False);
        // Unencrypted: any string as read, but ciphertext.
        assert_eq!(is(&file, &string, "fn:AlwaysUnencrypted()"), Truth::True);
        assert_eq!(
            is(&file, &cipher, "fn:AlwaysUnencrypted()"),
            unknown("the string's bytes")
        );
        assert_eq!(is(&file, &number, "fn:AlwaysUnencrypted()"), Truth::False);
    }

    #[test]
    fn a_name_tree_and_a_key_map_hold_the_rows_name() {
        let bytes = |b: &str| Object::new(Value::String(Str::Bytes(b.as_bytes().to_vec()), None));
        // A tree of two leaves under a root, one leaf indirect, and a
        // catalog's old-style /Dests.
        let leaf = |keys: &[&str]| {
            let names: Vec<Object> = (keys.iter())
                .flat_map(|k| [bytes(k), Object::from(0)])
                .collect();
            Object::from(dict! { "Names" => names })
        };
        let dests = dict! { "Kids" => vec![Object::reference(2), leaf(&["b", "c"])] };
        let catalog = dict! {
            "Type" => "Catalog",
            "Names" => dict! { "Dests" => dests },
            "Dests" => dict! { "Old" => vec![Object::from(0)] },
        };
        let file = File::new(
            dict! { "Root" => Object::reference(1) },
            [(1, catalog.into()), (2, leaf(&["a"]))],
        );
        let tree = "fn:InNameTree(trailer::Catalog::Names::Dests)";
        let row = |value: Object| Object::from(dict! { "X" => value });
        let is = |file: &File, x: &Object, text: &str| holds_in(file, x, "1.7", text);
        for key in ["a", "b", "c"] {
            assert_eq!(is(&file, &row(bytes(key)), tree), Truth::True, "{key}");
        }
        assert_eq!(is(&file, &row(bytes("d")), tree), Truth::False);
        assert_eq!(
            is(&file, &Object::from(dict! {}), tree),
            Truth::False,
            "an absent row"
        );
        assert_eq!(
            is(
                &file,
                &row(bytes("a")),
                "fn:InNameTree(trailer::Catalog::Names::Pages)"
            ),
            Truth::False,
            "no such tree"
        );
        assert_eq!(
            is(&file, &row(Object::from(1)), tree),
            unknown("fn:InNameTree")
        );
        let text = Object::new(Value::String(Str::Text("a".to_string()), None));
        assert_eq!(is(&file, &row(text), tree), unknown("the string's bytes"));
        // A tree that doesn't all read: what it's found to hold, it holds;
        // what it isn't can't be told.
        let broken = dict! { "Kids" => vec![Object::reference(9), leaf(&["b"])] };
        let catalog = dict! { "Type" => "Catalog", "Names" => dict! { "Dests" => broken } };
        let partial = File::new(
            dict! { "Root" => Object::reference(1) },
            [(1, catalog.into())],
        );
        assert_eq!(is(&partial, &row(bytes("b")), tree), Truth::True);
        assert_eq!(
            is(&partial, &row(bytes("a")), tree),
            unknown("fn:InNameTree")
        );
        // The old-style map, by name.
        let map = "fn:InKeyMap(trailer::Catalog::Dests)";
        assert_eq!(is(&file, &row(Object::from("Old")), map), Truth::True);
        assert_eq!(is(&file, &row(Object::from("New")), map), Truth::False);
        assert_eq!(is(&file, &Object::from(dict! {}), map), Truth::False);
        assert_eq!(is(&file, &row(bytes("Old")), map), unknown("fn:InKeyMap"));
        assert_eq!(
            is(
                &file,
                &row(Object::from("Old")),
                "fn:InKeyMap(trailer::Catalog::Gone)"
            ),
            Truth::False,
            "no such map"
        );
    }

    #[test]
    fn a_parent_is_what_holds_an_array_for_a_key_and_the_array_for_an_index() {
        let file = bare();
        // An object reference in a structure element's /K array.
        let element = Object::from(dict! { "Pg" => 1, "S" => "Link" });
        let kids = Object::from(vec![Object::from(dict! { "Type" => "OBJR" })]);
        let reference = Object::from(dict! { "Type" => "OBJR" });
        let parents = [&kids, &element];
        let is = |text: &str| {
            file.eval(&reference, At::Key(b"Pg"), &parents, "1.7", |cx| {
                holds(text, cx)
            })
        };
        assert_eq!(
            is("fn:IsPresent(parent::Pg)"),
            Truth::True,
            "past the array"
        );
        assert_eq!(is("fn:IsPresent(parent::0)"), Truth::True, "into the array");
        assert_eq!(
            is("fn:IsPresent(parent::1)"),
            Truth::False,
            "into the array"
        );
    }

    #[test]
    fn a_name_trees_keys_are_read_once() {
        let file = bare();
        let names = vec![
            Object::new(Value::String(Str::Bytes(b"a".to_vec()), None)),
            Object::from(0),
        ];
        let tree = Object::from(dict! { "Names" => names });
        let trees = Trees::default();
        let resolve = |id: ObjectId| file.objects.get(&id);
        let x = Object::from(
            dict! { "T" => tree, "X" => Object::new(Value::String(Str::Bytes(b"a".to_vec()), None)) },
        );
        let cx = Cx {
            container: &x,
            at: At::Key(b"X"),
            parents: &[],
            trailer: &file.trailer,
            resolve: &resolve,
            version: v("1.7"),
            defaults: None,
            size: None,
            trees: &trees,
            structure: &Structure::default(),
            fields: &Fields::default(),
            extensions: &none(),
        };
        assert_eq!(holds("fn:InNameTree(T)", &cx), Truth::True);
        assert_eq!(holds("fn:InNameTree(T)", &cx), Truth::True);
        assert_eq!(trees.0.borrow().len(), 1);
    }

    #[test]
    fn a_fully_qualified_field_name_is_a_fields_in_the_form() {
        let text = |s: &str| Object::string(s);
        // `a` with a field `b`, a widget of its own, and a field without a
        // name of its own, `a`'s, with a field `d`; `c`, whose name is
        // UTF-16; and a field at the top without a name.
        let utf16: Vec<u8> = [0xFE, 0xFF].into_iter().chain([0, b'c']).collect();
        let fields = vec![
            Object::reference(2),
            dict! { "T" => Object::bytes(utf16), "FT" => "Tx" }.into(),
            dict! { "FT" => "Tx" }.into(),
        ];
        let a = dict! {
            "T" => text("a"),
            "Kids" => vec![
                Object::from(dict! { "T" => text("b"), "Parent" => Object::reference(2) }),
                Object::from(dict! { "Subtype" => "Widget", "Parent" => Object::reference(2) }),
                Object::from(dict! { "Kids" => vec![Object::from(dict! { "T" => text("d") })] }),
            ],
        };
        let catalog = dict! { "Type" => "Catalog", "AcroForm" => dict! { "Fields" => fields } };
        let file = File::new(
            dict! { "Root" => Object::reference(1) },
            [(1, catalog.into()), (2, a.into())],
        );
        let fqn = "fn:IsFullyQualifiedFieldName(@X)";
        let is = |file: &File, name: Object| {
            holds_in(file, &Object::from(dict! { "X" => name }), "1.7", fqn)
        };
        for name in ["a", "a.b", "a.d", "c"] {
            assert_eq!(is(&file, text(name)), Truth::True, "{name}");
        }
        for name in ["b", "d", "a.c", "a.b.c", "", "A"] {
            assert_eq!(is(&file, text(name)), Truth::False, "{name}");
        }
        assert_eq!(
            is(&file, Object::from(1)),
            unknown("fn:IsFullyQualifiedFieldName")
        );
        let cipher = Object::new(Value::String(Str::Encrypted(b"a".to_vec()), None));
        assert_eq!(is(&file, cipher), unknown("the string's bytes"));
        // No form, no fields.
        assert_eq!(is(&bare(), text("a")), Truth::False);
        // A form that doesn't all read: a name it has is a field's, and one
        // it hasn't can't be told.
        let fields = vec![Object::reference(9), dict! { "T" => text("d") }.into()];
        let catalog = dict! { "Type" => "Catalog", "AcroForm" => dict! { "Fields" => fields } };
        let partial = File::new(
            dict! { "Root" => Object::reference(1) },
            [(1, catalog.into())],
        );
        assert_eq!(is(&partial, text("d")), Truth::True);
        assert_eq!(
            is(&partial, text("e")),
            unknown("fn:IsFullyQualifiedFieldName")
        );
        // An array's string, as `@*` reads it.
        let names = Object::from(vec![text("a.b"), text("z")]);
        let element = |i| {
            file.eval(&names, At::Index(i), &[], "1.7", |cx| {
                holds("fn:IsFullyQualifiedFieldName(@*)", cx)
            })
        };
        assert_eq!((element(0), element(1)), (Truth::True, Truth::False));
    }

    #[test]
    fn the_structure_trees_content_items_are_on_its_pages_and_objects() {
        // Pages 2 to 5 and an image, 6; a tree whose elements put marked
        // content on 2 (by MCID), 3 (by an MCR's own /Pg) and 4 (in a
        // form XObject's stream, which isn't the page's), and point at 6.
        let page = || Object::from(dict! { "Type" => "Page" });
        let image = Object::from(dict! { "Subtype" => "Image" });
        let mcr = |pg: Option<u32>, stm: bool| {
            let mut mcr = dict! { "Type" => "MCR", "MCID" => 0 };
            if let Some(pg) = pg {
                mcr.set("Pg", Object::reference(pg));
            }
            if stm {
                mcr.set("Stm", Object::reference(6));
            }
            Object::from(mcr)
        };
        let element = |pg: u32, kids: Vec<Object>| {
            Object::from(dict! { "S" => "P", "Pg" => Object::reference(pg), "K" => kids })
        };
        let objr = Object::from(dict! { "Type" => "OBJR", "Obj" => Object::reference(6) });
        let tree = |extra: Vec<Object>| {
            let mut kids = vec![
                element(2, vec![Object::from(0), Object::from(1)]),
                element(
                    2,
                    vec![mcr(Some(3), false), mcr(Some(4), true), objr.clone()],
                ),
            ];
            kids.extend(extra);
            let root = dict! { "Type" => "StructTreeRoot", "K" => kids };
            let catalog = dict! { "Type" => "Catalog", "StructTreeRoot" => root };
            File::new(
                dict! { "Root" => Object::reference(1) },
                [
                    (1, catalog.into()),
                    (2, page()),
                    (3, page()),
                    (4, page()),
                    (5, page()),
                    (6, image.clone()),
                ],
            )
        };
        let asks = |file: &File, n: u32, text: &str| {
            let container = file.objects.get(&(n, 0)).unwrap();
            file.eval(container, At::Key(b"X"), &[], "1.7", |cx| holds(text, cx))
        };
        let pages = "fn:PageContainsStructContentItems()";
        let file = tree(Vec::new());
        assert_eq!(asks(&file, 2, pages), Truth::True, "by MCID");
        assert_eq!(asks(&file, 3, pages), Truth::True, "by an MCR's /Pg");
        assert_eq!(
            asks(&file, 4, pages),
            Truth::False,
            "in a form XObject's stream"
        );
        assert_eq!(asks(&file, 5, pages), Truth::False);
        assert_eq!(asks(&file, 6, "fn:ImageIsStructContentItem()"), Truth::True);
        assert_eq!(
            asks(&file, 5, "fn:ImageIsStructContentItem()"),
            Truth::False
        );
        // An MCR without a page, its element's or its own: the pages not
        // found can't be told.
        let lost = Object::from(dict! { "S" => "P", "K" => vec![mcr(None, false)] });
        let file = tree(vec![lost]);
        assert_eq!(asks(&file, 2, pages), Truth::True);
        assert_eq!(asks(&file, 5, pages), unknown(&pages[..pages.len() - 2]));
        // No structure tree, no content items.
        let catalog = dict! { "Type" => "Catalog" };
        let file = File::new(
            dict! { "Root" => Object::reference(1) },
            [(1, catalog.into()), (2, page())],
        );
        assert_eq!(asks(&file, 2, pages), Truth::False);
    }

    #[test]
    fn a_note_to_readers_holds_and_says_when_they_ignore_the_row() {
        let file = bare();
        let x = |entries: Dict| Object::from(entries);
        let ignored = |o: &Object, version: &str, text: &str| {
            file.eval(o, At::Key(b"X"), &[], version, |cx| {
                (holds_fully(text, cx), ignored(text, cx))
            })
        };
        let some = Some(Truth::True);
        let none = Some(Truth::False);
        let flate = |p: i64| x(dict! { "Predictor" => p });
        assert_eq!(
            ignored(&flate(1), "1.7", "fn:Ignore(@Predictor<=1)"),
            (Truth::True, some.clone())
        );
        assert_eq!(
            ignored(&flate(12), "1.7", "fn:Ignore(@Predictor<=1)"),
            (Truth::True, none.clone())
        );
        assert_eq!(
            ignored(&x(dict! {}), "1.7", "fn:Ignore(@Predictor<=1)"),
            (Truth::True, Some(unknown("@Predictor")))
        );
        assert_eq!(
            ignored(&x(dict! {}), "1.7", "fn:Ignore()"),
            (Truth::True, some.clone())
        );
        // Only meaningful in a reply: ignored outside one.
        let reply = "fn:IsMeaningful(fn:IsPresent(IRT))";
        assert_eq!(
            ignored(&x(dict! { "IRT" => 1 }), "1.7", reply),
            (Truth::True, none.clone())
        );
        assert_eq!(
            ignored(&x(dict! {}), "1.7", reply),
            (Truth::True, some.clone())
        );
        // Where its version doesn't reach, it says nothing.
        let border = "fn:SinceVersion(1.2,fn:Ignore(fn:IsPresent(BS)))";
        let bs = x(dict! { "BS" => dict! {} });
        assert_eq!(ignored(&bs, "1.7", border), (Truth::True, some));
        assert_eq!(ignored(&bs, "1.1", border), (Truth::True, none));
        // A condition that calls what isn't supported never decides the
        // note, and leaves whether it's ignored unknown.
        assert_eq!(
            ignored(&x(dict! {}), "1.7", "fn:Ignore(fn:FontHasLatinChars())"),
            (Truth::True, Some(unknown("fn:FontHasLatinChars")))
        );
        assert_eq!(ignored(&x(dict! {}), "1.7", "fn:Eval(@A==1)").1, None);
    }

    #[test]
    fn a_required_value_says_its_condition_and_value() {
        let file = bare();
        let required = |o: Object, text: &str| {
            file.eval(&o, At::Key(b"Length"), &[], "1.7", |cx| {
                required_value(text, cx)
            })
        };
        let aes = "fn:RequiredValue(@CFM==AESV2,128)";
        let filter = |cfm: &str| Object::from(dict! { "CFM" => cfm, "Length" => 128 });
        assert_eq!(
            required(filter("AESV2"), aes),
            Some((Truth::True, "128".to_string()))
        );
        assert_eq!(
            required(filter("V2"), aes),
            Some((Truth::False, "128".to_string()))
        );
        assert_eq!(
            required(Object::from(dict! {}), aes),
            Some((unknown("@CFM"), "128".to_string()))
        );
        assert_eq!(
            required(filter("V2"), "fn:RequiredValue(fn:FontHasLatinChars(),128)"),
            Some((unknown("fn:FontHasLatinChars"), "128".to_string()))
        );
        assert_eq!(required(filter("V2"), "fn:Eval(@Length>=40)"), None);
    }

    #[test]
    fn a_special_cases_absent_key_takes_its_default() {
        let file = bare();
        let table = |name: &str| Grammar::embedded().table(name).unwrap();
        let special = |table: Option<&Table>, container: &Object, text: &str| {
            file.eval_with(table, container, At::Key(b"X"), &[], "1.7", |cx| {
                holds(text, cx)
            })
        };
        // An image's /ColorSpace, which an image mask mustn't have: its
        // /ImageMask false unless it says.
        let mask_free = "fn:Not(fn:IsPresent(@ImageMask==true))";
        let image = Object::from(dict! { "ColorSpace" => "DeviceRGB" });
        let images = Some(table("XObjectImage"));
        assert_eq!(special(images, &image, mask_free), Truth::True);
        assert_eq!(
            special(None, &image, mask_free),
            unknown("@ImageMask"),
            "a column without defaults"
        );
        let null = Object::from(dict! { "ImageMask" => Object::null() });
        assert_eq!(
            special(images, &null, mask_free),
            Truth::True,
            "null is absent"
        );
        let mask = Object::from(dict! { "ImageMask" => true });
        assert_eq!(special(images, &mask, mask_free), Truth::False);
        // No default, or not the key's own: none stands in.
        let none = |text: &str| special(images, &image, text);
        assert_eq!(none("fn:Eval(@Width>0)"), unknown("@Width"));
        assert_eq!(
            none("fn:Eval(parent::@ImageMask==false)"),
            unknown("parent::@ImageMask")
        );
        assert_eq!(
            none("fn:Eval(SMask::@ImageMask==false)"),
            unknown("SMask::@ImageMask")
        );
        let three_d = Some(table("Annot3D"));
        assert_eq!(
            special(three_d, &image, "fn:Eval(@ca==1.0)"),
            unknown("@ca"),
            "another key's value"
        );
        // A key of more than one type: the default its types give.
        let empty = Object::from(dict! {});
        let sort = Some(table("CollectionSort"));
        assert_eq!(special(sort, &empty, "fn:Eval(@A==true)"), Truth::True);
        let cid = Some(table("FontCIDType0"));
        assert_eq!(special(cid, &empty, "fn:Eval(@DW==1000)"), Truth::True);
        let mode = Some(table("3DRenderMode"));
        assert_eq!(special(mode, &empty, "fn:Eval(@FC==BG)"), Truth::True);
        // Not a wildcard row's.
        let sub = Some(table("AppearanceSubDict"));
        assert_eq!(special(sub, &empty, "fn:Eval(@Off==1)"), unknown("@Off"));
    }

    #[test]
    fn comparisons_read_names_numbers_strings_and_booleans() {
        let file = bare();
        let utf16: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("Marked".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        let x = Object::from(dict! {
            "Type" => "Page",
            "V" => 4.0,
            "CA" => 1,
            "StateModel" => Object::bytes(utf16),
            "ImageMask" => true,
            "Matrix" => integers([1, 0]),
        });
        let is = |text: &str| holds_in(&file, &x, "1.7", text);
        assert_eq!(is("fn:Eval(@Type==Page)"), Truth::True);
        assert_eq!(is("fn:Eval(@Type!=Page)"), Truth::False);
        assert_eq!(
            is("fn:Eval(@V==4)"),
            Truth::True,
            "an integer stands for a number"
        );
        assert_eq!(is("fn:Eval((@CA>=0.0) && (@CA<=1.0))"), Truth::True);
        assert_eq!(is("fn:Eval(@CA<1)"), Truth::False);
        assert_eq!(
            is("fn:Eval(@StateModel=='Marked')"),
            Truth::True,
            "a text string's text"
        );
        assert_eq!(is("fn:Eval(@ImageMask==true)"), Truth::True);
        assert_eq!(
            is("fn:Eval(@Type==4)"),
            unknown("@Type==4"),
            "a name isn't a number"
        );
        assert_eq!(
            is("fn:Eval(@Type<Page)"),
            unknown("@Type<Page"),
            "names aren't in order"
        );
        assert_eq!(
            is("fn:Eval(@Matrix==1)"),
            unknown("@Matrix"),
            "an array isn't compared"
        );
        assert_eq!(is("fn:Eval(fn:ArrayLength(Matrix)==2)"), Truth::True);
        assert_eq!(
            is("fn:Eval(fn:ArrayLength(Nonesuch)==2)"),
            unknown("Nonesuch")
        );

        // A key a parent has: a VRI's CRL is required when the DSS's CRLs has
        // any.
        let crl = &row("VRI", b"CRL").required;
        let dss = |n: usize| Object::from(dict! { "CRLs" => vec![Object::null(); n] });
        let vri = Object::from(dict! {});
        let required_under =
            |dss: &Object| file.eval(&vri, At::Key(b"CRL"), &[dss], "1.7", |cx| required(crl, cx));
        assert_eq!(required_under(&dss(2)), Truth::True);
        assert_eq!(required_under(&dss(0)), Truth::False);
        assert_eq!(
            file.eval(&vri, At::Key(b"CRL"), &[], "1.7", |cx| required(crl, cx)),
            unknown("parent::CRLs")
        );

        // A soft mask with a Matte is the size of its image.
        let width = &row("XObjectImageSoftMask", b"Width").special_case[0];
        let image = Object::from(dict! { "Width" => 10 });
        let mask = |w: i64, matte: bool| {
            let mut mask = dict! { "Width" => w };
            if matte {
                mask.set("Matte", integers([0]));
            }
            Object::stream(mask)
        };
        let met = |mask: &Object| {
            file.eval(mask, At::Key(b"Width"), &[&image], "1.7", |cx| {
                holds(width, cx)
            })
        };
        assert_eq!(met(&mask(10, true)), Truth::True);
        assert_eq!(met(&mask(11, true)), Truth::False);
        assert_eq!(
            met(&mask(11, false)),
            Truth::True,
            "without a Matte it asks nothing"
        );
    }

    #[test]
    fn a_wildcard_first_is_the_rows_own_value() {
        let file = bare();
        let array = integers([1, 2, -1]);
        let at = |i: usize, text: &str| {
            file.eval(&array, At::Index(i), &[], "1.7", |cx| holds(text, cx))
        };
        assert_eq!(at(1, "fn:Eval(@*>0)"), Truth::True);
        assert_eq!(at(2, "fn:Eval(@*>0)"), Truth::False);
        assert_eq!(at(2, "fn:Eval(@1*>=0)"), Truth::False);
        assert_eq!(at(0, "fn:Eval(@0<=@1)"), Truth::True, "indices");
        let urls = Object::from(vec![Object::from(dict! { "FS" => "URL" })]);
        let url = file.eval(&urls, At::Index(0), &[], "2.0", |cx| {
            holds("fn:Eval(fn:IsPresent(*::FS) && (*::@FS==URL))", cx)
        });
        assert_eq!(url, Truth::True);
        // `*` past the start is any entry, which only presence reads.
        assert_eq!(at(0, "fn:Eval(A::@*==1)"), unknown("A::@*"));
    }

    #[test]
    fn a_predicate_that_isnt_one_doesnt_parse() {
        let nested = |n: usize| format!("{}fn:IsPresent(X){}", "fn:Not(".repeat(n), ")".repeat(n));
        assert!(parse(&nested(20)).is_ok());
        let Err(e) = parse(&nested(40)) else {
            panic!("40 deep parsed");
        };
        assert!(format!("{e:#}").contains("nested deeper"), "{e:#}");
        for bad in [
            "(@A==1) && (@B==2) || (@C==3)",
            "fn:Not(a,b,c)",
            "fn:eval(@A==1)",
            "@A=='open",
            "@A==''",
            "@A == 1",
            "fn:IsPresent(A)x",
            "fn:IsPresent(A::parent::B)",
            "fn:IsPresent(trailer)",
            "fn:IsPresent(A:B)",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    /// A predicate that doesn't parse says so word for word: the predicate,
    /// then what stopped it and where.
    #[test]
    fn a_predicate_that_doesnt_parse_says_where_and_why() {
        let nested = format!("{}fn:IsPresent(X){}", "fn:Not(".repeat(40), ")".repeat(40));
        let range = "99999999999999999999".parse::<i64>().unwrap_err();
        let cases = [
            (
                "fn:Not(a,b,c)",
                r#""fn:Not(a,b,c)": ")" expected at 10, not ",c)""#.to_string(),
            ),
            (
                "(@A==1) && (@B==2) || (@C==3)",
                r#""(@A==1) && (@B==2) || (@C==3)": && and || mixed without brackets at 18"#
                    .to_string(),
            ),
            (&nested, format!("{nested:?}: nested deeper than 32 at 224")),
            (
                "@A=='open",
                r#""@A=='open": a string left open"#.to_string(),
            ),
            ("@A==''", r#""@A==''": an empty string at 5"#.to_string()),
            (
                "@A==)",
                r#""@A==)": a term expected at 4, not ")""#.to_string(),
            ),
            (
                "fn:eval(@A==1)",
                r#""fn:eval(@A==1)": fn: and a CamelCase name expected at 3"#.to_string(),
            ),
            (
                "@A==99999999999999999999",
                format!(
                    r#""@A==99999999999999999999": 99999999999999999999 is out of range: {range}"#
                ),
            ),
            (
                "fn:IsPresent(trailer)",
                r#""fn:IsPresent(trailer)": "trailer": a path to nothing"#.to_string(),
            ),
            (
                "fn:IsPresent(A:B)",
                r#""fn:IsPresent(A:B)": "A:B": "A:B" isn't a key"#.to_string(),
            ),
            (
                "fn:IsPresent(A)x",
                r#""fn:IsPresent(A)x": "x" after a whole predicate"#.to_string(),
            ),
        ];
        for (text, said) in cases {
            let Err(e) = parse(text) else {
                panic!("{text:?} parsed");
            };
            assert_eq!(format!("{e:#}"), said);
        }
    }

    #[test]
    fn a_simple_fonts_widths_are_required_but_of_the_standard_14_before_2_0() {
        let file = bare();
        let cell = &row("FontType1", b"Widths").required;
        let font = |base: &str| {
            Object::from(dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base })
        };
        let at = |font: &Object, version| required_in(&file, font, version, cell);
        assert_eq!(at(&font("Helvetica"), "1.7"), Truth::False, "{cell}");
        assert_eq!(at(&font("Helvetica"), "2.0"), Truth::True);
        assert_eq!(at(&font("ArialMT"), "1.7"), Truth::True);
        let untyped = Object::from(dict! { "Subtype" => "Type1", "BaseFont" => "ArialMT" });
        assert_eq!(at(&untyped, "1.7"), Truth::False, "not /Type /Font");
    }

    #[test]
    fn bits_are_the_rows_own_values() {
        let file = bare();
        // A button field is a push button, a checkbox or a radio button by
        // its /Ff's bits 16 and 17 (ISO 32000-2, 12.7.5.2).
        let ff = |table: &str| &row(table, b"Ff").special_case[0];
        let field = |ff: i64| Object::from(dict! { "FT" => "Btn", "Ff" => ff });
        let at = |table: &str, flags: i64| {
            file.eval(&field(flags), At::Key(b"Ff"), &[], "1.7", |cx| {
                holds(ff(table), cx)
            })
        };
        for (flags, push, checkbox, radio) in [
            (0, false, true, false),
            (1 << 16, true, false, false),
            ((1 << 15) | (1 << 14), false, false, true),
        ] {
            let expected =
                [push, checkbox, radio].map(|b| if b { Truth::True } else { Truth::False });
            let got = ["FieldBtnPush", "FieldBtnCheckbox", "FieldBtnRadio"].map(|t| at(t, flags));
            assert_eq!(got, expected, "/Ff {flags}");
        }

        // An encryption dictionary's /P is negative: its bits 13 to 32 set.
        let p = &row("EncryptionStandard", b"P").special_case[0];
        let allowed = |value: i64| {
            let encrypt = Object::from(dict! { "P" => value });
            file.eval(&encrypt, At::Key(b"P"), &[], "1.7", |cx| holds(p, cx))
        };
        assert_eq!(allowed(-3904), Truth::True, "{p}");
        assert_eq!(allowed(-3901), Truth::False, "bits 1 and 2 set");
        assert_eq!(allowed(3904), Truth::False, "bits 13 to 32 clear");
        assert_eq!(allowed(1 << 40), Truth::False, "past 32 bits");

        let x = Object::from(dict! { "F" => 4 });
        let is = |text: &str| file.eval(&x, At::Key(b"F"), &[], "1.7", |cx| holds(text, cx));
        assert_eq!(is("fn:Eval(fn:BitSet(3))"), Truth::True);
        assert_eq!(is("fn:Eval(fn:BitsSet(2,3))"), Truth::False);
        assert_eq!(is("fn:Eval(fn:BitSet(33))"), unknown("fn:BitSet"));
    }
}
