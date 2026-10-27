# pdf-crypt

PDF's standard security handler, decrypting, in one small Rust crate that depends on nothing: a file's key from its encryption dictionary and a password, and its strings and streams decrypted with it (ISO 32000-2, §7.6).

It's made for a reader that checks files as much as for one that shows them. Ciphertext that isn't what the handler wrote still decrypts as far as it can, and every fault is said.

## What it does

- **Every revision of the standard handler:** 2 to 4 (RC4 with keys of 40 to 128 bits, and AES-128 through crypt filters in version 4), 5 (AES-256, as Adobe's Extension Level 3 has it, which ISO 32000-2 leaves out) and 6 (AES-256, ISO 32000-2), opened by the owner's password or the user's, the first of several that opens it.
- **Values, not dictionaries.** A `Dictionary` is the encryption dictionary's values, typed; reading them from a file is the caller's, so it works with any object model. Short `/O` and `/U` are filled out with zeros, and revision 5's and 6's long ones cut short, as qpdf reads them.
- **Crypt filters** by name and method (`/CF`, `/StmF`, `/StrF`, a stream's own `/Crypt`): `/Identity`, `/None`, `/V2`, `/AESV2`, `/AESV3`, and one that isn't, said.
- **Every fault said, what decrypted kept:** AES ciphertext with no whole block past its IV (it stays ciphertext), a part block past the whole ones, padding that isn't, a crypt filter's method that isn't one, an AES key of the wrong length, and `/Perms` that doesn't match (the file opens all the same).
- **Its own primitives,** needing nothing else: MD5, SHA-256, SHA-384, SHA-512, RC4, AES-128 and AES-256, checked against each standard's examples, against Python's hashlib at the lengths where padding changes, and against RustCrypto's by a fuzz target outside the crate.

```rust
use pdf_crypt::{Handler, Refused};
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
let encrypted = writer::encrypt_string(
    &Handler::open(&dictionary, settings.id, &[b"user"]).unwrap(),
    (12, 0),
    b"Hello",
    [7; 16],
);

// A reader opens it with the password it's given, and decrypts.
let handler = Handler::open(&dictionary, settings.id, &[b"owner"]).unwrap();
assert!(handler.owner());
assert_eq!(handler.decrypt_string((12, 0), &encrypted).unwrap(), b"Hello");
assert_eq!(Handler::open(&dictionary, settings.id, &[b"wrong"]).unwrap_err(), Refused::Password);
```

## What isn't here

- **Other security handlers:** the public-key handler (`/Adobe.PubSec`) and those of other names are refused, saying so.
- **Knowing what isn't encrypted:** the encryption dictionary's own strings, a cross-reference stream, a signature's `/Contents`, metadata under `/EncryptMetadata false`. That takes the file, and is the caller's.
- **Encrypting a document.** The `writer` feature encrypts as the handler decrypts, but with the salts, keys and IVs given, not random: for writing test files, not for protecting a document.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option.
