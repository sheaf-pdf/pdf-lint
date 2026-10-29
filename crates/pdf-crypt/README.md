# pdf-crypt

PDF's standard security handler, in one small Rust crate on RustCrypto's primitives: a file's key from its encryption dictionary and a password, and its strings and streams decrypted with it, or encrypted for a writer; and a new file's key and encryption dictionary, from randomness the caller passes (ISO 32000-2, §7.6).

It's made for a reader that checks files as much as for one that shows them. Ciphertext that isn't what the handler wrote still decrypts as far as it can, and every fault is said.

## What it does

- **Every revision of the standard handler:** 2 to 4 (RC4 with keys of 40 to 128 bits, and AES-128 through crypt filters in version 4), 5 (AES-256, as Adobe's Extension Level 3 has it, which ISO 32000-2 leaves out) and 6 (AES-256, ISO 32000-2), opened by the owner's password or the user's, the first of several that opens it.
- **A password as typed, and the bytes tried** (`Password`, `Encoded`, `Encoding`): a password's text in its SASLprep form (RFC 4013) as of Unicode 3.2, which revisions 5 and 6 take, where that isn't the text as typed (`Encoding::Saslprep`); its SASLprep form by a later Unicode, where that's another again, as writers whose SASLprep normalizes by a later Unicode set it (`Encoding::SaslprepCurrentUnicode`); as typed, in UTF-8, as writers that skip SASLprep set it; then, for revisions 2 to 4, whose passwords are bytes, the text composed (NFC) in PDFDocEncoding, WinAnsiEncoding and MacRomanEncoding where each has every character of it, as qpdf tries them (`Password::encodings`); or bytes as they are (`Encoded::verbatim`). A password SASLprep refuses (an emoji, a control character) has the rest alone. Each is tagged with its encoding, and wiped when it's dropped. The handler tries those whose encoding fits the file's revision first, the rest after, and none twice where the revision takes two alike (padded or cut to 32 bytes, or cut to 127), as they'd give one key; it says which opened it (`opened_by`).
- **A new file's password, strictly** (`Password::prepared`): its SASLprep form, 1 to 127 bytes, or why it can't be one (`Unusable`): a character SASLprep doesn't allow, named (one Unicode 3.2 hadn't assigned among them, though a later Unicode decomposes it), right-to-left text it doesn't allow, a password that's empty, that prepares to nothing (each of its characters one SASLprep maps to nothing) or to over 127 bytes, which a reader would cut, or whose SASLprep forms by Unicode 3.2 and by a later Unicode differ, named at the character where they part, as readers would prepare it apart.
- **SASLprep as of Unicode 3.2,** which SASLprep is of (RFC 3454, sections 4 and 7). The `stringprep` crate prepares a password by the Unicode `unicode-normalization` has, and its form is taken back to 3.2's as UAX #15 has it done (section 11.3, Guaranteeing Process Stability): the five characters Unicode's Corrigendum #4 remapped in 4.0 (U+2F868, U+2F874, U+2F91F, U+2F95F, U+2F9BF) mapped first to the decompositions 3.2 gave them, a character 3.2 hadn't assigned refused, and the text composed by 3.2's definition of a blocked character (UAX #15 3.2.0's D2), which Corrigendum #5 changed in 4.1, so that a starter composes with the one before it across a mark (U+1100 U+0300 U+1161 is U+AC00 U+0300 by 3.2). That table of five and that rule are this crate's; `stringprep` does the rest. U+F951, Corrigendum #3's, was corrected in 3.2 itself. The sequences Corrigendum #5 changed (UAX #15's Table 10) are in no language's text, and the five characters are rare variants of others.
- **A new file's encryption** (`Handler::create`): a revision 6 encryption dictionary (AES-256, by Algorithms 8, 9 and 10) for a password prepared, and the handler that encrypts under it. Every permission is granted (`/P -4`), and the owner's password is the user's, so that the one password opens it, as the owner's. A password in another encoding is refused (`Unusable::Unprepared`), as is SASLprep's form that a reader tries but a new file can't take: empty, over 127 bytes, or 3.2's form of a password whose later Unicode's form is another, where it isn't a password prepared itself. The file's key, the user's salts, the owner's and `/Perms`' random bytes are drawn, in that order, from a random number generator the caller passes, rand_core's `CryptoRng` (re-exported as `pdf_crypt::rand_core`, at the version `create` takes): pdf-crypt has no randomness of its own. A program passes the operating system's, getrandom's `SysRng` through rand_core's `UnwrapErr`; a test, a seeded generator, which gives the same dictionary every run.
- **Values, not dictionaries.** A `Dictionary` is the encryption dictionary's values, typed; reading them from a file is the caller's, so it works with any object model. One without `/O`, `/U` or `/P` of its type (or of version 5, `/OE`, `/UE` or `/Perms`) is refused, saying so; short values are filled out with zeros, and version 5's long ones cut short, as qpdf reads them.
- **Crypt filters** by name and method (`/CF`, `/StmF`, `/StrF`, a stream's own `/Crypt`): `/Identity`, `/None`, `/V2`, `/AESV2`, `/AESV3`, and one that isn't, said.
- **Every fault said, what decrypted kept:** AES ciphertext with no whole block past its IV (shorter than an IV, it stays ciphertext), a part block past the whole ones, padding that isn't, a crypt filter's method that isn't one, AES that the file's key isn't for, and `/Perms` that doesn't match or doesn't confirm the key (the file opens all the same, by another password's key when one is confirmed).
- **Encrypting for a writer** (`encrypt_string`, `encrypt_stream`): as the handler decrypts, by the same crypt filters, a stream's own included. AES's IV is the first 16 bytes of HMAC-SHA256, under the file's key, of a context's length (8 bytes, big-endian), the context and the data. The context is the caller's name for the object, the same each time it's written, so the same string or stream of the same object encrypts to the same bytes on every save (under any number, with AES-256, whose key is the file's), and no IV serves two plaintexts; the length first, so no context and data run together into another's. RC4 takes no IV, `/Identity` and `/None` leave the data as it is, and a method that isn't one is refused, never written as plaintext.
- **Keys kept in, and wiped:** the handler decrypts and encrypts with the file's key and each object's, and gives out neither, its `Debug` included. Each is wiped when it's dropped (RustCrypto's `zeroize`), as is every buffer a password's or a key's bytes pass through on the way: the padded password, the hashes' and ciphers' inputs built from it, the intermediate keys, RC4's round keys. RustCrypto's crates are built with their own `zeroize`, which wipes their key schedules and hashing state; AES keeps a key's schedule in place of the key, keyed once for a file whose strings and streams are under the file's key.
- **RustCrypto's primitives,** not its own: MD5 (`md-5`), SHA-256, SHA-384 and SHA-512 (`sha2`), HMAC-SHA256 (`hmac`), RC4 (`rc4`), and AES-128 and AES-256 (`aes`, in CBC by `cbc`).

```rust
use pdf_crypt::{Encoding, Handler, Password, Refused};
use pdf_crypt::writer::{self, Kind, Settings};

// A dictionary for a user's and an owner's password, as the `writer`
// feature writes them for tests.
let settings = Settings {
    kind: Kind::Aes128,
    user: b"user",
    owner: b"owner",
    p: -4,
    id: b"0123456789abcdef",
    encrypt_metadata: true,
};
let dictionary = writer::dictionary(&settings);

// A writer encrypts a string of object 12 under it, AES's IV derived from
// what it names the object by and the string: the same each time.
let user = Password("user").encodings();
let writer = Handler::open(&dictionary, settings.id, &user).unwrap();
let encrypted = writer.encrypt_string((12, 0), b"the Info dictionary", b"Hello").unwrap();
assert_eq!(writer.encrypt_string((12, 0), b"the Info dictionary", b"Hello").unwrap(), encrypted);

// A reader opens it with the password it's given, in each encoding, and
// decrypts.
let handler = Handler::open(&dictionary, settings.id, &Password("owner").encodings()).unwrap();
assert!(handler.owner());
assert_eq!(handler.opened_by(), Some((0, Encoding::Utf8)));
assert_eq!(handler.decrypt_string((12, 0), &encrypted).unwrap(), b"Hello");
let wrong = Password("wrong").encodings();
assert_eq!(Handler::open(&dictionary, settings.id, &wrong).unwrap_err(), Refused::Password);
```

A new file's encryption, from a seeded generator (RustCrypto's `chacha20`, with its `rng` feature) where a program would pass the operating system's:

```rust
use chacha20::ChaCha20Rng;
use pdf_crypt::rand_core::SeedableRng;
use pdf_crypt::{Encoding, Handler, Password};

// The password prepared: Ⅸ's SASLprep form is `IX`.
let password = Password("\u{2168}").prepared().unwrap();
assert_eq!(password.as_bytes(), b"IX");
let mut rng = ChaCha20Rng::seed_from_u64(7);
let (writer, dictionary) = Handler::create(&password, true, &mut rng).unwrap();
assert_eq!((dictionary.v, dictionary.r, dictionary.p), (5, 6, Some(-4)));
let title = writer.encrypt_string((5, 0), b"the Info dictionary", b"A title").unwrap();

// A reader opens it by the password as typed, in its SASLprep form, as the
// owner's (revision 6 takes no /ID).
let reader = Handler::open(&dictionary, b"", &Password("\u{2168}").encodings()).unwrap();
assert!(reader.owner());
assert_eq!(reader.opened_by(), Some((0, Encoding::Saslprep)));
assert_eq!(reader.decrypt_string((5, 0), &title).unwrap(), b"A title");
```

## What isn't here

- **Other security handlers:** the public-key handler (`/Adobe.PubSec`) and those of other names are refused, saying so.
- **Knowing what isn't encrypted:** the encryption dictionary's own strings, a cross-reference stream, a signature's `/Contents`, metadata under `/EncryptMetadata false`. That takes the file, and is the caller's, writing as reading.
- **New files of revisions 2 to 5.** `create` makes revision 6 alone; a file of an older revision is read, and a writer encrypts under the handler it opened. The `writer` feature makes dictionaries of every revision, and encrypts under IVs given, but with the salts and keys given, not random: for writing test files, not for protecting a document.
- **Randomness of its own.** A new file's key and salts are what the caller's generator draws: one that isn't a cryptographic generator, or one seeded outside a test, makes a key that can be guessed.
- **Wiping what it can't reach:** copies the allocator made of a buffer before it was sized, values in registers and spilled to the stack, the buffers `stringprep` and `unicode-normalization` prepare and compose a password in, the caller's own copies of a password (a `Password` borrows the caller's text) or of plaintext, and the state of the generator a new file's key is drawn from, which is the caller's. Decrypted strings and streams are the document's, not key material, and aren't wiped.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
