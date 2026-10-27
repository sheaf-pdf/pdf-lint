//! What the fuzz target checks, which a unit test checks too: on any
//! bytes, a file is read and checked without a panic, in the same way
//! twice, with every finding's bytes inside what was read; and set against
//! its previous revision, when it has an incremental update. It's
//! compiled only for tests and under cargo-fuzz's `--cfg fuzzing`, with
//! [`seeds`], files for the fuzzer to begin from.

use std::collections::BTreeSet;

use crate::bytes;
use crate::check::{Exempt, Hold, Made, Options, UnlistedIn, check};
use crate::object::Span;
use crate::source::{Document, Source};
use crate::version::Version;
use pdf_crypt::writer::{Kind, Settings};

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
        let previous = bytes::read_from(file, update.previous);
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
        ("object-streams", object_streams(true)),
        ("object-streams-lzw", object_streams(false)),
        ("update", update()),
        (
            "encrypted",
            file(&encrypted, &format!("/Encrypt 4 0 R /ID [{id} {id}]")).0,
        ),
        ("encrypted-rc4-40", opens(Kind::Rc4By40, false)),
        ("encrypted-rc4-128", opens(Kind::Rc4By128, false)),
        ("encrypted-aes-128", opens(Kind::Aes128, false)),
        ("encrypted-aes-256-r5", opens(Kind::Aes256R5, false)),
        ("encrypted-aes-256", opens(Kind::Aes256, false)),
        ("encrypted-own-crypt", opens(Kind::Aes128, true)),
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
            let objects = crate::write::inflated(DEFLATED);
            let hex: String = crate::write::lzw(&objects)
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
        true => (crate::write::zlib(&predicted), " /Filter /FlateDecode"),
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

/// A file encrypted as `kind` with the empty user password, so that it
/// opens: its catalog's /Lang encrypted, its page tree and page in an
/// object stream compressed and encrypted, by the file's crypt filter for
/// streams or, with `own`, by its own /Crypt filter (/StmF /Identity).
fn opens(kind: Kind, own: bool) -> Vec<u8> {
    let id = b"0123456789abcdef";
    let settings = Settings {
        kind,
        user: b"",
        owner: b"owner",
        p: -4,
        id,
        encrypt_metadata: true,
    };
    let values = pdf_crypt::writer::dictionary(&settings);
    let handler = pdf_crypt::Handler::open(&values, id, &[]).expect("the writer's opens");
    let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let lang =
        pdf_crypt::writer::encrypt_string(&handler, (1, 0), b"en", [1; 16]).unwrap_or_default();
    let objects = "2 0 3 42 << /Type /Pages /Kids [3 0 R] /Count 1 >>\n\
                   << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (In plain view) >>] >>";
    let first = objects.find("<<").unwrap_or(0);
    let deflated = crate::write::zlib(objects.as_bytes());
    let crypt = own.then(|| handler.named(b"StdCF"));
    let data =
        pdf_crypt::writer::encrypt_stream(&handler, (4, 0), &deflated, crypt.as_ref(), [4; 16])
            .unwrap_or_default();
    let filter = match own {
        true => "/Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /StdCF >> null]",
        false => "/Filter /FlateDecode",
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
        let streams = if own { "/Identity" } else { "/StdCF" };
        entries.push_str(&format!(
            " /CF << /StdCF << /CFM /{method} >> >> /StmF {streams} /StrF /StdCF"
        ));
    }
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
        // Those that open, open, and their object stream reads.
        for (name, seed) in seeds()
            .into_iter()
            .filter(|(n, _)| n.starts_with("encrypted-"))
        {
            let doc = bytes::read(&seed[2..]);
            let opened = crate::source::Encryption::Opened { owner: false };
            assert_eq!(doc.encryption, opened, "{name}");
            assert_eq!(
                doc.get((3, 0)).map(|o| o.at.and_then(|at| at.stream)),
                Ok(Some((4, 0))),
                "{name}"
            );
            let lang = doc
                .get((1, 0))
                .ok()
                .and_then(|o| o.as_dict()?.get(b"Lang").cloned());
            let en = crate::object::Value::String(crate::object::Str::Bytes(b"en".to_vec()));
            assert_eq!(lang.map(|l| l.value), Some(en), "{name}");
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
