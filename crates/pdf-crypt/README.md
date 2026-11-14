# pdf-crypt

PDF's standard security handler (ISO 32000-2 §7.6) built on RustCrypto primitives, with no dependency on any specific PDF object model.

`pdf-crypt` derives file keys from encryption dictionary parameters (`Dictionary`) and user/owner passwords, decrypts and encrypts strings and streams, and generates new revision 6 (AES-256) encryption dictionaries using OS randomness (`getrandom`). For high-level file encryption and decryption, see [pdfs](https://crates.io/crates/pdfs) or [pdf-structure](https://crates.io/crates/pdf-structure).

## Key Capabilities

- **Revisions 2–6 supported (`Handler::open`):** Reads legacy RC4 (40–128 bit, revisions 2–4), AES-128 crypt filters (version 4), Adobe Extension Level 3 AES-256 (revision 5), and ISO 32000-2 AES-256 (revision 6).
- **Smart password encoding & SASLprep (`Password`, `Encoded`):**
  - When opening existing files, `Password::encodings()` generates candidate byte encodings (Unicode 3.2 RFC 4013 SASLprep, modern Unicode SASLprep, raw UTF-8, and NFC-normalized `PDFDocEncoding` / `WinAnsiEncoding` / `MacRomanEncoding`) ordered by the file's revision.
  - When creating new encrypted files, `Password::prepared()` strictly validates RFC 4013 SASLprep (1–127 bytes, consistent across Unicode 3.2 and current Unicode).
- **New AES-256 encryption (`Handler::create`, `Protection`):** Creates revision 6 encryption dictionaries with user ("open") password, owner ("permissions") password, or both, drawing file keys and salts from the OS CSPRNG.
- **Typed permissions (`Permissions`, `Printing`, `Changes`):** Models ISO 32000-2 Table 22 `/P` permission bits and standard dialog levels for printing and document modification.
- **Fault-tolerant decryption & safe encryption (`Ivs`):** Malformed ciphertext (short blocks, bad PKCS#7 padding, mismatched `/Perms`) reports typed `Fault`s alongside any recovered bytes (`Error::data` is omitted from `Debug` output so logs never leak plaintext). Encryption draws fresh 16-byte OS-random AES IVs via thread-safe `Ivs` and refuses to fall back to cleartext on RNG failure.
- **Zeroized key material:** File keys, object keys, password buffers, and cipher schedules are wiped on drop via `zeroize`. Constant-time `KeyCheck` (`Handler::key_check`, `Handler::has_key`) verifies whether two handlers share the same key without exposing key bytes.

## Quickstart

```rust
use pdf_crypt::{Changes, Encoding, Handler, Ivs, Password, Permissions, Printing, Protection, Refused};

// Prepare open (user) and permissions (owner) passwords via RFC 4013 SASLprep.
// U+2168 ('Ⅸ') normalizes to "IX".
let user = Password("sesame").prepared().unwrap();
let owner = Password("\u{2168}").prepared().unwrap();
assert_eq!(owner.as_bytes(), b"IX");

// Allow low-resolution printing and form filling for the user password.
let mut protection = Protection::new(&user);
protection.owner = Some(&owner);
protection.permissions = Permissions::from(Printing::Low) | Permissions::from(Changes::FillForms);
let (writer, dictionary) = Handler::create(&protection).unwrap();
assert_eq!((dictionary.v, dictionary.r, dictionary.p), (5, 6, Some(-2108)));

// Encrypt a string in object (5, 0) using OS-random AES IVs.
let ivs = Ivs::new();
let title = writer.encrypt_string((5, 0), b"A title", &ivs).unwrap();
assert_ne!(writer.encrypt_string((5, 0), b"A title", &ivs).unwrap(), title);

// Open with the user password and inspect granted permissions.
let by_user = Handler::open(&dictionary, b"", &Password("sesame").encodings()).unwrap();
assert!(!by_user.owner());
assert_eq!(by_user.permissions().printing(), Printing::Low);
assert!(!by_user.permissions().contains(Permissions::COPY));
assert_eq!(by_user.permissions().to_string(), "print, fill forms, accessibility, assemble");
assert_eq!(by_user.decrypt_string((5, 0), &title).unwrap(), b"A title");

// Open with the owner password (normalizes 'Ⅸ' -> "IX" automatically).
let by_owner = Handler::open(&dictionary, b"", &Password("\u{2168}").encodings()).unwrap();
assert!(by_owner.owner());
assert_eq!(by_owner.opened_by(), Some((0, Encoding::Saslprep)));

// Wrong or missing passwords fail with Refused::Password.
let wrong = Password("IV").encodings();
assert_eq!(Handler::open(&dictionary, b"", &wrong).unwrap_err(), Refused::Password);
assert_eq!(Handler::open(&dictionary, b"", &[]).unwrap_err(), Refused::Password);
```

## Scope & Security Notes

- **Standard handler only:** Public-key handlers (`/Adobe.PubSec`), ISO/TS 32003 AES-GCM (`/V 6 /R 7`), and ISO/TS 32004 `/AuthCode` MACs are not implemented (unsupported handlers/versions return `Refused::OtherHandler` or `Refused::Version`).
- **New files use AES-256 (revision 6):** Older revisions (2–5) are supported for reading and updating existing files; when re-writing RC4 files, callers must avoid reusing `(object, generation)` pairs across modified plaintexts to prevent RC4 keystream reuse (`pdf-structure` enforces this automatically).
- **Cleartext exemptions live in the file layer:** Knowing which PDF objects stay unencrypted (`/Encrypt` dictionary strings, xref streams, signature `/Contents`, unencrypted `/Metadata`) belongs to `pdf-structure`.
- **Permissions are advisory:** `Permissions` encodes and reports `/P` flags; enforcing UI restrictions is up to the application.
- **`wasm32-unknown-unknown`:** Configure a `getrandom` backend (`wasm_js` or custom) when building for WebAssembly.
- **`insecure-test-writer` feature:** Hidden test-only API for deterministic seeded keys/IVs in workspace tests and fuzzing. Never enable in production builds.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
