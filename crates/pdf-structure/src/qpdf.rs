//! qpdf, the tests' oracle, beside the bytes reader: pdf-fixtures' runner
//! made a decoder ([`Decode`]), decoding what the reader is made to ask of
//! it, a compressed cross-reference stream's rows, an object stream's
//! objects, and in an encrypted file the strings' plaintext, to set beside
//! the reader's own decoding; and a second reader, the file as qpdf alone
//! reads it made a document of this crate's ([`Alone`]), to set beside the
//! bytes reader. Its tests set the two side by side, and write anew the
//! files qpdf made that `tests/files` holds.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;
use std::time::Instant;

use pdf_fixtures::qpdf::Qpdf;
use pdf_objects::object::{Dict, Object, ObjectId};

use crate::read::{Decode, Unwritten, Written};
use crate::source::{Document, Trailer, Unread};

/// qpdf decodes for the reader as pdf-fixtures' runner decodes: each
/// stream and object by its number, through its JSON, and a stream from
/// its bytes alone through a file of its own.
impl Decode for Qpdf {
    fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
        Qpdf::decode(self, ids)
    }

    fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
        Qpdf::plaintext(self, ids)
    }

    fn decode_written(&self, stream: Written) -> Result<Vec<u8>, String> {
        let Written {
            filter,
            parms,
            data,
        } = stream;
        Qpdf::decode_written(
            self,
            pdf_fixtures::qpdf::Written {
                filter,
                parms,
                data,
            },
        )
    }
}

/// qpdf's warnings, each with the object it names, if it names one.
type Warnings = Vec<(Option<ObjectId>, String)>;

/// The file as qpdf alone reads it, a document of this crate's, as
/// pdf-fixtures' `Qpdf::read` builds one of the copy of the crate it links.
trait Alone {
    /// The file as qpdf reads it ([`Qpdf::listing`]): every object it
    /// lists, as written but for what qpdf doesn't keep, which is where
    /// anything was written, the order of a dictionary's keys and any key
    /// written twice, a name's escapes, and which encoding a text string
    /// had (`Str::Text`); its strings decrypted. Where qpdf reconstructed
    /// the cross-references, the trailer says so, in qpdf's words. The
    /// error is why qpdf read nothing.
    fn alone(&self) -> Result<Document, String> {
        self.heard().map(|(doc, _)| doc)
    }

    /// [`Alone::alone`], and qpdf's warnings beside it: a document's faults
    /// are the bytes reader's.
    fn heard(&self) -> Result<(Document, Warnings), String>;
}

impl Alone for Qpdf {
    fn heard(&self) -> Result<(Document, Warnings), String> {
        let listing = self.listing()?;
        let objects = (listing.objects.into_iter())
            .map(|(id, object)| (id, object.map(|o| (o, None)).map_err(Unread::Other)))
            .collect();
        let trailer = Trailer {
            stream: listing.stream,
            object: listing.trailer,
            unwritten: listing.unwritten.map(Unwritten::Other),
        };
        let mut doc = Document::read(objects, trailer, None);
        doc.header = listing.version;
        doc.size = listing.size;
        Ok((doc, listing.warnings))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use pdf_crypt::Password;
    use pdf_filters::Backend;
    use pdf_fixtures::fixture::{self, Body, Xref};
    use pdf_objects::dict;
    use pdf_objects::object::{Form, Object, Str, Value};
    use pdf_objects::text;
    use tempfile::TempDir;

    use super::*;
    use crate::read;
    use crate::source::Source;

    /// qpdf, which these tests need: `PDF_LINT_QPDF`, else `qpdf` on the
    /// `PATH`. They're ignored, so that `cargo test` needs no qpdf, and run
    /// by `just test-qpdf` against the pinned one; its absence then fails
    /// them, rather than passing them unrun.
    fn qpdf(file: &Path) -> Qpdf {
        let qpdf = Qpdf::new(file);
        if let Err(why) = qpdf.version() {
            panic!("these tests need qpdf ({why}): install it, or name it in PDF_LINT_QPDF");
        }
        qpdf
    }

    /// A scratch folder of the test's own, removed when it's dropped.
    fn scratch(test: &str) -> TempDir {
        tempfile::Builder::new()
            .prefix(&format!("pdf-lint-qpdf-{test}-"))
            .tempdir()
            .unwrap()
    }

    /// A clean file: a catalog, its page tree and page, and a text
    /// annotation with its appearance stream.
    fn clean() -> Vec<(u32, Body)> {
        let r = Object::reference;
        let form = dict! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(20), Object::from(20)],
            "Resources" => dict! { "ProcSet" => vec![Object::from("PDF")] },
        };
        vec![
            (1, dict! { "Type" => "Catalog", "Pages" => r(2) }.into()),
            (2, dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }.into()),
            (
                3,
                dict! {
                    "Type" => "Page",
                    "Parent" => r(2),
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
                    "Resources" => dict! {},
                    "Annots" => vec![r(4)],
                }
                .into(),
            ),
            (
                4,
                dict! {
                    "Type" => "Annot",
                    "Subtype" => "Text",
                    "Rect" => vec![Object::from(100), Object::from(100), Object::from(120), Object::from(120)],
                    "Contents" => Object::text("A note"),
                    "AP" => dict! { "N" => r(5) },
                }
                .into(),
            ),
            (5, Body::stream(form, b"0 0 m 20 20 l S")),
        ]
    }

    /// The files qpdf wrote from [`clean`], committed in `tests/files` so
    /// that what's read from them needs no qpdf, nor a run's random /ID:
    /// each name, and qpdf's arguments.
    const FIXTURES: [(&str, &[&str]); 8] = [
        ("qpdf-plain", &["--object-streams=disable"]),
        ("qpdf-compressed", &["--object-streams=generate"]),
        (
            "qpdf-rc4-40",
            &[
                "--object-streams=generate",
                "--allow-weak-crypto",
                "--encrypt",
                "",
                "owner",
                "40",
                "--",
            ],
        ),
        (
            "qpdf-rc4-128",
            &[
                "--object-streams=generate",
                "--allow-weak-crypto",
                "--encrypt",
                "",
                "owner",
                "128",
                "--use-aes=n",
                "--",
            ],
        ),
        (
            "qpdf-aes-128",
            &[
                "--object-streams=generate",
                "--encrypt",
                "",
                "owner",
                "128",
                "--use-aes=y",
                "--",
            ],
        ),
        (
            "qpdf-aes-256-r5",
            &[
                "--object-streams=generate",
                "--encrypt",
                "",
                "owner",
                "256",
                "--force-R5",
                "--",
            ],
        ),
        (
            "qpdf-aes-256",
            &[
                "--object-streams=generate",
                "--encrypt",
                "",
                "owner",
                "256",
                "--",
            ],
        ),
        (
            "qpdf-aes-256-unpacked",
            &[
                "--object-streams=disable",
                "--encrypt",
                "",
                "owner",
                "256",
                "--",
            ],
        ),
    ];

    /// A committed fixture's path, by name.
    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/files")
            .join(format!("{name}.pdf"))
    }

    /// The fixtures written anew by the qpdf `PDF_LINT_QPDF` names, else
    /// the PATH's, as `just fixtures` does: only when [`clean`] or the
    /// kinds change.
    #[test]
    #[ignore = "needs qpdf; writes the committed fixtures anew"]
    fn the_fixtures_are_written() {
        for (name, args) in FIXTURES {
            let (_dir, rewritten) = rewritten(name, args);
            std::fs::copy(rewritten, fixture(name)).unwrap();
        }
    }

    /// The clean file, written, then rewritten by qpdf with `args`; with
    /// `--object-streams=generate`, its dictionaries in object streams, and
    /// a cross-reference stream, compressed: its path, in the test's folder,
    /// which is removed when it's dropped.
    fn rewritten(test: &str, args: &[&str]) -> (TempDir, PathBuf) {
        let dir = scratch(test);
        let plain = dir.path().join("plain.pdf");
        let root = dict! { "Root" => Object::reference(1) };
        std::fs::write(&plain, fixture::file("1.7", &clean(), root, Xref::Table)).unwrap();
        let out = dir.path().join("compressed.pdf");
        let qpdf = qpdf(&plain);
        let mut command = qpdf.command();
        // An /ID from the content, not the clock, so that a run can't
        // draw one that begins FE FF, which qpdf reads as UTF-16 text;
        // qpdf can't for a file it encrypts.
        if !args.contains(&"--encrypt") {
            command.arg("--deterministic-id");
        }
        command.args(args).arg(&plain).arg(&out);
        let status = command.status().unwrap();
        assert!(matches!(status.code(), Some(0 | 3)), "qpdf {args:?} failed");
        (dir, out)
    }

    /// A decoder that mustn't be asked.
    struct Unasked;

    impl Decode for Unasked {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            panic!("asked to decode {ids:?}")
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_compressed_file_reads_without_qpdf_as_through_it() {
        let path = fixture("qpdf-compressed");
        let data = std::fs::read(&path).unwrap();
        let doc = read::read_with(&data, &Unasked);
        assert_eq!(doc.trailer.unwritten, None);
        assert!(doc.trailer.stream.is_some(), "a cross-reference stream");
        assert!(doc.faults().is_empty(), "{:?}", doc.faults());
        let mut in_streams = 0;
        assert_eq!(doc.unread().next(), None);
        for (id, object) in doc.objects() {
            let at = object.at.unwrap();
            if let Some(stream) = at.stream {
                in_streams += 1;
                let written = &doc.decoded[&stream][at.start..at.end];
                assert!(
                    written.starts_with(b"<<") && written.ends_with(b">>"),
                    "{id:?}"
                );
            }
        }
        assert!(
            in_streams >= 4,
            "the dictionaries are in object streams: {in_streams}"
        );

        // Through qpdf, asked for every stream, alike: the same streams'
        // data, and the same objects read from them.
        let through = read::read_through(&data, &qpdf(&path));
        assert!(through.faults().is_empty(), "{:?}", through.faults());
        assert_eq!(through.decoded, doc.decoded);
        assert!(through.objects().eq(doc.objects()));
        assert!(through.unread().eq(doc.unread()));
    }

    /// A decoder that mustn't be asked anything: neither to decode nor for
    /// plaintext.
    struct Unaskable;

    impl Decode for Unaskable {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            panic!("asked to decode {ids:?}")
        }

        fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
            panic!("asked for the plaintext of {ids:?}")
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn an_encrypted_files_object_streams_decrypt_by_the_reader_as_through_qpdf() {
        let kinds = [
            "qpdf-rc4-40",
            "qpdf-rc4-128",
            "qpdf-aes-128",
            "qpdf-aes-256-r5",
            "qpdf-aes-256",
        ];
        for kind in kinds {
            let path = fixture(kind);
            let data = std::fs::read(&path).unwrap();
            // The reader alone, by the empty user password and by the
            // owner's.
            let doc = read::read_with(&data, &Unaskable);
            assert_eq!(
                doc.encryption,
                crate::Encryption::Opened {
                    owner: false,
                    by: None,
                    permissions: pdf_crypt::Permissions::ALL,
                },
                "{kind}"
            );
            assert!(doc.faults().is_empty(), "{kind}: {:?}", doc.faults());
            assert_eq!(doc.trailer.unwritten, None, "{kind}");
            let annot = (doc.objects().map(|(_, o)| o))
                .find(|o| {
                    o.as_dict().is_some_and(|d| {
                        d.get(b"Subtype").and_then(Object::as_name) == Some(b"Text")
                    })
                })
                .unwrap_or_else(|| panic!("{kind}: the annotation reads"));
            assert!(
                annot.at.unwrap().stream.is_some(),
                "{kind}: in an object stream"
            );
            let contents = annot.as_dict().unwrap().get(b"Contents").unwrap();
            assert_eq!(
                *contents.value(),
                Value::String(Str::Bytes(b"A note".to_vec()), Some(Form::Literal)),
                "{kind}"
            );
            let owned = read::document(
                &data,
                &read::Options {
                    passwords: &[Password("owner")],
                    ..read::Options::DEFAULT
                },
            );
            let by = Some((0, pdf_crypt::Encoding::Utf8));
            assert_eq!(
                owned.encryption,
                crate::Encryption::Opened {
                    owner: true,
                    by,
                    permissions: pdf_crypt::Permissions::ALL
                },
                "{kind}"
            );
            assert_eq!(owned.decoded, doc.decoded, "{kind}");
            // Through qpdf, the object streams' data the same, byte for byte.
            let through = read::read_through(&data, &qpdf(&path));
            assert!(!through.decoded.is_empty(), "{kind}");
            assert_eq!(through.decoded, doc.decoded, "{kind}");
        }
    }

    /// A metadata stream under `/EncryptMetadata false` whose own crypt
    /// filter encrypts it, and an embedded file with a crypt filter of its
    /// own, each encrypted by one that isn't the one `/StmF` names, decrypt
    /// by the reader as through qpdf, byte for byte, by the user's password
    /// and the owner's, as does the file's other stream, by `/StmF`'s. But
    /// an embedded file without one of its own, under an `/EFF` that isn't
    /// `/StmF`, qpdf decrypts as `/StmF` says, as it reads `/EFF` only to
    /// tell what a writer should encrypt new embedded files by: under
    /// `/StmF /Identity`, as written; the reader by `/EFF`'s, which ISO
    /// 32000-2's Table 20 says decrypts it, to the plaintext it was written
    /// from. qpdf takes a stream's own crypt filter's name from parameters
    /// of `/Type /CryptFilterDecodeParms`, which the files have.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_metadata_stream_and_an_embedded_file_decrypt_as_through_qpdf_but_where_it_ignores_eff() {
        use crate::read::streams::{ATTACHED, Attached, CONTENT, XMP, aes_128_and_rc4, attached};
        use pdf_crypt::writer::Kind;

        let own = |name: &str| {
            format!(
                " /Filter /Crypt /DecodeParms << /Type /CryptFilterDecodeParms /Name /{name} >>"
            )
        };
        let (rc4, std_cf) = (own("RC4CF"), own("StdCF"));
        // Each file, and whether qpdf hands its embedded file out as
        // written, where the reader decrypts it.
        let cases = [
            // /StmF and /EFF AES-128; the metadata stream and the embedded
            // file each RC4 by its own.
            (
                Attached {
                    metadata: (&rc4, "RC4CF"),
                    embedded: (&rc4, "RC4CF"),
                    ..aes_128_and_rc4("/EFF /StdCF")
                },
                false,
            ),
            // /StmF /Identity; the metadata stream RC4 by its own, and the
            // embedded file RC4 by /EFF's.
            (
                Attached {
                    streams: "Identity",
                    metadata: (&rc4, "RC4CF"),
                    embedded: ("", "RC4CF"),
                    ..aes_128_and_rc4("/EFF /RC4CF")
                },
                true,
            ),
            // /StmF /Identity; the metadata stream AES-256 by its own, and
            // the embedded file by /EFF's.
            (
                Attached {
                    kind: Kind::Aes256,
                    filters: &[("StdCF", "AESV3")],
                    streams: "Identity",
                    strings: "StdCF",
                    eff: "/EFF /StdCF",
                    metadata: (&std_cf, "StdCF"),
                    embedded: ("", "StdCF"),
                },
                true,
            ),
        ];
        let dir = scratch("crypt-filters");
        let ids = [
            ObjectId::new(3, 0),
            ObjectId::new(4, 0),
            ObjectId::new(6, 0),
        ];
        for (i, (how, ignored)) in cases.iter().enumerate() {
            let path = dir.path().join(format!("{i}.pdf"));
            let file = attached(how, &[]);
            std::fs::write(&path, &file).unwrap();
            for password in ["user", "owner"] {
                let what = format!("{i}, {password}");
                let options = read::Options::DEFAULT;
                let reading = read::Reading::open(
                    &file[..],
                    &read::Options {
                        passwords: &[Password(password)],
                        ..options
                    },
                );
                let reading = reading.unwrap();
                let theirs = qpdf(&path).undecoding().password(password).decode(&ids);
                for (id, plain) in ids.into_iter().zip([XMP, ATTACHED, CONTENT]) {
                    let mine = reading.data(id).unwrap();
                    assert_eq!(mine, plain, "{what}: {id:?}");
                    let theirs = theirs[&id].as_deref();
                    if id == ObjectId::new(4, 0) && *ignored {
                        let at = reading.record(id).unwrap().data.unwrap().at;
                        assert_eq!(theirs, Ok(&file[at.start..at.end]), "{what}: {id:?}");
                    } else {
                        assert_eq!(theirs, Ok(&mine[..]), "{what}: {id:?}");
                    }
                }
            }
        }
    }

    /// Whether `x`, read by `a`, and `y`, by `b`, are alike, references
    /// followed in step, each pair once: a dictionary's keys in any order, a
    /// string's form aside, and a stream's data decoded, its `/Length`
    /// aside. Else where they differ.
    fn alike(
        (a, x): (&read::Reading<Vec<u8>>, &Object),
        (b, y): (&read::Reading<Vec<u8>>, &Object),
        followed: &mut BTreeMap<ObjectId, ObjectId>,
    ) -> Result<(), String> {
        let differ = || Err(format!("{:?} and {:?}", x.value(), y.value()));
        let dicts = |x: &Dict, y: &Dict, followed: &mut BTreeMap<_, _>| {
            let keys = |d: &Dict| {
                (d.iter().map(|(k, _)| k.to_vec()))
                    .filter(|k| k != b"Length")
                    .collect::<std::collections::BTreeSet<_>>()
            };
            if keys(x) != keys(y) {
                return Err(format!("keys {:?} and {:?}", keys(x), keys(y)));
            }
            keys(x).iter().try_for_each(|k| {
                let (x, y) = (x.get(k).unwrap(), y.get(k).unwrap());
                alike((a, x), (b, y), followed)
            })
        };
        match (x.value(), y.value()) {
            (Value::String(s, _), Value::String(t, _)) if s == t => Ok(()),
            (Value::Array(s), Value::Array(t)) if s.len() == t.len() => {
                (s.iter().zip(t)).try_for_each(|(x, y)| alike((a, x), (b, y), followed))
            }
            (Value::Dict(s), Value::Dict(t)) => dicts(s, t, followed),
            (Value::Ref(i), Value::Ref(j)) => match followed.get(i) {
                Some(seen) if seen == j => Ok(()),
                Some(_) => differ(),
                None => {
                    followed.insert(*i, *j);
                    let (x, y) = (a.get(*i).unwrap(), b.get(*j).unwrap());
                    match (x.value(), y.value()) {
                        (Value::Stream(s), Value::Stream(t)) => {
                            dicts(&s.dict, &t.dict, followed)?;
                            let (s, t) = (
                                a.decoded_within(*i, usize::MAX).unwrap(),
                                b.decoded_within(*j, usize::MAX).unwrap(),
                            );
                            (s == t).then_some(()).ok_or(format!("{i:?}'s data"))
                        }
                        _ => alike((a, x), (b, y), followed),
                    }
                }
            },
            (x, y) if x == y => Ok(()),
            _ => differ(),
        }
    }

    /// A new file's encryption, as pdf-crypt creates it (revision 6,
    /// AES-256) and the test writer writes it, opens in qpdf by the
    /// password's SASLprep form, which qpdf takes as given, preparing no
    /// password (Ⅸ, set as `IX`, opens only by `IX`): its `--check` finds
    /// nothing wrong; `--show-encryption` says revision 6, AESv3 for
    /// strings, streams and embedded files, `/P -4` with every permission,
    /// the password the owner's and the user's, and the key drawn first;
    /// and `--decrypt` writes a file the reader reads, from the trailer's
    /// `/Root` and `/Info`, as it reads this one by the password as typed.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_new_files_encryption_opens_in_qpdf_and_decrypts_as_the_reader_reads_it() {
        use chacha20::ChaCha20Rng;
        use pdf_crypt::rand_core::{Rng, SeedableRng};

        let dir = scratch("created");
        let mut objects = clean();
        objects.push((6, dict! { "Title" => Object::text("A title") }.into()));
        let cases = [
            ("user", "user"),
            ("p\u{e4}ssw\u{f6}rd", "p\u{e4}ssw\u{f6}rd"),
            ("\u{2168}", "IX"),
        ];
        for (i, (typed, prepared)) in cases.into_iter().enumerate() {
            let password = Password(typed).prepared().unwrap();
            assert_eq!(password.as_bytes(), prepared.as_bytes(), "{typed}");
            let mut rng = ChaCha20Rng::seed_from_u64(6);
            let trailer = dict! { "Root" => Object::reference(1), "Info" => Object::reference(6) };
            let protection = pdf_crypt::Protection::new(&password);
            let file = fixture::created(&protection, &objects, trailer, &mut rng);
            let path = dir.path().join(format!("{i}.pdf"));
            std::fs::write(&path, &file).unwrap();
            let qpdf = qpdf(&path).password(prepared);
            let run = |args: &[&Path]| {
                let mut all = vec![format!("--password={prepared}")];
                all.extend(args.iter().map(|a| a.display().to_string()));
                qpdf.exec(&all).unwrap()
            };
            let check = run(&[Path::new("--check"), &path]);
            let said = String::from_utf8_lossy(&check.stderr).into_owned();
            assert_eq!((check.code, said.as_str()), (Some(0), ""), "{typed}");
            let shown = run(&[
                Path::new("--show-encryption"),
                Path::new("--show-encryption-key"),
                &path,
            ]);
            let shown = String::from_utf8_lossy(&shown.stdout).into_owned();
            let mut key = [0; 32];
            ChaCha20Rng::seed_from_u64(6).fill_bytes(&mut key);
            let key: String = key.iter().map(|b| format!("{b:02x}")).collect();
            let lines = [
                "R = 6",
                "P = -4",
                &format!("Encryption key = {key}"),
                "Supplied password is owner password",
                "Supplied password is user password",
                "modify anything: allowed",
                "stream encryption method: AESv3",
                "string encryption method: AESv3",
                "file encryption method: AESv3",
            ];
            for line in lines {
                assert!(shown.lines().any(|l| l == line), "{typed}: {line}\n{shown}");
            }
            assert_eq!(shown.matches(": allowed").count(), 9, "{typed}: {shown}");
            if typed != prepared {
                let as_typed = Qpdf::new(&path).password(typed);
                assert_eq!(as_typed.locked(), Ok(true), "{typed}: qpdf prepares none");
            }
            let decrypted = dir.path().join(format!("{i}-decrypted.pdf"));
            let wrote = run(&[
                Path::new("--decrypt"),
                Path::new("--compress-streams=n"),
                Path::new("--decode-level=none"),
                &path,
                &decrypted,
            ]);
            assert_eq!(wrote.code, Some(0), "{typed}");
            let options = read::Options::DEFAULT;
            let ours = read::Reading::open(
                file,
                &read::Options {
                    passwords: &[Password(typed)],
                    ..options
                },
            )
            .unwrap();
            let opened = crate::Encryption::Opened {
                owner: true,
                by: Some((0, password_by(typed, prepared))),
                permissions: pdf_crypt::Permissions::ALL,
            };
            assert_eq!(*ours.encryption(), opened, "{typed}");
            let theirs = std::fs::read(&decrypted).unwrap();
            let theirs = read::Reading::open(theirs, &options).unwrap();
            assert_eq!(*theirs.encryption(), crate::Encryption::None, "{typed}");
            let mut followed = BTreeMap::new();
            for key in [&b"Root"[..], b"Info"] {
                let x = ours.trailer().dict().get(key).unwrap();
                let y = theirs.trailer().dict().get(key).unwrap();
                let alike = alike((&ours, x), (&theirs, y), &mut followed);
                assert_eq!(alike, Ok(()), "{typed}");
            }
            assert_eq!(followed.len(), 6, "{typed}: every object followed");
        }
    }

    /// The encoding that opens a file set with `prepared` by `typed`: its
    /// SASLprep form where that's another text, else as typed.
    fn password_by(typed: &str, prepared: &str) -> pdf_crypt::Encoding {
        match typed == prepared {
            true => pdf_crypt::Encoding::Utf8,
            false => pdf_crypt::Encoding::Saslprep,
        }
    }

    /// qpdf 12.4.2's nine permission lines, as `--show-encryption` writes
    /// them, for a file whose handler says `set`, by qpdf's own reading of
    /// the bits: printing at high resolution where bits 3 and 12 are both
    /// set; forms where bit 9 is, though bit 6 grants filling them in too
    /// (pdf-crypt's `Permissions::ANNOTATE`); anything where the four
    /// changes' bits are. A revision 2 file's are by the bits that cover
    /// them there, as pdf-crypt's `Permissions::from_p` reads them.
    fn permission_lines(set: pdf_crypt::Permissions) -> Vec<String> {
        use pdf_crypt::{Permissions, Printing};

        let changes = Permissions::MODIFY
            | Permissions::ANNOTATE
            | Permissions::FILL_FORMS
            | Permissions::ASSEMBLE;
        [
            (
                "extract for accessibility",
                set.contains(Permissions::ACCESSIBILITY),
            ),
            ("extract for any purpose", set.contains(Permissions::COPY)),
            ("print low resolution", set.contains(Permissions::PRINT)),
            ("print high resolution", set.printing() == Printing::High),
            (
                "modify document assembly",
                set.contains(Permissions::ASSEMBLE),
            ),
            ("modify forms", set.contains(Permissions::FILL_FORMS)),
            ("modify annotations", set.contains(Permissions::ANNOTATE)),
            ("modify other", set.contains(Permissions::MODIFY)),
            ("modify anything", set.contains(changes)),
        ]
        .map(|(what, allowed)| match allowed {
            true => format!("{what}: allowed"),
            false => format!("{what}: not allowed"),
        })
        .to_vec()
    }

    /// The permission lines of what qpdf's `--show-encryption` wrote.
    fn shown_permissions(shown: &str) -> Vec<String> {
        (shown.lines())
            .filter(|l| l.ends_with(": allowed") || l.ends_with(": not allowed"))
            .map(String::from)
            .collect()
    }

    /// A new file protected by each combination of passwords (both apart,
    /// the user's alone, the owner's alone), under every permission, none,
    /// and a security dialog's levels of printing and of changes with
    /// copying withheld, as pdf-crypt creates it and the test writer
    /// writes it, opens in qpdf by each password as its protection says:
    /// `--check` finds nothing wrong, and `--show-encryption` says revision
    /// 6, `/P` the permissions with bit 10, the key drawn first, "Supplied
    /// password is owner password" for the owner's, "user password" for the
    /// user's, and both where the one password is both, and for no password
    /// where the user's is empty; and each of its nine permission lines as
    /// the handler the reader opens it with says the permissions, which are
    /// those written. A wrong password is "Incorrect password supplied",
    /// whatever the file.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn new_files_open_in_qpdf_by_each_password_with_the_permissions_they_say() {
        use chacha20::ChaCha20Rng;
        use pdf_crypt::rand_core::{Rng, SeedableRng};
        use pdf_crypt::{Changes, Permissions, Printing, Protection};

        let dir = scratch("protected");
        let user = Password("user").prepared().unwrap();
        let owner = Password("owner").prepared().unwrap();
        let level = |p: Printing, c: Changes| Permissions::from(p) | Permissions::from(c);
        let sets = [
            Permissions::ALL,
            Permissions::NONE,
            level(Printing::Low, Changes::Denied),
            level(Printing::High, Changes::Assemble),
            level(Printing::Denied, Changes::FillForms),
            level(Printing::Low, Changes::CommentAndFill),
            level(Printing::High, Changes::Any),
        ];
        let mut cases = vec![(Some(&user), None, Permissions::ALL)];
        for set in sets {
            cases.push((Some(&user), Some(&owner), set));
            cases.push((None, Some(&owner), set));
        }
        let owners = "Supplied password is owner password";
        let users = "Supplied password is user password";
        for (i, (u, o, permissions)) in cases.into_iter().enumerate() {
            let mut protection = Protection::DEFAULT;
            protection.user = u;
            protection.owner = o;
            protection.permissions = permissions;
            let what = format!("{protection:?}");
            let seed = i as u64;
            let mut rng = ChaCha20Rng::seed_from_u64(seed);
            let trailer = dict! { "Root" => Object::reference(1) };
            let file = fixture::created(&protection, &clean(), trailer, &mut rng);
            let path = dir.path().join(format!("{i}.pdf"));
            std::fs::write(&path, &file).unwrap();
            let mut key = [0; 32];
            ChaCha20Rng::seed_from_u64(seed).fill_bytes(&mut key);
            let key: String = key.iter().map(|b| format!("{b:02x}")).collect();
            let written = permissions | Permissions::ACCESSIBILITY;
            // Each password, and what qpdf says of it.
            let mut opens: Vec<(&str, Vec<&str>)> = Vec::new();
            if o.is_some() {
                opens.push(("owner", vec![owners]));
            }
            match (u, o) {
                (Some(_), Some(_)) => opens.push(("user", vec![users])),
                (Some(_), None) => opens.push(("user", vec![owners, users])),
                (None, _) => opens.push(("", vec![users])),
            }
            let qpdf = qpdf(&path);
            let run = |password: &str, args: &[&str]| {
                let mut all = vec![format!("--password={password}")];
                all.extend(args.iter().map(|a| a.to_string()));
                all.push(path.display().to_string());
                qpdf.exec(&all).unwrap()
            };
            for (password, said) in opens {
                let what = format!("{what}: {password:?}");
                let check = run(password, &["--check"]);
                let err = String::from_utf8_lossy(&check.stderr).into_owned();
                assert_eq!((check.code, err.as_str()), (Some(0), ""), "{what}");
                let shown = run(password, &["--show-encryption", "--show-encryption-key"]);
                let shown = String::from_utf8_lossy(&shown.stdout).into_owned();
                // What qpdf said, for a run with --nocapture to read.
                println!("{what}\n{shown}");
                let p = format!("P = {}", written.p());
                let key = format!("Encryption key = {key}");
                for line in ["R = 6", &p, &key, "file encryption method: AESv3"] {
                    assert!(shown.lines().any(|l| l == line), "{what}: {line}\n{shown}");
                }
                let supplied: Vec<&str> = (shown.lines())
                    .filter(|l| l.starts_with("Supplied password is"))
                    .collect();
                assert_eq!(supplied, said, "{what}\n{shown}");
                let typed: &[Password] = match password {
                    "" => &[],
                    _ => &[Password(password)],
                };
                let (_, handler) = read::read_opened_by(&file, typed);
                let ours = handler.unwrap().permissions();
                assert_eq!(ours, written, "{what}");
                let lines = shown_permissions(&shown);
                assert_eq!(lines, permission_lines(ours), "{what}\n{shown}");
            }
            let wrong = run("wrong", &["--show-encryption"]);
            let wrong = String::from_utf8_lossy(&wrong.stdout).into_owned();
            let first = wrong.lines().next();
            assert_eq!(first, Some("Incorrect password supplied"), "{what}");
        }
    }

    /// The `/P` of the encryption dictionary `doc`'s trailer refers to.
    fn p_of(doc: &Document) -> Option<i64> {
        let at = doc.trailer.dict().get(b"Encrypt")?.as_reference()?;
        doc.get(at).ok()?.as_dict()?.get(b"P")?.as_integer()
    }

    /// The files qpdf 12.4.2 encrypts at each of its levels of printing,
    /// changes and copying (`--encrypt user owner 256` with `--print`,
    /// `--modify` and `--extract`) read back as the levels of a security
    /// dialog say: `/P` their permissions with bit 10, as pdf-crypt's
    /// `Permissions::p` writes them, and the handler opened by the user's
    /// password, as the user's, and by the owner's, as the owner's, saying
    /// those permissions.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn qpdfs_levels_read_back_as_a_security_dialogs() {
        use pdf_crypt::{Changes, Encoding, Permissions, Printing};

        let printing = [
            (Printing::Denied, "none"),
            (Printing::Low, "low"),
            (Printing::High, "full"),
        ];
        let changes = [
            (Changes::Denied, "none"),
            (Changes::Assemble, "assembly"),
            (Changes::FillForms, "form"),
            (Changes::CommentAndFill, "annotate"),
            (Changes::Any, "all"),
        ];
        for (printing, print) in printing {
            for (changes, modify) in changes {
                for (copy, extract) in [(Permissions::COPY, "y"), (Permissions::NONE, "n")] {
                    let print = format!("--print={print}");
                    let modify = format!("--modify={modify}");
                    let extract = format!("--extract={extract}");
                    let args = [
                        "--encrypt",
                        "user",
                        "owner",
                        "256",
                        &print,
                        &modify,
                        &extract,
                        "--",
                    ];
                    let (_dir, path) = rewritten("levels", &args);
                    let data = std::fs::read(&path).unwrap();
                    let set = Permissions::from(printing)
                        | Permissions::from(changes)
                        | copy
                        | Permissions::ACCESSIBILITY;
                    for (password, owner) in [("user", false), ("owner", true)] {
                        let what = format!("{args:?}: {password}");
                        let (doc, handler) = read::read_opened_by(&data, &[Password(password)]);
                        let by = Some((0, Encoding::Utf8));
                        // What `/P` grants a user, whoever opened it.
                        let opened = crate::Encryption::Opened {
                            owner,
                            by,
                            permissions: set,
                        };
                        assert_eq!(doc.encryption, opened, "{what}");
                        assert_eq!(p_of(&doc), Some(set.p()), "{what}");
                        assert_eq!(handler.unwrap().permissions(), set, "{what}");
                        println!("{what}: /P {}, {set}", set.p());
                    }
                }
            }
        }
    }

    /// The test writer's dictionaries of each revision, 2 to 6, under a
    /// grid of `/P`s, read in qpdf as the handler reads them: each of
    /// qpdf's nine permission lines, opened by the user's password, as the
    /// handler's permissions say, which are `/P`'s by the revision's
    /// meaning of its bits (pdf-crypt's `Permissions::from_p`).
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn the_writers_p_reads_in_qpdf_as_the_handler_reads_it() {
        use pdf_crypt::Permissions;
        use pdf_crypt::writer::{Kind, Settings};

        let dir = scratch("p");
        let kinds = [
            Kind::Rc4By40,
            Kind::Rc4By128,
            Kind::Aes128,
            Kind::Aes256R5,
            Kind::Aes256,
        ];
        let grid = [-4, -1, -3904, -60, -3388, -28, -2092, -260, -516, -2052];
        for (k, kind) in kinds.into_iter().enumerate() {
            for (i, p) in grid.into_iter().enumerate() {
                let what = format!("{kind:?}, /P {p}");
                let settings = Settings {
                    kind,
                    user: b"user",
                    owner: b"owner",
                    p,
                    id: b"0123456789abcdef",
                    encrypt_metadata: true,
                };
                let trailer = dict! { "Root" => Object::reference(1) };
                let data = fixture::encrypted(&settings, &clean(), trailer);
                let path = dir.path().join(format!("{k}-{i}.pdf"));
                std::fs::write(&path, &data).unwrap();
                let (doc, handler) = read::read_opened_by(&data, &[Password("user")]);
                let ours = handler.unwrap().permissions();
                let r = match kind {
                    Kind::Rc4By40 => 2,
                    Kind::Rc4By128 => 3,
                    Kind::Aes128 => 4,
                    Kind::Aes256R5 => 5,
                    _ => 6,
                };
                assert_eq!(ours, Permissions::from_p(p, r), "{what}");
                assert_eq!(p_of(&doc), Some(p), "{what}");
                let args = ["--password=user", "--show-encryption"].map(String::from);
                let mut args = args.to_vec();
                args.push(path.display().to_string());
                let shown = qpdf(&path).exec(&args).unwrap();
                let shown = String::from_utf8_lossy(&shown.stdout).into_owned();
                assert!(shown.contains(&format!("R = {r}\n")), "{what}\n{shown}");
                let lines = shown_permissions(&shown);
                println!("{what}: {ours}\n{}", lines.join("\n"));
                assert_eq!(lines, permission_lines(ours), "{what}\n{shown}");
            }
        }
    }

    /// What reading through qpdf alone loses: where anything was written,
    /// any key written twice, which encoding a text string had, and whether
    /// a string was written literal or hex. `a` with these left out, as `b`
    /// has them, compared with `b`: the first difference, what kind it is
    /// and where, or none. `nothing` says whether a value of `a` is null,
    /// or a reference to no object, which qpdf reads as null.
    fn difference(
        a: &Object,
        b: &Object,
        at: &str,
        nothing: &dyn Fn(&Object) -> bool,
    ) -> Option<(String, String)> {
        let kind = |v: &Value| match v {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Integer(_) => "an integer",
            Value::Real(_) => "a real",
            Value::Name(_) => "a name",
            Value::String(Str::Bytes(_), _) => "bytes",
            Value::String(Str::Text(_), _) => "text",
            Value::String(Str::Encrypted(_), _) => "ciphertext",
            Value::Array(_) => "an array",
            Value::Dict(_) => "a dictionary",
            Value::Stream(_) => "a stream",
            Value::Ref(_) => "a reference",
        };
        let differ = || {
            let kinds = format!("{} and {}", kind(a.value()), kind(b.value()));
            Some((kinds, format!("{at}: {:?} and {:?}", a.value(), b.value())))
        };
        let explained = |kind: &str| {
            Some((
                kind.to_string(),
                format!("{at}: {:?} and {:?}", a.value(), b.value()),
            ))
        };
        // Two dictionaries, or two streams' dictionaries.
        let dicts = |x: &Dict, y: &Dict| {
            let keys = |d: &Dict| {
                d.iter()
                    .map(|(k, _)| k.to_vec())
                    .collect::<std::collections::BTreeSet<_>>()
            };
            let name = |k: &Vec<u8>| format!("/{}", String::from_utf8_lossy(k));
            let (only_x, only_y): (Vec<_>, Vec<_>) = (
                keys(x).difference(&keys(y)).cloned().collect(),
                keys(y).difference(&keys(x)).cloned().collect(),
            );
            if only_y.is_empty()
                && !only_x.is_empty()
                && only_x.iter().all(|k| x.get(k).is_some_and(nothing))
            {
                return Some((
                    "keys qpdf leaves out, whose values are null or refer to no object".to_string(),
                    format!(
                        "{at}: {}",
                        only_x.iter().map(name).collect::<Vec<_>>().join(" ")
                    ),
                ));
            }
            if !only_x.is_empty() || !only_y.is_empty() {
                let names = |keys: &[Vec<u8>]| keys.iter().map(name).collect::<Vec<_>>().join(" ");
                let kind = format!(
                    "keys only the bytes give [{}], only qpdf [{}]",
                    names(&only_x),
                    names(&only_y)
                );
                return Some((kind, at.to_string()));
            }
            (x.last_entries()).find_map(|e| {
                let name = String::from_utf8_lossy(&e.key);
                difference(&e.value, y.get(&e.key)?, &format!("{at}/{name}"), nothing)
            })
        };
        match (a.value(), b.value()) {
            (Value::String(Str::Bytes(x), _), Value::String(Str::Text(y), _)) => {
                if &text::decode(x).text == y {
                    None
                } else if x.starts_with(&[0xFF, 0xFE]) {
                    explained(
                        "a string after FF FE, which qpdf reads as UTF-16LE, and the spec as PDFDocEncoding",
                    )
                } else {
                    differ()
                }
            }
            (Value::String(x, _), Value::String(y, _)) => (x != y).then(differ).flatten(),
            (Value::Ref(_), Value::Null) if nothing(a) => {
                explained("a reference to no object, which qpdf writes as null")
            }
            (Value::Integer(_), Value::Integer(_))
                if a.value() != b.value() && at.ends_with("/Length") =>
            {
                explained("a /Length qpdf recovered, which it writes for the one written")
            }
            (Value::Array(x), Value::Array(y)) => {
                if x.len() != y.len() {
                    let lengths = format!("{at}: {} and {} elements", x.len(), y.len());
                    return Some(("arrays of other lengths".to_string(), lengths));
                }
                (x.iter().zip(y).enumerate())
                    .find_map(|(i, (x, y))| difference(x, y, &format!("{at}[{i}]"), nothing))
            }
            (Value::Dict(x), Value::Dict(y)) => dicts(x, y),
            (Value::Stream(x), Value::Stream(y)) => dicts(&x.dict, &y.dict),
            (x, y) => (x != y).then(differ).flatten(),
        }
    }

    /// The differences between `a`, read from its bytes, and `b`, read
    /// through qpdf alone, each with what kind it is.
    fn differences(a: &Document, b: &Document) -> Vec<(String, String)> {
        let nothing = |o: &Object| match o.value() {
            Value::Null => true,
            Value::Ref(id) => a.get(*id).map_or(true, Object::is_null),
            _ => false,
        };
        let is_xref = |o: &Object| match o.value() {
            Value::Stream(s) => s.dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef"),
            _ => false,
        };
        let mut found: Vec<(String, String)> = Vec::new();
        let mut push = |kind: &str, d: String| found.push((kind.to_string(), d));
        for id in a.ids() {
            match (a.get(id), b.contains(id).then(|| b.get(id))) {
                (_, None) => push("only the bytes list it", format!("{id} R")),
                (Ok(x), Some(Ok(y))) => {
                    if let Some((kind, d)) = difference(x, y, &format!("{id} R"), &nothing) {
                        let kind = match is_xref(x) && d.contains("/ID[") {
                            true => "an older cross-reference stream's /ID, which isn't encrypted, and qpdf decrypts".to_string(),
                            false => format!("value: {kind}"),
                        };
                        push(&kind, d);
                    }
                }
                (Err(why), Some(Ok(y))) if y.is_null() => {
                    push("only qpdf reads it, as null", format!("{id} R: {why}"))
                }
                (Err(why), Some(Ok(_))) => push("only qpdf reads it", format!("{id} R: {why}")),
                (Ok(_), Some(Err(why))) => push("only the bytes read it", format!("{id} R: {why}")),
                (Err(_), Some(Err(_))) => {}
            }
        }
        for id in b.ids().into_iter().filter(|&id| !a.contains(id)) {
            let kind = match b.get(id) {
                Ok(y) if is_xref(y) => "only qpdf lists it: a cross-reference stream",
                Ok(y) if y.is_null() => "only qpdf lists it, as null",
                _ => "only qpdf lists it",
            };
            push(kind, format!("{id} R"));
        }
        if a.trailer.stream != b.trailer.stream {
            let streams = format!("{:?} and {:?}", a.trailer.stream, b.trailer.stream);
            push("trailer stream", streams);
        }
        let trailer = difference(&a.trailer.object, &b.trailer.object, "trailer", &nothing);
        if let Some((kind, d)) = trailer {
            push(&format!("trailer: {kind}"), d);
        }
        if a.header != b.header {
            push("header", format!("{:?} and {:?}", a.header, b.header));
        }
        found
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_file_reads_through_qpdf_alone_as_through_its_bytes() {
        for test in [
            "qpdf-plain",
            "qpdf-compressed",
            "qpdf-aes-256",
            "qpdf-aes-256-unpacked",
        ] {
            let path = fixture(test);
            let data = std::fs::read(&path).unwrap();
            let qpdf = qpdf(&path);
            let bytes = read::read_with(&data, &qpdf);
            let (alone, heard) = qpdf.heard().unwrap();
            assert_eq!(differences(&bytes, &alone), [], "{test}");
            assert!(heard.is_empty(), "{test}: {heard:?}");
            assert_eq!(alone.trailer.unwritten, None, "{test}");
            assert!(
                alone.objects().all(|(_, o)| o.at.is_none()),
                "{test}: qpdf says nowhere"
            );
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn what_qpdf_keeps_and_loses_reading_alone() {
        let dir = scratch("loses");
        let path = dir.path().join("odd.pdf");
        let catalog = b"<< /Type /Catalog /Pages 2 0 R /Odd#20Name 1 /Bin#FFx 2 /Dup 1 /Dup 2 /Utf16 <FEFF00410042> /Real +.5 >>";
        let pages = b"<< /Type /Pages /Kids [] /Count 0 >>";
        let file = fixture::assembled(
            "1.7",
            &[(1, &catalog[..]), (2, &pages[..])],
            "<< /Size 3 /Root 1 0 R >>",
        );
        std::fs::write(&path, &file).unwrap();
        let (alone, heard) = qpdf(&path).heard().unwrap();
        let catalog = alone.get(ObjectId::new(1, 0)).unwrap().as_dict().unwrap();
        assert!(catalog.has(b"Odd Name") && catalog.has(b"Bin\xFFx"));
        assert_eq!(
            catalog.entries().iter().filter(|e| e.key == b"Dup").count(),
            1
        );
        assert_eq!(catalog.get(b"Dup").and_then(Object::as_integer), Some(2));
        assert_eq!(
            *catalog.get(b"Utf16").unwrap().value(),
            Value::String(Str::Text("AB".to_string()), None),
            "text, its encoding lost"
        );
        assert_eq!(catalog.get(b"Real").and_then(Object::as_number), Some(0.5));
        assert!(
            (heard.iter()).any(|(object, message)| *object == Some(ObjectId::new(1, 0))
                && message.contains("duplicated key /Dup")),
            "{heard:?}"
        );
        let bytes = read::document(&file, &read::Options::DEFAULT);
        assert_eq!(differences(&bytes, &alone), []);

        // A file qpdf can't read at all says why.
        let unreadable = dir.path().join("unreadable.pdf");
        std::fs::write(&unreadable, b"not a PDF").unwrap();
        let why = qpdf(&unreadable).alone().unwrap_err();
        assert!(why.contains("exited with 2"), "{why}");

        // Nor what qpdf reconstructs is as the file writes it.
        let damaged = dir.path().join("damaged.pdf");
        let mut broken = fixture::file(
            "1.7",
            &clean(),
            dict! { "Root" => Object::reference(1) },
            Xref::Table,
        );
        let at = broken.windows(9).rposition(|w| w == b"startxref").unwrap();
        broken.truncate(at);
        broken.extend_from_slice(b"startxref\n999999\n%%EOF\n");
        std::fs::write(&damaged, &broken).unwrap();
        let (alone, heard) = qpdf(&damaged).heard().unwrap();
        assert!(alone.trailer.unwritten.is_some(), "{heard:?}");
        assert!(alone.contains(ObjectId::new(1, 0)));
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_cross_reference_stream_whose_number_an_update_reuses_decodes_from_its_bytes() {
        let path = fixture("qpdf-compressed");
        let mut data = std::fs::read(&path).unwrap();
        let old = read::document(&data, &read::Options::DEFAULT);
        let ObjectId { number: x, .. } = old.trailer.stream.expect("a cross-reference stream");
        let root = old
            .trailer
            .dict()
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();
        let size = old
            .trailer
            .dict()
            .get(b"Size")
            .unwrap()
            .as_integer()
            .unwrap();
        let prev = {
            let at = data.windows(9).rposition(|w| w == b"startxref").unwrap();
            let rest = String::from_utf8_lossy(&data[at + 9..]).into_owned();
            rest.split_whitespace().next().unwrap().to_string()
        };
        // An update that gives the stream's number to another stream.
        let offset = data.len();
        data.extend_from_slice(
            format!("{x} 0 obj\n<< /Length 11 /Filter /ASCIIHexDecode >>\nstream\n3132333435>\nendstream\nendobj\n").as_bytes(),
        );
        let xref = data.len();
        data.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \n{x} 1\n{offset:010} 00000 n \ntrailer\n<< /Size {size} /Root {} 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n",
                root.number
            )
            .as_bytes(),
        );
        let dir = scratch("reused");
        let updated = dir.path().join("updated.pdf");
        std::fs::write(&updated, &data).unwrap();
        let qpdf = qpdf(&updated);
        assert_eq!(
            qpdf.decode(&[ObjectId::new(x, 0)])
                .remove(&ObjectId::new(x, 0))
                .unwrap(),
            Ok(b"12345".to_vec()),
            "by its number, qpdf decodes the other stream"
        );
        let doc = read::read_with(&data, &qpdf);
        assert!(doc.faults().is_empty(), "{:?}", doc.faults());
        assert_eq!(doc.trailer.unwritten, None);
        assert_eq!(differences(&doc, &qpdf.alone().unwrap()), []);
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn an_encrypted_files_strings_in_plain_view_decrypt_to_their_bytes_and_through_qpdf() {
        let path = fixture("qpdf-aes-256-unpacked");
        let data = std::fs::read(&path).unwrap();
        let contents = |doc: &Document| {
            let annot = (doc.objects().map(|(_, o)| o))
                .find(|o| {
                    o.as_dict().is_some_and(|d| {
                        d.get(b"Subtype").and_then(Object::as_name) == Some(b"Text")
                    })
                })
                .expect("the annotation reads");
            annot.as_dict().unwrap().get(b"Contents").unwrap().clone()
        };
        // Decrypted by the reader, its bytes; through qpdf, as qpdf's JSON
        // gives it, text, its encoding unsaid.
        let through = read::read_through(&data, &qpdf(&path));
        assert!(through.faults().is_empty(), "{:?}", through.faults());
        assert_eq!(
            *contents(&through).value(),
            Value::String(Str::Text("A note".to_string()), Some(Form::Hex))
        );
        let doc = read::document(&data, &read::Options::DEFAULT);
        assert!(doc.faults().is_empty(), "{:?}", doc.faults());
        let opened = crate::Encryption::Opened {
            owner: false,
            by: None,
            permissions: pdf_crypt::Permissions::ALL,
        };
        assert_eq!(doc.encryption, opened);
        let contents = contents(&doc);
        assert_eq!(
            *contents.value(),
            Value::String(Str::Bytes(b"A note".to_vec()), Some(Form::Hex)),
            "its bytes"
        );
        let at = contents.at.unwrap();
        assert_eq!(at.stream, None, "in plain view");
        assert!(
            !data[at.start..at.end].windows(6).any(|w| w == b"A note"),
            "placed on its ciphertext"
        );
        // A password that doesn't open it, though its user password is
        // empty, is all that's tried: it's locked, and the string stays
        // ciphertext, where it is.
        let doc = read::document(
            &data,
            &read::Options {
                passwords: &[Password("wrong")],
                ..read::Options::DEFAULT
            },
        );
        assert_eq!(doc.encryption, crate::Encryption::Locked);
        let annot = (doc.objects().map(|(_, o)| o))
            .find(|o| {
                o.as_dict()
                    .is_some_and(|d| d.get(b"Contents").is_some_and(|c| c.at == Some(at)))
            })
            .expect("the annotation reads");
        let contents = annot.as_dict().unwrap().get(b"Contents").unwrap();
        assert!(matches!(
            *contents.value(),
            Value::String(Str::Encrypted(_), _)
        ));
    }

    /// The writer's sections and object streams read through qpdf alone as
    /// the reader reads them, each section sound: a whole file's
    /// cross-reference stream deflated under Up; a table linking its free
    /// entries, and an update after it freeing an object; a stream listing
    /// objects in an object stream, and an update after it with 100 bytes
    /// before the header, which qpdf and the reader count positions from
    /// (ISO 32000-2, 7.5.2), the reader saying where the header is and
    /// nothing else; and an update after that file as a reading of it
    /// describes it (`Base::of`), read so.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn written_sections_and_containers_read_through_qpdf_as_through_the_reader() {
        use crate::write::{self, Deflate, Entry, Section};
        use pdf_objects::serialize;

        // A catalog, and the page tree qpdf needs, as object 2.
        let catalog =
            || Object::from(dict! { "Type" => "Catalog", "Pages" => ObjectId::new(2, 0) });
        let pages = || {
            Object::from(dict! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 })
        };
        let root = || dict! { "Root" => ObjectId::new(1, 0) };
        let deflate = Deflate::DEFAULT;
        // Objects one after another, each listed where it starts.
        let objects = |out: &mut Vec<u8>, section: &mut Section, objects: Vec<(u32, Object)>| {
            for (num, obj) in objects {
                let pos = serialize::indirect(out, ObjectId::new(num, 0), &obj).unwrap();
                section.insert(
                    num,
                    Entry::InUse {
                        offset: pos,
                        generation: 0,
                    },
                );
            }
        };
        let packed = |members: &[(u32, Object)]| {
            let members: Vec<(u32, Vec<u8>)> = (members.iter())
                .map(|(num, obj)| {
                    let mut bytes = Vec::new();
                    serialize::write(&mut bytes, obj).unwrap();
                    (*num, bytes)
                })
                .collect();
            Object::from(write::container(&members, deflate).unwrap().unwrap())
        };
        let mut files = Vec::new();

        let mut up = b"%PDF-1.7\n".to_vec();
        let mut section = Section::full(0);
        let numbered = (1..=200).map(|n| match n {
            1 => (n, catalog()),
            2 => (n, pages()),
            n => (n, Object::from(n)),
        });
        objects(&mut up, &mut section, numbered.collect());
        let pos = up.len();
        section.stream(&mut up, 201, root(), pos, deflate).unwrap();
        files.push(("up", up, 0));

        let mut freed = b"%PDF-1.7\n".to_vec();
        let mut section = Section::full(0);
        let three = vec![(1, catalog()), (2, pages()), (3, Object::from(3))];
        objects(&mut freed, &mut section, three);
        let prev = freed.len();
        section.table(&mut freed, root(), prev).unwrap();
        let mut section = Section::append(section.size(), 0);
        let mut marked = catalog();
        marked.as_dict_mut().unwrap().set("Marked", true);
        objects(
            &mut freed,
            &mut section,
            vec![(1, marked), (5, Object::from(5))],
        );
        section.insert(3, Entry::Free { generation: 1 });
        let pos = freed.len();
        let trailer = dict! { "Root" => ObjectId::new(1, 0), "Prev" => prev as i64 };
        section.table(&mut freed, trailer, pos).unwrap();
        files.push(("freed", freed, 0));

        let five = Object::bytes("five");
        let six = Object::from(dict! { "Six" => 6 });
        let mut packs = b"%PDF-1.7\n".to_vec();
        let mut section = Section::full(0);
        let container = packed(&[(5, five), (6, six)]);
        objects(
            &mut packs,
            &mut section,
            vec![(1, catalog()), (2, pages()), (7, container)],
        );
        for (num, index) in [(5, 0), (6, 1)] {
            section.insert(num, Entry::Compressed { stream: 7, index });
        }
        let prev = packs.len();
        section
            .stream(&mut packs, 8, root(), prev, deflate)
            .unwrap();
        let mut file = [vec![b'~'; 100], packs.clone()].concat();
        // The same file updated after a reading of it, as `Base::of`
        // describes it: its header past the bytes before it.
        let mut by_base = file.clone();
        let reading = read::Reading::open(&by_base[..], &read::Options::DEFAULT).unwrap();
        let base = write::Base::of(&reading).unwrap();
        assert_eq!(base.header, 100);
        let marked = Object::from(
            dict! { "Type" => "Catalog", "Pages" => ObjectId::new(2, 0), "Marked" => true },
        );
        let given = [(ObjectId::new(1, 0), write::Body::from(&marked))];
        let options = write::Options::DEFAULT;
        write::update(&mut by_base, &base, given, None, None, &options).unwrap();
        files.push(("prefixed, by its base", by_base, 100));
        files.push(("packed", packs, 0));
        let mut section = Section::append(9, 100);
        let sixty = packed(&[(6, Object::from(dict! { "Six" => 60 }))]);
        objects(&mut file, &mut section, vec![(10, sixty)]);
        section.insert(
            6,
            Entry::Compressed {
                stream: 10,
                index: 0,
            },
        );
        let pos = file.len();
        let trailer = dict! { "Root" => ObjectId::new(1, 0), "Prev" => prev as i64 };
        section
            .stream(&mut file, 11, trailer, pos, deflate)
            .unwrap();
        files.push(("prefixed", file, 100));

        let dir = scratch("written");
        for (test, data, header) in files {
            let path = dir.path().join(format!("{test}.pdf"));
            std::fs::write(&path, &data).unwrap();
            let (alone, heard) = qpdf(&path).heard().unwrap();
            assert_eq!(alone.trailer.unwritten, None, "{test}");
            assert!(heard.is_empty(), "{test}: {heard:?}");
            let doc = read::document(&data, &read::Options::DEFAULT);
            assert_eq!(doc.trailer.unwritten, None, "{test}");
            let said: Vec<crate::Fault> = (doc.faults().iter())
                .map(|(_, fault)| fault.clone())
                .collect();
            let want = match header {
                0 => vec![],
                at => vec![crate::Fault::HeaderAt { at }],
            };
            assert_eq!(said, want, "{test}");
            assert_eq!(differences(&doc, &alone), [], "{test}");
        }
    }

    /// An update after a reading that read the file's sections or objects
    /// where the file proves they are (`Base::of`): a cut file, its main
    /// section and an object past the cut three bytes before where they're
    /// said; one whose offsets count from its first byte, 100 bytes before
    /// its header; one whose `startxref` gives the line after `xref`; and
    /// one whose rows are each two bytes past their objects. The update
    /// writes its `/Prev` where the newest section was read and lists each
    /// object read at a shift where it is: read through qpdf alone as the
    /// reader reads it, qpdf saying nothing where it says nothing of the
    /// file before the update, and reconstructing only where it does that
    /// of the file too (the cut's first section still gives the main
    /// section three bytes off); the reader telling only what it finds of
    /// the file before the update.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn an_update_after_a_recovered_reading_reads_through_qpdf_as_through_the_reader() {
        use crate::read::recovery::tests::{OBJECTS, cut, laid_out};
        use crate::write;

        let files = [
            ("cut", cut().0),
            (
                "from the first byte",
                laid_out(&[b'~'; 100], &OBJECTS, &|_, at| at, &|at| at).0,
            ),
            (
                "startxref past",
                laid_out(b"", &OBJECTS, &|_, at| at, &|at| at + 5).0,
            ),
            (
                "rows past",
                laid_out(b"", &OBJECTS, &|_, at| at + 2, &|at| at).0,
            ),
        ];
        let dir = scratch("recovered");
        for (test, file) in files {
            let reading = read::Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
            let base = write::Base::of(&reading).unwrap();
            let marked = Object::from(
                dict! { "Type" => "Catalog", "Pages" => ObjectId::new(2, 0), "Marked" => true },
            );
            let given = [(ObjectId::new(1, 0), write::Body::from(&marked))];
            let mut data = file.clone();
            write::update(
                &mut data,
                &base,
                given,
                None,
                None,
                &write::Options::DEFAULT,
            )
            .unwrap();
            let path = dir.path().join(format!("{test}.pdf"));
            std::fs::write(&path, &data).unwrap();
            let (alone, heard) = qpdf(&path).heard().unwrap();
            let before = dir.path().join(format!("{test}, before.pdf"));
            std::fs::write(&before, &file).unwrap();
            let (as_was, _) = qpdf(&before).heard().unwrap();
            if as_was.trailer.unwritten.is_none() {
                assert_eq!(alone.trailer.unwritten, None, "{test}");
                assert!(heard.is_empty(), "{test}: {heard:?}");
            }
            let doc = read::document(&data, &read::Options::DEFAULT);
            assert_eq!(doc.trailer.unwritten, None, "{test}");
            let recovery = |fault: &crate::Fault| {
                matches!(
                    fault,
                    crate::Fault::HeaderAt { .. }
                        | crate::Fault::Moved { .. }
                        | crate::Fault::Shifted { .. }
                )
            };
            assert!(doc.faults().iter().all(|(_, f)| recovery(f)), "{test}");
            assert_eq!(differences(&doc, &alone), [], "{test}");
        }
    }

    /// The writer's encrypted files, under every method, with a table and
    /// with a stream, checked by qpdf by their password ("No syntax or
    /// stream encoding errors found") and read through qpdf alone as the
    /// reader reads them: the same objects, and the same values but a
    /// signature's `/Contents` with no `/Type`, which the reader and the
    /// writer leave in the clear, as it has a `/ByteRange` (7.6.2), and
    /// qpdf decrypts, telling a signature by `/Type /Sig` alone.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn written_encrypted_files_read_through_qpdf_as_through_the_reader() {
        use crate::XrefKind;
        use crate::write::tests::{encrypted_by, listed, methods};

        let (plain, trailer) = listed();
        let dir = scratch("encrypted");
        for method in methods() {
            for kind in [XrefKind::Table, XrefKind::Stream] {
                let name = format!("{}, {kind:?}", method.name);
                let data = encrypted_by(&method.encryptor, &plain, &trailer, kind);
                let path = dir
                    .path()
                    .join(format!("{}-{kind:?}.pdf", method.name.replace(' ', "-")));
                std::fs::write(&path, &data).unwrap();
                let qpdf = qpdf(&path).password("user");
                let check = ["--password=user", "--check", &path.display().to_string()];
                let (out, _) = qpdf.run(&check.map(String::from)).unwrap();
                let out = String::from_utf8_lossy(&out);
                assert!(
                    out.contains("No syntax or stream encoding errors found"),
                    "{name}: {out}"
                );
                let (alone, heard) = qpdf.heard().unwrap();
                assert!(heard.is_empty(), "{name}: {heard:?}");
                let doc = read::document(
                    &data,
                    &read::Options {
                        passwords: &[Password("user")],
                        ..read::Options::DEFAULT
                    },
                );
                assert!(doc.faults().is_empty(), "{name}: {:?}", doc.faults());
                // The untyped signature's /Contents, where strings are
                // encrypted: qpdf decrypts it.
                let found = differences(&doc, &alone);
                assert_eq!(
                    found.len(),
                    usize::from(method.strings),
                    "{name}: {found:?}"
                );
                let untyped = |(_, d): &(String, String)| d.starts_with("10 0 R/Contents: ");
                assert!(found.iter().all(untyped), "{name}: {found:?}");
            }
        }
    }

    /// Whole files and updates the writer writes, plain and under every
    /// method: [`listed`](crate::write::tests::listed)'s objects whole with
    /// a table and with a stream, packed, and as an update rewriting every
    /// object of a file of them; checked by qpdf by their password ("No
    /// syntax or stream encoding errors found") and read through qpdf alone,
    /// without reconstructing, as the reader reads them, but a signature's
    /// `/Contents` with no `/Type`, as above. And an update of a file with
    /// 100 bytes before its header after a reading of it (`Base::of`), its
    /// positions counted from there: read by qpdf as its sections say, as
    /// the reader reads it, saying where the header is and nothing else;
    /// past them, the update of the file without them.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn written_whole_files_and_updates_read_through_qpdf_as_through_the_reader() {
        use crate::XrefKind;
        use crate::read::{Options, Reading};
        use crate::write::tests::{encrypted_by, listed, methods};
        use crate::write::{self, Base, Body, Encryptor, Pack};

        let (plain, trailer) = listed();
        let bodies = || {
            (plain.iter())
                .map(|(n, o)| (ObjectId::new(*n, 0), Body::from(o)))
                .collect::<Vec<_>>()
        };
        let past = Some(ObjectId::new((plain.len() + 1) as u32, 0));
        let methods = methods();
        let encryptors =
            std::iter::once(("plain", None)).chain(methods.iter().map(|m| (m.name, Some(m))));
        let dir = scratch("whole");
        for (name, method) in encryptors {
            let encryptor = method.map(|m| &m.encryptor);
            let mut files = Vec::new();
            for (way, kind, pack) in [
                ("table", XrefKind::Table, None),
                ("packed", XrefKind::Stream, Some(Pack::DEFAULT)),
            ] {
                let mut options = write::Options::DEFAULT;
                (options.kind, options.pack, options.encrypt_at) = (kind, pack, past);
                let mut out = Vec::new();
                write::full(&mut out, bodies(), &trailer, encryptor, None, &options).unwrap();
                files.push((way, out));
            }
            let source = match encryptor {
                Some(e) => encrypted_by(e, &plain, &trailer, XrefKind::Table),
                None => {
                    let mut out = Vec::new();
                    let options = write::Options::DEFAULT;
                    write::full(&mut out, bodies(), &trailer, None, None, &options).unwrap();
                    out
                }
            };
            let reading = Reading::open(
                &source[..],
                &Options {
                    passwords: &[Password("user")],
                    ..Options::DEFAULT
                },
            )
            .unwrap();
            let base = Base::of(&reading).unwrap();
            let own = Encryptor::of(&reading).unwrap();
            let mut updated = source.clone();
            let options = write::Options::DEFAULT;
            write::update(&mut updated, &base, bodies(), own.as_ref(), None, &options).unwrap();
            files.push(("updated", updated));
            for (way, data) in files {
                let name = format!("{name}, {way}");
                let path = dir
                    .path()
                    .join(format!("{}.pdf", name.replace([' ', ','], "-")));
                std::fs::write(&path, &data).unwrap();
                let qpdf = qpdf(&path).password("user");
                let check = ["--password=user", "--check", &path.display().to_string()];
                let (out, _) = qpdf.run(&check.map(String::from)).unwrap();
                let out = String::from_utf8_lossy(&out);
                assert!(
                    out.contains("No syntax or stream encoding errors found"),
                    "{name}: {out}"
                );
                let (alone, heard) = qpdf.heard().unwrap();
                assert_eq!(alone.trailer.unwritten, None, "{name}");
                assert!(heard.is_empty(), "{name}: {heard:?}");
                let doc = read::document(
                    &data,
                    &read::Options {
                        passwords: &[Password("user")],
                        ..read::Options::DEFAULT
                    },
                );
                assert!(doc.faults().is_empty(), "{name}: {:?}", doc.faults());
                let found = differences(&doc, &alone);
                let strings = method.is_some_and(|m| m.strings);
                assert_eq!(found.len(), usize::from(strings), "{name}: {found:?}");
                let untyped = |(_, d): &(String, String)| d.starts_with("10 0 R/Contents: ");
                assert!(found.iter().all(untyped), "{name}: {found:?}");
            }
        }

        // Bytes before the header: the update counts from it, as qpdf does.
        let file = {
            let mut out = Vec::new();
            let options = write::Options::DEFAULT;
            write::full(&mut out, bodies(), &trailer, None, None, &options).unwrap();
            out
        };
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let base = Base::of(&reading).unwrap();
        let mut page = reading.get(ObjectId::new(4, 0)).unwrap().clone();
        page.as_dict_mut().unwrap().set("Rotate", 90);
        let given = || [(ObjectId::new(4, 0), Body::from(&page))];
        let options = write::Options::DEFAULT;
        let mut appended = file.clone();
        write::update(&mut appended, &base, given(), None, None, &options).unwrap();
        let mut prefixed = [&[b'~'; 100][..], &file].concat();
        let after = {
            let reading = Reading::open(&prefixed[..], &Options::DEFAULT).unwrap();
            Base::of(&reading).unwrap()
        };
        assert_eq!((after.header, after.prev), (100, base.prev));
        write::update(&mut prefixed, &after, given(), None, None, &options).unwrap();
        assert!(prefixed[100..] == appended[..]);
        let path = dir.path().join("prefixed.pdf");
        std::fs::write(&path, &prefixed).unwrap();
        let (alone, heard) = qpdf(&path).heard().unwrap();
        assert_eq!(alone.trailer.unwritten, None);
        assert!(heard.is_empty(), "{heard:?}");
        let doc = read::document(&prefixed, &read::Options::DEFAULT);
        let said: Vec<&crate::Fault> = doc.faults().iter().map(|(_, fault)| fault).collect();
        assert_eq!(said, [&crate::Fault::HeaderAt { at: 100 }]);
        assert_eq!(differences(&doc, &alone), []);
    }

    /// Whole files whose objects are copied as the bytes they were read
    /// from: the test writer's, spelled otherwise than this writer spells
    /// them (objects by hand, object stream members by hand, and
    /// [`plaintext`](crate::write::tests::plaintext) under each method it
    /// encrypts by), each written by its document and reading under its
    /// own encryption, with a table and packed; checked by qpdf by their
    /// password, and read through qpdf alone as the reader reads them, but
    /// a signature's `/Contents` with no `/Type`, as above.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn written_copies_read_through_qpdf_as_through_the_reader() {
        use crate::XrefKind;
        use crate::read::{Options, Reading};
        use crate::write::tests::{KINDS_ENCRYPTED, members_spelled, spelled, written_otherwise};
        use crate::write::{self, Encryptor, Pack};

        let mut sources = vec![
            ("spelled".to_string(), spelled(), false),
            ("members spelled".to_string(), members_spelled(), false),
        ];
        for kind in KINDS_ENCRYPTED {
            sources.push((format!("{kind:?}"), written_otherwise(kind, true), true));
        }
        let dir = scratch("copies");
        for (name, source, strings) in sources {
            let passwords = [Password("user")];
            let reading = Reading::open(
                &source[..],
                &Options {
                    passwords: &passwords,
                    ..Options::DEFAULT
                },
            )
            .unwrap();
            let doc = reading.to_document().unwrap();
            let own = Encryptor::of(&reading).unwrap();
            for (way, kind, pack) in [
                ("table", XrefKind::Table, None),
                ("packed", XrefKind::Stream, Some(Pack::DEFAULT)),
            ] {
                let name = format!("{name}, {way}");
                let mut options = write::Options::DEFAULT;
                (options.kind, options.pack) = (kind, pack);
                let mut data = Vec::new();
                write::document(&mut data, &doc, own.as_ref(), Some(&reading), &options).unwrap();
                let path = dir
                    .path()
                    .join(format!("{}.pdf", name.replace([' ', ','], "-")));
                std::fs::write(&path, &data).unwrap();
                let qpdf = qpdf(&path).password("user");
                let check = ["--password=user", "--check", &path.display().to_string()];
                let (out, _) = qpdf.run(&check.map(String::from)).expect(&name);
                let out = String::from_utf8_lossy(&out);
                assert!(
                    out.contains("No syntax or stream encoding errors found"),
                    "{name}: {out}"
                );
                let (alone, heard) = qpdf.heard().expect(&name);
                assert_eq!(alone.trailer.unwritten, None, "{name}");
                assert!(heard.is_empty(), "{name}: {heard:?}");
                let doc = read::document(
                    &data,
                    &read::Options {
                        passwords: &passwords,
                        ..read::Options::DEFAULT
                    },
                );
                assert!(doc.faults().is_empty(), "{name}: {:?}", doc.faults());
                let found = differences(&doc, &alone);
                assert_eq!(found.len(), usize::from(strings), "{name}: {found:?}");
                let untyped = |(_, d): &(String, String)| d.starts_with("10 0 R/Contents: ");
                assert!(found.iter().all(untyped), "{name}: {found:?}");
            }
        }
    }

    /// The files the renumbered files' and updates' tests write from, each
    /// named, and with whether a signature's `/Contents` with no `/Type` is
    /// left in the clear in it, which qpdf decrypts: [`listed`]'s objects
    /// plain and under every method, the test writer's spelled objects and
    /// members, and [`plaintext`] under each method it encrypts by.
    ///
    /// [`listed`]: crate::write::tests::listed
    /// [`plaintext`]: crate::write::tests::plaintext
    fn renumbered_sources() -> Vec<(String, Vec<u8>, bool)> {
        use crate::XrefKind;
        use crate::write::tests::{
            KINDS_ENCRYPTED, encrypted_by, listed, members_spelled, methods, spelled,
            written_otherwise,
        };
        use crate::write::{self, Body};

        let (plain, trailer) = listed();
        let plain_file = {
            let mut out = Vec::new();
            let bodies = (plain.iter()).map(|(n, o)| (ObjectId::new(*n, 0), Body::from(o)));
            let options = write::Options::DEFAULT;
            write::full(&mut out, bodies, &trailer, None, None, &options).unwrap();
            out
        };
        let mut sources = vec![
            ("listed".to_string(), plain_file, false),
            ("spelled".to_string(), spelled(), false),
            ("members spelled".to_string(), members_spelled(), false),
        ];
        for method in methods() {
            let data = encrypted_by(&method.encryptor, &plain, &trailer, XrefKind::Table);
            sources.push((format!("listed, {}", method.name), data, method.strings));
        }
        for kind in KINDS_ENCRYPTED {
            sources.push((format!("{kind:?}"), written_otherwise(kind, true), true));
        }
        sources
    }

    /// Whole files written under two renumberings, each object's number
    /// reversed (each at the number of the one as far from the end as it is
    /// from the start), and every other number kept, the rest moved past
    /// every number, by their document and reading under their own
    /// encryption, with a table and packed, copied where the rules hold:
    /// their headers and the references that move written anew in place,
    /// at new numbers where no key is made of the number: each of
    /// [`renumbered_sources`]. Checked by qpdf by their password ("No syntax
    /// or stream encoding errors found") and read through qpdf alone as the
    /// reader reads them, but a signature's `/Contents` with no `/Type`, as
    /// above, at the number it's written at.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn renumbered_files_read_through_qpdf_as_through_the_reader() {
        use crate::XrefKind;
        use crate::read::{Options, Reading};
        use crate::write::{self, Encryptor, Pack};

        let dir = scratch("renumbered");
        for (name, source, strings) in renumbered_sources() {
            let passwords = [Password("user")];
            let reading = Reading::open(
                &source[..],
                &Options {
                    passwords: &passwords,
                    ..Options::DEFAULT
                },
            )
            .unwrap();
            let doc = reading.to_document().unwrap();
            let own = Encryptor::of(&reading).unwrap();
            let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
            let numbers = ids.iter().rev().map(|id| id.number);
            let reversed: BTreeMap<ObjectId, ObjectId> = (ids.iter().zip(numbers))
                .map(|(&id, n)| (id, ObjectId::new(n, id.generation)))
                .collect();
            let top = ids.iter().map(|id| id.number).max().unwrap_or(0) + 1;
            let halved: BTreeMap<ObjectId, ObjectId> = (ids.iter())
                .map(|&id| {
                    (
                        id,
                        if id.number % 2 == 0 {
                            id
                        } else {
                            ObjectId::new(id.number + top, id.generation)
                        },
                    )
                })
                .collect();
            let mut ways = vec![];
            for (numbering, map) in [("reversed", &reversed), ("halved", &halved)] {
                ways.push((numbering, map, "table", XrefKind::Table, None));
                if own.as_ref().is_none_or(Encryptor::packs) {
                    ways.push((
                        numbering,
                        map,
                        "packed",
                        XrefKind::Stream,
                        Some(Pack::DEFAULT),
                    ));
                }
            }
            for (numbering, map, way, kind, pack) in ways {
                let to = |id| map.get(&id).copied();
                let untyped = match to(ObjectId::new(10, 0)) {
                    Some(id) => format!("{id} R/Contents: "),
                    None => String::new(),
                };
                let name = format!("{name}, {numbering}, {way}");
                let mut options = write::Options::DEFAULT;
                (options.kind, options.pack) = (kind, pack);
                options.renumber = Some(&to);
                let mut data = Vec::new();
                write::document(&mut data, &doc, own.as_ref(), Some(&reading), &options).unwrap();
                let path = dir
                    .path()
                    .join(format!("{}.pdf", name.replace([' ', ','], "-")));
                std::fs::write(&path, &data).unwrap();
                let qpdf = qpdf(&path).password("user");
                let check = ["--password=user", "--check", &path.display().to_string()];
                let (out, _) = qpdf.run(&check.map(String::from)).expect(&name);
                let out = String::from_utf8_lossy(&out);
                assert!(
                    out.contains("No syntax or stream encoding errors found"),
                    "{name}: {out}"
                );
                let (alone, heard) = qpdf.heard().expect(&name);
                assert_eq!(alone.trailer.unwritten, None, "{name}");
                assert!(heard.is_empty(), "{name}: {heard:?}");
                let back = read::document(
                    &data,
                    &read::Options {
                        passwords: &passwords,
                        ..read::Options::DEFAULT
                    },
                );
                assert!(back.faults().is_empty(), "{name}: {:?}", back.faults());
                let found = differences(&back, &alone);
                assert_eq!(found.len(), usize::from(strings), "{name}: {found:?}");
                let untyped = |(_, d): &(String, String)| d.starts_with(&untyped);
                assert!(found.iter().all(untyped), "{name}: {found:?}");
            }
        }
    }

    /// [`renumbered_sources`], each updated under two renumberings, given
    /// its document's objects as read and its reading, under its own
    /// encryption: every object but an encryption dictionary and what it
    /// refers to moved past every number, so that each is written, copied
    /// where the rules hold; and the catalog alone moved, so that what
    /// names it is written and the rest passed over. Checked by qpdf by
    /// their password, and read through qpdf alone as the reader reads
    /// them, but a signature's `/Contents` with no `/Type`, as above, at
    /// each number the file holds it at, its own and the one it's written
    /// at.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn renumbered_updates_read_through_qpdf_as_through_the_reader() {
        use std::collections::BTreeSet;

        use crate::read::{Options, Reading};
        use crate::write::{self, Encryptor};

        let dir = scratch("renumbered-updates");
        for (name, source, strings) in renumbered_sources() {
            let passwords = [Password("user")];
            let reading = Reading::open(
                &source[..],
                &Options {
                    passwords: &passwords,
                    ..Options::DEFAULT
                },
            )
            .unwrap();
            let doc = reading.to_document().unwrap();
            let own = Encryptor::of(&reading).unwrap();
            let base = write::Base::of(&reading).unwrap();
            let stays = |id| own.as_ref().is_some_and(|e| e.dictionary_object(id));
            let root = doc
                .trailer
                .dict()
                .get(b"Root")
                .and_then(Object::as_reference);
            let ids: Vec<ObjectId> = reading.ids().collect();
            let past = |id: ObjectId| ObjectId::new(id.number + base.size, id.generation);
            let every: BTreeMap<ObjectId, ObjectId> = (ids.iter())
                .map(|&id| (id, if stays(id) { id } else { past(id) }))
                .collect();
            let catalog: BTreeMap<ObjectId, ObjectId> = (ids.iter())
                .map(|&id| (id, if Some(id) == root { past(id) } else { id }))
                .collect();
            for (numbering, map) in [("every object", &every), ("the catalog", &catalog)] {
                let to = |id| map.get(&id).copied();
                let name = format!("{name}, {numbering}");
                let mut options = write::Options::DEFAULT;
                options.renumber = Some(&to);
                let mut data = source.clone();
                let given = write::bodies(&doc);
                write::update(
                    &mut data,
                    &base,
                    given,
                    own.as_ref(),
                    Some(&reading),
                    &options,
                )
                .unwrap()
                .unwrap();
                let path = dir
                    .path()
                    .join(format!("{}.pdf", name.replace([' ', ','], "-")));
                std::fs::write(&path, &data).unwrap();
                let qpdf = qpdf(&path).password("user");
                let check = ["--password=user", "--check", &path.display().to_string()];
                let (out, _) = qpdf.run(&check.map(String::from)).expect(&name);
                let out = String::from_utf8_lossy(&out);
                assert!(
                    out.contains("No syntax or stream encoding errors found"),
                    "{name}: {out}"
                );
                let (alone, heard) = qpdf.heard().expect(&name);
                assert_eq!(alone.trailer.unwritten, None, "{name}");
                assert!(heard.is_empty(), "{name}: {heard:?}");
                let back = read::document(
                    &data,
                    &read::Options {
                        passwords: &passwords,
                        ..read::Options::DEFAULT
                    },
                );
                assert!(back.faults().is_empty(), "{name}: {:?}", back.faults());
                let found = differences(&back, &alone);
                // The untyped signature, where it was and where it went.
                let held: BTreeSet<ObjectId> = [ObjectId::new(10, 0)]
                    .into_iter()
                    .chain(to(ObjectId::new(10, 0)))
                    .collect();
                let untyped: Vec<String> = (held.iter())
                    .map(|id| format!("{id} R/Contents: "))
                    .collect();
                let want = if strings { held.len() } else { 0 };
                assert_eq!(found.len(), want, "{name}: {found:?}");
                let untyped = |(_, d): &(String, String)| untyped.iter().any(|u| d.starts_with(u));
                assert!(found.iter().all(untyped), "{name}: {found:?}");
            }
        }
    }

    /// Every PDF under `PDF_LINT_CORPUS` read from its bytes, qpdf
    /// decoding, and through qpdf alone, and the differences counted by what
    /// they're of, with examples.
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files, and qpdf"]
    fn a_corpus_reads_alike_through_qpdf_alone() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let files = corpus();
        /// Each kind's count, and its first examples.
        type Kinds = BTreeMap<String, (usize, Vec<String>)>;
        let add = |kinds: &mut Kinds, kind: String, example: String| {
            let (n, examples) = kinds.entry(kind).or_default();
            *n += 1;
            if examples.len() < 5 {
                examples.push(example);
            }
        };
        #[derive(Default)]
        struct Tally {
            objects: usize,
            files_differing: usize,
            differences: Kinds,
        }
        let tally = Mutex::new(Tally::default());
        let next = AtomicUsize::new(0);
        let start = Instant::now();
        thread::scope(|scope| {
            for _ in 0..thread::available_parallelism().map_or(1, |n| n.get()) {
                scope.spawn(|| {
                    while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        let data = std::fs::read(path).unwrap();
                        let qpdf = Qpdf::new(path);
                        let bytes = read::read_with(&data, &qpdf);
                        let alone = qpdf.alone();
                        let mut found = match &alone {
                            Ok(alone) => differences(&bytes, alone),
                            Err(why) => vec![("qpdf reads nothing".to_string(), why.clone())],
                        };
                        if let Some(why) = &bytes.trailer.unwritten {
                            found.push(("the bytes reader scanned".to_string(), why.to_string()));
                        }
                        if let Ok(a) = &alone
                            && let Some(why) = &a.trailer.unwritten
                        {
                            found.push(("qpdf reconstructed".to_string(), why.to_string()));
                        }
                        // Where either repaired the file, each its own way,
                        // their differences are apart.
                        let repaired = bytes.trailer.unwritten.is_some()
                            || alone.as_ref().is_ok_and(|a| a.trailer.unwritten.is_some());
                        if repaired {
                            for (kind, _) in &mut found {
                                if !kind.starts_with("qpdf reconstructed")
                                    && !kind.starts_with("the bytes reader scanned")
                                {
                                    *kind = format!("in a repaired file, {kind}");
                                }
                            }
                        }
                        let mut tally = tally.lock().unwrap();
                        tally.objects += bytes.ids().len();
                        tally.files_differing += usize::from(!found.is_empty());
                        for (kind, d) in found {
                            add(&mut tally.differences, kind, format!("{name}: {d}"));
                        }
                    }
                });
            }
        });
        let tally = tally.into_inner().unwrap();
        println!(
            "{} files in {:.1} s, {} objects: {} files read otherwise",
            files.len(),
            start.elapsed().as_secs_f64(),
            tally.objects,
            tally.files_differing,
        );
        for (kind, (n, examples)) in &tally.differences {
            println!("{kind}: {n}");
            for example in examples {
                let example: String = example.chars().take(300).collect();
                println!("    {example}");
            }
        }
    }

    /// Streams written by hand, decoded by the reader and by qpdf (at
    /// `specialized`): alike, but where the reader says a fault qpdf
    /// passes over, and where qpdf departs from the spec.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn hand_made_streams_decode_as_through_qpdf_but_where_it_departs_from_the_spec() {
        /// How the reader's decoding should stand to qpdf's.
        #[derive(Debug, PartialEq)]
        enum Expect {
            /// The same bytes, and no fault.
            Alike,
            /// The same bytes, and the reader says this fault.
            AlikeBut(&'static str),
            /// The reader stops where the spec says, qpdf reads on.
            Shorter,
            /// qpdf gives it undecoded; the reader decodes it.
            QpdfRefuses,
            /// The reader doesn't decode it; qpdf does.
            ReaderRefuses,
        }
        use Expect::*;
        let text = b"BT /F1 12 Tf (stream data) Tj ET\n".repeat(20);
        let hex = |data: &[u8]| -> Vec<u8> {
            data.iter()
                .flat_map(|b| format!("{b:02x}").into_bytes())
                .collect()
        };
        let ended = |data: &[u8]| [data, b">"].concat();
        let mut damaged = fixture::zlib(&text);
        *damaged.last_mut().unwrap() ^= 1;
        // Rows of four, each by a tag of its own, 0 to 4.
        let rows: Vec<u8> = (0..40u8)
            .flat_map(|i| [i % 5, i, i.wrapping_mul(7), 200, i ^ 0x55])
            .collect();
        let png = dict! { "Predictor" => 12, "Columns" => 4 };
        // Sixteen rows of four pixels of three samples.
        let tiff = dict! { "Predictor" => 2, "Colors" => 3, "Columns" => 4 };
        let tiff3 = dict! { "Predictor" => 2, "BitsPerComponent" => 3 };
        let identity = dict! { "Name" => "Identity" };
        let crypt: Vec<Object> = vec![identity.into(), Object::null()];
        let crypted: Vec<Object> = vec![Object::from("Crypt"), Object::from("FlateDecode")];
        let cases: Vec<(&str, Dict, Vec<u8>, Expect)> = vec![
            (
                "ASCIIHex",
                dict! { "Filter" => "ASCIIHexDecode" },
                ended(&hex(&text)),
                Alike,
            ),
            (
                "ASCII85",
                dict! { "Filter" => "ASCII85Decode" },
                fixture::ascii85(&text),
                Alike,
            ),
            (
                "LZW",
                dict! { "Filter" => "LZWDecode" },
                fixture::lzw(&text),
                Alike,
            ),
            (
                "RunLength",
                dict! { "Filter" => "RunLengthDecode" },
                fixture::run_length(&text),
                Alike,
            ),
            (
                "Flate",
                dict! { "Filter" => "FlateDecode" },
                fixture::zlib(&text),
                Alike,
            ),
            (
                "Fl",
                dict! { "Filter" => "Fl" },
                fixture::zlib(&text),
                Alike,
            ),
            (
                "PNG",
                dict! { "Filter" => "FlateDecode", "DecodeParms" => png },
                fixture::zlib(&rows),
                Alike,
            ),
            (
                "TIFF",
                dict! { "Filter" => "FlateDecode", "DecodeParms" => tiff },
                fixture::zlib(&rows[..192]),
                Alike,
            ),
            (
                "Crypt",
                dict! { "Filter" => crypted, "DecodeParms" => crypt },
                fixture::zlib(&text),
                Alike,
            ),
            (
                "checksum",
                dict! { "Filter" => "FlateDecode" },
                damaged,
                AlikeBut("Checksum"),
            ),
            (
                "no >",
                dict! { "Filter" => "ASCIIHexDecode" },
                hex(&text),
                AlikeBut("NoEnd"),
            ),
            // ISO 32000-2, 7.4.5: 128 is the end; qpdf reads on past it.
            (
                "past 128",
                dict! { "Filter" => "RunLengthDecode" },
                [&fixture::run_length(&text)[..], b"\x02abc\x80"].concat(),
                Shorter,
            ),
            // 7.2.3: NUL is white-space, which qpdf takes as a fault.
            (
                "NUL",
                dict! { "Filter" => "ASCIIHexDecode" },
                ended(&[&b"41\x00"[..], &hex(&text)].concat()),
                QpdfRefuses,
            ),
            // 7.4.4.4: TIFF's samples are 1, 2, 4, 8 or 16 bits.
            (
                "3 bits",
                dict! { "Filter" => "FlateDecode", "DecodeParms" => tiff3 },
                fixture::zlib(&rows),
                ReaderRefuses,
            ),
        ];
        let dir = scratch("hand-made");
        let path = dir.path().join("streams.pdf");
        let mut objects = clean();
        let first = 10;
        for (i, (_, dict, data, _)) in cases.iter().enumerate() {
            objects.push((first + i as u32, Body::stream(dict.clone(), data)));
        }
        let root = dict! { "Root" => Object::reference(1) };
        std::fs::write(&path, fixture::file("1.7", &objects, root, Xref::Table)).unwrap();
        let data = std::fs::read(&path).unwrap();
        let doc = read::document(&data, &read::Options::DEFAULT);
        let ids: Vec<ObjectId> = (0..cases.len())
            .map(|i| ObjectId::new(first + i as u32, 0))
            .collect();
        let theirs = Qpdf::new(&path).specialized().decode(&ids);
        for (i, (name, _, _, expect)) in cases.iter().enumerate() {
            let id = ids[i];
            let object = doc.get(id).unwrap();
            let Value::Stream(stream) = object.value() else {
                panic!("{name}")
            };
            let at = stream.data.at().unwrap();
            let written = &data[at.start..at.end];
            let theirs = &theirs[&id];
            let got = match crate::filter::filters(&stream.dict, Some(&|_| None)) {
                Err(_) => match theirs {
                    Ok(_) => ReaderRefuses,
                    Err(why) => panic!("{name}: neither decodes: {why}"),
                },
                Ok(filters) => {
                    let (mine, faults) = match pdf_filters::decode(
                        written,
                        &filters,
                        read::STREAM,
                        Backend::MinizOxide,
                    ) {
                        Ok(data) => (data, Vec::new()),
                        Err(e) => (e.data, e.faults),
                    };
                    match (theirs, &faults[..]) {
                        (Ok(t), []) if *t == mine => Alike,
                        (Ok(t), [(_, fault)]) if *t == mine => {
                            let kind = format!("{fault:?}");
                            AlikeBut(if kind.starts_with("Checksum") {
                                "Checksum"
                            } else if kind.starts_with("NoEnd") {
                                "NoEnd"
                            } else {
                                "other"
                            })
                        }
                        (Ok(t), []) if t.starts_with(&mine) && t.len() > mine.len() => Shorter,
                        (Err(why), []) if why.contains("doesn't decode its filters") => QpdfRefuses,
                        (theirs, faults) => {
                            panic!("{name}: qpdf {theirs:?}, the reader's faults {faults:?}")
                        }
                    }
                }
            };
            assert_eq!(&got, expect, "{name}");
        }
    }

    /// The PDF files under `PDF_LINT_CORPUS`, sorted.
    fn corpus() -> Vec<PathBuf> {
        let dir = std::env::var_os("PDF_LINT_CORPUS").expect("PDF_LINT_CORPUS names a folder");
        let mut folders = vec![PathBuf::from(dir)];
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else if (path.extension()).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    /// A difference between decoding through the reader and through qpdf
    /// that's understood, as `kind` names it, and why it's no fault of
    /// the reader's.
    fn explained(kind: &str) -> Option<&'static str> {
        let stopping = [
            "Byte",
            "BrokenEnd",
            "InGroup",
            "Code",
            "Header",
            "Corrupt",
            "Truncated",
            "Limit",
        ];
        let faults = kind
            .rsplit_once("faults: ")
            .map_or("", |(_, faults)| faults);
        let stopped = stopping.iter().any(|f| faults.contains(f));
        match kind {
            _ if kind.starts_with("qpdf doesn't read the stream the reader does") => Some(
                "qpdf finds a stream by its number, which an update can give another, reads one of a file it reconstructed, or refuses the whole file; the reader decodes the bytes it reads",
            ),
            _ if kind.starts_with("a stream decodes alike") => Some(
                "the same bytes: the reader says what qpdf reads without a word (no end marker, a checksum that doesn't match, an unknown row tag, a short row, data that ends early)",
            ),
            _ if kind.starts_with("qpdf leaves a stream undecoded") && stopped => Some(
                "a fault stopped a filter: qpdf gives the data undecoded, the reader what decoded before the fault",
            ),
            _ if kind.starts_with("qpdf leaves a stream undecoded")
                && faults.contains("TableFull") =>
            {
                Some(
                    "an LZW table full with no clear code: qpdf stops and gives the data undecoded, the reader reads on against the table as it is, as PDFium and pdf.js do",
                )
            }
            _ if kind
                .starts_with("a stream decodes otherwise (the reader's is a prefix of qpdf's)")
                && faults.contains("Limit") =>
            {
                Some("the reader stops at its limit, 256 MiB a stream")
            }
            _ if kind.starts_with("qpdf can't decode a stream") && faults == "Limit" => Some(
                "a predictor's rows too long to hold: qpdf exits for the overflow, the reader stops at its limit",
            ),
            _ => None,
        }
    }

    /// Every stream with filters the reader decodes, in each file of a
    /// folder (`PDF_LINT_CORPUS`), decoded by the reader and by qpdf (at
    /// `specialized`, RunLength too), byte for byte; and the object
    /// streams' data and objects as the reader reads them and as it reads
    /// them asking qpdf. Each difference is counted by its kind, with
    /// examples, and each kind must be one [`explained`] knows. A file that
    /// encrypts is left out: its streams are qpdf's to decrypt. With
    /// `PDF_LINT_ORACLE_DUMP` naming a folder, each stream that differs is
    /// written there as written, as the reader decodes it and as qpdf does.
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files, and qpdf"]
    fn a_corpus_decodes_alike_through_qpdf() {
        use std::sync::Mutex;

        use pdf_filters::Fault;

        let files = corpus();
        type Kinds = BTreeMap<String, (usize, Vec<String>)>;
        #[derive(Default)]
        struct Tally {
            files: usize,
            encrypted: usize,
            streams: usize,
            alike: usize,
            kinds: Kinds,
        }
        let tally = Mutex::new(Tally::default());
        let next = AtomicUsize::new(0);
        let start = Instant::now();
        let dump = std::env::var_os("PDF_LINT_ORACLE_DUMP").map(PathBuf::from);
        // A fault's kind, without its fields.
        let kind = |fault: &Fault| {
            let named = format!("{fault:?}");
            named
                .split([' ', '{'])
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let faults = |faults: &[(usize, Fault)]| {
            let mut kinds: Vec<String> = faults.iter().map(|(_, f)| kind(f)).collect();
            kinds.sort();
            kinds.dedup();
            match kinds.is_empty() {
                true => "none".to_string(),
                false => kinds.join(", "),
            }
        };
        // How the reader's data stands to qpdf's.
        let relation = |ours: &[u8], theirs: &[u8]| match () {
            _ if theirs.starts_with(ours) => "the reader's is a prefix of qpdf's",
            _ if ours.starts_with(theirs) => "qpdf's is a prefix of the reader's",
            _ => "neither is a prefix of the other",
        };
        thread::scope(|scope| {
            for _ in 0..thread::available_parallelism().map_or(1, |n| n.get()) {
                scope.spawn(|| {
                    while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        let data = std::fs::read(path).unwrap();
                        let ours = read::document(&data, &read::Options::DEFAULT);
                        let encrypted = (ours.trailer.dict().get(b"Encrypt")).is_some_and(|e| !e.is_null());
                        if encrypted {
                            let mut tally = tally.lock().unwrap();
                            tally.files += 1;
                            tally.encrypted += 1;
                            continue;
                        }
                        let qpdf = Qpdf::new(path).specialized();
                        let mut found: Vec<(String, String)> = Vec::new();
                        // The reader's own streams, and their objects, read
                        // asking qpdf.
                        let through = read::read_through(&data, &qpdf);
                        let ids: std::collections::BTreeSet<&ObjectId> = ours.decoded.keys().chain(through.decoded.keys()).collect();
                        for id in ids {
                            let kind = match (ours.decoded.get(id), through.decoded.get(id)) {
                                (Some(a), Some(b)) if a == b => continue,
                                (Some(a), Some(b)) => format!("an object stream decodes otherwise: {}", relation(a, b)),
                                (Some(_), None) => "an object stream only the reader decodes".to_string(),
                                (None, _) => "an object stream only qpdf decodes".to_string(),
                            };
                            found.push((kind, format!("{id} R")));
                        }
                        for id in ours.ids() {
                            let kind = match (ours.get(id), through.contains(id).then(|| through.get(id))) {
                                (Ok(a), Some(Ok(b))) if a == b => continue,
                                (Err(_), Some(Err(_)) | None) => continue,
                                (Ok(_), Some(Ok(_))) => "an object reads otherwise",
                                (Ok(_), Some(Err(_)) | None) => "an object only the reader reads",
                                (Err(_), Some(Ok(_))) => "an object only qpdf's decoding reads",
                            };
                            found.push((kind.to_string(), format!("{id} R")));
                        }
                        // Every stream with filters it decodes.
                        let resolve = |id| ours.get(id).ok();
                        let streams: Vec<(ObjectId, Vec<pdf_filters::Filter>, &[u8])> = (ours.objects())
                            .filter_map(|(id, object)| {
                                let Value::Stream(stream) = object.value() else {
                                    return None;
                                };
                                let at = stream.data.at().filter(|at| at.stream.is_none())?;
                                let filters = crate::filter::filters(&stream.dict, Some(&resolve)).ok()?;
                                (!filters.is_empty()).then(|| (id, filters, &data[at.start..at.end]))
                            })
                            .collect();
                        let ids: Vec<ObjectId> = streams.iter().map(|&(id, _, _)| id).collect();
                        let mut theirs = qpdf.decode(&ids);
                        let mut alike = 0;
                        // Each that differs, its kind as far as the data
                        // tells; whether qpdf read the same bytes is asked
                        // after, of those.
                        let mut differing: Vec<(ObjectId, String, bool)> = Vec::new();
                        for (id, filters, written) in &streams {
                            let (mine, mine_faults) = match pdf_filters::decode(written, filters, read::STREAM, Backend::MinizOxide) {
                                Ok(data) => (data, Vec::new()),
                                Err(e) => (e.data, e.faults),
                            };
                            let mut faults = faults(&mine_faults);
                            if mine_faults.iter().any(|(i, _)| *i > 0) {
                                faults = format!("{faults} (in a later filter)");
                            }
                            let theirs = theirs.remove(id);
                            if let Some(dir) = &dump
                                && !matches!(&theirs, Some(Ok(t)) if *t == mine && mine_faults.is_empty()) {
                                    let file = |what: &str| dir.join(format!("{name}-{}-{}.{what}", id.number, id.generation));
                                    std::fs::write(file("raw"), written).unwrap();
                                    std::fs::write(file("ours"), &mine).unwrap();
                                    let qpdf = match &theirs {
                                        Some(Ok(t)) => t.clone(),
                                        Some(Err(why)) => format!("ERR {why}").into_bytes(),
                                        None => b"NONE".to_vec(),
                                    };
                                    std::fs::write(file("qpdf"), qpdf).unwrap();
                                }
                            // Whether it matters which bytes qpdf read.
                            let (kind, which) = match theirs {
                                Some(Ok(t)) if t == mine && mine_faults.is_empty() => {
                                    alike += 1;
                                    continue;
                                }
                                Some(Ok(t)) if t == mine => (format!("a stream decodes alike, the reader's faults: {faults}"), false),
                                Some(Ok(t)) => (
                                    format!("a stream decodes otherwise ({}), the reader's faults: {faults}", relation(&mine, &t)),
                                    true,
                                ),
                                Some(Err(why)) if why.contains("doesn't decode its filters") => {
                                    (format!("qpdf leaves a stream undecoded, the reader's faults: {faults}"), true)
                                }
                                Some(Err(_)) | None => (format!("qpdf can't decode a stream, the reader's faults: {faults}"), true),
                            };
                            differing.push((*id, kind, which));
                        }
                        let asked: Vec<ObjectId> = (differing.iter()).filter(|d| d.2).map(|d| d.0).collect();
                        let read = Qpdf::new(path).undecoding().decode(&asked);
                        for (id, kind, which) in differing {
                            let written = streams.iter().find(|s| s.0 == id).map(|s| s.2);
                            let same = matches!(read.get(&id), Some(Ok(raw)) if Some(&raw[..]) == written);
                            let kind = match which && !same {
                                true => format!("qpdf doesn't read the stream the reader does: {kind}"),
                                false => kind,
                            };
                            found.push((kind, format!("{id} R")));
                        }
                        let mut tally = tally.lock().unwrap();
                        tally.files += 1;
                        tally.streams += streams.len();
                        tally.alike += alike;
                        for (kind, example) in found {
                            let (n, examples) = tally.kinds.entry(kind).or_default();
                            *n += 1;
                            if examples.len() < 5 {
                                examples.push(format!("{name}: {example}"));
                            }
                        }
                    }
                });
            }
        });
        let tally = tally.into_inner().unwrap();
        println!(
            "{} files ({} encrypted, left out) in {:.0?}: {} streams with filters the reader decodes, {} alike",
            tally.files,
            tally.encrypted,
            start.elapsed(),
            tally.streams,
            tally.alike
        );
        let mut unexplained = Vec::new();
        for (kind, (n, examples)) in &tally.kinds {
            let why = explained(kind);
            println!(
                "{n:>8}  {kind}{}",
                why.map_or(String::new(), |why| format!(": {why}"))
            );
            for example in examples {
                println!("            e.g. {example}");
            }
            if why.is_none() {
                unexplained.push(kind.clone());
            }
        }
        assert!(unexplained.is_empty(), "unexplained: {unexplained:#?}");
    }

    /// The passwords the test suites' encrypted files are opened with, as
    /// their tests give them.
    const PASSWORDS: [&str; 26] = [
        "user",
        "owner",
        "z",
        "u",
        "quack",
        "asdfqwer",
        "π",
        "ʬ",
        "Á•Ž€π🥔",
        "Á•Ž€",
        "y",
        "x",
        "user3",
        "owner3",
        "pass",
        "khoai",
        "attachment",
        "623",
        "19723102477",
        "secret",
        "usersecret",
        "ownersecret",
        "test",
        "password",
        "foo",
        "a",
    ];

    /// A difference between decrypting through the reader and through
    /// qpdf that's understood, as `kind` names it, and why it's no fault of
    /// the reader's.
    fn decryption_explained(kind: &str) -> Option<&'static str> {
        match kind {
            _ if kind.contains("in a file qpdf refuses") => Some(
                "qpdf refuses the whole file (a page tree without /Kids), and so gives none of its streams",
            ),
            _ if kind.starts_with("verdicts differ: the reader: plain; qpdf: locked") => Some(
                "nothing the reader reads is encrypted (/StmF and /StrF /Identity, as when only embedded files are), so it reads whole without the password, which qpdf asks for",
            ),
            _ if kind.starts_with("verdicts differ: the reader: opens; qpdf: can't open (root of pages tree has no /Kids array)") => Some(
                "qpdf refuses the whole file for its page tree; the reader opens it",
            ),
            _ if kind.starts_with("decrypted, ") && kind.contains("by a /Crypt filter that names none") => Some(
                "qpdf decrypts what isn't encrypted (see below), and what it gives doesn't decode",
            ),
            _ if kind.starts_with("decrypted, ") && kind.contains("an embedded file by /EFF") => Some(
                "qpdf decrypts it as /StmF says (see below), and what it gives doesn't decode",
            ),
            _ if kind.starts_with("decrypted, ") => explained(&kind["decrypted, ".len()..]),
            _ if kind.starts_with("decrypts otherwise") && kind.contains("an embedded file by /EFF") => Some(
                "an embedded file without a crypt filter of its own: the reader decrypts it by the one /EFF names, as ISO 32000-2's Table 20 has it; qpdf as /StmF says, reading /EFF only for what a writer should encrypt new embedded files by",
            ),
            _ if kind.contains("by a /Crypt filter that names none") => Some(
                "a /Crypt filter without /Name is /Identity (ISO 32000-2, Table 14), and the data plain (XMP, zlib): qpdf takes a /Name only from parameters of /Type /CryptFilterDecodeParms or in an array, and else decrypts by /StmF",
            ),
            "decrypts alike, the reader's fault: Short { len: 16 }" => Some(
                "just an IV: both decrypt it to nothing; the reader says it lacks the padding AES-CBC always writes",
            ),
            _ if kind.starts_with("decrypts otherwise (the reader's is a prefix of qpdf's), the reader's fault: PartBlock") => Some(
                "a part block past the whole blocks: the reader leaves it off, qpdf pads it with zeros and decrypts it",
            ),
            _ => None,
        }
    }

    /// Every encrypted PDF under `PDF_LINT_CORPUS`, opened by the reader
    /// with the empty password and by qpdf: whether it opens, is locked, or
    /// has a handler that can't open, the reader's verdict against qpdf's
    /// `--requires-password`; and in each the reader opens, every stream's
    /// data decrypted as the reader decrypts (its handler, a stream's own
    /// `/Crypt`, else an embedded file's `/EFF` where it's a name, a
    /// cross-reference stream not, the catalog's metadata stream not under
    /// `/EncryptMetadata false` but by its own) against qpdf's
    /// (`undecoding`), and those
    /// with filters the reader decodes, decrypted and decoded, against
    /// qpdf's at `specialized`. Each difference counted by its kind, with
    /// examples, and each kind must be one [`decryption_explained`] knows.
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files, and qpdf"]
    fn a_corpus_decrypts_alike_through_qpdf() {
        use std::sync::Mutex;

        let files = corpus();
        type Kinds = BTreeMap<String, (usize, Vec<String>)>;
        #[derive(Default)]
        struct Tally {
            files: usize,
            encrypted: usize,
            verdicts: BTreeMap<String, usize>,
            streams: usize,
            alike: usize,
            decoded: usize,
            decoded_alike: usize,
            kinds: Kinds,
        }
        let tally = Mutex::new(Tally::default());
        let next = AtomicUsize::new(0);
        let start = Instant::now();
        let dump = std::env::var_os("PDF_LINT_ORACLE_DUMP").map(PathBuf::from);
        let relation = |ours: &[u8], theirs: &[u8]| match () {
            _ if theirs.starts_with(ours) => "the reader's is a prefix of qpdf's",
            _ if ours.starts_with(theirs) => "qpdf's is a prefix of the reader's",
            _ => "neither is a prefix of the other",
        };
        thread::scope(|scope| {
            for _ in 0..thread::available_parallelism().map_or(1, |n| n.get()) {
                scope.spawn(|| {
                    while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        let data = std::fs::read(path).unwrap();
                        let (ours, handler) = read::read_opened_by(&data, &[]);
                        if ours.encryption == crate::Encryption::None {
                            tally.lock().unwrap().files += 1;
                            continue;
                        }
                        // Locked, or plain, its embedded files encrypted
                        // alone, by the first password the test suites use
                        // that opens it for qpdf, if one does, given to the
                        // reader as the command line gives it.
                        let password = matches!(ours.encryption, crate::Encryption::Locked | crate::Encryption::Plain)
                            .then(|| PASSWORDS.iter().find(|p| Qpdf::new(path).password(**p).locked() == Ok(false)))
                            .flatten();
                        let (ours, handler) = match password {
                            Some(password) => {
                                read::read_opened_by(&data, &[Password(password)])
                            }
                            None => (ours, handler),
                        };
                        let qpdf = match password {
                            Some(password) => Qpdf::new(path).password(*password),
                            None => Qpdf::new(path),
                        };
                        let mut found: Vec<(String, String)> = Vec::new();
                        // The verdicts.
                        let mut args = vec!["--requires-password".to_string(), path.display().to_string()];
                        if let Some(password) = password {
                            args.insert(0, format!("--password={password}"));
                        }
                        let theirs = match qpdf.exec(&args) {
                            Ok(ran) => match ran.code {
                                Some(0) => "locked".to_string(),
                                Some(3) => "opens".to_string(),
                                Some(2) if ran.stderr.is_empty() => "isn't encrypted".to_string(),
                                _ => {
                                    // Its error, after any warnings, without
                                    // the file's name.
                                    let stderr = String::from_utf8_lossy(&ran.stderr);
                                    let why = stderr.lines().rfind(|l| !l.is_empty()).unwrap_or_default();
                                    let why = why.rsplit_once("): ").map_or(why, |(_, w)| w);
                                    let why = why.rsplit_once(".pdf: ").map_or(why, |(_, w)| w);
                                    format!("can't open ({why})")
                                }
                            },
                            Err(why) => format!("doesn't run ({why})"),
                        };
                        let (mine, why) = match &ours.encryption {
                            crate::Encryption::Opened { .. } => ("opens", String::new()),
                            crate::Encryption::Locked => ("locked", String::new()),
                            crate::Encryption::Plain => ("plain", String::new()),
                            crate::Encryption::Unsupported(why) => ("can't open", why.to_string()),
                            crate::Encryption::None => unreachable!(),
                        };
                        let by = if password.is_some() { " by a password" } else { "" };
                        let verdict = format!("the reader: {mine}{by}; qpdf: {theirs}");
                        if !theirs.starts_with(mine) {
                            let why = if why.is_empty() { String::new() } else { format!(" ({why})") };
                            let verdict = format!("the reader: {mine}{why}; qpdf: {theirs}");
                            found.push((format!("verdicts differ: {verdict}"), String::new()));
                        }
                        // What qpdf gives of a file it refuses is nothing.
                        let refused = if theirs.starts_with("can't open") { ", in a file qpdf refuses" } else { "" };
                        // Each stream, decrypted.
                        let mut streams = 0;
                        let mut alike = 0;
                        let (mut decoded, mut decoded_alike) = (0, 0);
                        if let Some(handler) = &handler {
                            let resolve = |id| ours.get(id).ok();
                            // The document's metadata stream, the catalog's,
                            // which /EncryptMetadata false leaves plain.
                            let catalog = ours.trailer.dict().get(b"Root").and_then(|r| match *r.value() {
                                Value::Ref(id) => resolve(id),
                                _ => None,
                            });
                            let metadata = catalog
                                .and_then(Object::as_dict)
                                .and_then(|c| c.get(b"Metadata"))
                                .and_then(|m| match *m.value() {
                                    Value::Ref(id) => Some(id),
                                    _ => None,
                                });
                            // The crypt filter /EFF names, which decrypts an
                            // embedded file without one of its own.
                            let followed = |o: &'_ Object| match *o.value() {
                                Value::Ref(id) => resolve(id),
                                _ => None,
                            };
                            let eff = (ours.trailer.dict().get(b"Encrypt"))
                                .and_then(|e| followed(e).or(Some(e)))
                                .and_then(Object::as_dict)
                                .and_then(|d| d.get(b"EFF"))
                                .and_then(|e| followed(e).or(Some(e)))
                                .and_then(Object::as_name);
                            // Each stream: decrypted, its filters, whether a
                            // /Crypt filter of its names none, and whether it's
                            // an embedded file decrypted by /EFF's.
                            type Decrypted = (ObjectId, Result<Vec<u8>, pdf_crypt::Error>, Vec<pdf_filters::Filter>, bool, bool);
                            let mine: Vec<Decrypted> = (ours.objects())
                                .filter_map(|(id, object)| {
                                    let Value::Stream(stream) = object.value() else {
                                        return None;
                                    };
                                    let at = stream.data.at().filter(|at| at.stream.is_none())?;
                                    let written = &data[at.start..at.end];
                                    let typed = stream.dict.get(b"Type").and_then(Object::as_name);
                                    let crypt = crate::filter::crypt_filter(&stream.dict, Some(&resolve));
                                    let own = crypt.as_deref().filter(|&c| c != b"Identity");
                                    let plain = typed == Some(b"XRef")
                                        || (Some(id) == metadata && !handler.encrypts_metadata() && own.is_none());
                                    // A /Crypt filter that names no crypt
                                    // filter, so /Identity.
                                    let named = match stream.dict.get(b"DecodeParms").map(|p| p.value()) {
                                        Some(Value::Dict(d)) => d.has(b"Name"),
                                        Some(Value::Array(a)) => {
                                            a.iter().any(|p| matches!(p.value(), Value::Dict(d) if d.has(b"Name")))
                                        }
                                        _ => false,
                                    };
                                    let unnamed = crypt.is_some() && !named;
                                    let embedded = typed == Some(b"EmbeddedFile") && crypt.is_none();
                                    let by_eff = embedded && eff.is_some();
                                    let crypt = crypt.or_else(|| eff.filter(|_| embedded).map(<[u8]>::to_vec));
                                    let decrypted = match plain {
                                        true => Ok(written.to_vec()),
                                        false => {
                                            let method = crypt.map(|name| handler.named(&name));
                                            handler.decrypt_stream(id.into(), written, method.as_ref())
                                        }
                                    };
                                    let filters = crate::filter::filters(&stream.dict, Some(&resolve)).unwrap_or_default();
                                    Some((id, decrypted, filters, unnamed, by_eff))
                                })
                                .collect();
                            let ids: Vec<ObjectId> = mine.iter().map(|(id, ..)| *id).collect();
                            let mut raw = qpdf.clone().undecoding().decode(&ids);
                            let with_filters: Vec<ObjectId> = (mine.iter())
                                .filter(|(_, _, filters, ..)| !filters.is_empty())
                                .map(|(id, ..)| *id)
                                .collect();
                            let mut specialized = qpdf.clone().specialized().decode(&with_filters);
                            for (id, decrypted, filters, unnamed, by_eff) in mine {
                                streams += 1;
                                let example = format!("{id} R");
                                let (plain, fault) = match decrypted {
                                    Ok(plain) => (Some(plain), String::new()),
                                    Err(e) => (e.data, format!(", the reader's fault: {:?}", e.fault)),
                                };
                                let by = match (unnamed, by_eff) {
                                    (true, _) => ", by a /Crypt filter that names none",
                                    (false, true) => ", an embedded file by /EFF",
                                    (false, false) => "",
                                };
                                let by = format!("{by}{refused}");
                                let theirs = raw.remove(&id);
                                if let Some(dir) = &dump
                                    && !matches!((&plain, &theirs), (Some(p), Some(Ok(t))) if p == t) {
                                        let file = |what: &str| dir.join(format!("{name}-{}-{}.{what}", id.number, id.generation));
                                        std::fs::write(file("ours"), plain.as_deref().unwrap_or(b"NONE")).unwrap();
                                        let qpdf = match &theirs {
                                            Some(Ok(t)) => t.clone(),
                                            Some(Err(why)) => format!("ERR {why}").into_bytes(),
                                            None => b"NONE".to_vec(),
                                        };
                                        std::fs::write(file("qpdf"), qpdf).unwrap();
                                    }
                                match (&plain, &theirs) {
                                    (Some(p), Some(Ok(t))) if p == t && fault.is_empty() => alike += 1,
                                    (Some(p), Some(Ok(t))) if p == t => found.push((format!("decrypts alike{fault}"), example.clone())),
                                    (Some(p), Some(Ok(t))) => found.push((format!("decrypts otherwise ({}){fault}{by}", relation(p, t)), example.clone())),
                                    (None, Some(Ok(_))) => found.push((format!("only qpdf decrypts{fault}{by}"), example.clone())),
                                    (_, Some(Err(_)) | None) => found.push((format!("qpdf gives no data{fault}{by}"), example.clone())),
                                }
                                if filters.is_empty() {
                                    continue;
                                }
                                decoded += 1;
                                let Some(plain) = plain else { continue };
                                let (mine, mine_faults) = match pdf_filters::decode(&plain, &filters, read::STREAM, Backend::MinizOxide) {
                                    Ok(data) => (data, Vec::new()),
                                    Err(e) => (e.data, e.faults),
                                };
                                // As the decoding oracle names them.
                                let mut kinds: Vec<String> = (mine_faults.iter())
                                    .map(|(_, f)| format!("{f:?}").split([' ', '{']).next().unwrap_or_default().to_string())
                                    .collect();
                                kinds.sort();
                                kinds.dedup();
                                let faults = if kinds.is_empty() { "none".to_string() } else { kinds.join(", ") };
                                let unnamed = match (unnamed, refused.is_empty()) {
                                    (true, _) => " (by a /Crypt filter that names none)",
                                    (false, false) => " (in a file qpdf refuses)",
                                    (false, true) if by_eff => " (an embedded file by /EFF)",
                                    (false, true) => "",
                                };
                                let kind = match specialized.remove(&id) {
                                    Some(Ok(t)) if t == mine && mine_faults.is_empty() => {
                                        decoded_alike += 1;
                                        continue;
                                    }
                                    Some(Ok(t)) if t == mine => format!("a stream decodes alike, the reader's faults: {faults}"),
                                    Some(Ok(t)) => format!("a stream decodes otherwise ({}){unnamed}, the reader's faults: {faults}", relation(&mine, &t)),
                                    Some(Err(why)) if why.contains("doesn't decode its filters") => {
                                        format!("qpdf leaves a stream undecoded{unnamed}, the reader's faults: {faults}")
                                    }
                                    Some(Err(_)) | None => format!("qpdf can't decode a stream{unnamed}, the reader's faults: {faults}"),
                                };
                                found.push((format!("decrypted, {kind}"), example));
                            }
                        }
                        let mut tally = tally.lock().unwrap();
                        tally.files += 1;
                        tally.encrypted += 1;
                        *tally.verdicts.entry(verdict).or_default() += 1;
                        tally.streams += streams;
                        tally.alike += alike;
                        tally.decoded += decoded;
                        tally.decoded_alike += decoded_alike;
                        for (kind, example) in found {
                            let (n, examples) = tally.kinds.entry(kind).or_default();
                            *n += 1;
                            if examples.len() < 5 {
                                examples.push(format!("{name}: {example}"));
                            }
                        }
                    }
                });
            }
        });
        let tally = tally.into_inner().unwrap();
        println!(
            "{} files, {} encrypted, in {:.0?}: {} streams in those the reader opens, {} decrypted alike; {} with filters, {} decoded alike",
            tally.files,
            tally.encrypted,
            start.elapsed(),
            tally.streams,
            tally.alike,
            tally.decoded,
            tally.decoded_alike
        );
        for (verdict, n) in &tally.verdicts {
            println!("{n:>8}  {verdict}");
        }
        let mut unexplained = Vec::new();
        for (kind, (n, examples)) in &tally.kinds {
            let why = decryption_explained(kind);
            println!(
                "{n:>8}  {kind}{}",
                why.map_or(String::new(), |why| format!(": {why}"))
            );
            for example in examples {
                println!("            e.g. {example}");
            }
            if why.is_none() {
                unexplained.push(kind.clone());
            }
        }
        assert!(unexplained.is_empty(), "unexplained: {unexplained:#?}");
    }
}
