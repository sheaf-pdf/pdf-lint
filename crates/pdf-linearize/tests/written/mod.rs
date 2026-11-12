//! Small files written by hand, for the family adapter to lay out: what its own
//! tests and those against qpdf go through.

#![allow(dead_code)]

use pdf_linearize::family::Options;
use pdf_linearize::pdf_objects::ObjectId;
use pdf_structure::XrefKind;
use pdf_structure::pdf_crypt::Password;
use pdf_structure::read::{self, Reading};
use pdf_structure::write::{self, Encryptor};

/// The keys of a file qpdf encrypted, each with its empty user password: one of
/// each of the standard handler's revisions, `qpdf --encrypt` on a small clean
/// file.
pub const KEYS: [(&str, &[u8]); 5] = [
    ("RC4-40", include_bytes!("../files/qpdf-rc4-40.pdf")),
    ("RC4-128", include_bytes!("../files/qpdf-rc4-128.pdf")),
    ("AES-128", include_bytes!("../files/qpdf-aes-128.pdf")),
    ("AES-256 R5", include_bytes!("../files/qpdf-aes-256-r5.pdf")),
    ("AES-256", include_bytes!("../files/qpdf-aes-256.pdf")),
];

/// The reading of `bytes`, by the empty password or `password`.
pub fn opened<'b>(bytes: &'b [u8], password: &str) -> Reading<&'b [u8]> {
    let given = [Password(password)];
    let mut options = read::Options::DEFAULT;
    if !password.is_empty() {
        options.passwords = &given;
    }
    Reading::open(bytes, &options).unwrap()
}

/// `plain`'s objects as a whole file under `key`'s encryption, its dictionary
/// at 1000, so every number from the last object's to it is listed free.
pub fn encrypted_as(key: &[u8], plain: &[u8]) -> Vec<u8> {
    let key = opened(key, "");
    let encryptor = Encryptor::of(&key).unwrap().unwrap();
    let reading = opened(plain, "");
    let doc = reading.to_document().unwrap();
    let mut options = write::Options::DEFAULT;
    options.encrypt_at = Some(ObjectId::new(1000, 0));
    let mut out = Vec::new();
    write::document(&mut out, &doc, Some(&encryptor), Some(&reading), &options).unwrap();
    out
}

/// A file of `objects`, numbered from 1, under a table, with `trailer` beside
/// its `/Size`.
pub fn file(objects: &[&str], trailer: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.6\n".to_vec();
    let mut rows = Vec::new();
    for (number, object) in (1..).zip(objects) {
        rows.push(out.len());
        out.extend_from_slice(format!("{number} 0 obj\n{object}\nendobj\n").as_bytes());
    }
    let at = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for row in rows {
        out.extend_from_slice(format!("{row:010} 00000 n \n").as_bytes());
    }
    let size = objects.len() + 1;
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} {trailer} >>\nstartxref\n{at}\n%%EOF\n").as_bytes(),
    );
    out
}

/// `file` with `entries` added to its trailer after its `/Root`, in the clear
/// as a trailer is: nothing before the trailer moves.
pub fn with_info(file: &[u8], entries: &str) -> Vec<u8> {
    let at = file.windows(7).rposition(|w| w == b"trailer").unwrap();
    let root = at
        + file[at..]
            .windows(11)
            .position(|w| w == b"/Root 1 0 R")
            .unwrap()
        + 11;
    let mut out = file[..root].to_vec();
    out.extend_from_slice(format!(" {entries}").as_bytes());
    out.extend_from_slice(&file[root..]);
    out
}

/// A stream object of `dict`'s entries and `data`.
pub fn stream(dict: &str, data: &str) -> String {
    let length = data.len();
    format!("<< {dict} /Length {length} >>\nstream\n{data}\nendstream")
}

pub const ROOT: &str = "/Root 1 0 R";

/// A catalog of `more`, a root node, and pages 3 and 4, each with a stream of
/// its own (5, 6), an object they share (7), and an inherited box and
/// resources.
pub fn two_pages(more: &str) -> Vec<String> {
    vec![
        format!("<< /Type /Catalog /Pages 2 0 R {more} >>"),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 100 200] /Resources 8 0 R >>"
            .to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".to_string(),
        "<<  /Type /Page /Parent 2 0 R /Contents 6 0 R   /MediaBox [0 0 30 40] /Resources 8 0 R  >>"
            .to_string(),
        stream("/Mark (first content)", "0 0 m"),
        stream("/Mark (second content)", "1 1 m"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Mark (font) >>".to_string(),
        "<< /Font << /F0 7 0 R >> /Mark (resources) >>".to_string(),
    ]
}

pub fn built(objects: &[String]) -> Vec<u8> {
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    file(&objects, ROOT)
}

/// The three kinds: a table, a stream, a stream with object streams.
pub fn kinds() -> [Options<'static>; 3] {
    let stream = {
        let mut options = Options::DEFAULT;
        options.kind = XrefKind::Stream;
        options
    };
    let packed = {
        let mut options = stream;
        options.pack = true;
        options
    };
    [Options::DEFAULT, stream, packed]
}
