//! What the family adapter writes encrypted, read back with pdf-structure's reader
//! under its password: where the encryption dictionary and what it refers to go, what
//! the hint stream's data is under, what's listed, and that nothing's left in the
//! clear but what's to be. Each kind is written under each of the standard handler's
//! revisions, by the keys of a file qpdf encrypted.

#![cfg(feature = "family")]

use chacha20::ChaCha20Rng;
use pdf_linearize::family::{self, Error, Options, Rc4};
use pdf_objects::object::{Dict, Object, ObjectId, Value};
use pdf_objects::parse::Parser;
use pdf_objects::serialize;
use pdf_structure::pdf_crypt::Password;
use pdf_structure::pdf_crypt::rand_core::SeedableRng;
use pdf_structure::pdf_crypt::{Ivs, Protection};
use pdf_structure::read::{self, Reading, XrefEntry};
use pdf_structure::write::{self, Deflate, Encryptor};
use pdf_structure::{Document, Encryption, XrefKind};

mod written;
use written::{KEYS, ROOT, encrypted_as, file, kinds, opened, stream, with_info};

/// What every string and stream of [`secrets`] holds, and nothing else in a file.
const SECRET: &[u8] = b"secret";

/// A catalog of `more`, a root node holding what's inherited, two pages, each with a
/// stream of its own, a font and resources, and an information dictionary: each's
/// string as long as any other's.
fn secrets(more: &str) -> Vec<String> {
    vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Mark (secret 01) {more} >>"),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 100 200] /Resources 8 0 R >>"
            .to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Mark (secret 03) >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Mark (secret 04) >>".to_string(),
        stream("/Mark (secret 05)", "(a secret first stream) Tj"),
        stream("/Mark (secret 06)", "(a secret other stream) Tj"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Mark (secret 07) >>".to_string(),
        "<< /Font << /F0 7 0 R >> /Mark (secret 08) >>".to_string(),
        "<< /Mark (secret 09) >>".to_string(),
    ]
}

/// `objects` as a file, /Info the ninth.
fn built(objects: &[String]) -> Vec<u8> {
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    file(&objects, "/Root 1 0 R /Info 9 0 R")
}

/// A new encryption for `password`, the same each time: its key from a
/// seeded generator, and each write's IVs derived, by the test writer.
fn created(password: &str, encrypt_metadata: bool) -> Encryptor {
    let prepared = Password(password).prepared().unwrap();
    let mut protection = Protection::new(&prepared);
    protection.encrypt_metadata = encrypt_metadata;
    let mut rng = ChaCha20Rng::seed_from_u64(14);
    let created = Encryptor::create_with(&protection, &mut rng).unwrap();
    created.with_test_ivs(Ivs::derived)
}

/// [`Options::DEFAULT`], [`family::linearize_bytes`] opening a file by
/// `passwords`.
fn opening<'a>(passwords: &'a [Password<'a>]) -> Options<'a> {
    let mut options = Options::DEFAULT;
    options.read.passwords = passwords;
    options
}

/// `bytes`, read by `password`, written under `encryptor` as `options` says.
fn under(
    bytes: &[u8],
    password: &str,
    encryptor: Option<&Encryptor>,
    options: &Options<'_>,
) -> Result<(Vec<u8>, family::Linearized), Error> {
    let reading = opened(bytes, password);
    let doc = reading.to_document().unwrap();
    let mut out = Vec::new();
    let done = family::linearize(&mut out, &doc, encryptor, Some(&reading), options)?;
    Ok((out, done))
}

/// `bytes`, read by `password`, written encrypted as it is, by an encryptor of that
/// reading, so what may be copied is; under RC4, told to reuse the file's
/// keystreams, as this layout puts each object at another's (the module's
/// "Encrypted"), which under AES changes nothing.
fn own(
    bytes: &[u8],
    password: &str,
    options: &Options<'_>,
) -> Result<(Vec<u8>, family::Linearized), Error> {
    own_by(bytes, password, options, Encryptor::reusing_rc4_keystreams)
}

/// [`own`], each write's IVs the test writer's derived ones, so that a file
/// written twice is the same bytes.
fn own_derived(
    bytes: &[u8],
    password: &str,
    options: &Options<'_>,
) -> Result<(Vec<u8>, family::Linearized), Error> {
    own_by(bytes, password, options, |encryptor| {
        encryptor
            .reusing_rc4_keystreams()
            .with_test_ivs(Ivs::derived)
    })
}

/// [`own`], its encryptor as `how` makes it of the reading's.
fn own_by(
    bytes: &[u8],
    password: &str,
    options: &Options<'_>,
    how: impl Fn(Encryptor) -> Encryptor,
) -> Result<(Vec<u8>, family::Linearized), Error> {
    let reading = opened(bytes, password);
    let doc = reading.to_document().unwrap();
    let encryptor = Encryptor::of(&reading).unwrap().map(how);
    let mut out = Vec::new();
    let done = family::linearize(&mut out, &doc, encryptor.as_ref(), Some(&reading), options)?;
    Ok((out, done))
}

/// `file`'s objects read by `password`, each as it's written in the clear,
/// and each stream's data decrypted, but the linearization dictionary, the
/// hint stream (the stream with `/S`) and the cross-reference streams, which
/// say where the objects are and hash the file (`/ID`'s second string):
/// what two files of one document share, whatever IVs their AES
/// ciphertexts are under, which their strings' lengths written follow.
fn in_the_clear(file: &[u8], password: &str) -> Vec<(ObjectId, Vec<u8>, Vec<u8>)> {
    let reading = opened(file, password);
    let doc = reading.to_document().unwrap();
    let placing = |object: &Object| match object.as_stream() {
        Some(stream) => {
            let xref = stream.dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef");
            xref || stream.dict.get(b"S").is_some()
        }
        None => object
            .as_dict()
            .is_some_and(|d| d.get(b"Linearized").is_some()),
    };
    (doc.objects())
        .filter(|(_, object)| !placing(object))
        .map(|(id, object)| {
            let mut written = Vec::new();
            let (value, data) = match object.as_stream() {
                Some(stream) => {
                    let data = reading.data(id).unwrap().to_vec();
                    (Object::from(stream.dict.clone()), data)
                }
                None => (object.clone(), Vec::new()),
            };
            serialize::write(&mut written, &value).unwrap();
            (id, written, data)
        })
        .collect()
}

/// `data` as the hint stream's deflated.
fn deflated(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let raw = pdf_structure::pdf_filters::Prediction::None;
    Deflate::DEFAULT.encode(data, raw, &mut out).unwrap();
    out
}

/// Whether `bytes` holds `text` anywhere.
fn holds(bytes: &[u8], text: &[u8]) -> bool {
    bytes.windows(text.len()).any(|w| w == text)
}

/// The object in `doc` that holds `/Mark (text)`.
fn marked(doc: &Document, text: &str) -> ObjectId {
    let id = doc.objects().find(|(_, object)| {
        let dict = match object.value() {
            Value::Dict(dict) => dict,
            Value::Stream(stream) => &stream.dict,
            _ => return false,
        };
        let mark = dict.get(b"Mark");
        mark.is_some_and(|mark| match mark.value() {
            Value::String(string, _) => string.as_bytes() == Some(text.as_bytes()),
            _ => false,
        })
    });
    id.unwrap_or_else(|| panic!("nothing's marked {text}")).0
}

/// How many of `doc`'s objects are an encryption dictionary, by their shape.
fn dictionaries(doc: &Document) -> usize {
    let shaped = |object: &Object| match object.value() {
        Value::Dict(dict) => dict.has(b"O") && dict.has(b"U"),
        _ => false,
    };
    doc.objects().filter(|(_, object)| shaped(object)).count()
}

/// That `out` is [`secrets`] linearized, by `password`, nothing in the clear: every
/// string and stream reads back, by the number it's written at, and every table of
/// its hint stream.
fn reads_back(out: &[u8], done: &family::Linearized, password: &str, name: &str) {
    assert!(!holds(out, SECRET), "{name}: something's in the clear");
    let reading = opened(out, password);
    assert!(reading.facts().linearized, "{name}");
    assert!(
        matches!(reading.encryption(), Encryption::Opened { .. }),
        "{name}"
    );
    let doc = reading.to_document().unwrap();
    assert!(doc.faults().is_empty(), "{name}: {:?}", doc.faults());
    for text in [1, 3, 4, 5, 6, 7, 8, 9].map(|n| format!("secret 0{n}")) {
        marked(&doc, &text);
    }
    let content = |text: &str| reading.data(marked(&doc, text)).unwrap().to_vec();
    assert_eq!(
        content("secret 05"),
        b"(a secret first stream) Tj",
        "{name}"
    );
    assert_eq!(
        content("secret 06"),
        b"(a secret other stream) Tj",
        "{name}"
    );
    let trailer = doc.trailer().dict();
    assert_eq!(
        trailer.get(b"Info").unwrap().as_reference(),
        Some(marked(&doc, "secret 09")),
        "{name}"
    );
    // The hint stream, by what's at /H, under its number.
    let (at, _) = done.layout.hint_extent();
    let hint = Parser::at(out, at as usize).header().unwrap();
    assert_eq!(
        &*reading.data(hint).unwrap(),
        deflated(done.layout.hint_data()),
        "{name}"
    );
}

#[test]
fn each_kind_reads_back_under_each_encryption() {
    let plain = built(&secrets(""));
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        for (kind, options) in kinds().iter().enumerate() {
            let name = format!("{name}, kind {kind}");
            let (out, done) = own(&bytes, "", options).unwrap();
            reads_back(&out, &done, "", &name);
            // By the bytes alone it's the same file: the same document read
            // back, its AES ciphertexts under IVs of their own.
            let mut options = *options;
            options.rc4 = Rc4::ReusingKeystreams;
            let alone = family::linearize_bytes(&bytes, &options).unwrap();
            assert_eq!(in_the_clear(&alone, ""), in_the_clear(&out, ""), "{name}");
        }
    }
}

#[test]
fn the_hint_streams_data_is_encrypted_and_a_section_isnt() {
    let plain = built(&secrets(""));
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        for options in kinds() {
            let (out, done) = own(&bytes, "", &options).unwrap();
            let (at, length) = done.layout.hint_extent();
            let hint = &out[at as usize..(at + length) as usize];
            let clear = deflated(done.layout.hint_data());
            assert!(!holds(hint, &clear), "{name}: the hint's in the clear");
            // What lists the file is read before there's a key, so its rows are as
            // any file's: what reads them unwrapped by no filter but Flate's.
            if options.kind == XrefKind::Stream {
                let reading = opened(&out, "");
                for at in [done.layout.first_section(), done.layout.main_section()] {
                    let number = Parser::at(&out, at as usize).header().unwrap();
                    let listed = reading.get(number).unwrap();
                    let Value::Stream(listed) = listed.value() else {
                        panic!("{name}: no stream at {at}");
                    };
                    assert!(listed.dict.has_type(b"XRef"), "{name}");
                    let data = reading.data(number).unwrap();
                    assert!(holds(&out[at as usize..], &data), "{name}: it's encrypted");
                }
            }
        }
    }
}

#[test]
fn the_encryption_dictionary_is_in_part_four_written_from_the_encryptor() {
    let plain = built(&secrets(""));
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        let reading = opened(&bytes, "");
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        for options in kinds() {
            let (out, done) = own(&bytes, "", &options).unwrap();
            let back = opened(&out, "");
            let number = (back.trailer().dict().get(b"Encrypt").unwrap())
                .as_reference()
                .unwrap();
            assert_ne!(number, ObjectId::new(1000, 0), "{name}: it's where it was");
            // Between the first section and the hint stream, which part 6 follows.
            let at = done.layout.position(number.number).unwrap();
            assert!(done.layout.first_section() < at, "{name}");
            assert!(at < done.layout.hint_extent().0, "{name}");
            assert!(matches!(
                back.entries().find(|(n, ..)| *n == number.number),
                Some((_, _, XrefEntry::InUse { .. }))
            ));
            // As it's stored, each string as written, not encrypted.
            let held = back.get(number).unwrap().as_dict().unwrap();
            for key in [&b"O"[..], b"U", b"Filter", b"R"] {
                let value = |dict: &Dict| dict.get(key).map(|value| value.value().clone());
                assert_eq!(
                    value(held),
                    value(encryptor.dictionary()),
                    "{name}: /{}",
                    String::from_utf8_lossy(key)
                );
            }
            // Written once: what it was at in the file read isn't carried over.
            let o = encryptor.dictionary().get(b"O").unwrap();
            let mut written = Vec::new();
            serialize::write(&mut written, o).unwrap();
            let found = (out.windows(written.len()))
                .filter(|w| *w == &written[..])
                .count();
            assert_eq!(found, 1, "{name}");
        }
    }
}

#[test]
fn nothing_is_listed_free_and_each_number_is_reused() {
    let plain = built(&secrets(""));
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        let reading = opened(&bytes, "");
        let per_object = !name.starts_with("AES-256");
        // The numbers the file read lists free, whose keys an object there
        // takes under RC4 and AES-128.
        let free: Vec<u32> = (reading.entries())
            .filter(|(n, _, entry)| *n != 0 && matches!(entry, XrefEntry::Free { .. }))
            .map(|(n, ..)| n)
            .collect();
        assert!(!free.is_empty(), "{name}");
        for options in kinds() {
            let (out, _) = own(&bytes, "", &options).unwrap();
            let back = opened(&out, "");
            let rows: Vec<(u32, u16, XrefEntry)> = back.entries().collect();
            for (n, _, entry) in &rows {
                assert_eq!(
                    *n == 0,
                    matches!(entry, XrefEntry::Free { .. }),
                    "{name}: {n}"
                );
            }
            let last = rows.iter().map(|(n, ..)| *n).max().unwrap();
            assert_eq!(rows.len() as u32, last + 1, "{name}: numbers with gaps");
            // One the file read listed free holds an object here, read back by it.
            if per_object {
                assert!(free.iter().any(|n| *n <= last), "{name}");
            }
        }
    }
}

/// The string `/Mark` or `/Title` of the object at number `n` of `file`, as it's
/// stored: none where it's in an object stream, or has no such string.
fn stored_mark(file: &[u8], n: u32) -> Option<Vec<u8>> {
    let reading = opened(file, "");
    let (_, _, entry) = reading.entries().find(|(at, ..)| *at == n)?;
    let XrefEntry::InUse { offset, .. } = entry else {
        return None;
    };
    let mut parser = Parser::at(file, offset).ciphertext(false);
    parser.header().ok()?;
    let value = parser.value(&mut ()).ok()?;
    let mark = value.as_dict()?.get(b"Mark")?;
    match mark.value() {
        Value::String(text, _) => text.as_bytes().map(<[u8]>::to_vec),
        _ => None,
    }
}

/// Under RC4, told to reuse the file's keystreams, an object laid out at a
/// number and generation another object of the file read was at is encrypted
/// with that one's keystream: the XOR of the two ciphertexts is the XOR of the
/// two marks, which is what a caller who tells it so accepts. The file read is a whole file of
/// pdf-structure's under the key's encryption, which writes an object at a
/// number the key's file used at its next generation, under another keystream
/// than the one laid out there at 0: those don't leak so. Under AES nothing
/// does.
#[test]
fn under_rc4_each_number_s_keystream_encrypts_another_object_and_under_aes_it_doesn_t() {
    let plain = built(&secrets(""));
    let xor = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(a, b)| a ^ b).collect::<Vec<u8>>();
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        let before = opened(&bytes, "").to_document().unwrap();
        let (out, _) = own(&bytes, "", &Options::DEFAULT).unwrap();
        let after = opened(&out, "").to_document().unwrap();
        // The mark of number `n`, at the generation the document holds it at.
        let mark_at = |doc: &Document, n: u32| {
            let (id, object) = doc.objects().find(|(id, _)| id.number == n)?;
            let dict = match object.value() {
                Value::Dict(dict) => dict,
                Value::Stream(stream) => &stream.dict,
                _ => return None,
            };
            let mark = dict.get(b"Mark")?;
            match mark.value() {
                Value::String(text, _) => Some((id.generation, text.as_bytes()?.to_vec())),
                _ => None,
            }
        };
        let (mut moved, mut leaked, mut apart) = (0, 0, 0);
        for n in 1..40 {
            let (Some((then, was)), Some((now, is))) = (mark_at(&before, n), mark_at(&after, n))
            else {
                continue;
            };
            if was == is {
                continue;
            }
            let (Some(stored_was), Some(stored_is)) =
                (stored_mark(&bytes, n), stored_mark(&out, n))
            else {
                continue;
            };
            let xored = xor(&stored_was, &stored_is) == xor(&was, &is);
            match then == now {
                true => {
                    moved += 1;
                    leaked += usize::from(xored);
                }
                false => {
                    apart += 1;
                    assert!(!xored, "{name}: {n}, at another generation, leaks");
                }
            }
        }
        assert!(
            moved > 0 && moved + apart > 2,
            "{name}: {moved} and {apart} moved"
        );
        let rc4 = name.starts_with("RC4");
        assert_eq!(leaked, if rc4 { moved } else { 0 }, "{name}");
    }
}

/// Whether `object` holds a string that isn't empty, at any depth.
fn holds_a_string(object: &Object) -> bool {
    let mut stack = vec![object];
    while let Some(object) = stack.pop() {
        match object.value() {
            Value::String(text, _) => {
                if text.as_bytes().is_some_and(|text| !text.is_empty()) {
                    return true;
                }
            }
            Value::Array(items) => stack.extend(items),
            Value::Dict(dict) => stack.extend(dict.entries().iter().map(|e| &e.value)),
            Value::Stream(stream) => stack.extend(stream.dict.entries().iter().map(|e| &e.value)),
            _ => {}
        }
    }
    false
}

/// Each object of `out`, read by `password`, whose strings or stream's data RC4
/// would encrypt at a keystream `file`, read by the empty password, used, as its
/// rows say: an object's in use there at that generation or a later one, or
/// below a free entry's generation. What's in the clear isn't among them, the
/// encryption dictionary and the cross-reference streams, nor what holds no
/// string and no data, which encrypt to nothing. By number, ascending.
fn at_used_keystreams(file: &[u8], out: &[u8], password: &str) -> Vec<ObjectId> {
    let read = opened(file, "");
    let rows: Vec<(u32, u16, XrefEntry)> = read.entries().collect();
    let used = |id: ObjectId| {
        rows.iter().any(|&(n, generation, entry)| {
            n == id.number
                && match entry {
                    XrefEntry::InUse { .. } => id.generation <= generation,
                    XrefEntry::Free { .. } => id.generation < generation,
                    _ => false,
                }
        })
    };
    let back = opened(out, password);
    let dictionary = (back.trailer().dict().get(b"Encrypt")).and_then(Object::as_reference);
    let mut ids = Vec::new();
    for (n, generation, entry) in back.entries() {
        let id = ObjectId::new(n, generation);
        if !matches!(entry, XrefEntry::InUse { .. }) || Some(id) == dictionary || !used(id) {
            continue;
        }
        let object = back.get(id).unwrap();
        let encrypted = match object.value() {
            Value::Stream(stream) => {
                let length = stream.dict.get(b"Length").and_then(Object::as_integer);
                !stream.dict.has_type(b"XRef") && length.is_some_and(|length| length > 0)
            }
            _ => holds_a_string(object),
        };
        if encrypted {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    ids
}

/// Under RC4 the layout puts strings and streams at keystreams the file read
/// used, which is refused, `out` as it was given, unless the encryptor is told
/// to reuse them; told so, what's written lists exactly each object it put at
/// one, the object streams among them, and is otherwise the file it writes;
/// and [`family::linearize_bytes`] is refused alike unless its options say so.
/// Under AES nothing is refused, and nothing listed.
#[test]
fn under_rc4_a_keystream_the_file_used_is_refused_unless_the_caller_reuses_it() {
    let plain = built(&secrets(""));
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        let rc4 = name.starts_with("RC4");
        for (kind, options) in kinds().iter().enumerate() {
            let name = format!("{name}, kind {kind}");
            let reading = opened(&bytes, "");
            let doc = reading.to_document().unwrap();
            let encryptor = Encryptor::of(&reading).unwrap().unwrap();
            let mut out = vec![7];
            let as_given =
                family::linearize(&mut out, &doc, Some(&encryptor), Some(&reading), options);
            let (told, done) = own(&bytes, "", options).unwrap();
            let bytes_alone = family::linearize_bytes(&bytes, options);
            let mut reusing = *options;
            reusing.rc4 = Rc4::ReusingKeystreams;
            let bytes_reusing = family::linearize_bytes(&bytes, &reusing).unwrap();
            if !rc4 {
                let as_given = as_given.unwrap();
                assert!(as_given.keystreams_reused.is_empty(), "{name}");
                assert!(done.keystreams_reused.is_empty(), "{name}");
                assert_eq!(
                    in_the_clear(&out[1..], ""),
                    in_the_clear(&told, ""),
                    "{name}"
                );
                let alone = bytes_alone.unwrap();
                assert_eq!(in_the_clear(&alone, ""), in_the_clear(&told, ""), "{name}");
                continue;
            }
            let refused = as_given.unwrap_err();
            let keystream = |refused: &Error| match refused {
                Error::Write(write::Error::Encrypt { id, why }) => {
                    assert_eq!(*why, write::Unencryptable::Keystream, "{name}");
                    *id
                }
                refused => panic!("{name}: {refused}"),
            };
            let at = keystream(&refused);
            assert_eq!(out, [7], "{name}");
            assert_eq!(keystream(&bytes_alone.unwrap_err()), at, "{name}");
            // Each object the layout put at a keystream used, by the test's own
            // count of what's written and what the file's rows say.
            let counted = at_used_keystreams(&bytes, &told, "");
            assert_eq!(done.keystreams_reused, counted, "{name}");
            assert!(counted.contains(&at), "{name}: {at:?}");
            if options.pack {
                let back = opened(&told, "");
                let container = |id: &ObjectId| {
                    let object = back.get(*id).unwrap();
                    object
                        .as_stream()
                        .is_some_and(|s| s.dict.has_type(b"ObjStm"))
                };
                assert!(counted.iter().any(container), "{name}: no object stream");
            }
            // RC4's keystreams are the same each write, so by the bytes alone
            // it's the same file.
            assert_eq!(bytes_reusing, told, "{name}");
        }
    }
}

/// A file whose strings and streams the layout puts at keystreams the file
/// never used is written under RC4 whatever the caller says; and one whose only
/// string or stream at a keystream used is the hint stream's data is refused at
/// it, by the same error as a body's, or written listing it alone. The file
/// read is a whole file of pdf-structure's under the key's encryption, which
/// moved what it wrote past the numbers the key's file used, 1 to 8.
#[test]
fn under_rc4_only_what_goes_at_a_keystream_used_is_refused() {
    let rc4 = KEYS[1].1;
    // The pages, and objects enough that the hint stream, numbered last, is
    // past 8; no string or stream in any, so nothing else is encrypted.
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /More [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 1 1] >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ];
    objects.extend(["<< /More 1 >>"; 6]);
    let bytes = encrypted_as(rc4, &file(&objects, ROOT));
    let used = |number: u32| {
        let rows = opened(&bytes, "").entries().collect::<Vec<_>>();
        rows.iter().any(|&(n, generation, entry)| {
            n == number && (generation > 0 || matches!(entry, XrefEntry::InUse { .. }))
        })
    };
    assert!((1..=8).all(used) && !used(10), "the key's file's numbers");
    // Packed, the six would go in an object stream, numbered among them.
    for options in &kinds()[..2] {
        let (out, done) = own_by(&bytes, "", options, |encryptor| encryptor).unwrap();
        assert!(done.keystreams_reused.is_empty());
        assert_eq!(at_used_keystreams(&bytes, &out, ""), []);
        assert_eq!(family::linearize_bytes(&bytes, options).unwrap(), out);
    }
    // The pages alone, so the hint stream is at a number the file used.
    let bytes = encrypted_as(rc4, &file(&objects[..3], ROOT));
    for options in kinds() {
        let refused = own_by(&bytes, "", &options, |encryptor| encryptor).unwrap_err();
        let Error::Write(write::Error::Encrypt { id, why }) = refused else {
            panic!("{refused}");
        };
        assert_eq!(why, write::Unencryptable::Keystream);
        let (out, done) = own(&bytes, "", &options).unwrap();
        let (at, _) = done.layout.hint_extent();
        let hint = Parser::at(&out, at as usize).header().unwrap();
        assert_eq!(id, hint);
        assert_eq!(done.keystreams_reused, [hint]);
        assert_eq!(at_used_keystreams(&bytes, &out, ""), [hint]);
    }
}

#[test]
fn the_first_string_of_the_id_is_kept() {
    let plain = built(&secrets(""));
    let id_of = |bytes: &[u8]| {
        let reading = opened(bytes, "");
        let id = reading.trailer().dict().get(b"ID").unwrap().clone();
        let strings = id.as_array().unwrap().to_vec();
        strings
            .iter()
            .map(|s| s.as_bytes().unwrap().to_vec())
            .collect::<Vec<_>>()
    };
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        let first = opened(key, "")
            .trailer()
            .dict()
            .get(b"ID")
            .and_then(|id| id.as_array()?.first()?.as_bytes().map(<[u8]>::to_vec))
            .unwrap();
        for options in kinds() {
            let (out, done) = own(&bytes, "", &options).unwrap();
            let id = id_of(&out);
            assert_eq!(id[0], first, "{name}");
            assert_ne!(id[1], id[0], "{name}");
            assert_eq!(done.id.as_ref().unwrap().to_vec(), id, "{name}");
        }
    }

    // A new encryption keeps a file's, and a file with none has the hash for both.
    let encryptor = created("user", true);
    let (out, done) = under(&plain, "", Some(&encryptor), &Options::DEFAULT).unwrap();
    let [first, second] = done.id.unwrap();
    assert_eq!(first, second);
    assert_eq!(first.len(), 16);
    let (again, done) = under(
        &out,
        "user",
        Some(&created("other", true)),
        &Options::DEFAULT,
    )
    .unwrap();
    assert_eq!(done.id.as_ref().unwrap()[0], first);
    assert!(opened(&again, "other").facts().linearized);
}

#[test]
fn under_aes_256_an_object_as_read_is_copied_at_its_new_number() {
    let plain = built(&secrets(""));
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        let reading = opened(&bytes, "");
        // The font refers to nothing: what follows its header is as it was, ciphertext
        // and all, or written anew under another key.
        let before = reading.to_document().unwrap();
        let font = marked(&before, "secret 07");
        let XrefEntry::InUse { offset, .. } = (reading.entries())
            .find(|(n, ..)| *n == font.number)
            .unwrap()
            .2
        else {
            panic!("{name}");
        };
        let text = &bytes[offset..];
        let to = text.windows(6).position(|w| w == b"endobj").unwrap();
        let header = text.windows(3).position(|w| w == b"obj").unwrap() + 3;
        let body = &text[header..to];
        let content = marked(&before, "secret 05");
        let XrefEntry::InUse { offset, .. } = (reading.entries())
            .find(|(n, ..)| *n == content.number)
            .unwrap()
            .2
        else {
            panic!("{name}");
        };
        let text = &bytes[offset..];
        let to = text.windows(9).position(|w| w == b"endstream").unwrap();
        let from = text.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
        let data = &text[from..to];
        let (out, _) = own(&bytes, "", &Options::DEFAULT).unwrap();
        let aes = name.starts_with("AES-256");
        assert_eq!(holds(&out, body), aes, "{name}: its strings");
        assert_eq!(holds(&out, data), aes, "{name}: a stream's data");
        // Nothing's copied across a new key.
        let new = created("user", true);
        let (out, _) = under(&bytes, "", Some(&new), &Options::DEFAULT).unwrap();
        assert!(!holds(&out, body), "{name}");
        assert!(!holds(&out, data), "{name}");
    }
}

const METADATA: &str = "<x:xmpmeta>the metadata</x:xmpmeta>";

/// [`secrets`] with a document metadata stream, 10.
fn with_metadata() -> Vec<u8> {
    let mut objects = secrets("/Metadata 10 0 R");
    objects.push(stream("/Type /Metadata /Subtype /XML", METADATA));
    built(&objects)
}

#[test]
fn a_metadata_stream_under_encrypt_metadata_false_is_in_the_clear_at_its_new_number() {
    let bytes = with_metadata();
    let plain = created("user", false);
    let encrypted = created("user", true);
    for options in kinds() {
        let (out, done) = under(&bytes, "", Some(&plain), &options).unwrap();
        reads_back(&out, &done, "user", "/EncryptMetadata false");
        assert!(holds(&out, METADATA.as_bytes()));
        let back = opened(&out, "user");
        let doc = back.to_document().unwrap();
        let root = doc.trailer().dict().get(b"Root").unwrap().as_reference();
        let catalog = doc.get_dict(root.unwrap()).unwrap();
        let metadata = catalog.get(b"Metadata").unwrap().as_reference().unwrap();
        assert_ne!(metadata, ObjectId::new(10, 0), "it's moved");
        assert_eq!(&*back.data(metadata).unwrap(), METADATA.as_bytes());
        // At the number the file names, not the one it was at.
        let at = done.layout.position(metadata.number).unwrap() as usize;
        let to = (at + 80 + METADATA.len()).min(out.len());
        assert!(holds(&out[at..to], METADATA.as_bytes()));

        // Written as read, from that file: AES-256, its objects copied.
        let (again, done) = own(&out, "user", &options).unwrap();
        reads_back(&again, &done, "user", "/EncryptMetadata false, again");
        assert!(holds(&again, METADATA.as_bytes()));

        let (out, done) = under(&bytes, "", Some(&encrypted), &options).unwrap();
        reads_back(&out, &done, "user", "/EncryptMetadata true");
        assert!(!holds(&out, METADATA.as_bytes()));
    }
}

#[test]
fn a_new_encryption_is_given_a_number_and_part_four() {
    let plain = built(&secrets(""));
    let encryptor = created("user", true);
    let sources = [
        ("plain", plain.clone(), ""),
        ("RC4", encrypted_as(KEYS[1].1, &plain), ""),
        ("AES-256", encrypted_as(KEYS[4].1, &plain), ""),
    ];
    for (name, source, password) in &sources {
        for options in kinds() {
            let (out, done) = under(source, password, Some(&encryptor), &options).unwrap();
            reads_back(&out, &done, "user", name);
            assert!(
                Reading::open(&out[..], &read::Options::DEFAULT)
                    .unwrap()
                    .to_document()
                    .is_ok_and(|doc| *doc.encryption() == Encryption::Locked),
                "{name}: the empty password opens it"
            );
            let back = opened(&out, "user");
            let number = (back.trailer().dict().get(b"Encrypt").unwrap())
                .as_reference()
                .unwrap();
            let at = done.layout.position(number.number).unwrap();
            assert!(done.layout.first_section() < at, "{name}");
            assert!(at < done.layout.hint_extent().0, "{name}");
            // What the file read had as its dictionary isn't carried.
            let held = back.get(number).unwrap().as_dict().unwrap();
            assert_eq!(
                held.get(b"O").unwrap().as_bytes(),
                encryptor.dictionary().get(b"O").unwrap().as_bytes(),
                "{name}"
            );
            assert_eq!(dictionaries(&back.to_document().unwrap()), 1, "{name}");
        }
    }
}

#[test]
fn a_reference_to_no_object_doesnt_come_to_name_a_new_dictionary() {
    // 10 and 11 are past every object, and nothing's there: what names them, the
    // catalog and the trailer, reads null. A direct /Info, made an object, is
    // given a number past them too, and the new dictionary's, so neither names
    // it, nor the dictionary.
    let objects = secrets("/Other 10 0 R");
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    for info in [
        "/Info 9 0 R",
        "/Info << /Title (secret title) /Mark 9 0 R >>",
    ] {
        let bytes = file(&objects, &format!("/Root 1 0 R {info} /Dangling 11 0 R"));
        let encryptor = created("user", true);
        for options in kinds() {
            let (out, _) = under(&bytes, "", Some(&encryptor), &options).unwrap();
            let reading = opened(&out, "user");
            let doc = reading.to_document().unwrap();
            let trailer = doc.trailer().dict();
            let encrypt = trailer.get(b"Encrypt").unwrap().as_reference().unwrap();
            assert!(trailer.get(b"Dangling").unwrap().is_null());
            let root = trailer.get(b"Root").unwrap().as_reference().unwrap();
            let other = doc.get_dict(root).unwrap().get(b"Other").unwrap();
            assert!(other.is_null(), "{other:?} {encrypt:?}");
            let info = trailer.get(b"Info").unwrap().as_reference().unwrap();
            assert_ne!(info, encrypt);
            assert!(doc.get_dict(info).unwrap().has(b"Mark"), "{info}");
        }
    }
}

#[test]
fn with_no_encryptor_an_encrypted_reading_is_written_in_the_clear() {
    let plain = built(&secrets(""));
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        for options in kinds() {
            let (out, _) = under(&bytes, "", None, &options).unwrap();
            let back = opened(&out, "");
            assert_eq!(*back.encryption(), Encryption::None, "{name}");
            assert!(back.facts().linearized, "{name}");
            let doc = back.to_document().unwrap();
            assert!(!doc.trailer().dict().has(b"Encrypt"), "{name}");
            assert_eq!(dictionaries(&doc), 0, "{name}");
            marked(&doc, "secret 07");
            assert_eq!(
                &*back.data(marked(&doc, "secret 05")).unwrap(),
                b"(a secret first stream) Tj",
                "{name}"
            );
            if !options.pack {
                assert!(holds(&out, b"(secret 07)"), "{name}");
            }
        }
    }
}

#[test]
fn what_no_password_opened_is_refused() {
    let plain = built(&secrets(""));
    let encryptor = created("user", true);
    let bytes = {
        let (out, _) = under(&plain, "", Some(&encryptor), &Options::DEFAULT).unwrap();
        out
    };
    let reading = Reading::open(&bytes[..], &read::Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    assert_eq!(*doc.encryption(), Encryption::Locked);
    for given in [None, Some(&encryptor)] {
        let mut out = vec![7];
        let refused = family::linearize(&mut out, &doc, given, Some(&reading), &Options::DEFAULT)
            .unwrap_err();
        assert!(matches!(refused, Error::Locked), "{refused}");
        assert_eq!(out, [7]);
    }
    let refused = family::linearize_bytes(&bytes, &Options::DEFAULT).unwrap_err();
    assert!(matches!(refused, Error::Locked));
    let wrong = family::linearize_bytes(&bytes, &opening(&[Password("wrong")]));
    assert!(matches!(wrong.unwrap_err(), Error::Locked));
    let done = family::linearize_bytes(&bytes, &opening(&[Password("user")])).unwrap();
    assert!(opened(&done, "user").facts().linearized);

    // Its strings and streams in the clear, as an attachment's may not be: what no
    // password opened is no surer for that.
    let bytes = hand_made(
        KEYS[2].1,
        false,
        |dict| {
            in_hex(dict);
            dict.set("StmF", "Identity");
            dict.set("StrF", "Identity");
        },
        &[],
    );
    let reading = opened(&bytes, "wrong");
    assert_eq!(*reading.encryption(), Encryption::Plain);
    let doc = reading.to_document().unwrap();
    let mut out = vec![7];
    let refused =
        family::linearize(&mut out, &doc, None, Some(&reading), &Options::DEFAULT).unwrap_err();
    assert!(matches!(refused, Error::Locked), "{refused}");
    assert_eq!(out, [7]);
    let wrong = family::linearize_bytes(&bytes, &opening(&[Password("wrong")]));
    assert!(matches!(wrong.unwrap_err(), Error::Locked));
}

#[test]
fn a_handler_that_cant_be_opened_is_refused() {
    let bytes = hand_made(
        KEYS[1].1,
        false,
        |dict| {
            in_hex(dict);
            dict.set("Filter", "Other");
        },
        &[],
    );
    let reading = opened(&bytes, "");
    assert!(matches!(reading.encryption(), Encryption::Unsupported(_)));
    let doc = reading.to_document().unwrap();
    let mut out = vec![7];
    let refused =
        family::linearize(&mut out, &doc, None, Some(&reading), &Options::DEFAULT).unwrap_err();
    assert!(
        matches!(refused, Error::Write(write::Error::Unsupported(_))),
        "{refused}"
    );
    assert_eq!(out, [7]);
    let refused = family::linearize_bytes(&bytes, &Options::DEFAULT).unwrap_err();
    assert!(
        matches!(refused, Error::Write(write::Error::Unsupported(_))),
        "{refused}"
    );
}

/// A file of a page with no string or stream, encrypted as `key` is: its dictionary
/// as `edit` leaves it, which leaves its strings in hex ([`in_hex`]), as they go
/// through [`file`] as text, in the trailer or object 4, and `extra` after: nothing
/// in it wants a key but what opens it.
fn hand_made(key: &[u8], direct: bool, edit: impl FnOnce(&mut Dict), extra: &[&str]) -> Vec<u8> {
    hand_made_of("", key, direct, edit, extra)
}

/// [`hand_made`], its catalog of `more`.
fn hand_made_of(
    more: &str,
    key: &[u8],
    direct: bool,
    edit: impl FnOnce(&mut Dict),
    extra: &[&str],
) -> Vec<u8> {
    let key = opened(key, "");
    let encryptor = Encryptor::of(&key).unwrap().unwrap();
    let mut dictionary = encryptor.dictionary().clone();
    edit(&mut dictionary);
    let mut written = Vec::new();
    serialize::write(&mut written, &Object::from(dictionary)).unwrap();
    let written = String::from_utf8_lossy(&written).into_owned();
    let hex: String = (encryptor.first_id().unwrap().iter())
        .map(|byte| format!("{byte:02X}"))
        .collect();
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {more} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>".to_string(),
    ];
    let mut trailer = format!("/Root 1 0 R /ID [<{hex}> <{hex}>]");
    match direct {
        true => trailer += &format!(" /Encrypt {written}"),
        false => {
            trailer += " /Encrypt 4 0 R";
            objects.push(written.clone());
        }
    }
    objects.extend(extra.iter().map(|extra| extra.to_string()));
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    file(&objects, &trailer)
}

/// A direct `/Info` whose strings are secrets, and an object it refers to.
const INFO: &str = "/Info << /Title (secret title) /Author (secret author) /Mark 5 0 R >>";

/// That `out`, read by `password`, has [`INFO`] as an object in part 9, the
/// trailer naming it, its strings in the clear nowhere, the object it refers
/// to reached: its number.
fn info_moved(out: &[u8], done: &family::Linearized, password: &str, name: &str) -> ObjectId {
    for secret in [&b"secret title"[..], b"secret author"] {
        assert!(!holds(out, secret), "{name}: in the clear");
    }
    let back = opened(out, password);
    let trailer = back.trailer().dict();
    let at = (trailer.get(b"Info").and_then(Object::as_reference))
        .unwrap_or_else(|| panic!("{name}: /Info isn't an object"));
    let doc = back.to_document().unwrap();
    assert!(doc.faults().is_empty(), "{name}: {:?}", doc.faults());
    let info = doc.get_dict(at).unwrap();
    let text = |key: &[u8]| info.get(key).and_then(Object::as_bytes).map(<[u8]>::to_vec);
    assert_eq!(
        text(b"Title").as_deref(),
        Some(&b"secret title"[..]),
        "{name}"
    );
    assert_eq!(
        text(b"Author").as_deref(),
        Some(&b"secret author"[..]),
        "{name}"
    );
    let mark = info.get(b"Mark").and_then(Object::as_reference).unwrap();
    assert!(doc.get_dict(mark).unwrap().has(b"Kept"), "{name}");
    // In part 9: past the first page's section, in an object stream there or
    // on its own.
    let written = |n: u32| match back.entries().find(|(at, ..)| *at == n) {
        Some((_, _, XrefEntry::InUse { offset, .. })) => offset as u64,
        Some((_, _, XrefEntry::Compressed { stream, .. })) => done.layout.position(stream).unwrap(),
        entry => panic!("{name}: {n} is {entry:?}"),
    };
    assert!(written(at.number) > done.layout.first_page_end(), "{name}");
    assert!(written(at.number) < done.layout.main_section(), "{name}");
    at
}

/// An encrypted file's direct `/Info`, whose strings a trailer holds in the
/// clear, is written as an object in part 9, encrypted as any, the trailer
/// naming it: under the file's own encryption, each key and kind, and under a
/// new one, whose dictionary is another object. Read back, it's the dictionary
/// it was, and its strings are in the clear nowhere. In the clear it stays in
/// the trailer, as it was.
#[test]
fn a_direct_info_is_written_as_an_object_under_an_encryption() {
    let kept = ["<< /Kept true >>"];
    for (name, key) in KEYS {
        let bytes = with_info(&hand_made(key, false, in_hex, &kept), INFO);
        assert!(holds(&bytes, b"secret title"), "{name}");
        let back = opened(&bytes, "");
        assert!(
            back.trailer()
                .dict()
                .get(b"Info")
                .unwrap()
                .as_dict()
                .is_some()
        );
        for (kind, options) in kinds().iter().enumerate() {
            let name = format!("{name}, kind {kind}");
            let (out, done) = own(&bytes, "", options).unwrap();
            info_moved(&out, &done, "", &name);
            // By its bytes alone too, under RC4 told to reuse the keystreams.
            let mut options = *options;
            options.rc4 = Rc4::ReusingKeystreams;
            let alone = family::linearize_bytes(&bytes, &options).unwrap();
            assert!(!holds(&alone, b"secret title"), "{name}");
        }
    }
    // In the clear, under a new encryption, and with none.
    let plain = with_info(&file(&two_pages_of_kept(), ROOT), INFO);
    let encryptor = created("user", true);
    for (kind, options) in kinds().iter().enumerate() {
        let name = format!("new, kind {kind}");
        let (out, done) = under(&plain, "", Some(&encryptor), options).unwrap();
        let at = info_moved(&out, &done, "user", &name);
        let back = opened(&out, "user");
        let encrypt = back.trailer().dict().get(b"Encrypt").unwrap();
        assert_ne!(
            encrypt.as_reference(),
            Some(at),
            "{name}: one object for both"
        );
        let (out, _) = under(&plain, "", None, options).unwrap();
        let back = opened(&out, "");
        let info = back.trailer().dict().get(b"Info").unwrap();
        let title = info.as_dict().unwrap().get(b"Title").unwrap();
        assert_eq!(title.as_bytes(), Some(&b"secret title"[..]), "{kind}");
    }
}

/// A direct `/Info` made an object may be given the number an earlier section
/// named its encryption dictionary at, past every number an object has and a
/// reference names, where no object is: it's encrypted as any, as the file
/// written names one dictionary alone, which is all that's left in the clear.
#[test]
fn a_direct_info_at_an_earlier_dictionary_s_number_is_encrypted() {
    for (name, key) in KEYS {
        let first = hand_made(key, false, in_hex, &["<< /Kept true >>"]);
        // The first section's /Encrypt at 6, past every object, and an update
        // after whose trailer names 4, as the file's encryption is.
        let mut bytes = first.clone();
        let at = bytes
            .windows(14)
            .rposition(|w| w == b"/Encrypt 4 0 R")
            .unwrap();
        bytes[at..at + 14].copy_from_slice(b"/Encrypt 6 0 R");
        let id = {
            let at = first.windows(4).rposition(|w| w == b"/ID ").unwrap();
            let end = at + first[at..].iter().position(|&b| b == b']').unwrap() + 1;
            String::from_utf8_lossy(&first[at..end]).into_owned()
        };
        let prev = {
            let at = first.windows(9).rposition(|w| w == b"startxref").unwrap() + 10;
            let digits = first[at..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            String::from_utf8_lossy(&first[at..at + digits]).into_owned()
        };
        let section = bytes.len();
        bytes.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 6 /Prev {prev} /Root 1 0 R \
                 {id} /Encrypt 4 0 R {INFO} >>\nstartxref\n{section}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let reading = opened(&bytes, "");
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        assert!(encryptor.dictionary_object(ObjectId::new(6, 0)), "{name}");
        for (kind, options) in kinds().iter().enumerate() {
            let name = format!("{name}, kind {kind}");
            let (out, done) = own(&bytes, "", options).unwrap();
            info_moved(&out, &done, "", &name);
        }
    }
}

/// [`two_pages`]'s catalog and pages, and at 5 what [`INFO`] refers to.
fn two_pages_of_kept() -> Vec<&'static str> {
    vec![
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 1 1] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Kept true >>",
    ]
}

/// `dict` with its strings in hex, so they go through [`file`] as text.
fn in_hex(dict: &mut Dict) {
    let keys: Vec<Vec<u8>> = dict.iter().map(|(key, _)| key.to_vec()).collect();
    for key in keys {
        let text = dict
            .get(&key)
            .and_then(Object::as_bytes)
            .map(<[u8]>::to_vec);
        if let Some(text) = text {
            dict.set(&key[..], Object::hex(text));
        }
    }
}

#[test]
fn what_the_dictionary_refers_to_is_in_part_four_in_the_clear_and_at_its_new_number() {
    for (name, key) in KEYS {
        // Its /O an object of its own, as a dictionary's value may be.
        let o = opened(key, "");
        let o = Encryptor::of(&o).unwrap().unwrap();
        let o = o
            .dictionary()
            .get(b"O")
            .unwrap()
            .as_bytes()
            .unwrap()
            .to_vec();
        let hex: String = o.iter().map(|byte| format!("{byte:02X}")).collect();
        let value = format!("<{hex}>");
        let bytes = hand_made(
            key,
            false,
            |dict| {
                in_hex(dict);
                dict.set("O", Object::from(ObjectId::new(5, 0)));
            },
            &[&value],
        );
        let reading = opened(&bytes, "");
        assert!(
            matches!(reading.encryption(), Encryption::Opened { .. }),
            "{name}"
        );
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        assert!(encryptor.dictionary_object(ObjectId::new(5, 0)), "{name}");
        for options in kinds() {
            let (out, done) = own(&bytes, "", &options).unwrap();
            let back = opened(&out, "");
            assert!(back.facts().linearized, "{name}");
            let number = (back.trailer().dict().get(b"Encrypt").unwrap())
                .as_reference()
                .unwrap();
            let held = back.get(number).unwrap().as_dict().unwrap();
            let value = held.get(b"O").unwrap().as_reference().unwrap();
            // In part 4, on its own, whatever may be packed, as written.
            assert_eq!(back.get(value).unwrap().as_bytes(), Some(&o[..]), "{name}");
            let at = done.layout.position(value.number).unwrap();
            assert!(done.layout.first_section() < at, "{name}");
            assert!(at < done.layout.hint_extent().0, "{name}");
            assert!(holds(&out, hex.as_bytes()), "{name}");
        }
    }
}

#[test]
fn a_dictionary_in_the_trailer_stays_there() {
    for (name, key) in KEYS {
        let bytes = hand_made(key, true, in_hex, &[]);
        let reading = opened(&bytes, "");
        assert!(
            matches!(reading.encryption(), Encryption::Opened { .. }),
            "{name}"
        );
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        assert!(encryptor.direct(), "{name}");
        for options in kinds() {
            let (out, _) = own(&bytes, "", &options).unwrap();
            let back = opened(&out, "");
            assert!(back.facts().linearized, "{name}");
            assert!(
                matches!(back.encryption(), Encryption::Opened { .. }),
                "{name}"
            );
            let encrypt = back.trailer().dict().get(b"Encrypt").unwrap();
            assert!(encrypt.as_dict().is_some(), "{name}");
        }
    }
}

#[test]
fn asked_to_pack_under_an_encryption_that_doesnt_it_isnt_packed() {
    // AES-128's strings, its streams left in the clear.
    let key = KEYS[2].1;
    let bytes = hand_made(
        key,
        false,
        |dict| {
            in_hex(dict);
            dict.set("StmF", "Identity");
        },
        &[],
    );
    let reading = opened(&bytes, "");
    let encryptor = Encryptor::of(&reading).unwrap().unwrap();
    assert!(!encryptor.packs());
    let [_, stream, packed] = kinds();
    let (out, _) = own(&bytes, "", &packed).unwrap();
    assert!(!holds(&out, b"/ObjStm"));
    let (alone, _) = own(&bytes, "", &stream).unwrap();
    assert_eq!(out, alone);
    // A table asked to pack is refused first.
    let mut table = Options::DEFAULT;
    table.pack = true;
    let refused = own(&bytes, "", &table).unwrap_err();
    assert!(matches!(refused, Error::Write(write::Error::PackedTable)));
    // Under one that does, it is.
    let plain = built(&secrets(""));
    let (out, _) = under(&plain, "", Some(&created("user", true)), &packed).unwrap();
    assert!(holds(&out, b"/ObjStm"));
}

#[test]
fn an_object_at_an_earlier_dictionary_s_number_is_encrypted_as_any_is() {
    // Sections that name two dictionaries: the first's 5, the update's 4, which is
    // what opens it. So 5, which the catalog names, is read as it's written, and in
    // the file written, whose one dictionary is another, it's encrypted as any is.
    let text = "written as it is";
    let earlier = format!("<< /Mine ({text}) >>");
    let mut bytes = hand_made(KEYS[1].1, false, in_hex, &[&earlier]);
    let prev = (bytes.windows(6).rposition(|w| w == b"\nxref\n")).unwrap() + 1;
    let named = (bytes.windows(14).position(|w| w == b"/Encrypt 4 0 R")).unwrap();
    bytes[named..named + 14].copy_from_slice(b"/Encrypt 5 0 R");
    let id = {
        let id = (bytes.windows(4).rposition(|w| w == b"/ID ")).unwrap();
        let to = id + bytes[id..].iter().position(|byte| *byte == b']').unwrap();
        String::from_utf8_lossy(&bytes[id..=to]).into_owned()
    };
    let at = bytes.len();
    let catalog = "1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Mine 5 0 R >>\nendobj\n";
    bytes.extend_from_slice(catalog.as_bytes());
    let table = bytes.len();
    bytes.extend_from_slice(
        format!(
            "xref\n1 1\n{at:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Encrypt 4 0 R {id} /Prev {prev} >>\nstartxref\n{table}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let reading = opened(&bytes, "");
    assert!(matches!(reading.encryption(), Encryption::Opened { .. }));
    let encryptor = Encryptor::of(&reading).unwrap().unwrap();
    assert!(encryptor.dictionary_object(ObjectId::new(5, 0)));
    assert_eq!(encryptor.at(), Some(ObjectId::new(4, 0)));
    let (out, _) = own(&bytes, "", &Options::DEFAULT).unwrap();
    assert!(!holds(&out, text.as_bytes()), "it's in the clear");
    let back = opened(&out, "");
    let doc = back.to_document().unwrap();
    let root = doc.trailer().dict().get(b"Root").unwrap().as_reference();
    let mine = doc.get_dict(root.unwrap()).unwrap().get(b"Mine").unwrap();
    let mine = doc.get_dict(mine.as_reference().unwrap()).unwrap();
    assert_eq!(mine.get(b"Mine").unwrap().as_bytes(), Some(text.as_bytes()));
}

#[test]
fn an_object_at_the_number_an_encryptor_writes_its_dictionary_at_is_refused() {
    let key = opened(KEYS[4].1, "");
    let encryptor = Encryptor::of(&key).unwrap().unwrap();
    let ObjectId { number: n, .. } = encryptor.at().unwrap();
    // Numbers up to the dictionary's, so one of them is at it.
    let mut objects = secrets("");
    while objects.len() < n as usize {
        objects.push("<< /More true >>".to_string());
    }
    let more: Vec<String> = (10..=n).map(|n| format!("{n} 0 R")).collect();
    objects[0] = objects[0].replace(">>", &format!("/All [{}] >>", more.join(" ")));
    let bytes = built(&objects);
    let mut out = vec![7];
    let reading = opened(&bytes, "");
    let doc = reading.to_document().unwrap();
    let refused = family::linearize(
        &mut out,
        &doc,
        Some(&encryptor),
        Some(&reading),
        &Options::DEFAULT,
    )
    .unwrap_err();
    assert!(
        matches!(refused, Error::Write(write::Error::EncryptAt(at)) if at == n),
        "{refused}"
    );
    assert_eq!(out, [7]);
}

#[test]
fn a_file_with_no_id_of_its_own_takes_the_encryptor_s_first_string() {
    // Another file's encryption, its key made from that file's /ID (RC4, AES-128) or
    // not, which opens this one only by that string.
    let page = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] /Mark (secret page) >>",
    ];
    let bytes = file(&page, "/Root 1 0 R");
    for (name, key) in KEYS {
        let key = opened(key, "");
        // Under RC4, told to reuse the keystreams the key's file used.
        let encryptor = Encryptor::of(&key).unwrap().unwrap();
        let encryptor = encryptor.reusing_rc4_keystreams();
        let (out, done) = under(&bytes, "", Some(&encryptor), &Options::DEFAULT).unwrap();
        assert!(!holds(&out, SECRET), "{name}");
        let first = encryptor.first_id().unwrap();
        assert_eq!(done.id.unwrap()[0], first, "{name}");
        let doc = opened(&out, "").to_document().unwrap();
        marked(&doc, "secret page");
    }
}

#[test]
fn what_names_the_dictionary_of_the_file_read_names_what_s_written_or_nothing() {
    // The catalog names the encryption dictionary and a value of it, which only an
    // object can: a reference, as any, that follows them or reads null.
    let (_, key) = KEYS[1];
    let reading = opened(key, "");
    let o = Encryptor::of(&reading).unwrap().unwrap();
    let o = o
        .dictionary()
        .get(b"O")
        .unwrap()
        .as_bytes()
        .unwrap()
        .to_vec();
    let hex: String = o.iter().map(|byte| format!("{byte:02X}")).collect();
    let value = format!("<{hex}>");
    let bytes = hand_made_of(
        "/Mine 4 0 R /Value 5 0 R",
        key,
        false,
        |dict| {
            in_hex(dict);
            dict.set("O", Object::from(ObjectId::new(5, 0)));
        },
        &[&value],
    );
    let named = |doc: &Document, key: &[u8]| {
        let root = doc.trailer().dict().get(b"Root").unwrap().as_reference();
        doc.get_dict(root.unwrap())
            .unwrap()
            .get(key)
            .unwrap()
            .clone()
    };
    for options in kinds() {
        // As it is, the catalog's and the trailer's are one.
        let (out, _) = own(&bytes, "", &options).unwrap();
        let doc = opened(&out, "").to_document().unwrap();
        let encrypt = doc.trailer().dict().get(b"Encrypt").unwrap().as_reference();
        assert_eq!(named(&doc, b"Mine").as_reference(), encrypt);
        assert_eq!(dictionaries(&doc), 1);
        let dictionary = doc.get_dict(encrypt.unwrap()).unwrap();
        let value = dictionary.get(b"O").unwrap().as_reference();
        assert_eq!(named(&doc, b"Value").as_reference(), value);

        // Under another, or none: neither's carried, whatever named them.
        let new = created("user", true);
        let (out, _) = under(&bytes, "", Some(&new), &options).unwrap();
        let doc = opened(&out, "user").to_document().unwrap();
        assert!(named(&doc, b"Mine").is_null());
        assert!(named(&doc, b"Value").is_null());
        assert_eq!(dictionaries(&doc), 1);
        assert!(!holds(&out, hex.as_bytes()));
        let (out, _) = under(&bytes, "", None, &options).unwrap();
        let doc = opened(&out, "").to_document().unwrap();
        assert!(named(&doc, b"Mine").is_null());
        assert!(named(&doc, b"Value").is_null());
        assert_eq!(dictionaries(&doc), 0);
        assert!(!holds(&out, hex.as_bytes()));
    }
}

/// The same document writes the same bytes on one thread as on four, under
/// the test writer's derived IVs; under the operating system's, another's
/// bytes, which read back to the same document.
#[test]
fn the_same_document_writes_the_same_bytes_on_any_number_of_threads() {
    let mut objects = secrets("");
    for _ in 0..500 {
        objects.push("<< /Filler (secret filler) >>".to_string());
    }
    let list: Vec<String> = (10..objects.len() + 1)
        .map(|n| format!("{n} 0 R"))
        .collect();
    objects[7] = objects[7].replace(">>", &format!("/All [{}] >>", list.join(" ")));
    let bytes = encrypted_as(KEYS[2].1, &built(&objects));
    let [_, _, mut packed] = kinds();
    packed.threads = 1;
    let (one, _) = own_derived(&bytes, "", &packed).unwrap();
    packed.threads = 4;
    let (four, _) = own_derived(&bytes, "", &packed).unwrap();
    assert_eq!(one, four);
    assert!(!holds(&one, SECRET));
    // Under the operating system's IVs, as a caller writes, another's bytes
    // on four threads as on one, the same document read back.
    packed.threads = 1;
    let (one, _) = own(&bytes, "", &packed).unwrap();
    packed.threads = 4;
    let (four, _) = own(&bytes, "", &packed).unwrap();
    assert_ne!(one, four);
    assert_eq!(in_the_clear(&one, ""), in_the_clear(&four, ""));
}

#[test]
fn a_file_laid_out_encrypted_is_laid_out_again() {
    let plain = built(&secrets(""));
    for (name, key) in KEYS {
        let bytes = encrypted_as(key, &plain);
        for mut options in kinds() {
            options.rc4 = Rc4::ReusingKeystreams;
            let once = family::linearize_bytes(&bytes, &options).unwrap();
            let (twice, done) = own(&once, "", &options).unwrap();
            reads_back(&twice, &done, "", name);
        }
    }
}

/// Writes what's beside qpdf's own among PDFium's cases, in the folder
/// `PDF_LINEARIZE_ENCRYPTED` names, `tests/pdfium/encrypted`: each of its files,
/// password `user`, as this crate lays it out.
#[test]
#[ignore = "writes files PDFium's cases read"]
fn the_files_of_pdfiums_cases_are_written() {
    let folder = std::env::var("PDF_LINEARIZE_ENCRYPTED").unwrap();
    let folder = std::path::Path::new(&folder);
    let [table, _, packed] = kinds();
    for (file, kind, options) in [("rc4", "table", table), ("aes-256", "packed", packed)] {
        let bytes = std::fs::read(folder.join(format!("qpdf-{file}.pdf"))).unwrap();
        let given = [Password("user")];
        let mut options = options;
        options.read.passwords = &given;
        options.rc4 = Rc4::ReusingKeystreams;
        let out = family::linearize_bytes(&bytes, &options).unwrap();
        std::fs::write(folder.join(format!("written-{file}-{kind}.pdf")), out).unwrap();
    }
}
