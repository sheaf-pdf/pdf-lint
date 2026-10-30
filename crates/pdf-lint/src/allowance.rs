//! Findings known and allowed: a TSV file, a `#` header saying what it
//! holds, then a row for each finding allowed: its table, key, rule, the
//! file it's in (a path, or `*` for any), the PDF version from which on
//! it's allowed (`2.0`, or `*` for any), and why. A finding a writer is
//! answerable for that no row allows fails, and a row that allows nothing
//! can be told ([`Allowances::unused`]): each is known, and none is
//! skipped in silence.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Why an allowances file's text doesn't read ([`Allowances::parse`]):
/// the rule a line breaks, after the line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AllowanceError {
    /// The first line isn't a `#` line, so the file has no header.
    #[error("line 1: no # header")]
    Header,
    /// A `#` line after the first row.
    #[error("line {line}: a # line among the rows")]
    Comment {
        /// Its line.
        line: usize,
    },
    /// A row without its six fields.
    #[error("line {line}: {count} fields, not 6: table, key, rule, file, version, reason")]
    Fields {
        /// Its line.
        line: usize,
        /// How many it has.
        count: usize,
    },
    /// A table the model hasn't, and not `-`.
    #[error("line {line}: {table:?} isn't a table of the model's")]
    Table {
        /// Its line.
        line: usize,
        /// The table, as written.
        table: String,
    },
    /// No key.
    #[error("line {line}: no key")]
    Key {
        /// Its line.
        line: usize,
    },
    /// A rule there isn't ([`Rule::named`]).
    #[error("line {line}: {rule:?} isn't a rule")]
    Rule {
        /// Its line.
        line: usize,
        /// The rule, as written.
        rule: String,
    },
    /// No file, not even `*`.
    #[error("line {line}: no file (`*` for any)")]
    File {
        /// Its line.
        line: usize,
    },
    /// A version that isn't one, and not `*`.
    #[error("line {line}: {version:?} isn't a version (`*` for any)")]
    Version {
        /// Its line.
        line: usize,
        /// The version, as written.
        version: String,
    },
    /// No reason.
    #[error("line {line}: no reason")]
    Reason {
        /// Its line.
        line: usize,
    },
    /// A second row for a table, key, rule, file and version.
    #[error(
        "line {line}: {table}/{key} {rule} in {} from {} is on line {first} already",
        or_any(.file),
        or_any(.from)
    )]
    Again {
        /// Its line.
        line: usize,
        /// The table.
        table: String,
        /// The key.
        key: String,
        /// The rule.
        rule: Rule,
        /// The file, or none for any.
        file: Option<String>,
        /// The version, or none for any.
        from: Option<Version>,
        /// The line of the first row for them.
        first: usize,
    },
}

/// `value`, or `*` for any, for [`AllowanceError::Again`]'s text.
fn or_any<T: fmt::Display>(value: &Option<T>) -> String {
    value.as_ref().map_or("*".to_string(), T::to_string)
}

/// Why an allowances file doesn't read ([`Allowances::read`]): it can't be
/// read, or a line of it doesn't, said after its path. The std error under
/// one is a field, said after `: `, not a source.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReadError {
    /// The file can't be read.
    #[error("reading {path}: {error}")]
    Read {
        /// Its path.
        path: PathBuf,
        /// Why, as std says.
        error: io::Error,
    },
    /// A line of it doesn't read.
    #[error("{path}: {error}")]
    Parse {
        /// Its path.
        path: PathBuf,
        /// The line, and why.
        error: AllowanceError,
    },
}

impl Allowances {
    /// The allowances in the file `path`; a [`ReadError`] names the file
    /// and, as [`Allowances::parse`] does, the line.
    pub fn read(path: &Path) -> Result<Allowances, ReadError> {
        let text = std::fs::read_to_string(path).map_err(|error| ReadError::Read {
            path: path.to_path_buf(),
            error,
        })?;
        Allowances::parse(&text).map_err(|error| ReadError::Parse {
            path: path.to_path_buf(),
            error,
        })
    }

    /// The allowances in `text`. An [`AllowanceError`] names the line that
    /// doesn't read: a row without its six fields, a table the model
    /// hasn't, a rule there isn't, no file, a version that isn't one, no
    /// reason, or a second row for a table, key, rule, file and version.
    pub fn parse(text: &str) -> Result<Allowances, AllowanceError> {
        let header = (text.lines())
            .take_while(|line| line.starts_with('#'))
            .count();
        if header == 0 {
            return Err(AllowanceError::Header);
        }
        let mut list: Vec<Allowance> = Vec::new();
        for (i, text) in text.lines().enumerate().skip(header) {
            let line = i + 1;
            if text.starts_with('#') {
                return Err(AllowanceError::Comment { line });
            }
            let allowance = Allowance::parse(line, text)?;
            if let Some(first) = list.iter().find(|a| a.covers(&allowance)) {
                return Err(AllowanceError::Again {
                    line,
                    table: allowance.table,
                    key: allowance.key,
                    rule: allowance.rule,
                    file: allowance.file,
                    from: allowance.from,
                    first: first.line,
                });
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
    fn parse(line: usize, text: &str) -> Result<Allowance, AllowanceError> {
        let fields: Vec<&str> = text.split('\t').collect();
        let [table, key, rule, file, from, reason] = fields[..] else {
            return Err(AllowanceError::Fields {
                line,
                count: fields.len(),
            });
        };
        if table != "-" && grammar().table(table).is_none() {
            return Err(AllowanceError::Table {
                line,
                table: table.to_string(),
            });
        }
        if key.is_empty() {
            return Err(AllowanceError::Key { line });
        }
        let rule = Rule::named(rule).ok_or_else(|| AllowanceError::Rule {
            line,
            rule: rule.to_string(),
        })?;
        if file.is_empty() {
            return Err(AllowanceError::File { line });
        }
        let version = |from: &str| {
            Version::parse(from.as_bytes()).ok_or_else(|| AllowanceError::Version {
                line,
                version: from.to_string(),
            })
        };
        let from = match from {
            "*" => None,
            from => Some(version(from)?),
        };
        if reason.trim().is_empty() {
            return Err(AllowanceError::Reason { line });
        }
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
            revision: None,
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

    /// Each line that doesn't read is said word for word: its number and
    /// the rule it breaks.
    #[test]
    fn each_line_that_doesnt_read_says_why() {
        let header = "# Held for the test.\n";
        let row = "FieldTx\tDA\tmissing\t*\t*\ta reason\n";
        let cases = [
            (row.to_string(), "line 1: no # header"),
            (
                format!("{header}{row}# late\n"),
                "line 3: a # line among the rows",
            ),
            (
                format!("{header}FieldTx\tDA\tmissing\t*\ta reason\n"),
                "line 2: 5 fields, not 6: table, key, rule, file, version, reason",
            ),
            (
                format!("{header}Nope\tDA\tmissing\t*\t*\ta reason\n"),
                "line 2: \"Nope\" isn't a table of the model's",
            ),
            (
                format!("{header}FieldTx\t\tmissing\t*\t*\ta reason\n"),
                "line 2: no key",
            ),
            (
                format!("{header}FieldTx\tDA\tbogus\t*\t*\ta reason\n"),
                "line 2: \"bogus\" isn't a rule",
            ),
            (
                format!("{header}FieldTx\tDA\tmissing\t\t*\ta reason\n"),
                "line 2: no file (`*` for any)",
            ),
            (
                format!("{header}FieldTx\tDA\tmissing\t*\t2\ta reason\n"),
                "line 2: \"2\" isn't a version (`*` for any)",
            ),
            (
                format!("{header}FieldTx\tDA\tmissing\t*\t*\t \n"),
                "line 2: no reason",
            ),
            (
                format!("{header}{row}{row}"),
                "line 3: FieldTx/DA missing in * from * is on line 2 already",
            ),
            (
                format!(
                    "{header}FieldTx\tV\ttype\tforms/a.pdf\t2.0\ta\n\
                     FieldTx\tV\ttype\tforms/a.pdf\t2.0\tb\n"
                ),
                "line 3: FieldTx/V type in forms/a.pdf from 2.0 is on line 2 already",
            ),
        ];
        for (text, said) in cases {
            let e = Allowances::parse(&text).unwrap_err();
            assert_eq!(format!("{e:#}"), said, "{text:?}");
        }
    }

    /// A file that doesn't read is said after its path: what std said
    /// reading it, or the line that doesn't read.
    #[test]
    fn a_file_that_doesnt_read_says_its_path() {
        let dir = tempfile::Builder::new()
            .prefix("pdf-lint-allowance-")
            .tempdir()
            .unwrap();
        let said = |path: &Path| format!("{:#}", Allowances::read(path).unwrap_err());
        let missing = dir.path().join("missing.tsv");
        let why = std::fs::read_to_string(&missing).unwrap_err();
        assert_eq!(
            said(&missing),
            format!("reading {}: {why}", missing.display())
        );
        let reasonless = dir.path().join("reasonless.tsv");
        std::fs::write(&reasonless, "# Held.\nFieldTx\tDA\tmissing\t*\t*\t\n").unwrap();
        assert_eq!(
            said(&reasonless),
            format!("{}: line 2: no reason", reasonless.display())
        );
    }
}
