# pdf-structure

PDF's file structure, read, in one Rust crate on [pdf-objects](https://crates.io/crates/pdf-objects): a file's header, its cross-reference tables and streams along their `/Prev` chain, and each object at the offset they give, every value, key and stream's data with where it was written (ISO 32000-2, §7.5). Object streams and cross-reference streams are decoded through [pdf-filters](https://crates.io/crates/pdf-filters), and an encrypted file is opened through [pdf-crypt](https://crates.io/crates/pdf-crypt).

It's made for a reader that checks files as much as for one that shows them. Where a file isn't as the spec writes it, it's read as far as a reader can take it, and every choice the reader made is said, with where.

## What it does

- **A file read whole** (`bytes::read`): the header, `startxref`, the cross-reference tables and streams along their `/Prev` chain, and each object at its offset, through pdf-objects' parser, a stream's data by its `/Length` when that ends at `endstream`. What it read is a `Document`: each object its cross-references list, or why it doesn't read; the trailer; the header's version; and the file's newest incremental update.
- **Or read on demand** (`Reading`): opened, the header, sections and trailer read and an encrypted file's security handler opened with the passwords given, which aren't kept; then each object read when it's first asked for, kept (`get`) or not (`object_unkept`), and each object stream decoded once, when an object in it is first read, from any thread; and the bytes at a span (`Reading::bytes`), in the file or in an object stream's decoded data, but for an encrypted file's as written, which may be ciphertext. Read in any order and made a `Document`, a reading is the whole read, the same objects, warnings in the same order and the same decoded data, so long as no stream it decodes reaches the reading's budget, which is spent in the order streams are decoded. The whole read is every object read so, in order, by the same code. A reading holds its bytes as a `ByteSource`: a slice, a `Vec<u8>` or an `Arc<[u8]>` in memory; or a file read by position (`FileBytes`, on Unix and Windows), never whole but where the reader needs it whole, to scan a file whose sections don't read.
- **A file read by position**, a window at a time: the header from the file's start, `startxref` and the last `%%EOF` back from its end, each section at its offset, and each object at its own, its first window reaching a byte past the next offset the cross-references list, so that an object as it should be is one read; a window doubled while what's read runs to its end where more of the file could change it, so that each read reads what it would of the whole file (pdf-objects' `Parser::window`). Any `ByteSource` that doesn't hold its bytes in memory is read so, from its `first_window`. Where the source fails, the call fails (`bytes::Error::Io`), and nothing read is kept, so asked again it reads again. A file's `Identity`, taken as it's opened by one `stat`, its length and modification time, and its device and inode on Unix or its creation time on Windows, tells whether the file held open has changed in place (`FileBytes::unchanged`), and whether another is at its path now (`Identity::of`).
- **A stream's data** (`Reading::data`), read each time it's asked for, and not kept: as written, filters and all, borrowed from bytes in memory; in an encrypted file that opened, decrypted as an object stream's is, by the stream's own `/Crypt` filter when it has one, else as `/StmF` says, but for a cross-reference stream's and the catalog's metadata stream's under `/EncryptMetadata false`, which are left as written, as the file has them. Where it may be ciphertext the reader doesn't decrypt, it's refused, saying why: in a file no password opened (but one whose streams aren't encrypted, `Encryption::Plain`), or whose security handler the reader can't open; an embedded file's in a file whose encryption dictionary has an `/EFF`; a stream the encryption dictionary refers to; and the catalog's metadata stream under `/EncryptMetadata false` whose own `/Crypt` filter encrypts it. Decoded (`Reading::decoded`), it's that through its filters, read as an object stream's are, by pdf-filters, to at most the reading's limit on a stream. Data that doesn't decrypt or decode cleanly is an error holding what did (`bytes::Error`).
- **Where each object was read from** (`Record`): at the offset a section gives, at one the scan found, or in an object stream's decoded data, the `index`th of its objects; its bytes, from its `n g obj` to past its `endobj`, and whether `endobj` follows it; and a stream's data, and whether it was found by its `/Length`, and whether that's a reference. And what the header and sections say of the file as written (`Facts`): where `%PDF-` starts, the newest section's offset and kind, how many sections, whether one is a hybrid's, the largest cross-reference stream's number, and whether a linearization holds (ISO 32000-2, Annex F).
- **Recovery by scanning.** Where the sections don't read, it finds the objects by scanning the file, and the trailer says why (`Trailer::unwritten`).
- **Every choice said.** What the reader had to choose how to read is a `Warning`, with its object and where: a stream whose `/Length` doesn't end at `endstream`, an object without `endobj`, a cross-reference table whose rows aren't the 20 bytes the spec writes, a file whose last line isn't `%%EOF` alone, and what pdf-objects' parser reads past.
- **Streams decoded.** Cross-reference streams and object streams are decoded from their own data, through pdf-filters: ASCIIHex, ASCII85, LZW, Flate and RunLength, with their predictors. Their `/Filter` and `/DecodeParms` are read as qpdf reads them: the abbreviations `/Fl`, `/AHx`, `/A85`, `/LZW` and `/RL` on any stream, `/Crypt` as no filter (decrypting comes before the filters, by its `/Name`), a parameters dictionary that isn't in an array every filter's, a reference to no object as null. A filter it doesn't decode, or parameters it doesn't take, leave the stream undecoded, saying why; data a fault stopped is read as far as it decoded, each fault a warning on the stream. The objects read from an object stream are placed in its decoded data (`Span::stream`), which the `Document` keeps.
- **Encrypted files opened** (`bytes::read_opened`) with the passwords given, each tried as the owner's and then the user's (none, the empty password), through pdf-crypt's standard security handler, revisions 2 to 6: object streams decrypted before they're decoded, and strings decrypted where they are, their bytes exact, but a signature's `/Contents` and the encryption dictionary's. A string or stream that doesn't decrypt cleanly is a warning. `Document::encryption` says whether the file is encrypted, and whether it opened, by the owner's password or the user's, is locked, or has a handler the reader can't open, and why; or reads whole all the same, as its strings and streams aren't encrypted (`/Identity`). `bytes::read` opens a file with the empty password. A password is tried as the bytes given; pdf-crypt's `passwords` gives the bytes a password typed as text may have been set as. What it decrypts and what it leaves as written is listed in `bytes`' documentation, "What's encrypted", each with the clause of ISO 32000-2 that says so and the tests that hold it.
- **A revision before an update** (`bytes::read_from`): the file as of a cross-reference section, along its `/Prev` chain as if it were the newest, so that what later updates wrote isn't in it.
- **Limits, one value a caller sets** (`Limits`, in a reading's `Options`): by default, a stream decodes to at most 256 MiB (`bytes::STREAM`), and the streams of a reading of a file to 1 GiB in all (`bytes::BUDGET`); at most 1,024 sections along `/Prev` (`bytes::SECTIONS`); no more cross-reference entries than the file has bytes, or 2²⁰ if that's more; and pdf-objects' limits on each object, values nested 256 deep (`bytes::NESTING`) and 2²⁵ values in one. A caller sets them on a copy of `Limits::DEFAULT`, as it does `Options`', which also say as of which section a file is read. They're the same in every build.
- **Typed errors.** Why an object doesn't read is an `Unread`: it isn't in the file, doesn't parse (pdf-objects' error), or isn't the object at its offset; or the object stream it's in doesn't decode (`bytes::Undecoded`, its filters `bytes::Unfiltered`), needs a password, or doesn't say where the object is (`bytes::Unlisted`); or, from another reader, that reader's words (`Unread::Other`). Each says why as text too. What a reader says of the file it read stays text, as any reader fills it: why the trailer isn't the newest section's (`Trailer::unwritten`), why a security handler doesn't open (`Encryption::Unsupported`), and each warning.
- **A `Source`,** the trait a reader of a file implements, which `Document` does: its objects by number, its trailer, header version and warnings, and what a reader may know besides, the file's size and the catalog's metadata when it's in plain view.

## An example

```rust
use pdf_structure::pdf_objects::Object;
use pdf_structure::{Source, bytes};

// Two objects, then a cross-reference table saying where each starts.
let mut file = b"%PDF-1.7\n".to_vec();
let catalog = file.len();
file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
let pages = file.len();
file.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
let xref = file.len();
let table = format!("0000000000 65535 f \n{catalog:010} 00000 n \n{pages:010} 00000 n \n");
let tail = format!("trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n");
file.extend_from_slice(format!("xref\n0 3\n{table}{tail}").as_bytes());
// A line after the last %%EOF, where none should be.
file.extend_from_slice(b"% more\n");

let doc = bytes::read(&file);
assert_eq!(doc.trailer.unwritten, None, "read by its table");

// An object by its number, and where it was written.
let pages = doc.get((2, 0)).unwrap();
let count = pages.as_dict().unwrap().get(b"Count").and_then(Object::as_integer);
assert_eq!(count, Some(0));
let at = pages.at.unwrap();
assert_eq!(&file[at.start..at.end], b"<< /Type /Pages /Kids [] /Count 0 >>");

// What the reader had to choose how to read, and where.
let [warning] = doc.warnings() else { panic!() };
assert_eq!(warning.message, "the file goes on for 8 bytes after its last %%EOF");
assert_eq!(warning.at.map(|at| &file[at.start..at.end]), Some(&b"\n% more\n"[..]));
```

The same file read on demand, from bytes shared across threads, an object read when it's asked for, and where it was read from:

```rust
use std::sync::Arc;

use pdf_structure::{Options, Origin, Reading};

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
    xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
    trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
let bytes: Arc<[u8]> = Arc::from(&file[..]);
let reading = Reading::open(bytes, &[], &Options::DEFAULT).unwrap();
assert_eq!(reading.ids().collect::<Vec<_>>(), [(1, 0)]);

// Read on another thread, and kept.
std::thread::scope(|scope| {
    scope.spawn(|| assert!(reading.get((1, 0)).is_ok()));
});
let record = reading.record((1, 0)).unwrap();
assert_eq!(record.origin, Origin::File);
assert_eq!(&file[record.extent.start..record.extent.end], b"1 0 obj\n<< /Type /Catalog >>\nendobj");

// Whole, it's the file read whole.
let doc = reading.into_document().unwrap();
assert_eq!(doc, pdf_structure::bytes::read(file));
```

A file on disk read by position, each object a window of it, and the file's identity, to tell whether it's changed since:

```rust,no_run
use std::fs::File;

use pdf_structure::{FileBytes, Options, Reading};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let file = FileBytes::new(File::open("file.pdf")?)?;
let identity = file.identity();
let reading = Reading::open(file, &[], &Options::DEFAULT)?;
let catalog = reading.trailer().dict().get(b"Root").and_then(|root| root.as_reference());
if let Some(catalog) = catalog {
    println!("{:?}", reading.get(catalog)?);
}
// Later, before writing to the path: is the file there the one read?
let same = pdf_structure::Identity::of("file.pdf")? == identity;
println!("the same file: {same}");
# Ok(())
# }
```

A file without `startxref` is scanned for its objects, and the trailer says so:

```rust
use pdf_structure::bytes;

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
let doc = bytes::read(file);
assert!(doc.objects[&(1, 0)].is_ok());
assert_eq!(doc.trailer.unwritten.as_deref(), Some("no startxref"));
```

## What isn't here

- **Writing.** It reads only. The `fixtures` feature writes files from objects, and runs qpdf (the program `PDF_LINT_QPDF` names, else `qpdf` on the `PATH`) as a second reader, hidden from the docs: for this workspace's tests and fuzz targets, not for a user. Without it, nothing runs a program or reads `PDF_LINT_QPDF`.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
