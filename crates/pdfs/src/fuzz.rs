//! What the fuzz target checks, which unit tests check too: the promises the
//! facade's reading makes, on any input. It's compiled only for tests and
//! under cargo-fuzz's `--cfg fuzzing`.

#[cfg(any(test, feature = "insecure-test-writer"))]
use std::convert::Infallible;

use pdf_crypt::Password;
use pdf_linearize::check::Verdict;
use pdf_objects::Why;
use pdf_objects::version::Version;
use pdf_semantics::info::Info;
use pdf_semantics::xmp::Claims;
use pdf_structure::Encryption;
use pdf_structure::read::{ByteSource, Faults};
use pdf_structure::write::{self, Encryptor};

use crate::{Error, Limits, Options, Pdf};

/// The password of what [`read`] writes under a new encryption.
const PASSWORD: &str = "fuzz";

/// What a reading of an input gave, by its parts, compared by value: the
/// same reading under other options, or the same document written in the
/// clear and unlocked, gives the same. An error is its words.
#[derive(Debug, Clone, PartialEq)]
struct Answers {
    version: Result<Option<Version>, String>,
    catalog: Result<(), String>,
    info: Result<Option<Info>, String>,
    claims: Result<Option<Claims>, String>,
    pages: Result<Vec<Result<Content, String>>, String>,
    verdict: Result<Verdict, String>,
}

/// What a page's content gave: its bytes, how many streams, and how many
/// operations read before the first that didn't, if one didn't.
#[derive(Debug, Clone, PartialEq)]
struct Content {
    bytes: Vec<u8>,
    streams: usize,
    operations: usize,
    stopped: bool,
}

/// What [`read`] reached of an input, for the tests to say each promise was
/// met with.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reached {
    /// Pages read under the default options.
    pub pages: usize,
    /// Pages whose content was joined there.
    pub contents: usize,
    /// Pages of the input written under a new encryption whose content,
    /// read with no password, was refused as locked.
    pub locked: usize,
    /// Inputs written under a new encryption that were unlocked by its
    /// password and read as written in the clear.
    pub unlocked: usize,
}

/// `pdf`'s answers, each page's content held within `limits`' cap on it and
/// its operations walked to their end.
fn answers<S: ByteSource>(pdf: &Pdf<S>, reached: &mut Reached) -> Answers {
    let words = |error: Error| error.to_string();
    let pages = pdf.pages().map_err(words).map(|pages| {
        let cap = pdf.limits().content.content;
        let read = |page: crate::Page<'_, S>| {
            let content = page.content().map_err(words)?;
            assert!(content.bytes().len() <= cap, "past the content's cap");
            let mut operations = 0;
            let mut stopped = false;
            for operation in content.operations() {
                match operation {
                    Ok(_) => operations += 1,
                    Err(_) => {
                        stopped = true;
                        break;
                    }
                }
            }
            Ok(Content {
                bytes: content.bytes().to_vec(),
                streams: content.concat().stream_count(),
                operations,
                stopped,
            })
        };
        let pages: Vec<Result<Content, String>> = pages.iter().map(read).collect();
        reached.pages += pages.len();
        reached.contents += pages.iter().filter(|page| page.is_ok()).count();
        pages
    });
    let verdict = pdf.linearized().map_err(words);
    if let Ok(verdict) = verdict {
        assert!(
            !verdict.holds || verdict.linearized,
            "holds, not linearized"
        );
        assert!(
            !verdict.judged_whole || verdict.linearized,
            "judged, not linearized"
        );
    }
    Answers {
        version: pdf.version().map_err(words),
        catalog: pdf.catalog().map(|_| ()).map_err(words),
        info: pdf.info().map_err(words),
        claims: pdf.claims().map_err(words),
        pages,
        verdict,
    }
}

/// Caps small enough that an input of a few hundred bytes meets them.
fn small() -> Limits {
    let mut limits = Limits::DEFAULT;
    limits.read.stream = 64;
    limits.read.parser.values = 64;
    limits.read.parser.nesting = 4;
    limits.content.content = 48;
    limits.content.operands = 4;
    limits.content.depth = 4;
    limits.semantics.pages = 4;
    limits.semantics.page_depth = 3;
    limits.semantics.tree_depth = 2;
    limits.semantics.tree_nodes = 4;
    limits.semantics.xmp_bytes = 128;
    limits.linearize.pages = 4;
    limits
}

/// `data` read as a file by the facade, every answer `Ok` or an error,
/// never a panic: opened from memory, which always opens; its version,
/// catalog, document information dictionary, claims, page tree, each page's
/// content joined within its cap and its operations walked, and its
/// linearization's verdict, which holds and is judged whole only where it's
/// linearized. With faults kept, every answer the same as with none looked
/// for, which finds none; and under small caps, the same promises.
///
/// Then, where it isn't encrypted, its document written whole in the clear
/// and under a new encryption by a password: read with no password, each
/// page's content refused as locked or read as the clear file's; unlocked by
/// the password's encodings, by one of them, it reads as the clear file
/// does. Built with the test writer (`insecure-test-writer`, which the fuzz
/// target turns on, and for the crate's tests), the new encryption's key and
/// salts are drawn from the input and every AES IV derived from its object
/// and data, so an input writes the same bytes each time; built without it,
/// as a crate that fuzzes itself builds this one under `--cfg fuzzing`,
/// they're the operating system's.
pub fn read(data: &[u8]) -> Reached {
    let mut reached = Reached::default();
    let pdf = Options::DEFAULT
        .from_bytes(data)
        .expect("bytes in memory open");
    let off = answers(&pdf, &mut reached);
    assert!(
        pdf.faults().is_empty(),
        "faults told where none are looked for"
    );
    let mut kept = Options::DEFAULT;
    kept.faults = Faults::Kept;
    let pdf_kept = kept.from_bytes(data).expect("bytes in memory open");
    assert_eq!(
        answers(&pdf_kept, &mut Reached::default()),
        off,
        "faults kept"
    );

    let mut capped = Options::DEFAULT;
    capped.limits = small();
    let pdf_capped = capped.from_bytes(data).expect("bytes in memory open");
    answers(&pdf_capped, &mut Reached::default());

    if *pdf.encryption() == Encryption::None {
        encrypted(data, &pdf, &mut reached);
    }
    reached
}

/// The arm of [`read`] that writes `pdf`'s document, `data`'s, in the clear
/// and under a new encryption, and reads both back.
fn encrypted(data: &[u8], pdf: &Pdf<&[u8]>, reached: &mut Reached) {
    let reading = pdf.reading();
    let Ok(doc) = reading.to_document() else {
        return;
    };
    let Ok(password) = Password(PASSWORD).prepared() else {
        return;
    };
    let protection = pdf_crypt::Protection::new(&password);
    #[cfg(any(test, feature = "insecure-test-writer"))]
    let encryptor = Encryptor::create_with(&protection, &mut Drawn::over(data))
        .map(|encryptor| encryptor.with_test_ivs(pdf_crypt::Ivs::derived));
    #[cfg(not(any(test, feature = "insecure-test-writer")))]
    let encryptor = {
        let _ = data;
        Encryptor::create(&protection)
    };
    let Ok(encryptor) = encryptor else {
        return;
    };
    let mut options = write::Options::DEFAULT;
    options.threads = 1;
    let (mut clear, mut locked) = (Vec::new(), Vec::new());
    let from: &dyn write::ReadFrom = reading;
    if write::document(&mut clear, &doc, None, Some(from), &options).is_err()
        || write::document(&mut locked, &doc, Some(&encryptor), Some(from), &options).is_err()
    {
        return;
    }
    let clear = Options::DEFAULT
        .from_bytes(&clear[..])
        .expect("bytes in memory open");
    let wanted = answers(&clear, &mut Reached::default());
    let locked = Options::DEFAULT
        .from_bytes(&locked[..])
        .expect("bytes in memory open");
    assert_eq!(*locked.encryption(), Encryption::Locked);
    if let (Ok(wanted_pages), Ok(pages)) = (&wanted.pages, locked.pages()) {
        assert_eq!(pages.len(), wanted_pages.len(), "the locked file's pages");
        for (page, clear) in pages.iter().zip(wanted_pages) {
            match page.content() {
                Ok(content) => {
                    let clear = clear.as_ref().expect("read locked, not in the clear");
                    assert_eq!(
                        content.bytes(),
                        clear.bytes,
                        "read locked, not as in the clear"
                    );
                }
                Err(Error::Content(pdf_content_streams::Pushed::Decode(unreadable)))
                    if unreadable.why == Why::Locked =>
                {
                    reached.locked += 1;
                }
                Err(error) => {
                    let clear = clear
                        .as_ref()
                        .expect_err("refused locked, read in the clear");
                    assert_eq!(&error.to_string(), clear, "refused otherwise locked");
                }
            }
        }
    }
    let encodings = Password(PASSWORD).encodings();
    let unlocked = match locked.unlock(&encodings) {
        Ok(unlocked) => unlocked,
        Err((_, refused)) => panic!("its own password refused: {refused}"),
    };
    let Encryption::Opened { by, .. } = unlocked.encryption() else {
        panic!("unlocked, not opened");
    };
    assert!(
        by.is_some_and(|(index, _)| index < encodings.len()),
        "by {by:?}"
    );
    let got = answers(&unlocked, &mut Reached::default());
    assert_eq!(got, wanted, "unlocked, not as in the clear");
    reached.unlocked += 1;
}

/// A random number generator for [`read`]'s new encryption alone: the
/// input's bytes, then a counter's. It's no cryptographic generator, but
/// says it is (`TryCryptoRng`) so that pdf-crypt's test writer takes it,
/// through `Encryptor::create_with`, which only a fuzz body and the tests may
/// do: it's compiled for nothing else, and only with the test writer.
#[cfg(any(test, feature = "insecure-test-writer"))]
struct Drawn<'a> {
    data: &'a [u8],
    counter: u8,
}

#[cfg(any(test, feature = "insecure-test-writer"))]
impl Drawn<'_> {
    /// `data`'s bytes, then a counter's.
    fn over(data: &[u8]) -> Drawn<'_> {
        Drawn { data, counter: 0 }
    }
}

#[cfg(any(test, feature = "insecure-test-writer"))]
impl pdf_crypt::rand_core::TryRng for Drawn<'_> {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut bytes = [0; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut bytes = [0; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        for byte in dst {
            *byte = match self.data.split_first() {
                Some((&first, rest)) => {
                    self.data = rest;
                    first
                }
                None => {
                    self.counter = self.counter.wrapping_add(1);
                    self.counter
                }
            };
        }
        Ok(())
    }
}

#[cfg(any(test, feature = "insecure-test-writer"))]
impl pdf_crypt::rand_core::TryCryptoRng for Drawn<'_> {}

#[cfg(test)]
mod tests {
    use pdf_linearize::family;
    use pdf_objects::dict;
    use pdf_objects::object::{Object, Stream, Value};
    use pdf_semantics::xmp;
    use pdf_structure::{Document, XrefKind};

    use super::*;

    fn r(n: u32) -> Object {
        Object::reference(n)
    }

    fn stream(data: &str) -> Object {
        Object::new(Value::Stream(Box::new(Stream::new(
            dict! {},
            data.as_bytes().to_vec(),
        ))))
    }

    /// One page drawing stream 4.
    fn one_page() -> Document {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => r(1) });
        doc.insert(1, dict! { "Type" => "Catalog", "Pages" => r(2) });
        let media = vec![Object::from(0), 0.into(), 612.into(), 792.into()];
        let root =
            dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1, "MediaBox" => media };
        doc.insert(2, root);
        let page = dict! {
            "Type" => "Page", "Parent" => r(2), "Contents" => r(4), "Resources" => dict! {},
        };
        doc.insert(3, page);
        doc.insert(4, stream("0 0 m 9 9 l S"));
        doc
    }

    /// Three pages under a root that gives them a color space: one drawing an
    /// inline image in it, one whose `/Contents` holds a stream, a reference
    /// to nothing and a dictionary, one with none; a title and two dates, one
    /// that doesn't read; a `/Version` past the header's; a packet of claims.
    fn three_pages() -> Document {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => r(1), "Info" => r(9) });
        let mut claims = xmp::Claims::default();
        claims.pdfa_part = Some("2".into());
        claims.pdfa_conformance = Some("B".into());
        let packet = xmp::write(&claims).unwrap();
        let metadata = Stream::new(
            dict! { "Type" => "Metadata", "Subtype" => "XML" },
            packet.into_bytes(),
        );
        let catalog = dict! {
            "Type" => "Catalog", "Pages" => r(2), "Version" => "2.0", "Metadata" => r(10),
        };
        doc.insert(1, catalog);
        let resources = dict! { "ColorSpace" => dict! { "CS0" => "DeviceCMYK" } };
        let root = dict! {
            "Type" => "Pages", "Kids" => vec![r(3), r(4), r(5)], "Count" => 3,
            "Resources" => resources,
        };
        doc.insert(2, root);
        let page = |contents: Option<Object>| {
            let mut page = dict! { "Type" => "Page", "Parent" => r(2) };
            if let Some(contents) = contents {
                page.set("Contents", contents);
            }
            page
        };
        doc.insert(3, page(Some(r(6))));
        let elements = vec![r(7), r(99), Object::from(dict! { "A" => 1 })];
        doc.insert(4, page(Some(Object::from(elements))));
        doc.insert(5, page(None));
        doc.insert(6, stream("q BI /W 1 /H 1 /BPC 8 /CS /CS0 ID abcd EI Q"));
        doc.insert(7, stream("BT (text) Tj ET"));
        let info = dict! {
            "Title" => Object::text("A title"),
            "CreationDate" => Object::text("D:20261112093000Z"),
            "ModDate" => Object::text("last Tuesday"),
        };
        doc.insert(9, info);
        doc.insert(10, Object::new(Value::Stream(Box::new(metadata))));
        doc
    }

    /// `doc` written whole in `kind`, packed where asked, under `encryptor`.
    fn written(
        doc: &Document,
        kind: XrefKind,
        pack: bool,
        encryptor: Option<&Encryptor>,
    ) -> Vec<u8> {
        let mut options = write::Options::DEFAULT;
        options.kind = kind;
        options.pack = pack.then_some(write::Pack::DEFAULT);
        options.threads = 1;
        let mut out = Vec::new();
        write::document(&mut out, doc, encryptor, None, &options).unwrap();
        out
    }

    /// Files of what the facade reads apart: a page, pages with their
    /// `/Contents` of each shape, what a file says and claims, packed in
    /// object streams, linearized, and encrypted.
    fn seeds() -> Vec<Vec<u8>> {
        let one = written(&one_page(), XrefKind::Table, false, None);
        let three = written(&three_pages(), XrefKind::Table, false, None);
        let packed = written(&three_pages(), XrefKind::Stream, true, None);
        let linearized = family::linearize_bytes(&three, &family::Options::DEFAULT).unwrap();
        let password = Password(PASSWORD).prepared().unwrap();
        let protection = pdf_crypt::Protection::new(&password);
        let encryptor = Encryptor::create_with(&protection, &mut Drawn::over(b"a seed"))
            .unwrap()
            .with_test_ivs(pdf_crypt::Ivs::derived);
        let encrypted = written(&three_pages(), XrefKind::Table, false, Some(&encryptor));
        vec![one, three, packed, linearized, encrypted]
    }

    /// A mutation of `seed`, from `random`, as a fuzzer might make it.
    fn mutated(seed: &[u8], random: &mut impl FnMut() -> usize) -> Vec<u8> {
        const PIECES: [&[u8]; 30] = [
            b"[",
            b"]",
            b"<<",
            b">>",
            b"obj",
            b"endobj",
            b"stream",
            b"endstream",
            b"/Contents",
            b"/Kids",
            b"/Pages",
            b"/Count",
            b"/Resources",
            b"/MediaBox",
            b"/Version",
            b"/Info",
            b"/Metadata",
            b"/Linearized",
            b"1 0 R",
            b"2 0 R",
            b"3 0 R",
            b"4 0 R",
            b"null",
            b"(a)",
            b"7",
            b"BT",
            b"Tj",
            b"BI",
            b"ID",
            b"Q",
        ];
        let mut data = seed.to_vec();
        for _ in 0..1 + random() % 4 {
            let at = random() % (data.len() + 1);
            let end = match random() % 3 {
                0 => at,
                _ => data.len().min(at + 1 + random() % 8),
            };
            let piece: &[u8] = match random() % 3 {
                1 => &[],
                _ => PIECES[random() % PIECES.len()],
            };
            data.splice(at..end, piece.iter().copied());
        }
        data
    }

    fn generator(mut state: u64) -> impl FnMut() -> usize {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize
        }
    }

    /// Every seed keeps every promise, and between them each is met: pages
    /// read and joined, a locked file's content refused as locked, and a file
    /// written under a new encryption unlocked and read as in the clear.
    #[test]
    fn the_seeds_keep_every_promise_and_reach_each() {
        let mut all = Reached::default();
        for seed in seeds() {
            let reached = read(&seed);
            all.pages += reached.pages;
            all.contents += reached.contents;
            all.locked += reached.locked;
            all.unlocked += reached.unlocked;
        }
        assert!(all.pages >= 10 && all.contents >= 8, "{all:?}");
        assert!(all.locked >= 4 && all.unlocked >= 4, "{all:?}");
    }

    #[test]
    fn mutated_seeds_keep_every_promise() {
        let mut random = generator(0x5EED);
        for seed in seeds() {
            for _ in 0..60 {
                read(&mutated(&seed, &mut random));
            }
        }
    }

    #[test]
    fn random_bytes_keep_every_promise() {
        let mut random = generator(0xB17E5);
        for _ in 0..500 {
            let data: Vec<u8> = (0..random() % 96).map(|_| random() as u8).collect();
            read(&data);
        }
    }

    /// `PDFS_FUZZ_SEEDS` names a folder: writes the seeds there, for the
    /// fuzzer to begin from (`just fuzz pdfs`).
    #[test]
    #[ignore = "needs PDFS_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir = std::env::var_os("PDFS_FUZZ_SEEDS").expect("PDFS_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in seeds().into_iter().enumerate() {
            std::fs::write(dir.join(format!("file-{i}")), seed).unwrap();
        }
    }
}
