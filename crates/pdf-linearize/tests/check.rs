//! `family::check` over what's written and what's been damaged since: the bytes of a
//! file, read by pdf-structure's reader, the facts said by what's in them.

#![cfg(feature = "family")]

use std::borrow::Cow;
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use pdf_linearize::check::{Attribute, Facts, HintData};
use pdf_linearize::family::{self, Options};
use pdf_linearize::{Finding, Grade, Limits, Verdict};
use pdf_structure::bytes::{self, ByteSource, Reading, WithReading, XrefEntry};
use pdf_structure::write::{Deflate, Encryptor};
use pdf_structure::{Password, XrefKind};

mod written;
use written::{KEYS, encrypted_as, opened};

/// A plain file, not linearized, with an inherited box and resources, a form and an
/// outline, for the writer to lay out.
fn plain() -> Vec<u8> {
    written::file(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Outlines 7 0 R /AcroForm << /Fields [8 0 R] >> >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /Zzzzzz 0 /MediaBox [0 0 9 9] /Resources 6 0 R >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Font << >> >>",
            "<< /Count 0 >>",
            "<< /FT /Tx /T (a string long enough to find) >>",
        ],
        "/Root 1 0 R",
    )
}

/// `doc` as read and written in `kind`.
fn laid_out(file: &[u8], kind: XrefKind, pack: bool) -> Vec<u8> {
    let reading = Reading::open(file, &bytes::Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    let mut options = Options::DEFAULT;
    options.kind = kind;
    options.pack = pack;
    let mut out = Vec::new();
    family::linearize(&mut out, &doc, None, Some(&reading), &options).unwrap();
    out
}

fn kinds() -> [(XrefKind, bool); 3] {
    [
        (XrefKind::Table, false),
        (XrefKind::Stream, false),
        (XrefKind::Stream, true),
    ]
}

fn checked(file: &[u8]) -> (Verdict, Vec<Finding>) {
    let reading = Reading::open(file, &bytes::Options::DEFAULT).unwrap();
    let mut findings = Vec::new();
    let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
    let quiet = family::check(&reading, &Limits::DEFAULT, &mut ()).unwrap();
    assert_eq!(verdict, quiet);
    (verdict, findings)
}

/// `file` with its first `from` changed to `to`, of the same length so nothing moves.
fn changed(file: &[u8], from: &str, to: &str) -> Vec<u8> {
    assert_eq!(from.len(), to.len());
    let at = (file.windows(from.len()))
        .position(|window| window == from.as_bytes())
        .unwrap_or_else(|| panic!("{from} isn't in it"));
    let mut out = file.to_vec();
    out[at..at + to.len()].copy_from_slice(to.as_bytes());
    out
}

/// A dictionary's value, as it's written.
fn value(file: &[u8], key: &str) -> u64 {
    let text = String::from_utf8_lossy(&file[..300]).into_owned();
    let rest = text.split(&format!("/{key} ")).nth(1).unwrap();
    rest.split_ascii_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn what_the_writer_writes_holds_in_each_kind_with_nothing_to_say() {
    for (kind, pack) in kinds() {
        let file = laid_out(&plain(), kind, pack);
        let (verdict, findings) = checked(&file);
        assert!(verdict.linearized && verdict.holds && verdict.hints_read);
        // These are the file's own pages' to say: the source's has no box.
        assert_eq!(findings, []);
    }
}

#[test]
fn it_has_read_each_thing_it_held_the_file_to() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let reading = Reading::open(&file[..], &bytes::Options::DEFAULT).unwrap();
    let facts: Facts = family::facts(&reading, &Limits::DEFAULT).unwrap();
    assert_eq!(facts.len, file.len() as u64);
    assert_eq!(facts.header, 0);
    assert!(facts.first_object.is_some());
    assert!(facts.first.is_some() && facts.main.is_some());
    assert_eq!(facts.pages.len(), 3);
    assert!(facts.graph.is_some(), "what each page reaches is said");
    assert!(!facts.objects.is_empty());
    let hint = facts.hint.expect("a hint stream");
    assert!(matches!(hint.data, HintData::Decoded(_)));
    assert!(hint.keys.iter().any(|(key, _)| key == b"S"));
    assert_eq!(facts.update, None);
}

#[test]
fn a_file_that_doesnt_say_it_is_has_nothing_read_and_nothing_said() {
    let file = plain();
    let reading = Reading::open(&file[..], &bytes::Options::DEFAULT).unwrap();
    let facts = family::facts(&reading, &Limits::DEFAULT).unwrap();
    assert!(facts.first.is_none() && facts.hint.is_none() && facts.graph.is_none());
    let (verdict, findings) = checked(&file);
    assert!(!verdict.linearized && !verdict.holds && !verdict.hints_read);
    assert_eq!(findings, []);
    let (verdict, findings) = checked(b"not a file");
    assert!(!verdict.linearized);
    assert_eq!(findings, []);
}

#[test]
fn a_length_off_says_it_isnt_and_an_update_is_said() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let l = value(&file, "L");
    let off = changed(&file, &format!("/L {l}"), &format!("/L {}", l + 1));
    let (verdict, findings) = checked(&off);
    assert!(!verdict.linearized && !verdict.holds);
    assert_eq!(
        findings,
        [Finding::LengthNotFile {
            l: l as i64 + 1,
            len: l
        }]
    );

    // An update after it: the reader's rule, its section after the one it follows.
    let mut updated = file.clone();
    let at = updated.len();
    updated.extend_from_slice(b"1 0 obj\n<< >>\nendobj\n");
    let section = updated.len();
    let startxref = {
        let text = &file[file.len() - 40..];
        let text = String::from_utf8_lossy(text);
        let value = text
            .split("startxref")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next();
        value.unwrap().to_string()
    };
    updated.extend_from_slice(
        format!(
            "xref\n1 1\n{at:010} 00000 n \ntrailer\n<< /Size 3 /Prev {startxref} /Root 1 0 R >>\nstartxref\n{section}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let (verdict, findings) = checked(&updated);
    assert!(!verdict.linearized);
    assert_eq!(
        findings,
        [
            Finding::LengthNotFile {
                l: l as i64,
                len: updated.len() as u64
            },
            Finding::Updated { at: section as u64 }
        ]
    );
    assert_eq!(findings[1].grade(), Grade::Departure);
}

#[test]
fn a_page_count_off_is_said() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let (verdict, findings) = checked(&changed(&file, "/N 3 ", "/N 4 "));
    assert!(!verdict.holds);
    assert!(
        findings.contains(&Finding::PageCount { n: 4, pages: 3 }),
        "{findings:?}"
    );
}

#[test]
fn t_off_is_said() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let t = value(&file, "T");
    let (verdict, findings) = checked(&changed(
        &file,
        &format!("/T {t}"),
        &format!("/T {}", t - 1),
    ));
    assert!(!verdict.holds);
    assert!(
        (findings.iter()).any(|f| matches!(f, Finding::MainFirstEntry { .. })),
        "{findings:?}"
    );
}

#[test]
fn e_off_is_said() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let e = value(&file, "E");
    let (verdict, findings) = checked(&changed(
        &file,
        &format!("/E {e}"),
        &format!("/E {}", e - 3),
    ));
    assert!(!verdict.holds);
    assert!(
        (findings.iter()).any(|f| matches!(f, Finding::FirstPageEnd { .. })),
        "{findings:?}"
    );
}

#[test]
fn an_attribute_left_on_a_node_is_said_as_written_by_another() {
    // As another writer leaves it, which qpdf's check stops at.
    let file = laid_out(&plain(), XrefKind::Table, false);
    let edited = changed(&file, "/Zzzzzz", "/Rotate");
    let (verdict, findings) = checked(&edited);
    assert!(verdict.holds, "{findings:?}");
    assert!(
        (findings.iter()).any(|f| matches!(
            f,
            Finding::NodeAttribute {
                attribute: Attribute::Rotate,
                ..
            }
        )),
        "{findings:?}"
    );
}

#[test]
fn a_files_own_layout_by_another_writer_is_held_too() {
    // PDFium's cases are a layout of their own, not this crate's.
    for name in [
        "one-page",
        "two-pages",
        "two-pages-shared",
        "many-pages",
        "many-pages-shared",
    ] {
        let file = std::fs::read(format!(
            "{}/tests/pdfium/cases/{name}.pdf",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let (verdict, findings) = checked(&file);
        assert!(
            verdict.linearized && verdict.holds && verdict.hints_read,
            "{name}: {findings:?}"
        );
    }
}

/// A tracked case, as its generator wrote it.
fn case(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/pdfium/cases/{name}.pdf",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn a_file_that_says_it_has_no_pages_is_told_of_that_and_nothing_panics() {
    let file = changed(&case("two-pages"), "/N 2", "/N 0");
    let (verdict, findings) = checked(&file);
    assert!(verdict.linearized && verdict.hints_read && !verdict.holds);
    let told = |f: &Finding| matches!(f, Finding::PageCount { n: 0, pages: 2 });
    assert!(findings.iter().any(told), "{findings:?}");
}

#[test]
fn a_row_numbered_the_last_there_can_be_is_read_and_told_and_nothing_panics() {
    let file = case("two-pages");
    let trailer = file.windows(7).rposition(|w| w == b"trailer").unwrap();
    let mut top = file[..trailer].to_vec();
    top.extend_from_slice(b"4294967295 1\n0000000009 00000 n \n");
    top.extend_from_slice(&file[trailer..]);
    let (from, to) = (file.len().to_string(), top.len().to_string());
    let file = changed(&top, &format!("/L {from}"), &format!("/L {to}"));
    let (verdict, findings) = checked(&file);
    assert!(verdict.linearized && !verdict.holds);
    let told = |f: &Finding| matches!(f, Finding::MainSubsections { count: 2 });
    assert!(findings.iter().any(told), "{findings:?}");
}

#[test]
fn encrypted_it_reads_the_hint_stream_by_the_password_and_holds() {
    for (name, key) in KEYS {
        let file = encrypted_as(key, &plain());
        let reading = opened(&file, "");
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        let doc = reading.to_document().unwrap();
        for (kind, pack) in kinds() {
            let mut options = Options::DEFAULT;
            options.kind = kind;
            options.pack = pack;
            options.deflate = Deflate::DEFAULT;
            let mut file = Vec::new();
            family::linearize(&mut file, &doc, Some(&encryptor), Some(&reading), &options).unwrap();
            let (verdict, findings) = checked(&file);
            assert!(
                verdict.linearized && verdict.holds && verdict.hints_read,
                "{name}: {findings:?}"
            );
            assert_eq!(findings, [], "{name}");
        }
    }
}

#[test]
fn with_no_key_it_says_what_it_can_and_doesnt_say_it_read_the_tables() {
    let file = std::fs::read(format!(
        "{}/tests/pdfium/cases/encrypted-qpdf-rc4.pdf",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    // No password opens it this way, and there's no graph for what's locked.
    let reading = Reading::open(&file[..], &bytes::Options::DEFAULT).unwrap();
    assert!(
        family::facts(&reading, &Limits::DEFAULT)
            .unwrap()
            .graph
            .is_none()
    );
    let (verdict, findings) = checked(&file);
    assert!(verdict.linearized && verdict.holds, "{findings:?}");
    assert!(!verdict.hints_read);
    // qpdf numbers what's after it above its hint stream.
    assert!(
        (findings.iter()).all(|f| f.grade() == Grade::Departure),
        "{findings:?}"
    );
    let password = [Password("user")];
    let reading = Reading::open_with(&file[..], &password, &bytes::Options::DEFAULT).unwrap();
    let mut findings = Vec::new();
    let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
    assert!(
        verdict.linearized && verdict.holds && verdict.hints_read,
        "{findings:?}"
    );
}

#[test]
fn a_length_from_the_header_is_as_any_reader_has_it() {
    // Positions a reader counts from the file's first byte don't read past the junk:
    // that's said, but the first object and /L are from the header.
    let file = laid_out(&plain(), XrefKind::Table, false);
    let mut junk = b"junk\n".to_vec();
    junk.extend_from_slice(&file);
    let (verdict, _) = checked(&junk);
    assert!(verdict.linearized && !verdict.holds);
}

#[test]
fn a_page_with_no_attribute_of_its_own_is_said() {
    let source = written::file(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] >>",
        ],
        "/Root 1 0 R",
    );
    let file = laid_out(&source, XrefKind::Table, false);
    let (verdict, findings) = checked(&file);
    assert!(verdict.holds);
    assert_eq!(
        findings,
        [Finding::PageAttribute {
            page: 0,
            attribute: Attribute::Resources
        }]
    );
}

#[test]
fn another_first_page_holds_by_what_p_says() {
    let reading = Reading::open(&plain()[..], &bytes::Options::DEFAULT)
        .unwrap()
        .to_document();
    let doc = reading.unwrap();
    for page in [1, 2] {
        let mut options = Options::DEFAULT;
        options.first_page = family::FirstPage::Index(page);
        let mut file = Vec::new();
        family::linearize(&mut file, &doc, None, None, &options).unwrap();
        let (verdict, findings) = checked(&file);
        assert!(verdict.linearized && verdict.holds, "{findings:?}");
        assert_eq!(findings, [], "page {page}");
    }
}

/// Each linearized file here, read whole beside its reading, has the facts its
/// reading has: what's written in each kind, one packed into object streams among
/// them, and encrypted under each key, and the cases PDFium's answers are held at,
/// each by its password, broken and locked ones among them. Its graph is the
/// reading's whole read's, not a document like it.
#[test]
fn a_file_read_whole_beside_its_reading_has_its_readings_facts() {
    let mut files: Vec<(String, Vec<u8>, Option<String>)> = Vec::new();
    for (kind, pack) in kinds() {
        files.push((
            format!("{kind:?} {pack}"),
            laid_out(&plain(), kind, pack),
            None,
        ));
    }
    for (name, key) in KEYS {
        let file = encrypted_as(key, &plain());
        let reading = opened(&file, "");
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        let doc = reading.to_document().unwrap();
        for (kind, pack) in kinds() {
            let mut options = Options::DEFAULT;
            options.kind = kind;
            options.pack = pack;
            let mut out = Vec::new();
            family::linearize(&mut out, &doc, Some(&encryptor), Some(&reading), &options).unwrap();
            files.push((format!("{name} {kind:?} {pack}"), out, Some(String::new())));
        }
    }
    let cases = format!("{}/tests/pdfium/cases", env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(&cases).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "pdf") {
            continue;
        }
        let password = std::fs::read_to_string(path.with_extension("password")).ok();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        files.push((name, std::fs::read(&path).unwrap(), password));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let (mut graphs, mut packed) = (0, 0);
    for (name, file, password) in &files {
        let passwords: Vec<Password> = password.iter().map(|p| Password(p.trim_end())).collect();
        let read = WithReading::new(&file[..], &passwords, &bytes::Options::DEFAULT).unwrap();
        let with = family::facts_with_reading(&read, &Limits::DEFAULT).unwrap();
        let reading = Reading::open_with(&file[..], &passwords, &bytes::Options::DEFAULT).unwrap();
        let alone = family::facts(&reading, &Limits::DEFAULT).unwrap();
        assert_eq!(format!("{with:?}"), format!("{alone:?}"), "{name}");
        // And of the reading the document was read beside, asked after it.
        let again = family::facts(read.reading(), &Limits::DEFAULT).unwrap();
        assert_eq!(format!("{with:?}"), format!("{again:?}"), "{name}");
        graphs += usize::from(with.graph.is_some());
        let member =
            |(_, _, entry): (u32, u16, XrefEntry)| matches!(entry, XrefEntry::Compressed { .. });
        packed += usize::from(with.graph.is_some() && reading.entries().any(member));
    }
    let counts = (files.len(), graphs, packed);
    assert!(
        counts.0 >= 38 && counts.1 >= 34 && counts.2 >= 7,
        "{counts:?}"
    );
}

/// A file's bytes given only a window at a time.
struct ByPosition<'a>(&'a [u8]);

impl ByteSource for ByPosition<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        Ok(Cow::Borrowed(&self.0[range]))
    }

    fn first_window(&self) -> usize {
        64
    }
}

#[test]
fn by_position_it_says_the_same() {
    for (kind, pack) in kinds() {
        let file = laid_out(&plain(), kind, pack);
        let reading = Reading::open(ByPosition(&file), &bytes::Options::DEFAULT).unwrap();
        let mut findings = Vec::new();
        let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
        assert_eq!((verdict, findings), checked(&file));
    }
}

/// A source that fails once it's told to.
struct Failing<'a>(&'a [u8], AtomicBool);

impl ByteSource for Failing<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        match self.1.load(Ordering::Relaxed) {
            true => Err(io::Error::other("gone")),
            false => Ok(Cow::Borrowed(&self.0[range])),
        }
    }
}

#[test]
fn a_source_that_fails_is_an_error_not_a_finding() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let source = Failing(&file, AtomicBool::new(false));
    let reading = Reading::open(&source, &bytes::Options::DEFAULT).unwrap();
    source.1.store(true, Ordering::Relaxed);
    assert!(matches!(
        family::facts(&reading, &Limits::DEFAULT),
        Err(family::Error::Read(bytes::Error::Io(_)))
    ));
    assert!(matches!(
        family::check(&reading, &Limits::DEFAULT, &mut ()),
        Err(family::Error::Read(bytes::Error::Io(_)))
    ));
}

/// What a page's `/Parent` names is a node to the check as it is to the writer: not a
/// listed page, nor what has no kids and doesn't say it's one, though either holds an
/// attribute; a node that holds one is said.
#[test]
fn a_node_is_what_the_writer_calls_one() {
    let said = |objects: &[&str]| {
        let linearized = |length: usize| format!("<< /Linearized 1 /L {length:06} >>");
        let all = |first: &str| {
            let mut all = vec![first];
            all.extend_from_slice(objects);
            written::file(&all, "/Root 2 0 R")
        };
        let length = all(&linearized(0)).len();
        let bytes = all(&linearized(length));
        let reading = Reading::open(&bytes[..], &bytes::Options::DEFAULT).unwrap();
        let mut findings = Vec::new();
        let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
        assert!(verdict.linearized);
        let held = findings.iter();
        let held = held.filter_map(|finding| match finding {
            Finding::NodeAttribute { number, .. } => Some(*number),
            _ => None,
        });
        held.collect::<Vec<_>>()
    };
    let catalog = "<< /Type /Catalog /Pages 3 0 R >>";
    let own = "/MediaBox [0 0 9 9] /Resources << >>";
    let root = "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 >>";
    // A listed page with kids of its own, the other's /Parent.
    let listed = format!("<< /Type /Page /Kids [] /Parent 3 0 R {own} >>");
    let under = format!("<< /Type /Page /Parent 4 0 R {own} >>");
    assert_eq!(said(&[catalog, root, &listed, &under]), [] as [u32; 0]);
    // No page, and nothing that says it's a node.
    let page = format!("<< /Type /Page /Parent 3 0 R {own} >>");
    let under = format!("<< /Type /Page /Parent 6 0 R {own} >>");
    let other = "<< /Rotate 90 >>";
    assert_eq!(said(&[catalog, root, &page, &under, other]), [] as [u32; 0]);
    // A node, holding one.
    let root = "<< /Type /Pages /Kids [4 0 R] /Count 1 /Rotate 90 >>";
    assert_eq!(said(&[catalog, root, &page]), [3]);
}
