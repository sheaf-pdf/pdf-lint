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

/// The fuzz target `crypt`'s body: a dictionary of any version, revision
/// and fields of any length that `data` chooses, or one the writer makes
/// that opens, opened with passwords from it, and when it opens, strings
/// and streams from it decrypted by each method. Panics on a broken promise: none should, and decrypting
/// gives no more than it's given.
pub fn handler(data: &[u8]) {
    use crate::{CryptFilter, Dictionary, Handler, Method};
    let mut bytes = data.iter().copied();
    let mut next = || bytes.next().unwrap_or(0);
    let mut take = |n: usize| -> Vec<u8> { (0..n).map(|_| next()).collect() };
    let head = take(8);
    let field = |at: usize| usize::from(head[at] % 64);
    let methods = ["V2", "AESV2", "AESV3", "None", "Odd"];
    let dictionary = Dictionary {
        filter: b"Standard".to_vec(),
        v: i64::from(head[0] % 6),
        // Revision 6 runs thousands of AES blocks a password: rare.
        r: i64::from(head[1] % 7).min(if head[1] == 255 { 6 } else { 5 }),
        length: Some(i64::from(head[2]) * 2),
        o: take(field(3)),
        u: take(field(4)),
        oe: take(field(5)),
        ue: take(field(6)),
        perms: take(field(7)),
        p: i64::from(i32::from_le_bytes([head[2], head[3], head[4], head[5]])),
        encrypt_metadata: Some(head[6] & 1 == 0),
        crypt_filters: vec![CryptFilter {
            name: b"StdCF".to_vec(),
            method: Some(
                methods[usize::from(head[7]) % methods.len()]
                    .as_bytes()
                    .to_vec(),
            ),
        }],
        stm_f: Some(b"StdCF".to_vec()),
        str_f: Some(b"StdCF".to_vec()),
    };
    let id = take(usize::from(head[0] % 20));
    let password = take(usize::from(head[1] % 8));
    // Half the time, a dictionary the writer makes for the password, which
    // opens, so that decrypting is fuzzed too.
    #[cfg(any(test, feature = "writer"))]
    let dictionary = match head[0] & 0x80 {
        0 => dictionary,
        _ => {
            use crate::writer::{Kind, Settings, dictionary};
            let kinds = [Kind::Rc4By40, Kind::Rc4By128, Kind::Aes128, Kind::Aes256R5];
            let kind = match head[1] {
                255 => Kind::Aes256,
                n => kinds[usize::from(n) % kinds.len()],
            };
            dictionary(&Settings {
                kind,
                user: &password,
                owner: b"owner",
                p: -4,
                id: &id,
                encrypt_metadata: head[6] & 1 == 0,
            })
        }
    };
    let rest: Vec<u8> = data.iter().copied().skip(8 + 5 * 64 + 28).collect();
    let Ok(handler) = Handler::open(&dictionary, &id, &[&password, b""]) else {
        return;
    };
    let id = (u32::from(head[2]), u16::from(head[3]));
    let methods = [
        None,
        Some(Method::Identity),
        Some(Method::None),
        Some(Method::Rc4),
        Some(Method::Aes128),
        Some(Method::Aes256),
    ];
    for method in &methods {
        for data in [
            &rest[..],
            &rest[..rest.len().min(17)],
            &rest[..rest.len() / 16 * 16],
        ] {
            let check = |result: Result<Vec<u8>, crate::Error>| {
                let len = match result {
                    Ok(out) => out.len(),
                    Err(e) => e.data.map_or(0, |d| d.len()),
                };
                assert!(len <= data.len(), "no more out than in");
            };
            check(handler.decrypt_stream(id, data, method.as_ref()));
            check(handler.decrypt_string(id, data));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::handler;

    #[test]
    fn random_dictionaries_and_ciphertext_keep_every_promise() {
        let mut seed = 0x853c_49e6_748f_ea9b_u64;
        for i in 0..400 {
            let data: Vec<u8> = (0..300 + i)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed as u8
                })
                .collect();
            handler(&data);
        }
    }
}
