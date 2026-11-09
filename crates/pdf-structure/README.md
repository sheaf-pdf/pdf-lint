# pdf-structure

PDF's file structure, read, in one Rust crate on [pdf-objects](https://crates.io/crates/pdf-objects): a file's header, its cross-reference tables and streams along their `/Prev` chain, and each object at the offset they give, every value, key and stream's data with where it was written (ISO 32000-2, §7.5); and what's read, a document, which a caller can build in memory too, to number new objects in, edit, walk and prune. And it writes a file of numbered objects, whole or as an incremental update after the file it was read from, and what a file's structure is written in: cross-reference sections, as tables and streams, and object streams; an encrypted file's objects encrypted as they're written, by the list of what's left in the clear that its reader decrypts by. Object streams and cross-reference streams are decoded through [pdf-filters](https://crates.io/crates/pdf-filters), and an encrypted file is opened through [pdf-crypt](https://crates.io/crates/pdf-crypt).

It's made for a reader that checks files as much as for one that shows them. Where a file isn't as the spec writes it, it's read as far as a reader can take it, and every choice the reader made is said, with where.

## What it does

- **A file read whole** (`read::document`, under `Options`, the one convenience: a file in memory opened as `Reading::open` opens it, below, and read whole, which can't fail): the header, `startxref`, the cross-reference tables and streams along their `/Prev` chain, and each object at its offset, through pdf-objects' parser, a stream's data by its `/Length` when that ends at `endstream`. A `/Length` that's a reference is followed, to an object at an offset or in an object stream. Offsets count from the header (ISO 32000-2, §7.5.2), so a file with bytes before `%PDF-` reads as it would without them, and `Fault::HeaderAt` says where the header is. Every position a reading gives, an entry's offset, a revision's place or a fault's byte, is a place in the file's bytes, as a span is. What it read is a `Document`: each object its cross-references list, or why it doesn't read; the trailer; the header's version; and the file's newest incremental update, each read by an accessor: of `Source`'s name where the trait has one (`trailer`, `header_version`, `facts`, `revisions` and the rest), and the document's own beside (`update`, `encryption`). Its objects are read by number (`Source::get`), in order or in a range of numbers (`objects`, `range`), beside those that don't read (`unread`), and put in, taken to edit and removed (`set`, `get_mut`, `remove`).
- **Or read on demand** (`Reading`): opened, the header, sections and trailer read and an encrypted file's security handler opened by the passwords its `Options` give, or the empty password, given none (`Reading::open`, the one way in), which aren't kept, or by another once it's open (`unlock`); then each object read when it's first asked for, kept (`get`) or not (`object_unkept`), and each object stream decoded once, when an object in it is first read, from any thread; and the bytes at a span (`Reading::bytes`), in the file or in an object stream's decoded data, but for an encrypted file's as written, which may be ciphertext. Read in any order and made a `Document`, a reading is the whole read, the same objects, faults in the same order and the same decoded data, so long as no stream it decodes reaches the reading's budget, which is spent in the order streams are decoded. The whole read is every object read so, in order, by the same code. A reading is a `Source` too, answering of what it has read when it's asked: its faults of what it has read, an object stream's decoded data of each it has decoded; its own `ids`, `get`, `faults`, `metadata`, `size` and `facts`, each in a shape that says more, are what a call on it names, and the trait's are reached through a `&dyn Source` or by the trait's name (`Source::get(&reading, id)`); `decoded` is pdf-objects' `Streams`', which no method of a reading's shadows. Every iterator it gives is a type of its own (`read::Ids`, `read::Entries`). Made a `Document`, the reading is taken (`into_document`) or kept beside it (`to_document`), what it kept cloned and the rest read without keeping it, its object streams decoded and kept: then it hands out what the document doesn't hold, a stream's data, which a document has only where it was written, and the bytes at a span. `read::WithReading` (at the root too) is the two together: a file opened under its `Options` and made a `Document` at once beside its reading (`WithReading::new`), the whole read; it gives the document (`document`), and as a `Source` answers as the document does, but gives a stream's data decoded (`Source::decoded_within`, by `Reading::decoded_within`), which a document alone doesn't. A document read whole is taken by a reading of its file opened again (`adopt`), where that reading reads each of its objects alike, so that it's written as one made beside its reading is (below). A reading holds its bytes as a `ByteSource`: a slice, a `Vec<u8>` or an `Arc<[u8]>` in memory; or a file read by position (`FileBytes`, on Unix and Windows), never whole but where the reader needs it whole, to scan the file for its objects; or any of them behind a reference, a `Box` or an `Arc`, a `dyn ByteSource` among them. It gives its source back (`Reading::source`, `into_source`), and each reading has an id of its own (`Reading::id`, a `SourceId`).
- **A file read by position**, a window at a time: the header from the file's start, `startxref` and the last `%%EOF` back from its end, each section at its offset, with the `startxref` and `%%EOF` after it, and each object at its own, its first window reaching a byte past the next offset the cross-references list, so that an object as it should be is one read; a window doubled while what's read runs to its end where more of the file could change it, so that each read reads what it would of the whole file (pdf-objects' `Parser::window`). Any `ByteSource` that doesn't hold its bytes in memory is read so, from its `first_window`. Where the source fails, the call fails (`read::Error::Io`), keeping nothing of what it was reading, so asked again it reads again; an object it read whole on the way, as the object stream an object is in, is kept, as `get` keeps it. A file's `Identity`, taken as it's opened, the file itself by a handle it holds open (its device and inode on Unix, its volume serial number and file index on Windows, through same-file) and its length and modification time, tells whether the file held open has changed in place (`FileBytes::unchanged`), and whether another is at its path now (`Identity::of`), even one made where the first was removed. `Identity::of` opens the file to read, without waiting, so it needs leave to read it, and holds it open while it lives. A path that's neither a file nor a folder, a FIFO, a socket or a device, it refuses without opening it. One swapped in after its metadata is read it refuses by what its handle is, never waiting on it.
- **A stream's data** (`Reading::data`), read each time it's asked for, and not kept: as written, filters and all, borrowed from bytes in memory; in an encrypted file that opened, decrypted as an object stream's is, by the stream's own `/Crypt` filter when it has one, else as `/StmF` says, and an embedded file's without one of its own by the crypt filter the encryption dictionary's `/EFF` names, where it names one (from `/V` 4); but for a cross-reference stream's, and the catalog's metadata stream's under `/EncryptMetadata false` unless its own `/Crypt` filter encrypts it, which are left as written, as the file has them. Where it may be ciphertext the reader doesn't decrypt, it's refused, saying why: in a file no password opened (but one whose streams aren't encrypted, `Encryption::Plain`, where a stream's own crypt filter, or an embedded file's `/EFF`, is all that can encrypt it), or whose security handler the reader can't open; a stream the encryption dictionary refers to; and an embedded file's without a crypt filter of its own where `/EFF` isn't a name, which doesn't say how. Decoded (`Reading::decoded_within`), it's that through its filters, read as an object stream's are, by pdf-filters, to at most the less of what it's given and the reading's limit on a stream, which holds for a stream without filters too. Data that doesn't decrypt or decode cleanly is an error holding what did (`read::Error`). As a viewer takes it, a reading, a document beside it (`read::WithReading`) and a document alone give it through pdf-objects' `Streams` (`Streams::decoded(&reading, id, limit)`): what decoded of a stream that decodes in part, unless the limit stopped it, or why there's none in a resolver's words, `Why::Filter` for filters the reader doesn't decode; a document alone gives the data of the object streams it holds that the reader decoded whole, and of a stream it holds as bytes, built or edited in memory, decoded as a reading decodes one (its own `/Crypt` filter no filter, as those bytes are plaintext, which a writer encrypts as it writes them), and of any other stream says it hasn't the file's bytes to read it from (`Why::Detached`), which asked again it won't have, where a source that failed to give them says `Why::Source`.
- **Each cross-reference section on its own** (`Reading::sections`, `Reading::first_object`), read again from the source each time it's asked for, and not kept: where it starts, how it's written, its rows in the order written, its trailer, its `/Prev`, its stream's number and what the `startxref` after it gives, and the file's first object as written, for what checks a file's layout. A reading merges sections into one list of entries, as a reader wants them. Each section is read where the reading read it, and one read away from where it's said gives that place too (`SectionRows::said`).
- **Where each object was read from** (`Record`): in which reading (its `SourceId`, so a record is known for the file its places are in, though two readings of one file read alike); the number and generation it was read as; at the offset a section gives, at a shift from it the file proves (`Origin::Shifted`), at one a scan found, or in an object stream's decoded data, the `index`th of its objects; its bytes, from its `n g obj` to past its `endobj`, and whether `endobj` follows it; a stream's data (`DataExtent`), and whether it was found by its `/Length`, and whether that's a reference; and a hash of its bytes as read, private, which the writer checks a copy of them by. And what the header and sections say of the file as written (`Facts`): where `%PDF-` starts, the newest section's offset and kind, how many sections, whether one is a hybrid's, the largest cross-reference stream's number, whether a linearization holds (ISO 32000-2, Annex F), and whether the reader took a recovery (`Facts::recovered`); and its revisions (`Revision`, `Document::revisions`, `Reading::revisions`), the first first, each where its newest section starts, its trailer, the numbers its sections list but the first's, and where its bytes end, past the `%%EOF` after its last-written section's `startxref` and past that line's end (`Revision::end`), a linearized file's two sections one revision, ending where its main section does. A `Document` keeps each object with its record while the object is as read (`Document::record`): one put in has none, and one taken to edit loses it, so a record is always of the object beside it.
- **A document in memory** (`Document::new`, or a file read whole): added to and edited, its new objects numbered past every number the file's cross-references list, in use or free, its cross-reference streams' and the trailer's `/Size` less one (`new_id`, `add`; where they start, `max_id` and `set_max_id`), never 0, and once those reach 4,294,967,295, the largest number, at numbers no entry holds. References followed to where their chain ends (`dereference`, as many as 128 of them; `get_dict` and `get_dict_mut`, a stream's dictionary too; `catalog` and `catalog_mut`), and the trailer's dictionary changed where it is (`trailer_mut`, then `Trailer::dict_mut`). What a reader outside the crate knows of a file it read is set (`trailer_mut`, `set_header_version`, `set_update`, `set_size`, `set_metadata`, `set_encryption`, `set_revisions`); what the bytes reader keeps of its own reading isn't. An object stream it decoded is lent decoded (`Streams::decoded`) only while its object is as read: put, edited, set unread or removed, it's no longer lent, its objects' places in its data kept (`held`). Every iterator it gives is a type of its own (`source::Objects`, `Range`, `Unreads`, `Records`; `write::Bodies`). What the trailer reaches walked, each once, in the order found (`reachable`), every entry of a key written twice among it; and what it doesn't removed (`prune`): the object streams and cross-reference streams, which nothing refers to, among them, the encryption dictionary, which the trailer does, kept, and the decoded data left as it is.
- **Read a damaged file where it proves it is.** `Options::recovery` (a `read::Recovery`) says how a file is read where its sections or objects aren't where it says. The default, `Recovery::Verified`, takes a recovery only where the file proves it. A section a few bytes off is read where an `xref` or a cross-reference stream starts exactly, within 64 bytes, or where its offset puts it counted from the file's first byte, there or past the line's end. A section's entries off by one shift are read at it, each at its own `n g obj`. Once a file is shown not to be as written, an entry at neither place is read where a scan of the file finds it. Each recovery is one fault a section (`Fault::Moved`, `Fault::Shifted`). Each object read elsewhere says so (`Origin::Shifted`, `Origin::Recovered`), and `Facts::recovered` says whether a reading took any, with faults off too. Where the sections don't read at all, the file is scanned for its objects, those in the object streams it finds among them (`Fault::Members`), and the trailer says why (`Trailer::unwritten`). A scan passes over a stream's data as PDFium's does: by its `/Length` where `endstream` follows it, else to the first `endstream` or `endobj`. Where it finds neither, it reads on through the data. `endstream` ends the data wherever it's written, run into what's beside it or not, where PDFium's scan, and `Recovery::Pdfium`'s, takes only a word of its own. Each stream the scan ends at `endobj`, or finds no end to, is said (`Fault::StreamEnd`). Where it ends a stream at `endobj` before an `endstream`, and reading on meets that `endstream` on its own, the data ran there: what it found inside is taken back, so a document embedded in a stream doesn't stand over the file's own objects, and nothing is said. Where it still ends a stream at `endobj` before an `endstream`, or passes an `endstream` inside the data by its `/Length`, the file is scanned again with each stream's data run to its first `endstream`. Each object of a number the first scan didn't find, and each `trailer`, is taken from that second scan, so the reading finds every object number a scan by `endstream` alone finds. The scan's last trailer, a `trailer` dictionary's or a cross-reference stream's, takes what it lacks of `/Root`, `/Encrypt`, `/ID` and `/Info` from an earlier one, as a linearized file's last may have only `/Size` (`Fault::Taken`). A table's first subsection numbered from 1 that starts with object 0's row is read from 0 where every row in use proves it, as pdf.js reads one (`Fault::Renumbered`, `Origin::Renumbered`).
- **Or read it as PDFium does.** `Recovery::Pdfium` reads a file as PDFium does, as of chromium/8125, for a caller that shows it as a viewer would. It looks for no section near where it's said. It scans the file where `startxref` gives no section that reads, where a section along the `/Prev` chain doesn't read or the chain loops, where the first object the first-written table lists isn't at its row, where a later section's `/XRefStm` doesn't read, or where the trailer's `/Root` gives no page. The scan lists the objects, members and trailers PDFium's lists, merged over what the sections gave, and the trailer says why. Where it doesn't scan a file and `Recovery::Verified` takes no recovery, the two read the file alike. It parts from PDFium in what it parses: objects are read by ISO 32000-2's rules, so a damaged dictionary PDFium reads past doesn't read here. And it reads a file PDFium won't open, one without `%PDF-` or an encrypted one without its password. A few edges of how PDFium finds and lists sections are read by this crate's rules, as `Recovery::Pdfium`'s docs say. The rules are PDFium's, read from its source and written in this crate's own code. Each case is recorded in the repository's `tests/pdfium` beside the crate and held to PDFium's answers there, and its `differ.tsv` says why `Recovery::Verified` reads a case otherwise.
- **Every choice said.** What the reader had to choose how to read, or read past, is a `Fault` (`read::Fault`, at the root too), kept at its place, pdf-objects' `fault::Place`, the object it's in and where it was written: a stream whose `/Length` doesn't end at `endstream`, an object without `endobj`, a cross-reference table whose rows aren't the 20 bytes the spec writes (`read::RowShape`), a file whose last line isn't `%%EOF` alone, an object an object stream's header starts inside the bytes of another read from it, a string that doesn't decrypt cleanly, and what pdf-objects' parser reads past (`Fault::Parse`, its kind a `read::ParseKind` and its words pdf-objects'). Each variant's fields are the whole of what it says, and its `Display` says it in words. A reading keeps them as it reads, from any thread, and gives them in the whole read's order, whatever order it read in (`Source::faults`), or tells them to any of pdf-objects' `fault::Sink`s (`Source::tell_faults`), so that one sink takes several crates' faults. A caller that wants none says so (`Options::faults`, `read::Faults::Off` where it's `Kept` by default): then none is looked for, pdf-objects' parser is asked for none, and the reads made only to say one, the file's end read back for its `%%EOF` and an object stream's members sorted for those that overlap, aren't made; what's read is the same either way.
- **Streams decoded.** Cross-reference streams and object streams are decoded from their own data, through pdf-filters: ASCIIHex, ASCII85, LZW, Flate and RunLength, with their predictors, Flate inflated by the backend `Options::flate` names (miniz_oxide by default; zlib-rs with this crate's `zlib-rs` feature). Their `/Filter` and `/DecodeParms` are read as qpdf reads them: the abbreviations `/Fl`, `/AHx`, `/A85`, `/LZW` and `/RL` on any stream, `/Crypt` as no filter (decrypting comes before the filters, by its `/Name`), a parameters dictionary that isn't in an array every filter's, a reference to no object as null. A filter it doesn't decode, or parameters it doesn't take, leave the stream undecoded, saying why; data a fault stopped is read as far as it decoded, each fault told on the stream. The objects read from an object stream are placed in its decoded data (`Span::stream`), which the `Document` keeps. What the reader reads by is public, for a reader of its own: `read::filters`, a stream's filters as it reads them; `read::encryption`, the values of an encryption dictionary that pdf-crypt's handler opens a file by, its references followed as it follows them; and `read::viewed`, what a viewer takes of a decode.
- **Encrypted files opened** (`Options::passwords`, which `Reading::open`, `read::document` and `WithReading::new` take) with the passwords given, each a pdf-crypt `Password` as typed, tried in each of its encodings, as the owner's and then the user's, every password's that fit the file's revision before any password's others (pdf-crypt's `Password::encodings` and `Handler::open`), in the empty password's place: given none, the empty password, and a wrong one leaves the file locked, or plain where its strings and streams aren't encrypted, as no password would. A reading opened by the empty password (`Reading::open`) and left locked is opened by another once it's open (`Reading::unlock`, with the `Encoded` bytes to try), its handler opened from what the reading kept: what it read is dropped, to be read again decrypted, and a password that doesn't open it gives it back as it was, with why. A file read by scanning has its object streams read again for their members, and under `Recovery::Pdfium` its page tree too, as `Reading::unlock`'s docs say. Through pdf-crypt's standard security handler, revisions 2 to 6: object streams decrypted before they're decoded, and strings decrypted where they are, their bytes exact, but a signature's `/Contents` and the encryption dictionary's; an object stream whose data is in the clear while strings aren't, whose objects' strings are then in the clear, is recorded (`Document::in_the_clear`). A string or stream that doesn't decrypt cleanly is a fault. `Document::encryption` says whether the file is encrypted, and whether it opened. Opened, it says by the owner's password or the user's, and which of those given in which encoding (`by`). It says what the file's `/P` grants one who opens it by the user's password, by its revision's meaning of the bits (`permissions`, pdf-crypt's `Permissions`, which the reader doesn't enforce). It says how the file's strings, streams and embedded files are encrypted (pdf-crypt's `Method`), its revision, and whether its metadata is encrypted. And it says what pdf-crypt's handler found wrong with the encryption dictionary (`faults`: `/Perms` that doesn't match, or doesn't confirm the key), whatever faults the reading looks for. `Encryption::protection` gives a new encryption the file's permissions, so a rewrite under AES-256 grants no more. A file that didn't open is locked, or has a handler the reader can't open, and why (`source::Unsupported`: an `/Encrypt` that isn't a dictionary it reads, `source::Uncrypt`, or pdf-crypt's refusal). Or it reads whole all the same, as its strings and streams aren't encrypted (`/Identity`). `Options::DEFAULT` opens a file with the empty password. pdf-crypt is re-exported (`pdf_crypt`), its `Password`, `Encoded` and `Encoding` named through it. What it decrypts and what it leaves as written is listed in `read`'s documentation, "What's encrypted", each with the clause of ISO 32000-2 that says so and the tests that hold it.
- **A revision before an update** (`Options::section`): the file as of a cross-reference section, read exactly where it's given, along its `/Prev` chain as if it were the newest, so that what later updates wrote isn't in it. Where no section reads there, the verified recovery reads nothing, and the trailer says why (`Unwritten::AsOf`). A scan would find what later updates wrote. And **the file as a revision was written** (`Reading::prefix`): its bytes up to the revision's end (`read::Prefix`, at the root too, a source's first bytes as a source of their own), read as a file of their own, lazily, under the limits the caller gives (`Reading::limits`, the reading's, or less), its length, its sections and its last `%%EOF` the revision's; an encrypted one opened by the reading's security handler where its encryption dictionary's values and first `/ID` element are those the reading opened by, so that no password is asked for or kept.
- **Whole files and updates written** (`write`): a document's objects, or any numbered objects, as a whole file (`write::full`, `write::document`) or as an incremental update after the file they were read from (`write::update`), encrypted as that file is or under a new encryption, or decrypted; an object as it was read copied as the bytes it was read from where they're right to copy, checked against a hash of them taken as it was read; and the parts a file is laid out in, cross-reference sections and object streams, for a caller laying a file out itself; below, "Writing".
- **Bodies written one at a time** (`write::Parts`), for a caller that lays a file out itself: each object and object stream as a whole file writes them, at the numbers it gives. An encrypted file's are encrypted by the `Encrypting` that `Encryptor::laid_out` gives for those numbers, written by its renumbering, and copied where that's the key they were read under. Beside them are what a whole file leaves out (`write::left_out`, `write::encryption_objects`), a trailer's own entries (`write::trailer_entries`), and `/ID` (`write::first_id`, `write::file_id`, `write::id_value`).
- **Limits, one value a caller sets** (`Limits`, in a reading's `Options`): by default, a stream decodes to at most 256 MiB (`read::STREAM`), and the streams of a reading of a file to 1 GiB in all (`read::BUDGET`); at most 1,024 sections along `/Prev` (`read::SECTIONS`); no more cross-reference entries than the file has bytes, or 2²⁰ if that's more; and pdf-objects' limits on each object, values nested 256 deep (`read::NESTING`) and 2²⁵ values in one; and, the reader's own beside them, at most 25 filters a stream (`read::FILTERS`). A caller sets them on a copy of `Limits::DEFAULT`, as it does `Options`', which also say as of which section a file is read, by which passwords, and which Flate backend its streams inflate on (`Options::flate`). They're the same in every build.
- **Its phases traced**, with the `tracing` feature, off by default: the open, its sections and a scan, each object stream decoded, a whole read, an earlier revision's open, and each write and the object streams it builds, each a span of [tracing](https://crates.io/crates/tracing), at `debug` (an object stream decoded at `trace`), its fields numbers alone (an object's number, an offset, a count, a length), never a value, a key, a password or plaintext. Without the feature, nothing of them is compiled, and tracing isn't a dependency.
- **Typed errors.** Why an object doesn't read is an `Unread`: it isn't in the file, doesn't parse (pdf-objects' error), or isn't the object at its offset; or the object stream it's in doesn't decode (`read::Undecoded`, its filters `read::Unfiltered`), needs a password, or doesn't say where the object is (`read::Unindexed`); or the source of the file's bytes didn't give them (`Unread::Source`); or, from another reader, that reader's words (`Unread::Other`). Each says why as text too, and the object stream the object is in where why is that stream's or its place in it (`Unread::container`); and, as a resolver's `try_resolve` says it, of the kind a caller decides by, damaged, past a cap, locked or the source's (`Unread::unreadable`), so that a resolver over any `Source` says it as a document's does. A reading's call fails with a `read::Error` (`Error` at the root), which may hold pdf-filters' error, re-exported as `pdf_filters`, or pdf-crypt's. What the reader says of the file it read is typed too, each in its words: why the trailer isn't the newest section's (`Trailer::unwritten`, a `read::Unwritten`, or another reader's words, `Unwritten::Other`), why a security handler doesn't open (`Encryption::Unsupported`), why a section doesn't read (`read::SectionFault`), and each fault.
- **A `Source`,** the trait a reader of a file implements, with pdf-objects' `Streams` (above), which `Document`, `Reading` and `WithReading` do: its objects by number (where the source of the file's bytes didn't give an object's, `Unread::Source`, which asked again may read), its trailer, header version and faults (lent, or made for the call, and told to a sink by `tell_faults`), and what a reader may know besides, each defaulting to nothing: the file's size and the catalog's metadata when it's in plain view, where each object was read (`record`), an object stream's decoded data as the reader holds it (`held`), the file's revisions, its `Facts`, whether an object stream leaves its objects' strings in the clear (`in_the_clear`), and a stream's data decoded within a limit (`decoded_within`), the reader's own account, every fault of it, beside `Streams::decoded`, what a viewer takes, which a reading and a reader kept beside its document give (`read::WithReading`) and a document alone gives only of what it holds: past the limit an error holding what decoded, with pdf-filters' `Fault::Limit`, never the data cut short, and none of any stream from a reader that gives none. A `Document`, a `Reading` and a `WithReading`, by reference, are pdf-objects' `Resolve` too, which what follows references reads through, across threads too: a reference resolves to its object, read and kept as `get` keeps it, or to none, null, where the file hasn't it, it doesn't read, or the source fails the read, without saying why, which `try_resolve` says: none where the file hasn't it, or an `Unreadable`, of `Why::Damaged`, `Limit`, `Locked` or `Source`. `Reading::failed_resolves` counts the resolves the source failed, where what read through `resolve` was given a null and nothing after it would say so.

## An example

```rust
use pdf_structure::pdf_objects::{Object, ObjectId};
use pdf_structure::{Fault, Options, Source, read};

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

let doc = read::document(&file, &Options::DEFAULT);
assert_eq!(doc.trailer().unwritten, None, "read by its table");

// An object by its number, and where it was written.
let pages = doc.get(ObjectId::new(2, 0)).unwrap();
let count = pages.as_dict().unwrap().get(b"Count").and_then(Object::as_integer);
assert_eq!(count, Some(0));
let at = pages.at.unwrap();
assert_eq!(&file[at.start..at.end], b"<< /Type /Pages /Kids [] /Count 0 >>");

// What the reader had to choose how to read, and where.
let [(place, fault)] = doc.faults() else { panic!() };
assert_eq!(*fault, Fault::AfterEof { bytes: 8 });
assert_eq!(fault.to_string(), "the file goes on for 8 bytes after its last %%EOF");
assert_eq!(place.at.map(|at| &file[at.start..at.end]), Some(&b"\n% more\n"[..]));
```

The same file read on demand, from bytes shared across threads, an object read when it's asked for, and where it was read from:

```rust
use std::sync::Arc;

use pdf_structure::{Options, Origin, Reading};
use pdf_structure::pdf_objects::ObjectId;

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
    xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
    trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
let bytes: Arc<[u8]> = Arc::from(&file[..]);
let reading = Reading::open(bytes, &Options::DEFAULT).unwrap();
assert_eq!(reading.ids().collect::<Vec<_>>(), [ObjectId::new(1, 0)]);

// Read on another thread, and kept.
std::thread::scope(|scope| {
    scope.spawn(|| assert!(reading.get(ObjectId::new(1, 0)).is_ok()));
});
let record = reading.record(ObjectId::new(1, 0)).unwrap();
assert_eq!(record.origin, Origin::File);
assert_eq!(&file[record.at.start..record.at.end], b"1 0 obj\n<< /Type /Catalog >>\nendobj");

// Whole, it's the file read whole: beside the reading, which hands out
// what the document doesn't hold, or taking it.
let doc = reading.to_document().unwrap();
assert_eq!(doc, pdf_structure::read::document(file, &Options::DEFAULT));
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
use pdf_structure::pdf_objects::{Object, ObjectId, dict, version::Version};
use pdf_structure::{Document, Source};

let mut doc = Document::new(Version::new(1, 7), dict! { "Size" => 1 });
let kids: Vec<Object> = Vec::new();
let pages = doc.add(dict! { "Type" => "Pages", "Kids" => kids, "Count" => 0 });
let catalog = doc.add(dict! { "Type" => "Catalog", "Pages" => pages });
let font = doc.add(dict! { "Type" => "Font", "Subtype" => "Type1" });
assert_eq!((pages, catalog, font), (ObjectId::new(1, 0), ObjectId::new(2, 0), ObjectId::new(3, 0)));
doc.trailer_mut().dict_mut().set("Root", catalog);

// The catalog, and the pages its /Pages refers to.
let root = doc.catalog().unwrap();
let (id, tree) = doc.dereference(root.get(b"Pages").unwrap()).unwrap();
assert_eq!(id, Some(pages));
assert_eq!(tree.as_dict().and_then(|d| d.type_name()), Some(&b"Pages"[..]));

// What the trailer reaches, and the rest removed.
assert_eq!(doc.reachable(), [catalog, pages]);
assert_eq!(doc.prune(), [font]);
assert_eq!(doc.ids(), [pages, catalog]);
assert_eq!(doc.new_id(), ObjectId::new(4, 0));
```

A file without `startxref` is scanned for its objects, and the trailer says so:

```rust
use pdf_structure::pdf_objects::ObjectId;
use pdf_structure::read::Unwritten;
use pdf_structure::{Options, Source, read};

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
let doc = read::document(file, &Options::DEFAULT);
assert!(doc.contains(ObjectId::new(1, 0)));
assert!(doc.get(ObjectId::new(1, 0)).is_ok());
assert_eq!(doc.trailer().unwritten, Some(Unwritten::NoStartxref));
```

A file whose `startxref` is 3 bytes off is read where it proves its table is, and said; read as PDFium reads it, it's scanned:

```rust
use pdf_structure::pdf_objects::ObjectId;
use pdf_structure::read::{self, Recovery, Unwritten};
use pdf_structure::{Fault, Options, Origin, Source};

// A catalog, a page tree and its page, each listed where it starts.
let mut file = b"%PDF-1.7\n".to_vec();
let mut rows = String::from("0000000000 65535 f \n");
for object in [
    "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
    "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
] {
    rows.push_str(&format!("{:010} 00000 n \n", file.len()));
    file.extend_from_slice(object.as_bytes());
}
// The table, which startxref puts 3 bytes past its `xref`.
let (found, at) = (file.len(), file.len() + 3);
let tail = format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n");
file.extend_from_slice(format!("xref\n0 4\n{rows}{tail}").as_bytes());

// By default, read where the file proves its table is, and said.
let doc = read::document(&file, &Options::DEFAULT);
assert_eq!(doc.trailer().unwritten, None, "read by its table");
assert!(doc.facts().recovered);
let [(_, fault)] = doc.faults() else { panic!() };
let moved = Fault::Moved { at, found, from_start: false, moved: 0, scanned: 0, neither: 0 };
assert_eq!(*fault, moved);
assert_eq!(fault.to_string(), format!("the section said to be at byte {at} is at byte {found}, 3 bytes before"));
assert_eq!(doc.record(ObjectId::new(1, 0)).unwrap().origin, Origin::File);

// As PDFium reads it: no section looked for near, so the file is scanned.
let mut options = Options::DEFAULT;
options.recovery = Recovery::Pdfium;
let doc = read::document(&file, &options);
assert_eq!(doc.trailer().unwritten, Some(Unwritten::Neither { at }), "found by scanning");
assert_eq!(doc.record(ObjectId::new(1, 0)).unwrap().origin, Origin::Recovered);
```

## Writing

`write` writes numbered objects as a whole file (`write::full`), a document's objects among them (`write::document`, but object streams, cross-reference streams and a linearization dictionary, which it writes anew or would no longer hold), or as an incremental update after a file (`write::update`), at the end of a caller's `Vec<u8>`, positions counted from where the write starts in it. A whole file is the header, each object at its number in the order given, the encryption dictionary, and one cross-reference section, a table or a stream (`write::Options`, set on a copy of `Options::DEFAULT`); asked to (`Options::pack`), what may go in an object stream goes in one, grouped as the caller says (`Pack::groups`) or by number, a hundred each, numbered from the numbers the caller gives first (`Pack::reuse`), built on threads, the same objects on any number, and in the clear the same bytes. Its trailer is the caller's but for the keys a section writes itself, and its `/ID` keeps its first string and makes the second the first 16 bytes of SHA-256 of what it wrote, so in the clear the same objects write the same bytes. An update goes after a file as a `Base` describes it: each object at its number, then a section of the file's kind listing only those, its `/Prev` the file's last section, its trailer the file's newest trailer's entries carried (ISO 32000-2, §7.5.6). `Base::of` describes a reading's file, its header and its newest section where the reading found them, so an update after a file with bytes before its header, or after one read under a recovery, reads as the file does. After a recovered reading, the update also lists each object read at a shift or found by a scan where it is, so every reader finds it there. `Base::of` refuses a file read by scanning, one without `/Root`, and a reading as of an earlier revision. It takes a reading as of the newest section where a reading of the whole file reads that section, as its recovery finds it. `Base::new` describes a file the caller read otherwise, its fields set after. Each write gives back what it wrote (`Written`): where each object landed, and the file as it now is, so a second update goes after it without reading it again, and `Base::holds` tells whether a file is still the one a base describes. A whole file is of no reading, so an update after it passes over nothing as read: what changed since the save is the caller's to give. A stream whose data is where the reader read it (`Data::At`) has it read from the reading given (`&dyn ReadFrom`, a `Reading`), only at its own number there, decrypted and encrypted again as the write is. Under an encryptor, each object is encrypted under the number it's written at, an object stream whole. An update is encrypted as its file is, by that file's encryptor, and refuses another. The encryptor is matched to the file by where its dictionary is, the dictionary, `/ID[0]` and a check of the key. Under RC4, an update's object at a number the file lists free, below the generation its entry gives, is refused, as its keystream may be a deleted object's. So is a string or stream's data new under a keystream the file used (below). What's refused writes nothing (`write::Error`): a number 0 or past `NUMBERS`, or given twice, among it, and a write a flag stops (`Options::stop`). A trailer isn't encrypted (§7.6.2), so under an encryptor a direct `/Info` is written as an object (below).

An object as it was read (`write::Body::Read`, its value and its `Record`; `write::bodies` gives a document's so, and `write::document` writes them) is copied as the bytes it was read from, read again from the reading given, where they're right to copy: of that reading (`Record::source`), at its own number (`Record::id`), where its section puts it (not at a shift, nor found by a scan), `endobj` after it, a stream's `/Length` a number and its data as long; read at an offset, under that reading's own encryption (`Encryptor::of`), the list of what's in the clear saying the same of it in the file written, or with neither encrypted; read from an object stream, into an object stream of the write's, under any encryption or none, as its bytes there are plaintext. Else it's written anew from its value, and so it always is with no reading given. A document to write so is made by `Reading::to_document`, its reading kept and given to the write: one read whole (`read::document`, `Reading::into_document`) has dropped it, and a stream of it is refused (`write::Error::Placed`), until a reading of its file opened again takes its objects as its own (`Reading::adopt`). That reading compares each object's record with its own of that number, read again where it hasn't kept it, a parse each: where every one is the same record but its own (read as that number, where and how, and its bytes' hash), what the document holds unread doesn't read there either, and the document is encrypted, and opened, as the file is (read by a password where the reading was, the encryption dictionary as stored, where the trailer names it, and the first `/ID` element, which a key is made of), each record then names it; else nothing is taken (`read::Error::Unlike`, `UnlikeEncryption`), another file, the file changed since, another revision of it, or a document read without the password among what's refused. Packed, what may go in an object stream goes in one, written anew, whether it was read at an offset or not; a caller that would keep such an object copied leaves its number out of every group (`Pack::groups`), which writes it on its own. A copy's bytes are checked first against a hash of them taken as the object was read, and a write given a reading first asks its source whether its bytes changed (`ByteSource::unchanged`: for a `FileBytes`, the file's length and modification time): where they did, or a record is another reading's or another object's, the write is refused (`write::Error::Changed`, `Elsewhere`). The hash is keyed by one random key for the process, std's `RandomState`'s. The key is never shown, so bytes can't be written to hash alike without it. On a target where std has no randomness from the operating system (`wasm32-unknown-unknown`), the key can be guessed. Nothing here closes the race with a writer between the check and an append. An update after a reading's file passes over each object as that reading read it, which the file holds, so a document's bodies write what was built or changed since, and a second update after the first's `Written::base` does too.

A whole file is written renumbered where it's given a renumbering (`Options::renumber`, any function from an id to an `Option` of one): each object at the number it gives, each reference in it and in the trailer named as it names it, `null` where it names none, as a reference to no object reads (ISO 32000-2, §7.3.10). The objects given stay at the numbers they're held by, a document's ids, an object as read at the number it was read as, and a stream's data, a copy's record and what the encryptor resolves are found by those; what the file uses is checked in the numbers written: 0 and `NUMBERS`, two objects at one number (`write::Error::WrittenTwice`; two numbers may be given one, but one object is written there, so `write::document`, giving every object, refuses a renumbering that shares one, and under an encryption no other object shares the number of an encryption dictionary or what one refers to, which a reader reads in the clear, `write::Error::Moved`), the encryption dictionary's, `Pack::reuse` and the writer's own objects; `Pack::groups` is given the numbers objects are given at. An object it gives no number is refused (`write::Error::Unnumbered`). Encrypted, each object is encrypted under the number it's written at, by the encryptor in the numbers written (what it leaves in the clear, its dictionary's place and references), the document's metadata stream found in the numbers given and kept as its number written, as `Encryptor::laid_out` gives it a caller laying a renumbered file out. Under RC4, an object written anew where the renumbering puts it at a keystream the file read used goes at that number's next generation instead (`Written::regenerated`). An object as read is copied as the bytes it was read from but its header, `n g obj`, where it's written at another number, and each reference the renumbering names otherwise, `n g R` or `null`, each written anew where its bytes had it, a comment between a changed reference's tokens going with them and every other byte, strings and stream data among them, as it was; each of its references is first checked to be where its bytes have it, in order, and each changed one to read there as itself, else the write is refused (`write::Error::Elsewhere`). At another number than it was read as, it's copied only where neither file is encrypted or no method of the reading's encryptor makes a key per object: RC4's and AES-128's keys are made of the object's number (§7.6.3, Algorithm 1), AES-256's is the file's (Algorithm 1.A), its IV beside its ciphertext, so under RC4 and AES-128 an object that moves is written anew, encrypted under its new number, and one that stays is copied, its references rewritten. An object stream's member goes into one of the write's under any encryption, its references rewritten. What a renumbered write wrote says so (`Base::renumbered`), and an update after it is given objects at the numbers it wrote, built or edited, and refuses an object as read and a reading (`write::Error::Renumbered`), whose numbers aren't the file's any more.

An update is written renumbered too: what it's given, and the trailer it carries, is in the numbers of the file it goes after, and each object is written at the number the renumbering gives it, each reference, `/Root` and `/Info` among them, named as it names it. An object as read is passed over only where the renumbering leaves its number and every reference in it where they are; one that moves, or names one that does, is written, copied where the rules for a copy hold, and nothing else is written where one passed over is (`write::Error::WrittenTwice`). The update doesn't write the encryption dictionary again, so it encrypts by the list of what's left in the clear in its file's numbers, as its encryptor has it, the document's metadata stream at the number it's written at, and a renumbering that moves the dictionary or what it refers to, or writes another object at one's number, which the file's reader reads in the clear, is refused (`write::Error::Moved`). An update frees nothing: an object it moves stays at its old number too, and what it doesn't write still names it there, so a caller gives every object a reference in which moves.

```rust
use pdf_structure::pdf_objects::{Object, ObjectId, dict};
use pdf_structure::write::{self, Base, Body, Pack, ReadFrom};
use pdf_structure::{Options, Reading, Source, XrefKind};

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
    xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
    trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
let from: &dyn ReadFrom = &reading;
let mut doc = reading.to_document().unwrap();
doc.catalog_mut().unwrap().set("Lang", Object::text("en"));
let lang = |reading: &Reading<&[u8]>| {
    let catalog = reading.get(ObjectId::new(1, 0)).unwrap().as_dict().unwrap();
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
assert!(back.faults().is_empty());
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
let again = Object::from(dict! { "Type" => "Catalog", "Lang" => Object::text("fr") });
let catalog = [(ObjectId::new(1, 0), Body::from(&again))];
write::update(&mut updated, &read, catalog, None, None, &defaults).unwrap();
let back = Reading::open(&updated[..], &Options::DEFAULT).unwrap();
assert_eq!(back.facts().sections, 3);
assert_eq!(lang(&back).as_deref(), Some(&b"fr"[..]));
```

For a caller that lays the file out itself, `write` writes what a file's structure is written in: its objects are written by pdf-objects' writer (`serialize::indirect`), each where the output has got to, and a `Section` lists each at that position. A whole file's section (`Section::full`) is one subsection from 0, every number it doesn't list written free, the free entries linked in a list from entry 0, each at the generation its number is to be used at next (ISO 32000-2, §7.5.4); an update's (`Section::append`) lists only what it's given, in runs (§7.5.6). Either is written as a table (`Section::table`), which refuses an object in an object stream and an offset past ten digits, or as a stream (`Section::stream`), its `/W` as narrow as its widest field, a whole file's deflated under PNG's Up predictor, as qpdf writes its own; `Section::get` gives back what it lists each number as. Positions count from the header, past any bytes before `%PDF-` (ISO 32000-2, §7.5.2), as this crate's reader, PDFium, pdf.js and qpdf count them.

`packable` says whether an object may go in an object stream (§7.5.7): not a stream, an object whose generation isn't 0, a dictionary shaped as the standard security handler's encryption dictionary (a `/Filter` name, `/O` and `/U`), the linearization dictionary, or a signature dictionary, so that a signing tool finds it where it looks. The clause's list is `KeptOut`, which a reader checks what it reads by too, each with how either side tells it; `kept_out` says what of it the object alone tells, and `packable` is said through it. `container` makes an object stream of objects written direct, a header saying where each starts, deflated, refusing a member numbered 0 or past `NUMBERS` and a number given twice; none where it would have more than 65,536 members, or its data would be past what the reader decodes a stream to (`MAX_DATA`, 256 MiB, `Limits::stream`'s default). A section lists each object in it as `Entry::Compressed`, which only a stream can. Both deflate through pdf-filters, by the backend and at the level a `Deflate` says: by default (`Deflate::DEFAULT`) miniz_oxide at 6, which every build has; zlib-rs with the `zlib-rs` feature. A written file's objects are numbered below `NUMBERS`, 8,388,607, the most a file may hold (ISO 32000-1, Annex C): a section listing a number past it, or a whole file's `/Size` past it, is refused as it's written, before a row is made. What can't be written is refused, and nothing is written (`write::Error`): a value pdf-objects' writer refuses among it.

```rust
use pdf_structure::pdf_objects::{Object, ObjectId, dict, serialize};
use pdf_structure::write::{self, Deflate, Entry, Section};
use pdf_structure::{Options, Source, read};

// A catalog and its page tree, each written where the file has got to.
let mut file = b"%PDF-1.7\n".to_vec();
let mut section = Section::full(0);
let kids: Vec<Object> = Vec::new();
let objects = [
    (1, Object::from(dict! { "Type" => "Catalog", "Pages" => ObjectId::new(2, 0) })),
    (2, Object::from(dict! { "Type" => "Pages", "Kids" => kids, "Count" => 0 })),
];
for (num, obj) in &objects {
    let offset = serialize::indirect(&mut file, ObjectId::new(*num, 0), obj).unwrap();
    section.insert(*num, Entry::InUse { offset, generation: 0 });
}
// Two objects in an object stream, object 5, each listed as its place there.
let members = [(3, b"(three)".to_vec()), (4, b"<</Four 4>>".to_vec())];
let container = write::container(&members, Deflate::DEFAULT).unwrap().unwrap();
let offset = serialize::indirect(&mut file, ObjectId::new(5, 0), &Object::from(container)).unwrap();
section.insert(5, Entry::InUse { offset, generation: 0 });
for (index, (num, _)) in (0..).zip(&members) {
    section.insert(*num, Entry::Compressed { stream: 5, index });
}
// The section as a stream, object 6, the trailer its dictionary.
let pos = file.len();
section.stream(&mut file, 6, dict! { "Root" => ObjectId::new(1, 0) }, pos, Deflate::DEFAULT).unwrap();

let doc = read::document(&file, &Options::DEFAULT);
assert_eq!(doc.trailer().unwritten, None, "read by its section");
assert!(doc.faults().is_empty());
let four = doc.get(ObjectId::new(4, 0)).unwrap().as_dict().unwrap();
assert_eq!(four.get(b"Four").and_then(Object::as_integer), Some(4));
```

An encrypted file's objects are encrypted as they're written, and an `Encryptor` says how. `Encryptor::of` takes a reading's: its security handler as a password opened it, its encryption dictionary as the file stores it, at its number or direct in the trailer, and the `/ID[0]` its key is made from. It's refused for a file no password opened (`write::Error::Locked`), or whose handler the reader can't open (`Unsupported`). `Encryptor::create` makes a new file's of revision 6 (AES-256), by pdf-crypt's `Handler::create`, protected as a `pdf_crypt::Protection` says: by an open password, a permissions password or both, and what one who opens it by the open password may do. Its key and salts come from the operating system.

What encrypts a file's objects is an `Encrypting`, pdf-objects' `serialize::Encrypt`, made by `Encryptor::laid_out` (below). Written through `serialize::indirect_with` and `indirect_stream_with`, every string of an object and every stream's data is encrypted with the object's key. AES's IV is the next of the write's own `Ivs` (pdf-crypt's), 16 bytes from the operating system. So no IV serves two plaintexts, and two saves of one document differ in every AES ciphertext written anew. What's left in the clear is one list, which the reader decrypts by too (`read`'s docs, "What's encrypted"): the encryption dictionaries and what they refer to, by number, a cross-reference stream, a signature's `/Contents`, a stream whose crypt filter is `/Identity`, and the document's metadata stream's data under `/EncryptMetadata false`. An embedded file's data is encrypted by `/EFF`, and a stream's by its own crypt filter. A container is encrypted whole, as a stream, and `Encryptor::packs` says none is made where streams are in the clear and strings aren't. What can't be encrypted is refused, with nothing written in the clear (`write::Unencryptable`, `write::Error::Encrypt`). So is an `Encrypting` where `/EncryptMetadata false` leaves the metadata stream in the clear and the catalog, which says which stream that is, doesn't resolve.

The encryption dictionary and the trailer are written as they are, but for a direct `/Info`, whose strings the trailer would hold in the clear. That's written as an object at a fresh number, encrypted, and the trailer names it. An update appends, so an earlier trailer's `/Info` stays in the file as it was. Under a new encryption that encrypts metadata, the metadata stream's own `/Crypt` filter `/Identity`, which would leave it in the clear, is taken out as it's written. Written by pdf-objects' writer alone, past `write::Parts`, it's refused (`write::Unencryptable::Metadata`). Only strings and streams are encrypted (§7.6.2), so what a file held only inside an object stream is in the clear once written unpacked. A caller who minds packs (`Options::pack`). `write::encryption` writes the dictionary of pdf-crypt's values as they say it, its crypt filters by their names, so a dictionary read (`read::encryption`) and written back names what the file named.

A caller that lays a whole file out itself encrypts it as `write::full` does, through `Encryptor::laid_out`. It's given the renumbering the objects are written by (none for each at its own number), where the dictionary goes, the trailer, and what resolves in the numbers held. It gives the encryptor in the numbers written, made whole, with the metadata stream found and kept as its number written. `Encrypting::dictionary`, `at` and `first_id` say what to write of the dictionary, and `write::Parts::new` writes the bodies by it. The caller puts in no object stream what `Encrypting::dictionary_object` names, and numbers no other object there, as what's there is left in the clear (`packable` tells an encryption dictionary by its shape alone, not what one refers to). Where the dictionary goes, which numbers are listed free and which generations objects go at stay the caller's, and so does a direct `/Info`, which `write::trailer_entries` keeps in the trailer, in the clear, where a whole file writes it as an object. Under RC4, what it lays out at a keystream the file's sections show used is refused, as it reads no versions to tell a plaintext is unchanged, unless the encryptor is told to reuse keystreams (`Encrypting::keystreams_reused` lists them). Under a reading's encryption (`Encryptor::of`), a whole file of `write`'s lists each number the file read lists and it doesn't write free at its next generation, and moves what it writes anew under RC4, as below.

Under RC4 every string and stream of an object is encrypted from the start of one keystream, made of the file's key and the object's number and generation. Two plaintexts under one keystream leak their XOR. That happens where a changed object is written again at its number and generation, as an incremental update writes it, beside its earlier version. It happens too where an object is put at a number and generation another object had. So the writer puts no new plaintext under a keystream the file read shows used, counting every cross-reference section of the file. A whole file writes each object it writes anew at a generation its number hasn't used, or past every number where its generations are spent, its references with it. `Written::regenerated` says which it moved, and copies keep their own. An update writes a string or stream's data at a used keystream only where a version of that object the file holds has it there already, or starts with it. So a page tree's `/Count` changed is written, and a changed `/Contents` is refused (`write::Unencryptable::Keystream`). An update after another, given its `Written::base`, counts what that one wrote as used too. Told to (`Encryptor::reusing_rc4_keystreams`), the update writes it anyway and lists it (`Written::keystreams_reused`). Two whole files written from one reading put what each writes anew at the same fresh generations. Where both are kept and differ, their XOR leaks, so a caller saving twice opens the file it wrote before writing again. A file written whole under a new AES-256 encryption (`Encryptor::create`) leaks nothing.

## Exhaustive on purpose, or not

Nothing here is exhaustive on purpose. What a reader says of a file is found in files rather than designed, and what a writer refuses grows with what it writes, so every public enum is `#[non_exhaustive]`, a variant more a minor release and a match on one needing a wildcard: `Fault` and the reasons beside it (`read::Unwritten`, `read::ParseKind`, `read::RowShape`, `Unread`, `read::Undecoded`, `read::Unindexed`, `read::Unfiltered`, `read::SectionFault`, `source::Unsupported` and `source::Uncrypt`), `read::Error` and `write::Error`, `write::Unencryptable` and `write::KeptOut`, what a section, a record or an encryption says of an object or a file (`read::XrefEntry`, `Origin`, `XrefKind`, `Encryption`, `write::Entry` and `write::Body`), and whose recovery a reading takes (`read::Recovery`). So is every struct whose fields are public, as a part can join one: `Limits`, `Options` and `write::Options`, `write::Pack` and `write::Deflate`, each set on a copy of its `DEFAULT`; `Trailer`, `Revision`, `Update` and `write::Base`, made outside the crate by their constructors (`Trailer::new` and the rest), where a reader of its own fills them in, and `Facts` by its `Default`; `Record`, `DataExtent` and `write::Written`, made by the crate alone; and the sections' `read::Sections`, `SectionRows`, `SectionRow` and `FirstObject`, made by the reader alone. A variant's fields are the whole of what it says, so no variant of `Fault` is itself, and a fault is built by a literal, as a test compares with it, one that says more joining as a variant of its own; `XrefEntry::Free` and `Encryption::Opened` are, as they may come to say more of a free number and of how a file opened. The rest keep their fields to themselves: `Document`, `Reading`, `WithReading`, `Prefix`, `FileBytes`, `Identity`, `SourceId`, `RevisionEnd`, `write::Section`, `write::Encryptor`, `write::Encrypting`, `write::Parts`, and the iterators.

## What isn't here

- **Which objects, and at which numbers.** `write` writes what it's given at the numbers it's given, or that a renumbering gives them; which of a document's objects to write, and what to renumber them to, are a caller's. Under RC4 a whole file moves what it writes anew off a used keystream (`Written::regenerated`), and an update refuses an object at a number the file read lists free below its generation (`write::Error::Free`). An object as read (`Body::Read`) is given at the number it was read as, and copied from the reading given, renumbered in place where a renumbering moves it or what it refers to: one given at another number, or with another reading's record, refuses the write (`write::Error::Elsewhere`), however it's encrypted; a caller that moves objects from one file to another gives them as `Body::New`, unplaced (`Object::unplace`).
- **A generator for wasm32-unknown-unknown.** AES's IVs and a new file's key come from the operating system through pdf-crypt, which takes getrandom, and that target has none by default. A build for it picks getrandom's backend, its `wasm_js` feature for a browser or a custom one, as getrandom's docs say, or fails. With getrandom's `unsupported` backend, a reader that never writes encrypted builds, and every encrypted write is refused (pdf-crypt's `Random`).
- **A durable write, or an append in place.** `write` writes into a `Vec<u8>`; writing it to a file, replacing one whole or appending to it, and keeping another writer from changing it meanwhile (`Base::holds` says whether a file is still the one a base describes), are the caller's.
- **Faults told as they're met.** A reading keeps its faults and gives or tells them in the whole read's order when it's asked (`Source::faults`, `Source::tell_faults`), as reads on any thread, in any order, meet them in another; a sink a reading tells as each read meets a fault isn't taken.
- **Files for tests, and qpdf.** The tests' file writer, and qpdf (the program `PDF_LINT_QPDF` names, else `qpdf` on the `PATH`) run as a second reader and a decoder to set the reader's decoding beside, are pdf-fixtures, a crate of this workspace's that's never published, which the tests take by its path. This crate runs no program, reads no `PDF_LINT_QPDF` and writes no file. Its `insecure-test-writer` feature, off by default and its items hidden from the docs, makes the tests' choices reachable: a new encryption from a seeded generator, and each write's IVs fixed or derived. It's for tests, never a document's protection.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
