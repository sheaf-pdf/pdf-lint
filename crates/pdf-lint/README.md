# pdf-lint

Checks PDF files against the [Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model), the PDF Association's machine-readable grammar of ISO 32000-2: for every object, which keys it may have, their types, which are required, the values they may take, and the PDF version each came in or was deprecated in.

It's a command line and a library. It reads a file from its bytes, through [pdf-structure](../pdf-structure)'s reader, on [pdf-tokenizer](../pdf-tokenizer), so every finding says which bytes it's about. The reader decodes object streams and cross-reference streams itself, through [pdf-filters](../pdf-filters), and reads their objects from the decoded data, placed in it. What's encrypted it decrypts itself, through [pdf-crypt](../pdf-crypt), by the password it's given: an encrypted file's object streams, before they're decoded, and its strings where they are.

## What it checks

It walks every object reachable from the trailer, and then every object stream's dictionary, reached or not, from its number (nothing in the model links to one but another's `/Extends`), and what it refers to, and each earlier revision's trailer, as an incremental update leaves it (`trailer 1` the first's; a cross-reference stream from its number), and what's direct in it; picks the model's table for each one the way the PDF Association's TestGrammar does, and checks each key:

- its **type**, and whether it must be **direct** or **indirect**;
- whether a required key is **missing**, counting what a page inherits from its parents;
- whether a key or value is **newer** than the file's version, or **deprecated** in it (counted only, unless `--deprecations` makes it a finding);
- its **allowed values**, and the model's **special cases**;
- references that **don't resolve**, **name and number trees**, and keys the model **doesn't list**;
- a key written **twice** in one dictionary.

What an object stream holds that it mustn't (ISO 32000-2, 7.5.7) is a finding too, `packed`, in the object stream, from its number, its table and key `ObjectStream` and `-`, at the object's bytes in its data: a stream (its dictionary, as an object in one is read, followed by the keyword `stream`), the document's encryption dictionary (what a revision's trailer names as `/Encrypt`), the value of an object stream's `/Length`, a linearization dictionary, and, in a file whose linearization holds, its catalog and pages. And so are, in a file that encrypts its strings, the strings an object stream leaves in the clear, its data not encrypted (`/Identity`): counted, at the first. An object of a generation other than 0, which 7.5.7 keeps out too, can't be read from one, as its entry has no generation.

What the reader had to choose how to read is a finding too: a stream whose `/Length` doesn't end at `endstream`, a lone CR after `stream`, an object without `endobj`, a name with a `#` not followed by two hex digits or with `#00`, a cross-reference table whose rows aren't the 20 bytes ISO 32000-2 writes, a file whose last line isn't `%%EOF` alone, and an object an object stream's header starts inside the bytes of another read from it. A rule whose predicate the checker can't evaluate yet, or that needs a string's bytes the reader didn't give, or, in an earlier revision's trailer, an object a later revision wrote over or freed (what that revision had there isn't read), is counted as not checked, and never guessed. Predicates' arithmetic is evaluated, and a key an object lacks counts as its default value in a SpecialCase or PossibleValues, as readers take it (an image without `/ImageMask` is no mask), but not in Required. Where the model says readers ignore a key (`fn:Ignore`, or `fn:IsMeaningful` when its condition doesn't hold), it's counted apart, as `ignored`, and never a finding.

### Fields merged with their widgets

A field with a single widget may be one dictionary with it (ISO 32000-2 12.5.6.19), which the model splits between two tables: the widget's lists the keys every field has, but not a field type's (`/FT`, `/V`, `/DA`, `/MaxLen`...), and a field's tables none of a widget's. By default such a dictionary is checked as both at once, the field's table by its `/FT` and `/Ff`, its own or inherited, and so are the triggers of its `/AA`, a widget's and a field's: each key against each table that lists it, and unlisted only if neither does. A key of an object reached again as another table is unlisted only if that table doesn't list it either. `--one-table` checks as TestGrammar does, each object as the first table that reaches it: on forms, it counts many more keys the model doesn't list, nearly all of them fields' keys on widgets.

### Extensions

A file is checked under the extensions it declares, as the model names them (`Report::extensions`, `extension::declared`): Adobe's extension level 3 or later at PDF 1.7 (`ADBE_Extn3`) and ETSI's (`ETSI_PAdES`), from the catalog's extensions dictionary (ISO 32000-2, 7.12); ISO's, there under `/ISO_` with the standard's number; and PDF/A-3 (`ISO_19005_3`) and PDF/VT-2 (`PDF_VT2`), from its XMP metadata, when that's unfiltered and unencrypted. What's under any other extension is as if the model didn't have it, as in TestGrammar without the extension in its `--extensions`. `--extensions` names more for every file, as TestGrammar's does: the model's names, a comma between (`Malforms,AAPL`); a name it hasn't is refused, with those it has. Unlike TestGrammar's, it has no `*` for all of them: each is a claim about the file, and no file is PDF/A-3, PDF/VT-2 and PDF/X at once. That's the way to the keys no file can declare, Apple's (`AAPL`), OpenOffice's (`OpenOffice`), and what the model takes as known malformations (`Malforms`). `--full` and the JSON say which extensions a file was checked under.

## The byte-for-byte record

Each finding carries where its bytes are: a key and its value, an array's element, the dictionary that lacks a key, the object nothing reaches. The command line prints them under it. From a file made to show it:

```text
example.pdf: PDF 1.7, 6 objects checked, 2 findings
- type: PageObject/MediaBox at Root/Pages/Kids[0], in 3 0 R (merged): [0 0 612] [bytes 163..182]
    | /MediaBox [0 0 612]
- duplicate: PageObject/Rotate at Root/Pages/Kids[0], in 3 0 R (merged): 90 (its key written again later) [bytes 152..162]
    | /Rotate 90
```

## Checking only what you wrote

Most PDFs in the wild break some rule somewhere, so for a tool that edits files, a report on everything is noise.

- **`--hold objects.txt`** holds exactly the objects your tool says it wrote, a line each: `12 0 built` for one it built whole, `3 0 merged` for one it emitted over an object it read. A key the model doesn't list is a finding only in a built object.
- **`--exempt keys.txt`** names the keys your tool writes on purpose though the model doesn't list them, so that they're no findings in a built object: a name a line, as a PDF file writes it (`/MyKey`), or with a `*` after it for every key it begins (`/My*`).
- **`--last-update`** holds only the objects the newest incremental update wrote, for an incremental writer, with no earlier file needed: one it rewrote as merged, a new one as built. A finding in a rewritten object, or in the trailer, is left out when the same object, or the trailer, had a like one in the revision the update follows, which is the file read from the cross-reference section before the newest (`bytes::read_from`), checked at the file's version. An earlier revision's trailer isn't the update's, and isn't held; the revision before's are its own, and carry none of the update's trailer's findings, so a fault the update brings back, which the revision before had mended, is the update's.
- **`--baseline in.pdf`** reports only the findings `in.pdf`, the file each was written from, didn't already have. A file that begins with `in.pdf`'s bytes is an incremental update of it, and is set against it object by object, as `--last-update` is. Any other is a rewrite, which renumbers objects but leaves most where they were: a finding is carried when `in.pdf` had a like one at the same path (`Root/Pages/Kids[3]/Annots[0]`), or, where `in.pdf` has nothing at that path, as where a page moved, anywhere in it. A path from an object's number, an object stream's (`12 0 R`) or an earlier cross-reference stream's, is the number a rewrite renumbers, so a finding there is carried by a like one anywhere in `in.pdf`.
- **`--unlisted all`** makes a key the model doesn't list a finding in a merged object too, where by default (`built`) it's counted only, as it may be the source's. With `--baseline` or `--last-update`, a merged object's unlisted key the earlier file already had is carried, so what's left is the keys your tool added to objects it merged. `--unlisted never` counts them only, even in built objects. Second- and third-class names (ISO 32000-2 Annex E) are never findings.

Findings are alike when their table, key, rule and value are, with references' numbers left out of the value, and for an object nothing reaches, the object alone. A value is a summary for that: a dictionary's keys in order, a stream's encoding left out, so that it's the same after a rewrite that sorts keys or encodes streams anew. The bytes as written are where its span is.

The library has the rest: `Report::since` matches by number (an earlier revision's trailer by any trailer before, `Finding::revision`) and `Report::by_path` by path (its baseline checked with `Options::paths`), `Report::beyond` leaves out what the sources of a merged object already had in any object, and `Options::exempt` names a writer's own private keys, which `Exempt::parse` reads from a file as `--exempt` does.

## The command line

```text
Checks each FILE against the Arlington PDF Model, reading it from its
bytes, and prints each finding with the bytes it's at. It decodes object
streams and cross-reference streams, and decrypts what's encrypted,
itself.

Usage: pdf-lint [OPTIONS] <FILE>...

Arguments:
  <FILE>...  a file to check; - is standard input

Options:
      --hold <FILE>      hold only the objects FILE names, a line each:
                         `12 0 built` or `3 0 merged`
      --built            hold every object as built, so that a key the model
                         doesn't list is a finding (by default, as merged)
      --exempt <FILE>    keys a built object may have though the model doesn't
                         list them, a name a line: `/MyKey`, or `/My*` for every
                         key it begins
      --unlisted <W>     which held objects a key the model doesn't list is a
                         finding in: built (the default), all, or never; it's
                         counted in any
      --deprecations     find what the version checked at deprecates: a key, a
                         type or a value (by default, counted only)
      --extensions <E>   check each FILE under these extensions too, besides
                         those it declares: the model's names for them, a comma
                         between (Malforms,AAPL)
      --last-update      hold only the objects the newest incremental update
                         wrote: those it rewrote as merged, new ones as built;
                         and leave out what a rewritten object, or the trailer,
                         already had in the revision it updates
      --baseline <FILE>  leave out the findings FILE, the file each FILE was
                         written from, already had: an incremental update of it
                         object by object, a rewrite of it by where each finding
                         is, or, where FILE has nothing there, anywhere in it
      --pdf-version <V>  check each FILE as PDF V, 1.0 to 1.7 or 2.0, not as
                         the version its header and catalog give, and what it's
                         set against, a baseline or the revision before an
                         update, as V too
      --model <DIR>      check against the tables in DIR, a checkout of the
                         Arlington model's tsv/latest, as they are, not the
                         embedded ones, whose cells that contradict ISO 32000-2
                         are corrected
      --one-table        check each object as one table, as TestGrammar does:
                         a field and its widget merged in one dictionary as
                         one of the two (by default, as both), and a key
                         unlisted by the first table that reaches an object
                         though another that does lists it
      --allow <TSV>      leave out the findings TSV allows: a # header, then
                         table, key, rule, file, version and reason, tab apart;
                         a version, as the file is checked as
      --full             print the whole report: tables, checks not made, keys
                         the model doesn't list
      --password <PW>    the files' password, user or owner, when it isn't
                         empty: tried in its SASLprep form (RFC 4013) and as
                         UTF-8, then in PDFDocEncoding, WinAnsiEncoding and
                         MacRomanEncoding
      --jobs <N>         check N files at once (by default, as many as there
                         are CPUs); the report is in the files' order all the
                         same
      --format <F>       text (the default), or json: a line of JSON for each
                         file, with its findings and the whole report's counts
  -V, --version          print the version, and the model's commit,
                         corrections, additions and hash
  -h, --help             print this

It exits 0 when every file is clean, 1 when there are findings, and 2
when a file can't be read or the command line is wrong.
```

A file that can't be read doesn't stop the others being checked. An encrypted file that won't open with the password given, none or `--password`'s, is one that can't be read, and says so: what its object streams hold would be missing, and its strings ciphertext. `--password` is tried as the owner's and the user's: in its SASLprep form (RFC 4013), which revisions 5 and 6 take, where that isn't the password as typed; as typed (UTF-8), as a writer that skips SASLprep sets it; then, composed (NFC), in each single-byte encoding that has every character of it, as a writer may have set it for revisions 2 to 4; those the file's revision takes first. The empty password isn't tried once one is given. A file that encrypts only its embedded files (its strings and streams `/Identity`) needs none. One whose security handler the reader can't open (the public-key handler, or a dictionary the standard handler doesn't take) is checked with its strings ciphertext and its object streams unread, a `reader` finding saying why. A FILE of `-` is standard input, named `-` in the report and in an allowance. A finding in an object stream is printed with its bytes from the stream's decoded data. With `--pdf-version`, the summary says the version a file was checked as beside its own, and an allowance's version is matched against the one it was checked as.

With `--format json`, each file's report is a line of JSON. Every line says which version of pdf-lint wrote it and against which model, so a report can always be read against what made it. It has:

- `pdf_lint`, the version of pdf-lint that wrote it, and `file`;
- how it was checked: `version`, `checked_as` (`--pdf-version`'s, else `version`), `one_table` (whether `--one-table`), `unlisted_in` (`--unlisted`'s), `deprecations` (whether `--deprecations`), `extensions` (those it was checked under), and `model`, the model's hash (XXH64 of its tables' files, as `Grammar::hash` says, 16 hex digits, which `--version` prints for the embedded one);
- `objects` and `keys`;
- `findings`, each with its `rule`, `table`, `key`, `path` (where the walk reached it: from the trailer, `trailer` for it; from an earlier revision's, `trailer 1` for the first's; from an object stream's number, or an earlier revision's cross-reference stream's, `12 0 R`, a `packed` finding's too; `-` for an object nothing reaches), `object` (`[n, g]`, or null for a table's trailer), `made`, `value`, and `at` (`stream`, `start` and `end`, or null);
- how many findings were left out as `carried` and as `allowed`;
- the counts `--full` prints: `tables`, `not_checked`, `unlisted`, `deprecated` (keys the version checked at deprecates, or deprecates the type or value of, by table and key), `ignored` (keys the model says readers ignore there), `no_table`, `ties`, `truncated`, `two_contexts` and `two_tables` (objects checked as two tables at once, counted in `tables` as each, and once in `objects`).

A file that can't be read is a line of `pdf_lint`, `model`, its `file` and the `error`.

## Installing

Each [release](https://github.com/sheaf-pdf/pdf-lint/releases) has an archive for Linux (x86_64 and aarch64), macOS (Apple silicon and Intel) and Windows (x64): `pdf-lint`, which decodes and decrypts by itself, needing nothing beside it, the Arlington model's notices, and the notices of what's compiled in. Unpack it anywhere. (0.1's archives held qpdf too, in `qpdf/`, which it decrypted with.)

The Linux binary is static, needing nothing of the system; the Windows binary has the C runtime linked in, so it needs Windows 10 or later and no Visual C++ Redistributable (one `cargo install` builds links it dynamically, unless `RUSTFLAGS` has `-C target-feature=+crt-static`). On macOS, an archive a browser downloaded is quarantined, and its binaries won't run unsigned until `xattr -dr com.apple.quarantine` clears the folder; one fetched with `curl` or `gh release download` isn't.

From source, `cargo install --locked pdf-lint` builds the binary from crates.io, with Rust 1.88 or later, and `cargo install --locked --path crates/pdf-lint` from a checkout; neither needs qpdf. `--locked` builds with the versions of its dependencies the release was built with, whose clap prints the help as above; without it, cargo takes the newest that fit, and a newer clap may lay the help out otherwise. `just dist` builds this platform's archive, as the release workflow builds each. A crate that uses the library alone can depend on it with `default-features = false`, leaving out the `cli` feature: the binary and what only it needs, clap, serde, serde_json and anyhow.

## Readers

The checker reads a file through a `Source`:

```rust,ignore
pub trait Source {
    /// Every object its cross-references list.
    fn ids(&self) -> Vec<ObjectId>;
    /// Object `id`, as written: references kept as references. The error
    /// says why there's none: the file hasn't it, or it doesn't read.
    fn get(&self, id: ObjectId) -> Result<&Object, Unread>;
    /// The trailer: the dictionary after `trailer`, or the cross-reference
    /// stream, and why it isn't as the newest section writes it, if it isn't.
    fn trailer(&self) -> &Trailer;
    /// The version in the header.
    fn header_version(&self) -> Option<Version>;
    /// What the reader had to choose how to read.
    fn warnings(&self) -> &[Warning];
}
```

Its other methods are for what a reader may know, and default to nothing: the file's size (`fn:FileSize`); the catalog's XMP metadata, when it's in plain view (the extensions a file declares); and where each object was read, an object stream's decoded data, the file's revisions, what its header and sections say of it, and whether an object stream leaves its objects' strings in the clear, which the earlier revisions' trailers and `packed` read. Why an object doesn't read is a `source::Unread`, a variant for each reason the bytes reader gives; a reader of its own says why in its own words with `Unread::Other`.

The objects are the [pdf-objects](../pdf-objects) crate's types, which pdf-lint re-exports as its `object`, `text` and `version` modules and at its root: an `Object` is a `Value` and where it was written, a `Span`, when the reader knows and no edit has changed it since: an edit, through `value_mut` or an `as_*_mut` accessor, drops the span of what it changed and of what holds it. A stream's data is a `Data`, where it was written or its bytes. A `Dict` keeps its entries as written, in order and with any key written twice, and each key's `Span` too. A string is `Str::Bytes`; or `Str::Text`, text a reader decoded without saying from which encoding; or `Str::Encrypted`, ciphertext a reader didn't decrypt. Beside it, a string keeps its `Form`, literal or hex, as written, when the reader says, which `fn:IsHexString` reads. Where a reader loses something, the checker counts what depends on it as not checked. `Document` is a `Source` read whole, which readers fill, tests build, and a caller builds and edits in memory.

`Source`, `Document` and the reader, `bytes::read` and its kin, are the [pdf-structure](../pdf-structure) crate's, which pdf-lint re-exports as its `source` and `bytes` modules, and `Source`, `Document`, `Trailer`, `Warning`, `Update`, `Revision` and `Encryption` at its root; pdf-filters, whose error a reading gives for a stream's data that doesn't decode cleanly, as `pdf_filters`; and pdf-crypt, whose `Password` the reader is given, as `pdf_crypt`.

**`bytes::read`** reads a file from its bytes: the header, `startxref`, the cross-reference tables and streams along their `/Prev` chain, and each object at its offset, through pdf-objects' parser, a stream's data by its `/Length` when that ends at `endstream`. Where the sections don't read, it finds the objects by scanning, and the trailer says so.

**`bytes::read`** decodes cross-reference streams and object streams from their own data through pdf-filters. Their `/Filter` and `/DecodeParms` are read as qpdf reads them: the abbreviations `/Fl`, `/AHx`, `/A85`, `/LZW` and `/RL` on any stream, `/Crypt` as no filter (decrypting comes before the filters, by its `/Name`), a parameters dictionary that isn't in an array every filter's, a reference to no object as null. A stream decodes to at most `bytes::STREAM` bytes (256 MiB), and the streams of a reading of a file to `bytes::BUDGET` (1 GiB) in all; and it reads no more cross-reference entries than the file has bytes, or 2²⁰ if that's more: objects may be numbered sparsely, but a small stream can't list hundreds of millions. A filter it doesn't decode, or parameters it doesn't take, leave the stream undecoded, saying why; data a fault stopped is read as far as it decoded, but for a number, name or keyword the cut ends, which may be cut short. Each fault is a warning on the stream, so a `reader` finding: at the byte where the first filter met it, else at the stream's data. The objects it reads from an object stream are placed in its decoded data (`Span::stream`), which the `Document` keeps.

**`bytes::read_opened`** opens an encrypted file with the passwords it's given, each a `Password` as typed, tried in each of its encodings (pdf-crypt's `Password::encodings`) as the owner's and then the user's, every password's that fit the file's revision before any password's others, in the empty password's place (none, the empty password; a wrong one leaves it locked, or plain where its strings and streams aren't encrypted), through pdf-crypt's standard security handler, revisions 2 to 6: its object streams are decrypted before they're decoded, and its strings decrypted where they are, their bytes exact, but a signature's `/Contents` (a dictionary with `/ByteRange` and a string `/Contents`, of `/Type /Sig`, `/DocTimeStamp` or none), which isn't encrypted, and the encryption dictionary's, any trailer's `/Encrypt`; what it decrypts and what it leaves as written is listed in full in pdf-structure's `bytes` documentation, "What's encrypted". A string or stream that doesn't decrypt cleanly is a warning, once an object and a fault; `/Perms` that doesn't match, or doesn't confirm the key, is one on the encryption dictionary. `Document::encryption` says whether the file is encrypted, and whether it opened, by the owner's password or the user's, and which of those it was given in which encoding (`by`), or is locked (no password it was given opens it), or has a handler it can't open, and why: then its strings stay ciphertext and its object streams aren't read. Or it's plain: no password opened it, or the reader can't open its handler, but its strings and streams aren't encrypted (`/StrF` and `/StmF` `/Identity`, as when only its embedded files are), so it reads whole, but for an object stream whose own `/Crypt` filter encrypts it, which isn't read, saying why. `bytes::read` is `read_opened` with no password, so the empty one. **`bytes::read_from`** reads the file as of a cross-reference section, along its `/Prev` chain as if it were the newest, so that what later updates wrote isn't in it, as `--last-update` reads an update's revision before; `bytes::read_from_opened` is `read_from` by passwords.

### Two readers, set side by side

The tests set the reader beside qpdf, as an oracle, through a decoder and a second `Source` of their own (pdf-structure's tests, and pdf-lint's `tests/qpdf.rs` for the findings), which pdf-structure keeps behind its hidden `fixtures` feature, for tests, and which nothing in pdf-lint's binary or library reaches: qpdf run as a process, its JSON read leniently (qpdf writes a real such as `+.5` into it unchanged). The reader can be made to ask it for every stream it would decode itself, and its object streams' data and objects are then compared byte for byte; and qpdf reads a whole file alone, without spans, a string it reads as text coming back as `Str::Text`, whose encoding qpdf doesn't say. They run against qpdf-written files committed in pdf-structure's `tests/files`, which pdf-lint's tests read and check without qpdf too, and, ignored, against a folder of files (`PDF_LINT_CORPUS`).

Set side by side, the two readings agree on nearly every object, and every difference has a cause:

- qpdf leaves out a key whose value is null or refers to no object, which the spec reads as absent, and writes such a reference as null in an array. Most differences are this.
- qpdf lists the cross-reference streams it read and the object numbered `/Size` − 1 though no section lists them; reads an in-use entry at offset 0 as null, or finds the object elsewhere; invents keys (`/QPDFFake1`) in a dictionary it can't read; reads a string after `FF FE` as UTF-16LE, where the spec reads PDFDocEncoding; decrypts an older cross-reference stream's `/ID`, which isn't encrypted; and won't read a file whose page tree has no `/Kids` (the bytes reader does, its object streams decoded from their bytes).
- Where either reader repaired a file, each repairs it its own way; and where two cross-reference streams have one number, as in one linearized file, each takes another.

Checked, the two readings give the same findings but for those causes, and for one in the checker: an object is checked as the table it's first reached as, and with keys in another order, qpdf's sorted, another reference can reach it first.

The comparison found three faults in the bytes reader, now fixed: a table's row with a number out of range lost the whole table (it's read as free, and the rest read); `/Encrypt null` was taken as encrypted; and a cross-reference stream decoded by its number could be another object's.

Another ignored test, `a_corpus_decodes_alike_through_qpdf`, sets the reader's decoding beside qpdf's: every stream with filters the reader decodes, decoded both ways (qpdf at `--decode-level=specialized`, so RunLength too) byte for byte, and the object streams' data and objects as the reader reads them and as it reads them asking qpdf. Each difference is sorted into a kind the test explains, and it fails on one it can't: the same bytes, the reader saying a fault qpdf passes over; data a fault stopped, which qpdf gives undecoded and the reader decodes as far as the fault; a stream qpdf finds by its number and the reader by its place, which an update can make two (the test asks qpdf for the raw bytes it read to tell); the reader's limit. A test of hand-made streams pins where the two part on purpose, as qpdf departs from the spec: a run-length of 128 ends the data, NUL is white-space between hex digits, TIFF's samples are 1, 2, 4, 8 or 16 bits.

A third, `a_corpus_decrypts_alike_through_qpdf`, sets the reader's decryption beside qpdf's in every encrypted file of a folder, opened with the empty password or one the test suites use: whether it opens, is locked, or has a handler that can't, against qpdf's `--requires-password`; and every stream decrypted both ways, byte for byte, and decoded. Each difference is again sorted into a kind it explains: a string or stream that's just an IV, a part block, a `/Crypt` filter without `/Name` (the spec's `/Identity`, which qpdf decrypts by `/StmF`), a file qpdf refuses whole.

### How qpdf and lopdf map onto it

The tests' qpdf reader is the `Source` through qpdf. lopdf won't be shipped. It's here to show what implementing the trait for a Rust reader takes.

| `Source` | qpdf 12, as a subprocess | lopdf 0.46 |
|---|---|---|
| `ids` | The objects of `qpdf --json=2 --json-key=qpdf --json-stream-data=none`, `"obj:n g R"`. Free entries aren't listed | `doc.reference_table`: `XrefEntry::Normal`, `Compressed`, `Free` and `UnusableFree` |
| `get` | `"obj:n g R"`'s `value`, or its `stream.dict` | `doc.objects` |
| `trailer` | The JSON's `"trailer"`. For a cross-reference stream it's that stream's dictionary, and the stream is the object whose dictionary it is | `doc.trailer`, without `/Encrypt`, which loading removes |
| `header_version` | The JSON header's `pdfversion` | `doc.version` |
| `warnings` | stderr lines: `WARNING: file (object n g, offset k): message` | None directly. Recoveries go to the `log` crate as `warn!` records |
| Decryption | `--password` | `Document::load_with_password` |

| `Value` | qpdf's JSON | lopdf's `Object` |
|---|---|---|
| `Null`, `Bool`, `Integer` | `null`, `true` and `false`, an integer | `Null`, `Boolean`, `Integer(i64)` |
| `Real` | A number with a point, read as text: the digits are kept, but `-.5` comes as `-0.5` | `Real(f32)`, widened, so `0.1` is no longer exactly `0.1` |
| `Name` | `"/Name"`, `#xx` escapes decoded, or `"n:/Name"` with them, for a name that isn't UTF-8 | `Name(Vec<u8>)`, decoded |
| `Str::Bytes` | `"b:"` and hex, the string's form unsaid | `String(bytes, format)`, always, its form the format |
| `Str::Text` | `"u:"` and text: ASCII or UTF-16BE, without saying which | Never |
| `Array` | An array | `Array` |
| `Dict` | An object with its keys sorted. Of duplicate keys, the last is kept, with a warning | `Dictionary`, in written order. Of duplicate keys, the last is kept, silently |
| `Stream` | `{"stream": {"dict": …}}`, as written, but a `/Length` qpdf recovered | `Stream`, but its `/Length` is rewritten as a direct integer when the file is read |
| `Ref` | `"n g R"` | `Reference((n, g))` |

What each loses, and the checker then can't see: for qpdf, which encoding a text string had, whether a string was written literal or hex, the order keys were written in, any key written twice, names' escapes, keys whose values are null or refer to no object, and where anything was written; for lopdf, a stream's `/Length` as written, the trailer's `/Encrypt`, structured warnings, reals' precision, and where anything was written. `bytes::read` loses none of it, for what's in plain view.

## The model

The model's `tsv/latest` is embedded at a pinned upstream commit, which `--version` prints, so `cargo install` needs no checkout: by [pdf-arlington](../pdf-arlington), whose tables and predicates the checker reads, re-exported here as `grammar` and `predicate`. Which model every check reads, the embedded one or another (below), is this crate's to choose (`grammar()`, `use_model`). The files in its `model/tsv/latest` are upstream's byte for byte, but the embedded copy has the cells corrected where the model contradicts ISO 32000-2 or the reference it cites, as its `model/corrections.tsv` lists them (`grammar::corrections`), and the rows and tables added that a reference defines and the model lacks, as its `model/additions` has them, in upstream's format (`grammar::additions`), each with the clause it follows. The build stops when upstream has changed a corrected cell, or has an added row itself. So far:

- the button field tables require `/DA`, which ISO 32000-2 requires only of fields containing variable text (Table 228);
- a hide action's string, and each in its array, must be `fn:IsFieldName`, a valid partial field name, which has no period, where ISO 32000-2 makes it a fully qualified one (12.6.4, 12.7.4.2). Corrected, it's `fn:IsFullyQualifiedFieldName`, which pdf-arlington defines, not the model: the string is the fully qualified name of a field in the file's form, its partial names joined by periods, so a hide action naming no field is found;
- a stream's `/Filter` asks that `/DecodeParms` be as long as it, which can't be told when `/DecodeParms` is absent, as ISO 32000-2 allows when no filter has parameters other than its defaults (7.3.8.2); corrected, it asks only when `/DecodeParms` is there;
- an image's `SMaskInData` has no default in the model, where ISO 32000-2 gives 0 (8.9.5);
- a variable-text field's own `/DR`, which PDF 1.4 defines (Table 8.51) and the model has no row for, is added to the text and choice field tables, obsolete from PDF 1.5 (the 1.7 reference's implementation note 118), so deprecated there (counted only, unless `--deprecations` makes it a finding);
- the PDF 1.0 `Encoding` resource, a dictionary of named encodings as Acrobat writes one in a form's `/DR`, is linked to `EncodingMap`, an added table of them, where the model checks it as one encoding.

`--model <DIR>` checks against a `tsv/latest` checkout as it is instead, upstream's own or a newer one, which must read whole: every row, and every table a row links to (`Grammar::load`), and the five tables the checker reads by name (`use_model`, `grammar::NAMED`). The JSON report's `model` tells them apart: the embedded model's hash is of its tables with the corrections made, and a checkout of the same commit has upstream's.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)).

The Arlington model's files, which pdf-arlington embeds from its `model/`, are Apache-2.0 too, with upstream's LICENSE and NOTICE.txt beside them there, in pdf-arlington's package. A release archive carries the NOTICE.txt as `ARLINGTON-NOTICE.txt`, with what the embedded copy changes and adds as `ARLINGTON-CORRECTIONS.tsv` and `ARLINGTON-ADDITIONS/`. What's compiled into the archive's binary has its notices beside it: `THIRD-PARTY-NOTICES.txt`, each crate with its licence and its package's licence files, written for each platform from `Cargo.lock`; `RUST-NOTICES.html`, the Rust standard library's; and on Linux, `MUSL-COPYRIGHT`, the C library's, linked in statically. Parts of the checker are adapted from TestGrammar (Apache-2.0, Copyright 2020-2022 PDF Association), in the same upstream repository and under the same NOTICE.txt, and a header in `src/check.rs` names them and carries the NOTICE's notices.
