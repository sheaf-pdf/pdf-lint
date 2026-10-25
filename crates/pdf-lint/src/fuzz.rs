//! What the fuzz target checks, which a unit test checks too: on any
//! bytes, a file is read and checked without a panic, in the same way
//! twice, with every finding's bytes inside what was read; and set against
//! its previous revision, when it has an incremental update. It's
//! compiled only for tests and under cargo-fuzz's `--cfg fuzzing`, with
//! [`seeds`], files for the fuzzer to begin from.

use std::collections::{BTreeMap, BTreeSet};

use crate::bytes;
use crate::check::{Exempt, Hold, Made, Options, UnlistedIn, check};
use crate::object::{ObjectId, Span};
use crate::source::{Document, Source};
use crate::version::Version;

/// The fuzz target's body: `data`'s first two bytes choose how the file
/// after them is held and checked ([`setting`]), and the rest is the file.
pub fn check_file(data: &[u8]) {
    let (choice, file) = match data {
        [a, b, file @ ..] => (u16::from_le_bytes([*a, *b]), file),
        _ => (0, data),
    };
    let doc = bytes::read(file);
    let (hold, options) = setting(choice, &doc);
    let report = check(&doc, &hold, &options);
    assert_eq!(
        report,
        check(&doc, &hold, &options),
        "a check is the same twice"
    );
    for finding in &report.findings {
        if let Some(at) = finding.at {
            assert!(
                within(&doc, file, at),
                "{finding}: its bytes are out of what was read"
            );
        }
    }
    let _ = report.to_string();
    let _ = report.objects();
    if let Some(update) = &doc.update {
        let none: BTreeMap<ObjectId, Vec<u8>> = BTreeMap::new();
        let previous = bytes::read_from(file, &none, update.previous);
        let options = Options {
            paths: true,
            ..options
        };
        let before = check(&previous, &Hold::All(Made::Merged), &options);
        let _ = report.since(&before);
        let _ = report.by_path(&before);
        let _ = report.beyond(&[&before]);
    }
}

/// How `choice`'s bits hold and check `doc`: every object built or merged,
/// or only every other one; as one table an object; deprecations found;
/// unlisted keys found in built objects, all or none; at the file's
/// version, 1.0's or 2.0's; with paths recorded; under some extensions;
/// with keys exempt.
pub fn setting(choice: u16, doc: &Document) -> (Hold, Options) {
    let bit = |n: u16| choice & (1 << n) != 0;
    let made = if bit(0) { Made::Built } else { Made::Merged };
    let hold = match bit(1) {
        false => Hold::All(made),
        true => Hold::Only(
            doc.ids()
                .into_iter()
                .step_by(2)
                .map(|id| (id, made))
                .collect(),
        ),
    };
    let names = ["ADBE_Extn3", "ETSI_PAdES", "Malforms", "ISO_19005_3"];
    let options = Options {
        version: match (choice >> 6) & 3 {
            1 => Some(Version::new(1, 0)),
            2 => Some(Version::new(2, 0)),
            _ => None,
        },
        exempt: match bit(8) {
            false => Vec::new(),
            true => vec![Exempt {
                name: b"Writer".to_vec(),
                prefix: true,
            }],
        },
        paths: bit(9),
        one_table: bit(2),
        unlisted: match (choice >> 4) & 3 {
            1 => UnlistedIn::All,
            2 => UnlistedIn::Never,
            _ => UnlistedIn::Built,
        },
        deprecations: bit(3),
        extensions: match bit(10) {
            false => BTreeSet::new(),
            true => names.iter().map(|name| name.to_string()).collect(),
        },
    };
    (hold, options)
}

/// Whether `at` is inside what was read: the file, or the decoded data of
/// the stream it's in.
fn within(doc: &Document, file: &[u8], at: Span) -> bool {
    let len = match at.stream {
        None => Some(file.len()),
        Some(stream) => doc.decoded.get(&stream).map(Vec::len),
    };
    len.is_some_and(|len| at.start <= at.end && at.end <= len)
}

/// Files for the fuzzer to begin from, each with its name, made for it:
/// a page with an annotation and its appearance; an interactive form with
/// a field merged with its widget, a radio group and a hide action; a
/// structure tree, a name tree and a number tree; a cross-reference stream
/// with extensions declared and XMP metadata; an incremental update; and
/// an encrypted file. Each begins with the two bytes that choose how it's
/// checked, all clear.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let page = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 6 0 R >> >> /Contents 5 0 R /Annots [4 0 R] >>"
            .to_string(),
        "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (A note) \
         /AP << /N 7 0 R >> >>"
            .to_string(),
        stream("", "BT /F1 12 Tf 72 720 Td (Hi) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 20 20]",
            "0 0 m 20 20 l S",
        ),
    ];
    let form = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] \
         /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 7 0 R >> \
         /Encoding << /PDFDocEncoding << /Type /Encoding /Differences [24 /breve] >> >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Rect [10 10 110 30] /P 3 0 R /FT /Tx \
         /T (name) /DA (/Helv 12 Tf 0 g) /V (Ann) /MaxLen 8 /Ff 4096 \
         /AA << /K << /S /JavaScript /JS (AFNumber_Keystroke\\(2\\);) >> \
         /E << /S /JavaScript /JS (e\\(\\)) >> >> \
         /A << /S /Hide /T (group.a) /H false >> /DR << /Font << /Helv 7 0 R >> >> >>",
        "<< /FT /Btn /Ff 49152 /T (group) /V /Off /Kids [6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Rect [10 40 30 60] /P 3 0 R /Parent 5 0 R \
         /T (a) /AS /Off /MK << /CA (l) >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /Helv >>",
    ]
    .map(String::from);
    let structure = [
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 6 0 R /MarkInfo << /Marked true >> \
         /Names << /Dests << /Kids [5 0 R 8 0 R] >> >> /OpenAction << /S /GoTo /D (top) >> >>"
            .to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /StructParents 0 \
         /Contents 4 0 R /Tabs /S >>"
            .to_string(),
        stream("", "/P << /MCID 0 >> BDC 0 0 m 1 1 l S EMC"),
        "<< /Limits [(a) (b)] /Names [(b) [3 0 R /Fit]] >>".to_string(),
        "<< /Type /StructTreeRoot /K 7 0 R /ParentTree << /Nums [0 [7 0 R]] >> \
         /RoleMap << /Para /P >> >>"
            .to_string(),
        "<< /Type /StructElem /S /Para /P 6 0 R /Pg 3 0 R /K 0 >>".to_string(),
        "<< /Limits [(top) (top)] /Names [(top) [3 0 R /XYZ 0 792 null]] >>".to_string(),
    ];
    let encrypted = [
        "<< /Type /Catalog /Pages 2 0 R /Lang <a5b3> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
        "<< /Filter /Standard /V 4 /R 4 /Length 128 /P -4 \
         /CF << /StdCF << /CFM /AESV2 /AuthEvent /DocOpen /Length 16 >> >> \
         /StmF /StdCF /StrF /StdCF \
         /O <0ba3835f88f90388e74e54584125ce142be0de24c6b0d37746e075b891756671> \
         /U <c537d2828205039ff59b444fcf97f0f10021446990b9e4114071a4d9104984c1> >>",
    ]
    .map(String::from);
    let id = "<0123456789abcdef0123456789abcdef>";
    let mut seeds = vec![
        ("page", file(&page, "").0),
        ("form", file(&form, "").0),
        ("structure", file(&structure, "").0),
        ("xref-stream", xref_stream()),
        ("update", update()),
        (
            "encrypted",
            file(&encrypted, &format!("/Encrypt 4 0 R /ID [{id} {id}]")).0,
        ),
    ];
    for (_, seed) in &mut seeds {
        seed.splice(0..0, [0, 0]);
    }
    seeds
}

/// A stream's object: its dictionary's `entries` and its `data`, with the
/// `/Length` it has.
fn stream(entries: &str, data: &str) -> String {
    format!(
        "<< {entries} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

/// A file of these objects' bodies, numbered from 1, with a table, and
/// `trailer`'s entries in the trailer too; and where its table is.
fn file(objects: &[String], trailer: &str) -> (Vec<u8>, usize) {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let offsets = body(&mut out, objects, 1);
    let at = out.len();
    table(&mut out, 1, &offsets);
    let size = objects.len() + 1;
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R {trailer} >>\nstartxref\n{at}\n%%EOF\n")
            .as_bytes(),
    );
    (out, at)
}

/// Writes `objects`, numbered from `first`, and gives where each began.
fn body(out: &mut Vec<u8>, objects: &[String], first: usize) -> Vec<usize> {
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", first + i).as_bytes());
    }
    offsets
}

/// Writes a cross-reference table of `offsets`, the first object's number
/// `first`, and before them object 0's free entry when `first` is 1.
fn table(out: &mut Vec<u8>, first: usize, offsets: &[usize]) {
    let free = first == 1;
    let start = if free { 0 } else { first };
    let count = offsets.len() + usize::from(free);
    out.extend_from_slice(format!("xref\n{start} {count}\n").as_bytes());
    if free {
        out.extend_from_slice(b"0000000000 65535 f \n");
    }
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
}

/// A file whose cross-reference section is a stream, unfiltered, and whose
/// catalog declares Adobe's extension level and, in its XMP metadata,
/// PDF/A-3.
fn xref_stream() -> Vec<u8> {
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF><rdf:Description pdfaid:part="3" pdfaid:conformance="B"/></rdf:RDF></x:xmpmeta>"#;
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /Metadata 4 0 R \
         /Extensions << /ADBE << /BaseVersion /1.7 /ExtensionLevel 8 >> >> >>"
            .to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Tabs /W >>".to_string(),
        stream("/Type /Metadata /Subtype /XML", xmp),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let offsets = body(&mut out, &objects, 1);
    let at = out.len();
    let mut rows = vec![0u8, 0, 0, 0, 0, 0xff, 0xff];
    for offset in offsets.iter().chain([&at]) {
        rows.push(1);
        rows.extend_from_slice(&u32::try_from(*offset).unwrap_or(0).to_be_bytes());
        rows.extend_from_slice(&[0, 0]);
    }
    let n = objects.len() + 1;
    let dict = format!(
        "<< /Type /XRef /Size {} /W [1 4 2] /Root 1 0 R /Length {} >>",
        n + 1,
        rows.len()
    );
    out.extend_from_slice(format!("{n} 0 obj\n{dict}\nstream\n").as_bytes());
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{at}\n%%EOF\n").as_bytes());
    out
}

/// A file and an incremental update of it, which rewrites its page and
/// adds an annotation.
fn update() -> Vec<u8> {
    let original = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /WriterMark 1 >>",
    ]
    .map(String::from);
    let (mut out, previous) = file(&original, "");
    let updated = [
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R] /Rotat 90 >>",
        "<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] /IC [1 0 0] >>",
    ]
    .map(String::from);
    let offsets = body(&mut out, &updated, 3);
    let at = out.len();
    table(&mut out, 3, &offsets);
    out.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R /Prev {previous} >>\nstartxref\n{at}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small, fast generator of bytes, the same each run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
        }
    }

    #[test]
    fn the_seeds_read_and_check_as_written() {
        for (name, seed) in seeds() {
            let doc = bytes::read(&seed[2..]);
            assert!(
                doc.trailer().unwritten.is_none(),
                "{name}: {:?}",
                doc.trailer()
            );
            assert!(doc.warnings.is_empty(), "{name}: {:?}", doc.warnings);
            let report = check(&doc, &Hold::All(Made::Merged), &Options::default());
            assert!(report.objects() > 3, "{name}: {report}");
            check_file(&seed);
        }
        let update = &seeds().into_iter().find(|(n, _)| *n == "update").unwrap().1;
        assert!(
            bytes::read(&update[2..]).update.is_some(),
            "the update is one"
        );
    }

    /// The fuzz target's checks on the seeds, each mutated many times as a
    /// fuzzer might: bytes changed, cut, repeated, and moved, under every
    /// setting in turn.
    #[test]
    fn mutated_files_check_within_their_bytes_the_same_twice() {
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        let fragments: &[&[u8]] = &[
            b"<<",
            b">>",
            b"[",
            b"]",
            b"(",
            b")",
            b"/",
            b" ",
            b"\n",
            b"\r",
            b"0",
            b"9",
            b"-1",
            b"R",
            b"obj",
            b"endobj",
            b"stream",
            b"endstream",
            b"null",
            b"/Kids",
            b"/Parent",
            b"/Type",
            b"/Subtype",
            b"/Length",
            b"/Prev",
            b"1 0 R",
            b"%%EOF",
            b"xref",
            b"trailer",
            b"startxref",
            b"\xff",
        ];
        for (n, (_, seed)) in seeds().into_iter().cycle().take(2000).enumerate() {
            let mut file = seed;
            file[0] = n as u8;
            file[1] = (n >> 8) as u8;
            for _ in 0..1 + random.below(6) {
                let at = 2 + random.below(file.len() - 2);
                match random.below(5) {
                    0 => file[at] = random.next() as u8,
                    1 => {
                        let end = (at + random.below(16)).min(file.len());
                        file.drain(at..end);
                    }
                    2 => {
                        let fragment = fragments[random.below(fragments.len())];
                        file.splice(at..at, fragment.iter().copied());
                    }
                    3 => {
                        let end = (at + random.below(64)).min(file.len());
                        let piece = file[at..end].to_vec();
                        file.splice(at..at, piece);
                    }
                    _ => {
                        let to = 2 + random.below(file.len() - 2);
                        let end = (at + random.below(32)).min(file.len());
                        let piece: Vec<u8> = file.drain(at..end).collect();
                        let to = to.min(file.len());
                        file.splice(to..to, piece);
                    }
                }
                if file.len() < 3 {
                    break;
                }
            }
            check_file(&file);
        }
    }

    /// With PDF_LINT_FUZZ_SEEDS naming a folder, writes the seeds there,
    /// for the fuzzer to begin from (`just fuzz check`).
    #[test]
    #[ignore = "needs PDF_LINT_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_LINT_FUZZ_SEEDS").expect("PDF_LINT_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (name, seed) in seeds() {
            std::fs::write(dir.join(format!("{name}.pdf")), seed).unwrap();
        }
    }
}
