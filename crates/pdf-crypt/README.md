# pdf-crypt

PDF's standard security handler, in one small Rust crate on RustCrypto's primitives: a file's key from its encryption dictionary and a password, and its strings and streams decrypted with it, or encrypted for a writer; and a new file's key and encryption dictionary, from the operating system's randomness (ISO 32000-2, §7.6).

It's made for a reader that checks files as much as for one that shows them. Ciphertext that isn't what the handler wrote still decrypts as far as it can, and every fault is said.

## What it does

- **Every revision of the standard handler:** 2 to 4 (RC4 with keys of 40 to 128 bits, and AES-128 through crypt filters in version 4), 5 (AES-256, as Adobe's Extension Level 3 has it, which ISO 32000-2 leaves out) and 6 (AES-256, ISO 32000-2), opened by the owner's password or the user's, the first of several that opens it.
- **A password as typed, and the bytes tried** (`Password`, `Encoded`, `Encoding`): a password's text in its SASLprep form (RFC 4013) as of Unicode 3.2, which revisions 5 and 6 take, where that isn't the text as typed (`Encoding::Saslprep`); its SASLprep form by a later Unicode, where that's another again, as writers whose SASLprep normalizes by a later Unicode set it (`Encoding::SaslprepCurrentUnicode`); as typed, in UTF-8, as writers that skip SASLprep set it; then, for revisions 2 to 4, whose passwords are bytes, the text composed (NFC) in PDFDocEncoding, WinAnsiEncoding and MacRomanEncoding where each has every character of it, as qpdf tries them (`Password::encodings`); or bytes as they are (`Encoded::verbatim`). A password SASLprep refuses (an emoji, a control character) has the rest alone. Each is tagged with its encoding, and wiped when it's dropped. The handler tries those whose encoding fits the file's revision first, the rest after, and none twice where the revision takes two alike (padded or cut to 32 bytes, or cut to 127), as they'd give one key; it says which opened it (`opened_by`).
- **A new file's password, strictly** (`Password::prepared`): its SASLprep form, 1 to 127 bytes, or why it can't be one (`Unusable`): a character SASLprep doesn't allow, named (one Unicode 3.2 hadn't assigned among them, though a later Unicode decomposes it), right-to-left text it doesn't allow, a password that's empty, that prepares to nothing (each of its characters one SASLprep maps to nothing) or to over 127 bytes, which a reader would cut, or whose SASLprep forms by Unicode 3.2 and by a later Unicode differ, named at the character where they part, as readers would prepare it apart.
- **SASLprep as of Unicode 3.2,** which SASLprep is of (RFC 3454, sections 4 and 7). The `stringprep` crate prepares a password by the Unicode `unicode-normalization` has, and its form is taken back to 3.2's as UAX #15 has it done (section 11.3, Guaranteeing Process Stability): the five characters Unicode's Corrigendum #4 remapped in 4.0 (U+2F868, U+2F874, U+2F91F, U+2F95F, U+2F9BF) mapped first to the decompositions 3.2 gave them, a character 3.2 hadn't assigned refused, and the text composed by 3.2's definition of a blocked character (UAX #15 3.2.0's D2), which Corrigendum #5 changed in 4.1, so that a starter composes with the one before it across a mark (U+1100 U+0300 U+1161 is U+AC00 U+0300 by 3.2). That table of five and that rule are this crate's; `stringprep` does the rest. U+F951, Corrigendum #3's, was corrected in 3.2 itself. The sequences Corrigendum #5 changed (UAX #15's Table 10) are in no language's text, and the five characters are rare variants of others.
- **A new file's encryption** (`Handler::create`): a revision 6 encryption dictionary (AES-256, by Algorithms 8, 9 and 10), and the handler that encrypts under it, protected as a `Protection` says: an open password (the user's), a permissions password (the owner's) or both, each prepared; the permissions, what one who opens the file by the open password may do; and whether its metadata is encrypted. With both, each opens the file, the user's under the permissions, the owner's with every one. With the user's alone, the owner's is the user's, so the one password opens the file as the owner's; the permissions are then every one, or the file is refused (`Unusable::Shared`), as permissions withheld would bind no one who can open it. With the owner's alone, the user's is the empty password, so anyone opens the file under the permissions and the owner's lifts them: a file that limits what's done with it, not who reads it, as a security dialog's permissions password alone makes one. `Handler::open` opens it given no password, or the empty one, and refuses it given others alone that aren't the owner's, as it tries only what it's given: a reader tries the empty password first, as ISO 32000-2 has it (7.6.4.1), before it asks for one. With neither, it's refused (`Unusable::NoPassword`), as a protection left unset, whose file anyone would open as its owner; the owner's alone with every permission, which anyone opens with every permission too, is taken, as a password was given. `/P` is the permissions as revision 6 writes them, bit 10 set whatever's asked, as PDF 2.0 has a writer set it (every one is `/P -4`), and `/Perms` confirms it. A password in another encoding is refused (`Unusable::Unprepared`), as is SASLprep's form that a reader tries but a new file can't take: empty, over 127 bytes, or 3.2's form of a password whose later Unicode's form is another, where it isn't a password prepared itself. The file's key, the user's salts, the owner's and `/Perms`' random bytes are drawn from the operating system's random number generator (getrandom's). If it fails, nothing is made (`Unusable::Random`).
- **Permissions, said** (`Permissions`, `Printing`, `Changes`): a set of the eight permissions `/P` has a bit for (ISO 32000-2, Table 22), each a constant (`Permissions::PRINT`, `COPY`, `FILL_FORMS`, ...), joined by `|`; a security dialog's levels reachable from them, printing's three (`Printing`: denied, low and high resolution) and changes' five (`Changes`: none, assembling, filling in forms, commenting and filling in, any), each of those granting the one before it and more, as qpdf's `--modify` levels set the bits; and `/P` as a new file writes it (`Permissions::p`) and as a reader reads it (`Permissions::from_p`), by its revision's meaning of the bits: revision 2 has four of them, and the other four follow the bits that cover them there. A handler says the permissions of the file it opened (`Handler::permissions`), for a reader that shows them, as a document's properties do: `/P`'s, where `/Perms`' copy disagrees, as qpdf reads them, a fault said. A set's `Debug` names the bits, `Permissions(PRINT | COPY)`, and its `Display` says them in words, `print, copy`.
- **Values, not dictionaries.** A `Dictionary` is the encryption dictionary's values, typed; reading them from a file is the caller's, so it works with any object model. Another security handler's is refused as that, by its name (`Refused::OtherHandler`: the public-key handler's, say, whose files open by a certificate), and a version and revision the standard handler isn't by the pair (`Refused::Version`, a later handler's revision told by `r`); one without `/Filter`, or without `/O`, `/U` or `/P` of its type (or of version 5, `/OE`, `/UE` or `/Perms`), is refused as a dictionary that's wrong, saying why (`Refused::Malformed`); short values are filled out with zeros, and version 5's long ones cut short, as qpdf reads them.
- **Crypt filters** by name and method (`/CF`, `/StmF`, `/StrF`, a stream's own `/Crypt`): `/Identity`, `/None`, `/V2`, `/AESV2`, `/AESV3`, and one that isn't, said.
- **Every fault said, what decrypted kept:** AES ciphertext with no whole block past its IV (shorter than an IV, it stays ciphertext), a part block past the whole ones, padding that isn't, a crypt filter's method that isn't one, AES that the file's key isn't for, and `/Perms` that doesn't match or doesn't confirm the key (the file opens all the same, by another password's key when one is confirmed). An error's `Debug` gives what decrypted by its length, not its bytes, so a log or a failed `unwrap` never prints plaintext; its `data` is public, for a caller that wants them.
- **Encrypting for a writer** (`encrypt_string`, `encrypt_stream`): as the handler decrypts, by the same crypt filters, a stream's own included. AES's IV is the next of a write's `Ivs`: 16 bytes from the operating system's random number generator, drawn 4 KiB at a time and shared by the write's threads. Equal plaintexts encrypt apart, and an IV holds nothing of the key. If the generator fails, the string or stream is refused (`Fault::Random`), never written in the clear. RC4 takes no IV, `/Identity` and `/None` leave the data as it is, and a method that isn't one is refused. Under RC4 every string and stream of an object is encrypted with one keystream, from the file's key and the object's number and generation, so an object written again at its number and generation with a string or stream changed, as an incremental update writes a changed object, XORs with its earlier version to the XOR of the two plaintexts: what changed leaks to one who holds both, as an update's file does, and the earlier file does after a whole save under the same key and `/ID`. So does an object a whole save or an update puts at a number another object had, as a writer that renumbers does: it's encrypted with that object's keystream, and the two ciphertexts XOR to the two plaintexts' XOR. Laid out for reading over a network (ISO 32000-2, Annex F), a file's objects all move, onto numbers that run from 1 without a gap, so that's every one of them. A writer that doesn't want that writes the file whole under a new AES-256 encryption (`create`), which leaks neither way (`Method::Rc4`).
- **Keys kept in, and wiped:** the handler decrypts and encrypts with the file's key and each object's, and gives out neither, its `Debug` included. Each is wiped when it's dropped (RustCrypto's `zeroize`), as is every buffer a password's or a key's bytes pass through on the way: the padded password, the hashes' and ciphers' inputs built from it, the intermediate keys, RC4's round keys. RustCrypto's crates are built with their own `zeroize`, which wipes their key schedules and hashing state; AES keeps a key's schedule in place of the key, keyed once for a file whose strings and streams are under the file's key.
- **RustCrypto's primitives,** not its own: MD5 (`md-5`), SHA-256, SHA-384 and SHA-512 (`sha2`), HMAC-SHA256 (`hmac`), RC4 (`rc4`), and AES-128 and AES-256 (`aes`, in CBC by `cbc`).

## An example

A new file's encryption under an open password and a permissions password, and the file opened by each:

```rust
use pdf_crypt::{Changes, Encoding, Handler, Ivs, Password, Permissions, Printing, Protection, Refused};

// The two passwords prepared: the open password, and the permissions
// password, Ⅸ, whose SASLprep form is `IX`.
let user = Password("sesame").prepared().unwrap();
let owner = Password("\u{2168}").prepared().unwrap();
assert_eq!(owner.as_bytes(), b"IX");

// One who opens the file by the open password may print it at low
// resolution and fill in its forms (and assemble it, as that level has it),
// but not copy from it.
let mut protection = Protection::new(&user);
protection.owner = Some(&owner);
protection.permissions = Permissions::from(Printing::Low) | Permissions::from(Changes::FillForms);
let (writer, dictionary) = Handler::create(&protection).unwrap();
assert_eq!((dictionary.v, dictionary.r, dictionary.p), (5, 6, Some(-2108)));

// A writer encrypts a string of object 5 under it, AES's IV the next of
// the write's, from the operating system: the same string encrypts apart
// each time.
let ivs = Ivs::new();
let title = writer.encrypt_string((5, 0), b"A title", &ivs).unwrap();
assert_ne!(writer.encrypt_string((5, 0), b"A title", &ivs).unwrap(), title);

// A reader opens it by the open password as the user's (revision 6 takes
// no /ID), with those permissions and bit 10, which revision 6 always
// sets, and decrypts.
let by_user = Handler::open(&dictionary, b"", &Password("sesame").encodings()).unwrap();
assert!(!by_user.owner());
assert_eq!(by_user.permissions().printing(), Printing::Low);
assert!(!by_user.permissions().contains(Permissions::COPY));
assert_eq!(by_user.permissions().to_string(), "print, fill forms, accessibility, assemble");
assert_eq!(by_user.decrypt_string((5, 0), &title).unwrap(), b"A title");

// The permissions password, as typed, in its SASLprep form, opens it as
// the owner's, whom the permissions don't bind.
let by_owner = Handler::open(&dictionary, b"", &Password("\u{2168}").encodings()).unwrap();
assert!(by_owner.owner());
assert_eq!(by_owner.opened_by(), Some((0, Encoding::Saslprep)));

// Another password doesn't open it, nor none.
let wrong = Password("IV").encodings();
assert_eq!(Handler::open(&dictionary, b"", &wrong).unwrap_err(), Refused::Password);
assert_eq!(Handler::open(&dictionary, b"", &[]).unwrap_err(), Refused::Password);
```

## Test files

The `insecure-test-writer` feature, off by default, writes test files: an encryption dictionary of every revision the handler reads, its salts and keys given, and strings and streams encrypted under IVs given. It's for this crate's tests and its workspace's, never for protecting a document, which `create` is for. Its items are hidden from the docs.

## Exhaustive on purpose, or not

`Error` and `Password` are exhaustive on purpose: each is built by a literal and taken apart by its public fields (an error is what decrypted and why, a password its text), so a field more would be a breaking release. So are `Printing` and `Changes`, a security dialog's levels, designed and listed whole (`Printing::ALL`, `Changes::ALL`), so a level more would be one too. What may say more is `#[non_exhaustive]`, so it can grow in a minor release, a match on it needing a wildcard and a struct being made from its `default()`: `Dictionary` and `CryptFilter`, as a field can come, another handler's or a crypt filter's; `Protection`, as a new file's choices can (a revision ISO/TS 32003 adds, say), made by `Protection::new` or from `Protection::DEFAULT`, its fields set after; `Method`, as a method can, ISO/TS 32003's AES-GCM for one; `Refused`, `Fault` and `Unusable`, as refusals, faults and reasons are found in files and passwords rather than designed, `Refused::OtherHandler` and `Refused::Version` each so too, matched with `..`, so a field can join one (another handler's `/SubFilter`, say), while `Fault`'s variants are built by a literal and matched whole, each field the whole of what it says (a length, a count, a name), a fault that says more joining as a variant of its own; and `Encoding`, as writers are found setting passwords in more. `Handler`, `Encoded` and `Permissions` keep their fields to themselves.

## What isn't here

- **Other security handlers:** the public-key handler (`/Adobe.PubSec`) and those of other names are refused, by name (`Refused::OtherHandler`).
- **ISO/TS 32003's AES-GCM:** a dictionary of `/V 6 /R 7` is refused by the pair (`Refused::Version`), and a crypt filter's `/AESV4` in a file the handler opens is a method that isn't one (`Method::Unknown`), refused where a string or a stream is decrypted or encrypted by it.
- **ISO/TS 32004's integrity protection:** a MAC over the file's bytes, in the trailer's `/AuthCode`, under a key wrapped by one derived from the file's key and the encryption dictionary's `/KDFSalt`. Neither `/AuthCode` nor `/KDFSalt` is read here (a `Dictionary` has no field for the salt), so a file that has them opens as one without, its MAC unchecked, and `create` writes neither.
- **Knowing what isn't encrypted:** the encryption dictionary's own strings, a cross-reference stream, a signature's `/Contents`, metadata under `/EncryptMetadata false`. That takes the file, and is the caller's: pdf-structure's reader decrypts by one list of it, and its writer encrypts by the same list.
- **Enforcing permissions.** pdf-crypt says what a file's `/P` grants and writes what a new file's grants; honouring them is a reader's. One who can open a file, by the user's password or by none where that's empty, can decrypt all of it, whatever they withhold, and the owner's password isn't bound by them.
- **New files of revisions 2 to 5.** `create` makes revision 6 alone; a file of an older revision is read, and a writer encrypts under the handler it opened. The `insecure-test-writer` feature makes dictionaries of every revision, its salts and keys given: for test files (above), never for protecting a document.
- **A generator for wasm32-unknown-unknown.** That target has no operating system's generator, and getrandom picks none by default: a build for it picks getrandom's backend (its `wasm_js` feature for a browser, or a custom one), as getrandom's docs say. Without one, the build fails; with getrandom's `unsupported` backend, every IV and new file is refused (`Random`).
- **Wiping what it can't reach:** copies the allocator made of a buffer before it was sized, values in registers and spilled to the stack, the buffers `stringprep` and `unicode-normalization` prepare and compose a password in, the caller's own copies of a password (a `Password` borrows the caller's text) or of plaintext, and the operating system's generator's own state. Decrypted strings and streams are the document's, not key material, and aren't wiped.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
