//! The primitives, for the fuzz crate to set beside another
//! implementation's (`just fuzz primitives`). It's compiled only for tests
//! and under cargo-fuzz's `--cfg fuzzing`.

/// MD5's digest.
pub fn md5(data: &[u8]) -> [u8; 16] {
    crate::md5::md5(data)
}

/// SHA-256's digest.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    crate::sha2::sha256(data)
}

/// SHA-384's digest.
pub fn sha384(data: &[u8]) -> [u8; 48] {
    crate::sha2::sha384(data)
}

/// SHA-512's digest.
pub fn sha512(data: &[u8]) -> [u8; 64] {
    crate::sha2::sha512(data)
}

/// `data` through RC4 keyed by `key`.
pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    crate::rc4::rc4(key, data)
}

/// `data`, whole blocks, encrypted in CBC by AES keyed by `key`, 16 or 32
/// bytes, from `iv`.
pub fn aes_cbc_encrypt(key: &[u8], iv: [u8; 16], data: &[u8]) -> Option<Vec<u8>> {
    Some(crate::aes::Aes::new(key)?.encrypt_cbc(iv, data))
}

/// `data`, whole blocks, decrypted in CBC by AES keyed by `key` from `iv`.
pub fn aes_cbc_decrypt(key: &[u8], iv: [u8; 16], data: &[u8]) -> Option<Vec<u8>> {
    Some(crate::aes::Aes::new(key)?.decrypt_cbc(iv, data))
}
