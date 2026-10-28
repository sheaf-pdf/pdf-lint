//! The reader's messages that no other test holds whole, each word for
//! word, from a file made to give it: why an object doesn't read, why a
//! file's sections don't, and what it warns of.

use super::*;
use crate::source::Source;
use crate::write;

/// Why object `id` of `doc` doesn't read, as it says.
fn why(doc: &Document, id: ObjectId) -> String {
    doc.get(id).unwrap_err().to_string()
}

/// Each warning's message.
fn said(doc: &Document) -> Vec<&str> {
    doc.warnings.iter().map(|w| w.message.as_str()).collect()
}

/// Objects 2 and 3 as an object stream holds them, from `/First` 8.
const MEMBERS: &[u8] = b"2 0 3 6 << >> []";

/// An object stream's body: `dict`'s entries and `/Length`, and `data`.
fn container(dict: &str, data: &[u8]) -> Vec<u8> {
    let head = format!("<< {dict} /Length {} >>\nstream\n", data.len());
    [head.as_bytes(), data, b"\nendstream"].concat()
}

/// A file whose objects 2 and 3 are in object stream 4, written as
/// `body`, listed by cross-reference stream 5 with `trailer`'s entries
/// besides its own.
fn packed(body: &[u8], trailer: &str) -> Vec<u8> {
    let mut file = b"%PDF-1.7\n".to_vec();
    let one = file.len();
    file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let four = file.len();
    file.extend_from_slice(b"4 0 obj\n");
    file.extend_from_slice(body);
    file.extend_from_slice(b"\nendobj\n");
    let five = file.len();
    let row = |kind: u8, field: usize, index: u8| {
        let [high, low] = (field as u16).to_be_bytes();
        [kind, high, low, index]
    };
    let rows = [
        row(0, 0, 255),
        row(1, one, 0),
        row(2, 4, 0),
        row(2, 4, 1),
        row(1, four, 0),
        row(1, five, 0),
    ]
    .concat();
    let dict = format!(
        "5 0 obj\n<< /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R{trailer} /Length {} >>\nstream\n",
        rows.len()
    );
    file.extend_from_slice(dict.as_bytes());
    file.extend_from_slice(&rows);
    file.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes());
    file
}

/// A file of a catalog and cross-reference stream 2, its dictionary's
/// entries `dict` besides `/Type`, `/Root` and `/Length`, and its data
/// `rows`.
fn xref_stream(dict: &str, rows: &[u8]) -> Vec<u8> {
    let mut file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
    let two = file.len();
    let head = format!(
        "2 0 obj\n<< /Type /XRef /Root 1 0 R {dict} /Length {} >>\nstream\n",
        rows.len()
    );
    file.extend_from_slice(head.as_bytes());
    file.extend_from_slice(rows);
    file.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{two}\n%%EOF\n").as_bytes());
    file
}

/// Its encryption dictionary and `/ID`, as trailer entries, for pdf-crypt's
/// writer's `kind` with user password `user`: AES-256's with `streams` and
/// `strings` the crypt filters of each; another kind's, version 4 with
/// AES-128 as `/StdCF` for embedded files alone.
fn encrypt(kind: pdf_crypt::writer::Kind, streams: &str, strings: &str) -> String {
    let settings = pdf_crypt::writer::Settings {
        kind,
        user: b"user",
        owner: b"owner",
        p: -4,
        id: b"0123456789abcdef",
        encrypt_metadata: true,
    };
    let values = pdf_crypt::writer::dictionary(&settings);
    let hex = |b: &Option<Vec<u8>>| -> String {
        (b.as_ref().unwrap().iter())
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let (o, u) = (hex(&values.o), hex(&values.u));
    let id = " /ID [(0123456789abcdef) (0123456789abcdef)]";
    match kind {
        pdf_crypt::writer::Kind::Aes256 => format!(
            " /Encrypt << /Filter /Standard /V 5 /R 6 /Length 256 /P -4 /O <{o}> /U <{u}> /OE <{}> /UE <{}> /Perms <{}> \
             /CF << /StdCF << /CFM /AESV3 >> >> /StmF /{streams} /StrF /{strings} >>{id}",
            hex(&values.oe),
            hex(&values.ue),
            hex(&values.perms)
        ),
        _ => format!(
            " /Encrypt << /Filter /Standard /V 4 /R 4 /Length 128 /P -4 /O <{o}> /U <{u}> \
             /CF << /StdCF << /CFM /AESV2 /AuthEvent /EFOpen >> >> /StmF /{streams} /StrF /{strings} /EFF /StdCF >>{id}"
        ),
    }
}

/// What decodes nothing: every stream it's asked for, left out.
struct Nothing;

impl Decode for Nothing {
    fn decode(&self, _: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
        BTreeMap::new()
    }
}

#[test]
fn an_object_stream_that_isnt_a_stream_says_so() {
    let file = packed(b"<< /Type /ObjStm /N 2 /First 8 >>", "");
    assert_eq!(
        why(&read(&file), (2, 0)),
        "its object stream 4 0 R doesn't decode: it isn't a stream"
    );
}

#[test]
fn an_object_streams_own_crypt_filter_in_a_file_no_password_opens_says_so() {
    // Strings and streams not encrypted, so the file reads without its
    // password, but for this stream, which its own crypt filter encrypts.
    let kind = pdf_crypt::writer::Kind::Aes128;
    let body = container(
        "/Type /ObjStm /N 2 /First 8 /Filter /Crypt /DecodeParms << /Name /StdCF >>",
        MEMBERS,
    );
    let doc = read(&packed(&body, &encrypt(kind, "Identity", "Identity")));
    assert_eq!(doc.encryption, Encryption::Plain);
    assert_eq!(
        why(&doc, (2, 0)),
        "its object stream 4 0 R doesn't decode: its own /Crypt filter, /StdCF, encrypts it, and no password the reader was given opens the file"
    );
}

#[test]
fn an_object_stream_whose_data_doesnt_decrypt_says_so_and_why() {
    // AES ciphertext shorter than its IV: nothing decrypts.
    let kind = pdf_crypt::writer::Kind::Aes256;
    let body = container("/Type /ObjStm /N 2 /First 8", b"ABCDEFGH");
    let doc = read_opened(
        &packed(&body, &encrypt(kind, "StdCF", "Identity")),
        &[b"user"],
    );
    assert_eq!(
        said(&doc),
        ["its data doesn't decrypt cleanly: its AES ciphertext has no whole block past its IV"]
    );
    assert_eq!(doc.warnings[0].object, Some((4, 0)));
    assert_eq!(
        why(&doc, (2, 0)),
        "its object stream 4 0 R doesn't decode: its data doesn't decrypt: its AES ciphertext has no whole block past its IV"
    );
}

#[test]
fn startxref_inside_another_word_says_where() {
    let file = write::assembled(
        "1.7",
        &[(1, b"<< /Type /Catalog >>")],
        "<< /Size 2 /Root 1 0 R >>",
    );
    let at = rfind(&file, b"startxref").unwrap();
    let file = [&file[..at], b"startxrefs", &file[at + 9..]].concat();
    let doc = read(&file);
    let why = format!("startxref at byte {at} is inside another word");
    assert_eq!(doc.trailer.unwritten, Some(why));
    assert!(doc.get((1, 0)).is_ok());
}

#[test]
fn more_sections_than_the_reader_reads_along_prev_says_how_many() {
    let mut file = b"%PDF-1.7\n".to_vec();
    let one = file.len();
    file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
    let mut prev = String::new();
    let mut xref = 0;
    for _ in 0..=SECTIONS {
        xref = file.len();
        let section = format!(
            "xref\n0 2\n0000000000 65535 f \n{one:010} 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R{prev} >>\n"
        );
        file.extend_from_slice(section.as_bytes());
        prev = format!(" /Prev {xref}");
    }
    file.extend_from_slice(format!("startxref\n{xref}\n%%EOF\n").as_bytes());
    let doc = read(&file);
    assert_eq!(said(&doc), ["more than 1024 cross-reference sections"]);
    assert_eq!(doc.trailer.unwritten, None);
}

#[test]
fn a_cross_reference_streams_w_index_and_size_say_whats_wrong() {
    let rows = [1, 0, 9, 0, 1, 0, 9, 0];
    let cases = [
        (
            "/Size 3 /W [1 2]",
            "its /W isn't three widths of 8 bytes or fewer",
        ),
        (
            "/Size 3 /W [1 2 1] /Index [0]",
            "its /Index isn't pairs of numbers",
        ),
        ("/W [1 2 1]", "it has no /Size"),
        (
            "/Size 3 /W [1 2 1] /Index [4294967295 2]",
            "its /Index numbers no object",
        ),
    ];
    for (dict, why) in cases {
        let doc = read(&xref_stream(dict, &rows));
        assert_eq!(doc.trailer.unwritten.as_deref(), Some(why), "{dict}");
    }
}

#[test]
fn a_cross_reference_stream_a_decoder_doesnt_decode_says_its_filters() {
    let rows = [0, 0, 0, 255, 1, 0, 9, 0, 1, 0, 9, 0];
    let zlib = write::zlib(&rows);
    for (filter, shown) in [
        ("/FlateDecode", "/FlateDecode"),
        ("[/FlateDecode]", "filters"),
    ] {
        let file = xref_stream(&format!("/Size 3 /W [1 2 1] /Filter {filter}"), &zlib);
        let doc = read_through(&file, &BTreeMap::new());
        let why = format!("its cross-reference stream ({shown}) doesn't decode: no data for 2 0 R");
        assert_eq!(doc.trailer.unwritten, Some(why), "{filter}");
        let doc = read_through(&file, &Nothing);
        let why =
            format!("its cross-reference stream is compressed ({shown}), and nothing decodes it");
        assert_eq!(doc.trailer.unwritten, Some(why), "{filter}");
        // Decoded by a decoder to data not its rows' length.
        let doc = read_through(&file, &BTreeMap::from([((2, 0), vec![0; 3])]));
        let why = "decoded, it's 3 bytes, not the 12 its /W and /Index make: the decoder may have decoded another object 2 0 R";
        assert_eq!(doc.trailer.unwritten.as_deref(), Some(why), "{filter}");
    }
}

#[test]
fn an_encrypt_that_refers_to_no_object_is_read_as_not_encrypted_and_said() {
    let file = write::assembled(
        "1.7",
        &[(1, b"<< /Type /Catalog >>")],
        "<< /Size 2 /Root 1 0 R /Encrypt 9 0 R >>",
    );
    let doc = read(&file);
    assert_eq!(
        said(&doc),
        [
            "the trailer's /Encrypt is 9 0 R, which is no object, so the file is read as not encrypted"
        ]
    );
    assert_eq!(doc.encryption, Encryption::None);
}

#[test]
fn an_object_stream_of_a_file_nothing_opens_says_nothing_decodes_it() {
    let body = container("/Type /ObjStm /N 2 /First 8", MEMBERS);
    let doc = read(&packed(&body, " /Encrypt << /Filter /Standard /V 2 >>"));
    assert_eq!(
        why(&doc, (2, 0)),
        "it's in object stream 4 0 R, and nothing decodes that"
    );
}

#[test]
fn an_object_that_runs_to_where_a_fault_cut_its_object_stream_off_says_so() {
    // Object 3 is 12345, cut off at 123.
    let zlib = write::zlib(b"2 0 3 6 << >>\n12345");
    let cut = &zlib[..zlib.len() - 6];
    let body = container("/Type /ObjStm /N 2 /First 8 /Filter /FlateDecode", cut);
    let doc = read(&packed(&body, ""));
    assert_eq!(
        why(&doc, (3, 0)),
        "it runs to where a fault cut object stream 4 0 R's data off, so it may be cut short"
    );
}

#[test]
fn an_object_streams_header_that_doesnt_read_says_why_for_its_objects() {
    let cases: [(&str, &[u8], &str); 3] = [
        ("/N 2", MEMBERS, "its /N or /First isn't a count"),
        (
            "/N 2 /First 9",
            b"-2 0 3 6 << >> []",
            "its header numbers no object",
        ),
        (
            "/N 2 /First 9",
            b"2 -1 3 6 << >> []",
            "its header gives an offset that isn't one",
        ),
    ];
    for (dict, data, said) in cases {
        let body = container(&format!("/Type /ObjStm {dict}"), data);
        let doc = read(&packed(&body, ""));
        assert_eq!(why(&doc, (2, 0)), said);
        assert_eq!(why(&doc, (3, 0)), said);
    }
}

#[test]
fn an_object_streams_filters_the_reader_doesnt_take_say_why() {
    let many = vec!["/AHx"; filter::FILTERS + 1].join(" ");
    let cases = [
        (
            format!("/Filter [{many}]"),
            "its /Filter lists 26 filters, more than 25",
        ),
        (
            "/Filter [/Crypt] /DecodeParms [1]".to_string(),
            "/Crypt's parameters aren't a dictionary",
        ),
    ];
    for (filter, said) in cases {
        let body = container(&format!("/Type /ObjStm /N 2 /First 8 {filter}"), MEMBERS);
        let doc = read(&packed(&body, ""));
        let said = format!("its object stream 4 0 R doesn't decode: {said}");
        assert_eq!(why(&doc, (2, 0)), said, "{filter}");
    }
}

#[test]
fn a_decoder_that_decodes_nothing_from_its_bytes_says_so() {
    let stream = Written {
        filter: b"/FlateDecode",
        parms: None,
        data: b"",
    };
    let none: BTreeMap<ObjectId, Vec<u8>> = BTreeMap::new();
    assert_eq!(
        none.decode_written(stream).unwrap_err(),
        "nothing decodes a stream from its bytes"
    );
}

/// Each reason an object stream doesn't decode, or its header doesn't
/// say where its objects are, says it as the reader's text for it did, by
/// the same format string.
#[test]
fn each_reason_an_object_stream_doesnt_read_is_said_as_it_was() {
    let name = b"StdCF";
    let fault = pdf_crypt::Fault::Padding;
    let unfiltered = Unfiltered::Filter;
    let (pairs, count) = (1, 2);
    let undecoded = [
        (Undecoded::NotAStream, "it isn't a stream".to_string()),
        (
            Undecoded::Filters(unfiltered.clone()),
            unfiltered.to_string(),
        ),
        (
            Undecoded::Encrypted {
                filter: name.to_vec(),
            },
            {
                let name = String::from_utf8_lossy(name);
                format!(
                    "its own /Crypt filter, /{name}, encrypts it, and no password the reader was given opens the file"
                )
            },
        ),
        (
            Undecoded::Decrypt(fault.clone()),
            format!("its data doesn't decrypt: {fault}"),
        ),
        (
            Undecoded::Other("no data for 4 0 R".to_string()),
            "no data for 4 0 R".to_string(),
        ),
    ];
    for (undecoded, said) in undecoded {
        assert_eq!(undecoded.to_string(), said, "{undecoded:?}");
    }
    let unlisted = [
        (Unlisted::NotAStream, "it isn't a stream".to_string()),
        (
            Unlisted::NotAnObjectStream,
            "it isn't /Type /ObjStm".to_string(),
        ),
        (
            Unlisted::Counts,
            "its /N or /First isn't a count".to_string(),
        ),
        (
            Unlisted::Pairs { pairs, count },
            format!("its header has {} of its {count} pairs", pairs),
        ),
        (Unlisted::Number, "its header numbers no object".to_string()),
        (
            Unlisted::Offset,
            "its header gives an offset that isn't one".to_string(),
        ),
    ];
    for (unlisted, said) in unlisted {
        assert_eq!(unlisted.to_string(), said, "{unlisted:?}");
    }
}
