//! `check` held to PDFium's answers, case by case. Each case is in `tests/pdfium`
//! (`cases.tsv`, its bytes in `cases/`, written by `cases.py`), with PDFium's answer
//! for it recorded beside it at the pinned build (`chromium-N.txt`, by the
//! repository's `oracle/pdfium.py`, which `oracle/README.md` describes); here the
//! same words are made from what the check says of the file, and the two must agree,
//! but for the cases `differ.tsv` names, which must still differ.
//!
//! PDFium's answer starts: whether it takes the file as linearized, then, where a
//! document opens, its count of pages and its first page. Here those are the
//! [`Verdict`](pdf_linearize::check::Verdict)'s `linearized`, and what the dictionary says.
//! What it took for each page, it took by the hint tables, so where the check says a
//! file holds, what the page table has a page taking covers what PDFium asked for it.

#![cfg(feature = "family")]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use pdf_linearize::check::{Facts, Finding, HintData, Raw, Unread, Verdict};
use pdf_linearize::hint::{self, HintKeys};
use pdf_linearize::{Limits, check, family};
use pdf_structure::pdf_crypt::Password;
use pdf_structure::read::{Options, Reading};

/// What PDFium reads ahead of what it's asked for, at most: a block on either side
/// of a read (`kAlignBlockValue`, and its parser's buffer).
const READS_AHEAD: u64 = 4096;

/// The set's folder.
fn set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium")
}

/// A file's lines that aren't comments, each cut at its tabs.
fn rows(path: &Path) -> Vec<Vec<String>> {
    let text = fs::read_to_string(path).unwrap_or_default();
    (text.lines())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// The answers recorded at the pinned build: the set's one `chromium-N.txt`.
fn recorded() -> HashMap<String, String> {
    let files: Vec<PathBuf> = (fs::read_dir(set()).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with("chromium-") && name.ends_with(".txt")
        })
        .collect();
    assert_eq!(files.len(), 1, "one recorded answer file: {files:?}");
    (rows(&files[0]).into_iter())
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect()
}

/// The case's bytes and its password, if it has one.
fn case(name: &str) -> (Vec<u8>, Option<String>) {
    let at = set().join("cases");
    let bytes = fs::read(at.join(format!("{name}.pdf"))).unwrap();
    let password = fs::read_to_string(at.join(format!("{name}.password"))).ok();
    (bytes, password)
}

/// What the check says of the case, and its findings.
fn checked(name: &str) -> (Facts, Verdict, Vec<Finding>) {
    let (bytes, password) = case(name);
    let given = password.as_deref().map(Password);
    let given: Vec<Password> = given.into_iter().collect();
    let mut options = Options::DEFAULT;
    options.passwords = &given;
    let reading = Reading::open(&bytes[..], &options).unwrap();
    let facts = family::facts(&reading, &Limits::DEFAULT).unwrap();
    let mut findings = Vec::new();
    let verdict = check(&facts, &Limits::DEFAULT, &mut findings);
    (facts, verdict, findings)
}

/// The words PDFium's answer starts with, as the check has them: without a count
/// where no document opens.
fn ours(name: &str) -> String {
    let (facts, verdict, _) = checked(name);
    let linearized = u8::from(verdict.linearized);
    let integer = |raw: Option<Raw>| match raw {
        Some(Raw::Integer(n)) => Some(n),
        _ => None,
    };
    let first = facts.first_object.as_ref();
    let count = integer(first.map(|first| first.n)).unwrap_or(facts.pages.len() as i64);
    let first_page = integer(first.map(|first| first.p)).unwrap_or(0);
    format!("linearized {linearized} count {count} first {first_page}")
}

/// Of PDFium's answer, the same words: none past `linearized` where it opened no
/// document.
fn theirs(answer: &str) -> String {
    let words: Vec<&str> = answer.split_whitespace().collect();
    let after = |key: &str| {
        (words.iter().position(|word| *word == key)).and_then(|at| words.get(at + 1).copied())
    };
    let linearized = after("linearized").unwrap();
    match (after("count"), after("first")) {
        (Some(count), Some(first)) => {
            format!("linearized {linearized} count {count} first {first}")
        }
        _ => format!("linearized {linearized}"),
    }
}

#[test]
fn every_case_reads_as_pdfium_reads_it() {
    let recorded = recorded();
    let differ = rows(&set().join("differ.tsv"));
    let cases = rows(&set().join("cases.tsv"));
    assert!(!cases.is_empty());
    for case in cases {
        let name = &case[0];
        let answer = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no answer"));
        let theirs = theirs(answer);
        let ours = ours(name);
        // Where no document opens, the first word alone is PDFium's to say; and it's
        // the check's where the file isn't linearized, nothing past that being read.
        let compared =
            |answer: &str| match theirs.contains("count") && ours.contains("linearized 1") {
                true => answer.to_string(),
                false => answer.split(" count").next().unwrap().to_string(),
            };
        let (ours, theirs) = (compared(&ours), compared(&theirs));
        let listed = differ.iter().any(|row| row[0] == *name);
        match listed {
            true => assert_ne!(ours, theirs, "{name} is said to differ, and doesn't"),
            false => assert_eq!(ours, theirs, "{name}"),
        }
    }
    for row in &differ {
        assert!(row.len() > 1, "{row:?} has no reason");
    }
}

/// What the check says of each case by name: which hold, and what's said of those
/// that don't.
#[test]
fn what_holds_is_what_the_cases_are_there_for() {
    let holds_and_reads = [
        "one-page",
        "two-pages",
        "two-pages-shared",
        "many-pages",
        "many-pages-shared",
        "first-is-page-1",
        "first-is-page-5",
        "encrypted-qpdf-rc4",
        "encrypted-qpdf-aes-256",
        "encrypted-written-rc4",
        "encrypted-written-aes-256",
    ];
    for name in holds_and_reads {
        let (_, verdict, findings) = checked(name);
        assert!(
            verdict.linearized && verdict.holds && verdict.hints_read,
            "{name}: {findings:?}"
        );
    }
    for name in ["encrypted-wrong-password", "encrypted-no-password"] {
        let (_, verdict, findings) = checked(name);
        assert!(verdict.linearized && verdict.holds, "{name}: {findings:?}");
        assert!(!verdict.hints_read, "{name}: there's no key for its hints");
        // Which is said once, and not judged.
        let why = Unread::Locked;
        let unread = |f: &&Finding| **f == Finding::HintUnread { why };
        assert_eq!(findings.iter().filter(unread).count(), 1, "{name}");
    }
    let (_, verdict, findings) = checked("not-linearized");
    assert!(!verdict.linearized);
    assert_eq!(findings, []);
    let (_, verdict, findings) = checked("broken-length");
    assert!(!verdict.linearized);
    assert!(
        matches!(findings[..], [Finding::LengthNotFile { .. }]),
        "{findings:?}"
    );
    // Its width says 0, its columns are as they were, and PDFium drops the hints.
    let (_, verdict, findings) = checked("broken-width-zero");
    assert!(verdict.linearized && !verdict.holds && verdict.hints_read);
    assert!(
        (findings.iter()).any(|f| matches!(f, Finding::PageObjects { .. })),
        "{findings:?}"
    );
    let (_, verdict, findings) = checked("broken-hint-short");
    assert!(verdict.linearized && !verdict.holds && !verdict.hints_read);
    assert!(
        matches!(findings[0], Finding::HintTables { .. }),
        "{findings:?}"
    );
    let (_, verdict, findings) = checked("broken-hint-past");
    assert!(verdict.linearized && !verdict.holds, "{findings:?}");
    assert!(
        (findings.iter()).any(|f| matches!(f, Finding::HintNotListed { .. })),
        "{findings:?}"
    );
    let (_, verdict, findings) = checked("broken-first-past");
    assert!(verdict.linearized && !verdict.holds, "{findings:?}");
    assert!(
        findings.contains(&Finding::BadValue {
            key: pdf_linearize::check::DictKey::P
        }),
        "{findings:?}"
    );
    let (_, verdict, findings) = checked("broken-shared-past");
    assert!(verdict.linearized && !verdict.holds, "{findings:?}");
    assert!(
        (findings.iter()).any(|f| matches!(f, Finding::HintTables { .. })),
        "{findings:?}"
    );
}

/// The page table of what's in `facts`.
fn page_lengths(facts: &Facts) -> Vec<u64> {
    let hint = facts.hint.as_ref().unwrap();
    let HintData::Decoded(data) = &hint.data else {
        panic!("decoded");
    };
    let (_, shared) = (hint.keys.iter())
        .find(|(key, _)| key == b"S")
        .expect("it has /S");
    let Raw::Integer(shared) = shared else {
        panic!("/S");
    };
    let first = facts.first_object.as_ref().unwrap();
    let Raw::Integer(pages) = first.n else {
        panic!("/N");
    };
    let keys = HintKeys::new(*shared as u64);
    let tables = hint::tables(data, &keys, pages as u32, &Limits::DEFAULT).unwrap();
    (tables.pages.pages.iter())
        .zip(0..)
        .map(|(entry, _)| {
            let shared: u64 = (entry.shared.iter())
                .filter_map(|(id, _)| tables.shared.groups.get(*id as usize))
                .map(|group| group.length)
                .sum();
            entry.length + shared
        })
        .collect()
}

/// Each page's bytes as PDFium's answer has them: `page N 1 took B`, for each page
/// that came.
fn took(answer: &str) -> Vec<(usize, u64)> {
    let mut pages = Vec::new();
    for stage in answer.split(';') {
        let words: Vec<&str> = stage.split_whitespace().collect();
        if let ["page", page, _, "took", bytes, ..] = words[..] {
            pages.push((page.parse().unwrap(), bytes.parse().unwrap()));
        }
    }
    pages
}

#[test]
fn what_pdfium_took_for_a_page_is_what_the_table_says_it_takes() {
    let recorded = recorded();
    let cases = rows(&set().join("cases.tsv"));
    let mut pages_read = 0;
    for case in cases {
        let name = &case[0];
        // Its hints it doesn't take: a width of 0, encrypted, or a part of them gone.
        if name.starts_with("broken") || name.starts_with("encrypted") || name == "not-linearized" {
            continue;
        }
        let (facts, verdict, findings) = checked(name);
        assert!(verdict.holds, "{name}: {findings:?}");
        let lengths = page_lengths(&facts);
        for (page, took) in took(&recorded[name]) {
            assert!(
                took <= lengths[page] + READS_AHEAD,
                "{name}: page {page}: {took} of {}",
                lengths[page]
            );
            pages_read += 1;
        }
    }
    assert!(pages_read > 20, "{pages_read}");
}
