//! A stream's data handed out by a reading: as written, decrypted as the
//! reader decrypts an object stream's, or refused where it may be
//! ciphertext the reader doesn't decrypt; and decoded through its filters.

use pdf_crypt::writer::Kind;
use pdf_filters::Fault;
use pdf_objects::dict;

use super::lazy::{attachments_only, encrypted_objects, hex, only_attachments_encrypted, open};
use super::tests::{KINDS, encryption_updated, filtered, objects_2_and_3, root, settings};
use super::*;
use crate::fuzz::{self, Counted};
use crate::write::{self, Body};

/// The plaintext of [`content`]'s stream 4.
pub(crate) const CONTENT: &[u8] = b"0 0 m 99 99 l S";

/// What [`content`]'s stream 5 decodes to.
const TEXT: &[u8] = b"BT /F1 12 Tf 72 712 Td (A page of text) Tj ET";

/// A catalog, its page tree, and a page whose content is stream 4,
/// [`CONTENT`] unfiltered, and stream 5, [`TEXT`] through `/FlateDecode`.
fn content() -> Vec<(u32, Body)> {
    vec![
        (
            1,
            dict! { "Type" => "Catalog", "Pages" => Object::reference(2) }.into(),
        ),
        (
            2,
            dict! { "Type" => "Pages", "Kids" => vec![Object::reference(3)], "Count" => 1 }.into(),
        ),
        (
            3,
            dict! {
                "Type" => "Page",
                "Parent" => Object::reference(2),
                "Contents" => vec![Object::reference(4), Object::reference(5)],
            }
            .into(),
        ),
        (4, Body::stream(Dict::new(), CONTENT)),
        (
            5,
            Body::stream(dict! { "Filter" => "FlateDecode" }, &write::zlib(TEXT)),
        ),
    ]
}

/// A file of a catalog and stream 2, its dictionary's `entries` and its
/// `data`, besides `extra` objects.
fn a_stream(entries: &str, data: &[u8], extra: &[(u32, &[u8])]) -> Vec<u8> {
    let stream = [
        format!("<< {entries} /Length {} >>\nstream\n", data.len()).as_bytes(),
        data,
        b"\nendstream",
    ]
    .concat();
    let objects = [
        &[(1, &b"<< /Type /Catalog >>"[..]), (2, &stream)][..],
        extra,
    ]
    .concat();
    write::assembled("1.7", &objects, "<< /Size 9 /Root 1 0 R >>")
}

/// The plaintext of [`attached`]'s metadata stream, 3.
pub(crate) const XMP: &[u8] = b"<?xpacket begin?><x:xmpmeta/><?xpacket end?>";

/// The plaintext of [`attached`]'s embedded file, 4.
pub(crate) const ATTACHED: &[u8] = b"an attached file's bytes";

/// How [`attached`]'s file is encrypted, by the user's password `user` and
/// the owner's `owner`, under `/EncryptMetadata false`.
pub(crate) struct Attached<'a> {
    /// The kind: of `/V` 4 or later, from which `/EncryptMetadata` counts,
    /// or below, which has no crypt filters.
    pub(crate) kind: Kind,
    /// Its crypt filters, `/CF`'s entries: each one's name and `/CFM`.
    pub(crate) filters: &'a [(&'a str, &'a str)],
    /// The name of the crypt filter its streams are encrypted by, `/StmF`.
    pub(crate) streams: &'a str,
    /// The name of the crypt filter its strings are encrypted by, `/StrF`.
    pub(crate) strings: &'a str,
    /// Its encryption dictionary's `/EFF`, as written, if it has one.
    pub(crate) eff: &'a str,
    /// The metadata stream's dictionary's own entries, as written (its
    /// crypt filter, if it has one), and the name of the crypt filter its
    /// data is encrypted by.
    pub(crate) metadata: (&'a str, &'a str),
    /// The embedded file's, likewise.
    pub(crate) embedded: (&'a str, &'a str),
}

/// [`attached`]'s file encrypted by AES-128 (`/V` 4) with two crypt
/// filters, `/StdCF`, AES-128, its strings' and streams', and `/RC4CF`,
/// RC4; its encryption dictionary's `/EFF` as `eff` writes it; its metadata
/// stream without a crypt filter of its own, so as written, and its
/// embedded file without one, encrypted as its streams are.
pub(crate) fn aes_128_and_rc4(eff: &str) -> Attached<'_> {
    Attached {
        kind: Kind::Aes128,
        filters: &[("StdCF", "AESV2"), ("RC4CF", "V2")],
        streams: "StdCF",
        strings: "StdCF",
        eff,
        metadata: ("", "Identity"),
        embedded: ("", "StdCF"),
    }
}

/// A file encrypted as `how` says, besides `extra` objects: a catalog
/// whose metadata stream, 3, is [`XMP`], and whose one embedded file,
/// stream 4, named `a.txt` by file specification 5, is [`ATTACHED`], each
/// encrypted by the crypt filter `how` names; and stream 6, [`CONTENT`],
/// encrypted as `/StmF` says. Its encryption dictionary is in its trailer.
pub(crate) fn attached(how: &Attached, extra: &[(u32, &[u8])]) -> Vec<u8> {
    let settings = pdf_crypt::writer::Settings {
        encrypt_metadata: false,
        ..settings(how.kind, b"user")
    };
    let mut values = pdf_crypt::writer::dictionary(&settings);
    values.crypt_filters = (how.filters.iter())
        .map(|(name, method)| pdf_crypt::CryptFilter {
            name: name.as_bytes().to_vec(),
            method: Some(method.as_bytes().to_vec()),
        })
        .collect();
    values.stm_f = Some(how.streams.as_bytes().to_vec());
    values.str_f = Some(how.strings.as_bytes().to_vec());
    let handler = Handler::open(&values, settings.id, &[settings.user]).unwrap();
    let name = |n: u32| {
        let name = handler.encrypt_string((n, 0), b"its name", b"a.txt");
        format!("<{}>", hex(&name.unwrap()))
    };
    let stream = |n: u32, entries: String, data: &[u8], by: Option<&str>| {
        let method = by.map(|name| handler.named(name.as_bytes()));
        let data = handler.encrypt_stream((n, 0), b"its data", data, method.as_ref());
        let data = data.unwrap();
        let head = format!("<< {entries}/Length {} >>\nstream\n", data.len());
        [head.as_bytes(), &data, b"\nendstream"].concat()
    };
    let (metadata, metadata_by) = how.metadata;
    let (embedded, embedded_by) = how.embedded;
    let files = format!("/EmbeddedFiles << /Names [{} 5 0 R] >>", name(1));
    let objects = [
        format!("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R /Names << {files} >> >>")
            .into_bytes(),
        b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec(),
        stream(
            3,
            format!("/Type /Metadata /Subtype /XML{metadata} "),
            XMP,
            Some(metadata_by),
        ),
        stream(
            4,
            format!("/Type /EmbeddedFile{embedded} "),
            ATTACHED,
            Some(embedded_by),
        ),
        format!("<< /Type /Filespec /F {} /EF << /F 4 0 R >> >>", name(5)).into_bytes(),
        stream(6, String::new(), CONTENT, None),
    ];
    let objects: Vec<(u32, &[u8])> = (1..)
        .zip(objects.iter().map(Vec::as_slice))
        .chain(extra.iter().copied())
        .collect();
    let mut encrypt = format!("/Filter /Standard /V {} /R {}", values.v, values.r);
    if let Some(length) = values.length {
        encrypt.push_str(&format!(" /Length {length}"));
    }
    encrypt.push_str(&format!(" /P {}", settings.p));
    let fields = [
        ("O", &values.o),
        ("U", &values.u),
        ("OE", &values.oe),
        ("UE", &values.ue),
        ("Perms", &values.perms),
    ];
    for (key, value) in fields {
        if let Some(value) = value {
            encrypt.push_str(&format!(" /{key} <{}>", hex(value)));
        }
    }
    if let Some(encrypt_metadata) = values.encrypt_metadata {
        encrypt.push_str(&format!(" /EncryptMetadata {encrypt_metadata}"));
    }
    let filters: String = (how.filters.iter())
        .map(|(name, method)| format!("/{name} << /CFM /{method} >> "))
        .collect();
    let (streams, strings, eff) = (how.streams, how.strings, how.eff);
    encrypt.push_str(&format!(
        " /CF << {filters}>> /StmF /{streams} /StrF /{strings} {eff}"
    ));
    let size = objects.iter().map(|(n, _)| n + 1).max().unwrap_or(1);
    let id = hex(settings.id);
    let trailer =
        format!("<< /Size {size} /Root 1 0 R /Encrypt << {encrypt} >> /ID [<{id}> <{id}>] >>");
    write::assembled("1.7", &objects, &trailer)
}

/// In a file each kind of encryption writes, opened by the user's password
/// or the owner's, each stream's data is the plaintext the writer was
/// given, not the ciphertext written, and decoded, what that decodes to:
/// from memory and read by position alike.
#[test]
fn each_kind_of_encryption_hands_out_each_streams_data_decrypted() {
    let compressed = write::zlib(TEXT);
    for kind in KINDS {
        let file = write::encrypted(&settings(kind, b"user"), &content(), root());
        // The plaintext isn't written.
        assert!(find(&file, CONTENT).is_none() && find(&file, &compressed).is_none());
        for password in [&b"user"[..], b"owner"] {
            let reading = open(&file, &[password], &Options::DEFAULT);
            assert!(
                matches!(reading.encryption(), Encryption::Opened { .. }),
                "{kind:?}"
            );
            let source = Counted::new(&file, 1);
            let by_position = fuzz::open(&source, &[password], &Options::DEFAULT, false).unwrap();
            for (id, plain, decoded) in [((4, 0), CONTENT, CONTENT), ((5, 0), &compressed, TEXT)] {
                let what = format!("{kind:?}, {id:?}");
                let data = reading.data(id).unwrap();
                assert_eq!(data, plain, "{what}");
                assert!(matches!(data, Cow::Owned(_)), "{what}: decrypted");
                let at = reading.record(id).unwrap().data.unwrap().at;
                assert_ne!(&file[at.start..at.end], plain, "{what}: ciphertext written");
                assert_eq!(reading.decoded(id).unwrap(), decoded, "{what}");
                assert_eq!(by_position.data(id).unwrap(), plain, "{what}");
                assert_eq!(by_position.decoded(id).unwrap(), decoded, "{what}");
            }
            assert!(reading.warnings().is_empty(), "{:?}", reading.warnings());
        }
    }
}

/// A stream's own `/Crypt` filter says how its data is decrypted, as an
/// object stream's: by the crypt filter it names, `/Identity` when it
/// names none; without one, as `/StmF` says. One the encryption dictionary
/// doesn't define doesn't decrypt. Decoded, it's what the whole read
/// decodes it to.
#[test]
fn a_streams_own_crypt_filter_says_how_its_data_is_handed_out() {
    let (objects, _) = objects_2_and_3();
    let zlib = write::zlib(&objects);
    let cases = [
        (" /Filter /FlateDecode", None),
        (
            " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /StdCF >> null]",
            None,
        ),
        (
            " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Identity >> null]",
            Some(&zlib[..]),
        ),
        (" /Filter [/Crypt /FlateDecode]", Some(&zlib[..])),
    ];
    for (filter, data) in cases {
        let file = encrypted_objects(filter, data);
        let reading = open(&file, &[b"user"], &Options::DEFAULT);
        assert_eq!(reading.data((4, 0)).unwrap(), zlib, "{filter}");
        let decoded = reading.decoded((4, 0)).unwrap();
        assert_eq!(decoded, objects, "{filter}");
        let doc = reading.into_document().unwrap();
        assert_eq!(doc.decoded[&(4, 0)], decoded, "{filter}");
    }
    let undefined = " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Undefined >> null]";
    let file = encrypted_objects(undefined, None);
    let reading = open(&file, &[b"user"], &Options::DEFAULT);
    let error = reading.data((4, 0)).unwrap_err();
    assert_eq!(
        error.to_string(),
        "its data doesn't decrypt cleanly: its crypt filter's method or name, /Undefined, isn't one"
    );
    assert!(matches!(error, Error::Decrypt(e) if e.data.is_none()));
}

/// A cross-reference stream's data is handed out as written, as it's never
/// encrypted, in a file that opens and in one that doesn't; decoded, it's
/// its rows, its filters' references not followed, as the file's own
/// reading doesn't follow them.
#[test]
fn a_cross_reference_streams_data_is_handed_out_as_written() {
    let file = encrypted_objects(" /Filter /FlateDecode", None);
    for passwords in [&[&b"user"[..]][..], &[]] {
        let reading = open(&file, passwords, &Options::DEFAULT);
        let data = reading.data((5, 0)).unwrap();
        let at = reading.record((5, 0)).unwrap().data.unwrap().at;
        assert!(matches!(data, Cow::Borrowed(_)), "{passwords:?}");
        assert_eq!(data, &file[at.start..at.end], "{passwords:?}");
        assert_eq!(reading.decoded((5, 0)).unwrap(), &file[at.start..at.end]);
        assert!(matches!(reading.data((4, 0)), Err(Error::Locked)) == passwords.is_empty());
    }
    // Compressed, decoded to its rows.
    let (file, decoded) = super::tests::streams(false, true, b"", None);
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(reading.decoded((5, 0)).unwrap(), decoded[&(5, 0)]);
    // Its /Filter a reference, which isn't followed.
    let at = find(&file, b"/Type /XRef").unwrap();
    let at = at + find(&file[at..], b"/Filter /FlateDecode").unwrap();
    let mut file = file;
    file[at..at + 20].copy_from_slice(b"/Filter 1 0 R       ");
    let reading = open(&file, &[], &Options::DEFAULT);
    let decoded = reading.decoded((5, 0));
    assert!(
        matches!(decoded, Err(Error::Filters(Unfiltered::Reference))),
        "{decoded:?}"
    );
}

/// The catalog's metadata stream under `/EncryptMetadata false` is handed
/// out as written, whether the file opens or not, as the document's
/// metadata is; under `/EncryptMetadata true`, it's decrypted, and refused
/// in a file no password opens.
#[test]
fn metadata_not_encrypted_is_handed_out_as_written_whether_the_file_opens_or_not() {
    let xmp = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>";
    let metadata = dict! { "Type" => "Metadata", "Subtype" => "XML" };
    let objects = [
        (1, dict! { "Type" => "Catalog", "Pages" => Object::reference(2), "Metadata" => Object::reference(3) }.into()),
        (2, dict! { "Type" => "Pages", "Kids" => vec![], "Count" => 0 }.into()),
        (3, Body::stream(metadata, xmp)),
    ];
    // The kinds of /V 4 and 5, from which /EncryptMetadata counts.
    for kind in [Kind::Aes128, Kind::Aes256R5, Kind::Aes256] {
        for encrypt_metadata in [false, true] {
            let settings = pdf_crypt::writer::Settings {
                encrypt_metadata,
                ..settings(kind, b"user")
            };
            let file = write::encrypted(&settings, &objects, root());
            for passwords in [&[][..], &[&b"user"[..]]] {
                let what = format!("{kind:?}, {encrypt_metadata}, {passwords:?}");
                let reading = open(&file, passwords, &Options::DEFAULT);
                let data = reading.data((3, 0));
                match (encrypt_metadata, passwords.is_empty()) {
                    (false, _) => {
                        let data = data.unwrap();
                        assert!(matches!(data, Cow::Borrowed(_)), "{what}");
                        assert_eq!(data, &xmp[..], "{what}");
                        assert_eq!(reading.metadata().unwrap().unwrap(), &xmp[..], "{what}");
                    }
                    (true, false) => assert_eq!(data.unwrap(), &xmp[..], "{what}"),
                    (true, true) => assert!(matches!(data, Err(Error::Locked)), "{what}"),
                }
            }
        }
    }
}

/// The catalog's metadata stream under `/EncryptMetadata false` whose own
/// `/Crypt` filter names one that encrypts it is decrypted by that, not as
/// `/StmF` says nor left as written, by the user's password and the
/// owner's; refused in a file no password opens, whose streams are
/// encrypted or not, and in one whose security handler the reader can't
/// open. One whose own is `/Identity` is handed out as written, whether the
/// file opens or not. Neither is the document's metadata, which is
/// unfiltered.
#[test]
fn metadata_not_encrypted_whose_own_crypt_filter_encrypts_it_is_decrypted_by_it() {
    let own = |name: &str| {
        format!(" /Filter /Crypt /DecodeParms << /Type /CryptFilterDecodeParms /Name /{name} >>")
    };
    let rc4 = own("RC4CF");
    let std_cf = own("StdCF");
    let identity = own("Identity");
    // /StmF a crypt filter of another method; or /Identity, under which its
    // ciphertext would be handed out as written.
    let cases = [
        Attached {
            metadata: (&rc4, "RC4CF"),
            ..aes_128_and_rc4("")
        },
        Attached {
            kind: Kind::Aes256,
            filters: &[("StdCF", "AESV3")],
            streams: "Identity",
            strings: "StdCF",
            eff: "",
            metadata: (&std_cf, "StdCF"),
            embedded: ("", "Identity"),
        },
    ];
    for how in &cases {
        let file = attached(how, &[]);
        let what = format!("{:?}", how.kind);
        assert!(find(&file, XMP).is_none(), "{what}: written encrypted");
        for password in [&b"user"[..], b"owner"] {
            let reading = open(&file, &[password], &Options::DEFAULT);
            assert!(matches!(reading.encryption(), Encryption::Opened { .. }));
            let data = reading.data((3, 0)).unwrap();
            assert!(matches!(data, Cow::Owned(_)), "{what}: decrypted");
            assert_eq!(data, XMP, "{what}");
            assert_eq!(reading.decoded((3, 0)).unwrap(), XMP, "{what}");
            assert_eq!(
                reading.data((6, 0)).unwrap(),
                CONTENT,
                "{what}: as /StmF says"
            );
            assert_eq!(reading.metadata().unwrap(), None, "{what}");
            assert!(
                reading.warnings().is_empty(),
                "{what}: {:?}",
                reading.warnings()
            );
        }
        let reading = open(&file, &[], &Options::DEFAULT);
        assert_eq!(*reading.encryption(), Encryption::Locked, "{what}");
        let error = reading.data((3, 0)).unwrap_err();
        assert!(matches!(error, Error::Locked), "{what}: {error:?}");
        assert!(
            matches!(reading.decoded((3, 0)), Err(Error::Locked)),
            "{what}"
        );
        assert_eq!(reading.metadata().unwrap(), None, "{what}");
        let at = find(&file, b"/Standard").unwrap();
        let other = [&file[..at], b"/Standarx", &file[at + 9..]].concat();
        let reading = open(&other, &[b"user"], &Options::DEFAULT);
        assert!(matches!(reading.encryption(), Encryption::Unsupported(_)));
        assert!(
            matches!(reading.data((3, 0)), Err(Error::Unsupported)),
            "{what}"
        );
    }
    // In a file whose strings and streams aren't encrypted.
    let plain = Attached {
        metadata: (&rc4, "RC4CF"),
        ..aes_128_and_rc4("")
    };
    let file = attached(
        &Attached {
            streams: "Identity",
            strings: "Identity",
            ..plain
        },
        &[],
    );
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(*reading.encryption(), Encryption::Plain);
    assert!(matches!(reading.data((3, 0)), Err(Error::Locked)));
    assert!(matches!(reading.decoded((3, 0)), Err(Error::Locked)));
    assert_eq!(reading.data((6, 0)).unwrap(), CONTENT);
    // Its own crypt filter /Identity.
    let file = attached(
        &Attached {
            metadata: (&identity, "Identity"),
            ..aes_128_and_rc4("")
        },
        &[],
    );
    for passwords in [&[][..], &[&b"user"[..]]] {
        let reading = open(&file, passwords, &Options::DEFAULT);
        let data = reading.data((3, 0)).unwrap();
        assert!(matches!(data, Cow::Borrowed(_)), "{passwords:?}");
        assert_eq!(data, XMP, "{passwords:?}");
        assert_eq!(reading.decoded((3, 0)).unwrap(), XMP, "{passwords:?}");
    }
}

/// In a file no password it's given opens, a stream's data is refused, its
/// password needed, decoded too; what isn't a stream, or isn't in the
/// file, says so.
#[test]
fn a_locked_files_streams_are_refused() {
    for kind in KINDS {
        let file = write::encrypted(&settings(kind, b"user"), &content(), root());
        for passwords in [&[][..], &[&b"wrong"[..]]] {
            let reading = open(&file, passwords, &Options::DEFAULT);
            assert_eq!(*reading.encryption(), Encryption::Locked, "{kind:?}");
            for id in [(4, 0), (5, 0)] {
                assert!(matches!(reading.data(id), Err(Error::Locked)), "{kind:?}");
                assert!(
                    matches!(reading.decoded(id), Err(Error::Locked)),
                    "{kind:?}"
                );
            }
        }
    }
    let file = write::encrypted(&settings(Kind::Aes256, b"user"), &content(), root());
    let reading = open(&file, &[], &Options::DEFAULT);
    let said = |id| reading.data(id).unwrap_err().to_string();
    assert_eq!(
        said((4, 0)),
        "the file is encrypted, and no password the reader was given opens it"
    );
    assert_eq!(said((1, 0)), "1 0 R isn't a stream");
    assert_eq!(said((9, 0)), "9 0 R isn't in the file");
}

/// In a file no password opens whose strings and streams aren't
/// encrypted, as one that encrypts only its embedded files, a stream's
/// data is handed out as written, borrowed from the bytes in memory, but
/// one whose own `/Crypt` filter encrypts it, which is refused, its
/// password needed; one whose own is `/Identity` is as written.
#[test]
fn a_plain_files_streams_are_handed_out_as_written_but_one_its_own_crypt_filter_encrypts() {
    let (objects, _) = objects_2_and_3();
    let file = attachments_only();
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(*reading.encryption(), Encryption::Plain);
    let data = reading.data((4, 0)).unwrap();
    assert!(matches!(data, Cow::Borrowed(_)));
    assert_eq!(data, objects);
    assert_eq!(reading.decoded((4, 0)).unwrap(), objects);
    let crypt = " /Filter /Crypt /DecodeParms << /Name /StdCF >>";
    let file = only_attachments_encrypted(&filtered(false, crypt, &objects));
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(*reading.encryption(), Encryption::Plain);
    assert!(matches!(reading.data((4, 0)), Err(Error::Locked)));
    assert!(matches!(reading.decoded((4, 0)), Err(Error::Locked)));
    let identity = " /Filter /Crypt /DecodeParms << /Name /Identity >>";
    let file = only_attachments_encrypted(&filtered(false, identity, &objects));
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(reading.data((4, 0)).unwrap(), objects);
    assert_eq!(reading.decoded((4, 0)).unwrap(), objects);
}

/// An embedded file's data, in a file whose encryption dictionary has an
/// `/EFF`, is decrypted by the crypt filter `/EFF` names, not as `/StmF`
/// says, by the user's password and the owner's, `/EFF` followed where
/// it's a reference; but one with a `/Crypt` filter of its own is
/// decrypted by that. In a file no password opens, whose streams are
/// encrypted or not, it's refused, but where `/EFF` is `/Identity`. An
/// `/EFF` that isn't a name doesn't say how embedded files are encrypted,
/// so one's data is refused in a file that opens; a null is none, and so
/// is a reference to no object, and below `/V` 4, which has no crypt
/// filters, `/EFF` is nothing: as any stream's.
#[test]
fn an_embedded_files_data_under_eff_is_decrypted_by_it() {
    let own = |name: &str| {
        format!(" /Filter /Crypt /DecodeParms << /Type /CryptFilterDecodeParms /Name /{name} >>")
    };
    let std_cf = own("StdCF");
    let decrypted = |file: &[u8], what: &str| {
        for password in [&b"user"[..], b"owner"] {
            let reading = open(file, &[password], &Options::DEFAULT);
            assert!(matches!(reading.encryption(), Encryption::Opened { .. }));
            let data = reading.data((4, 0)).unwrap();
            assert!(matches!(data, Cow::Owned(_)), "{what}: decrypted");
            assert_eq!(data, ATTACHED, "{what}");
            assert_eq!(reading.decoded((4, 0)).unwrap(), ATTACHED, "{what}");
            assert_eq!(
                reading.data((6, 0)).unwrap(),
                CONTENT,
                "{what}: as /StmF says"
            );
            assert!(
                reading.warnings().is_empty(),
                "{what}: {:?}",
                reading.warnings()
            );
        }
    };
    // What an /EFF can refer to: a name, a string, and an object that
    // doesn't read; each in a file whose /EFF refers to it.
    let referred: [(u32, &[u8]); 3] = [(7, b"/RC4CF"), (8, b"(RC4CF)"), (9, b"<< /Open")];
    let written = |how: &Attached| {
        let extra: Vec<(u32, &[u8])> = (referred.iter())
            .filter(|(n, _)| how.eff == format!("/EFF {n} 0 R"))
            .copied()
            .collect();
        attached(how, &extra)
    };
    let cases = [
        // /StmF a crypt filter of another method.
        (
            Attached {
                embedded: ("", "RC4CF"),
                ..aes_128_and_rc4("/EFF /RC4CF")
            },
            "/EFF /RC4CF",
        ),
        (
            Attached {
                embedded: ("", "RC4CF"),
                ..aes_128_and_rc4("/EFF 7 0 R")
            },
            "/EFF by reference",
        ),
        // Its own crypt filter, not /EFF's.
        (
            Attached {
                embedded: (&std_cf, "StdCF"),
                ..aes_128_and_rc4("/EFF /RC4CF")
            },
            "its own",
        ),
        (
            Attached {
                embedded: (&std_cf, "StdCF"),
                ..aes_128_and_rc4("/EFF 8 0 R")
            },
            "its own, /EFF not a name",
        ),
        // /StmF /Identity, under which its ciphertext would be handed out as
        // written.
        (
            Attached {
                kind: Kind::Aes256,
                filters: &[("StdCF", "AESV3")],
                streams: "Identity",
                strings: "StdCF",
                eff: "/EFF /StdCF",
                metadata: ("", "Identity"),
                embedded: ("", "StdCF"),
            },
            "/V 5",
        ),
    ];
    for (how, what) in &cases {
        let file = written(how);
        assert!(find(&file, ATTACHED).is_none(), "{what}: written encrypted");
        decrypted(&file, what);
        let reading = open(&file, &[], &Options::DEFAULT);
        assert_eq!(*reading.encryption(), Encryption::Locked, "{what}");
        assert!(matches!(reading.data((4, 0)), Err(Error::Locked)), "{what}");
        assert!(
            matches!(reading.decoded((4, 0)), Err(Error::Locked)),
            "{what}"
        );
    }
    // A null /EFF, a reference to no object, and none: as /StmF says.
    for eff in ["/EFF null", "/EFF 99 0 R", ""] {
        decrypted(&written(&aes_128_and_rc4(eff)), eff);
    }
    // Below /V 4, as every stream is, by RC4, whatever /EFF is.
    for eff in ["/EFF /RC4CF", "/EFF 8 0 R"] {
        let how = Attached {
            kind: Kind::Rc4By128,
            ..aes_128_and_rc4(eff)
        };
        decrypted(&written(&how), eff);
    }
    // /EFF that isn't a name: refused as it could be ciphertext, the other
    // streams decrypted; in a file no password opens, refused all the same.
    for eff in ["/EFF 8 0 R", "/EFF 9 0 R", "/EFF [/RC4CF]", "/EFF 1"] {
        let how = Attached {
            embedded: ("", "RC4CF"),
            ..aes_128_and_rc4(eff)
        };
        let file = written(&how);
        let reading = open(&file, &[b"user"], &Options::DEFAULT);
        let error = reading.data((4, 0)).unwrap_err();
        assert!(matches!(error, Error::Encrypted), "{eff}: {error:?}");
        assert!(
            matches!(reading.decoded((4, 0)), Err(Error::Encrypted)),
            "{eff}"
        );
        assert_eq!(reading.data((6, 0)).unwrap(), CONTENT, "{eff}");
        let reading = open(&file, &[], &Options::DEFAULT);
        assert!(matches!(reading.data((4, 0)), Err(Error::Locked)), "{eff}");
    }
    // In a file whose strings and streams aren't encrypted, refused, its
    // password needed, but where /EFF is /Identity, as written.
    let file =
        only_attachments_encrypted(&a_stream("/Type /EmbeddedFile", b"0123456789abcdef", &[]));
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(*reading.encryption(), Encryption::Plain);
    let error = reading.data((2, 0)).unwrap_err();
    assert!(matches!(error, Error::Locked), "{error:?}");
    assert!(matches!(reading.decoded((2, 0)), Err(Error::Locked)));
    let plain = |eff, by| Attached {
        streams: "Identity",
        strings: "Identity",
        embedded: ("", by),
        ..aes_128_and_rc4(eff)
    };
    for (eff, by) in [("/EFF /RC4CF", "RC4CF"), ("/EFF 8 0 R", "RC4CF")] {
        let file = written(&plain(eff, by));
        let reading = open(&file, &[], &Options::DEFAULT);
        assert_eq!(*reading.encryption(), Encryption::Plain, "{eff}");
        assert!(matches!(reading.data((4, 0)), Err(Error::Locked)), "{eff}");
        assert_eq!(reading.data((6, 0)).unwrap(), CONTENT, "{eff}");
    }
    let file = written(&plain("/EFF /Identity", "Identity"));
    let reading = open(&file, &[], &Options::DEFAULT);
    let data = reading.data((4, 0)).unwrap();
    assert!(matches!(data, Cow::Borrowed(_)));
    assert_eq!(data, ATTACHED);
}

/// A stream the encryption dictionary refers to, whose strings the reader
/// leaves as written, is refused, as its data as written could be
/// ciphertext; the file's other streams are decrypted.
#[test]
fn a_stream_the_encryption_dictionary_refers_to_is_refused() {
    let file = write::encrypted(&settings(Kind::Aes256, b""), &content(), root());
    let stream = "<< /Length 3 >>\nstream\nabc\nendstream";
    let file = encryption_updated(&file, "/Length 256", "/Length 8 0 R", &[(8, stream)]);
    let reading = open(&file, &[], &Options::DEFAULT);
    assert!(matches!(reading.encryption(), Encryption::Opened { .. }));
    let error = reading.data((8, 0)).unwrap_err();
    assert!(matches!(error, Error::Encrypted), "{error:?}");
    assert!(matches!(reading.decoded((8, 0)), Err(Error::Encrypted)));
    assert_eq!(reading.data((4, 0)).unwrap(), CONTENT);
    let said = reading.data((6, 0)).unwrap_err().to_string();
    assert_eq!(said, "6 0 R isn't a stream");
}

/// In a file whose security handler the reader can't open, a stream's data
/// is refused, as it may be ciphertext.
#[test]
fn an_unsupported_handlers_streams_are_refused() {
    let file = write::encrypted(&settings(Kind::Aes128, b""), &content(), root());
    // Another handler's name, of the same length, so nothing moves.
    let at = find(&file, b"/Standard").unwrap();
    let file = [&file[..at], b"/Standarx", &file[at + 9..]].concat();
    let reading = open(&file, &[], &Options::DEFAULT);
    assert!(matches!(reading.encryption(), Encryption::Unsupported(_)));
    let error = reading.data((4, 0)).unwrap_err();
    assert!(matches!(error, Error::Unsupported));
    assert_eq!(
        error.to_string(),
        "the file is encrypted by a security handler the reader can't open"
    );
    assert!(matches!(reading.decoded((5, 0)), Err(Error::Unsupported)));
}

/// Data that doesn't decrypt cleanly is an error holding what decrypted:
/// AES ciphertext shorter than its IV, nothing; with part of a block past
/// its whole ones, the whole ones.
#[test]
fn a_streams_data_that_doesnt_decrypt_cleanly_is_an_error_holding_what_did() {
    let cases = [
        (
            &[b'x'; 8][..],
            None,
            "its AES ciphertext has no whole block past its IV",
        ),
        (
            &[b'x'; 35][..],
            Some(16),
            "its AES ciphertext has a part block past its whole blocks",
        ),
    ];
    for (junk, decrypted, fault) in cases {
        let file = encrypted_objects(" /Filter /FlateDecode", Some(junk));
        let reading = open(&file, &[b"user"], &Options::DEFAULT);
        let error = reading.data((4, 0)).unwrap_err();
        let said = format!("its data doesn't decrypt cleanly: {fault}");
        assert_eq!(error.to_string(), said);
        let Error::Decrypt(error) = error else {
            panic!("{error:?}")
        };
        assert_eq!(error.data.as_ref().map(Vec::len), decrypted);
        assert!(matches!(reading.decoded((4, 0)), Err(Error::Decrypt(_))));
    }
}

/// A stream's data decoded through each filter the reader decodes, by its
/// name or abbreviation, a chain of them, and parameters by reference; and
/// handed out as written, borrowed from the bytes in memory.
#[test]
fn a_streams_data_decodes_through_each_filter() {
    let hex = |data: &[u8]| format!("{}>", hex(data)).into_bytes();
    let cases = [
        ("/Filter /FlateDecode", write::zlib(TEXT)),
        ("/Filter /Fl", write::zlib(TEXT)),
        ("/Filter /LZWDecode", write::lzw(TEXT)),
        (
            "/Filter /LZW /DecodeParms << /EarlyChange 1 >>",
            write::lzw(TEXT),
        ),
        ("/Filter /ASCIIHexDecode", hex(TEXT)),
        ("/Filter /AHx", hex(TEXT)),
        ("/Filter /ASCII85Decode", write::ascii85(TEXT)),
        ("/Filter /A85", write::ascii85(TEXT)),
        ("/Filter /RunLengthDecode", write::run_length(TEXT)),
        ("/Filter /RL", write::run_length(TEXT)),
        ("/Filter [/AHx /Fl]", hex(&write::zlib(TEXT))),
        ("/Filter 3 0 R /DecodeParms 4 0 R", hex(&write::zlib(TEXT))),
        ("/Filter []", TEXT.to_vec()),
        ("", TEXT.to_vec()),
    ];
    let extra: [(u32, &[u8]); 2] = [(3, b"[/AHx /Fl]"), (4, b"[null << /Predictor 1 >>]")];
    for (entries, data) in cases {
        let file = a_stream(entries, &data, &extra);
        let reading = open(&file, &[], &Options::DEFAULT);
        let written = reading.data((2, 0)).unwrap();
        assert!(matches!(written, Cow::Borrowed(_)), "{entries}");
        assert_eq!(written, data, "{entries}");
        assert_eq!(reading.decoded((2, 0)).unwrap(), TEXT, "{entries}");
        let source = Counted::new(&file, 1);
        let by_position = fuzz::open(&source, &[], &Options::DEFAULT, false).unwrap();
        assert_eq!(by_position.data((2, 0)).unwrap(), data, "{entries}");
        assert_eq!(by_position.decoded((2, 0)).unwrap(), TEXT, "{entries}");
    }
}

/// Data a fault stops decoding is an error holding what decoded before it,
/// and data that decodes past the reading's limit on a stream is cut off
/// there, an error holding as much as the limit allows; filters the reader
/// doesn't take say why.
#[test]
fn a_streams_data_that_doesnt_decode_cleanly_is_an_error_holding_what_did() {
    let file = a_stream("/Filter /AHx", b"48 65 6c 6c 6fZ 21>", &[]);
    let reading = open(&file, &[], &Options::DEFAULT);
    let error = reading.decoded((2, 0)).unwrap_err();
    assert_eq!(
        error.to_string(),
        "its data doesn't decode cleanly: filter 0: byte 0x5a at 14 isn't one it takes"
    );
    let Error::Decode(error) = error else {
        panic!("{error:?}")
    };
    assert_eq!(error.data, b"Hello");
    let file = a_stream("/Filter /FlateDecode", &write::zlib(&[b'a'; 4096]), &[]);
    let mut options = Options::DEFAULT;
    options.limits.stream = 100;
    let reading = open(&file, &[], &options);
    let Err(Error::Decode(error)) = reading.decoded((2, 0)) else {
        panic!("decoded past its limit")
    };
    assert_eq!(error.data, [b'a'; 100]);
    assert!(
        error.faults.iter().any(|(_, f)| *f == Fault::Limit),
        "{error:?}"
    );
    let file = a_stream("/Filter /DCTDecode", b"\xff\xd8", &[]);
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(reading.data((2, 0)).unwrap(), &b"\xff\xd8"[..]);
    let error = reading.decoded((2, 0)).unwrap_err();
    assert!(matches!(
        error,
        Error::Filters(Unfiltered::Unsupported { .. })
    ));
    assert_eq!(
        error.to_string(),
        "its filter /DCTDecode isn't one the reader decodes"
    );
}

/// A stream's data, read and decoded in any order with the objects, keeps
/// the reading the whole read: what's handed out isn't kept, nor counted
/// against the budget.
#[test]
fn a_streams_data_handed_out_leaves_the_reading_the_whole_read() {
    let file = write::encrypted(&settings(Kind::Aes128, b"user"), &content(), root());
    let reading = open(&file, &[b"user"], &Options::DEFAULT);
    for id in [(5, 0), (4, 0)] {
        assert!(reading.decoded(id).is_ok() && reading.data(id).is_ok());
    }
    let doc = reading.into_document().unwrap();
    assert_eq!(doc, read_opened(&file, &[b"user"]));
    let (objects, _) = objects_2_and_3();
    let file = filtered(false, " /Filter /FlateDecode", &write::zlib(&objects));
    let mut options = Options::DEFAULT;
    options.limits.budget = objects.len();
    let reading = open(&file, &[], &options);
    assert_eq!(reading.decoded((4, 0)).unwrap(), objects);
    assert_eq!(reading.decoded((4, 0)).unwrap(), objects);
    assert!(reading.get((2, 0)).is_ok());
    assert_eq!(reading.reader.budget.load(Ordering::Relaxed), 0);
}
