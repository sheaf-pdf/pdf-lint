//! The command line: each file read from its bytes and checked against the
//! Arlington PDF Model, each finding printed with the bytes it's at.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, Read as _, Write as _};
use std::num::NonZero;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;

use anyhow::{Context, Result, bail};
use pdf_lint::{
    Allowances, Class, Document, Encryption, Exempt, Finding, Grammar, Hold, Made, Options, Report,
    Span, UnlistedIn, Version, bytes, check, file_version, text, use_model,
};
use serde::Serialize;
use serde_json::ser::{CharEscape, CompactFormatter, Formatter};

const USAGE: &str = "\
usage: pdf-lint [OPTIONS] FILE...

Checks each FILE against the Arlington PDF Model, reading it from its
bytes, and prints each finding with the bytes it's at; a FILE of - is
standard input. It decodes object streams and cross-reference streams,
and decrypts what's encrypted, itself.

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
  --password <PW> the files' password, user or owner, when it isn't empty:
                  tried as UTF-8, then in PDFDocEncoding, WinAnsiEncoding
                  and MacRomanEncoding
  --jobs <N>      check N files at once (by default, as many as there are
                  CPUs); the report is in the files' order all the same
  --format <F>    text (the default), or json: a line of JSON for each
                  file, with its findings and the whole report's counts
  --version       print the version, and the model's commit, corrections,
                  additions and hash
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
    password: Option<String>,
    jobs: Option<NonZero<usize>>,
    json: bool,
}

/// What every file is checked with.
struct Setup {
    args: Args,
    /// `--password`'s, as UTF-8 and in each single-byte encoding that has
    /// it ([`text::passwords`]); none without it.
    passwords: Vec<Vec<u8>>,
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
                print(&format!(
                    "pdf-lint {}\nArlington PDF Model {}, {n} cells corrected and {m} rows added (xxh64 {hash:016x})\n",
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
            Some(flag) if flag.starts_with('-') && flag != "-" => {
                bail!("{flag}: no such option\n\n{USAGE}")
            }
            _ => args.files.push(PathBuf::from(word)),
        }
    }
    if args.files.is_empty() {
        bail!("no file\n\n{USAGE}");
    }
    let stdins = (args.files.iter()).filter(|path| *path == Path::new("-"));
    if stdins.count() > 1 {
        bail!("- more than once: standard input is read once");
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
    let passwords = (args.password.as_deref()).map_or_else(Vec::new, text::passwords);
    let baseline = match &args.baseline {
        Some(path) => {
            let data =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let doc = bytes::read_opened(&data, &slices(&passwords));
            if let Some(why) = locked(&doc, args.password.is_some()) {
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
        passwords,
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

/// `passwords` as the reader takes them.
fn slices(passwords: &[Vec<u8>]) -> Vec<&[u8]> {
    passwords.iter().map(Vec::as_slice).collect()
}

/// Why `doc` isn't checked, when it's encrypted and no password it was
/// given opens it (`password` says whether one was), though it encrypts
/// its strings or streams: what's in its object streams would be missing,
/// and its strings ciphertext.
fn locked(doc: &Document, password: bool) -> Option<&'static str> {
    (doc.encryption == Encryption::Locked).then_some(match password {
        false => "it needs a password: give it with --password",
        true => "the password --password gives doesn't open it",
    })
}

/// One file read and checked, with what's to print about it.
fn check_file(path: &Path, setup: &Setup) -> Checked {
    let Setup {
        args,
        passwords,
        hold,
        exempt,
        allow,
        baseline,
        model,
    } = setup;
    let name = path.display().to_string();
    let stdin = path == Path::new("-");
    let data = match stdin {
        true => {
            let mut data = Vec::new();
            io::stdin().lock().read_to_end(&mut data).map(|_| data)
        }
        false => std::fs::read(path),
    };
    let data = match data {
        Ok(data) => data,
        Err(e) => return unread(path, &e.to_string(), args.json),
    };
    let doc = bytes::read_opened(&data, &slices(passwords));
    if let Some(why) = locked(&doc, args.password.is_some()) {
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
            let previous = bytes::read_from_opened(&data, &slices(passwords), update.previous);
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
        let line = json(
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
        );
        return match line {
            Ok(out) => Checked {
                out,
                err: String::new(),
                findings: shown.len(),
                unread: false,
            },
            Err(e) => unread(path, &format!("{e:#}"), true),
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
    let mut err = format!("{name}: {why}\n");
    let out = match json {
        true => {
            let model = format!("{:016x}", pdf_lint::grammar().hash());
            let unread = Unread {
                pdf_lint: env!("CARGO_PKG_VERSION"),
                model: &model,
                file: &name,
                error: why,
            };
            // It can't fail, as `line` says; if it did, that's said too.
            line(&unread).unwrap_or_else(|e| {
                let _ = writeln!(err, "{name}: {e:#}");
                String::new()
            })
        }
        false => String::new(),
    };
    Checked {
        out,
        err,
        findings: 0,
        unread: true,
    }
}

/// One file's report as a line of JSON, a [`Line`].
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
) -> Result<String> {
    line(&Line {
        pdf_lint: env!("CARGO_PKG_VERSION"),
        file: name,
        version: version.to_string(),
        checked_as: checked_as.to_string(),
        one_table,
        unlisted_in: unlisted.name(),
        deprecations,
        extensions: &report.extensions,
        model,
        objects: report.objects(),
        keys: report.keys,
        findings: (shown.iter())
            .map(|f| Found {
                rule: f.rule.name(),
                table: &f.table,
                key: &f.key,
                path: &f.path,
                object: f.object,
                made: f.made.name(),
                value: &f.value,
                at: f.at.map(|at| At {
                    stream: at.stream,
                    start: at.start,
                    end: at.end,
                }),
            })
            .collect(),
        carried,
        allowed,
        tables: &report.checked,
        not_checked: (report.unchecked.iter())
            .map(|(u, &count)| NotChecked {
                column: u.column,
                why: &u.why,
                table: &u.table,
                key: &u.key,
                count,
            })
            .collect(),
        unlisted: (report.unlisted.iter())
            .map(|(u, &count)| {
                let (class, extension) = match &u.class {
                    Class::Second => ("second", None),
                    Class::Third => ("third", None),
                    Class::Other => ("other", None),
                    Class::Extension(name) => ("extension", Some(name.as_str())),
                };
                Unlisted {
                    table: &u.table,
                    key: &u.key,
                    class,
                    extension,
                    count,
                }
            })
            .collect(),
        deprecated: (report.deprecated.iter())
            .map(|((table, key), &count)| Count { table, key, count })
            .collect(),
        ignored: (report.ignored.iter())
            .map(|((table, key), &count)| Count { table, key, count })
            .collect(),
        no_table: report.no_table,
        ties: report.ties,
        truncated: report.truncated,
        two_contexts: report.two_contexts,
        two_tables: report.two_tables,
    })
}

/// One file's report, as a line of JSON: the version of pdf-lint that
/// wrote it, the file's version, the version it was checked as, whether as
/// one table an object (`--one-table`), which held objects a key the model
/// doesn't list is a finding in (`--unlisted`), whether deprecations are
/// findings (`--deprecations`), the extensions it was checked under, and
/// the model's hash, `model`; its findings, each with where its bytes are;
/// how many were left out as carried and as allowed; and the counts
/// `--full` prints. The fields are in the order the line has them.
#[derive(Serialize)]
struct Line<'a> {
    pdf_lint: &'a str,
    /// The file as the text report names it, a byte that isn't UTF-8 as
    /// U+FFFD; not a `Path`, which serde writes only when it's UTF-8.
    file: &'a str,
    version: String,
    checked_as: String,
    one_table: bool,
    unlisted_in: &'a str,
    deprecations: bool,
    extensions: &'a BTreeSet<String>,
    model: &'a str,
    objects: usize,
    keys: usize,
    findings: Vec<Found<'a>>,
    carried: usize,
    allowed: usize,
    /// How many objects were checked as each table, by its name.
    tables: &'a BTreeMap<String, usize>,
    not_checked: Vec<NotChecked<'a>>,
    unlisted: Vec<Unlisted<'a>>,
    deprecated: Vec<Count<'a>>,
    ignored: Vec<Count<'a>>,
    no_table: usize,
    ties: usize,
    truncated: usize,
    two_contexts: usize,
    two_tables: usize,
}

/// A finding, with where its bytes are; its object is `[n,g]`, or null
/// for the trailer.
#[derive(Serialize)]
struct Found<'a> {
    rule: &'a str,
    table: &'a str,
    key: &'a str,
    path: &'a str,
    object: Option<(u32, u16)>,
    made: &'a str,
    value: &'a str,
    at: Option<At>,
}

/// Where a finding's bytes are: in the file, or in the decoded data of the
/// object stream `stream`, `[n,g]`.
#[derive(Serialize)]
struct At {
    stream: Option<(u32, u16)>,
    start: usize,
    end: usize,
}

/// A key the model doesn't list, and how many times: `extension` names the
/// extension when that's its class, and isn't written otherwise.
#[derive(Serialize)]
struct Unlisted<'a> {
    table: &'a str,
    key: &'a str,
    class: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    extension: Option<&'a str>,
    count: usize,
}

/// A key, by table, and how many times: one deprecated, or ignored.
#[derive(Serialize)]
struct Count<'a> {
    table: &'a str,
    key: &'a str,
    count: usize,
}

/// A check not made, why, where, and how many times.
#[derive(Serialize)]
struct NotChecked<'a> {
    column: &'a str,
    why: &'a str,
    table: &'a str,
    key: &'a str,
    count: usize,
}

/// The line of a file that couldn't be read or checked.
#[derive(Serialize)]
struct Unread<'a> {
    pdf_lint: &'a str,
    model: &'a str,
    file: &'a str,
    error: &'a str,
}

/// `value` as a line of JSON, written by [`Reports`]. Into a `Vec`, and
/// with strings alone for keys, it can't fail.
fn line(value: &impl Serialize) -> Result<String> {
    let mut out = Vec::new();
    let mut json = serde_json::Serializer::with_formatter(&mut out, Reports);
    value.serialize(&mut json)?;
    out.push(b'\n');
    Ok(String::from_utf8(out)?)
}

/// How the reports are written: serde_json's compact JSON, but for
/// backspace and form feed, which serde_json writes `\b` and `\f`. Here
/// they're `\u0008` and `\u000c`, as every other character below U+0020
/// is but tab, line feed and carriage return, in lowercase (RFC 8259, 7,
/// allows either), so that a report is the bytes it's always been.
struct Reports;

impl Formatter for Reports {
    fn write_char_escape<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        escape: CharEscape,
    ) -> io::Result<()> {
        match escape {
            CharEscape::Backspace => writer.write_all(b"\\u0008"),
            CharEscape::FormFeed => writer.write_all(b"\\u000c"),
            escape => CompactFormatter.write_char_escape(writer, escape),
        }
    }
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
