//! A reading read on demand: in any order, on any thread, it's the whole
//! read, so long as no stream it decodes reaches the budget; what it keeps
//! of what it reads, and where it read each object from.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::tests::{
    KINDS, clean, compressed, filtered, junk, objects_2_and_3, root, settings, small, to_encrypt,
    unfiltered, updated_with_padding, xref_row,
};
use super::*;
use crate::fixture::{self, Xref};
use crate::source::Source;

/// What a test that reads a bomb takes at most.
const QUICK: Duration = Duration::from_secs(5);

/// `file` opened on demand under `options`, by `passwords`.
pub(super) fn open<'f>(
    file: &'f [u8],
    passwords: &[Password],
    options: &Options,
) -> Reading<&'f [u8]> {
    Reading::open_with(file, passwords, options).unwrap()
}

/// `file` read whole under `options`, by `passwords`.
fn whole(file: &[u8], passwords: &[Password], options: &Options) -> Whole {
    Reading::in_memory(file, passwords, options, true, None).whole(None)
}

/// `bytes` as hex digits.
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `file` with its trailer's `/Root 1 0 R` followed by `entries`.
pub(super) fn with_trailer(file: &[u8], entries: &str) -> Vec<u8> {
    let at = find(file, b"/Root 1 0 R").unwrap() + b"/Root 1 0 R".len();
    [&file[..at], entries.as_bytes(), &file[at..]].concat()
}

/// [`filtered`]'s file, its object stream's `filter` and data `data`, in a
/// file encrypted by AES-256 with the user's password `user`, its strings
/// and streams by `/StdCF`; and the object stream's data, zlib, encrypted
/// so.
pub(super) fn encrypted_objects(filter: &str, data: Option<&[u8]>) -> Vec<u8> {
    let values = pdf_crypt::writer::dictionary(&settings(pdf_crypt::writer::Kind::Aes256, b"user"));
    let handler =
        Handler::open(&values, b"0123456789abcdef", &[Encoded::verbatim(b"user")]).unwrap();
    let (objects, _) = objects_2_and_3();
    let encrypted = pdf_crypt::writer::encrypt_stream(
        &handler,
        (4, 0),
        &fixture::zlib(&objects),
        None,
        [4; 16],
    )
    .unwrap();
    let field = |b: &Option<Vec<u8>>| hex(b.as_deref().unwrap_or_default());
    let encrypt = format!(
        " /Encrypt << /Filter /Standard /V 5 /R 6 /Length 256 /P -4 /O <{}> /U <{}> /OE <{}> /UE <{}> /Perms <{}> \
         /CF << /StdCF << /CFM /AESV3 >> >> /StmF /StdCF /StrF /StdCF >> \
         /ID [(0123456789abcdef) (0123456789abcdef)]",
        field(&values.o),
        field(&values.u),
        field(&values.oe),
        field(&values.ue),
        field(&values.perms)
    );
    let mut file = filtered(false, filter, data.unwrap_or(&encrypted));
    // The catalog's string, written in plain view, blanked out, so that
    // nothing moves.
    let at = find(&file, b"/Lang (en)").unwrap();
    file[at..at + 10].fill(b' ');
    with_trailer(&file, &encrypt)
}

/// [`unfiltered`]'s file, encrypted, but only its embedded files: its
/// strings and streams `/Identity`, its `/EFF` a crypt filter of its own,
/// which no password it's given opens ([`Encryption::Plain`]).
pub(super) fn attachments_only() -> Vec<u8> {
    only_attachments_encrypted(&unfiltered(false))
}

/// `file`, whose trailer has `/Root 1 0 R`, encrypted as
/// [`attachments_only`]'s is.
pub(super) fn only_attachments_encrypted(file: &[u8]) -> Vec<u8> {
    let zeros = hex(&[0; 32]);
    let encrypt = format!(
        " /Encrypt << /Filter /Standard /V 4 /R 4 /Length 128 /P -4 /O <{zeros}> /U <{zeros}> \
         /CF << /StdCF << /CFM /AESV2 /AuthEvent /EFOpen >> >> /StmF /Identity /StrF /Identity /EFF /StdCF >> \
         /ID [(0123456789abcdef) (0123456789abcdef)]"
    );
    with_trailer(file, &encrypt)
}

/// A file whose last object, 4, is `3` without its `endobj`, which object
/// 3's `/Length` refers to.
pub(super) fn length_by_reference() -> Vec<u8> {
    fixture::assembled(
        "1.7",
        &[
            (1, &b"<< /Type /Catalog >>"[..]),
            (2, b"<< /A 1 >>"),
            (3, b"<< /Length 4 0 R >>\nstream\nabc\nendstream"),
            (4, b"3 /Stray"),
        ],
        "<< /Size 5 /Root 1 0 R >>",
    )
}

/// [`filtered`]'s file, the page tree in its object stream with a key the
/// parser reads past (`/Mi#z`, an escape of one digit), and the object
/// stream's zlib data without its last byte, which decodes whole but for
/// its checksum: so that the object stream's decode warns, and so does an
/// object in it.
fn warning_twice() -> Vec<u8> {
    let (objects, _) = objects_2_and_3();
    let at = find(&objects, b"/Mine").unwrap();
    let objects = [&objects[..at], b"/Mi#z", &objects[at + 5..]].concat();
    let data = fixture::zlib(&objects);
    filtered(false, " /Filter /FlateDecode", &data[..data.len() - 1])
}

/// A file whose table lists object 0 in use.
fn object_0_in_use() -> Vec<u8> {
    let mut file = fixture::assembled(
        "1.7",
        &[(1, &b"<< /Type /Catalog >>"[..])],
        "<< /Size 2 /Root 1 0 R >>",
    );
    let row = find(&file, b"0000000000 65535 f").unwrap();
    file[row..row + 18].copy_from_slice(b"0000000009 00000 n");
    file
}

/// Whether the crate has the fixtures qpdf wrote: not where it's without
/// them, as its package is, which holds no `tests/files`, so a test of
/// them is skipped there, and one that counts what it read counts fewer.
pub(crate) fn fixtures() -> bool {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/files")
        .exists()
}

/// The fixture qpdf wrote, by name: none where the crate is without them
/// ([`fixtures`]).
pub(crate) fn fixture(name: &str) -> Option<Vec<u8>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/files");
    fixtures().then(|| std::fs::read(dir.join(format!("{name}.pdf"))).unwrap())
}

/// A test file: its name, its bytes, and the passwords it's read by.
pub(super) type Named = (String, Vec<u8>, Vec<Password<'static>>);

/// The files the tests' writer makes, each kind bytes.rs's tests read, and
/// the fixtures qpdf wrote, where they are: each named, with the passwords
/// it's read by.
pub(super) fn files() -> Vec<Named> {
    let none: Vec<Password> = Vec::new();
    let both = vec![Password("user"), Password("owner")];
    let mut files = vec![
        (
            "a table".to_string(),
            fixture::file("1.7", &clean(), root(), Xref::Table),
            none.clone(),
        ),
        (
            "a stream".to_string(),
            fixture::file("1.7", &clean(), root(), Xref::Stream),
            none.clone(),
        ),
        ("small".to_string(), small(), none.clone()),
        (
            "updated".to_string(),
            updated_with_padding().0,
            none.clone(),
        ),
        ("compressed".to_string(), compressed(false).0, none.clone()),
        (
            "compressed, encrypted".to_string(),
            compressed(true).0,
            none.clone(),
        ),
        ("junk".to_string(), junk(false).0, none.clone()),
        ("unfiltered".to_string(), unfiltered(false), none.clone()),
        (
            "unfiltered, encrypted".to_string(),
            unfiltered(true),
            none.clone(),
        ),
        (
            "a /Length by reference".to_string(),
            length_by_reference(),
            none.clone(),
        ),
        (
            "object 0 in use".to_string(),
            object_0_in_use(),
            none.clone(),
        ),
        (
            "attachments only".to_string(),
            attachments_only(),
            both.clone(),
        ),
    ];
    let (objects, _) = objects_2_and_3();
    let zlib = fixture::zlib(&objects);
    let filters = [
        (" /Filter /ASCIIHexDecode", hex(&objects).into_bytes()),
        (" /Filter /FlateDecode", zlib.clone()),
        // Cut off by a fault: the objects past it don't read.
        (" /Filter /FlateDecode", zlib[..zlib.len() / 2].to_vec()),
        (" /Filter /JBIG2Decode", objects.clone()),
    ];
    for (filter, data) in filters {
        let name = format!("filtered{filter}, {} bytes", data.len());
        files.push((name, filtered(false, filter, &data), none.clone()));
    }
    files.push((
        "an object stream and an object in it that warn".to_string(),
        warning_twice(),
        none.clone(),
    ));
    let mut scanned = fixture::file("1.7", &clean(), root(), Xref::Table);
    let at = rfind(&scanned, b"startxref").unwrap();
    scanned.truncate(at);
    files.push(("scanned".to_string(), scanned, none.clone()));
    for kind in KINDS {
        let file = fixture::encrypted(&settings(kind, b"user"), &to_encrypt(&[]), root());
        files.push((
            format!("{kind:?}, locked"),
            file.clone(),
            vec![Password("wrong")],
        ));
        files.push((format!("{kind:?}"), file, both.clone()));
    }
    let crypt = [
        " /Filter /FlateDecode",
        " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /StdCF >> null]",
        // A crypt filter its dictionary doesn't define.
        " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Undefined >> null]",
    ];
    for filter in crypt {
        let file = encrypted_objects(filter, None);
        files.push((
            format!("AES-256{filter}"),
            file.clone(),
            vec![Password("user")],
        ));
        files.push((format!("AES-256{filter}, locked"), file, none.clone()));
    }
    let zlib_plain = fixture::zlib(&objects);
    let identity = " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Identity >> null]";
    let file = encrypted_objects(identity, Some(&zlib_plain));
    files.push((
        "AES-256, an object stream of /Identity".to_string(),
        file,
        both.clone(),
    ));
    for name in [
        "qpdf-plain",
        "qpdf-compressed",
        "qpdf-rc4-40",
        "qpdf-rc4-128",
        "qpdf-aes-128",
        "qpdf-aes-256-r5",
        "qpdf-aes-256",
        "qpdf-aes-256-unpacked",
    ] {
        if let Some(file) = fixture(name) {
            files.push((name.to_string(), file.clone(), none.clone()));
            files.push((
                format!("{name}, by the owner's"),
                file,
                vec![Password("owner")],
            ));
        }
    }
    files
}

/// `file`, under `options`, opened by `passwords`, read on demand by
/// number, from the last back, and on eight threads, each taking every
/// eighth from the last back: each time, what it warned of, each object's
/// record and the metadata are the whole read's, and made a document it's
/// the whole read, but where either's decoding reached the budget. How
/// many of the three were compared.
fn on_demand(name: &str, file: &[u8], passwords: &[Password], options: &Options) -> usize {
    let whole = whole(file, passwords, options);
    let doc = &whole.document;
    let mut compared = 0;
    for order in ["by number", "from the last", "on eight threads"] {
        let reading = open(file, passwords, options);
        let ids: Vec<ObjectId> = reading.ids().collect();
        assert_eq!(ids, doc.ids(), "{name}");
        match order {
            "by number" => ids.iter().for_each(|&id| drop(reading.get(id))),
            "from the last" => ids.iter().rev().for_each(|&id| drop(reading.get(id))),
            _ => std::thread::scope(|scope| {
                for t in 0..8 {
                    let (reading, ids) = (&reading, &ids);
                    scope.spawn(move || {
                        for &id in ids.iter().rev().skip(t).step_by(8) {
                            let _ = reading.get(id);
                        }
                    });
                }
            }),
        }
        let warnings = reading.warnings();
        let records: BTreeMap<ObjectId, Record> = (ids.iter())
            .filter_map(|&id| Some((id, reading.record(id)?.clone())))
            .collect();
        let metadata = reading.metadata().unwrap().map(|m| m.into_owned());
        let lazily = reading.whole(None);
        if whole.reached || lazily.reached {
            continue;
        }
        assert_eq!(lazily.document, *doc, "{name}, {order}");
        assert_eq!(warnings, doc.warnings, "{name}, {order}");
        let held: BTreeMap<ObjectId, Record> = (doc.records())
            .map(|(id, record)| (id, record.clone()))
            .collect();
        assert_eq!(records, held, "{name}, {order}");
        assert_eq!(metadata, doc.metadata, "{name}, {order}");
        compared += 1;
    }
    compared
}

/// Every test file, and the revision before an update, read on demand in
/// three orders, is the whole read, which is `bytes::read_opened`'s.
#[test]
fn a_reading_on_demand_is_the_whole_read_in_any_order_on_any_thread() {
    let mut compared = 0;
    for (name, file, passwords) in files() {
        let options = Options::DEFAULT;
        assert_eq!(
            whole(&file, &passwords, &options).document,
            read_opened(&file, &passwords),
            "{name}"
        );
        compared += on_demand(&name, &file, &passwords, &options);
        if let Some(update) = read_opened(&file, &passwords).update {
            let mut options = Options::DEFAULT;
            options.section = Some(update.previous);
            assert_eq!(
                whole(&file, &passwords, &options).document,
                read_from_opened(&file, &passwords, update.previous),
                "{name}"
            );
            compared += on_demand(&format!("{name}, before"), &file, &passwords, &options);
        }
    }
    assert!(compared > 100, "{compared}");
}

/// Every test file, read whole and on demand, each stream decoded, and
/// the revision before an update, with its Flate inflated by zlib-rs: the
/// same documents, and data, as by miniz_oxide, the default.
#[test]
fn every_test_file_reads_the_same_on_either_flate_backend() {
    let mut zlib = Options::DEFAULT;
    zlib.flate = Backend::ZlibRs;
    let mut decoded = 0;
    for (name, file, passwords) in files() {
        let by_miniz = whole(&file, &passwords, &Options::DEFAULT);
        let by_zlib = whole(&file, &passwords, &zlib);
        assert_eq!(by_zlib.document, by_miniz.document, "{name}");
        on_demand(&name, &file, &passwords, &zlib);
        let (miniz, other) = (
            open(&file, &passwords, &Options::DEFAULT),
            open(&file, &passwords, &zlib),
        );
        for id in miniz.ids().collect::<Vec<_>>() {
            let data = |r: &Reading<&[u8]>| format!("{:?}", r.decoded(id));
            assert_eq!(data(&other), data(&miniz), "{name}, {id:?}");
            decoded += usize::from(miniz.decoded(id).is_ok());
        }
        if let Some(update) = by_miniz.document.update {
            let mut before = Options::DEFAULT;
            before.section = Some(update.previous);
            let mut zlib = before;
            zlib.flate = Backend::ZlibRs;
            let by_miniz = whole(&file, &passwords, &before);
            assert_eq!(
                whole(&file, &passwords, &zlib).document,
                by_miniz.document,
                "{name}"
            );
        }
    }
    let floor = if fixtures() { 50 } else { 20 };
    assert!(decoded > floor, "{decoded}");
}

/// What `reading` keeps: each object's record, by number, and its
/// warnings.
fn kept<S: ByteSource>(reading: &Reading<S>) -> (BTreeMap<ObjectId, Record>, Vec<Warning>) {
    let records = (reading.ids())
        .filter_map(|id| Some((id, reading.record(id)?.clone())))
        .collect();
    (records, reading.warnings())
}

/// `file`, under `options`, opened by `passwords`, made a document with its
/// reading kept: after nothing's read, after every object's got by number,
/// after every third, and on eight threads at once, each having got every
/// eighth from the last back. Each time, made after it, `into_document`'s
/// document is the same, and it's the whole read, but where either's
/// decoding reached the budget. Made on one thread, it keeps what a reading
/// keeps that has read every object unkept besides: nothing it read unkept
/// but the object streams and the objects on the way to them, which, from
/// nothing read, are all it keeps. How many were set beside the whole
/// read.
fn beside(name: &str, file: &[u8], passwords: &[Password], options: &Options) -> usize {
    let whole = whole(file, passwords, options);
    let mut compared = 0;
    let orders = [
        "nothing read",
        "every object",
        "every third",
        "on eight threads",
    ];
    for order in orders {
        let reading = open(file, passwords, options);
        let ids: Vec<ObjectId> = reading.ids().collect();
        let documents: Vec<Document> = match order {
            "on eight threads" => std::thread::scope(|scope| {
                let threads: Vec<_> = (0..8)
                    .map(|t| {
                        let (reading, ids) = (&reading, &ids);
                        scope.spawn(move || {
                            for &id in ids.iter().rev().skip(t).step_by(8) {
                                let _ = reading.get(id);
                            }
                            reading.to_document().unwrap()
                        })
                    })
                    .collect();
                threads.into_iter().map(|t| t.join().unwrap()).collect()
            }),
            _ => {
                let got: Vec<ObjectId> = match order {
                    "nothing read" => Vec::new(),
                    "every object" => ids.clone(),
                    _ => ids.iter().copied().step_by(3).collect(),
                };
                let twin = open(file, passwords, options);
                for reading in [&reading, &twin] {
                    got.iter().for_each(|&id| drop(reading.get(id)));
                }
                let document = reading.to_document().unwrap();
                ids.iter().for_each(|&id| drop(twin.object_unkept(id)));
                let (records, warnings) = kept(&reading);
                let (twins, said) = kept(&twin);
                assert_eq!(records, twins, "{name}, {order}: what's kept");
                let reached = |r: &Reading<&[u8]>| r.reader.reached.load(Ordering::Relaxed);
                if !reached(&reading) && !reached(&twin) {
                    assert_eq!(warnings, said, "{name}, {order}: what's said");
                }
                if order == "nothing read" {
                    // The object streams, and what their dictionaries refer
                    // to, which their filters may.
                    let mut ways = BTreeSet::new();
                    for &n in reading.reader.containers.keys() {
                        ways.insert((n, 0));
                        if let Ok(o) = document.get((n, 0)) {
                            ways.extend(o.references());
                        }
                    }
                    let unkept: Vec<_> = (records.keys()).filter(|id| !ways.contains(id)).collect();
                    assert!(unkept.is_empty(), "{name}: {unkept:?} kept");
                }
                vec![document]
            }
        };
        let lazily = reading.whole(None);
        for document in &documents {
            assert_eq!(*document, lazily.document, "{name}, {order}: made after it");
        }
        if whole.reached || lazily.reached {
            continue;
        }
        assert_eq!(documents[0], whole.document, "{name}, {order}");
        compared += 1;
    }
    compared
}

/// Every test file, and the revision before an update, made a document
/// with its reading kept, whatever it read before, by number or on eight
/// threads, is the whole read, and so is the reading made a document after
/// it; and it keeps nothing it read unkept but the object streams.
#[test]
fn a_document_made_with_its_reading_kept_is_the_whole_read_whatever_was_read() {
    let mut compared = 0;
    for (name, file, passwords) in files() {
        compared += beside(&name, &file, &passwords, &Options::DEFAULT);
        if let Some(update) = read_opened(&file, &passwords).update {
            let mut options = Options::DEFAULT;
            options.section = Some(update.previous);
            compared += beside(&format!("{name}, before"), &file, &passwords, &options);
        }
    }
    // The writer's files, and qpdf's where they are.
    let floor = if fixtures() { 150 } else { 100 };
    assert!(compared > floor, "{compared}");
}

/// A document made with its reading kept holds each stream's data where
/// it was written, and the reading beside it hands it out, as a reading of
/// its own does, and decoded: the file's bytes there, where nothing's
/// encrypted. Each test file.
#[test]
fn a_documents_stream_data_is_handed_out_by_the_reading_kept_beside_it() {
    let mut streams = 0;
    for (name, file, passwords) in files() {
        let reading = open(&file, &passwords, &Options::DEFAULT);
        let doc = reading.to_document().unwrap();
        let own = open(&file, &passwords, &Options::DEFAULT);
        for (id, object) in doc.objects() {
            let Value::Stream(stream) = object.value() else {
                continue;
            };
            let at = stream.data.at();
            assert!(at.is_some(), "{name}: {id:?}'s data isn't placed");
            let data = reading.data(id);
            let given = format!("{:?}", own.data(id));
            assert_eq!(format!("{data:?}"), given, "{name}: {id:?}'s data");
            let decoded = format!("{:?}", reading.decoded(id));
            assert_eq!(decoded, format!("{:?}", own.decoded(id)), "{name}: {id:?}");
            if doc.encryption == Encryption::None {
                let at = at.unwrap();
                assert_eq!(data.unwrap(), &file[at.start..at.end], "{name}: {id:?}");
            }
            streams += 1;
        }
        assert_eq!(reading.into_document().unwrap(), doc, "{name}");
    }
    // The writer's files, and qpdf's where they are.
    let floor = if fixtures() { 50 } else { 30 };
    assert!(streams > floor, "{streams}");
}

/// A document loaded from bytes as they are, under options (as of an
/// earlier section, and under limits a stream reaches) and by a password,
/// each a reading made a document, with the reading kept or not, which is
/// `bytes::read`, `read_from`, `read_limited` or `read_opened`'s.
#[test]
fn a_document_is_loaded_from_bytes_under_options_and_by_a_password() {
    let file = fixture::file("1.7", &clean(), root(), Xref::Stream);
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    assert_eq!(reading.to_document().unwrap(), read(&file));
    assert_eq!(reading.into_document().unwrap(), read(&file));

    let (file, _) = updated_with_padding();
    let previous = read(&file).update.unwrap().previous;
    let mut options = Options::DEFAULT;
    options.section = Some(previous);
    let reading = Reading::open(&file[..], &options).unwrap();
    let doc = reading.to_document().unwrap();
    assert_eq!(doc, read_from(&file, previous));
    assert_ne!(doc, read(&file));
    assert_eq!(reading.into_document().unwrap(), doc);

    let size = 64 << 10;
    let file = streams_of(2, size);
    let mut options = Options::DEFAULT;
    (options.limits.stream, options.limits.budget) = (size, size + size / 2);
    let reading = Reading::open(&file[..], &options).unwrap();
    let doc = reading.to_document().unwrap();
    assert!(doc.get((2, 0)).is_ok());
    assert!(doc.get((3, 0)).is_err(), "cut");
    assert_eq!(doc, read_limited(&file, size, size + size / 2));
    assert_eq!(reading.into_document().unwrap(), doc);

    for kind in KINDS {
        let file = fixture::encrypted(&settings(kind, b"user"), &to_encrypt(&[]), root());
        let doc = read_opened(&file, &[Password("user")]);
        assert!(
            matches!(doc.encryption, Encryption::Opened { owner: false, .. }),
            "{kind:?}: {:?}",
            doc.encryption
        );
        let reading =
            Reading::open_with(&file[..], &[Password("user")], &Options::DEFAULT).unwrap();
        assert_eq!(reading.to_document().unwrap(), doc, "{kind:?}");
        assert_eq!(reading.into_document().unwrap(), doc, "{kind:?}");
    }
}

/// A reading's types can be shared across threads, over each source in
/// memory.
#[test]
fn a_reading_is_send_and_sync_over_each_source() {
    fn shared<T: Send + Sync>() {}
    shared::<Reading<&[u8]>>();
    shared::<Reading<Vec<u8>>>();
    shared::<Reading<Arc<[u8]>>>();
    shared::<Reading<&Vec<u8>>>();
    shared::<Reading<Box<[u8]>>>();
    shared::<Reading<Box<dyn ByteSource>>>();
    shared::<Reading<Arc<dyn ByteSource>>>();
    #[cfg(any(unix, windows))]
    shared::<Reading<FileBytes>>();
    shared::<Identity>();
    shared::<SourceId>();
    shared::<Error>();
    shared::<Options>();
}

/// A source behind a box, or shared, or chosen as the program runs
/// (`dyn ByteSource`), reads as the source does, in memory or by position;
/// and the source a reading reads is handed back.
#[test]
fn a_source_behind_a_box_or_shared_reads_as_it_does() {
    let file = fixture::file("1.7", &clean(), root(), Xref::Stream);
    let doc = read(&file);
    let shared: Arc<dyn ByteSource> = Arc::new(file.clone());
    let reading = Reading::open(Arc::clone(&shared), &Options::DEFAULT).unwrap();
    assert!(reading.source().whole().is_some());
    let id = reading.id();
    let read = reading.into_document().unwrap();
    assert!(read.records().all(|(_, record)| record.source == id));
    assert_eq!(read, doc);
    let reading = Reading::open(Arc::clone(&shared), &Options::DEFAULT).unwrap();
    assert!(Arc::ptr_eq(&reading.into_source(), &shared));
    let boxed: Box<dyn ByteSource + '_> = Box::new(crate::fuzz::Counted::new(&file, 7));
    let reading = Reading::open(boxed, &Options::DEFAULT).unwrap();
    assert!(reading.source().whole().is_none(), "read by position");
    assert_eq!(reading.into_document().unwrap(), doc);
    let boxed: Box<[u8]> = file.clone().into_boxed_slice();
    let reading = Reading::open(boxed, &Options::DEFAULT).unwrap();
    assert_eq!(reading.into_document().unwrap(), doc);
}

/// Each source in memory reads alike, and each reading is a source of
/// its own.
#[test]
fn each_source_in_memory_reads_alike() {
    let file = fixture::file("1.7", &clean(), root(), Xref::Stream);
    let doc = read(&file);
    let shared: Arc<[u8]> = Arc::from(&file[..]);
    let readings = [
        Reading::open(file.clone(), &Options::DEFAULT)
            .unwrap()
            .into_document(),
        Reading::open(shared, &Options::DEFAULT)
            .unwrap()
            .into_document(),
        Reading::open(&file, &Options::DEFAULT)
            .unwrap()
            .into_document(),
    ];
    for reading in readings {
        assert_eq!(reading.unwrap(), doc);
    }
    let (one, two) = (
        open(&file, &[], &Options::DEFAULT),
        open(&file, &[], &Options::DEFAULT),
    );
    assert_ne!(one.id(), two.id());
    // Each record holds its reading's id, and two readings' records of one
    // object are equal all the same.
    one.get((1, 0)).unwrap();
    two.get((1, 0)).unwrap();
    let (first, second) = (one.record((1, 0)).unwrap(), two.record((1, 0)).unwrap());
    assert_eq!((first.source, second.source), (one.id(), two.id()));
    assert_eq!(first, second);
    assert_eq!(one.object_unkept((2, 0)).unwrap().1.source, one.id());
    assert_eq!(one.size(), file.len());
    assert_eq!(one.header_version(), doc.header);
    assert_eq!(*one.trailer(), doc.trailer);
    assert_eq!(*one.encryption(), doc.encryption);
    assert_eq!(*one.facts(), doc.facts);
    let entries: Vec<(u32, u16, XrefEntry)> = one.entries().collect();
    assert_eq!(entries.len(), 7, "{entries:?}");
    assert!(matches!(entries[0], (0, _, XrefEntry::Free)), "{entries:?}");
}

/// Each record's hash is of the bytes it names, as they were read, which
/// the writer checks a copy of them by: in every test file, read in
/// memory and by position, in windows of any size, each first window
/// reaching the next listed offset or not, the same; and bytes that differ
/// hash otherwise.
#[test]
fn a_record_s_hash_is_of_the_bytes_it_names_however_it_was_read() {
    let mut hashes: std::collections::HashMap<Vec<u8>, u64> = Default::default();
    for (name, file, passwords) in files() {
        let reading = open(&file, &passwords, &Options::DEFAULT);
        let doc = reading.to_document().unwrap();
        assert!(doc.records().next().is_some(), "{name}");
        for (id, record) in doc.records() {
            let bytes = reading.written(record.at).unwrap();
            assert_eq!(record.hash, Record::hash_of(&bytes), "{name}: {id:?}");
            let was = hashes.insert(bytes.into_owned(), record.hash);
            assert!(was.is_none_or(|was| was == record.hash), "{name}: {id:?}");
        }
        for first in [1, 7, 64, 4096] {
            for bound in [true, false] {
                let source = crate::fuzz::Counted::new(&file, first);
                let by = crate::fuzz::open(source, &passwords, &Options::DEFAULT, bound).unwrap();
                let read = by.to_document().unwrap();
                for (id, record) in doc.records() {
                    let hash = read.record(id).map(|r| r.hash);
                    assert_eq!(hash, Some(record.hash), "{name}, {first}, {bound}: {id:?}");
                }
            }
        }
    }
    let distinct: BTreeSet<u64> = hashes.values().copied().collect();
    assert_eq!(
        distinct.len(),
        hashes.len(),
        "bytes that differ hash otherwise"
    );
}

/// What the cross-references list as object 0, always free, is said as the
/// open's last warning, before any object is read.
#[test]
fn object_0_in_use_is_said_as_the_file_opens() {
    let file = object_0_in_use();
    let reading = open(&file, &[], &Options::DEFAULT);
    let warnings = reading.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].message.contains("object 0"));
    assert!(matches!(
        reading.get((0, 0)),
        Err(Error::Unread(Unread::Missing { id: (0, 0) }))
    ));
    assert_eq!(reading.ids().collect::<Vec<_>>(), [(1, 0)]);
}

/// A stream's `/Length` by reference is read again with the stream, its
/// faults said with the stream's, and again with its own object, as the
/// whole read says them.
#[test]
fn a_length_by_references_faults_are_said_with_the_stream_and_its_own() {
    let file = length_by_reference();
    let reading = open(&file, &[], &Options::DEFAULT);
    let stray = |w: &Warning| w.object == Some((4, 0)) && w.message == "4 0 obj has no endobj";
    let Value::Stream(stream) = reading.get((3, 0)).unwrap().value() else {
        panic!("not a stream");
    };
    let at = stream.data.at().unwrap();
    assert_eq!(&file[at.start..at.end], b"abc");
    let warnings = reading.warnings();
    assert!(warnings.len() == 1 && stray(&warnings[0]), "{warnings:?}");
    reading.get((4, 0)).unwrap();
    let warnings = reading.warnings();
    assert!(
        warnings.len() == 2 && warnings.iter().all(stray),
        "{warnings:?}"
    );
    assert_eq!(warnings, read(&file).warnings);
}

/// An object in an object stream is read on its own: its object stream is
/// decoded, and the stream itself kept, but not the others in it.
#[test]
fn an_object_in_an_object_stream_is_read_before_the_others_in_it() {
    let (file, _) = compressed(false);
    let reading = open(&file, &[], &Options::DEFAULT);
    let page = reading.get((3, 0)).unwrap();
    assert_eq!(page.at.unwrap().stream, Some((4, 0)));
    let record = reading.record((3, 0)).unwrap();
    let origin = Origin::Stream {
        stream: 4,
        index: 1,
    };
    assert_eq!(record.origin, origin);
    assert_eq!(Some(record.at), page.at);
    assert!(!record.endobj && record.data.is_none());
    assert!(
        reading.record((4, 0)).is_some(),
        "the object stream is kept"
    );
    assert!(reading.record((2, 0)).is_none(), "the page tree isn't read");
    assert!(reading.record((1, 0)).is_none(), "nor the catalog");
    let doc = read(&file);
    let pages = reading.get((2, 0)).unwrap();
    assert_eq!(pages, doc.get((2, 0)).unwrap());
    assert_eq!(reading.into_document().unwrap(), doc);
}

/// Read unkept, an object is given with the record a kept read would keep,
/// and nothing of it is kept: not it, its warnings, a failure to read it,
/// nor its record; but for the object stream it's in, decoded and kept.
#[test]
fn an_object_read_unkept_keeps_nothing_but_its_object_stream() {
    let file = fixture::assembled(
        "1.7",
        &[
            (1, &b"<< /Type /Catalog >>"[..]),
            (3, b"<< /Length 4 0 R >>\nstream\nabc\nendstream"),
            (4, b"3"),
            (5, b"<< /Kept true >> /Stray"),
            (6, b"<< /Broken"),
        ],
        "<< /Size 7 /Root 1 0 R >>",
    );
    let reading = open(&file, &[], &Options::DEFAULT);
    let (stream, record) = reading.object_unkept((3, 0)).unwrap();
    assert!(reading.record((3, 0)).is_none() && reading.record((4, 0)).is_none());
    assert_eq!(&stream, reading.get((3, 0)).unwrap());
    assert_eq!(Some(&record), reading.record((3, 0)));
    // Its warnings aren't kept, nor its failure.
    let (_, record) = reading.object_unkept((5, 0)).unwrap();
    assert!(!record.endobj);
    assert!(reading.warnings().is_empty(), "{:?}", reading.warnings());
    assert!(reading.object_unkept((6, 0)).is_err());
    assert!(reading.warnings().is_empty(), "{:?}", reading.warnings());
    assert!(reading.record((6, 0)).is_none());
    assert!(matches!(
        reading.object_unkept((9, 0)),
        Err(Error::Unread(Unread::Missing { id: (9, 0) }))
    ));
    // Kept, it's given as it's kept.
    let kept = reading.get((5, 0)).unwrap().clone();
    assert_eq!(reading.object_unkept((5, 0)).unwrap().0, kept);
    assert_eq!(reading.warnings().len(), 1);

    // An object in an object stream: the stream is decoded and kept, with
    // the object stream's own object, and nothing of it.
    let (file, _) = compressed(false);
    let reading = open(&file, &[], &Options::DEFAULT);
    let (page, record) = reading.object_unkept((3, 0)).unwrap();
    assert!(reading.record((3, 0)).is_none());
    assert!(
        reading.record((4, 0)).is_some(),
        "the object stream is kept"
    );
    assert!(reading.reader.containers[&4].get().is_some(), "and decoded");
    assert_eq!(&page, reading.get((3, 0)).unwrap());
    assert_eq!(Some(&record), reading.record((3, 0)));
}

/// An object's record says whether `endobj` follows it, its extent
/// running past it when it does, and to its value's end when it doesn't;
/// a stream's, how its data was chosen.
#[test]
fn an_objects_record_says_whether_endobj_follows_it() {
    let file = fixture::assembled(
        "1.7",
        &[
            (1, &b"<< /Type /Catalog >>"[..]),
            (2, b"<< /A 1 >> /Stray"),
            (3, b"<< /Length 4 0 R >>\nstream\nabc\nendstream"),
            (4, b"3"),
            (5, b"<< /Length 99 >>\nstream\nabcdef\nendstream"),
        ],
        "<< /Size 6 /Root 1 0 R >>",
    );
    let doc = read(&file);
    let extent = |n: u32| {
        let at = doc.record((n, 0)).unwrap().at;
        &file[at.start..at.end]
    };
    assert!(doc.record((1, 0)).unwrap().endobj);
    assert_eq!(extent(1), b"1 0 obj\n<< /Type /Catalog >>\nendobj");
    assert!(!doc.record((2, 0)).unwrap().endobj);
    assert_eq!(extent(2), b"2 0 obj\n<< /A 1 >>");
    assert_eq!(doc.record((1, 0)).unwrap().origin, Origin::File);
    let data = |n: u32| doc.record((n, 0)).unwrap().data.unwrap();
    assert!(data(3).declared && data(3).indirect_length);
    assert_eq!(&file[data(3).at.start..data(3).at.end], b"abc");
    assert!(!data(5).declared && !data(5).indirect_length);
    assert_eq!(&file[data(5).at.start..data(5).at.end], b"abcdef\n");
    assert!(extent(5).ends_with(b"endstream\nendobj"));
    assert_eq!(doc.record((4, 0)).unwrap().data, None);

    // Scanned, each is recovered.
    let mut scanned = file.clone();
    scanned.truncate(rfind(&file, b"startxref").unwrap());
    let doc = read(&scanned);
    assert!(doc.trailer.unwritten.is_some());
    assert!(doc.records().all(|(_, r)| r.origin == Origin::Recovered));
}

/// The budget is spent in the order streams are decoded: read whole, by
/// number, the first object stream decodes and the second is cut; on
/// demand, the one an object is first asked of; made a document with the
/// reading kept, the rest by number. Each says it reached the budget.
#[test]
fn the_budget_cuts_the_stream_decoded_last_whole_by_number_on_demand_as_asked() {
    let size = 64 << 10;
    let file = streams_of(2, size);
    let mut options = Options::DEFAULT;
    options.limits.stream = size;
    options.limits.budget = size + size / 2;
    let read = whole(&file, &[], &options);
    let doc = &read.document;
    assert!(read.reached);
    assert!(doc.get((2, 0)).is_ok());
    assert!(doc.get((3, 0)).is_err(), "cut");
    let reading = open(&file, &[], &options);
    assert!(reading.get((3, 0)).is_ok());
    assert!(reading.get((2, 0)).is_err(), "cut");
    let lazily = reading.whole(None);
    assert!(lazily.reached);
    assert_ne!(lazily.document, *doc);
    // In number order, the whole read's.
    let reading = open(&file, &[], &options);
    reading.ids().for_each(|id| drop(reading.get(id)));
    assert_eq!(reading.into_document().unwrap(), *doc);
    // Made a document with the reading kept, from nothing read, its object
    // streams decoded by number, the whole read's; after the second's
    // asked for, the second's whole, and the reading made a document after
    // it the same.
    let reached = |reading: &Reading<&[u8]>| reading.reader.reached.load(Ordering::Relaxed);
    let reading = open(&file, &[], &options);
    assert_eq!(reading.to_document().unwrap(), *doc);
    assert!(reached(&reading));
    let reading = open(&file, &[], &options);
    assert!(reading.get((3, 0)).is_ok());
    let kept = reading.to_document().unwrap();
    assert!(kept.get((3, 0)).is_ok() && kept.get((2, 0)).is_err(), "cut");
    assert!(reached(&reading));
    assert_eq!(reading.into_document().unwrap(), kept);
    // With the budget for both, neither reaches it.
    options.limits.budget = 2 * size;
    let read = whole(&file, &[], &options);
    assert!(!read.reached);
    assert!(read.document.get((3, 0)).is_ok());
}

/// Object streams decoded on many threads at once never take the budget
/// past none, which an unsigned count would wrap to nearly all of it:
/// what's left is none, and a stream decoded after them decodes nothing.
#[test]
fn threads_decoding_near_the_budget_never_wrap_it() {
    let size = 16 << 10;
    let file = streams_of(17, size);
    let mut options = Options::DEFAULT;
    options.limits.budget = 2 * size;
    for _ in 0..20 {
        let reading = open(&file, &[], &options);
        std::thread::scope(|scope| {
            for t in 0..8u32 {
                let reading = &reading;
                scope.spawn(move || {
                    for n in [2 + 2 * t, 3 + 2 * t] {
                        let _ = reading.get((n, 0));
                    }
                });
            }
        });
        assert_eq!(reading.reader.budget.load(Ordering::Relaxed), 0);
        assert!(reading.get((18, 0)).is_err(), "nothing left to decode it");
    }
}

/// A file of a catalog and `count` objects, 2 and on, each in an object
/// stream of its own, which decodes to `size` bytes: its header, and a
/// string that fills the rest. A cross-reference stream lists them, its
/// fields 1, 4 and 1 bytes wide.
fn streams_of(count: u32, size: usize) -> Vec<u8> {
    let xref_row = |kind: u8, field: usize, index: u8| {
        let field = u32::try_from(field).unwrap().to_be_bytes();
        [&[kind][..], &field, &[index]].concat()
    };
    let mut file = b"%PDF-1.7\n".to_vec();
    let one = file.len();
    file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
    let first = 2 + count;
    let mut rows = vec![xref_row(0, 0, 255), xref_row(1, one, 0)];
    let mut containers = Vec::new();
    for n in 2..first {
        rows.push(xref_row(2, (first + n - 2) as usize, 0));
        let header = format!("{n} 0 ");
        let string = format!("({})", "x".repeat(size - header.len() - 2));
        let data = fixture::zlib(format!("{header}{string}").as_bytes());
        containers.push((first + n - 2, header.len(), data));
    }
    for (number, first, data) in containers {
        rows.push(xref_row(1, file.len(), 0));
        let dict = format!(
            "<< /Type /ObjStm /N 1 /First {first} /Filter /FlateDecode /Length {} >>",
            data.len()
        );
        file.extend_from_slice(format!("{number} 0 obj\n{dict}\nstream\n").as_bytes());
        file.extend_from_slice(&data);
        file.extend_from_slice(b"\nendstream\nendobj\n");
    }
    let xref = file.len();
    let number = 2 + 2 * count;
    rows.push(xref_row(1, xref, 0));
    let rows = rows.concat();
    let dict = format!(
        "<< /Type /XRef /Size {} /W [1 4 1] /Root 1 0 R /Length {} >>",
        number + 1,
        rows.len()
    );
    file.extend_from_slice(format!("{number} 0 obj\n{dict}\nstream\n").as_bytes());
    file.extend_from_slice(&rows);
    file.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    file
}

/// An object stream that inflates to 16 MiB and says it holds a billion
/// objects reads, under a stream's cap of 1 MiB, quickly: cut at the cap,
/// and its objects unread, as the whole read reads it.
#[test]
fn an_object_stream_bomb_is_cut_at_its_cap_quickly() {
    let bomb = fixture::zlib(&vec![0; 16 << 20]);
    let mut file = b"%PDF-1.7\n".to_vec();
    let one = file.len();
    file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let three = file.len();
    let dict = format!(
        "<< /Type /ObjStm /N 1000000000 /First 0 /Filter /FlateDecode /Length {} >>",
        bomb.len()
    );
    file.extend_from_slice(format!("3 0 obj\n{dict}\nstream\n").as_bytes());
    file.extend_from_slice(&bomb);
    file.extend_from_slice(b"\nendstream\nendobj\n");
    let four = file.len();
    let rows = [
        xref_row(0, 0, 255),
        xref_row(1, one, 0),
        xref_row(2, 3, 0),
        xref_row(1, three, 0),
        xref_row(1, four, 0),
    ]
    .concat();
    let dict = format!(
        "<< /Type /XRef /Size 5 /W [1 2 1] /Root 1 0 R /Length {} >>",
        rows.len()
    );
    file.extend_from_slice(format!("4 0 obj\n{dict}\nstream\n").as_bytes());
    file.extend_from_slice(&rows);
    file.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{four}\n%%EOF\n").as_bytes());
    let mut options = Options::DEFAULT;
    options.limits.stream = 1 << 20;
    let started = Instant::now();
    let reading = open(&file, &[], &options);
    assert!(reading.get((1, 0)).is_ok());
    for _ in 0..2 {
        let why = reading.get((2, 0)).unwrap_err().to_string();
        assert_eq!(why, "its header has 0 of its 1000000000 pairs");
    }
    let warnings = reading.warnings();
    let limit =
        "its data doesn't decode cleanly through /FlateDecode: its output is over the limit";
    assert!(warnings.iter().any(|w| w.message == limit), "{warnings:?}");
    assert_eq!(
        reading.into_document().unwrap(),
        whole(&file, &[], &options).document
    );
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
}

/// A cross-reference stream that inflates to 16 MiB is decoded no
/// further than its `/W` and `/Size` make its rows, quickly.
#[test]
fn a_cross_reference_stream_bomb_is_decoded_no_further_than_its_rows() {
    let catalog = b"1 0 obj\n<< /Type /Catalog >>\nendobj\n";
    let two = 9 + catalog.len();
    let mut rows = [0, 0, 0, 1, 9, 0, 1, 0, 0].to_vec();
    rows[7] = u8::try_from(two).unwrap();
    let mut data = rows.clone();
    data.resize(16 << 20, 0);
    let bomb = fixture::zlib(&data);
    let dict = format!(
        "<< /Type /XRef /Size 3 /W [1 1 1] /Root 1 0 R /Filter /FlateDecode /Length {} >>",
        bomb.len()
    );
    let file = [
        &b"%PDF-1.7\n"[..],
        catalog,
        format!("2 0 obj\n{dict}\nstream\n").as_bytes(),
        &bomb,
        format!("\nendstream\nendobj\nstartxref\n{two}\n%%EOF\n").as_bytes(),
    ]
    .concat();
    let started = Instant::now();
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(reading.trailer().unwritten, None);
    assert!(reading.get((1, 0)).is_ok());
    assert!(reading.warnings().is_empty(), "{:?}", reading.warnings());
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
}

/// A file that's only an object stream's members and an encryption
/// dictionary that encrypts only embedded files reads whole without a
/// password, on demand as whole.
#[test]
fn a_file_encrypting_only_its_attachments_reads_on_demand_as_whole() {
    let file = attachments_only();
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(*reading.encryption(), Encryption::Plain);
    assert!(reading.get((3, 0)).is_ok());
    assert_eq!(
        on_demand("attachments only", &file, &[], &Options::DEFAULT),
        3
    );
}

/// An object stream whose own `/Crypt` filter names a crypt filter the
/// encryption dictionary doesn't define reads on demand as whole.
#[test]
fn an_undefined_crypt_filter_reads_on_demand_as_whole() {
    let filter = " /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Undefined >> null]";
    let file = encrypted_objects(filter, None);
    let doc = read_opened(&file, &[Password("user")]);
    let by = Some((0, pdf_crypt::Encoding::Utf8));
    assert_eq!(doc.encryption, Encryption::Opened { owner: false, by });
    assert_eq!(
        on_demand("undefined", &file, &[Password("user")], &Options::DEFAULT),
        3
    );
}

/// An encrypted reading hands out no bytes of the file as written, which
/// may be ciphertext, but its object streams' decoded data, decrypted;
/// one in plain view hands out what's asked, within the file.
#[test]
fn an_encrypted_readings_bytes_are_refused_but_its_object_streams() {
    let file = encrypted_objects(" /Filter /FlateDecode", None);
    let reading = open(&file, &[Password("user")], &Options::DEFAULT);
    let pages = reading.get((2, 0)).unwrap().at.unwrap();
    let (objects, _) = objects_2_and_3();
    assert_eq!(
        reading.bytes(pages).unwrap(),
        &objects[pages.start..pages.end]
    );
    assert!(matches!(reading.bytes(span(0, 8)), Err(Error::Encrypted)));
    let past = Span::new(Some((4, 0)), 0, objects.len() + 1);
    assert!(matches!(reading.bytes(past), Err(Error::Outside { .. })));
    let file = fixture::file("1.7", &clean(), root(), Xref::Table);
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(reading.bytes(span(0, 8)).unwrap(), &b"%PDF-1.7"[..]);
    let past = span(0, file.len() + 1);
    let said = reading.bytes(past).unwrap_err().to_string();
    assert_eq!(
        said,
        format!("bytes 0..{} aren't in the file", file.len() + 1)
    );
    let none = Span::new(Some((9, 0)), 0, 1);
    assert!(matches!(reading.bytes(none), Err(Error::Outside { .. })));
}

/// What `reading` reads, asked for each of `ids` in turn: each object and
/// its record, or why it doesn't read; then what it warned of, and how
/// the file is encrypted, `by` given as the index of the password the
/// encoding it names is of, by `of`.
type Read = (
    Vec<Result<(Object, Option<Record>), String>>,
    Vec<Warning>,
    Encryption,
);

/// [`Read`]'s, of `reading`.
fn read_by(reading: &Reading<&[u8]>, ids: &[ObjectId], of: &[usize]) -> Read {
    let objects = (ids.iter())
        .map(|&id| match reading.get(id) {
            Ok(object) => Ok((object.clone(), reading.record(id).cloned())),
            Err(why) => Err(why.to_string()),
        })
        .collect();
    let mut encryption = reading.encryption().clone();
    if let Encryption::Opened {
        by: Some((i, _)), ..
    } = &mut encryption
    {
        *i = of[*i];
    }
    (objects, reading.warnings(), encryption)
}

/// Each of `passwords` in each of its encodings, and which password each
/// is of.
fn encodings(passwords: &[Password]) -> (Vec<Encoded>, Vec<usize>) {
    (passwords.iter().enumerate())
        .flat_map(|(i, password)| password.encodings().into_iter().map(move |e| (e, i)))
        .unzip()
}

/// A reading [`Reading::open`] left locked, unlocked by the encodings of
/// the passwords a test file is read by, reads what one opened by them
/// does ([`Reading::open_with`]): each object and its record, what it
/// warns of, in order, and its encryption, `by` the password's index;
/// and read whole, the same document, its decoded object streams among
/// it. Whether it had read nothing, every object, or every third, by
/// number or from the last, before it was unlocked. Every test file and
/// qpdf's fixtures; a file the empty password opens, or that isn't
/// encrypted, is given back as it was.
#[test]
fn a_reading_unlocked_reads_as_one_opened_by_the_password() {
    let mut unlocked = 0;
    for (name, file, passwords) in files() {
        let (encoded, of) = encodings(&passwords);
        for asked in ["nothing", "every object", "every third, from the last"] {
            let what = format!("{name}, {asked}");
            let reading = open(&file, &[], &Options::DEFAULT);
            let ids: Vec<ObjectId> = reading.ids().collect();
            let before: Vec<ObjectId> = match asked {
                "nothing" => Vec::new(),
                "every object" => ids.clone(),
                _ => ids.iter().rev().copied().step_by(3).collect(),
            };
            for &id in &before {
                drop(reading.get(id));
            }
            let was = reading.encryption().clone();
            let reading = match reading.unlock(&encoded) {
                Ok(reading) => reading,
                Err((reading, _)) => reading,
            };
            if matches!(was, Encryption::None | Encryption::Opened { .. }) {
                assert_eq!(*reading.encryption(), was, "{what}");
                continue;
            }
            let opened = open(&file, &passwords, &Options::DEFAULT);
            let order: Vec<ObjectId> = ids.iter().rev().copied().collect();
            assert_eq!(
                read_by(&reading, &order, &of),
                read_by(&opened, &order, &[0]),
                "{what}"
            );
            let documents = [reading, opened].map(|r| r.whole(None));
            let mut doc = documents[0].document.clone();
            if let Encryption::Opened {
                by: Some((i, _)), ..
            } = &mut doc.encryption
            {
                *i = of[*i];
            }
            assert_eq!(doc, documents[1].document, "{what}: whole");
            assert_eq!(
                doc,
                whole(&file, &passwords, &Options::DEFAULT).document,
                "{what}"
            );
            unlocked += usize::from(matches!(doc.encryption, Encryption::Opened { .. }));
        }
    }
    // The writer's files no empty password opens, three times each.
    assert!(unlocked >= 27, "{unlocked}");
}

/// A reading unlocked decodes as one opened by the password does, from
/// what was left of the budget when it opened, whatever it decoded
/// locked; and what `/Perms` says is wrong with the encryption dictionary
/// is said where opening the file says it: after what it warned of before
/// it read the dictionary, and before what it warned of after.
#[test]
fn a_reading_unlocked_decodes_on_what_opening_left_and_says_what_opening_says() {
    // Its object stream decrypts as written (/StmF /Identity), and decodes
    // locked, spending the budget, which is its decoded data's length, as
    // the limit on a stream is.
    let kind = pdf_crypt::writer::Kind::Aes128;
    let values = pdf_crypt::writer::dictionary(&settings(kind, b"user"));
    let encrypt = format!(
        "/Root 1 0 R /Encrypt << /Filter /Standard /V 4 /R 4 /Length 128 /P -4 /O <{}> /U <{}> \
         /CF << /StdCF << /CFM /AESV2 >> >> /StmF /Identity /StrF /Identity >> \
         /ID [(0123456789abcdef) (0123456789abcdef)]",
        hex(values.o.as_deref().unwrap()),
        hex(values.u.as_deref().unwrap())
    );
    let (objects, _) = objects_2_and_3();
    let file = filtered(false, " /Filter /FlateDecode", &fixture::zlib(&objects));
    let at = find(&file, b"/Root 1 0 R").unwrap();
    let file = [&file[..at], encrypt.as_bytes(), &file[at + 11..]].concat();
    let mut options = Options::DEFAULT;
    (options.limits.stream, options.limits.budget) = (objects.len(), objects.len());
    let user = [Password("user")];
    let reading = Reading::open(&file[..], &options).unwrap();
    assert_eq!(*reading.encryption(), Encryption::Plain);
    assert!(reading.get((2, 0)).is_ok());
    assert_eq!(reading.reader.budget.load(Ordering::Relaxed), 0, "spent");
    let reading = reading.unlock(&user[0].encodings()).unwrap();
    let opened = Reading::open_with(&file[..], &user, &options).unwrap();
    assert_eq!(
        read_by(&reading, &[(2, 0)], &[0]),
        read_by(&opened, &[(2, 0)], &[0])
    );
    assert!(reading.get((2, 0)).is_ok(), "decoded again");
    assert!(!reading.reader.reached.load(Ordering::Relaxed));

    // /P not what /Perms holds; the file going on past its %%EOF, which is
    // said before the dictionary's read, and object 0 in use, said after.
    let file = encrypted_objects(" /Filter /FlateDecode", None);
    let at = find(&file, b"/P -4").unwrap();
    let mut file = [&file[..at], b"/P -8", &file[at + 5..]].concat();
    // Object 0's row, the cross-reference stream's first, made one in use.
    let rows = rfind(&file, b">>\nstream\n").unwrap() + 10;
    file[rows] = 1;
    file.extend_from_slice(b"% more\n");
    let reading = open(&file, &[], &Options::DEFAULT);
    assert_eq!(*reading.encryption(), Encryption::Locked);
    let said = |r: &Reading<&[u8]>| -> Vec<String> {
        r.warnings().into_iter().map(|w| w.message).collect()
    };
    let locked = said(&reading);
    assert_eq!(locked.len(), 2, "{locked:?}");
    let reading = reading.unlock(&user[0].encodings()).unwrap();
    let opened = open(&file, &user, &Options::DEFAULT);
    let unlocked = said(&reading);
    assert_eq!(unlocked, said(&opened));
    let perms = "its encryption dictionary isn't right: its /Perms doesn't match its /P and /EncryptMetadata";
    assert_eq!(unlocked, [&locked[0], perms, &locked[1]]);
}

/// A reading [`Reading::unlock`] doesn't open is given back as it was,
/// with why: by passwords none of which opens it, keeping what it read and
/// said, and opened by the right one after; by a security handler the
/// reader can't open, or an `/Encrypt` that isn't a dictionary, saying so
/// as its encryption does. One not encrypted, or that a password opened,
/// is given back as it is.
#[test]
fn a_reading_unlock_doesnt_open_is_given_back_as_it_was() {
    let kind = pdf_crypt::writer::Kind::Aes256;
    let file = fixture::encrypted(&settings(kind, b"user"), &to_encrypt(&[]), root());
    let reading = open(&file, &[], &Options::DEFAULT);
    let ids: Vec<ObjectId> = reading.ids().collect();
    ids.iter().for_each(|&id| drop(reading.get(id)));
    let kept_before = kept(&reading);
    let id = reading.id();
    let wrong = Password("wrong").encodings();
    let Err((reading, why)) = reading.unlock(&wrong) else {
        panic!("a wrong password opened it");
    };
    assert_eq!(why, Refused::Password);
    assert_eq!(*reading.encryption(), Encryption::Locked);
    assert_eq!((kept(&reading), reading.id()), (kept_before, id));
    let reading = reading.unlock(&Password("owner").encodings()).unwrap();
    let by = Some((0, pdf_crypt::Encoding::Utf8));
    assert_eq!(
        *reading.encryption(),
        Encryption::Opened { owner: true, by }
    );
    assert_eq!(reading.id(), id, "the same reading");

    // A handler the reader can't open; an /Encrypt that isn't a
    // dictionary.
    let at = find(&file, b"/Standard").unwrap();
    let other = [&file[..at], b"/Standarx", &file[at + 9..]].concat();
    let not_one = with_trailer(
        &fixture::file("1.7", &clean(), root(), Xref::Table),
        " /Encrypt 7",
    );
    for file in [other, not_one] {
        let reading = open(&file, &[], &Options::DEFAULT);
        let Encryption::Unsupported(said) = reading.encryption().clone() else {
            panic!("{:?}", reading.encryption());
        };
        let warnings = reading.warnings();
        let Err((reading, why)) = reading.unlock(&Password("user").encodings()) else {
            panic!("unlocked");
        };
        assert_eq!(why, Refused::Unsupported(said.clone()));
        assert_eq!(*reading.encryption(), Encryption::Unsupported(said));
        assert_eq!(reading.warnings(), warnings);
    }

    // Not encrypted, and opened by the empty password: as they are.
    let plain = fixture::file("1.7", &clean(), root(), Xref::Table);
    let opened = fixture::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
    for (file, encryption) in [
        (plain, Encryption::None),
        (
            opened,
            Encryption::Opened {
                owner: false,
                by: None,
            },
        ),
    ] {
        let reading = open(&file, &[], &Options::DEFAULT);
        reading.get((1, 0)).unwrap();
        let before = kept(&reading);
        let reading = reading.unlock(&Password("owner").encodings()).unwrap();
        assert_eq!(*reading.encryption(), encryption);
        assert_eq!(kept(&reading), before);
    }
}

/// [`Reading::open_with`] tries the passwords given in the empty one's
/// place: on a file whose user password is empty, a wrong one leaves it
/// locked, and the owner's opens it as the owner's, `by` saying which and
/// in which encoding; the empty password, given none.
#[test]
fn open_with_tries_the_passwords_given_in_the_empty_ones_place() {
    let kind = pdf_crypt::writer::Kind::Aes256;
    let file = fixture::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
    let by = |passwords: &[Password]| {
        Reading::open_with(&file[..], passwords, &Options::DEFAULT)
            .unwrap()
            .encryption()
            .clone()
    };
    assert_eq!(by(&[Password("wrong")]), Encryption::Locked);
    let utf8 = pdf_crypt::Encoding::Utf8;
    assert_eq!(
        by(&[Password("wrong"), Password("owner")]),
        Encryption::Opened {
            owner: true,
            by: Some((1, utf8))
        }
    );
    assert_eq!(
        by(&[]),
        Encryption::Opened {
            owner: false,
            by: None
        }
    );
    assert_eq!(
        by(&[]),
        *Reading::open(&file[..], &Options::DEFAULT)
            .unwrap()
            .encryption()
    );
    // A password typed in a single-byte encoding opens a revision 4 file
    // whose password was set in it, by that encoding.
    let pdf_doc = pdf_crypt::writer::Settings {
        user: b"caf\xE9",
        ..settings(pdf_crypt::writer::Kind::Aes128, b"")
    };
    let file = fixture::encrypted(&pdf_doc, &to_encrypt(&[]), root());
    let reading = Reading::open_with(&file[..], &[Password("café")], &Options::DEFAULT).unwrap();
    let by = Some((0, pdf_crypt::Encoding::PdfDoc));
    assert_eq!(
        *reading.encryption(),
        Encryption::Opened { owner: false, by }
    );
}

/// A linearization holds while its dictionary is the file's first
/// object, whole within 1024 bytes of the header, its `/Linearized` is 1
/// (a number whose floor is 1, as qpdf reads it) and its `/L` is a direct
/// integer, the file's length from the header (ISO 32000-2, Annex F); an
/// update appended voids it.
#[test]
fn a_linearization_holds_while_its_length_is_the_files() {
    let file = linearized(b"");
    let len = file.len();
    // `file` with `from` replaced by `to`, padded to its width.
    let edited = |from: &str, to: &str| {
        let at = find(&file, from.as_bytes()).unwrap();
        let to = format!("{to:<width$}", width = from.len());
        let mut bytes = file.clone();
        bytes[at..][..from.len()].copy_from_slice(to.as_bytes());
        bytes
    };
    let l = format!("/L {len:<10}");
    // `file` with `/Linearized` given the value `v`.
    let version = |v: &str| {
        edited(
            &format!("/Linearized 1 {l}"),
            &format!("/Linearized {v} /L {len}"),
        )
    };
    // The dictionary ends `end` bytes into the file: a comment of `n`
    // bytes before it ends it at byte `end + n`.
    let end = find(&file, b">>").unwrap() + 2;
    let comment = |n: usize| [b"%".as_slice(), &vec![b'x'; n - 2], b"\n"].concat();
    let prev = startxref_at(&file);
    let mut updated = file.clone();
    let object = updated.len();
    updated.extend_from_slice(b"6 0 obj\n(an update)\nendobj\n");
    let section = updated.len();
    updated.extend_from_slice(
        format!(
            "xref\n6 1\n{object:010} 00000 n \ntrailer\n<< /Size 7 /Root 1 0 R /Prev {prev} >>\n\
             startxref\n{section}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let prefix = [b'~'; 100];
    let counted = edited(&l, &format!("/L {}", len + prefix.len()));
    let cases = [
        ("/L the length", file.clone(), true),
        (
            "/L one past it",
            edited(&l, &format!("/L {}", len + 1)),
            false,
        ),
        ("no /L", edited(&l, ""), false),
        ("/L a reference", edited(&l, "/L 9 0 R"), false),
        ("/L a real", edited(&l, &format!("/L {len}.0")), false),
        ("no /Linearized", edited("/Linearized 1", ""), false),
        ("/Linearized 1.0", version("1.0"), true),
        ("/Linearized 1.5", version("1.5"), true),
        ("/Linearized 0.9", version("0.9"), false),
        ("/Linearized 2", version("2"), false),
        ("/Linearized true", version("true"), false),
        ("/Linearized null", version("null"), false),
        ("/Linearized a name", version("/One"), false),
        (
            "another object first",
            linearized(b"9 0 obj\nnull\nendobj\n"),
            false,
        ),
        (
            "a stream's dictionary",
            edited(">>\nendobj", ">>\nstream"),
            false,
        ),
        (
            "ending at byte 1024",
            linearized(&comment(1024 - end)),
            true,
        ),
        // Stricter than qpdf, which asks only that the object start
        // within the 1024 bytes; Annex F has the whole dictionary within
        // them.
        (
            "ending at byte 1025",
            linearized(&comment(1025 - end)),
            false,
        ),
        ("junk before", [&prefix[..], &file].concat(), true),
        (
            "junk before, counted",
            [&prefix[..], &counted].concat(),
            false,
        ),
        ("an update after", updated, false),
    ];
    for (name, bytes, holds) in cases {
        let reading = open(&bytes, &[], &Options::DEFAULT);
        assert_eq!(reading.facts().linearized, holds, "{name}");
        assert_eq!(read(&bytes).facts.linearized, holds, "{name}");
    }
}

/// A linearized file of a page, `lead` between its header and its first
/// object, the linearization dictionary, whose `/L` is the file's length.
fn linearized(lead: &[u8]) -> Vec<u8> {
    const L: &str = "/L 0000000000";
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    out.extend_from_slice(lead);
    let content = b"q 0 0 1 RG 72 72 m 540 720 l S Q";
    let objects = [
        (
            5,
            format!("<< /Linearized 1 {L} /O 3 /E 0 /N 1 /T 0 /H [0 0] >>").into_bytes(),
        ),
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> \
              /Contents 4 0 R >>"
                .to_vec(),
        ),
        (
            4,
            [
                format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
                content,
                b"\nendstream",
            ]
            .concat(),
        ),
    ];
    let mut offsets = BTreeMap::new();
    for (number, body) in objects {
        offsets.insert(number, out.len());
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets.values() {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let tail = format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n");
    out.extend_from_slice(tail.as_bytes());
    let at = find(&out, L.as_bytes()).unwrap();
    let length = format!("/L {:<10}", out.len());
    out[at..at + L.len()].copy_from_slice(length.as_bytes());
    out
}

/// Where the last `startxref` of `file` says its section starts.
fn startxref_at(file: &[u8]) -> usize {
    let at = rfind(file, b"startxref").unwrap() + b"startxref\n".len();
    let digits: String = (file[at..].iter())
        .take_while(|b| b.is_ascii_digit())
        .map(|&b| char::from(b))
        .collect();
    digits.parse().unwrap()
}

/// What a reading keeps for each entry its cross-references list: a slot,
/// a pointer and its lock's state, and, once read, the box it points to.
#[test]
fn a_slot_is_a_pointer_and_its_lock() {
    use std::mem::size_of;
    let slot = size_of::<OnceLock<Box<Read>>>();
    println!(
        "a slot: {slot} bytes; read, its box: {} bytes",
        size_of::<Read>()
    );
    println!("an entry: {} bytes", size_of::<(u32, (u16, XrefEntry))>());
    println!("a record: {} bytes", size_of::<Record>());
    println!(
        "an object stream, decoded: {} bytes",
        size_of::<Container>()
    );
    println!("a reading: {} bytes", size_of::<Reading<&[u8]>>());
    assert!(slot <= 2 * size_of::<usize>(), "{slot}");
}

/// Every file in the folder `PDF_LINT_CORPUS` names, read whole and on
/// demand, from the last object back on eight threads, then made a
/// document: the two the same, but where either's decoding reached the
/// budget.
#[test]
#[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files"]
fn a_corpus_reads_alike_whole_and_lazily() {
    use std::path::PathBuf;

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
    let started = Instant::now();
    let (mut alike, mut reached, mut previous) = (0usize, 0usize, 0usize);
    let mut differ = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let mut sections = vec![None];
        if let Some(update) = read(&data).update {
            sections.push(Some(update.previous));
            previous += 1;
        }
        for section in sections {
            let mut options = Options::DEFAULT;
            options.section = section;
            let read = whole(&data, &[], &options);
            let reading = open(&data, &[], &options);
            let ids: Vec<ObjectId> = reading.ids().collect();
            std::thread::scope(|scope| {
                for t in 0..8 {
                    let (reading, ids) = (&reading, &ids);
                    scope.spawn(move || {
                        for &id in ids.iter().rev().skip(t).step_by(8) {
                            let _ = reading.get(id);
                        }
                    });
                }
            });
            let lazily = reading.whole(None);
            match (
                read.reached || lazily.reached,
                lazily.document == read.document,
            ) {
                (true, _) => reached += 1,
                (false, true) => alike += 1,
                (false, false) => differ.push(format!("{} as of {section:?}", path.display())),
            }
        }
    }
    println!(
        "{} files, {previous} with an update read as of the one before too, in {:.1} s: \
         {alike} readings alike, {reached} whose decoding reached the budget, {} differ",
        files.len(),
        started.elapsed().as_secs_f64(),
        differ.len()
    );
    for path in &differ {
        println!("  differs: {path}");
    }
    assert!(differ.is_empty());
}
