//! Findings known and allowed: a TSV file, a `#` header saying what it
//! holds, then a row for each finding allowed: its table, key, rule, the
//! file it's in (a path, or `*` for any), the PDF version from which on
//! it's allowed (`2.0`, or `*` for any), and why. A finding a writer is
//! answerable for that no row allows fails, and a row that allows nothing
//! can be told ([`Allowances::unused`]): each is known, and none is
//! skipped in silence.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail, ensure};

use crate::check::{Finding, Rule};
use crate::grammar::grammar;
use crate::version::Version;

/// The findings a file allows, each as many times as it comes.
#[derive(Debug)]
pub struct Allowances {
    list: Vec<Allowance>,
    /// Whether each of `list` has allowed a finding.
    used: Vec<AtomicBool>,
}

/// A row of an allowances file: the findings of its table, key and rule,
/// in one file or any, of a version and later or any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowance {
    /// The line of its file it's on.
    pub line: usize,
    /// A table of the model's, or `-`, as an unreached object's finding
    /// and a reader's have it.
    pub table: String,
    pub key: String,
    pub rule: Rule,
    /// The file whose findings alone it allows, as its caller names it;
    /// none for any file's.
    pub file: Option<String>,
    /// The version of the files whose findings alone it allows, and the
    /// later ones', as the version a file is checked at is told: its own
    /// ([`file_version`](crate::file_version)), or the one
    /// [`Options::version`](crate::Options::version) names; none for any
    /// version's.
    pub from: Option<Version>,
    pub reason: String,
}

impl Allowances {
    /// The allowances in the file `path`; an error names the file and, as
    /// [`Allowances::parse`] does, the line.
    pub fn read(path: &Path) -> Result<Allowances> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Allowances::parse(&text).with_context(|| path.display().to_string())
    }

    /// The allowances in `text`. An error names the line that doesn't
    /// read: a row without its six fields, a table the model hasn't, a
    /// rule there isn't, no file, a version that isn't one, no reason, or
    /// a second row for a table, key, rule, file and version.
    pub fn parse(text: &str) -> Result<Allowances> {
        let header = (text.lines())
            .take_while(|line| line.starts_with('#'))
            .count();
        ensure!(header > 0, "line 1: no # header");
        let mut list: Vec<Allowance> = Vec::new();
        for (i, text) in text.lines().enumerate().skip(header) {
            let line = i + 1;
            ensure!(
                !text.starts_with('#'),
                "line {line}: a # line among the rows"
            );
            let allowance = Allowance::parse(line, text).with_context(|| format!("line {line}"))?;
            if let Some(first) = list.iter().find(|a| a.covers(&allowance)) {
                bail!(
                    "line {line}: {}/{} {} in {} from {} is on line {} already",
                    allowance.table,
                    allowance.key,
                    allowance.rule,
                    allowance.file.as_deref().unwrap_or("*"),
                    allowance.from.map_or("*".to_string(), |v| v.to_string()),
                    first.line
                );
            }
            list.push(allowance);
        }
        let used = list.iter().map(|_| AtomicBool::new(false)).collect();
        Ok(Allowances { list, used })
    }

    /// The allowance for `finding`, in a file checked at `version` named
    /// `file` (none for a file of no name, which only rows for any file
    /// allow), if there's one; it's then used.
    pub fn allows(
        &self,
        finding: &Finding,
        version: Version,
        file: Option<&str>,
    ) -> Option<&Allowance> {
        let at = (self.list.iter()).position(|a| {
            a.table == finding.table
                && a.key == finding.key
                && a.rule == finding.rule
                && a.file
                    .as_ref()
                    .is_none_or(|only| Some(only.as_str()) == file)
                && a.from.is_none_or(|from| version >= from)
        })?;
        self.used[at].store(true, Ordering::Relaxed);
        Some(&self.list[at])
    }

    /// Every allowance, in the file's order.
    pub fn rows(&self) -> impl Iterator<Item = &Allowance> {
        self.list.iter()
    }

    /// The allowances that haven't allowed a finding.
    pub fn unused(&self) -> Vec<&Allowance> {
        (self.list.iter().zip(&self.used))
            .filter(|(_, used)| !used.load(Ordering::Relaxed))
            .map(|(a, _)| a)
            .collect()
    }
}

impl Allowance {
    fn parse(line: usize, text: &str) -> Result<Allowance> {
        let fields: Vec<&str> = text.split('\t').collect();
        let [table, key, rule, file, from, reason] = fields[..] else {
            bail!(
                "{} fields, not 6: table, key, rule, file, version, reason",
                fields.len()
            );
        };
        ensure!(
            table == "-" || grammar().table(table).is_some(),
            "{table:?} isn't a table of the model's"
        );
        ensure!(!key.is_empty(), "no key");
        let rule = Rule::named(rule).with_context(|| format!("{rule:?} isn't a rule"))?;
        ensure!(!file.is_empty(), "no file (`*` for any)");
        let from = match from {
            "*" => None,
            from => Some(
                Version::parse(from.as_bytes())
                    .with_context(|| format!("{from:?} isn't a version (`*` for any)"))?,
            ),
        };
        ensure!(!reason.trim().is_empty(), "no reason");
        Ok(Allowance {
            line,
            table: table.to_string(),
            key: key.to_string(),
            rule,
            file: (file != "*").then(|| file.to_string()),
            from,
            reason: reason.to_string(),
        })
    }

    /// Whether it allows what `other` does.
    fn covers(&self, other: &Allowance) -> bool {
        let own = (&self.table, &self.key, self.rule, &self.file, self.from);
        own == (
            &other.table,
            &other.key,
            other.rule,
            &other.file,
            other.from,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Made;

    fn finding(table: &str, key: &str, rule: Rule) -> Finding {
        Finding {
            object: Some((6, 0)),
            made: Made::Built,
            path: "Root/AcroForm/Fields[0]".to_string(),
            table: table.to_string(),
            key: key.to_string(),
            rule,
            value: "absent".to_string(),
            at: None,
        }
    }

    #[test]
    fn allowances_each_say_what_they_allow_and_why() {
        let header = "# Held for the test.\n";
        let rows = "FieldTx\tDA\tmissing\t*\t*\tthe form's /DA, which the model doesn't read\n\
                    -\t-\tunreached\t*\t*\ta reason\n\
                    FieldTx\tFT\tmissing\tforms/a.pdf\t*\tone file's\n\
                    FieldTx\tV\ttype\t*\t2.0\t2.0 files' and later\n";
        let held = Allowances::parse(&format!("{header}{rows}")).unwrap();
        let v1_7 = Version::new(1, 7);
        let missing = finding("FieldTx", "DA", Rule::Missing);
        let allowed = held.allows(&missing, v1_7, None).map(|a| a.line);
        assert_eq!(allowed, Some(2));
        let typed = Finding {
            rule: Rule::Type,
            ..missing.clone()
        };
        assert_eq!(held.allows(&typed, v1_7, None), None);
        let ft = Finding {
            key: "FT".to_string(),
            ..missing.clone()
        };
        assert_eq!(held.allows(&ft, v1_7, None), None, "in a file of no name");
        assert_eq!(
            held.allows(&ft, v1_7, Some("forms/b.pdf")),
            None,
            "in another"
        );
        let allowed = held.allows(&ft, v1_7, Some("forms/a.pdf")).map(|a| a.line);
        assert_eq!(allowed, Some(4));
        let v = Finding {
            key: "V".to_string(),
            ..typed
        };
        assert_eq!(held.allows(&v, v1_7, None), None, "in a 1.7 file");
        for version in [Version::new(2, 0), Version::new(2, 1)] {
            let allowed = held.allows(&v, version, None).map(|a| a.line);
            assert_eq!(allowed, Some(5), "in a {version} file");
        }
        let unused: Vec<_> = held.unused().iter().map(|a| a.line).collect();
        assert_eq!(unused, [3]);
        assert_eq!(held.rows().count(), 4);

        for (rows, why) in [
            ("FieldTx\tDA\tmissing\t*\t*\t\n", "line 2: no reason"),
            ("FieldTx\tDA\tmissing\t\t*\ta\n", "line 2: no file"),
            (
                "FieldTx\tDA\tmissing\t*\t2\ta\n",
                "line 2: \"2\" isn't a version",
            ),
            ("FieldTx\tDA\tmissing\t*\ta\n", "line 2: 5 fields"),
            (
                "Nope\tDA\tmissing\t*\t*\ta reason\n",
                "line 2: \"Nope\" isn't a table",
            ),
            (
                "FieldTx\tDA\tbogus\t*\t*\ta reason\n",
                "line 2: \"bogus\" isn't a rule",
            ),
            (
                "FieldTx\tDA\tmissing\t*\t*\ta\nFieldTx\tDA\tmissing\t*\t*\tb\n",
                "line 3: FieldTx/DA missing in * from * is on line 2 already",
            ),
            (
                "FieldTx\tDA\tmissing\t*\t*\ta\n# late\n",
                "line 3: a # line among the rows",
            ),
        ] {
            let Err(e) = Allowances::parse(&format!("{header}{rows}")) else {
                panic!("{rows:?} read");
            };
            assert!(format!("{e:#}").contains(why), "{rows:?}: {e:#}");
        }
        let Err(e) = Allowances::parse(rows) else {
            panic!("no header, read");
        };
        assert!(format!("{e:#}").contains("line 1: no # header"), "{e:#}");
    }
}
