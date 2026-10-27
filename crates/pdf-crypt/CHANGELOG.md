# Changelog

## 0.1.0 (unreleased)

- **Primitives.** MD5 (RFC 1321), SHA-256, SHA-384 and SHA-512 (FIPS 180-4), RC4, and AES-128 and AES-256 (FIPS 197), encrypting and decrypting a block, and in CBC (SP 800-38A), all of the crate's own: checked against each standard's examples, against Python's hashlib at the lengths where padding changes, and against RustCrypto by a fuzz target in the repository's fuzz crate (`just fuzz primitives`).
