//! The command line: each file read from its bytes and checked against the
//! Arlington PDF Model, each finding printed with the bytes it's at.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::num::NonZero;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, mpsc};
use std::thread;

use anyhow::{Context, Result, bail};
use pdf_lint::bytes::{Decode, Written};
use pdf_lint::{
    Allowances, Class, Document, Exempt, Finding, Grammar, Hold, Made, Object, ObjectId, Options,
    Qpdf, Report, Source, Span, Stream, UnlistedIn, Value, Version, bytes, check, file_version,
    use_model,
};

const USAGE: &str = "\
usage: pdf-lint [OPTIONS] FILE...

Checks each FILE against the Arlington PDF Model, reading it from its
bytes, and prints each finding with the bytes it's at. What's compressed
or encrypted, qpdf decodes: object streams, cross-reference streams and
strings. Without qpdf 11.9 or later, a file that has any isn't checked.

  --hold <FILE>   hold only the objects FILE names, a line each:
                  `12 0 built` or `3 0 merged`
  --built         hold every object as built, so that a key the model
                  doesn't list is a finding (by default, as merged)
  --exempt <FILE> keys a built object may have though the model doesn't
                  list them, a name a line: `/MyKey`, or `/My*` for every
                  key it begins
  --unlisted <W>  which held objects a key the model doesn't list is a
                  finding in: built (the default), all, or never; it's
                  counted in any
  --deprecations  find what the version checked at deprecates: a key, a
                  type or a value (by default, counted only)
  --extensions <E>
                  check each FILE under these extensions too, besides
                  those it declares: the model's names for them, a comma
                  between (Malforms,AAPL)
  --last-update   hold only the objects the newest incremental update
                  wrote: those it rewrote as merged, new ones as built;
                  and leave out what a rewritten object, or the trailer,
                  already had in the revision it updates
  --baseline <FILE>
                  leave out the findings FILE, the file each FILE was
                  written from, already had: an incremental update of it
                  object by object, a rewrite of it by where each finding
                  is, or, where FILE has nothing there, anywhere in it
  --pdf-version <V>
                  check each FILE as PDF V, 1.0 to 1.7 or 2.0, not as
                  the version its header and catalog give, and what it's
                  set against, a baseline or the revision before an
                  update, as V too
  --model <DIR>   check against the tables in DIR, a checkout of the
                  Arlington model's tsv/latest, as they are, not the
                  embedded ones, whose cells that contradict ISO 32000-2
                  are corrected
  --one-table     check each object as one table, as TestGrammar does:
                  a field and its widget merged in one dictionary as
                  one of the two (by default, as both), and a key
                  unlisted by the first table that reaches an object
                  though another that does lists it
  --allow <TSV>   leave out the findings TSV allows: a # header, then
                  table, key, rule, file, version and reason, tab apart;
                  a version, as the file is checked as
  --full          print the whole report: tables, checks not made, keys
                  the model doesn't list
  --qpdf <PATH>   the qpdf to decode with (by default PDF_LINT_QPDF,
                  else the one bundled beside this program, in
                  qpdf/bin, else qpdf on the PATH)
  --password <PW> the files' password, user or owner, when it isn't empty
  --jobs <N>      check N files at once (by default, as many as there are
                  CPUs); the report is in the files' order all the same
  --format <F>    text (the default), or json: a line of JSON for each
                  file, with its findings and the whole report's counts
  --version       print the version, the model's commit, corrections,
                  additions and hash, and the qpdf it decodes with
  -h, --help      print this

It exits 0 when every file is clean, 1 when there are findings, and 2
when a file can't be read or the command line is wrong.";

/// How many bytes of a finding's place its line shows.
const SHOWN: usize = 96;

struct Args {
    files: Vec<PathBuf>,
    hold: Option<PathBuf>,
    built: bool,
    exempt: Option<PathBuf>,
    last_update: bool,
    baseline: Option<PathBuf>,
    pdf_version: Option<Version>,
    model: Option<PathBuf>,
    one_table: bool,
    unlisted: UnlistedIn,
    deprecations: bool,
    /// `--extensions` as written: the names.
    extensions: Vec<String>,
    /// The extensions it names, checked against the model's.
    named: BTreeSet<String>,
    allow: Option<PathBuf>,
    full: bool,
    qpdf: Option<PathBuf>,
    password: Option<String>,
    jobs: Option<NonZero<usize>>,
    json: bool,
}

/// What every file is checked with.
struct Setup {
    args: Args,
    hold: Hold,
    exempt: Vec<Exempt>,
    allow: Option<Allowances>,
    baseline: Option<Baseline>,
    /// The model the files are checked against, as the JSON report says:
    /// its hash ([`Grammar::hash`]), 16 hex digits.
    model: String,
}

/// The file `--baseline` names: its bytes, read, and its report at each
/// version a file is checked at, as each is first asked for.
struct Baseline {
    data: Vec<u8>,
    doc: Document,
    /// What it's checked with, as the files are, but its version.
    options: Options,
    reports: Mutex<BTreeMap<Version, Arc<Report>>>,
}

impl Baseline {
    /// Its report at `version`, every object held as merged, with the
    /// paths the walk reached.
    fn report(&self, version: Version) -> Arc<Report> {
        let mut reports = self.reports.lock().unwrap_or_else(PoisonError::into_inner);
        let report = reports.entry(version).or_insert_with(|| {
            let options = Options {
                version: Some(version),
                paths: true,
                ..self.options.clone()
            };
            Arc::new(check(&self.doc, &Hold::All(Made::Merged), &options))
        });
        Arc::clone(report)
    }
}

/// What a file's findings are set against, to leave out what it carried.
enum Against {
    Nothing,
    /// The file it updates, object by object.
    Number(Arc<Report>),
    /// The file it rewrote, finding by finding where it is.
    Path(Arc<Report>),
}

/// What checking one file printed and found.
struct Checked {
    out: String,
    err: String,
    findings: usize,
    unread: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("pdf-lint: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// The command line's arguments; none when it asked only for the version
/// or the usage, which are printed.
fn args() -> Result<Option<Args>> {
    let mut args = Args {
        files: Vec::new(),
        hold: None,
        built: false,
        exempt: None,
        last_update: false,
        baseline: None,
        pdf_version: None,
        model: None,
        one_table: false,
        unlisted: UnlistedIn::Built,
        deprecations: false,
        extensions: Vec::new(),
        named: BTreeSet::new(),
        allow: None,
        full: false,
        qpdf: None,
        password: None,
        jobs: None,
        json: false,
    };
    let mut words = std::env::args_os().skip(1);
    while let Some(word) = words.next() {
        let value = |words: &mut dyn Iterator<Item = std::ffi::OsString>, flag: &str| {
            words
                .next()
                .map(PathBuf::from)
                .with_context(|| format!("{flag} takes a file"))
        };
        match word.to_str() {
            Some("--version" | "-V") => {
                let hash = pdf_lint::grammar().hash();
                let n = pdf_lint::grammar::corrections().len();
                let m = pdf_lint::grammar::additions().len();
                let qpdf = match bundled_unless_named() {
                    Some(program) => Qpdf::new("").program(program),
                    None => Qpdf::new(""),
                };
                let qpdf = match qpdf.ready() {
                    Ok(version) => format!("{version} ({})", qpdf.program_path().display()),
                    Err(why) => format!("no qpdf to decode with: {why}"),
                };
                print(&format!(
                    "pdf-lint {}\nArlington PDF Model {}, {n} cells corrected and {m} rows added (xxh64 {hash:016x})\n{qpdf}\n",
                    env!("CARGO_PKG_VERSION"),
                    pdf_lint::COMMIT
                ));
                return Ok(None);
            }
            Some("--help" | "-h") => {
                print(&format!("{USAGE}\n"));
                return Ok(None);
            }
            Some("--hold") => args.hold = Some(value(&mut words, "--hold")?),
            Some("--exempt") => args.exempt = Some(value(&mut words, "--exempt")?),
            Some("--allow") => args.allow = Some(value(&mut words, "--allow")?),
            Some("--model") => args.model = Some(value(&mut words, "--model")?),
            Some("--baseline") => args.baseline = Some(value(&mut words, "--baseline")?),
            Some("--qpdf") => args.qpdf = Some(value(&mut words, "--qpdf")?),
            Some("--password") => {
                let password = words.next().context("--password takes a password")?;
                let password = password
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("--password: not UTF-8"))?;
                args.password = Some(password);
            }
            Some("--jobs") => {
                let n = words.next().context("--jobs takes a number")?;
                let n = (n.to_str().and_then(|n| n.parse().ok()))
                    .with_context(|| format!("--jobs {}: not a number above 0", n.display()))?;
                args.jobs = Some(n);
            }
            Some("--pdf-version") => {
                let v = words.next().context("--pdf-version takes a version")?;
                let version =
                    (v.to_str().and_then(pdf_lint::grammar::version)).with_context(|| {
                        format!(
                            "--pdf-version {}: not a PDF version, 1.0 to 1.7 or 2.0",
                            v.display()
                        )
                    })?;
                args.pdf_version = Some(version);
            }
            Some("--unlisted") => {
                let w = words
                    .next()
                    .context("--unlisted takes built, all or never")?;
                args.unlisted = (w.to_str().and_then(UnlistedIn::named))
                    .with_context(|| format!("--unlisted {}: built, all or never", w.display()))?;
            }
            Some("--extensions") => {
                let names = words.next().context("--extensions takes names")?;
                let names = (names.to_str())
                    .context("--extensions: not UTF-8")?
                    .split(',')
                    .map(|name| name.trim().to_string())
                    .collect();
                args.extensions = names;
            }
            Some("--format") => {
                let format = words.next().context("--format takes text or json")?;
                args.json = match format.to_str() {
                    Some("text") => false,
                    Some("json") => true,
                    _ => bail!("--format {}: text or json", format.display()),
                };
            }
            Some("--built") => args.built = true,
            Some("--one-table") => args.one_table = true,
            Some("--deprecations") => args.deprecations = true,
            Some("--last-update") => args.last_update = true,
            Some("--full") => args.full = true,
            Some(flag) if flag.starts_with('-') => bail!("{flag}: no such option\n\n{USAGE}"),
            _ => args.files.push(PathBuf::from(word)),
        }
    }
    if args.files.is_empty() {
        bail!("no file\n\n{USAGE}");
    }
    if args.hold.is_some() && args.built {
        bail!("--hold and --built: a hold file says how each object was made");
    }
    if args.last_update && (args.hold.is_some() || args.built) {
        bail!("--last-update holds what the update wrote, with neither --hold nor --built");
    }
    let built = args.hold.is_some() || args.built || args.last_update;
    match args.unlisted {
        _ if args.exempt.is_none() => {}
        UnlistedIn::Never => {
            bail!("--exempt and --unlisted never: no key the model doesn't list is a finding")
        }
        UnlistedIn::Built if !built => bail!(
            "--exempt: a key the model doesn't list is a finding only in a built object, which takes --built, --hold or --last-update (or in any, with --unlisted all)"
        ),
        UnlistedIn::Built | UnlistedIn::All => {}
    }
    if args.last_update && args.baseline.is_some() {
        bail!("--last-update and --baseline: the update's baseline is the revision before it");
    }
    Ok(Some(args))
}

/// `text` on stdout, as much as is read of it: no one reading any more,
/// as when it's piped to `head`, isn't an error.
fn print(text: &str) {
    let _ = io::stdout().lock().write_all(text.as_bytes());
}

fn run() -> Result<ExitCode> {
    let Some(mut args) = args()? else {
        return Ok(ExitCode::SUCCESS);
    };
    // First, before an allowance's table, or anything else, reads it.
    if let Some(dir) = &args.model {
        let model = Grammar::load(dir).with_context(|| format!("--model {}", dir.display()))?;
        use_model(model).with_context(|| format!("--model {}", dir.display()))?;
    }
    if !args.extensions.is_empty() {
        let model = pdf_lint::extension::named();
        // Each is a claim about the file, so there's no naming them all:
        // no file is PDF/A-3, PDF/VT-2 and PDF/X at once.
        if let Some(name) = args.extensions.iter().find(|name| !model.contains(*name)) {
            let model: Vec<&str> = model.iter().map(String::as_str).collect();
            bail!(
                "--extensions: {name:?} isn't an extension the model names, which are {}",
                model.join(", ")
            );
        }
        args.named = args.extensions.iter().cloned().collect();
    }
    let hold = match &args.hold {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            Hold::parse(&text).with_context(|| path.display().to_string())?
        }
        None if args.built => Hold::All(Made::Built),
        None => Hold::All(Made::Merged),
    };
    let exempt = match &args.exempt {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            Exempt::parse(&text).with_context(|| path.display().to_string())?
        }
        None => Vec::new(),
    };
    let allow = args.allow.as_deref().map(Allowances::read).transpose()?;
    let baseline = match &args.baseline {
        Some(path) => {
            let data =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let qpdf = qpdf(path, &args);
            let decoder = Decoder::new(&qpdf);
            let doc = bytes::read_with(&data, &decoder);
            if let Some(why) = decoder.unready() {
                bail!("--baseline {}: {why}", path.display());
            }
            if let Some(why) = locked(&doc, &qpdf, args.password.is_some()) {
                bail!("--baseline {}: {why}", path.display());
            }
            let reports = Mutex::new(BTreeMap::new());
            let options = against(&args, None);
            Some(Baseline {
                data,
                doc,
                options,
                reports,
            })
        }
        None => None,
    };
    let model = format!("{:016x}", pdf_lint::grammar().hash());
    let setup = Setup {
        args,
        hold,
        exempt,
        allow,
        baseline,
        model,
    };
    let (mut findings, mut unread) = (0, false);
    let mut stdout = io::stdout().lock();
    checked_in_order(&setup, |checked| {
        eprint!("{}", checked.err);
        findings += checked.findings;
        unread |= checked.unread;
        // Stopped when no one reads the report any more, as when it's
        // piped to `head`.
        stdout.write_all(checked.out.as_bytes()).is_ok()
    });
    if let Some(allow) = &setup.allow {
        for unused in allow.unused() {
            let line = unused.line;
            eprintln!("note: the allowance on line {line} allowed nothing");
        }
    }
    Ok(match (unread, findings) {
        (true, _) => ExitCode::from(2),
        (false, 0) => ExitCode::SUCCESS,
        (false, _) => ExitCode::from(1),
    })
}

/// Each file checked, by `--jobs` threads, and handed to `report` in the
/// order the files were given, each as soon as those before it are, until
/// `report` says to stop.
fn checked_in_order(setup: &Setup, mut report: impl FnMut(Checked) -> bool) {
    let files = &setup.args.files;
    let jobs = (setup.args.jobs)
        .or_else(|| thread::available_parallelism().ok())
        .map_or(1, NonZero::get)
        .min(files.len());
    let next = AtomicUsize::new(0);
    let (send, receive) = mpsc::channel();
    thread::scope(|scope| {
        for _ in 0..jobs {
            let (send, next) = (send.clone(), &next);
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    // A panic is a bug, but it's one file's: say which, and
                    // check the rest.
                    let checked = panic::catch_unwind(AssertUnwindSafe(|| check_file(path, setup)))
                        .unwrap_or_else(|_| {
                            let why = "pdf-lint panicked checking it";
                            unread(path, why, setup.args.json)
                        });
                    if send.send((i, checked)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(send);
        // Files checked ahead of their turn wait here.
        let mut early = BTreeMap::new();
        let mut due = 0;
        'files: for (i, checked) in receive {
            early.insert(i, checked);
            while let Some(checked) = early.remove(&due) {
                if !report(checked) {
                    // The workers stop as they find no one to send to.
                    break 'files;
                }
                due += 1;
            }
        }
    });
}

/// qpdf for `path`, as the command line has it.
fn qpdf(path: &Path, args: &Args) -> Qpdf {
    let mut qpdf = Qpdf::new(path);
    if let Some(program) = args.qpdf.clone().or_else(bundled_unless_named) {
        qpdf = qpdf.program(program);
    }
    if let Some(password) = &args.password {
        qpdf = qpdf.password(password);
    }
    qpdf
}

/// qpdf as a file is read with: run only when it's ready
/// ([`Qpdf::ready`]), and noting whether the reader asked it for anything.
struct Decoder<'a> {
    qpdf: &'a Qpdf,
    asked: Cell<bool>,
}

impl<'a> Decoder<'a> {
    fn new(qpdf: &'a Qpdf) -> Decoder<'a> {
        Decoder {
            qpdf,
            asked: Cell::new(false),
        }
    }

    /// Why the file isn't checked, when the reader asked qpdf for anything
    /// and it isn't ready: what it asked for is missing.
    fn unready(&self) -> Option<String> {
        if !self.asked.get() {
            return None;
        }
        let why = ready(self.qpdf).as_ref().err()?;
        Some(format!(
            "it needs qpdf to decode what's compressed or encrypted in it, and {why}: install qpdf {} or later, or name one with --qpdf",
            pdf_lint::qpdf::OLDEST
        ))
    }

    /// Whether qpdf is ready, noting that it was asked for something.
    fn ready(&self) -> Result<(), String> {
        self.asked.set(true);
        ready(self.qpdf).as_ref().map(drop).map_err(String::clone)
    }
}

impl Decode for Decoder<'_> {
    fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
        match self.ready() {
            Ok(()) => self.qpdf.decode(ids),
            Err(why) => ids.iter().map(|&id| (id, Err(why.clone()))).collect(),
        }
    }

    fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
        match self.ready() {
            Ok(()) => self.qpdf.plaintext(ids),
            Err(why) => ids.iter().map(|&id| (id, Err(why.clone()))).collect(),
        }
    }

    fn decode_written(&self, stream: Written) -> Result<Vec<u8>, String> {
        self.ready()?;
        self.qpdf.decode_written(stream)
    }
}

/// [`Qpdf::ready`] for the qpdf the command line has, which every file is
/// decoded with: asked once, when a file first needs it.
fn ready(qpdf: &Qpdf) -> &'static Result<String, String> {
    static READY: OnceLock<Result<String, String>> = OnceLock::new();
    READY.get_or_init(|| qpdf.ready())
}

/// Why `doc`, read through `qpdf`, isn't checked, when it's encrypted
/// and won't open with the password given (`password` says whether one
/// was): nothing in it decodes, so what's in its object streams would be
/// missing, and its strings ciphertext.
fn locked(doc: &Document, qpdf: &Qpdf, password: bool) -> Option<&'static str> {
    let encrypted = match &doc.trailer().object.value {
        Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => {
            d.get(b"Encrypt").is_some_and(|e| !e.is_null())
        }
        _ => false,
    };
    // A qpdf that can't say is no reason not to check what reads.
    if !encrypted || !qpdf.locked().unwrap_or(false) {
        return None;
    }
    Some(match password {
        false => "it needs a password: give it with --password",
        true => "the password --password gives doesn't open it",
    })
}

/// The qpdf to run when the command line names none: the one bundled
/// beside this program, unless `PDF_LINT_QPDF` names one, which
/// [`Qpdf::new`] takes, as it takes qpdf on the `PATH` failing both.
fn bundled_unless_named() -> Option<PathBuf> {
    bundled().filter(|_| std::env::var_os("PDF_LINT_QPDF").is_none())
}

/// The qpdf a release archive bundles beside this program, in `qpdf/bin`,
/// when there's one.
fn bundled() -> Option<PathBuf> {
    static BUNDLED: OnceLock<Option<PathBuf>> = OnceLock::new();
    let find = || {
        let exe = std::env::current_exe().ok()?;
        let qpdf = (exe.parent()?.join("qpdf").join("bin"))
            .join(format!("qpdf{}", std::env::consts::EXE_SUFFIX));
        qpdf.is_file().then_some(qpdf)
    };
    BUNDLED.get_or_init(find).clone()
}

/// One file read and checked, with what's to print about it.
fn check_file(path: &Path, setup: &Setup) -> Checked {
    let Setup {
        args,
        hold,
        exempt,
        allow,
        baseline,
        model,
    } = setup;
    let name = path.display().to_string();
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) => return unread(path, &e.to_string(), args.json),
    };
    let qpdf = qpdf(path, args);
    let decoder = Decoder::new(&qpdf);
    let doc = bytes::read_with(&data, &decoder);
    if let Some(why) = decoder.unready() {
        return unread(path, &why, args.json);
    }
    if let Some(why) = locked(&doc, &qpdf, args.password.is_some()) {
        return unread(path, why, args.json);
    }
    let version = file_version(&doc);
    // What it's checked as: --pdf-version's, else its own. What it's set
    // against is checked as that too, as a version moves findings.
    let checked_as = args.pdf_version.unwrap_or(version);
    // The file's own check, and what it's set against, which is held as
    // merged, so that only the file's findings take its exempt keys.
    let options = Options {
        exempt: exempt.clone(),
        ..against(args, Some(checked_as))
    };
    let (report, against) = match (args.last_update, &doc.update) {
        (false, _) => {
            let report = check(&doc, hold, &options);
            // An incremental update of the baseline begins with its bytes.
            let against = match baseline {
                Some(b) if data.starts_with(&b.data) => Against::Number(b.report(checked_as)),
                Some(b) => Against::Path(b.report(checked_as)),
                None => Against::Nothing,
            };
            (report, against)
        }
        (true, None) => {
            let why = "--last-update: it has no incremental update (one cross-reference section, or a newest one written before the one it follows)";
            return unread(path, why, args.json);
        }
        (true, Some(update)) => {
            let previous = bytes::read_from(&data, &decoder, update.previous);
            if let Some(why) = decoder.unready() {
                return unread(path, &why, args.json);
            }
            let made = |id| match previous.objects.contains_key(id) {
                true => Made::Merged,
                false => Made::Built,
            };
            let hold = Hold::Only(update.objects.iter().map(|id| (*id, made(id))).collect());
            let at_version = against(args, Some(checked_as));
            let before = check(&previous, &Hold::All(Made::Merged), &at_version);
            let report = check(&doc, &hold, &options);
            (report, Against::Number(Arc::new(before)))
        }
    };
    let kept = match &against {
        Against::Nothing => report.findings.iter().collect(),
        Against::Number(before) => report.since(before),
        Against::Path(baseline) => report.by_path(baseline),
    };
    let carried = report.findings.len() - kept.len();
    let shown: Vec<_> = (kept.iter().copied())
        .filter(|f| (allow.as_ref()).is_none_or(|a| a.allows(f, checked_as, Some(&name)).is_none()))
        .collect();
    let allowed = kept.len() - shown.len();
    if args.json {
        return Checked {
            out: json(
                &name,
                version,
                checked_as,
                args.one_table,
                args.unlisted,
                args.deprecations,
                model,
                &report,
                &shown,
                allowed,
                carried,
            ),
            err: String::new(),
            findings: shown.len(),
            unread: false,
        };
    }
    let objects = report.objects();
    let left_out = |n, what| match n {
        0 => String::new(),
        n => format!(", {n} {what}"),
    };
    let allowed = left_out(carried, "carried") + &left_out(allowed, "allowed");
    let checked_as = match checked_as == version {
        true => String::new(),
        false => format!(", checked as {checked_as}"),
    };
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{name}: PDF {version}{checked_as}, {objects} objects checked, {} findings{allowed}",
        shown.len()
    );
    if args.full {
        for line in report.to_string().lines().filter(|l| !l.starts_with("- ")) {
            let _ = writeln!(out, "  {line}");
        }
    }
    for finding in &shown {
        let _ = writeln!(out, "- {finding}");
        match finding.at {
            Some(at @ Span { stream: None, .. }) => {
                let _ = writeln!(out, "    | {}", snippet(&data, at));
            }
            Some(
                at @ Span {
                    stream: Some((n, g)),
                    ..
                },
            ) => {
                if let Some(decoded) = doc.decoded.get(&(n, g)) {
                    let _ = writeln!(
                        out,
                        "    | {}   (of {n} {g} R's data, decoded)",
                        snippet(decoded, at)
                    );
                }
            }
            None => {}
        }
    }
    Checked {
        out,
        err: String::new(),
        findings: shown.len(),
        unread: false,
    }
}

/// What a file, and what it's set against, are checked with, as the
/// command line says, at `version`: how many tables an object is checked
/// as, which objects a key the model doesn't list is a finding in,
/// whether deprecations are findings, and the extensions it's checked
/// under besides those it declares.
fn against(args: &Args, version: Option<Version>) -> Options {
    Options {
        version,
        one_table: args.one_table,
        unlisted: args.unlisted,
        deprecations: args.deprecations,
        extensions: args.named.clone(),
        ..Options::default()
    }
}

/// A file that couldn't be read or checked, and why: said on standard
/// error, and in the JSON report too, with the version of pdf-lint and
/// the model's hash, as every line has them.
fn unread(path: &Path, why: &str, json: bool) -> Checked {
    let name = path.display().to_string();
    let out = match json {
        true => format!(
            "{{\"pdf_lint\":\"{}\",\"model\":\"{:016x}\",\"file\":{},\"error\":{}}}\n",
            env!("CARGO_PKG_VERSION"),
            pdf_lint::grammar().hash(),
            string(&name),
            string(why)
        ),
        false => String::new(),
    };
    Checked {
        out,
        err: format!("{name}: {why}\n"),
        findings: 0,
        unread: true,
    }
}

/// One file's report as a line of JSON: the version of pdf-lint that
/// wrote it, the file's version, the version it was
/// checked as, whether as one table an object (`--one-table`), which held
/// objects a key the model doesn't list is a finding in (`--unlisted`),
/// whether deprecations are findings (`--deprecations`), the extensions
/// it was checked under, and the model's hash, `model`; its findings, each with where its bytes are; how
/// many were left out as carried and as allowed; and the counts `--full`
/// prints.
#[allow(clippy::too_many_arguments)]
fn json(
    name: &str,
    version: Version,
    checked_as: Version,
    one_table: bool,
    unlisted: UnlistedIn,
    deprecations: bool,
    model: &str,
    report: &Report,
    shown: &[&Finding],
    allowed: usize,
    carried: usize,
) -> String {
    let id = |(n, g): (u32, u16)| format!("[{n},{g}]");
    let mut out = String::new();
    let objects = report.objects();
    let _ = write!(
        out,
        "{{\"pdf_lint\":\"{}\",\"file\":{},\"version\":\"{version}\",\"checked_as\":\"{checked_as}\",\"one_table\":{one_table},\"unlisted_in\":\"{}\",\"deprecations\":{deprecations},\"extensions\":[{}],\"model\":\"{model}\",\"objects\":{objects},\"keys\":{},\"findings\":[",
        env!("CARGO_PKG_VERSION"),
        string(name),
        unlisted.name(),
        (report.extensions.iter())
            .map(|name| string(name))
            .collect::<Vec<_>>()
            .join(","),
        report.keys
    );
    for (i, f) in shown.iter().enumerate() {
        let at = match f.at {
            Some(at) => format!(
                "{{\"stream\":{},\"start\":{},\"end\":{}}}",
                at.stream.map_or("null".to_string(), id),
                at.start,
                at.end
            ),
            None => "null".to_string(),
        };
        let _ = write!(
            out,
            "{}{{\"rule\":\"{}\",\"table\":{},\"key\":{},\"path\":{},\"object\":{},\"made\":\"{}\",\"value\":{},\"at\":{at}}}",
            if i == 0 { "" } else { "," },
            f.rule,
            string(&f.table),
            string(&f.key),
            string(&f.path),
            f.object.map_or("null".to_string(), id),
            f.made.name(),
            string(&f.value),
        );
    }
    let _ = write!(
        out,
        "],\"carried\":{carried},\"allowed\":{allowed},\"tables\":{{"
    );
    for (i, (table, n)) in report.checked.iter().enumerate() {
        let _ = write!(
            out,
            "{}{}:{n}",
            if i == 0 { "" } else { "," },
            string(table)
        );
    }
    out.push_str("},\"not_checked\":[");
    for (i, (u, n)) in report.unchecked.iter().enumerate() {
        let _ = write!(
            out,
            "{}{{\"column\":{},\"why\":{},\"table\":{},\"key\":{},\"count\":{n}}}",
            if i == 0 { "" } else { "," },
            string(u.column),
            string(&u.why),
            string(&u.table),
            string(&u.key),
        );
    }
    out.push_str("],\"unlisted\":[");
    for (i, (u, n)) in report.unlisted.iter().enumerate() {
        let class = match &u.class {
            Class::Second => "\"second\"".to_string(),
            Class::Third => "\"third\"".to_string(),
            Class::Other => "\"other\"".to_string(),
            Class::Extension(name) => format!("\"extension\",\"extension\":{}", string(name)),
        };
        let _ = write!(
            out,
            "{}{{\"table\":{},\"key\":{},\"class\":{class},\"count\":{n}}}",
            if i == 0 { "" } else { "," },
            string(&u.table),
            string(&u.key),
        );
    }
    out.push_str("],\"deprecated\":[");
    for (i, ((table, key), n)) in report.deprecated.iter().enumerate() {
        let _ = write!(
            out,
            "{}{{\"table\":{},\"key\":{},\"count\":{n}}}",
            if i == 0 { "" } else { "," },
            string(table),
            string(key),
        );
    }
    out.push_str("],\"ignored\":[");
    for (i, ((table, key), n)) in report.ignored.iter().enumerate() {
        let _ = write!(
            out,
            "{}{{\"table\":{},\"key\":{},\"count\":{n}}}",
            if i == 0 { "" } else { "," },
            string(table),
            string(key),
        );
    }
    let _ = writeln!(
        out,
        "],\"no_table\":{},\"ties\":{},\"truncated\":{},\"two_contexts\":{},\"two_tables\":{}}}",
        report.no_table, report.ties, report.truncated, report.two_contexts, report.two_tables
    );
    out
}

/// `s` as a JSON string.
fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The bytes at `at`, the first [`SHOWN`] of them, printable ASCII as it is
/// and any other byte escaped.
fn snippet(data: &[u8], at: Span) -> String {
    let end = at.end.min(data.len());
    let start = at.start.min(end);
    let mut out = String::new();
    for &b in &data[start..end.min(start + SHOWN)] {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02X}")),
        }
    }
    if end - start > SHOWN {
        out.push('…');
    }
    out
}
