//! What the fuzz target checks, which a unit test checks too: on any
//! bytes, a file is read beside its reading, so that its content is read
//! too, and checked without a panic, in the same way twice, with every
//! finding's bytes inside what was read, the file or a stream's decoded
//! data, an object stream's as the document holds it and a content
//! stream's or the metadata stream's as the reading decodes it again; what
//! its content finds, and what its fonts' streams say of their text,
//! moving nothing else a check of its document alone finds, which reads no
//! stream's data ([`streams_alone`]); and set against its previous
//! revision, when it has an incremental update. It reads under small
//! limits ([`options`], and 4 MiB of content and 4 MiB of fonts' streams in
//! [`setting`]), so that a run tries more inputs. It's compiled only for
//! tests and under cargo-fuzz's `--cfg fuzzing`, with [`seeds`], files for
//! the fuzzer to begin from.

use std::collections::{BTreeMap, BTreeSet};

use crate::bytes::{self, WithReading};
use crate::check::{
    Exempt, Finding, Hold, Made, Options, Report, Rule, Unchecked, UnlistedIn, check,
};
use crate::object::{ObjectId, Span};
use crate::pdf_crypt;
use crate::pdf_crypt::writer::{Kind, Settings};
use crate::source::{Document, Source};
use crate::version::Version;

/// The limits the fuzz target reads under: a stream decodes to at most
/// 1 MiB, and a reading's streams to 4 MiB.
pub fn options() -> bytes::Options {
    let mut options = bytes::Options::DEFAULT;
    options.limits.stream = 1 << 20;
    options.limits.budget = 4 << 20;
    options
}

/// `file` read whole under [`options`], its reading kept beside it, as of
/// the cross-reference section at `section` when there's one.
fn read(file: &[u8], section: Option<usize>) -> WithReading<&[u8]> {
    let mut options = options();
    options.section = section;
    WithReading::new(file, &[], &options).expect("a file in memory reads")
}

/// The fuzz target's body: `data`'s first two bytes choose how the file
/// after them is held and checked ([`setting`]), and the rest is the file,
/// read under [`options`].
pub fn check_file(data: &[u8]) {
    let (choice, file) = match data {
        [a, b, file @ ..] => (u16::from_le_bytes([*a, *b]), file),
        _ => (0, data),
    };
    let doc = read(file, None);
    let (hold, options) = setting(choice, doc.document());
    let report = check(&doc, &hold, &options);
    assert_eq!(
        report,
        check(&doc, &hold, &options),
        "a check is the same twice"
    );
    streams_alone(&report, &check(doc.document(), &hold, &options));
    let mut lengths = BTreeMap::new();
    for finding in &report.findings {
        if let Some(at) = finding.at {
            assert!(
                within(&doc, file, at, &mut lengths),
                "{finding}: its bytes are out of what was read"
            );
        }
    }
    let _ = report.to_string();
    let _ = report.objects();
    if let Some(update) = &doc.document().update {
        let previous = read(file, Some(update.previous));
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
/// with keys exempt; unused resources found. Its content within 4 MiB
/// decoded, and its fonts' streams within 4 MiB, so that a check keeps
/// within the fuzz target's time for an input.
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
        unused: bit(11),
        extensions: match bit(10) {
            false => BTreeSet::new(),
            true => names.iter().map(|name| name.to_string()).collect(),
        },
        content_budget: Some(4 << 20),
        font_budget: Some(4 << 20),
    };
    (hold, options)
}

/// The rules that find what's in content, and nothing else.
const CONTENT: [Rule; 7] = [
    Rule::Balance,
    Rule::Marked,
    Rule::Operator,
    Rule::Operands,
    Rule::Resource,
    Rule::Unused,
    Rule::Content,
];

/// Whether `finding` is a font descriptor's `/CapHeight` missing: what
/// the model's `fn:FontHasLatinChars()` decides, which a font's
/// `/ToUnicode` can, read beside a reading alone.
fn cap_height(finding: &Finding) -> bool {
    finding.rule == Rule::Missing
        && finding.key == "CapHeight"
        && finding.table.starts_with("FontDescriptor")
}

/// Whether `row` is one `fn:FontHasLatinChars()` stood in the way of.
fn latin_row(row: &Unchecked) -> bool {
    row.why == "fn:FontHasLatinChars"
}

/// That `beside`, a check of a file read beside its reading, is `alone`, a
/// check of its document alone, which reads no stream's data, and what
/// streams' data tells: content's own, and what a font's `/ToUnicode`,
/// embedded CMap or glyphs say of whether it has Latin characters. The
/// same findings but content's rules' and a font descriptor's `missing
/// CapHeight`, in the same order, and alone's of those beside too; the
/// same rows not checked, but content's, their column `-`, added, and
/// `fn:FontHasLatinChars`'s, as many or fewer; and the same counts, but
/// units of content read and unused resources, which `alone` has none of.
/// So what content finds, and what a font's streams say of its text, moves
/// nothing else; nor color spaces, which no stream's data says.
fn streams_alone(beside: &Report, alone: &Report) {
    let others = |report: &Report| -> Vec<Finding> {
        (report.findings.iter())
            .filter(|f| !CONTENT.contains(&f.rule) && !cap_height(f))
            .cloned()
            .collect()
    };
    assert_eq!(
        others(beside),
        others(alone),
        "streams' data moves no other finding"
    );
    assert!(
        (alone.findings.iter()).all(|f| !CONTENT.contains(&f.rule)),
        "content found where none is read"
    );
    for finding in alone.findings.iter().filter(|f| cap_height(f)) {
        assert!(
            beside.findings.contains(finding),
            "{finding}: found alone, where the fonts' streams aren't read"
        );
    }
    assert_eq!(
        (alone.content, alone.unused.len()),
        (0, 0),
        "none read alone"
    );
    for (row, &n) in &beside.unchecked {
        let was = alone.unchecked.get(row).copied().unwrap_or(0);
        if latin_row(row) {
            assert!(n <= was, "{row:?} not checked more times");
            continue;
        }
        assert!(n >= was, "{row:?} not checked fewer times");
        assert!(
            n == was || row.column == "-",
            "{row:?} not checked for content"
        );
    }
    for row in alone.unchecked.keys() {
        assert!(
            beside.unchecked.contains_key(row) || latin_row(row),
            "{row:?} not checked alone"
        );
    }
    let rest = |report: &Report| Report {
        findings: Vec::new(),
        unchecked: BTreeMap::new(),
        content: 0,
        unused: BTreeMap::new(),
        ..report.clone()
    };
    assert_eq!(rest(beside), rest(alone), "streams' data moves no count");
}

/// Whether `at` is inside what was read: the file, or the decoded data of
/// the stream it's in, an object stream's as `doc` holds it, else as its
/// reading decodes it, as content is read, each stream's length kept in
/// `lengths` once it's decoded.
fn within(
    doc: &WithReading<&[u8]>,
    file: &[u8],
    at: Span,
    lengths: &mut BTreeMap<ObjectId, Option<usize>>,
) -> bool {
    let len = match at.stream {
        None => Some(file.len()),
        Some(stream) => match doc.document().decoded.get(&stream) {
            Some(data) => Some(data.len()),
            None => *lengths.entry(stream).or_insert_with(|| {
                let decoded = doc.reading().decoded_within(stream, usize::MAX);
                decoded.ok().map(|data| data.len())
            }),
        },
    };
    len.is_some_and(|len| at.start <= at.end && at.end <= len)
}

/// Files for the fuzzer to begin from, each with its name, made for it:
/// a page with an annotation and its appearance; an interactive form with
/// a field merged with its widget, a radio group and a hide action; a
/// structure tree, a name tree and a number tree; a page tree with each
/// fault the walk can't say that ISO's reading of it tells, but a tree of
/// no pages; a page tree a viewer reads otherwise, by its count, a page
/// and a page's box; a name tree whose keys are out of order; XMP metadata
/// with an attribute that doesn't read; content of each kind the check
/// reads, two pages sharing an array of streams, a form, a pattern and a
/// Type 3 font's glyphs, with something in it for each rule of content to
/// find, and a page's content encrypted; a cross-reference stream with
/// extensions declared and XMP metadata; an object stream whose header
/// starts two objects inside another's bytes; an incremental update; and
/// encrypted files, one of them with an object stream whose data isn't
/// encrypted, so that its objects' strings are in the clear.
/// Each begins with the two bytes that choose how it's checked, all
/// clear, as the `check` and `structure` targets take them; the `write`
/// target's take a third ([`write_seeds`]).
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
    let leaf = |parent: &str| {
        format!("<< /Type /Page /Parent {parent} /MediaBox [0 0 612 792] /Resources << >> >>")
    };
    let page_tree = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Parent 2 0 R /Kids [3 0 R 3 0 R 4 0 R 5 0 R 7 0 R] /Count 9 >>"
            .to_string(),
        leaf("2 0 R"),
        "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 6 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Kids [8 0 R] /Count 1 >>".to_string(),
        leaf("2 0 R"),
        "9 0 R".to_string(),
        leaf("5 0 R"),
        leaf("2 0 R"),
    ];
    // A viewer counts 3, has no page where the null is, and reads the first
    // page's null box as none, not the root's.
    let viewer = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R null 4 0 R] /Count 3 /MediaBox [0 0 595 842] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox null /Resources << >> >>".to_string(),
        leaf("2 0 R"),
    ];
    let name_tree = [
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 4 0 R >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        leaf("2 0 R"),
        "<< /Names [(b) [3 0 R /Fit] (a) [3 0 R /XYZ 0 792 null]] >>".to_string(),
    ];
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="3" pdfaid:conformance=B/></rdf:RDF></x:xmpmeta>"#;
    let metadata = [
        "<< /Type /Catalog /Pages 2 0 R /Metadata 4 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        leaf("2 0 R"),
        stream("/Type /Metadata /Subtype /XML", xmp),
    ];
    let id = "<0123456789abcdef0123456789abcdef>";
    let mut seeds = vec![
        ("page", file(&page, "").0),
        ("form", file(&form, "").0),
        ("structure", file(&structure, "").0),
        ("page-tree", file(&page_tree, "").0),
        ("viewer", file(&viewer, "").0),
        ("name-tree", file(&name_tree, "").0),
        ("xmp", file(&metadata, "").0),
        ("content", content()),
        ("content-encrypted", encrypted_content()),
        ("xref-stream", xref_stream()),
        ("object-streams", object_streams(true)),
        ("object-streams-lzw", object_streams(false)),
        ("object-streams-overlapping", overlapping()),
        ("update", update()),
        (
            "encrypted",
            file(&encrypted, &format!("/Encrypt 4 0 R /ID [{id} {id}]")).0,
        ),
        ("encrypted-rc4-40", opens(Kind::Rc4By40, Packing::Streams)),
        ("encrypted-rc4-128", opens(Kind::Rc4By128, Packing::Streams)),
        ("encrypted-aes-128", opens(Kind::Aes128, Packing::Streams)),
        (
            "encrypted-aes-256-r5",
            opens(Kind::Aes256R5, Packing::Streams),
        ),
        ("encrypted-aes-256", opens(Kind::Aes256, Packing::Streams)),
        ("encrypted-own-crypt", opens(Kind::Aes128, Packing::Own)),
        (
            "encrypted-in-the-clear",
            opens(Kind::Aes128, Packing::Clear),
        ),
    ];
    for (_, seed) in &mut seeds {
        seed.splice(0..0, [0, 0]);
    }
    seeds
}

/// [`seeds`] for pdf-structure's `write` target, whose input has a third
/// byte after the two that choose how it's written, before the file: which
/// renumbering the whole file is written under, of five, and, its top bit
/// set, whether its document is read whole and taken by the reading. Each
/// seed once with each, named for it, so each way is fuzzed from the
/// start.
pub fn write_seeds() -> Vec<(String, Vec<u8>)> {
    let mut written = Vec::new();
    for (name, seed) in seeds() {
        for third in (0..5).chain(0x80..0x85) {
            let mut seed = seed.clone();
            seed.insert(2, third);
            written.push((format!("{name}-{third:02x}"), seed));
        }
    }
    written
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
fn file(objects: &[impl AsRef<[u8]>], trailer: &str) -> (Vec<u8>, usize) {
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
fn body(out: &mut Vec<u8>, objects: &[impl AsRef<[u8]>], first: usize) -> Vec<usize> {
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", first + i).as_bytes());
        out.extend_from_slice(object.as_ref());
        out.extend_from_slice(b"\nendobj\n");
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
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF><rdf:Description xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="3" pdfaid:conformance="B"/></rdf:RDF></x:xmpmeta>"#;
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

/// A file whose page tree, page, font and annotations are in an object
/// stream, and whose cross-reference stream is PNG-predicted: with
/// `flate`, both compressed (the object stream with dynamic Huffman
/// codes), else the object stream through LZW and hex, the
/// cross-reference stream as it is.
fn object_streams(flate: bool) -> Vec<u8> {
    // The object stream's data, its header and five objects, as Python's
    // zlib.compress gives it at level 9.
    const DEFLATED: &[u8] = b"\x78\xda\x7d\x50\x4d\x6b\xc3\x30\x0c\xbd\xef\x57\xbc\xe3\x7a\x72\xec\xa4\x29\x85\x12\x68\x0b\x63\x63\xec\x83\x76\xb7\xb0\x83\xdb\x6a\x5b\x46\x63\x77\xb6\x33\xba\x7f\x3f\x29\x61\xb0\x5c\x0a\x0f\x23\xc9\x7a\xef\x49\x32\xc8\x90\xa3\x30\x98\x42\x4f\xe7\x28\x61\x74\x81\x19\xf2\xbc\xc0\x62\x01\xf5\xf2\x73\x22\xa8\x67\xfb\x4e\x11\xea\xbe\x39\x44\xd4\x39\x33\x36\xaf\x50\x6b\xdf\xb9\x04\x8d\xaa\xba\x1a\x77\xca\x1b\x88\xff\x8c\x74\x42\x3d\xd0\xa1\xb1\x2b\x7f\x46\x9d\x71\xa1\xd4\x06\xb3\xb9\x61\x81\x0d\x45\xdf\x85\x3d\x2b\x0b\xff\xc6\x33\xa3\x0f\x34\xcf\x22\xc4\xaa\x12\xa8\xa5\x73\x3e\xb1\x6f\xd9\x17\x67\x83\xfb\xc8\xb4\xa7\xaa\x6d\xb7\x4b\x7d\x2a\x45\x0d\xb5\xb2\x91\x86\x9f\x5b\x3a\x7e\x53\x6a\xf6\x76\x4c\xeb\x85\xff\xf3\xe8\x9c\x64\xaa\x7d\x42\xad\x33\x30\x72\x41\xbf\xaa\x4b\xbc\x50\xc4\xf5\x12\xcc\x21\x34\x0e\xd6\xc1\xef\x3e\xa5\x39\xa6\x40\xb6\x9d\x40\x3d\x9d\xc8\xe1\xcd\x1e\x23\x8b\x3d\xda\x96\x84\xd8\xb6\x72\x89\x8b\xbe\xdb\xaf\x8e\xef\xf5\xe7\x5c\x64\x60\xcc\x05\xe2\xcc\xa3\xf0\xc6\x12\xde\xad\x87\x03\x6a\x8e\x57\x3e\x1c\x28\x0c\xb9\x19\x0f\xc8\xda\x1f\x14\x26\x62\xf9\x0b\x2a\x30\x86\x09";
    let (data, filter) = match flate {
        true => (DEFLATED.to_vec(), "/FlateDecode"),
        false => {
            let objects = pdf_structure::fixture::inflated(DEFLATED);
            let hex: String = pdf_structure::fixture::lzw(&objects)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            (
                format!("{hex}>").into_bytes(),
                "[/ASCIIHexDecode /LZWDecode]",
            )
        }
    };
    let mut out = b"%PDF-1.7\n".to_vec();
    let one = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let four = out.len();
    let dict = format!(
        "<< /Type /ObjStm /N 5 /First 27 /Filter {filter} /Length {} >>",
        data.len()
    );
    out.extend_from_slice(format!("4 0 obj\n{dict}\nstream\n").as_bytes());
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let eight = out.len();
    let row = |kind: u8, field: usize, index: u8| {
        let [high, low] = u16::try_from(field).unwrap_or(0).to_be_bytes();
        [kind, high, low, index]
    };
    let rows = [
        row(0, 0, 255),
        row(1, one, 0),
        row(2, 4, 0),
        row(2, 4, 1),
        row(1, four, 0),
        row(2, 4, 2),
        row(2, 4, 3),
        row(2, 4, 4),
        row(1, eight, 0),
    ];
    // Each row by PNG's Up: its difference from the row above.
    let mut above = [0u8; 4];
    let mut predicted = Vec::new();
    for row in rows {
        predicted.push(2);
        predicted.extend(row.iter().zip(above).map(|(b, a)| b.wrapping_sub(a)));
        above = row;
    }
    let (rows, filter) = match flate {
        true => (
            pdf_structure::fixture::zlib(&predicted),
            " /Filter /FlateDecode",
        ),
        false => (rows.concat(), ""),
    };
    let parms = match flate {
        true => " /DecodeParms << /Predictor 12 /Columns 4 >>",
        false => "",
    };
    let dict = format!(
        "<< /Type /XRef /Size 9 /W [1 2 1] /Root 1 0 R{filter}{parms} /Length {} >>",
        rows.len()
    );
    out.extend_from_slice(format!("8 0 obj\n{dict}\nstream\n").as_bytes());
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{eight}\n%%EOF\n").as_bytes());
    out
}

/// A file whose page tree and pages are in an object stream, unfiltered,
/// whose header starts the second page where the first starts, and the
/// page tree's resources inside the first page, at its own resources.
fn overlapping() -> Vec<u8> {
    let pages = "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 /Resources 5 0 R >>";
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                /Resources << /ProcSet [/PDF] >> >>";
    let three = pages.len() + 1;
    let five = three + page.find("<< /ProcSet").unwrap_or(0);
    let header = format!("2 0 3 {three} 6 {three} 5 {five} ");
    let data = format!("{header}{pages} {page}");
    let mut out = b"%PDF-1.7\n".to_vec();
    let one = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let four = out.len();
    let dict = format!(
        "<< /Type /ObjStm /N 4 /First {} /Length {} >>",
        header.len(),
        data.len()
    );
    out.extend_from_slice(
        format!("4 0 obj\n{dict}\nstream\n{data}\nendstream\nendobj\n").as_bytes(),
    );
    let seven = out.len();
    let row = |kind: u8, field: usize, index: u8| {
        let [high, low] = u16::try_from(field).unwrap_or(0).to_be_bytes();
        [kind, high, low, index]
    };
    let rows = [
        row(0, 0, 255),
        row(1, one, 0),
        row(2, 4, 0),
        row(2, 4, 1),
        row(1, four, 0),
        row(2, 4, 3),
        row(2, 4, 2),
        row(1, seven, 0),
    ]
    .concat();
    let dict = format!(
        "<< /Type /XRef /Size 8 /W [1 2 1] /Root 1 0 R /Length {} >>",
        rows.len()
    );
    out.extend_from_slice(format!("7 0 obj\n{dict}\nstream\n").as_bytes());
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{seven}\n%%EOF\n").as_bytes());
    out
}

/// What encrypts the object stream of [`opens`]'s file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Packing {
    /// The file's crypt filter for streams.
    Streams,
    /// Its own `/Crypt` filter, the file's for streams `/Identity`.
    Own,
    /// Nothing: the file's crypt filter for streams `/Identity`, and none
    /// of its own, so that its objects' strings are in the clear, where
    /// the file's are encrypted.
    Clear,
}

/// A file encrypted as `kind` with the empty user password, so that it
/// opens: its catalog's /Lang encrypted, its page tree and page in an
/// object stream compressed, and encrypted as `packing` says (by a crypt
/// filter only where `kind` has them).
fn opens(kind: Kind, packing: Packing) -> Vec<u8> {
    let id = ID;
    let (values, handler) = encryption(kind);
    let lang =
        pdf_crypt::writer::encrypt_string(&handler, (1, 0), b"en", [1; 16]).unwrap_or_default();
    let objects = "2 0 3 42 << /Type /Pages /Kids [3 0 R] /Count 1 >>\n\
                   << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (In plain view) >>] >>";
    let first = objects.find("<<").unwrap_or(0);
    let deflated = pdf_structure::fixture::zlib(objects.as_bytes());
    let crypt = (packing == Packing::Own).then(|| handler.named(b"StdCF"));
    let data = match packing {
        Packing::Clear => deflated,
        _ => {
            pdf_crypt::writer::encrypt_stream(&handler, (4, 0), &deflated, crypt.as_ref(), [4; 16])
                .unwrap_or_default()
        }
    };
    let filter = match packing {
        Packing::Own => "/Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /StdCF >> null]",
        _ => "/Filter /FlateDecode",
    };
    let mut out = b"%PDF-1.7\n".to_vec();
    let one = out.len();
    out.extend_from_slice(
        format!(
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Lang <{}> >>\nendobj\n",
            hex(&lang)
        )
        .as_bytes(),
    );
    let four = out.len();
    let dict = format!(
        "<< /Type /ObjStm /N 2 /First {first} {filter} /Length {} >>",
        data.len()
    );
    out.extend_from_slice(format!("4 0 obj\n{dict}\nstream\n").as_bytes());
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let five = out.len();
    let row = |kind: u8, field: usize, index: u8| {
        let [high, low] = u16::try_from(field).unwrap_or(0).to_be_bytes();
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
    let streams = match packing {
        Packing::Streams => "/StdCF",
        _ => "/Identity",
    };
    let entries = entries(&values, streams);
    let dict = format!(
        "<< /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R /Encrypt << {entries} >> /ID [<{0}> <{0}>] /Length {1} >>",
        hex(id),
        rows.len()
    );
    out.extend_from_slice(format!("5 0 obj\n{dict}\nstream\n").as_bytes());
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes());
    out
}

/// The file identifier the encrypted seeds are written with.
const ID: &[u8; 16] = b"0123456789abcdef";

/// The encryption dictionary's values for `kind` with the empty user
/// password, and the handler that opens it, under [`ID`].
fn encryption(kind: Kind) -> (pdf_crypt::Dictionary, pdf_crypt::Handler) {
    let settings = Settings {
        kind,
        user: b"",
        owner: b"owner",
        p: -4,
        id: ID,
        encrypt_metadata: true,
    };
    let values = pdf_crypt::writer::dictionary(&settings);
    let handler = pdf_crypt::Handler::open(&values, ID, &[]).expect("the writer's opens");
    (values, handler)
}

/// Bytes in hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// An encryption dictionary's entries for `values`, its crypt filter, where
/// it has one, `/StdCF` for strings and `streams` for streams.
fn entries(values: &pdf_crypt::Dictionary, streams: &str) -> String {
    let mut entries = format!("/Filter /Standard /V {} /R {} /P -4", values.v, values.r);
    if let Some(length) = values.length {
        entries.push_str(&format!(" /Length {length}"));
    }
    let strings = [
        ("O", &values.o),
        ("U", &values.u),
        ("OE", &values.oe),
        ("UE", &values.ue),
        ("Perms", &values.perms),
    ];
    for (name, value) in strings {
        if let Some(value) = value {
            entries.push_str(&format!(" /{name} <{}>", hex(value)));
        }
    }
    if let Some(filter) = values.crypt_filters.first() {
        let method = String::from_utf8_lossy(filter.method.as_deref().unwrap_or_default());
        entries.push_str(&format!(
            " /CF << /StdCF << /CFM /{method} >> >> /StmF {streams} /StrF /StdCF"
        ));
    }
    entries
}

/// A file whose two pages share their content, an array of two streams
/// that `q` and marked content run across, under the resources the page
/// tree gives them: a form XObject drawn, a tiling pattern painted with, a
/// Type 3 font's two glyphs shown, an inline image, an ExtGState nothing
/// uses; and something for each rule of content to find: in the form a
/// name its resources lack and a `q` left open, in the pattern an `EMC`
/// ending none, in a glyph a `d0` of one operand and an `EX` with no `BX`
/// open, and a byte 0x80 that reads as white-space.
fn content() -> Vec<u8> {
    let stream = |entries: &str, data: &[u8]| -> Vec<u8> {
        let mut out = format!("<< {entries} /Length {} >>\nstream\n", data.len()).into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream");
        out
    };
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources << \
          /Font << /F1 9 0 R /T3 10 0 R >> /XObject << /Fm 7 0 R >> \
          /Pattern << /P1 8 0 R >> /ExtGState << /G0 << /CA 0.5 >> >> >> >>"
            .to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [5 0 R 6 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [5 0 R 6 0 R] >>".to_vec(),
        stream(
            "",
            b"q /Pattern cs /P1 scn 0 0 m 9 9 l S BT /F1 12 Tf 72 720 Td (Hi) Tj ET \
              /Span <</MCID 0>> BDC",
        ),
        stream(
            "",
            b"EMC /Fm Do BT /T3 9 Tf (ab) Tj ET\x80BI /W 2 /H 1 /BPC 8 /CS /G ID ab EI Q",
        ),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 9 9] /Resources << /XObject << /Im1 11 0 R >> >>",
            b"q /Im1 Do Q /Gone gs 0 0 9 9 re f q",
        ),
        stream(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 9 9] /XStep 9 /YStep 9 /Resources << >>",
            b"0 0 9 9 re f EMC",
        ),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 9 9] /FontMatrix [0.001 0 0 0.001 0 0] \
          /CharProcs << /a 12 0 R /b 13 0 R >> /Encoding << /Type /Encoding /Differences [97 /a /b] >> \
          /FirstChar 97 /LastChar 98 /Widths [9 9] /Resources << >> >>"
            .to_vec(),
        stream(
            "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
            b"x",
        ),
        stream("", b"9 0 0 0 9 9 d1 0 0 9 9 re f"),
        stream("", b"9 d0 EX"),
    ];
    file(&objects, "").0
}

/// A file encrypted as AES-128 with the empty user password, so that it
/// opens, whose page's content is compressed and encrypted, a `Q` with
/// nothing saved among it.
fn encrypted_content() -> Vec<u8> {
    let (values, handler) = encryption(Kind::Aes128);
    let content = b"q 0 0 1 rg 0 0 9 9 re f BT /F1 9 Tf (In secret) Tj ET Q Q";
    let deflated = pdf_structure::fixture::zlib(content);
    let data = pdf_crypt::writer::encrypt_stream(&handler, (4, 0), &deflated, None, [4; 16])
        .unwrap_or_default();
    let mut stream = format!(
        "<< /Filter /FlateDecode /Length {} >>\nstream\n",
        data.len()
    )
    .into_bytes();
    stream.extend_from_slice(&data);
    stream.extend_from_slice(b"\nendstream");
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        stream,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        format!("<< {} >>", entries(&values, "/StdCF")).into_bytes(),
    ];
    let trailer = format!("/Encrypt 6 0 R /ID [<{0}> <{0}>]", hex(ID));
    file(&objects, &trailer).0
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

    /// The rules that find what's in the common structures: name and number
    /// trees, the page tree, a viewer's pages, and XMP metadata.
    const STRUCTURES: [Rule; 4] = [Rule::Tree, Rule::PageTree, Rule::Viewer, Rule::Xmp];

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

    /// Its metadata declares PDF/A-3, so a mutation of it reaches what's under
    /// that, as one of a file whose prefix isn't bound doesn't.
    #[test]
    fn the_xref_stream_seed_declares_its_extensions() {
        let declared = crate::extension::declared(&bytes::read(&xref_stream()));
        assert_eq!(
            declared.into_iter().collect::<Vec<_>>(),
            ["ADBE_Extn3", "ISO_19005_3"]
        );
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
            // But where objects start inside another's bytes, the second
            // page and the page tree's resources, inside the first page.
            let inside: &[_] = match name {
                "object-streams-overlapping" => &[Some((6, 0)), Some((5, 0))],
                _ => &[],
            };
            let on: Vec<_> = doc.warnings.iter().map(|w| w.object).collect();
            assert_eq!(on, inside, "{name}: {:?}", doc.warnings);
            let report = check(&doc, &Hold::All(Made::Merged), &Options::default());
            assert!(report.objects() > 3, "{name}: {report}");
            // The common structures' seeds find what they're made to, and
            // the rest nothing of theirs.
            let structures: BTreeSet<&str> = (report.findings.iter())
                .filter(|f| STRUCTURES.contains(&f.rule))
                .map(|f| f.rule.name())
                .collect();
            let made: &[_] = match name {
                "page-tree" => &["page-tree"],
                "viewer" => &["page-tree", "viewer"],
                "name-tree" => &["tree"],
                "xmp" => &["xmp"],
                _ => &[],
            };
            assert_eq!(
                structures.into_iter().collect::<Vec<_>>(),
                made,
                "{name}: {report}"
            );
            check_file(&seed);
        }
        // Those that open, open, and their object stream reads.
        for (name, seed) in seeds()
            .into_iter()
            .filter(|(n, _)| n.starts_with("encrypted-"))
        {
            let doc = bytes::read(&seed[2..]);
            let opened = matches!(
                doc.encryption,
                crate::source::Encryption::Opened { owner: false, .. }
            );
            assert!(opened, "{name}: {:?}", doc.encryption);
            assert_eq!(
                doc.get((3, 0)).map(|o| o.at.and_then(|at| at.stream)),
                Ok(Some((4, 0))),
                "{name}"
            );
            let lang = doc
                .get((1, 0))
                .ok()
                .and_then(|o| o.as_dict()?.get(b"Lang").cloned());
            let en = crate::object::Value::String(
                crate::object::Str::Bytes(b"en".to_vec()),
                Some(crate::object::Form::Hex),
            );
            assert_eq!(lang.map(|l| l.into_value()), Some(en), "{name}");
            // The one whose object stream leaves its annotation's string in
            // the clear is recorded so, and found.
            let clear = name == "encrypted-in-the-clear";
            let left: BTreeSet<_> = clear.then_some((4, 0)).into_iter().collect();
            assert_eq!(doc.in_the_clear, left, "{name}");
            let report = check(&doc, &Hold::All(Made::Merged), &Options::default());
            let packed: Vec<_> = (report.findings.iter())
                .filter(|f| f.rule == crate::check::Rule::Packed)
                .map(|f| f.value.as_str())
                .collect();
            let found: &[_] = match clear {
                true => &[
                    "1 string in the clear: its data isn't encrypted, where the file's strings are",
                ],
                false => &[],
            };
            assert_eq!(packed, found, "{name}: {report}");
        }
        let update = &seeds().into_iter().find(|(n, _)| *n == "update").unwrap().1;
        assert!(
            bytes::read(&update[2..]).update.is_some(),
            "the update is one"
        );
    }

    /// Each seed read beside its reading is the whole read, as
    /// `bytes::read_opened` reads it, and under the fuzz target's limits as a
    /// reading taken whole reads it; and with its streams' data read, a
    /// check finds what one of its document alone finds and content's own
    /// and what its fonts' streams say ([`streams_alone`]), and says how
    /// many units of content it read, where one of its document alone reads
    /// none. The content seeds' content has each rule of content finding
    /// something; the rest is clean.
    #[test]
    fn the_seeds_read_alike_beside_their_readings() {
        for (name, seed) in seeds() {
            let file = &seed[2..];
            let opened = WithReading::new(file, &[], &bytes::Options::DEFAULT).unwrap();
            assert_eq!(*opened.document(), bytes::read_opened(file, &[]), "{name}");
            let whole = bytes::Reading::open(file, &options())
                .and_then(bytes::Reading::into_document)
                .unwrap();
            assert_eq!(*read(file, None).document(), whole, "{name}");
            let hold = Hold::All(Made::Merged);
            let options = Options {
                unused: true,
                ..Options::default()
            };
            let checked = |source: &dyn Source| check(source, &hold, &options);
            let (beside, alone) = (checked(&opened), checked(opened.document()));
            // The page's content and its annotation's appearance; the
            // structure's page; the two pages' one unit, the form, the
            // pattern and the two glyphs; the encrypted page.
            let read = match name {
                "page" => 2,
                "structure" | "content-encrypted" => 1,
                "content" => 5,
                _ => 0,
            };
            assert_eq!((beside.content, alone.content), (read, 0), "{name}");
            streams_alone(&beside, &alone);
            let found: BTreeSet<&str> = (beside.findings.iter())
                .filter(|f| CONTENT.contains(&f.rule))
                .map(|f| f.rule.name())
                .collect();
            let want: &[_] = match name {
                "content" => &[
                    "balance", "content", "marked", "operands", "operator", "resource", "unused",
                ],
                "content-encrypted" => &["balance"],
                _ => &[],
            };
            assert_eq!(
                found.into_iter().collect::<Vec<_>>(),
                want,
                "{name}: {beside}"
            );
        }
    }

    /// A Type 0 font's `/ToUnicode`, read beside its reading, says its text
    /// is Latin, so its descriptor's `/CapHeight` is missing; its document
    /// alone reads no stream's data, so there it can't be told, and is
    /// counted beside another's that has no `/ToUnicode`, counted both
    /// ways: what [`streams_alone`] sets aside, and nothing else moves.
    #[test]
    fn a_font_s_text_read_beside_its_reading_and_alone() {
        use pdf_structure::fixture::{self, Body, Xref};

        use crate::dict;
        use crate::object::Object;

        let r = Object::reference;
        let numbers =
            |n: &[i64]| Object::from(n.iter().map(|&n| Object::from(n)).collect::<Vec<_>>());
        // A Type 0 font, object `n`, its CIDFont `n + 1`, its descriptor
        // `n + 2`, and its `/ToUnicode` `n + 3`, mapping a code to `A`,
        // where it has one.
        let type0 = |n: u32, to_unicode: bool| {
            let mut font = dict! {
                "Type" => "Font",
                "Subtype" => "Type0",
                "BaseFont" => "Batang",
                "Encoding" => "Identity-H",
                "DescendantFonts" => vec![r(n + 1)],
            };
            let info = dict! {
                "Registry" => Object::string("Adobe"),
                "Ordering" => Object::string("Identity"),
                "Supplement" => 0,
            };
            let cid_font = dict! {
                "Type" => "Font",
                "Subtype" => "CIDFontType2",
                "BaseFont" => "Batang",
                "CIDSystemInfo" => info,
                "FontDescriptor" => r(n + 2),
                "CIDToGIDMap" => "Identity",
            };
            let descriptor = dict! {
                "Type" => "FontDescriptor",
                "FontName" => "Batang",
                "Flags" => 4,
                "FontBBox" => numbers(&[-200, -200, 1000, 900]),
                "ItalicAngle" => 0,
                "Ascent" => 900,
                "Descent" => -200,
                "StemV" => 80,
            };
            let mut objects = Vec::new();
            if to_unicode {
                font.set("ToUnicode", r(n + 3));
                let cmap = b"1 begincodespacerange <0000> <FFFF> endcodespacerange \
                             1 beginbfchar <0001> <0041> endbfchar";
                objects.push((n + 3, Body::stream(dict! {}, cmap)));
            }
            objects.extend([
                (n, font.into()),
                (n + 1, cid_font.into()),
                (n + 2, descriptor.into()),
            ]);
            objects
        };
        let fonts = dict! { "F1" => r(4), "F2" => r(8) };
        let mut objects: Vec<(u32, Body)> = vec![
            (1, dict! { "Type" => "Catalog", "Pages" => r(2) }.into()),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }.into(),
            ),
            (
                3,
                dict! {
                    "Type" => "Page",
                    "Parent" => r(2),
                    "MediaBox" => numbers(&[0, 0, 612, 792]),
                    "Resources" => dict! { "Font" => fonts },
                }
                .into(),
            ),
        ];
        objects.extend(type0(4, true));
        objects.extend(type0(8, false));
        let file = fixture::file("1.7", &objects, dict! { "Root" => r(1) }, Xref::Table);
        let read = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let (hold, options) = (Hold::All(Made::Merged), Options::default());
        let (beside, alone) = (
            check(&read, &hold, &options),
            check(read.document(), &hold, &options),
        );
        let missing = |report: &Report| -> Vec<Option<ObjectId>> {
            let found = report.findings.iter().filter(|f| cap_height(f));
            found.map(|f| f.object).collect()
        };
        let latin = |report: &Report| -> Vec<(String, usize)> {
            let rows = report.unchecked.iter().filter(|(row, _)| latin_row(row));
            rows.map(|(row, &n)| (row.table.clone(), n)).collect()
        };
        let table = "FontDescriptorCIDType2".to_string();
        assert_eq!(missing(&beside), [Some((6, 0))]);
        assert_eq!(latin(&beside), [(table.clone(), 1)]);
        assert_eq!(missing(&alone), []);
        assert_eq!(latin(&alone), [(table, 2)]);
        streams_alone(&beside, &alone);
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

    /// With PDF_LINT_FUZZ_SEEDS naming a folder, writes the `write`
    /// target's seeds there ([`write_seeds`], `just fuzz write`).
    #[test]
    #[ignore = "needs PDF_LINT_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_write_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_LINT_FUZZ_SEEDS").expect("PDF_LINT_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (name, seed) in write_seeds() {
            std::fs::write(dir.join(format!("{name}.pdf")), seed).unwrap();
        }
    }

    /// The `write` target's seeds are the seeds, each with a third byte
    /// for each of its ways, the file after it as it was: so each file
    /// read whole is the seed's.
    #[test]
    fn the_write_seeds_are_the_seeds_with_a_third_byte() {
        let (seeds, written) = (seeds(), write_seeds());
        assert_eq!(written.len(), 10 * seeds.len());
        for ((name, seed), ways) in seeds.iter().zip(written.chunks(10)) {
            let thirds: Vec<u8> = ways.iter().map(|(_, w)| w[2]).collect();
            assert_eq!(thirds, [0, 1, 2, 3, 4, 0x80, 0x81, 0x82, 0x83, 0x84]);
            for (_, way) in ways {
                assert_eq!(way[..2], seed[..2], "{name}");
                assert_eq!(way[3..], seed[2..], "{name}");
                assert!(way[3..].starts_with(b"%PDF-"), "{name}");
            }
        }
    }
}
