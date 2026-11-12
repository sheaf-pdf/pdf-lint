//! `family::check` over what's written and what's been damaged since: the bytes of a
//! file, read by pdf-structure's reader, the facts said by what's in them.

#![cfg(feature = "family")]

use std::borrow::Cow;
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use pdf_linearize::check::{Attribute, DictKey, Facts, HintData, Unread};
use pdf_linearize::family::{self, Options};
use pdf_linearize::{Finding, Grade, Limits, Verdict, hint, plan};
use pdf_structure::XrefKind;
use pdf_structure::pdf_crypt::Password;
use pdf_structure::read::{self, ByteSource, Reading, WithReading, XrefEntry};
use pdf_structure::write::{Deflate, Encryptor};

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
    let reading = Reading::open(file, &read::Options::DEFAULT).unwrap();
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
    let reading = Reading::open(file, &read::Options::DEFAULT).unwrap();
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
    let reading = Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
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
    let reading = Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
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
        // Under RC4, told to reuse the file's keystreams, as the layout
        // puts each object at another's.
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        let encryptor = encryptor.reusing_rc4_keystreams();
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
    let reading = Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
    assert!(
        family::facts(&reading, &Limits::DEFAULT)
            .unwrap()
            .graph
            .is_none()
    );
    let (verdict, findings) = checked(&file);
    assert!(verdict.linearized && verdict.holds, "{findings:?}");
    assert!(!verdict.hints_read);
    // qpdf numbers what's after it above its hint stream; and the hint stream
    // isn't read, which is said once.
    let why = Unread::Locked;
    let unread = |f: &&Finding| **f == Finding::HintUnread { why };
    assert_eq!(findings.iter().filter(unread).count(), 1, "{findings:?}");
    assert!(
        (findings.iter())
            .filter(|f| !unread(f))
            .all(|f| f.grade() == Grade::Departure),
        "{findings:?}"
    );
    let password = [Password("user")];
    let mut options = read::Options::DEFAULT;
    options.passwords = &password;
    let reading = Reading::open(&file[..], &options).unwrap();
    let mut findings = Vec::new();
    let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
    assert!(
        verdict.linearized && verdict.holds && verdict.hints_read,
        "{findings:?}"
    );
}

/// `file` checked as the reader reads it under `options`.
fn checked_as(file: &[u8], options: &read::Options<'_>) -> (Verdict, Vec<Finding>) {
    let reading = Reading::open(file, options).unwrap();
    let mut findings = Vec::new();
    let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
    let quiet = family::check(&reading, &Limits::DEFAULT, &mut ()).unwrap();
    assert_eq!(verdict, quiet);
    (verdict, findings)
}

/// The hint stream's decoded data, as the reader gives it by default.
fn hint_data(file: &[u8]) -> Vec<u8> {
    let reading = Reading::open(file, &read::Options::DEFAULT).unwrap();
    let facts = family::facts(&reading, &Limits::DEFAULT).unwrap();
    match facts.hint.unwrap().data {
        HintData::Decoded(data) => data,
        other => panic!("{other:?}"),
    }
}

/// A hint stream whose data is past the reader's cap on a stream's isn't read,
/// and isn't judged: what decoded up to the cap isn't its data. At the cap it's
/// read whole. Its tables aren't read, and the file holds by the rest.
#[test]
fn a_hint_stream_past_the_readers_cap_isnt_judged() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let len = hint_data(&file).len();
    let mut options = read::Options::DEFAULT;
    options.limits.stream = len;
    let (verdict, findings) = checked_as(&file, &options);
    assert!(verdict.linearized && verdict.holds && verdict.hints_read);
    assert_eq!(findings, []);
    options.limits.stream = len - 1;
    let (verdict, findings) = checked_as(&file, &options);
    assert!(verdict.linearized && verdict.holds && !verdict.hints_read);
    let why = Unread::Capped;
    assert_eq!(findings, [Finding::HintUnread { why }]);
}

/// Cross-reference streams cut at the reader's cap stop its sections short of
/// the main one, which is said, and nothing they place, or the hint stream they
/// list, is judged.
#[test]
fn sections_cut_at_the_readers_cap_on_a_stream_arent_judged() {
    let file = laid_out(&plain(), XrefKind::Stream, false);
    let mut options = read::Options::DEFAULT;
    options.limits.stream = 16;
    let reading = Reading::open(&file[..], &options).unwrap();
    let facts = family::facts(&reading, &Limits::DEFAULT).unwrap();
    assert_eq!(facts.unread.sections, Some(Unread::Capped));
    let (verdict, findings) = checked_as(&file, &options);
    assert!(verdict.linearized && verdict.holds, "{findings:?}");
    let why = Unread::Capped;
    assert_eq!(findings, [Finding::SectionsUnread { why }]);
}

/// Sections past how many the reader reads stop short of the main one, which is
/// said: of what the first lists, its own shape and the hint stream it lists
/// are judged, and nothing they'd place.
#[test]
fn sections_past_the_readers_cap_on_how_many_it_reads_arent_judged() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let mut options = read::Options::DEFAULT;
    options.limits.sections = 1;
    let reading = Reading::open(&file[..], &options).unwrap();
    let facts = family::facts(&reading, &Limits::DEFAULT).unwrap();
    assert_eq!(facts.unread.sections, Some(Unread::Capped));
    assert!(facts.first.is_some() && facts.main.is_none());
    let (verdict, findings) = checked_as(&file, &options);
    assert!(verdict.linearized && verdict.holds, "{findings:?}");
    let why = Unread::Capped;
    assert_eq!(findings, [Finding::SectionsUnread { why }]);
}

/// Two sections and no hint stream: the first, read first, lists the
/// dictionary, the catalog, a root node of two pages holding `/Rotate` for
/// them, and the first page; the main one, by its `/Prev`, the second page.
fn a_tree_across_the_sections() -> Vec<u8> {
    let made = |length: usize| {
        let page = "<< /Type /Page /Parent 3 0 R /MediaBox [0 0 9 9] /Resources << >> >>";
        let objects = [
            format!("<< /Linearized 1 /L {length:08} /N 2 /O 4 >>"),
            "<< /Type /Catalog /Pages 3 0 R >>".to_string(),
            "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /Rotate 0 >>".to_string(),
            page.to_string(),
            page.to_string(),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut rows = Vec::new();
        for (number, object) in (1..).zip(&objects) {
            rows.push(out.len());
            out.extend_from_slice(format!("{number} 0 obj\n{object}\nendobj\n").as_bytes());
        }
        let main = out.len();
        let row = |at: usize| format!("{at:010} 00000 n \n");
        out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n5 1\n");
        out.extend_from_slice(row(rows[4]).as_bytes());
        out.extend_from_slice(b"trailer\n<< /Size 6 /Root 2 0 R >>\n");
        let first = out.len();
        out.extend_from_slice(b"xref\n1 4\n");
        for &at in &rows[..4] {
            out.extend_from_slice(row(at).as_bytes());
        }
        let trailer = format!("<< /Size 6 /Root 2 0 R /Prev {main} >>");
        let end = format!("trailer\n{trailer}\nstartxref\n{first}\n%%EOF\n");
        out.extend_from_slice(end.as_bytes());
        out
    };
    made(made(0).len())
}

/// A page tree read past where the reader stopped the sections, at its cap on
/// how many it reads, is read without the page listed past there: neither `/N`
/// nor which page is first is judged of it, nor a page by its place, and what
/// its node holds is.
#[test]
fn a_page_tree_read_past_where_the_sections_stopped_isnt_counted() {
    let file = a_tree_across_the_sections();
    let mut options = read::Options::DEFAULT;
    let pages = |options: &read::Options<'_>| {
        let reading = Reading::open(&file[..], options).unwrap();
        let facts = family::facts(&reading, &Limits::DEFAULT).unwrap();
        (facts.unread.sections, facts.pages.len())
    };
    assert_eq!(pages(&options), (None, 2));
    let (_, findings) = checked_as(&file, &options);
    let node = Finding::NodeAttribute {
        number: 3,
        attribute: Attribute::Rotate,
    };
    assert!(findings.contains(&node), "{findings:?}");
    assert!(
        !findings
            .iter()
            .any(|f| matches!(f, Finding::PageCount { .. })),
        "{findings:?}"
    );
    options.limits.sections = 1;
    assert_eq!(pages(&options), (Some(Unread::Capped), 1));
    let (verdict, findings) = checked_as(&file, &options);
    // The dictionary's own, as there's no hint stream; the cap; the node.
    let bad = |key| Finding::BadValue { key };
    let why = Unread::Capped;
    assert_eq!(
        findings,
        [
            bad(DictKey::H),
            bad(DictKey::E),
            bad(DictKey::T),
            Finding::SectionsUnread { why },
            node
        ]
    );
    assert!(verdict.linearized && !verdict.holds);
}

/// A hint stream through a filter the reader doesn't take, an image's or one
/// it doesn't know, isn't read, and isn't judged: no key, cap or damage of the
/// file's is said of it.
#[test]
fn a_hint_stream_through_a_filter_the_reader_doesnt_take_isnt_judged() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let reading = Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
    let hint = family::facts(&reading, &Limits::DEFAULT)
        .unwrap()
        .hint
        .unwrap();
    for filter in ["/DCTDecode  ", "/XXXDecode  "] {
        let unfiltered = changed(&file, "/FlateDecode", filter);
        let at = (unfiltered.windows(filter.len()))
            .position(|window| window == filter.as_bytes())
            .unwrap() as u64;
        assert!(
            (hint.at..hint.end).contains(&at),
            "the hint stream's filter"
        );
        let (verdict, findings) = checked(&unfiltered);
        assert!(verdict.linearized && verdict.holds && !verdict.hints_read);
        let why = Unread::Unfiltered;
        assert_eq!(findings, [Finding::HintUnread { why }], "{filter}");
    }
}

/// An encrypted file read with no password has its hint stream not read, and,
/// packed, its page tree's nodes in object streams no key decrypts: each said
/// once, and nothing of either judged.
#[test]
fn what_no_key_opens_isnt_judged() {
    use chacha20::ChaCha20Rng;
    use pdf_structure::pdf_crypt::Protection;
    use pdf_structure::pdf_crypt::rand_core::SeedableRng;

    let prepared = Password("user").prepared().unwrap();
    let protection = Protection::new(&prepared);
    let mut rng = ChaCha20Rng::seed_from_u64(3);
    let encryptor = Encryptor::create_with(&protection, &mut rng).unwrap();
    let plain = plain();
    let reading = Reading::open(&plain[..], &read::Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    let why = Unread::Locked;
    for (kind, pack) in kinds() {
        let mut options = Options::DEFAULT;
        options.kind = kind;
        options.pack = pack;
        let mut file = Vec::new();
        family::linearize(&mut file, &doc, Some(&encryptor), Some(&reading), &options).unwrap();
        let (verdict, findings) = checked(&file);
        assert!(verdict.linearized && verdict.holds && !verdict.hints_read);
        let mut unread = vec![Finding::HintUnread { why }];
        if pack {
            unread.push(Finding::PagesUnread { why });
        }
        assert_eq!(findings, unread, "{kind:?} {pack}");
        // By its password, all of it is read and holds.
        let password = [Password("user")];
        let mut options = read::Options::DEFAULT;
        options.passwords = &password;
        let (verdict, findings) = checked_as(&file, &options);
        assert!(verdict.linearized && verdict.holds && verdict.hints_read);
        assert_eq!(findings, []);
    }
}

/// What's past a cap the check reads under, the hint tables past its entries
/// or pages and the file's graph past a plan's visits, is said, not judged,
/// and the file holds by the rest; at the cap, it's judged.
#[test]
fn what_s_past_the_checks_own_caps_isnt_judged() {
    let file = laid_out(&plain(), XrefKind::Table, false);
    let reading = Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
    let under = |limits: &Limits| {
        let mut findings = Vec::new();
        let verdict = family::check(&reading, limits, &mut findings).unwrap();
        assert_eq!(family::check(&reading, limits, &mut ()).unwrap(), verdict);
        assert!(verdict.linearized && verdict.holds, "{findings:?}");
        (verdict, findings)
    };
    let mut limits = Limits::DEFAULT;
    limits.hint_entries = 2;
    let (verdict, findings) = under(&limits);
    assert!(!verdict.hints_read);
    let entries = |f: &Finding| {
        matches!(
            f,
            Finding::HintTablesCapped {
                fault: hint::Error::TooManyEntries { limit: 2, .. }
            }
        )
    };
    assert!(matches!(&findings[..], [f] if entries(f)), "{findings:?}");
    let mut limits = Limits::DEFAULT;
    limits.pages = 2;
    let (verdict, findings) = under(&limits);
    assert!(!verdict.hints_read);
    let fault = hint::Error::TooManyPages { pages: 3, limit: 2 };
    assert_eq!(findings, [Finding::HintTablesCapped { fault }]);
    limits.pages = 3;
    let (verdict, findings) = under(&limits);
    assert!(verdict.hints_read);
    assert_eq!(findings, []);
    let mut limits = Limits::DEFAULT;
    limits.shared_visits = 1;
    let (verdict, findings) = under(&limits);
    assert!(verdict.hints_read);
    let visits = |f: &Finding| {
        matches!(
            f,
            Finding::GraphCapped {
                why: plan::Error::TooManyHints { .. }
            }
        )
    };
    assert!(matches!(&findings[..], [f] if visits(f)), "{findings:?}");
}

/// A file that says it's linearized, one section and no hint stream, its
/// page tree a chain of `depth` nodes over one page, its `/N` 7 and its `/O`
/// 9, which aren't its page's.
fn deep(depth: u32) -> Vec<u8> {
    let mut objects = vec!["<< /Type /Catalog /Pages 3 0 R >>".to_string()];
    for node in 3..3 + depth {
        let parent = match node {
            3 => String::new(),
            _ => format!("/Parent {} 0 R", node - 1),
        };
        let kid = node + 1;
        objects.push(format!(
            "<< /Type /Pages /Kids [{kid} 0 R] /Count 1 {parent} >>"
        ));
    }
    let last = 2 + depth;
    objects.push(format!("<< /Type /Page /Parent {last} 0 R >>"));
    let said = |length: usize| format!("<< /Linearized 1 /L {length:06} /N 7 /O 9 >>");
    let all = |first: &str| {
        let mut all = vec![first];
        all.extend(objects.iter().map(String::as_str));
        written::file(&all, "/Root 2 0 R")
    };
    let length = all(&said(0)).len();
    all(&said(length))
}

/// A page tree deeper than pdf-semantics walks isn't read, and isn't judged:
/// none of its pages, their count or the first page's object is said, though
/// the rest is; within the depth, they are.
#[test]
fn a_page_tree_past_the_readers_depth_isnt_judged() {
    let of_pages = |f: &&Finding| {
        matches!(
            f,
            Finding::NoPages
                | Finding::PageCount { .. }
                | Finding::FirstPageObject { .. }
                | Finding::PageAttribute { .. }
                | Finding::NodeAttribute { .. }
        )
    };
    let (verdict, findings) = checked(&deep(1100));
    assert!(verdict.linearized && !verdict.holds);
    let why = Unread::Capped;
    let unread = Finding::PagesUnread { why };
    assert_eq!(findings.iter().filter(|f| **f == unread).count(), 1);
    assert_eq!(findings.iter().filter(of_pages).count(), 0, "{findings:?}");
    assert!(findings.contains(&Finding::NoMainSection), "{findings:?}");
    let (_, findings) = checked(&deep(10));
    assert!(!findings.contains(&unread), "{findings:?}");
    assert!(
        findings.contains(&Finding::PageCount { n: 7, pages: 1 }),
        "{findings:?}"
    );
}

/// Every offset a file writes counts from its header (ISO 32000-2, 7.5.2), the
/// reader's rows and the linearization's values alike: bytes before the header
/// move nothing the check holds, in each layout, a table, a stream and one
/// packed, whatever comes before it.
#[test]
fn a_length_from_the_header_is_as_any_reader_has_it() {
    for (kind, pack) in kinds() {
        let file = laid_out(&plain(), kind, pack);
        let (verdict, findings) = checked(&file);
        assert!(verdict.linearized && verdict.holds, "{kind:?} {pack}");
        for junk in [&b"junk\n"[..], b" ", &[b'~'; 100]] {
            let prefixed = [junk, &file].concat();
            let reading = Reading::open(&prefixed[..], &read::Options::DEFAULT).unwrap();
            assert_eq!(reading.facts().header_offset, Some(junk.len()));
            let (got, said) = checked(&prefixed);
            let name = format!("{kind:?} {pack}, {} before", junk.len());
            assert_eq!(got, verdict, "{name}: {said:?}");
            assert_eq!(said, findings, "{name}");
        }
    }
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
    let reading = Reading::open(&plain()[..], &read::Options::DEFAULT)
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
        // Under RC4, told to reuse the file's keystreams.
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        let encryptor = encryptor.reusing_rc4_keystreams();
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
        let mut options = read::Options::DEFAULT;
        options.passwords = &passwords;
        let read = WithReading::new(&file[..], &options).unwrap();
        let with = family::facts_with_reading(&read, &Limits::DEFAULT).unwrap();
        let reading = Reading::open(&file[..], &options).unwrap();
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
        let reading = Reading::open(ByPosition(&file), &read::Options::DEFAULT).unwrap();
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
    let reading = Reading::open(&source, &read::Options::DEFAULT).unwrap();
    source.1.store(true, Ordering::Relaxed);
    assert!(matches!(
        family::facts(&reading, &Limits::DEFAULT),
        Err(family::Error::Read(read::Error::Io(_)))
    ));
    assert!(matches!(
        family::check(&reading, &Limits::DEFAULT, &mut ()),
        Err(family::Error::Read(read::Error::Io(_)))
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
        let reading = Reading::open(&bytes[..], &read::Options::DEFAULT).unwrap();
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
