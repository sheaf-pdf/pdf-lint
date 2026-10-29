//! The fuzz target `crypt`'s body, for the fuzz crate. It's compiled only
//! for tests and under cargo-fuzz's `--cfg fuzzing`.

/// The fuzz target `crypt`'s body: a dictionary of any version, revision
/// and fields of any length that `data` chooses, or one the writer makes
/// that opens, opened with passwords from it, and when it opens, strings
/// and streams from it decrypted by each method, and encrypted under a
/// context from it. Panics on a broken promise: none should, decrypting
/// gives no more than it's given, and what's encrypted decrypts to what
/// it was, the same bytes each time, or is refused alike both ways.
pub fn handler(data: &[u8]) {
    use crate::{CryptFilter, Dictionary, Fault, Handler, Method};
    let mut bytes = data.iter().copied();
    let mut next = || bytes.next().unwrap_or(0);
    let mut take = |n: usize| -> Vec<u8> { (0..n).map(|_| next()).collect() };
    let head = take(8);
    let field = |at: usize| usize::from(head[at] % 64);
    let methods = ["V2", "AESV2", "AESV3", "None", "Odd"];
    // A field of the length `head[at]` chooses, or past 60, none at all.
    let mut string = |at: usize| {
        let value = take(field(at));
        (field(at) <= 60).then_some(value)
    };
    let dictionary = Dictionary {
        filter: b"Standard".to_vec(),
        v: i64::from(head[0] % 6),
        // Revision 6 runs thousands of AES blocks a password: rare.
        r: i64::from(head[1] % 7).min(if head[1] == 255 { 6 } else { 5 }),
        length: Some(i64::from(head[2]) * 2),
        o: string(3),
        u: string(4),
        oe: string(5),
        ue: string(6),
        perms: string(7),
        p: (head[5] != 0x7f).then_some(i64::from(i32::from_le_bytes([
            head[2], head[3], head[4], head[5],
        ]))),
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
    // Encrypted under a context from the input, by each method: decrypted,
    // it's what it was; or it's refused, as decrypting refuses it.
    let context = &rest[..rest.len().min(usize::from(head[4] % 40))];
    for method in &methods {
        for data in [&rest[..], &rest[..rest.len().min(17)]] {
            let stream = handler.encrypt_stream(id, context, data, method.as_ref());
            let again = handler.encrypt_stream(id, context, data, method.as_ref());
            assert_eq!(stream, again, "the same each time");
            let string = handler.encrypt_string(id, context, data);
            let decrypted = [
                stream.map(|s| handler.decrypt_stream(id, &s, method.as_ref())),
                string.map(|s| handler.decrypt_string(id, &s)),
            ];
            for (decrypted, stream) in decrypted.into_iter().zip([true, false]) {
                match decrypted {
                    Ok(back) => assert_eq!(back.as_deref(), Ok(data), "decrypts to what it was"),
                    Err(fault) => {
                        assert!(matches!(fault, Fault::Key { .. } | Fault::Method { .. }));
                        // Ciphertext of no white-space, so a stream's isn't
                        // taken for it.
                        let refused = match stream {
                            true => handler.decrypt_stream(id, &[1; 32], method.as_ref()),
                            false => handler.decrypt_string(id, &[1; 32]),
                        };
                        assert_eq!(refused.map_err(|e| e.fault), Err(fault), "refused alike");
                    }
                }
            }
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
