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

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::num::ParseIntError;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use pdf_objects::{Dict, Form, Object, ObjectId, Resolve, Span, Str, Value, Version};

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
    "fn:NoCycle",
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

/// What every row of one file is evaluated in: the trailer, the version,
/// how a reference resolves (`R`, any [`Resolve`]), the file's length and
/// the extensions it's checked under; and what the predicates read of the
/// file as a whole (its name trees' keys, its structure tree's content
/// items, its form's fields' names), kept inside it once read, the first
/// kept where two threads read one at once. [`FileCx::at`] makes a row's
/// [`Cx`] from it. It's shared across threads when its resolver is `Sync`,
/// each thread making its rows' [`Cx`]s there.
///
/// Its type names its resolver: one made from a closure is of the
/// closure's type, and one over a document or a reading of that
/// reference's. `FileCx<'a>`, the default, is one over any resolver, a
/// `dyn Resolve`, as a [`Cx`] reads through it: so `&FileCx` in a
/// signature takes a reference to one over any resolver. It isn't kept so
/// by value: one kept is kept by its resolver's type, or boxed
/// (`Box<FileCx>`).
///
/// An object its resolver gives none for is null to the predicates, as
/// one the file hasn't is (ISO 32000-2, 7.3.10), whatever the reason: over
/// a reading by position, one whose bytes the source failed to give is
/// null in that row, and missing from any tree read then, which is kept
/// so.
///
/// Those it keeps know an object by where it was written and by its
/// number, so the spans of the objects it's given, and of those its
/// resolver gives, are places in one file's bytes: an object copied from
/// another file keeps the span it had there, which can be another object's
/// here, so a copy is given its number, or its spans are dropped
/// ([`Object::unplace`]). Over a document of several files' objects, then,
/// its indirect objects are told apart by their numbers.
pub struct FileCx<'a, R: ?Sized = dyn Resolve<'a> + 'a> {
    /// The trailer as written (`trailer::`).
    trailer: &'a Dict,
    /// The file's version: its header's, or its catalog's `/Version` when
    /// later.
    version: Version,
    /// The file's length in bytes, when the reader knows (`fn:FileSize`).
    size: Option<usize>,
    /// The extensions the file is checked under, as the model names them
    /// (`fn:Extension`).
    extensions: &'a BTreeSet<String>,
    /// The file's name trees' keys, as `fn:InNameTree` reads them: each
    /// tree once.
    trees: Trees,
    /// The structure tree's content items, as `fn:ImageIsStructContentItem`
    /// and `fn:PageContainsStructContentItems` read them: once.
    structure: Structure,
    /// The form's fields' fully qualified names, as
    /// `fn:IsFullyQualifiedFieldName` reads them: once.
    fields: Fields,
    /// An indirect object; none for one the file hasn't, which is null
    /// (ISO 32000-2, 7.3.10). Last, so that a `FileCx` of any resolver is
    /// read through one of `dyn Resolve` ([`Cx`]'s).
    resolve: R,
}

/// The extensions a file is checked under when [`FileCx::extensions`]
/// gives none.
static NO_EXTENSIONS: BTreeSet<String> = BTreeSet::new();

impl<'a, R: Resolve<'a>> FileCx<'a, R> {
    /// A file of `trailer` at `version`, whose references `resolve` gives
    /// the objects of; of no known length, checked under no extension.
    pub fn new(trailer: &'a Dict, version: Version, resolve: R) -> FileCx<'a, R> {
        FileCx {
            trailer,
            version,
            size: None,
            extensions: &NO_EXTENSIONS,
            trees: Trees::default(),
            structure: Structure::default(),
            fields: Fields::default(),
            resolve,
        }
    }

    /// The file's length in bytes, when the reader knows (`fn:FileSize`).
    #[must_use = "it gives the FileCx back, with the length"]
    pub fn size(mut self, size: Option<usize>) -> FileCx<'a, R> {
        self.size = size;
        self
    }

    /// The extensions it's checked under, as the model names them
    /// (`fn:Extension`).
    #[must_use = "it gives the FileCx back, with the extensions"]
    pub fn extensions(mut self, extensions: &'a BTreeSet<String>) -> FileCx<'a, R> {
        self.extensions = extensions;
        self
    }

    /// The row at `at` in `container`, the dictionary, array or stream its
    /// value is in: reached through no parents, with no defaults, and of
    /// no number, until [`Cx::parents`], [`Cx::defaults`] and [`Cx::id`]
    /// say.
    pub fn at<'c>(&'c self, container: &'c Object, at: At<'c>) -> Cx<'c, 'a>
    where
        R: 'c,
    {
        Cx {
            file: self,
            container,
            id: None,
            at,
            parents: &[],
            defaults: None,
        }
    }
}

/// One over any resolver, as `&FileCx` in a signature takes it.
impl<'a> FileCx<'a> {
    /// The row at `at` in `container`, as one over its resolver's own type
    /// makes it.
    pub fn at<'c>(&'c self, container: &'c Object, at: At<'c>) -> Cx<'c, 'a> {
        Cx {
            file: self,
            container,
            id: None,
            at,
            parents: &[],
            defaults: None,
        }
    }
}

/// Its parts but its resolver, which needn't be one that can be shown.
impl<R: ?Sized> fmt::Debug for FileCx<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileCx")
            .field("trailer", &self.trailer)
            .field("version", &self.version)
            .field("size", &self.size)
            .field("extensions", &self.extensions)
            .field("trees", &self.trees)
            .field("structure", &self.structure)
            .field("fields", &self.fields)
            .finish_non_exhaustive()
    }
}

/// What a predicate is evaluated in: the row's place, the walk's way to
/// it, and its file's [`FileCx`], which makes it ([`FileCx::at`]). Small,
/// and made for each row.
#[derive(Clone, Copy)]
pub struct Cx<'c, 'a> {
    /// The file, its resolver whichever it is.
    file: &'c FileCx<'a, dyn Resolve<'a> + 'c>,
    /// The dictionary, array or stream the row's value is in.
    container: &'c Object,
    /// The indirect object the container is, when the caller says.
    id: Option<ObjectId>,
    /// Where in it: the key or element the row matched.
    at: At<'c>,
    /// The objects the walk came through to `container`, nearest first.
    parents: &'c [&'c Object],
    /// The container's table, for its DefaultValues.
    defaults: Option<&'c Table>,
}

impl<'c, 'a> Cx<'c, 'a> {
    /// The objects the walk came through to the container, nearest first
    /// (`parent::`).
    #[must_use = "a Cx is a copy: it gives a new one, with the parents"]
    pub fn parents(mut self, parents: &'c [&'c Object]) -> Cx<'c, 'a> {
        self.parents = parents;
        self
    }

    /// The container's table, whose DefaultValues stand in for a key the
    /// container lacks or has null, read as `@Key` (`@ImageMask`): given
    /// for SpecialCase and PossibleValues, none for the other columns.
    #[must_use = "a Cx is a copy: it gives a new one, with the defaults"]
    pub fn defaults(mut self, table: Option<&'c Table>) -> Cx<'c, 'a> {
        self.defaults = table;
        self
    }

    /// The number of the indirect object the container is, not one it's
    /// written inside: `fn:ImageIsStructContentItem`,
    /// `fn:PageContainsStructContentItems` and `fn:NoCycle` know it by that
    /// alone, where without one they know it by where it was written. So one built in
    /// memory, or edited, which has no span, is still known, and one whose
    /// bytes are another object's too, as an object stream's header can
    /// start two of its objects at one offset, is told apart from it.
    #[must_use = "a Cx is a copy: it gives a new one, with the number"]
    pub fn id(mut self, id: ObjectId) -> Cx<'c, 'a> {
        self.id = Some(id);
        self
    }

    /// The dictionary, array or stream the row's value is in.
    /// `fn:ImageIsStructContentItem` and `fn:PageContainsStructContentItems`
    /// know it as the objects the structure tree points at are known: by
    /// its number when [`Cx::id`] gives it, and else by where it was
    /// written, its span; not by where it's kept, so a copy is known as the
    /// object it copies while it has its span or its number.
    pub fn container(&self) -> &'c Object {
        self.container
    }

    /// Where in the container the row is: the key or element it matched,
    /// which `@*` and `@1*` read.
    pub fn at(&self) -> At<'c> {
        self.at
    }
}

/// Its row's parts, and the defaults' table by name; not its file.
impl fmt::Debug for Cx<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cx")
            .field("container", &self.container)
            .field("id", &self.id)
            .field("at", &self.at)
            .field("parents", &self.parents)
            .field("defaults", &self.defaults.map(|table| &table.name))
            .finish_non_exhaustive()
    }
}

/// The interactive form's fields' fully qualified names, kept once read,
/// the first kept where two threads read them at once.
#[derive(Debug, Default)]
struct Fields(OnceLock<Names>);

/// Fields' fully qualified names, as text, and whether they're all of
/// them: every field read, and every partial name a string whose text
/// the reader gave.
#[derive(Debug)]
struct Names {
    names: HashSet<String>,
    whole: bool,
}

/// The structure tree's content items, kept once read, the first kept
/// where two threads read them at once. It knows the
/// objects it found by where they were written, or their numbers
/// ([`Known`]), so it serves one file.
#[derive(Debug, Default)]
struct Structure(OnceLock<Items>);

/// What the structure tree says of its content items (ISO 32000-2,
/// 14.7.5): the objects its object references (OBJR) point at, and the
/// pages its marked-content items are on.
#[derive(Debug)]
struct Items {
    objects: Objects,
    pages: Objects,
}

/// The objects content items of one kind name, each as it's [`Known`],
/// and whether they're all of them.
#[derive(Debug)]
struct Objects {
    /// The numbers of those named by reference.
    ids: HashSet<ObjectId>,
    /// Where they were written, those the reader placed.
    spans: HashSet<Span>,
    /// Whether one the reader didn't place.
    unplaced: bool,
    /// Whether they're all of them: every node read, and every item's
    /// object found.
    whole: bool,
}

impl Objects {
    /// None yet, and all of them.
    fn new() -> Objects {
        Objects {
            ids: HashSet::new(),
            spans: HashSet::new(),
            unplaced: false,
            whole: true,
        }
    }

    /// One more, as it's known.
    fn insert(&mut self, object: Known) {
        self.ids.extend(object.id);
        match object.at {
            Some(at) => {
                self.spans.insert(at);
            }
            None => self.unplaced = true,
        }
    }

    /// Whether `object` is one of them. An indirect object, by its number
    /// alone: it is when one has that number, and else isn't, as one
    /// written directly never is an indirect object. Without a number, by
    /// where it was written: it is when one was written there; it isn't
    /// when there are none, or it and each of them were placed, elsewhere;
    /// else it can't be told, none.
    fn has(&self, object: Known) -> Option<bool> {
        if let Some(id) = object.id {
            return Some(self.ids.contains(&id));
        }
        let none = self.ids.is_empty() && self.spans.is_empty() && !self.unplaced;
        match object.at {
            Some(at) if self.spans.contains(&at) => Some(true),
            Some(_) if !self.unplaced => Some(false),
            _ => none.then_some(false),
        }
    }
}

/// An object as the structure tree's content items and the name trees'
/// roots are known: by where it was written, and by its number when it's
/// an indirect object. Either may be unknown: an object built in memory,
/// or edited, has no span, and one written directly, no number. Two
/// objects of a file never share a number, but may share a span, as an
/// object stream's header can start two of its objects at one offset, or
/// one inside another; so where there's a number, it says. Not by where
/// it's kept, which a lazy reader needn't keep the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Known {
    at: Option<Span>,
    id: Option<ObjectId>,
}

/// The keys of a file's name trees, each kept once read, the first kept
/// where two threads read one at once, by the tree's root,
/// the object its own entry comes to, as it's [`Known`]: where it was
/// written and the number of the object its entry's references end at
/// ([`entry_id`]), both, so that two roots written at one span are kept
/// apart by their numbers; so it serves one file. A root known by neither
/// is read each time it's asked of. Not by where it's kept: a root in a
/// container given for a row may be dropped after it, and another root
/// take its place.
#[derive(Debug, Default)]
struct Trees(Mutex<HashMap<Known, Arc<Tree>>>);

impl Trees {
    /// The trees read so far. A map that a panic poisoned is read as it
    /// is: a tree is stored whole, or not at all.
    fn lock(&self) -> MutexGuard<'_, HashMap<Known, Arc<Tree>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

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
                "fn:SinceVersion" => cx.file.version >= v,
                "fn:BeforeVersion" => cx.file.version < v,
                _ => cx.file.version == v,
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
/// written, then what stopped the parser, and where when the parser knows:
/// a byte offset in it, or the text it stopped at.
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
        ("fn:Extension", [Expr::Path(name)]) => Tri::Is(cx.file.extensions.contains(&name.text)),
        ("fn:Extension", [Expr::Path(name), statement]) => {
            match cx.file.extensions.contains(&name.text) {
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
        ("fn:NoCycle", []) => no_cycle(f, cx),
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
        ("fn:ImageIsStructContentItem", []) => content_item(f, &items(cx).objects, cx),
        ("fn:PageContainsStructContentItems", []) => content_item(f, &items(cx).pages, cx),
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

/// Whether the row's key, followed from the row's container through each
/// object its value refers to, keyed the same, never comes back to an
/// object met on the way (`fn:NoCycle`: "no cycles (loops) when using the
/// current key or array index to key into the linked list of objects").
/// True where it ends: at an object without the key, or whose value isn't
/// a reference, or names no object. False where it comes to one met
/// already, the container among them, known by its number where
/// [`Cx::id`] gives it, else by where it was written. Unknown past
/// [`HOPS`] objects.
fn no_cycle(f: &str, cx: &Cx) -> Tri {
    let mut met: HashSet<ObjectId> = cx.id.into_iter().collect();
    let written = cx.container.at.filter(|_| cx.id.is_none());
    let mut o = cx.container;
    for _ in 0..HOPS {
        let Some(id) = entry(o, cx.at, cx).and_then(Object::as_reference) else {
            return Tri::Is(true);
        };
        if !met.insert(id) {
            return Tri::Is(false);
        }
        let Some(next) = cx.file.resolve.resolve(id) else {
            return Tri::Is(true);
        };
        if written.is_some() && next.at == written {
            return Tri::Is(false);
        }
        o = next;
    }
    Tri::Unknown(f.to_string())
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
        [] => Tri::Is(applies(cx.file.version, v)),
        [statement] if applies(cx.file.version, v) => truth(statement, cx),
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
fn walk<'c>(p: &Path, keys: &[String], cx: &Cx<'c, '_>) -> Result<Option<&'c Object>, String> {
    let (mut at, keys) = match (p.base, keys) {
        (Base::Here, [first, rest @ ..]) if wild(first) => {
            (value_at(cx.container, cx.at, cx), rest)
        }
        (Base::Here, _) => (Some(cx.container), keys),
        (Base::Parent(n), _) => (parent(n, keys, cx), keys),
        (Base::Trailer, [first, rest @ ..]) => {
            let entry = cx.file.trailer.get(first.as_bytes());
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
fn parent<'c>(n: usize, keys: &[String], cx: &Cx<'c, '_>) -> Option<&'c Object> {
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
fn step<'c>(o: &'c Object, key: &str, cx: &Cx<'c, '_>) -> Option<Option<&'c Object>> {
    let at = match deref(o, cx).map(|o| o.value()) {
        None => return Some(None),
        Some(Value::Array(_)) => At::Index(key.parse().ok()?),
        Some(Value::Dict(_) | Value::Stream(_)) => At::Key(key.as_bytes()),
        Some(_) => return None,
    };
    Some(value_at(o, at, cx))
}

/// `o`'s entry at `at`, a stream's from its dictionary; none for null.
fn value_at<'c>(o: &'c Object, at: At, cx: &Cx<'c, '_>) -> Option<&'c Object> {
    deref(entry(o, at, cx)?, cx)
}

/// `o`'s entry at `at` as written, its references not followed.
fn entry<'c>(o: &'c Object, at: At, cx: &Cx<'c, '_>) -> Option<&'c Object> {
    match (deref(o, cx)?.value(), at) {
        (Value::Dict(d), At::Key(key)) => d.get(key),
        (Value::Stream(s), At::Key(key)) => s.dict.get(key),
        (Value::Array(a), At::Index(i)) => a.get(i),
        _ => None,
    }
}

/// `o` with its references followed, at most [`HOPS`]: none for null, or
/// for a reference to an object the file hasn't, which is null (ISO
/// 32000-2, 7.3.10).
fn deref<'c>(mut o: &'c Object, cx: &Cx<'c, '_>) -> Option<&'c Object> {
    for _ in 0..HOPS {
        match o.value() {
            Value::Ref(id) => o = cx.file.resolve.resolve(*id)?,
            Value::Null => return None,
            _ => return Some(o),
        }
    }
    None
}

/// How the object `o` comes to, its references followed as [`deref()`]
/// follows them, is [`Known`]: by where it was written, and the number of
/// the last of them, none when `o` is written directly.
fn deref_known<'c>(mut o: &'c Object, cx: &Cx<'c, '_>) -> Option<Known> {
    let mut id = None;
    for _ in 0..HOPS {
        match o.value() {
            Value::Ref(to) => {
                id = Some(*to);
                o = cx.file.resolve.resolve(*to)?;
            }
            Value::Null => return None,
            _ => return Some(Known { at: o.at, id }),
        }
    }
    None
}

/// Whether `o` is a dictionary or array with an entry that isn't null.
fn has_entry<'c>(o: &'c Object, cx: &Cx<'c, '_>) -> bool {
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
            ("fn:FileSize", []) => (cx.file.size)
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
fn array<'c>(k: &Expr, cx: &Cx<'c, '_>) -> Result<&'c [Object], String> {
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
    let known = Known {
        at: root.at,
        id: entry_id(tree, cx),
    };
    // Read with no lock held, as reading it resolves; two threads may
    // both read it, and the first stored is kept.
    let tree = match known {
        Known { at: None, id: None } => Arc::new(name_tree(root, cx)),
        known => {
            let kept = cx.file.trees.lock().get(&known).cloned();
            kept.unwrap_or_else(|| {
                let read = Arc::new(name_tree(root, cx));
                Arc::clone(cx.file.trees.lock().entry(known).or_insert(read))
            })
        }
    };
    match (tree.keys.contains(key), tree.whole) {
        (true, _) => Tri::Is(true),
        (false, true) => Tri::Is(false),
        (false, false) => Tri::Unknown(f.to_string()),
    }
}

/// The number of the object at the end of `p` when its own entry, the
/// last on the way, is a reference: the last of the references that
/// entry starts, as [`deref()`] follows them, never one followed before it.
/// None when the entry is written directly, `p` names none (the container
/// itself, or a parent), or doesn't reach one.
fn entry_id(p: &Path, cx: &Cx) -> Option<ObjectId> {
    let (last, before) = p.keys.split_last()?;
    let raw = match (p.base, before) {
        (Base::Here, []) if wild(last) => entry(cx.container, cx.at, cx),
        (Base::Trailer, []) => cx.file.trailer.get(last.as_bytes()),
        _ => {
            // What holds the entry, as [`walk`] reaches it: a parent by the
            // whole path, whose first key says which.
            let holder = match (p.base, before) {
                (Base::Parent(n), []) => parent(n, &p.keys, cx),
                _ => walk(p, before, cx).ok().flatten(),
            }?;
            let at = match deref(holder, cx)?.value() {
                Value::Array(_) => At::Index(last.parse().ok()?),
                _ => At::Key(last.as_bytes()),
            };
            entry(holder, at, cx)
        }
    }?;
    deref_known(raw, cx)?.id
}

/// The keys of the name tree at `root` (ISO 32000-2, 7.9.6): each node's
/// `/Names` pairs' first, through its `/Kids`, each node once, as deep and
/// as many as [`TREE_DEPTH`] and [`TREE_NODES`] let.
fn name_tree<'c>(root: &'c Object, cx: &Cx<'c, '_>) -> Tree {
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
    // Read outside the cell, as reading them resolves; the first kept.
    let fields = match cx.file.fields.0.get() {
        Some(fields) => fields,
        None => {
            let read = field_names(cx);
            cx.file.fields.0.get_or_init(|| read)
        }
    };
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
fn field_names<'c>(cx: &Cx<'c, '_>) -> Names {
    let mut names = Names {
        names: HashSet::new(),
        whole: true,
    };
    let fields = (cx.file.trailer.get(b"Root").and_then(|o| deref(o, cx)))
        .and_then(|catalog| value_at(catalog, At::Key(b"AcroForm"), cx))
        .and_then(|form| value_at(form, At::Key(b"Fields"), cx));
    let Some(fields) = fields else {
        return names;
    };
    let mut seen = HashSet::new();
    // Each node, its parent's name, and its depth.
    let mut nodes: Vec<(&'c Object, Option<Rc<str>>, usize)> = vec![(fields, None, 0)];
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

/// Whether the row's container is one of `found`, the objects the
/// structure tree points at or the pages it puts marked content on,
/// known by the number [`Cx::id`] gives, else by its span
/// ([`Objects::has`]): false when each is told apart from it and they're
/// all there are, else unknown.
fn content_item(f: &str, found: &Objects, cx: &Cx) -> Tri {
    let container = Known {
        at: cx.container.at,
        id: cx.id,
    };
    match (found.has(container), found.whole) {
        (Some(true), _) => Tri::Is(true),
        (Some(false), true) => Tri::Is(false),
        (Some(false), false) | (None, _) => Tri::Unknown(f.to_string()),
    }
}

/// The structure tree's content items ([`Items`]), read the first time
/// they're asked for: outside the cell, as reading them resolves, the
/// first kept.
fn items<'c>(cx: &Cx<'c, '_>) -> &'c Items {
    match cx.file.structure.0.get() {
        Some(items) => items,
        None => {
            let read = structure(cx);
            cx.file.structure.0.get_or_init(|| read)
        }
    }
}

/// The content items of the structure tree from the catalog's
/// `/StructTreeRoot` (ISO 32000-2, 14.7.2 and 14.7.5): a structure
/// element's kids, through its `/K`, are elements, marked-content
/// identifiers (an integer, on the element's `/Pg`), marked-content
/// references (MCR, on their own `/Pg`, else the element's; one in a
/// form XObject's stream, `/Stm`, isn't on the page) and object references
/// (OBJR, to their `/Obj`). An MCR or OBJR is told by its `/Type`, or
/// without one by its `/MCID` or `/Obj` where an element would have `/S`.
/// Each page and object is [`Known`] as its entry, `/Pg` or `/Obj`, names
/// it. A file without a structure tree has no content items.
fn structure<'c>(cx: &Cx<'c, '_>) -> Items {
    let mut items = Items {
        objects: Objects::new(),
        pages: Objects::new(),
    };
    let address = |o: &Object| std::ptr::from_ref(o) as usize;
    let Some(root) = (cx.file.trailer.get(b"Root").and_then(|o| deref(o, cx)))
        .and_then(|catalog| value_at(catalog, At::Key(b"StructTreeRoot"), cx))
    else {
        return items;
    };
    let mut seen = HashSet::new();
    // Each node, the page of the element it's a kid of, and its depth.
    let mut nodes: Vec<(&'c Object, Option<Known>, usize)> = Vec::new();
    if let Some(kids) = value_at(root, At::Key(b"K"), cx) {
        nodes.push((kids, None, 0));
    }
    while let Some((node, page, depth)) = nodes.pop() {
        if depth > TREE_DEPTH || seen.len() >= TREE_NODES {
            items.objects.whole = false;
            items.pages.whole = false;
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
                Some(page) => items.pages.insert(page),
                None => items.pages.whole = false,
            },
            Value::Dict(d) => {
                if !seen.insert(address(node)) {
                    continue;
                }
                let name = |key: &[u8]| d.get(key).and_then(|o| deref(o, cx)?.as_name());
                let has = |key: &[u8]| d.get(key).and_then(|o| deref(o, cx)).is_some();
                let own_page = d.get(b"Pg").and_then(|o| deref_known(o, cx));
                let element = !has(b"S");
                if name(b"Type") == Some(b"MCR") || (element && has(b"MCID")) {
                    if has(b"Stm") {
                        continue;
                    }
                    match own_page.or(page) {
                        Some(page) => items.pages.insert(page),
                        None => items.pages.whole = false,
                    }
                } else if name(b"Type") == Some(b"OBJR") || (element && has(b"Obj")) {
                    match d.get(b"Obj").and_then(|o| deref_known(o, cx)) {
                        Some(object) => items.objects.insert(object),
                        None => items.objects.whole = false,
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

    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::Barrier;
    use std::thread::ThreadId;

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
            let file = FileCx::new(&self.trailer, v(version), &resolve)
                .size(self.size)
                .extensions(&self.extensions);
            f(&file.at(container, at).parents(parents).defaults(defaults))
        }

        /// What `f` says over one [`FileCx`] of this file at `version`,
        /// every row it makes reading the same; and how many times each
        /// object was resolved.
        fn counted<T>(
            &self,
            version: &str,
            f: impl FnOnce(&FileCx) -> T,
        ) -> (T, BTreeMap<ObjectId, usize>) {
            let resolved = RefCell::new(BTreeMap::new());
            let said = {
                let resolve = |id: ObjectId| {
                    *resolved.borrow_mut().entry(id).or_insert(0) += 1;
                    self.objects.get(&id)
                };
                let file = FileCx::new(&self.trailer, v(version), &resolve)
                    .size(self.size)
                    .extensions(&self.extensions);
                f(&file)
            };
            (said, resolved.into_inner())
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

    /// `fn:NoCycle`, the model's on an object stream's `/Extends`: the key
    /// followed through the objects it refers to ends, true; or comes back
    /// to one met on the way, the container among them, by its number or
    /// where it was written, false. Past as many objects as a value goes
    /// through to its object, it can't be told.
    #[test]
    fn no_cycle_follows_the_key_through_the_objects_it_names() {
        let cell = &row("ObjectStream", b"Extends").special_case[0];
        assert_eq!(cell, "fn:NoCycle()");
        // Object stream `n`, extending `to`, written at byte `n`.
        let container = |n: u32, to: Option<u32>| {
            let mut dict = dict! { "Type" => "ObjStm", "N" => 0, "First" => 0 };
            if let Some(to) = to {
                dict.set("Extends", Object::reference(to));
            }
            let stream = Object::from(pdf_objects::Stream::new(dict, Vec::new()));
            (n, stream.at(written(n as usize)))
        };
        let file = |links: &[(u32, Option<u32>)]| {
            let objects = links.iter().map(|&(n, to)| container(n, to));
            File::new(dict! { "Root" => Object::reference(1) }, objects)
        };
        // Whether object `n`'s /Extends has no cycle: given its number
        // where `id`, else known by where it was written.
        let no_cycle = |file: &File, n: u32, id: bool| {
            let o = &file.objects[&(n, 0)];
            file.eval(o, At::Key(b"Extends"), &[], "1.7", |cx| match id {
                true => holds(cell, &cx.id((n, 0))),
                false => holds(cell, cx),
            })
        };

        // A chain, 10 to 11 to 12, which extends none; and one without
        // /Extends.
        let chain = file(&[(10, Some(11)), (11, Some(12)), (12, None)]);
        for (n, id) in [(10, true), (10, false), (11, true), (12, true)] {
            assert_eq!(no_cycle(&chain, n, id), Truth::True, "{n}");
        }
        // A cycle of two, from either.
        let two = file(&[(10, Some(11)), (11, Some(10))]);
        for (n, id) in [(10, true), (10, false), (11, true), (11, false)] {
            assert_eq!(no_cycle(&two, n, id), Truth::False, "{n} {id}");
        }
        // One extending itself: by its number, by where it was written,
        // and by neither, once it comes back to itself.
        let own = file(&[(10, Some(10))]);
        assert_eq!(no_cycle(&own, 10, true), Truth::False);
        assert_eq!(no_cycle(&own, 10, false), Truth::False);
        let unplaced = Object::from(dict! { "Extends" => Object::reference(10) });
        let own = File::new(dict! {}, [(10, unplaced.clone())]);
        let said = own.eval(&unplaced, At::Key(b"Extends"), &[], "1.7", |cx| {
            holds(cell, cx)
        });
        assert_eq!(said, Truth::False);
        // A cycle entered past the start: 10 to 11 to 12 to 11.
        let looped = file(&[(10, Some(11)), (11, Some(12)), (12, Some(11))]);
        assert_eq!(no_cycle(&looped, 10, true), Truth::False);
        // A reference to no object ends it, as null.
        let none = file(&[(10, Some(11)), (11, Some(99))]);
        assert_eq!(no_cycle(&none, 10, true), Truth::True);
        // A chain of HOPS objects, the container among them, is told; one
        // of more can't be.
        let last = 10 + HOPS as u32 - 1;
        let links: Vec<_> = (10..last).map(|n| (n, Some(n + 1))).collect();
        let mut within = links.clone();
        within.push((last, None));
        assert_eq!(no_cycle(&file(&within), 10, true), Truth::True);
        let mut past = links;
        past.push((last, Some(last + 1)));
        past.push((last + 1, None));
        assert_eq!(no_cycle(&file(&past), 10, true), unknown("fn:NoCycle"));
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

    /// Bytes `n` of the file: where a value was written.
    fn written(n: usize) -> Span {
        Span::new(None, n, n + 1)
    }

    /// A name tree's leaf of `keys`.
    fn leaf(keys: &[&str]) -> Object {
        let names: Vec<Object> = (keys.iter())
            .flat_map(|k| [Object::string(k), Object::from(0)])
            .collect();
        dict! { "Names" => names }.into()
    }

    /// Whether each of `keys`, a row of a container of its own made for
    /// it, is in the name tree at `tree`, over `file`.
    fn in_tree(file: &FileCx, tree: &str, keys: &[&str]) -> Vec<Truth> {
        let text = format!("fn:InNameTree({tree})");
        (keys.iter())
            .map(|key| {
                let container = Object::from(dict! { "X" => Object::string(key) });
                holds(&text, &file.at(&container, At::Key(b"X")))
            })
            .collect()
    }

    #[test]
    fn a_name_tree_is_read_once_for_every_row_of_a_file() {
        // The catalog's /Dests, by reference, over a kid by reference,
        // which only reading the tree resolves: each row's own path
        // resolves the catalog and the root, and again for the number its
        // entry ends at.
        let catalog =
            dict! { "Type" => "Catalog", "Names" => dict! { "Dests" => Object::reference(2) } };
        let root = dict! { "Kids" => vec![Object::reference(3)] };
        let file = File::new(
            dict! { "Root" => Object::reference(1) },
            [
                (1, Object::from(catalog).at(written(10))),
                (2, Object::from(root).at(written(20))),
                (3, leaf(&["a", "b"]).at(written(30))),
            ],
        );
        let tree = "trailer::Catalog::Names::Dests";
        let (said, resolved) = file.counted("1.7", |file| {
            in_tree(file, tree, &["a", "b", "c", "a", "b", "c"])
        });
        let (t, f) = (Truth::True, Truth::False);
        assert_eq!(said, [t.clone(), t.clone(), f.clone(), t.clone(), t, f]);
        assert_eq!(resolved[&(3, 0)], 1, "the kid, read once");
        assert_eq!(resolved[&(2, 0)], 12, "the root, twice a row");
    }

    #[test]
    fn a_name_tree_is_kept_by_where_its_root_was_written_or_its_number() {
        // A root over a kid by reference, 3, which only reading the tree
        // resolves: the catalog's /Dests, written directly or by
        // reference, to 2; placed or not.
        let root = || Object::from(dict! { "Kids" => vec![Object::reference(3)] });
        let file = |dests: Object, two: Option<Object>| {
            let catalog = dict! { "Type" => "Catalog", "Names" => dict! { "Dests" => dests } };
            let mut objects = vec![(1, Object::from(catalog)), (3, leaf(&["a"]))];
            objects.extend(two.map(|two| (2, two)));
            File::new(dict! { "Root" => Object::reference(1) }, objects)
        };
        // The answers, each the same; and how often the kid is read.
        let reads = |file: &File| {
            let (said, resolved) = file.counted("1.7", |file| {
                in_tree(
                    file,
                    "trailer::Catalog::Names::Dests",
                    &["a", "c", "a", "c"],
                )
            });
            let (t, f) = (Truth::True, Truth::False);
            assert_eq!(said, [t.clone(), f.clone(), t, f]);
            resolved[&(3, 0)]
        };
        // Where it was written, directly or by reference.
        assert_eq!(reads(&file(root().at(written(20)), None)), 1);
        let two = Some(root().at(written(20)));
        assert_eq!(reads(&file(Object::reference(2), two)), 1);
        // Not placed: by its number.
        assert_eq!(reads(&file(Object::reference(2), Some(root()))), 1);
        // Neither: read for each row.
        assert_eq!(reads(&file(root(), None)), 4);

        // A container's own, built in memory: read for each row; placed,
        // once.
        for (root, reads) in [(root(), 4), (root().at(written(40)), 1)] {
            let file = file(Object::null(), None);
            let ((), resolved) = file.counted("1.7", |file| {
                for (key, holds_it) in [("a", true), ("c", false), ("a", true), ("c", false)] {
                    let container =
                        Object::from(dict! { "T" => root.clone(), "X" => Object::string(key) });
                    let said = holds("fn:InNameTree(T)", &file.at(&container, At::Key(b"X")));
                    assert_eq!(said == Truth::True, holds_it, "{key}");
                }
            });
            assert_eq!(resolved[&(3, 0)], reads);
        }
    }

    #[test]
    fn two_name_trees_written_directly_in_one_catalog_each_hold_their_own_keys() {
        // /Dests and /EmbeddedFiles, each written directly in the
        // catalog's /Names, itself direct: no reference names either but
        // the catalog's, which names both.
        for placed in [true, false] {
            let (mut dests, mut files) = (leaf(&["d"]), leaf(&["e"]));
            if placed {
                (dests, files) = (dests.at(written(20)), files.at(written(30)));
            }
            let names = dict! { "Dests" => dests, "EmbeddedFiles" => files };
            let catalog = dict! { "Type" => "Catalog", "Names" => names };
            let file = File::new(
                dict! { "Root" => Object::reference(1) },
                [(1, Object::from(catalog).at(written(10)))],
            );
            let (said, _) = file.counted("1.7", |file| {
                let tree = |name: &str| format!("trailer::Catalog::Names::{name}");
                [
                    in_tree(file, &tree("Dests"), &["d", "e"]),
                    in_tree(file, &tree("EmbeddedFiles"), &["d", "e"]),
                    in_tree(file, &tree("Dests"), &["d", "e"]),
                ]
            });
            let (t, f) = (Truth::True, Truth::False);
            let dests = vec![t.clone(), f.clone()];
            assert_eq!(said, [dests.clone(), vec![f, t], dests], "placed: {placed}");
        }
    }

    #[test]
    fn two_name_trees_written_at_one_span_are_told_apart_by_their_numbers() {
        // /Dests, 2, and /EmbeddedFiles, 3, copied from two files, each
        // keeping where it was written in its own: the same span.
        let names =
            dict! { "Dests" => Object::reference(2), "EmbeddedFiles" => Object::reference(3) };
        let catalog = dict! { "Type" => "Catalog", "Names" => names };
        let file = File::new(
            dict! { "Root" => Object::reference(1) },
            [
                (1, Object::from(catalog)),
                (2, leaf(&["d"]).at(written(20))),
                (3, leaf(&["e"]).at(written(20))),
            ],
        );
        let (said, _) = file.counted("1.7", |file| {
            let tree = |name: &str| format!("trailer::Catalog::Names::{name}");
            [
                in_tree(file, &tree("Dests"), &["d", "e"]),
                in_tree(file, &tree("EmbeddedFiles"), &["d", "e"]),
                in_tree(file, &tree("Dests"), &["d", "e"]),
            ]
        });
        let (t, f) = (Truth::True, Truth::False);
        let dests = vec![t.clone(), f.clone()];
        assert_eq!(said, [dests.clone(), vec![f, t], dests]);
    }

    #[test]
    fn rows_of_containers_made_and_dropped_in_turn_read_their_own_trees() {
        // One file; containers made for a row and dropped after it, each
        // with a tree of its own written directly under the same key, and
        // a row of its own key and of the one before's.
        let file = bare();
        let key = |i: usize| Object::string(&format!("k{i}"));
        let tree = |i: usize, placed: bool| {
            let tree = Object::from(dict! { "Names" => vec![key(i), Object::from(0)] });
            match placed {
                true => tree.at(written(100 + 10 * i)),
                false => tree,
            }
        };
        let rows = |file: &FileCx, container: &Object| {
            let row = |at: &[u8]| holds("fn:InNameTree(T)", &file.at(container, At::Key(at)));
            (row(b"X"), row(b"Y"))
        };
        let own = vec![(Truth::True, Truth::False); 8];
        for placed in [true, false] {
            let (said, _) = file.counted("1.7", |file| {
                (1..9)
                    .map(|i| {
                        let container =
                            dict! { "T" => tree(i, placed), "X" => key(i), "Y" => key(i - 1) };
                        rows(file, &container.into())
                    })
                    .collect::<Vec<_>>()
            });
            assert_eq!(said, own, "placed: {placed}");
            // One container, its tree replaced in turn: each kept where the
            // one before it was.
            let (said, _) = file.counted("1.7", |file| {
                let mut container = Object::from(dict! { "T" => Object::null() });
                let mut kept = None;
                (1..9)
                    .map(|i| {
                        let dict = container.as_dict_mut().unwrap();
                        dict.set("T", tree(i, placed));
                        dict.set("X", key(i));
                        dict.set("Y", key(i - 1));
                        let at = std::ptr::from_ref(dict.get(b"T").unwrap());
                        assert_eq!(*kept.get_or_insert(at), at, "kept in the same place");
                        rows(file, &container)
                    })
                    .collect::<Vec<_>>()
            });
            assert_eq!(said, own, "placed: {placed}, in turn in one container");
        }
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
            item_in(file, container, Some((n, 0)), text)
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

    /// `text`, a content item's function, held of `container` in `file`:
    /// the indirect object `id`, when it's one.
    fn item_in(file: &File, container: &Object, id: Option<ObjectId>, text: &str) -> Truth {
        file.eval(container, At::Key(b"X"), &[], "1.7", |cx| match id {
            Some(id) => holds(text, &cx.id(id)),
            None => holds(text, cx),
        })
    }

    /// A file of `objects` and a catalog, 1, whose structure tree's one
    /// element puts marked content on its `/Pg`, `pg`, by an MCID, and
    /// points at `obj`, an object reference's `/Obj`.
    fn tagged(pg: Object, obj: Object, objects: impl IntoIterator<Item = (u32, Object)>) -> File {
        let objr = Object::from(dict! { "Type" => "OBJR", "Obj" => obj });
        let element = dict! { "S" => "P", "Pg" => pg, "K" => vec![Object::from(0), objr] };
        let root = dict! { "Type" => "StructTreeRoot", "K" => element };
        let catalog = dict! { "Type" => "Catalog", "StructTreeRoot" => root };
        let mut all = vec![(1, Object::from(catalog))];
        all.extend(objects);
        File::new(dict! { "Root" => Object::reference(1) }, all)
    }

    #[test]
    fn a_content_item_is_known_by_where_it_was_written_or_its_number() {
        let page = || Object::from(dict! { "Type" => "Page" });
        let image = || Object::from(dict! { "Subtype" => "Image" });
        let (pages, images) = (
            "fn:PageContainsStructContentItems()",
            "fn:ImageIsStructContentItem()",
        );
        let unknown_page = unknown("fn:PageContainsStructContentItems");
        let unknown_image = unknown("fn:ImageIsStructContentItem");
        let (t, f) = (Truth::True, Truth::False);
        // Pages 2 and 5 and an image, 6, placed or not; the tree names
        // page 2 and the image by reference.
        let file = |placed: bool| {
            let at = |o: Object, n| if placed { o.at(written(n)) } else { o };
            let objects = [
                (2, at(page(), 20)),
                (5, at(page(), 50)),
                (6, at(image(), 60)),
            ];
            tagged(Object::reference(2), Object::reference(6), objects)
        };
        let object = |file: &File, n| file.objects.get(&(n, 0)).unwrap().clone();

        // Placed: each item known by where it was written and its number.
        let placed = file(true);
        // By where: the object as resolved, or a copy that keeps its span,
        // without its number.
        assert_eq!(item_in(&placed, &object(&placed, 2), None, pages), t);
        assert_eq!(item_in(&placed, &object(&placed, 6), None, images), t);
        // By its number: an equal copy built in memory, which has no span,
        // where its address said it wasn't.
        assert_eq!(item_in(&placed, &page(), Some((2, 0)), pages), t);
        assert_eq!(item_in(&placed, &image(), Some((6, 0)), images), t);
        // Told apart by either: another page, as resolved or numbered.
        assert_eq!(item_in(&placed, &object(&placed, 5), None, pages), f);
        assert_eq!(item_in(&placed, &page(), Some((5, 0)), pages), f);
        // A page written directly, elsewhere: no item's span is its.
        assert_eq!(item_in(&placed, &page().at(written(90)), None, pages), f);
        // An equal copy without its number: nothing tells it apart.
        assert_eq!(item_in(&placed, &page(), None, pages), unknown_page);
        assert_eq!(item_in(&placed, &image(), None, images), unknown_image);

        // Not placed: each item known by its number alone.
        let unplaced = file(false);
        assert_eq!(item_in(&unplaced, &page(), Some((2, 0)), pages), t);
        assert_eq!(item_in(&unplaced, &page(), Some((5, 0)), pages), f);
        // Without its number, the object resolve gives can't be told: its
        // address no longer says.
        let two = object(&unplaced, 2);
        assert_eq!(item_in(&unplaced, &two, None, pages), unknown_page);
        // A page known by its span alone, beside items known by their
        // numbers alone, can't be told apart from them either.
        let direct = page().at(written(90));
        assert_eq!(item_in(&unplaced, &direct, None, pages), unknown_page);

        // Beside no content items, an object nothing identifies isn't one.
        let catalog = dict! { "Type" => "Catalog" };
        let bare = File::new(
            dict! { "Root" => Object::reference(1) },
            [(1, catalog.into())],
        );
        assert_eq!(item_in(&bare, &page(), None, pages), f);
        assert_eq!(item_in(&bare, &image(), None, images), f);

        // An object an item names directly, placed: known by where it was
        // written, which no number could say.
        let inline = image().at(written(70));
        let direct = tagged(
            Object::reference(2),
            inline.clone(),
            [(2, page()), (6, image())],
        );
        assert_eq!(item_in(&direct, &inline, None, images), t);
        assert_eq!(item_in(&direct, &image().at(written(60)), None, images), f);

        // An item the reader didn't place, written directly: never an
        // indirect object, so not image 6; a container nothing identifies
        // could be it, though no other item is.
        let lost = tagged(
            Object::reference(2),
            image(),
            [(2, page().at(written(20))), (6, image().at(written(60)))],
        );
        let six = object(&lost, 6);
        assert_eq!(item_in(&lost, &six, Some((6, 0)), images), f);
        assert_eq!(item_in(&lost, &image(), None, images), unknown_image);
        assert_eq!(item_in(&lost, &page(), Some((2, 0)), pages), t);

        // Named through a chain of references, 7 to 6: by the last, the
        // object the image is, never one followed before it.
        let chain = tagged(
            Object::reference(2),
            Object::reference(7),
            [(2, page()), (6, image()), (7, Object::reference(6))],
        );
        assert_eq!(item_in(&chain, &image(), Some((6, 0)), images), t);
        assert_eq!(item_in(&chain, &image(), Some((7, 0)), images), f);

        // Pages 3 and 4, which an object stream's header starts at one
        // offset, and the tree names 4: one span, told apart by their
        // numbers; given none, 3 is known by its span alone, 4's.
        let at = Span::new(Some((9, 0)), 8, 80);
        let twins = tagged(
            Object::reference(4),
            Object::reference(6),
            [(3, page().at(at)), (4, page().at(at)), (6, image())],
        );
        assert_eq!(item_in(&twins, &object(&twins, 4), Some((4, 0)), pages), t);
        assert_eq!(item_in(&twins, &object(&twins, 3), Some((3, 0)), pages), f);
        assert_eq!(item_in(&twins, &page(), Some((3, 0)), pages), f);
        assert_eq!(item_in(&twins, &object(&twins, 3), None, pages), t);

        // The element's /Pg written directly, at bytes where an object
        // stream's header starts page 3 too: one span, but a value written
        // directly is never page 3.
        let inside = tagged(
            page().at(at),
            Object::reference(6),
            [(3, page().at(at)), (6, image())],
        );
        assert_eq!(
            item_in(&inside, &object(&inside, 3), Some((3, 0)), pages),
            f
        );
        assert_eq!(item_in(&inside, &page().at(at), None, pages), t);
    }

    #[test]
    fn a_page_written_directly_as_an_elements_pg_is_known_by_where_it_was_written() {
        // The element's /Pg is the page itself: its MCIDs are on that page,
        // which only where it was written can name.
        let page = || Object::from(dict! { "Type" => "Page" });
        let pages = "fn:PageContainsStructContentItems()";
        let image = Object::from(dict! { "Subtype" => "Image" });
        let file = |pg: &Object| {
            let objects = [
                (2, page().at(written(20))),
                (6, image.clone().at(written(60))),
            ];
            tagged(pg.clone(), Object::reference(6), objects)
        };
        // The page as the walk reaches it in the tree: the same span.
        let inline = page().at(written(80));
        let placed = file(&inline);
        assert_eq!(item_in(&placed, &inline, None, pages), Truth::True);
        let two = placed.objects.get(&(2, 0)).unwrap();
        assert_eq!(item_in(&placed, two, Some((2, 0)), pages), Truth::False);
        // Not placed, it's known by neither, but written directly, it isn't
        // page 2; a page nothing identifies could be it.
        let unplaced = file(&page());
        let two = unplaced.objects.get(&(2, 0)).unwrap();
        assert_eq!(item_in(&unplaced, two, Some((2, 0)), pages), Truth::False);
        assert_eq!(
            item_in(&unplaced, &page(), None, pages),
            unknown("fn:PageContainsStructContentItems")
        );
    }

    #[test]
    fn an_object_streams_member_isnt_what_the_file_has_at_its_offsets() {
        // Image 6 is in object stream 9, at bytes 20 to 21 of its decoded
        // data; an image written at bytes 20 to 21 of the file isn't it.
        let images = "fn:ImageIsStructContentItem()";
        let image = || Object::from(dict! { "Subtype" => "Image" });
        let member = image().at(Span::new(Some((9, 0)), 20, 21));
        let page = Object::from(dict! { "Type" => "Page" }).at(written(40));
        let file = tagged(
            Object::reference(2),
            Object::reference(6),
            [(2, page), (6, member.clone())],
        );
        assert_eq!(item_in(&file, &member, None, images), Truth::True);
        let at_its_offsets = image().at(written(20));
        assert_eq!(item_in(&file, &at_its_offsets, None, images), Truth::False);
    }

    /// A boxed resolver over `file`'s objects. Written in a `let`, its type
    /// would need the objects' lifetime, which only a signature can name.
    fn boxed<'f>(file: &'f File) -> Box<dyn Fn(ObjectId) -> Option<&'f Object> + 'f> {
        Box::new(|id| file.objects.get(&id))
    }

    /// Whether `file`'s catalog, by reference, has `/X`, and has `/Y`,
    /// through a `FileCx` over any resolver.
    fn present(file: &FileCx) -> (Truth, Truth) {
        let row = |text| holds(text, &file.at(&Object::null(), At::Key(b"X")));
        let (x, y) = (
            "fn:IsPresent(trailer::Catalog::X)",
            "fn:IsPresent(trailer::Catalog::Y)",
        );
        (row(x), row(y))
    }

    #[test]
    fn a_file_resolves_through_a_closure_a_dyn_fn_or_a_box() {
        let catalog = dict! { "Type" => "Catalog", "X" => 7 };
        let file = File::new(
            dict! { "Root" => Object::reference(1) },
            [(1, catalog.into())],
        );
        let (t, f) = (Truth::True, Truth::False);
        let resolve = |id: ObjectId| file.objects.get(&id);
        let by_value = FileCx::new(&file.trailer, v("1.7"), resolve);
        assert_eq!(present(&by_value), (t.clone(), f.clone()));
        let by_reference = FileCx::new(&file.trailer, v("1.7"), &resolve);
        assert_eq!(present(&by_reference), (t.clone(), f.clone()));
        let dynamic = FileCx::new(&file.trailer, v("1.7"), &resolve as &dyn Fn(_) -> _);
        assert_eq!(present(&dynamic), (t.clone(), f.clone()));
        let boxed = FileCx::new(&file.trailer, v("1.7"), boxed(&file));
        assert_eq!(present(&boxed), (t, f));
    }

    #[test]
    fn a_file_over_a_sync_resolver_is_send_and_sync() {
        fn shared<T: Send + Sync>(_: &T) {}
        let file = bare();
        let resolve = |id: ObjectId| file.objects.get(&id);
        shared(&FileCx::new(&file.trailer, v("1.7"), resolve));
        shared(&FileCx::new(&file.trailer, v("1.7"), &resolve));
    }

    /// The rows a thread makes of `file`, each by [`FileCx::at`]: three keys
    /// asked of the catalog's `/Dests` tree; an image and pages asked of
    /// the structure tree's content items; and three names of the form's
    /// fields.
    fn rows<'a, R: Resolve<'a>>(
        file: &FileCx<'a, R>,
        objects: &BTreeMap<ObjectId, Object>,
    ) -> Vec<Truth> {
        let row = |text: &str, value: &str| {
            let container = Object::from(dict! { "X" => Object::string(value) });
            holds(text, &file.at(&container, At::Key(b"X")))
        };
        let item = |text: &str, n: u32| {
            let container = &objects[&(n, 0)];
            holds(text, &file.at(container, At::Key(b"X")).id((n, 0)))
        };
        let dests = "fn:InNameTree(trailer::Catalog::Names::Dests)";
        let (images, pages) = (
            "fn:ImageIsStructContentItem()",
            "fn:PageContainsStructContentItems()",
        );
        let fqn = "fn:IsFullyQualifiedFieldName(@X)";
        vec![
            row(dests, "a"),
            row(dests, "b"),
            row(dests, "c"),
            item(images, 6),
            item(images, 5),
            item(pages, 4),
            item(pages, 5),
            row(fqn, "a.b"),
            row(fqn, "a"),
            row(fqn, "b"),
        ]
    }

    #[test]
    fn one_file_shared_by_threads_reads_each_tree_at_most_once_a_thread() {
        // A name tree over a kid, 3; a structure tree over an element, 8,
        // which puts marked content on page 4 and points at image 6; and a
        // form over a field, 9, with a kid, 10. Only reading its tree
        // resolves each of 3, 8 and 10.
        let names = dict! { "Dests" => Object::reference(2) };
        let form = dict! { "Fields" => vec![Object::reference(9)] };
        let catalog = dict! {
            "Type" => "Catalog",
            "Names" => names,
            "StructTreeRoot" => Object::reference(7),
            "AcroForm" => form,
        };
        let objr = Object::from(dict! { "Type" => "OBJR", "Obj" => Object::reference(6) });
        let element =
            dict! { "S" => "P", "Pg" => Object::reference(4), "K" => vec![Object::from(0), objr] };
        let page = |n| Object::from(dict! { "Type" => "Page" }).at(written(n));
        let image = Object::from(dict! { "Subtype" => "Image" }).at(written(60));
        let root = dict! { "Type" => "StructTreeRoot", "K" => Object::reference(8) };
        let a = dict! { "T" => Object::string("a"), "Kids" => vec![Object::reference(10)] };
        let b = dict! { "T" => Object::string("b"), "Parent" => Object::reference(9) };
        let file = File::new(
            dict! { "Root" => Object::reference(1) },
            [
                (1, Object::from(catalog)),
                (2, dict! { "Kids" => vec![Object::reference(3)] }.into()),
                (3, leaf(&["a", "b"])),
                (4, page(40)),
                (5, page(50)),
                (6, image),
                (7, root.into()),
                (8, element.into()),
                (9, a.into()),
                (10, b.into()),
            ],
        );
        // Each object resolved, counted by the thread that resolved it.
        let counts = Mutex::new(HashMap::new());
        let resolve = |id: ObjectId| {
            let thread = std::thread::current().id();
            *counts.lock().unwrap().entry((thread, id)).or_insert(0) += 1;
            file.objects.get(&id)
        };
        let read = |counts: &HashMap<_, usize>, thread, id| counts.get(&(thread, id)).copied();
        let kids = [(3, 0), (8, 0), (10, 0)];

        // One thread alone: each tree read once.
        let alone = rows(
            &FileCx::new(&file.trailer, v("1.7"), &resolve),
            &file.objects,
        );
        let (t, f) = (Truth::True, Truth::False);
        let (dests, images, pages) = (
            [t.clone(), t.clone(), f.clone()],
            [t.clone(), f.clone()],
            [t.clone(), f.clone()],
        );
        assert_eq!(
            alone,
            [&dests[..], &images, &pages, &[t.clone(), t, f]].concat()
        );
        let mut said = std::mem::take(&mut *counts.lock().unwrap());
        let here = std::thread::current().id();
        for id in kids {
            assert_eq!(read(&said, here, id), Some(1), "{id:?}");
        }

        // Eight, sharing one FileCx: each makes its own rows and says what
        // one alone does, and reads each tree at most once.
        let shared = FileCx::new(&file.trailer, v("1.7"), &resolve);
        let start = Barrier::new(8);
        let threads: Vec<ThreadId> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        assert_eq!(rows(&shared, &file.objects), alone);
                        std::thread::current().id()
                    })
                })
                .collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        said = counts.into_inner().unwrap();
        for id in kids {
            let each: Vec<usize> = (threads.iter())
                .map(|&thread| read(&said, thread, id).unwrap_or(0))
                .collect();
            assert!(each.iter().all(|&n| n <= 1), "{id:?}: {each:?}");
            assert!(each.contains(&1), "{id:?}: {each:?}");
        }
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
