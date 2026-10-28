//! The command line, run as a user runs it.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use pdf_crypt::writer::{self, Kind, Settings};
use pdf_crypt::{Dictionary, Handler};

// pdf-structure's reader of qpdf's JSON, to read the JSON report back with.
#[path = "../../pdf-structure/src/json.rs"]
#[allow(dead_code)]
mod json;

/// A file of these objects' bodies, with a table, written by hand.
fn file(objects: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let size = objects.len() + 1;
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// `bytes` written to a file of the test's own, named `name`.
fn written(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdf-lint-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// pdf-lint.
fn pdf_lint() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pdf-lint"))
}

fn run(args: &[&str]) -> Output {
    pdf_lint().args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

const CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &str = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";

#[test]
fn a_clean_file_exits_0() {
    let clean = file(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>",
    ]);
    let path = written("clean.pdf", &clean);
    let out = run(&[path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert!(
        text(&out.stdout).contains("0 findings"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn a_finding_is_printed_with_its_bytes_and_exits_1() {
    let faulty = file(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1] /Resources << >> >>",
    ]);
    let path = written("faulty.pdf", &faulty);
    let out = run(&[path.to_str().unwrap()]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("type: PageObject/MediaBox"), "{stdout}");
    assert!(stdout.contains("    | /MediaBox [0 0 1]"), "{stdout}");

    // Allowed, it's left out.
    let allow = written(
        "allow.tsv",
        b"# test\nPageObject\tMediaBox\ttype\t*\t*\tthe test's\n",
    );
    let out = run(&["--allow", allow.to_str().unwrap(), path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert!(
        text(&out.stdout).contains("0 findings, 1 allowed"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn a_hold_file_holds_only_what_it_names() {
    let faulty = file(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1] /Resources << >> >>",
    ]);
    let path = written("held.pdf", &faulty);
    let hold = written("hold.txt", b"# the catalog alone\n1 0 built\n");
    let out = run(&["--hold", hold.to_str().unwrap(), path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
}

#[test]
fn exempt_keys_are_no_findings_in_a_built_object() {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> \
                /WriterPage 1 /WriterMark 2 /Contnets 3 >>";
    let path = written("exempt.pdf", &file(&[CATALOG, PAGES, page]));
    let path = path.to_str().unwrap();
    let unlisted = |args: &[&str]| {
        let out = run(args);
        let stdout = text(&out.stdout);
        let keys: Vec<String> = (stdout.lines())
            .filter_map(|l| l.strip_prefix("- unlisted: "))
            .map(|l| l.split(' ').next().unwrap().to_string())
            .collect();
        assert_eq!(out.status.code(), Some(1), "{stdout}");
        keys
    };
    assert_eq!(
        unlisted(&["--built", path]),
        [
            "PageObject/WriterPage",
            "PageObject/WriterMark",
            "PageObject/Contnets"
        ]
    );
    let one = written("exempt-one.txt", b"# the writer's\n/WriterPage\n");
    let one = one.to_str().unwrap();
    assert_eq!(
        unlisted(&["--built", "--exempt", one, path]),
        ["PageObject/WriterMark", "PageObject/Contnets"]
    );
    // Every key a name begins; and with a hold file, or in what the last
    // update wrote, the built objects.
    let prefix = written("exempt-prefix.txt", b"/Writer*\n");
    let prefix = prefix.to_str().unwrap();
    assert_eq!(
        unlisted(&["--built", "--exempt", prefix, path]),
        ["PageObject/Contnets"]
    );
    let hold = written("exempt-hold.txt", b"3 0 built\n");
    let hold = hold.to_str().unwrap();
    assert_eq!(
        unlisted(&["--hold", hold, "--exempt", prefix, path]),
        ["PageObject/Contnets"]
    );
    let note = "<< /Type /Annot /Subtype /Text /Rect [0 0 1 1] /WriterNote 1 /Contnets 2 >>";
    let page = page.replace("/Resources", "/Annots [4 0 R] /Resources");
    let update = updated(&file(&[CATALOG, PAGES, &page]), &[(3, &page), (4, note)]);
    let update = written("exempt-update.pdf", &update);
    let update = update.to_str().unwrap();
    assert_eq!(
        unlisted(&["--last-update", "--exempt", prefix, update]),
        ["AnnotText/Contnets"]
    );

    // Nothing built, it would leave out nothing, and is refused; and a
    // line that isn't a name is named.
    let out = run(&["--exempt", prefix, path]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("--exempt: "),
        "{}",
        text(&out.stderr)
    );
    let bad = written("exempt-bad.txt", b"/Writer\nMine\n");
    let out = run(&["--built", "--exempt", bad.to_str().unwrap(), path]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("exempt-bad.txt: line 2: \"Mine\" isn't a name"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn unlisted_says_which_objects_a_key_the_model_doesnt_list_is_a_finding_in() {
    let page = |keys: &str| {
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> {keys} >>")
    };
    let path = written(
        "unlisted.pdf",
        &file(&[CATALOG, PAGES, &page("/Contnets 3 /WriterMark 1")]),
    );
    let path = path.to_str().unwrap();
    let unlisted = |args: &[&str]| {
        let out = run(args);
        let stdout = text(&out.stdout);
        let keys: Vec<String> = (stdout.lines())
            .filter_map(|l| l.strip_prefix("- unlisted: "))
            .map(|l| l.split(' ').next().unwrap().to_string())
            .collect();
        (out.status.code(), keys)
    };
    let none = (Some(0), Vec::<String>::new());
    // Every object merged, by default: counted only.
    assert_eq!(unlisted(&[path]), none);
    let both = ["PageObject/Contnets", "PageObject/WriterMark"].map(String::from);
    assert_eq!(
        unlisted(&["--unlisted", "all", path]),
        (Some(1), both.to_vec())
    );
    assert_eq!(unlisted(&["--built", "--unlisted", "never", path]), none);
    assert_eq!(unlisted(&["--built", "--unlisted", "built", path]).1, both);
    // A writer's own key exempt, merged objects and all.
    let exempt = written("unlisted-exempt.txt", b"/WriterMark\n");
    let exempt = exempt.to_str().unwrap();
    assert_eq!(
        unlisted(&["--unlisted", "all", "--exempt", exempt, path]).1,
        ["PageObject/Contnets"]
    );

    // A rewrite, set against what it was written from, carries the key
    // the page had there, and finds the one the writer added.
    let rewrite = written(
        "unlisted-rewrite.pdf",
        &file(&[
            CATALOG,
            PAGES,
            &page("/Rotate 0 /Contnets 3 /WriterMark 1 /Rotat 9"),
        ]),
    );
    let rewrite = rewrite.to_str().unwrap();
    let out = run(&["--unlisted", "all", "--baseline", path, rewrite]);
    let stdout = text(&out.stdout);
    let found: Vec<&str> = stdout.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(found.len(), 1, "{stdout}");
    assert!(found[0].contains("unlisted: PageObject/Rotat "), "{stdout}");
    assert!(stdout.contains("2 carried"), "{stdout}");
    // And so does an incremental update, set against the revision before.
    let update = updated(
        &file(&[CATALOG, PAGES, &page("/Contnets 3")]),
        &[(3, &page("/Contnets 3 /Rotat 9"))],
    );
    let update = written("unlisted-update.pdf", &update);
    let out = run(&[
        "--unlisted",
        "all",
        "--last-update",
        update.to_str().unwrap(),
    ]);
    let stdout = text(&out.stdout);
    let found: Vec<&str> = stdout.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(found.len(), 1, "{stdout}");
    assert!(found[0].contains("unlisted: PageObject/Rotat "), "{stdout}");

    // The JSON says which.
    let out = run(&["--format", "json", "--unlisted", "all", path]);
    let report = json::parse(&out.stdout).unwrap();
    assert_eq!(
        report.get("unlisted_in").cloned(),
        Some(json::Json::String("all".to_string()))
    );

    // Exempting keys where none is a finding is refused, and so is a
    // setting it hasn't.
    for (args, why) in [
        (
            &["--unlisted", "never", "--built", "--exempt", exempt, path][..],
            "--exempt and --unlisted never",
        ),
        (
            &["--unlisted", "some", path][..],
            "--unlisted some: built, all or never",
        ),
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(2));
        assert!(text(&out.stderr).contains(why), "{}", text(&out.stderr));
    }
}

#[test]
fn extensions_name_what_a_file_is_checked_under_besides_what_it_declares() {
    // A page's /Tabs /A, which PDF 1.7 has only under Adobe's level 3.
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> /Tabs /A >>";
    let path = written("extensions.pdf", &file(&[CATALOG, PAGES, page]));
    let path = path.to_str().unwrap();
    let out = run(&[path]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    assert!(text(&out.stdout).contains("value: PageObject/Tabs"));
    for names in ["ADBE_Extn3", "Malforms, ADBE_Extn3"] {
        let out = run(&["--extensions", names, path]);
        assert_eq!(out.status.code(), Some(0), "{names}: {}", text(&out.stdout));
    }
    let out = run(&[
        "--extensions",
        "ADBE_Extn3,Malforms",
        "--format",
        "json",
        path,
    ]);
    let report = json::parse(&out.stdout).unwrap();
    assert_eq!(
        report.get("extensions").cloned(),
        Some(json::parse(br#"["ADBE_Extn3","Malforms"]"#).unwrap())
    );
    // A name the model hasn't is refused, with those it has; and so is
    // `*`, as no file is every extension at once.
    for name in ["ADBE_Extn9", "*"] {
        let out = run(&["--extensions", name, path]);
        assert_eq!(out.status.code(), Some(2));
        let err = text(&out.stderr);
        let refused = format!("{name:?} isn't an extension the model names, which are ");
        assert!(
            err.contains(&refused) && err.contains("ADBE_Extn3, "),
            "{err}"
        );
    }
}

/// An encrypted file's first `/ID` element, which its key is made with.
const ID: &[u8] = b"0123456789abcdef";

/// What the writer encrypts `kind` with, `user` the user's password.
fn settings(kind: Kind, user: &[u8]) -> Settings<'_> {
    Settings {
        kind,
        user,
        owner: b"owner",
        p: -4,
        id: ID,
        encrypt_metadata: true,
    }
}

/// [`in_an_object_stream`]'s file, encrypted as `kind` with `user` for the
/// user's password and `owner` for the owner's: its object stream's data
/// encrypted, the encryption dictionary and `/ID` in its trailer.
fn crypted(kind: Kind, user: &[u8]) -> Vec<u8> {
    crypted_with(kind, user, &object_stream_objects().0)
}

/// [`crypted`]'s file, its object stream holding `objects`.
fn crypted_with(kind: Kind, user: &[u8], objects: &[u8]) -> Vec<u8> {
    let dictionary = writer::dictionary(&settings(kind, user));
    let handler = Handler::open(&dictionary, ID, &[user]).unwrap();
    let data = writer::encrypt_stream(&handler, (4, 0), objects, None, [4; 16]).unwrap();
    in_an_object_stream_under("", &data, &encryption(&dictionary))
}

/// A trailer's `/Encrypt`, `dictionary`, and its `/ID`.
fn encryption(dictionary: &Dictionary) -> String {
    let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let d = dictionary;
    let mut entries = format!(
        "/Filter /Standard /V {} /R {} /P {}",
        d.v,
        d.r,
        d.p.unwrap()
    );
    if let Some(length) = d.length {
        entries.push_str(&format!(" /Length {length}"));
    }
    let strings = [
        ("O", &d.o),
        ("U", &d.u),
        ("OE", &d.oe),
        ("UE", &d.ue),
        ("Perms", &d.perms),
    ];
    for (name, value) in strings {
        if let Some(value) = value {
            entries.push_str(&format!(" /{name} <{}>", hex(value)));
        }
    }
    if let Some(encrypt_metadata) = d.encrypt_metadata {
        entries.push_str(&format!(" /EncryptMetadata {encrypt_metadata}"));
    }
    if let Some(filter) = d.crypt_filters.first() {
        let method = String::from_utf8_lossy(filter.method.as_deref().unwrap());
        let length = if method == "AESV3" { 32 } else { 16 };
        entries.push_str(&format!(
            " /CF << /StdCF << /CFM /{method} /Length {length} >> >> /StmF /StdCF /StrF /StdCF"
        ));
    }
    format!(" /Encrypt << {entries} >> /ID [<{0}> <{0}>]", hex(ID))
}

#[test]
fn an_encrypted_file_is_read_by_the_password_it_needs_and_unread_without_it() {
    let locked = written("password-locked.pdf", &crypted(Kind::Aes256, b"user"));
    let locked = locked.to_str().unwrap();
    let open = written("password-open.pdf", &crypted(Kind::Aes128, b""));
    let open = open.to_str().unwrap();
    let said = |args: &[&str]| {
        let out = run(args);
        (out.status.code(), text(&out.stdout), text(&out.stderr))
    };
    let (code, _, err) = said(&[locked]);
    assert_eq!(code, Some(2), "{err}");
    assert_eq!(
        err,
        format!("{locked}: it needs a password: give it with --password\n")
    );
    let (code, _, err) = said(&["--password", "wrong", locked]);
    assert_eq!(code, Some(2), "{err}");
    assert!(
        err.contains("the password --password gives doesn't open it"),
        "{err}"
    );
    // The user's or the owner's opens it, and what its object stream holds
    // is there, nothing unresolved: checked, its findings on revision 6's
    // values in a PDF 1.7 file.
    for password in ["user", "owner"] {
        let (code, out, err) = said(&["--password", password, locked]);
        assert_eq!((code, err.as_str()), (Some(1), ""), "{password}: {out}");
        assert!(out.contains("EncryptionStandard/R"), "{out}");
        assert!(!out.contains("unresolved"), "{out}");
    }
    // One with no user password opens without one, but not with a wrong
    // one, as the empty password isn't tried once one's given.
    let (code, out, _) = said(&[open]);
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(said(&["--password", "wrong", open]).0, Some(2));
    // A baseline that won't open is refused; given its password, it reads.
    let (code, _, err) = said(&["--baseline", locked, open]);
    assert_eq!(code, Some(2), "{err}");
    assert!(
        err.contains("--baseline ") && err.contains("needs a password"),
        "{err}"
    );
    let (code, out, _) = said(&["--password", "user", "--baseline", locked, locked]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("0 findings, 4 carried"), "{out}");
    // The JSON report says why, as for any file that isn't read.
    let out = run(&["--format", "json", locked]);
    let line = json::parse(&out.stdout).unwrap();
    assert_eq!(
        line.get("error").cloned(),
        Some(json::Json::String(
            "it needs a password: give it with --password".to_string()
        ))
    );
}

#[test]
fn a_password_is_tried_as_utf_8_and_in_each_single_byte_encoding() {
    // café, as each kind's writer may have set it: in PDFDocEncoding
    // (WinAnsiEncoding's alike), in MacRomanEncoding, and in UTF-8.
    let kinds = [
        (Kind::Rc4By128, &b"caf\xE9"[..]),
        (Kind::Aes128, b"caf\x8E"),
        (Kind::Aes256, "café".as_bytes()),
    ];
    for (kind, user) in kinds {
        let path = written("password-cafe.pdf", &crypted(kind, user));
        let out = run(&["--password", "café", path.to_str().unwrap()]);
        assert_ne!(
            out.status.code(),
            Some(2),
            "{kind:?}: {}",
            text(&out.stderr)
        );
        let out = run(&["--password", "cafe", path.to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(2), "{kind:?}");
    }
}

#[test]
fn an_encrypted_files_last_update_is_read_with_its_password() {
    // The page, in the object stream, has a /Rotate that isn't a number,
    // which the update, rewriting it as it was, carries: so the revision
    // before it must open too.
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> /Rotate /B >>";
    let base = crypted_with(Kind::Aes128, b"user", &object_stream_with(page).0);
    let trailer = encryption(&writer::dictionary(&settings(Kind::Aes128, b"user")));
    let update = updated_under(&base, &[(3, page)], &trailer);
    let path = written("updated-encrypted.pdf", &update);
    let path = path.to_str().unwrap();
    let out = run(&["--last-update", "--password", "user", path]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("0 findings, 1 carried"), "{stdout}");
    // A new finding is the update's.
    let rotated = page.replace("/Rotate /B", "/Rotate /C /UserUnit /D");
    let update = updated_under(&base, &[(3, rotated.as_str())], &trailer);
    let rotated = written("updated-encrypted-2.pdf", &update);
    let rotated = rotated.to_str().unwrap();
    let out = run(&["--last-update", "--password", "user", rotated]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("PageObject/UserUnit"), "{stdout}");
    let out = run(&["--last-update", path]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("it needs a password"));
}

/// What [`in_an_object_stream`]'s object stream holds, a catalog, page
/// tree and page after their header, and where the first object begins.
fn object_stream_objects() -> (Vec<u8>, usize) {
    object_stream_with("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>")
}

/// [`object_stream_objects`], the page `page`: the first object begins
/// where it does, the page last.
fn object_stream_with(page: &str) -> (Vec<u8>, usize) {
    let (mut header, mut body) = (String::new(), String::new());
    for (n, object) in [CATALOG, PAGES, page].iter().enumerate() {
        header.push_str(&format!("{} {} ", n + 1, body.len()));
        body.push_str(object);
        body.push('\n');
    }
    let first = header.len();
    (format!("{header}{body}").into_bytes(), first)
}

/// A file whose objects are in object stream 4, its data `data` under
/// `filter`, with a cross-reference stream.
fn in_an_object_stream(filter: &str, data: &[u8]) -> Vec<u8> {
    in_an_object_stream_under(filter, data, "")
}

/// [`in_an_object_stream`]'s file, its trailer given `trailer`'s entries.
fn in_an_object_stream_under(filter: &str, data: &[u8], trailer: &str) -> Vec<u8> {
    let (_, first) = object_stream_objects();
    let mut out = b"%PDF-1.7\n".to_vec();
    let four = out.len();
    let dict = format!(
        "<< /Type /ObjStm /N 3 /First {first}{filter} /Length {} >>",
        data.len()
    );
    out.extend_from_slice(format!("4 0 obj\n{dict}\nstream\n").as_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let five = out.len();
    let row = |kind: u8, field: usize, index: u8| {
        let [high, low] = (field as u16).to_be_bytes();
        [kind, high, low, index]
    };
    let rows = [
        row(0, 0, 255),
        row(2, 4, 0),
        row(2, 4, 1),
        row(2, 4, 2),
        row(1, four, 0),
        row(1, five, 0),
    ]
    .concat();
    let dict = format!(
        "<< /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R{trailer} /Length {} >>",
        rows.len()
    );
    out.extend_from_slice(format!("5 0 obj\n{dict}\nstream\n").as_bytes());
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn a_fault_in_a_streams_data_is_a_finding() {
    let (objects, _) = object_stream_objects();
    let hex: String = objects.iter().map(|b| format!("{b:02x}")).collect();
    let clean = written(
        "hex-clean.pdf",
        &in_an_object_stream(" /Filter /AHx", format!("{hex}>").as_bytes()),
    );
    let out = run(&[clean.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    // Without its `>`, which qpdf reads without a word: everything reads,
    // and the fault is the reader's finding, at the data.
    let faulted = written(
        "hex-no-end.pdf",
        &in_an_object_stream(" /Filter /AHx", hex.as_bytes()),
    );
    let out = run(&[faulted.to_str().unwrap()]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("- reader: -/- at -, in 4 0 R (merged): its data doesn't decode cleanly through /ASCIIHexDecode: no end-of-data marker"),
        "{stdout}"
    );
    assert!(stdout.contains("objects checked, 1 findings"), "{stdout}");
}

#[test]
fn a_file_of_dash_is_standard_input() {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> /Rotate (abc) >>";
    let plain = written("stdin-plain.pdf", &file(&[CATALOG, PAGES, page]));
    let plain = plain.to_str().unwrap();
    let (objects, _) = object_stream_objects();
    let packed = written("stdin-packed.pdf", &in_an_object_stream("", &objects));
    let packed = packed.to_str().unwrap().to_string();
    let encrypted = written("stdin-encrypted.pdf", &crypted(Kind::Aes256, b""));
    let encrypted = encrypted.to_str().unwrap().to_string();
    let piped = |path: &str, args: &[&str]| {
        let child = pdf_lint()
            .args(args)
            .stdin(std::fs::File::open(path).unwrap())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.wait_with_output().unwrap()
    };
    // In plain view, in object streams, and encrypted: each checked as by
    // its path, and named -.
    for path in [plain, &packed, &encrypted] {
        let by_path = run(&[path]);
        assert!(text(&by_path.stdout).contains(" objects checked"), "{path}");
        let out = piped(path, &["-"]);
        assert_eq!(
            out.status.code(),
            by_path.status.code(),
            "{}",
            text(&out.stderr)
        );
        assert_eq!(
            text(&out.stdout),
            text(&by_path.stdout).replacen(path, "-", 1)
        );
    }
    // Among files, in its place.
    let out = piped(&packed, &[plain, "-", plain]);
    let named: Vec<String> = (text(&out.stdout).lines())
        .filter_map(|line| line.split_once(": PDF ").map(|(name, _)| name.to_string()))
        .collect();
    assert_eq!(named, [plain, "-", plain]);
    // Once.
    let out = piped(plain, &["-", "-"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        text(&out.stderr),
        "pdf-lint: - more than once: standard input is read once\n"
    );
}

#[test]
fn one_table_checks_a_field_merged_with_its_widget_as_the_widget_alone() {
    let catalog = "<< /Type /Catalog /Pages 2 0 R \
                   /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) >> >>";
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> \
                /Annots [4 0 R] >>";
    let field = "<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] /P 3 0 R \
                 /FT /Tx /T (name) /MaxLen 5 >>";
    let path = written("one-table.pdf", &file(&[catalog, PAGES, page, field]));
    let path = path.to_str().unwrap();
    let out = run(&["--built", path]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    let out = run(&["--built", "--one-table", path]);
    let stdout = text(&out.stdout);
    let unlisted: Vec<&str> = (stdout.lines())
        .filter_map(|l| l.strip_prefix("- unlisted: "))
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    assert_eq!(
        unlisted,
        ["AnnotWidget/FT", "AnnotWidget/MaxLen"],
        "{stdout}"
    );
    assert_eq!(out.status.code(), Some(1));

    for (args, one_table, two_tables) in [
        (&["--format", "json", path][..], "false", "1"),
        (&["--format", "json", "--one-table", path][..], "true", "0"),
    ] {
        let out = run(args);
        let report = json::parse(&out.stdout).unwrap();
        let field = |key: &str| report.get(key).cloned().unwrap();
        assert_eq!(
            field("one_table"),
            json::parse(one_table.as_bytes()).unwrap()
        );
        assert_eq!(
            field("two_tables"),
            json::parse(two_tables.as_bytes()).unwrap()
        );
    }
}

#[test]
fn pdf_version_checks_a_file_as_another_version() {
    // /OutputIntents came in on a page with 2.0, and /ProcSet went.
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] \
                /Resources << /ProcSet [/PDF] >> /OutputIntents [] >>";
    let original = file(&[CATALOG, PAGES, page]);
    let path = written("version.pdf", &original);
    let path = path.to_str().unwrap();
    let found = |args: &[&str]| {
        let out = run(args);
        let stdout = text(&out.stdout);
        let summary = stdout.lines().next().unwrap_or_default().to_string();
        let found: Vec<String> = (stdout.lines())
            .filter_map(|l| l.strip_prefix("- "))
            .map(|l| l.split(" at ").next().unwrap().to_string())
            .collect();
        (summary, found)
    };
    let (summary, found_as_written) = found(&[path]);
    assert!(summary.contains(": PDF 1.7, 8 objects"), "{summary}");
    assert_eq!(found_as_written, ["too-new: PageObject/OutputIntents"]);
    // Named as its own version, it's as written.
    assert_eq!(found(&["--pdf-version", "1.7", path]).0, summary);
    // As 2.0, which wants a trailer's /ID too, and deprecates /ProcSet,
    // which is counted, and found when asked.
    let (summary, as_2) = found(&["--pdf-version", "2.0", path]);
    assert!(
        summary.contains(": PDF 1.7, checked as 2.0, 8 objects"),
        "{summary}"
    );
    assert_eq!(as_2, ["missing: FileTrailer/ID"]);
    let v2 = ["--pdf-version", "2.0", "--deprecations"];
    let (_, as_2) = found(&[&v2[..], &[path]].concat());
    assert_eq!(
        as_2,
        [
            "missing: FileTrailer/ID",
            "deprecated: Resource/ProcSet",
            "deprecated: ArrayOfNamesForProcSet/0"
        ]
    );

    // What it's set against is checked as 2.0 too: a baseline, and the
    // revision an update follows; and with deprecations found.
    let (summary, _) = found(&[&v2[..], &["--baseline", path, path]].concat());
    assert!(summary.ends_with("0 findings, 3 carried"), "{summary}");
    let update = updated(&original, &[(3, page)]);
    let update = written("version-update.pdf", &update);
    let update = update.to_str().unwrap();
    let (summary, _) = found(&[&v2[..], &["--last-update", update]].concat());
    assert!(summary.ends_with("0 findings, 3 carried"), "{summary}");
    // An allowance's version is the one the file is checked as.
    let allow = written(
        "version-allow.tsv",
        b"# test\nResource\tProcSet\tdeprecated\t*\t2.0\tthe test's\n",
    );
    let allow = allow.to_str().unwrap();
    let (summary, _) = found(&[&v2[..], &["--allow", allow, path]].concat());
    assert!(summary.ends_with("2 findings, 1 allowed"), "{summary}");

    let out = run(&["--pdf-version", "2.0", "--format", "json", path]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains(r#""version":"1.7","checked_as":"2.0","#),
        "{stdout}"
    );
    let report = json::parse(&out.stdout).unwrap();
    assert_eq!(
        report.get("deprecations").cloned(),
        Some(json::Json::Bool(false))
    );
    assert_eq!(
        report.get("deprecated").cloned(),
        Some(
            json::parse(
                br#"[{"table":"ArrayOfNamesForProcSet","key":"0","count":1},{"table":"Resource","key":"ProcSet","count":1}]"#
            )
            .unwrap()
        )
    );
    for v in ["1.8", "2", "two"] {
        let out = run(&["--pdf-version", v, path]);
        assert_eq!(out.status.code(), Some(2));
        let why = format!("--pdf-version {v}: not a PDF version, 1.0 to 1.7 or 2.0");
        assert!(text(&out.stderr).contains(&why), "{}", text(&out.stderr));
    }
}

#[test]
fn model_checks_against_another_checkout() {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> \
                /OutputIntents [] >>";
    let path = written("model.pdf", &file(&[CATALOG, PAGES, page]));
    let path = path.to_str().unwrap();
    // A copy of the embedded model, where /OutputIntents came in on a
    // page with 1.4, not 2.0; and one without the trailer's table.
    let embedded =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pdf-arlington/model/tsv/latest");
    let copy = |name: &str, change: &dyn Fn(&str, String) -> Option<String>| {
        let dir = std::env::temp_dir().join(format!("pdf-lint-cli-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for entry in std::fs::read_dir(&embedded).unwrap() {
            let from = entry.unwrap().path();
            let file = from.file_name().unwrap().to_str().unwrap().to_string();
            let text = std::fs::read_to_string(&from).unwrap();
            if let Some(text) = change(&file, text) {
                std::fs::write(dir.join(&file), text).unwrap();
            }
        }
        dir
    };
    let earlier = copy("model-earlier", &|file, text| {
        Some(match file {
            // Key, Type, SinceVersion, ...
            "PageObject.tsv" => (text.lines())
                .map(|line| {
                    let mut cells: Vec<&str> = line.split('\t').collect();
                    if cells[0] == "OutputIntents" {
                        assert_eq!(cells[2], "2.0");
                        cells[2] = "1.4";
                    }
                    cells.join("\t") + "\n"
                })
                .collect(),
            _ => text,
        })
    });
    let out = run(&[path]);
    assert!(
        text(&out.stdout).contains("too-new: PageObject/OutputIntents"),
        "{}",
        text(&out.stdout)
    );
    let out = run(&["--model", earlier.to_str().unwrap(), path]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    // The JSON report says which, by the tables' hash: upstream's for a
    // copy of its files, the embedded one's another, as it has cells
    // corrected, and the earlier one's another again.
    let model = |args: &[&str], path: &str| {
        let args = [args, &["--format", "json", path]].concat();
        let report = json::parse(text(&run(&args).stdout).trim().as_bytes()).unwrap();
        report.get("model").cloned().unwrap()
    };
    let upstream = copy("model-upstream", &|_, text| Some(text));
    let upstream = upstream.to_str().unwrap();
    let embedded = model(&[], path);
    let hash = |h: &str| json::Json::String(h.to_string());
    assert_eq!(embedded, hash("fc22d1fadc1fd269"));
    assert_eq!(
        model(&["--model", upstream], path),
        hash("c2bdcab22f823d8d")
    );
    assert_ne!(
        model(&["--model", earlier.to_str().unwrap()], path),
        embedded
    );

    // A checkbox without /DA, on it or its form: upstream requires it of
    // a button, and the embedded model, as ISO 32000-2 does, doesn't.
    let catalog = "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>";
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> \
                /Annots [4 0 R] >>";
    let checkbox = "<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] /P 3 0 R \
                    /FT /Btn /T (box) >>";
    let form = written("model-form.pdf", &file(&[catalog, PAGES, page, checkbox]));
    let form = form.to_str().unwrap();
    let out = run(&[form]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    let out = run(&["--model", upstream, form]);
    assert!(
        text(&out.stdout).contains("missing: FieldBtnCheckbox/DA"),
        "{}",
        text(&out.stdout)
    );

    // A folder that isn't one, or a model without a table the checker
    // reads by name, is refused.
    let out = run(&["--model", "/nonexistent/tsv", path]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--model /nonexistent/tsv"));
    let trailerless = copy("model-trailerless", &|file, text| {
        (file != "FileTrailer.tsv").then_some(text)
    });
    let out = run(&["--model", trailerless.to_str().unwrap(), path]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("it has no FileTrailer table"),
        "{}",
        text(&out.stderr)
    );
}

/// A model that doesn't load, or that lacks a table the checker reads by
/// name, is said word for word after `--model` and its folder.
#[test]
fn a_model_refused_is_said_word_for_word() {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>";
    let path = written("model-refused.pdf", &file(&[CATALOG, PAGES, page]));
    let path = path.to_str().unwrap();
    let refused = |dir: &str| {
        let out = run(&["--model", dir, path]);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out.stdout));
        text(&out.stderr)
    };
    let missing = "/nonexistent/tsv";
    let why = std::fs::read_dir(missing).unwrap_err();
    assert_eq!(
        refused(missing),
        format!("pdf-lint: --model {missing}: reading {missing}: {why}\n")
    );
    // A folder of one table, `T.tsv`, of this row.
    let one = |name: &str, row: &str| {
        let dir = std::env::temp_dir().join(format!("pdf-lint-cli-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let header = pdf_lint::grammar::HEADER.join("\t");
        std::fs::write(dir.join("T.tsv"), format!("{header}\n{row}\n")).unwrap();
        dir
    };
    let broken = one("model-broken", "Rotate\tinteger");
    let table = broken.join("T.tsv");
    let broken = broken.to_str().unwrap();
    assert_eq!(
        refused(broken),
        format!(
            "pdf-lint: --model {broken}: {}: line 2: 2 fields, not 12\n",
            table.display()
        )
    );
    let row = "Rotate\tinteger\t1.0\t\tFALSE\tFALSE\tTRUE\t0\t\t\t\t";
    let lacking = one("model-lacking", row);
    let lacking = lacking.to_str().unwrap();
    assert_eq!(
        refused(lacking),
        format!(
            "pdf-lint: --model {lacking}: it has no FileTrailer table, which the checker reads by name\n"
        )
    );
}

/// A hold, exempt or allowances file that can't be read, or that has a
/// line that doesn't read, is said word for word: what std said reading
/// it, or the line, after its path.
#[test]
fn a_hold_exempt_or_allowances_file_refused_is_said_word_for_word() {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>";
    let path = written("files-refused.pdf", &file(&[CATALOG, PAGES, page]));
    let missing = path.with_file_name("files-refused-missing.txt");
    let why = std::fs::read_to_string(&missing).unwrap_err();
    let missing = missing.to_str().unwrap();
    let path = path.to_str().unwrap();
    let refused = |args: &[&str]| {
        let out = run(&[args, &[path]].concat());
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stdout)
        );
        text(&out.stderr)
    };
    assert_eq!(
        refused(&["--hold", missing]),
        format!("pdf-lint: reading {missing}: {why}\n")
    );
    assert_eq!(
        refused(&["--built", "--exempt", missing]),
        format!("pdf-lint: reading {missing}: {why}\n")
    );
    assert_eq!(
        refused(&["--allow", missing]),
        format!("pdf-lint: reading {missing}: {why}\n")
    );

    let hold = written("files-refused-hold.txt", b"# held\n4 x built\n");
    let hold = hold.to_str().unwrap();
    let generation = "x".parse::<u16>().unwrap_err();
    assert_eq!(
        refused(&["--hold", hold]),
        format!("pdf-lint: {hold}: line 2: \"x\" isn't a generation: {generation}\n")
    );
    let exempt = written("files-refused-exempt.txt", b"/Writer\nMine\n");
    let exempt = exempt.to_str().unwrap();
    assert_eq!(
        refused(&["--built", "--exempt", exempt]),
        format!("pdf-lint: {exempt}: line 2: \"Mine\" isn't a name, as /MyKey, or /My*\n")
    );
    let allow = written(
        "files-refused-allow.tsv",
        b"# Held.\nFieldTx\tDA\tmissing\t*\t*\t\n",
    );
    let allow = allow.to_str().unwrap();
    assert_eq!(
        refused(&["--allow", allow]),
        format!("pdf-lint: {allow}: line 2: no reason\n")
    );
}

#[test]
fn a_file_that_cant_be_read_or_a_wrong_command_line_exits_2() {
    let out = run(&["/nonexistent/file.pdf"]);
    assert_eq!(out.status.code(), Some(2));
    let out = run(&["--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--bogus: no such option"));
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2));
    let out = run(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains(pdf_lint::COMMIT));
}

#[test]
fn files_checked_at_once_are_reported_in_order() {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>";
    let faulty = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1] /Resources << >> >>";
    // The first file takes the longest, so it's checked last of all.
    let filler = format!("[{}]", ["1"; 50].join(" "));
    let mut big = vec![CATALOG, PAGES, page];
    big.extend(std::iter::repeat_n(filler.as_str(), 2000));
    let mut paths = vec![written("order-big.pdf", &file(&big))];
    for i in 0..8 {
        let third = if i % 2 == 0 { page } else { faulty };
        paths.push(written(
            &format!("order-{i}.pdf"),
            &file(&[CATALOG, PAGES, third]),
        ));
    }
    let mut args: Vec<&str> = paths.iter().map(|p| p.to_str().unwrap()).collect();
    args.insert(5, "/nonexistent/file.pdf");

    let one = run(&[&["--jobs", "1"], &args[..]].concat());
    let many = run(&[&["--jobs", "4"], &args[..]].concat());
    assert_eq!(text(&many.stdout), text(&one.stdout));
    assert_eq!(text(&many.stderr), text(&one.stderr));
    let summaries: Vec<_> = (text(&many.stdout).lines())
        .filter(|l| l.contains(": PDF "))
        .map(|l| l.split(": PDF ").next().unwrap().to_string())
        .collect();
    let given: Vec<_> = (args.iter())
        .filter(|a| !a.starts_with("/nonexistent"))
        .map(|a| a.to_string())
        .collect();
    assert_eq!(summaries, given);
    assert!(
        text(&many.stderr).contains("/nonexistent/file.pdf"),
        "{}",
        text(&many.stderr)
    );
    // A file that can't be read doesn't stop the others, and wins the
    // exit code.
    assert_eq!(many.status.code(), Some(2));
    let readable: Vec<_> = (args.iter())
        .filter(|a| !a.starts_with("/nonexistent"))
        .copied()
        .collect();
    let out = run(&[&["--jobs", "3"], &readable[..]].concat());
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));

    for bad in ["0", "x", "-1"] {
        let out = run(&["--jobs", bad, args[1]]);
        assert_eq!(out.status.code(), Some(2), "--jobs {bad}");
        assert!(
            text(&out.stderr).contains("--jobs"),
            "{}",
            text(&out.stderr)
        );
    }
}

#[test]
fn a_report_no_one_reads_stops_without_a_panic() {
    let filler = format!("[{}]", ["1"; 50].join(" "));
    let mut big = vec![CATALOG, PAGES];
    big.extend(std::iter::repeat_n(filler.as_str(), 2000));
    let path = written("unread.pdf", &file(&big));
    let path = path.to_str().unwrap();
    // Far more than a pipe holds, with the pipe's reader gone.
    let mut child = pdf_lint()
        .args(["--jobs", "2"])
        .args([path; 20])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let stderr = text(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_ne!(out.status.code(), Some(101), "{stderr}");
}

#[test]
fn the_version_or_usage_no_one_reads_exits_0_without_a_panic() {
    for flag in ["--version", "--help"] {
        // The pipe's reader gone before a line is written, as with `head`
        // given a wrong option.
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let out = pdf_lint().arg(flag).stdout(writer).output().unwrap();
        let stderr = text(&out.stderr);
        assert!(!stderr.contains("panicked"), "{flag}: {stderr}");
        assert_eq!(out.status.code(), Some(0), "{flag}: {stderr}");
    }
}

#[test]
fn json_reports_a_line_for_each_file() {
    let faulty = file(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /MediaBox (q\"\\\\\t) /Resources << >> /Annots [4 0 R] >>",
        // A link's /Border, which readers ignore beside a /BS.
        "<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] /Border [0 0 1] /BS << >> >>",
    ]);
    let path = written("json.pdf", &faulty);
    let out = run(&[
        "--format",
        "json",
        path.to_str().unwrap(),
        "/nonexistent/file.pdf",
    ]);
    assert_eq!(out.status.code(), Some(2));
    let stdout = text(&out.stdout);
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    let report = json::parse(lines[0].as_bytes()).unwrap_or_else(|e| panic!("{e}: {}", lines[0]));
    let field = |j: &json::Json, key: &str| j.get(key).cloned().unwrap_or_else(|| panic!("{key}"));
    assert_eq!(
        field(&report, "file"),
        json::Json::String(path.display().to_string())
    );
    assert_eq!(
        field(&report, "version"),
        json::Json::String("1.7".to_string())
    );
    let model = format!("{:016x}", pdf_lint::grammar().hash());
    assert_eq!(field(&report, "model"), json::Json::String(model));
    assert_eq!(
        field(&report, "ignored"),
        json::parse(br#"[{"table":"AnnotLink","key":"Border","count":1}]"#).unwrap()
    );
    let finding = field(&report, "findings").at(0).cloned().unwrap();
    assert_eq!(
        field(&finding, "rule"),
        json::Json::String("type".to_string())
    );
    assert_eq!(
        field(&finding, "key"),
        json::Json::String("MediaBox".to_string())
    );
    assert_eq!(field(&finding, "object"), json::parse(b"[3,0]").unwrap());
    let value = field(&finding, "value");
    assert!(
        value
            .as_str()
            .is_some_and(|v| v.contains('"') && v.contains('\\') && v.contains('\t')),
        "{value:?}"
    );
    let at = field(&finding, "at");
    let number = |key: &str| match field(&at, key) {
        json::Json::Number(n) => n.parse::<usize>().unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(field(&at, "stream"), json::Json::Null);
    assert_eq!(
        &faulty[number("start")..number("end")],
        b"/MediaBox (q\"\\\\\t)"
    );
    assert!(matches!(field(&report, "tables"), json::Json::Object(_)));

    let unread = json::parse(lines[1].as_bytes()).unwrap();
    assert_eq!(
        field(&unread, "file"),
        json::Json::String("/nonexistent/file.pdf".to_string())
    );
    // The extensions it was checked under: none it declares; and those of
    // a catalog that declares Adobe's level 8 and ETSI's.
    assert_eq!(field(&report, "extensions"), json::parse(b"[]").unwrap());
    let catalog = "<< /Type /Catalog /Pages 2 0 R /Extensions << \
                   /ADBE << /BaseVersion /1.7 /ExtensionLevel 8 >> \
                   /ESIC << /BaseVersion /1.7 /ExtensionLevel 1 >> >> >>";
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>";
    let declaring = written("json-extensions.pdf", &file(&[catalog, PAGES, page]));
    let out = run(&["--format", "json", declaring.to_str().unwrap()]);
    let declared = json::parse(&out.stdout).unwrap();
    assert_eq!(
        field(&declared, "extensions"),
        json::parse(br#"["ADBE_Extn3","ETSI_PAdES"]"#).unwrap()
    );
    // Each line says which pdf-lint wrote it.
    let version = json::Json::String(env!("CARGO_PKG_VERSION").to_string());
    assert_eq!(field(&report, "pdf_lint"), version);
    assert_eq!(field(&unread, "pdf_lint"), version);
    assert_eq!(field(&unread, "model"), field(&report, "model"));
    assert!(field(&unread, "error").as_str().is_some());

    let out = run(&["--format", "yaml", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
}

/// `file` with an incremental update appended: these objects' bodies, by
/// number, and a table listing them.
fn updated(file: &[u8], objects: &[(u32, &str)]) -> Vec<u8> {
    updated_under(file, objects, "")
}

/// [`updated`]'s file, the update's trailer given `trailer`'s entries.
fn updated_under(file: &[u8], objects: &[(u32, &str)], trailer: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(file);
    let previous = text.rsplit("startxref").next().unwrap();
    let previous: usize = previous.split_whitespace().next().unwrap().parse().unwrap();
    let mut out = file.to_vec();
    let mut rows = String::from("xref\n0 1\n0000000000 65535 f \n");
    for (n, body) in objects {
        rows.push_str(&format!("{n} 1\n{:010} 00000 n \n", out.len()));
        out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let size = objects.iter().map(|(n, _)| n + 1).max().unwrap();
    let xref = out.len();
    out.extend_from_slice(rows.as_bytes());
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {size} /Root 1 0 R{trailer} /Prev {previous} >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

#[test]
fn the_last_update_answers_for_what_it_wrote() {
    // A, in the page as first written.
    let original = file(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1] /Resources << >> >>",
    ]);
    // The update rewrites the page, keeping A and adding B, and adds an
    // annotation with C.
    let update = updated(
        &original,
        &[
            (
                3,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1] /Rotate (B) /Resources << >> /Annots [4 0 R] >>",
            ),
            (4, "<< /Type /Annot /Subtype /Text /Rect (C) >>"),
        ],
    );
    let path = written("updated.pdf", &update);
    let out = run(&["--last-update", path.to_str().unwrap()]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let findings: Vec<_> = stdout.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(findings.len(), 2, "{stdout}");
    assert!(
        findings[0].contains("PageObject/Rotate") && findings[0].contains("(merged)"),
        "{stdout}"
    );
    assert!(
        findings[1].contains("AnnotText/Rect") && findings[1].contains("(built)"),
        "{stdout}"
    );
    assert!(stdout.contains("1 carried"), "{stdout}");

    // A file of one revision has no update to answer for.
    let path = written("one-revision.pdf", &original);
    let out = run(&["--last-update", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("no incremental update"),
        "{}",
        text(&out.stderr)
    );
    let out = run(&["--last-update", "--built", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_baseline_leaves_out_what_the_file_was_written_from_already_had() {
    let faulty = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1] /Resources << >> >>";
    let clean = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>";
    // A baseline whose first page has a fault, A, and its second none.
    let baseline = file(&[
        CATALOG,
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        faulty,
        clean,
    ]);
    let base = written("baseline.pdf", &baseline);

    // An incremental update of it is set against it object by object: it
    // puts the first page second, where it keeps A, and adds B; and a new
    // object, C.
    let update = updated(
        &baseline,
        &[
            (2, "<< /Type /Pages /Kids [4 0 R 3 0 R] /Count 2 >>"),
            (
                3,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1] /Rotate (B) /Resources << >> /Annots [5 0 R] >>",
            ),
            (5, "<< /Type /Annot /Subtype /Text /Rect (C) >>"),
        ],
    );
    let path = written("baseline-updated.pdf", &update);
    let out = run(&["--baseline", base.to_str().unwrap(), path.to_str().unwrap()]);
    let stdout = text(&out.stdout);
    let findings: Vec<_> = stdout.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(findings.len(), 2, "{stdout}");
    assert!(findings[0].contains("PageObject/Rotate"), "{stdout}");
    assert!(findings[1].contains("AnnotText/Rect"), "{stdout}");
    assert!(stdout.contains("1 carried"), "{stdout}");

    // A rewrite, renumbered, with a page added before them, is set against
    // it by where each finding is: Kids[0] had A; Kids[1] didn't, though
    // another page did; and the baseline has no Kids[2], so A anywhere
    // carries it.
    let rewrite = file(&[
        CATALOG,
        "<< /Type /Pages /Kids [5 0 R 4 0 R 3 0 R] /Count 3 >>",
        faulty,
        faulty,
        faulty,
    ]);
    let path = written("baseline-rewritten.pdf", &rewrite);
    let out = run(&["--baseline", base.to_str().unwrap(), path.to_str().unwrap()]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let findings: Vec<_> = stdout.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(findings.len(), 1, "{stdout}");
    assert!(
        findings[0].contains("at Root/Pages/Kids[1], in 4 0 R"),
        "{stdout}"
    );
    assert!(stdout.contains("2 carried"), "{stdout}");

    let out = run(&[
        "--baseline",
        "/nonexistent/file.pdf",
        path.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    let out = run(&[
        "--baseline",
        base.to_str().unwrap(),
        "--last-update",
        path.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
}
