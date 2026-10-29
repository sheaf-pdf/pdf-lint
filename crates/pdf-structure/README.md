# pdf-structure

PDF's file structure, read, in one Rust crate on [pdf-objects](https://crates.io/crates/pdf-objects): a file's header, its cross-reference tables and streams along their `/Prev` chain, and each object at the offset they give, every value, key and stream's data with where it was written (ISO 32000-2, §7.5); and what's read, a document, which a caller can build in memory too, to number new objects in, edit, walk and prune. And it writes a file of numbered objects, whole or as an incremental update after the file it was read from, and what a file's structure is written in: cross-reference sections, as tables and streams, and object streams; an encrypted file's objects encrypted as they're written, by the list of what's left in the clear that its reader decrypts by. Object streams and cross-reference streams are decoded through [pdf-filters](https://crates.io/crates/pdf-filters), and an encrypted file is opened through [pdf-crypt](https://crates.io/crates/pdf-crypt).

It's made for a reader that checks files as much as for one that shows them. Where a file isn't as the spec writes it, it's read as far as a reader can take it, and every choice the reader made is said, with where.

## What it does

- **A file read whole** (`bytes::read`): the header, `startxref`, the cross-reference tables and streams along their `/Prev` chain, and each object at its offset, through pdf-objects' parser, a stream's data by its `/Length` when that ends at `endstream`. What it read is a `Document`: each object its cross-references list, or why it doesn't read; the trailer; the header's version; and the file's newest incremental update. Its objects are read by number (`Source::get`), in order or in a range of numbers (`objects`, `range`), beside those that don't read (`unread`), and put in, taken to edit and removed (`set`, `get_mut`, `remove`).
- **Or read on demand** (`Reading`): opened, the header, sections and trailer read and an encrypted file's security handler opened by the empty password (`open`) or by the passwords given (`open_with`), which aren't kept, or by another once it's open (`unlock`); then each object read when it's first asked for, kept (`get`) or not (`object_unkept`), and each object stream decoded once, when an object in it is first read, from any thread; and the bytes at a span (`Reading::bytes`), in the file or in an object stream's decoded data, but for an encrypted file's as written, which may be ciphertext. Read in any order and made a `Document`, a reading is the whole read, the same objects, warnings in the same order and the same decoded data, so long as no stream it decodes reaches the reading's budget, which is spent in the order streams are decoded. The whole read is every object read so, in order, by the same code. Made a `Document`, the reading is taken (`into_document`) or kept beside it (`to_document`), what it kept cloned and the rest read without keeping it, its object streams decoded and kept: then it hands out what the document doesn't hold, a stream's data, which a document has only where it was written, and the bytes at a span. A reading holds its bytes as a `ByteSource`: a slice, a `Vec<u8>` or an `Arc<[u8]>` in memory; or a file read by position (`FileBytes`, on Unix and Windows), never whole but where the reader needs it whole, to scan a file whose sections don't read; or any of them behind a reference, a `Box` or an `Arc`, a `dyn ByteSource` among them. It gives its source back (`Reading::source`, `into_source`), and each reading has an id of its own (`Reading::id`, a `SourceId`).
- **A file read by position**, a window at a time: the header from the file's start, `startxref` and the last `%%EOF` back from its end, each section at its offset, and each object at its own, its first window reaching a byte past the next offset the cross-references list, so that an object as it should be is one read; a window doubled while what's read runs to its end where more of the file could change it, so that each read reads what it would of the whole file (pdf-objects' `Parser::window`). Any `ByteSource` that doesn't hold its bytes in memory is read so, from its `first_window`. Where the source fails, the call fails (`bytes::Error::Io`), keeping nothing of what it was reading, so asked again it reads again; an object it read whole on the way, as the object stream an object is in, is kept, as `get` keeps it. A file's `Identity`, taken as it's opened, the file itself by a handle it holds open (its device and inode on Unix, its volume serial number and file index on Windows, through same-file) and its length and modification time, tells whether the file held open has changed in place (`FileBytes::unchanged`), and whether another is at its path now (`Identity::of`), even one made where the first was removed. `Identity::of` opens the file to read, without waiting, so it needs leave to read it, and holds it open while it lives; a path that's neither a file nor a folder, a FIFO, a socket or a device, it refuses without opening it.
- **A stream's data** (`Reading::data`), read each time it's asked for, and not kept: as written, filters and all, borrowed from bytes in memory; in an encrypted file that opened, decrypted as an object stream's is, by the stream's own `/Crypt` filter when it has one, else as `/StmF` says, and an embedded file's without one of its own by the crypt filter the encryption dictionary's `/EFF` names, where it names one (from `/V` 4); but for a cross-reference stream's, and the catalog's metadata stream's under `/EncryptMetadata false` unless its own `/Crypt` filter encrypts it, which are left as written, as the file has them. Where it may be ciphertext the reader doesn't decrypt, it's refused, saying why: in a file no password opened (but one whose streams aren't encrypted, `Encryption::Plain`, where a stream's own crypt filter, or an embedded file's `/EFF`, is all that can encrypt it), or whose security handler the reader can't open; a stream the encryption dictionary refers to; and an embedded file's without a crypt filter of its own where `/EFF` isn't a name, which doesn't say how. Decoded (`Reading::decoded`), it's that through its filters, read as an object stream's are, by pdf-filters, to at most the reading's limit on a stream. Data that doesn't decrypt or decode cleanly is an error holding what did (`bytes::Error`).
- **Where each object was read from** (`Record`): in which reading (its `SourceId`, so a record is known for the file its places are in, though two readings of one file read alike); at the offset a section gives, at one the scan found, or in an object stream's decoded data, the `index`th of its objects; its bytes, from its `n g obj` to past its `endobj`, and whether `endobj` follows it; a stream's data (`DataExtent`), and whether it was found by its `/Length`, and whether that's a reference; and a hash of its bytes as read, private, which the writer checks a copy of them by. And what the header and sections say of the file as written (`Facts`): where `%PDF-` starts, the newest section's offset and kind, how many sections, whether one is a hybrid's, the largest cross-reference stream's number, and whether a linearization holds (ISO 32000-2, Annex F). A `Document` keeps each object with its record while the object is as read (`Document::record`): one put in has none, and one taken to edit loses it, so a record is always of the object beside it.
- **A document in memory** (`Document::new`, or a file read whole): added to and edited, its new objects numbered past every number the file's cross-references list, in use or free, its cross-reference streams' and the trailer's `/Size` less one (`new_id`, `add`; where they start, `max_id` and `set_max_id`), never 0, and once those reach 4,294,967,295, the largest number, at numbers no entry holds. References followed to where their chain ends (`dereference`, as many as 128 of them; `get_dict` and `get_dict_mut`, a stream's dictionary too; `catalog` and `catalog_mut`), and the trailer's dictionary changed where it is (`Trailer::dict_mut`). What the trailer reaches walked, each once, in the order found (`reachable`), every entry of a key written twice among it; and what it doesn't removed (`prune`): the object streams and cross-reference streams, which nothing refers to, among them, the encryption dictionary, which the trailer does, kept, and the decoded data left as it is.
- **Recovery by scanning.** Where the sections don't read, it finds the objects by scanning the file, and the trailer says why (`Trailer::unwritten`).
- **Every choice said.** What the reader had to choose how to read is a `Warning`, with its object and where: a stream whose `/Length` doesn't end at `endstream`, an object without `endobj`, a cross-reference table whose rows aren't the 20 bytes the spec writes, a file whose last line isn't `%%EOF` alone, and what pdf-objects' parser reads past.
- **Streams decoded.** Cross-reference streams and object streams are decoded from their own data, through pdf-filters: ASCIIHex, ASCII85, LZW, Flate and RunLength, with their predictors, Flate inflated by the backend `Options::flate` names (miniz_oxide by default; zlib-rs with this crate's `zlib-rs` feature). Their `/Filter` and `/DecodeParms` are read as qpdf reads them: the abbreviations `/Fl`, `/AHx`, `/A85`, `/LZW` and `/RL` on any stream, `/Crypt` as no filter (decrypting comes before the filters, by its `/Name`), a parameters dictionary that isn't in an array every filter's, a reference to no object as null. A filter it doesn't decode, or parameters it doesn't take, leave the stream undecoded, saying why; data a fault stopped is read as far as it decoded, each fault a warning on the stream. The objects read from an object stream are placed in its decoded data (`Span::stream`), which the `Document` keeps.
- **Encrypted files opened** (`Reading::open_with`, `bytes::read_opened`) with the passwords given, each a `Password` as typed, tried in each of its encodings, as the owner's and then the user's, every password's that fit the file's revision before any password's others (pdf-crypt's `Password::encodings` and `Handler::open`), in the empty password's place: given none, the empty password, and a wrong one leaves the file locked, or plain where its strings and streams aren't encrypted, as no password would. A reading opened by the empty password (`Reading::open`) and left locked is opened by another once it's open (`Reading::unlock`, with the `Encoded` bytes to try), without reading the file again: what it read is dropped, to be read again decrypted, and a password that doesn't open it gives it back as it was, with why. Through pdf-crypt's standard security handler, revisions 2 to 6: object streams decrypted before they're decoded, and strings decrypted where they are, their bytes exact, but a signature's `/Contents` and the encryption dictionary's. A string or stream that doesn't decrypt cleanly is a warning. `Document::encryption` says whether the file is encrypted, and whether it opened, by the owner's password or the user's, and which of those given in which encoding (`by`), is locked, or has a handler the reader can't open, and why; or reads whole all the same, as its strings and streams aren't encrypted (`/Identity`). `bytes::read` opens a file with the empty password. pdf-crypt is re-exported (`pdf_crypt`), and its `Password`, `Encoded` and `Encoding` at the root. What it decrypts and what it leaves as written is listed in `bytes`' documentation, "What's encrypted", each with the clause of ISO 32000-2 that says so and the tests that hold it.
- **A revision before an update** (`bytes::read_from`): the file as of a cross-reference section, along its `/Prev` chain as if it were the newest, so that what later updates wrote isn't in it.
- **Whole files and updates written** (`write`): a document's objects, or any numbered objects, as a whole file (`write::full`, `write::document`) or as an incremental update after the file they were read from (`write::update`), encrypted as that file is or under a new encryption, or decrypted; an object as it was read copied as the bytes it was read from where they're right to copy, checked against a hash of them taken as it was read; and the parts a file is laid out in, cross-reference sections and object streams, for a caller laying a file out itself; below, "Writing".
- **Limits, one value a caller sets** (`Limits`, in a reading's `Options`): by default, a stream decodes to at most 256 MiB (`bytes::STREAM`), and the streams of a reading of a file to 1 GiB in all (`bytes::BUDGET`); at most 1,024 sections along `/Prev` (`bytes::SECTIONS`); no more cross-reference entries than the file has bytes, or 2²⁰ if that's more; and pdf-objects' limits on each object, values nested 256 deep (`bytes::NESTING`) and 2²⁵ values in one. A caller sets them on a copy of `Limits::DEFAULT`, as it does `Options`', which also say as of which section a file is read, and which Flate backend its streams inflate on (`Options::flate`). They're the same in every build.
- **Typed errors.** Why an object doesn't read is an `Unread`: it isn't in the file, doesn't parse (pdf-objects' error), or isn't the object at its offset; or the object stream it's in doesn't decode (`bytes::Undecoded`, its filters `bytes::Unfiltered`), needs a password, or doesn't say where the object is (`bytes::Unindexed`); or, from another reader, that reader's words (`Unread::Other`). Each says why as text too. A reading's call fails with a `bytes::Error` (`Error` at the root), which may hold pdf-filters' error, re-exported as `pdf_filters`, or pdf-crypt's. What a reader says of the file it read stays text, as any reader fills it: why the trailer isn't the newest section's (`Trailer::unwritten`), why a security handler doesn't open (`Encryption::Unsupported`), and each warning.
- **A `Source`,** the trait a reader of a file implements, which `Document` does: its objects by number, its trailer, header version and warnings, and what a reader may know besides, the file's size and the catalog's metadata when it's in plain view. A `Document` and a `Reading`, by reference, are pdf-objects' `Resolve` too, which what follows references reads through, across threads too: a reference resolves to its object, read and kept as `get` keeps it, or to none, null, where the file hasn't it, it doesn't read, or the source fails the read, without saying why.

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
let reading = Reading::open(bytes, &Options::DEFAULT).unwrap();
assert_eq!(reading.ids().collect::<Vec<_>>(), [(1, 0)]);

// Read on another thread, and kept.
std::thread::scope(|scope| {
    scope.spawn(|| assert!(reading.get((1, 0)).is_ok()));
});
let record = reading.record((1, 0)).unwrap();
assert_eq!(record.origin, Origin::File);
assert_eq!(&file[record.at.start..record.at.end], b"1 0 obj\n<< /Type /Catalog >>\nendobj");

// Whole, it's the file read whole: beside the reading, which hands out
// what the document doesn't hold, or taking it.
let doc = reading.to_document().unwrap();
assert_eq!(doc, pdf_structure::bytes::read(file));
assert_eq!(reading.into_document().unwrap(), doc);
```

A file on disk read by position, each object a window of it, and the file's identity, to tell whether it's changed since:

```rust,no_run
use std::fs::File;

use pdf_structure::{FileBytes, Options, Reading};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let file = FileBytes::new(File::open("file.pdf")?)?;
let identity = file.identity().clone();
let reading = Reading::open(file, &Options::DEFAULT)?;
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

A document built in memory, followed from its catalog, and pruned of what its trailer doesn't reach:

```rust
use pdf_structure::pdf_objects::{Object, dict, version::Version};
use pdf_structure::{Document, Source};

let mut doc = Document::new(Version::new(1, 7), dict! { "Size" => 1 });
let kids: Vec<Object> = Vec::new();
let pages = doc.add(dict! { "Type" => "Pages", "Kids" => kids, "Count" => 0 });
let catalog = doc.add(dict! { "Type" => "Catalog", "Pages" => pages });
let font = doc.add(dict! { "Type" => "Font", "Subtype" => "Type1" });
assert_eq!((pages, catalog, font), ((1, 0), (2, 0), (3, 0)));
doc.trailer.dict_mut().set("Root", catalog);

// The catalog, and the pages its /Pages refers to.
let root = doc.catalog().unwrap();
let (id, tree) = doc.dereference(root.get(b"Pages").unwrap()).unwrap();
assert_eq!(id, Some(pages));
assert_eq!(tree.as_dict().and_then(|d| d.type_name()), Some(&b"Pages"[..]));

// What the trailer reaches, and the rest removed.
assert_eq!(doc.reachable(), [catalog, pages]);
assert_eq!(doc.prune(), [font]);
assert_eq!(doc.ids(), [pages, catalog]);
assert_eq!(doc.new_id(), (4, 0));
```

A file without `startxref` is scanned for its objects, and the trailer says so:

```rust
use pdf_structure::{Source, bytes};

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
let doc = bytes::read(file);
assert!(doc.contains((1, 0)));
assert!(doc.get((1, 0)).is_ok());
assert_eq!(doc.trailer.unwritten.as_deref(), Some("no startxref"));
```

## Writing

`write` writes numbered objects as a whole file (`write::full`), a document's objects among them (`write::document`, but object streams, cross-reference streams and a linearization dictionary, which it writes anew or would no longer hold), or as an incremental update after a file (`write::update`), at the end of a caller's `Vec<u8>`, positions counted from where the write starts in it. A whole file is the header, each object at its number in the order given, the encryption dictionary, and one cross-reference section, a table or a stream (`write::Options`, set on a copy of `Options::DEFAULT`); asked to (`Options::pack`), what may go in an object stream goes in one, grouped as the caller says (`Pack::groups`) or by number, a hundred each, numbered from the numbers the caller gives first (`Pack::reuse`), built on threads, the same bytes on any number. Its trailer is the caller's but for the keys a section writes itself, and its `/ID` keeps its first string and makes the second XXH3-128 of what it wrote (through twox-hash), so the same objects write the same bytes. An update goes after a file as a `Base` describes it, a reading's (`Base::of`, refused for a file with bytes before its header, whose positions this reader counts from the file's start and PDFium, pdf.js and qpdf from the header, a file read by scanning, one without `/Root`, or a reading as of an earlier revision) or one the caller describes (`Base::new`, its fields set after, a header past bytes before `%PDF-` among them): each object at its number, then a section of the file's kind listing only those, its `/Prev` the file's last section, its trailer the file's newest trailer's entries carried (ISO 32000-2, §7.5.6). Each write gives back what it wrote (`Written`): where each object landed, and the file as it now is, so a second update goes after it without reading it again, and `Base::holds` tells whether a file is still the one a base describes. A stream whose data is where the reader read it (`Data::At`) has it read from the reading given (`&dyn ReadFrom`, a `Reading`), only at its own number there, decrypted and encrypted again as the write is. Under an encryptor, each object is encrypted under the number it's written at, an object stream whole; an update is encrypted as its file is, by that file's encryptor, and refuses another. Where the file's keys are made per object (RC4, AES-128), an object at a number it lists free is refused, as its key would be a deleted object's. What's refused writes nothing (`write::Error`): a number 0 or past `NUMBERS`, or given twice, among it, and a write a flag stops (`Options::stop`). A trailer isn't encrypted (§7.6.2): its values are written as they are, a direct `/Info` dictionary's strings in the clear in an encrypted file, as in the file it was read from.

An object as it was read (`write::Body::Read`, its value and its `Record`; `write::bodies` gives a document's so, and `write::document` writes them) is copied as the bytes it was read from, read again from the reading given, where they're right to copy: of that reading (`Record::source`), at its own number, not found by scanning, `endobj` after it, a stream's `/Length` a number and its data as long; read at an offset, under that reading's own encryption (`Encryptor::of`), the list of what's in the clear saying the same of it in the file written, or with neither encrypted; read from an object stream, into an object stream of the write's, under any encryption or none, as its bytes there are plaintext. Else it's written anew from its value, and so it always is with no reading given. A copy's bytes are checked first against a hash of them taken as the object was read (XXH3-64, through twox-hash), and a write given a reading first asks its source whether its bytes changed (`ByteSource::unchanged`: for a `FileBytes`, the file's length and modification time): where they did, or a record is another reading's or another object's, the write is refused (`write::Error::Changed`, `Elsewhere`). The hash tells bytes changed by accident or by another program, not by one who means them to hash alike, and nothing here closes the race with a writer between the check and an append. An update after a reading's file passes over each object as that reading read it, which the file holds, so a document's bodies write what was built or changed since, and a second update after the first's `Written::base` does too.

```rust
use pdf_structure::pdf_objects::{Object, dict};
use pdf_structure::write::{self, Base, Body, Pack, ReadFrom};
use pdf_structure::{Options, Reading, Source, XrefKind};

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
    xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
    trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
let from: &dyn ReadFrom = &reading;
let mut doc = reading.to_document().unwrap();
doc.catalog_mut().unwrap().set("Lang", Object::string("en"));
let lang = |reading: &Reading<&[u8]>| {
    let catalog = reading.get((1, 0)).unwrap().as_dict().unwrap();
    catalog.get(b"Lang").and_then(Object::as_bytes).map(<[u8]>::to_vec)
};

// Whole, with a cross-reference stream, what may go in an object stream
// in one.
let mut options = write::Options::DEFAULT;
options.kind = XrefKind::Stream;
options.pack = Some(Pack::DEFAULT);
let mut whole = Vec::new();
write::document(&mut whole, &doc, None, Some(from), &options).unwrap();
let back = Reading::open(&whole[..], &Options::DEFAULT).unwrap();
assert!(back.warnings().is_empty());
assert_eq!(lang(&back).as_deref(), Some(&b"en"[..]));

// As an update after the file: what changed, its catalog alone, after its
// last section, the rest, as read, passed over.
let base = Base::of(&reading).unwrap();
let mut updated = file.to_vec();
let defaults = write::Options::DEFAULT;
let written = write::update(&mut updated, &base, write::bodies(&doc), None, Some(from), &defaults)
    .unwrap()
    .unwrap();
assert!(written.section.lists(1));
let back = Reading::open(&updated[..], &Options::DEFAULT).unwrap();
assert_eq!(back.facts().sections, 2);
assert_eq!(lang(&back).as_deref(), Some(&b"en"[..]));
assert!(written.base.holds(&updated[..]).unwrap());

// A second, after the file as a caller read it otherwise: its length, its
// last section and kind, its next number and its trailer.
let trailer = written.base.trailer.clone();
let (len, size) = (updated.len(), written.size);
let mut read = Base::new(len, written.start, size, XrefKind::Table, trailer);
read.ends_line = true;
let again = Object::from(dict! { "Type" => "Catalog", "Lang" => Object::string("fr") });
let catalog = [((1, 0), Body::from(&again))];
write::update(&mut updated, &read, catalog, None, None, &defaults).unwrap();
let back = Reading::open(&updated[..], &Options::DEFAULT).unwrap();
assert_eq!(back.facts().sections, 3);
assert_eq!(lang(&back).as_deref(), Some(&b"fr"[..]));
```

For a caller that lays the file out itself, `write` writes what a file's structure is written in: its objects are written by pdf-objects' writer (`serialize::indirect`), each where the output has got to, and a `Section` lists each at that position. A whole file's section (`Section::full`) is one subsection from 0, every number it doesn't list written free, the free entries linked in a list from entry 0, each at the generation its number is to be used at next (ISO 32000-2, §7.5.4); an update's (`Section::append`) lists only what it's given, in runs (§7.5.6). Either is written as a table (`Section::table`), which refuses an object in an object stream and an offset past ten digits, or as a stream (`Section::stream`), its `/W` as narrow as its widest field, a whole file's deflated under PNG's Up predictor, as qpdf writes its own; `Section::get` gives back what it lists each number as. Positions are counted from the header, as PDFium, pdf.js and qpdf read them, past any bytes before `%PDF-`, where ISO 32000-2 counts them from the file's start (§7.5.4), as this crate's reader does: the two differ only for an update of a file with bytes before its header, whose sections this reader reads only by scanning.

`packable` says whether an object may go in an object stream (§7.5.7): not a stream, an object whose generation isn't 0, a dictionary shaped as the standard security handler's encryption dictionary (a `/Filter` name, `/O` and `/U`), the linearization dictionary, or a signature dictionary, so that a signing tool finds it where it looks. `container` makes an object stream of objects written direct, a header saying where each starts, deflated; none where its data would be past what the reader decodes a stream to (`MAX_DATA`, 256 MiB, `Limits::stream`'s default). A section lists each object in it as `Entry::Packed`, which only a stream can. Both deflate through pdf-filters, by the backend and at the level a `Deflate` says: by default (`Deflate::DEFAULT`) miniz_oxide at 6, which every build has; zlib-rs with the `zlib-rs` feature. A written file's objects are numbered below `NUMBERS`, 8,388,607, the most a file may hold (ISO 32000-1, Annex C), which the caller keeps to. What can't be written is refused, and nothing is written (`write::Error`): a value pdf-objects' writer refuses among it.

```rust
use pdf_structure::pdf_objects::{Object, dict, serialize};
use pdf_structure::write::{self, Deflate, Entry, Section};
use pdf_structure::{Source, bytes};

// A catalog and its page tree, each written where the file has got to.
let mut file = b"%PDF-1.7\n".to_vec();
let mut section = Section::full(0);
let kids: Vec<Object> = Vec::new();
let objects = [
    (1, Object::from(dict! { "Type" => "Catalog", "Pages" => (2, 0) })),
    (2, Object::from(dict! { "Type" => "Pages", "Kids" => kids, "Count" => 0 })),
];
for (num, obj) in &objects {
    let pos = serialize::indirect(&mut file, (*num, 0), obj).unwrap();
    section.insert(*num, Entry::InUse { pos, generation: 0 });
}
// Two objects in an object stream, object 5, each listed as its place there.
let members = [(3, b"(three)".to_vec()), (4, b"<</Four 4>>".to_vec())];
let container = write::container(&members, Deflate::DEFAULT).unwrap().unwrap();
let pos = serialize::indirect(&mut file, (5, 0), &Object::from(container)).unwrap();
section.insert(5, Entry::InUse { pos, generation: 0 });
for (index, (num, _)) in (0..).zip(&members) {
    section.insert(*num, Entry::Packed { container: 5, index });
}
// The section as a stream, object 6, the trailer its dictionary.
let pos = file.len();
section.stream(&mut file, 6, dict! { "Root" => (1, 0) }, pos, Deflate::DEFAULT).unwrap();

let doc = bytes::read(&file);
assert_eq!(doc.trailer.unwritten, None, "read by its section");
assert!(doc.warnings().is_empty());
let four = doc.get((4, 0)).unwrap().as_dict().unwrap();
assert_eq!(four.get(b"Four").and_then(Object::as_integer), Some(4));
```

An encrypted file's objects are encrypted as they're written. An `Encryptor` is how a file is encrypted: a reading's (`Encryptor::of`, its security handler as a password opened it, its encryption dictionary as the file stores it, at its number or direct in the trailer, and the `/ID[0]` its key is made from; refused for a file no password opened, `write::Error::Locked`, or whose handler the reader can't open, `Unsupported`), or a new file's of revision 6 (`Encryptor::create`, AES-256, from pdf-crypt's `Handler::create` and the caller's generator). Its `encrypting`, given the file's trailer and what resolves its references, is an `Encrypting` (refused where `/EncryptMetadata false` leaves the metadata stream in the clear and the catalog, which says which that is, doesn't resolve), pdf-objects' `serialize::Encrypt`: written through `serialize::indirect_with` and `indirect_stream_with`, every string of an object and every stream's data is encrypted with the object's key, AES's IV derived from `n g obj` and the data, so the same object encrypts to the same bytes on every save and no IV serves two plaintexts, but what the list leaves in the clear, which the reader decrypts by too, one list both ways (`bytes`' docs, "What's encrypted"): the encryption dictionaries and what they refer to, by number, a cross-reference stream, a signature's `/Contents`, a stream whose crypt filter is `/Identity`, the document's metadata stream's data under `/EncryptMetadata false`; an embedded file's data by `/EFF`, a stream's by its own crypt filter. A container is encrypted whole, as a stream, and `Encryptor::packs` says none is made where streams are in the clear and strings aren't. What can't be encrypted is refused, with nothing written in the clear (`write::Unencryptable`, `write::Error::Encrypt`). The encryption dictionary itself, and the trailer, are written as they are. A caller that lays a whole file out itself encrypts it as `write::full` does through `Encryptor::whole`, writes nothing at a number `Encryptor::free` gives, where the file read lists it free under keys made per object, and lists each such number free at its generation (`Encryptor::free_numbers`), and puts in no object stream what `Encryptor::dictionary_object` names: `packable` tells an encryption dictionary by its shape alone, not what one refers to. Under RC4 a changed object written again at its number and generation, as an incremental update writes it, is encrypted with the keystream its earlier version was, and the XOR of the two is the XOR of their plaintexts: what changed leaks to one who holds both, an update's file or the earlier file after a whole save under the same key. A file written whole under a new AES-256 encryption (`Encryptor::create`) leaks neither way.

## What isn't here

- **Which objects, and at which numbers.** `write` writes what it's given at the numbers it's given; which of a document's objects to write, and what to renumber, are a caller's. An object copied as the bytes it was read from goes at its own number, from the reading it was read in: one moved to another number, or from another file, is written anew.
- **A durable write, or an append in place.** `write` writes into a `Vec<u8>`; writing it to a file, replacing one whole or appending to it, and keeping another writer from changing it meanwhile (`Base::holds` says whether a file is still the one a base describes), are the caller's.
- **Warnings turned off, or typed.** Each warning is text (`Warning::message`), made as the reader meets what it says and kept with the object it's in for as long as the reading is; a clean object costs nothing, but a caller can't turn them off.
- **Files for tests, and qpdf.** The `fixtures` feature writes files from objects, and runs qpdf (the program `PDF_LINT_QPDF` names, else `qpdf` on the `PATH`) as a second reader, hidden from the docs: for this workspace's tests and fuzz targets, not for a user. A stream qpdf decodes alone is handed to it in a file of its own, which tempfile makes in the temp folder and removes after. Without it, nothing runs a program, reads `PDF_LINT_QPDF` or writes a file.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
