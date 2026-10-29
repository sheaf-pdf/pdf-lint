//! The fuzz target `crypt`'s bodies, for the fuzz crate. They're compiled
//! only for tests and under cargo-fuzz's `--cfg fuzzing`.

use std::convert::Infallible;

/// The fuzz target `crypt`'s body: a dictionary of any version, revision
/// and fields of any length that `data` chooses, or one the writer makes
/// that opens, opened with passwords from it, each encoding of its text and
/// its bytes as they are, and when it opens, strings and streams from it
/// decrypted by each method, and encrypted under a context from it; and now
/// and then, as revision 6 is, a new file's dictionary created for text
/// from it (`created`). Panics on a broken promise: none should, the
/// handler says which password opened it, which opens it alike alone,
/// decrypting gives no more than it's given, and what's encrypted decrypts
/// to what it was, the same bytes each time, or is refused alike both ways.
pub fn handler(data: &[u8]) {
    use crate::{CryptFilter, Dictionary, Encoded, Fault, Handler, Method, Password};
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
    // A new file's dictionary runs revision 6's hash a dozen times: rare.
    if head[1] == 254 {
        created(&rest, head[6] & 1 == 0);
    }
    let text = String::from_utf8_lossy(&password);
    let mut passwords = Password(&text).encodings();
    passwords.extend([Encoded::verbatim(&password), Encoded::verbatim(b"")]);
    let Ok(handler) = Handler::open(&dictionary, &id, &passwords) else {
        return;
    };
    let (i, encoding) = handler.opened_by().expect("a password given opened it");
    assert_eq!(passwords[i].encoding(), encoding, "the password's encoding");
    let alone = Handler::open(&dictionary, &id, &passwords[i..=i]).expect("it opens alone");
    assert_eq!(alone.opened_by(), Some((0, encoding)), "opened by it alone");
    assert_eq!(
        (alone.owner(), alone.faults()),
        (handler.owner(), handler.faults()),
        "opened alike alone"
    );
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

/// A new file's handler and dictionary ([`Handler::create`](crate::Handler::create)),
/// for text from `data`: its first byte the text's length, then the text
/// (UTF-8, lossily), then what's drawn ([`Drawn`]). Panics on a broken
/// promise: none should; each of the text's encodings is taken exactly
/// when it's SASLprep's form and `Password::prepared` gives its bytes,
/// else refused for its tag or its length; and prepared, the same draws
/// give the same dictionary, of revision 6 with every permission, which
/// opens by the text's encodings as the owner's, `/Perms` and
/// `/EncryptMetadata` as asked, its handler and the one opened
/// encrypting alike and each decrypting the other's.
fn created(data: &[u8], encrypt_metadata: bool) {
    use crate::{Encoding, Handler, Password, Unusable};
    let (len, rest) = data.split_first().unwrap_or((&0, &[]));
    let (text, draws) = rest.split_at(usize::from(*len).min(rest.len()));
    let text = String::from_utf8_lossy(text);
    let typed = Password(&text);
    let prepared = typed.prepared();
    for form in typed.encodings() {
        let created = Handler::create(&form, encrypt_metadata, &mut Drawn::over(draws));
        let bytes = form.as_bytes();
        match created {
            Ok(_) => assert!(
                form.encoding() == Encoding::Saslprep
                    && prepared.as_ref().is_ok_and(|p| p.as_bytes() == bytes),
                "taken: prepared's"
            ),
            Err(why) => {
                let expected = match (form.encoding(), bytes.len()) {
                    (Encoding::Saslprep, 0) => Unusable::Empty,
                    (Encoding::Saslprep, len @ 128..) => Unusable::TooLong { len },
                    (Encoding::Saslprep, _) => panic!("a form of 1 to 127 bytes refused"),
                    (encoding, _) => Unusable::Unprepared(encoding),
                };
                assert_eq!(why, expected, "refused for its tag or length");
            }
        }
    }
    let Ok(password) = prepared else {
        return;
    };
    let created = Handler::create(&password, encrypt_metadata, &mut Drawn::over(draws));
    let (handler, dictionary) = created.expect("a password prepared");
    let again = Handler::create(&password, encrypt_metadata, &mut Drawn::over(draws));
    assert_eq!(
        again.map(|(_, d)| d).as_ref(),
        Ok(&dictionary),
        "the same draws"
    );
    assert_eq!(
        (dictionary.v, dictionary.r, dictionary.p),
        (5, 6, Some(-4)),
        "revision 6, every permission"
    );
    assert!(handler.owner() && handler.opened_by().is_none() && handler.faults().is_empty());
    let opened = Handler::open(&dictionary, b"", &typed.encodings()).expect("it opens");
    assert!(
        opened.owner() && opened.faults().is_empty(),
        "as the owner's"
    );
    assert_eq!(
        opened.encrypts_metadata(),
        encrypt_metadata,
        "/EncryptMetadata"
    );
    let id = (1, 0);
    for data in [draws, &draws[..draws.len().min(17)]] {
        let string = handler.encrypt_string(id, b"a context", data);
        assert_eq!(
            opened.encrypt_string(id, b"a context", data),
            string,
            "alike"
        );
        let string = string.expect("AES-256");
        assert_eq!(opened.decrypt_string(id, &string), Ok(data.to_vec()));
        let stream = opened
            .encrypt_stream(id, b"a context", data, None)
            .expect("AES-256");
        assert_eq!(handler.decrypt_stream(id, &stream, None), Ok(data.to_vec()));
    }
}

/// A random number generator for the fuzz body alone: the input's bytes,
/// then a counter's. It's no cryptographic generator, but says it is
/// (`TryCryptoRng`) so that [`Handler::create`](crate::Handler::create)
/// takes it, which only a fuzz body may do: it's compiled for nothing
/// else.
struct Drawn<'a> {
    data: &'a [u8],
    counter: u8,
}

impl Drawn<'_> {
    fn over(data: &[u8]) -> Drawn<'_> {
        Drawn { data, counter: 0 }
    }
}

impl rand_core::TryRng for Drawn<'_> {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut bytes = [0; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut bytes = [0; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        for byte in dst {
            *byte = match self.data.split_first() {
                Some((&first, rest)) => {
                    self.data = rest;
                    first
                }
                None => {
                    self.counter = self.counter.wrapping_add(1);
                    self.counter
                }
            };
        }
        Ok(())
    }
}

impl rand_core::TryCryptoRng for Drawn<'_> {}

/// The fuzz target `crypt`'s body for passwords as typed: text from
/// `data`, read as UTF-8 (lossily), as a character each two bytes (the
/// Basic Multilingual Plane's) and as one each three (every plane's),
/// given to `Password::encodings` and `Password::prepared`. Panics on a
/// broken promise: none should; the forms are SASLprep's first where it
/// isn't the text, then the text as typed, then single-byte forms of the
/// text composed, a byte a character, none twice among those a revision
/// takes first; prepared, the text is SASLprep's form, of 1 to 127 bytes
/// and its own SASLprep, or it's refused for what the text is.
pub fn password(data: &[u8]) {
    use crate::{Encoding, Password, Unusable};
    use stringprep::saslprep;
    use unicode_normalization::UnicodeNormalization;
    let lossy = String::from_utf8_lossy(data);
    let wide: String = (data.chunks_exact(2))
        .filter_map(|b| char::from_u32(u32::from(u16::from_le_bytes([b[0], b[1]]))))
        .collect();
    let planes: String = (data.chunks_exact(3))
        .filter_map(|b| char::from_u32(u32::from_le_bytes([b[0], b[1], b[2], 0]) % 0x11_0000))
        .collect();
    for text in [&lossy[..], &wide, &planes] {
        let typed = Password(text);
        let forms = typed.encodings();
        let form = |i: usize| (forms[i].encoding(), forms[i].as_bytes());
        let prepared = saslprep(text);
        let first = match &prepared {
            Ok(prepared) if prepared != text => {
                assert_eq!(
                    form(0),
                    (Encoding::Saslprep, prepared.as_bytes()),
                    "SASLprep's"
                );
                1
            }
            _ => 0,
        };
        assert_eq!(form(first), (Encoding::Utf8, text.as_bytes()), "as typed");
        let len = text.nfc().count();
        for (encoding, bytes) in (first + 1..forms.len()).map(form) {
            let single = [Encoding::PdfDoc, Encoding::WinAnsi, Encoding::MacRoman];
            assert!(single.contains(&encoding), "{encoding:?}");
            assert_eq!(bytes.len(), len, "a byte a character composed");
        }
        for revision in 2..=6 {
            let fit: Vec<&[u8]> = (forms.iter())
                .filter(|f| f.encoding().fits(revision))
                .map(|f| f.as_bytes())
                .collect();
            for (i, a) in fit.iter().enumerate() {
                assert!(!fit[i + 1..].contains(a), "twice for revision {revision}");
            }
        }
        match typed.prepared() {
            Ok(encoded) => {
                assert_eq!(encoded.encoding(), Encoding::Saslprep);
                let bytes = encoded.as_bytes();
                assert!((1..=127).contains(&bytes.len()), "1 to 127 bytes");
                let encoded = std::str::from_utf8(bytes).expect("UTF-8");
                assert_eq!(prepared.as_deref().ok(), Some(encoded), "SASLprep's");
                let again = saslprep(encoded);
                assert_eq!(again.as_deref().ok(), Some(encoded), "its own SASLprep");
            }
            Err(Unusable::Empty) => assert!(text.is_empty(), "empty"),
            Err(Unusable::Vanished) => {
                assert!(
                    !text.is_empty() && prepared.as_deref().ok() == Some(""),
                    "vanished"
                );
            }
            Err(Unusable::TooLong { len }) => {
                assert!(len > 127, "too long");
                assert_eq!(prepared.map(|p| p.len()).ok(), Some(len), "its length");
            }
            Err(Unusable::Prohibited(c)) => {
                assert!(prepared.is_err() && text.contains(c), "prohibited");
                assert!(saslprep(c.encode_utf8(&mut [0; 4])).is_err(), "alone too");
            }
            Err(Unusable::Bidi) => assert!(prepared.is_err(), "bidi"),
            Err(other) => panic!("{other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{handler, password};

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

    /// New files' dictionaries, as the fuzz body creates them now and then:
    /// for text whose SASLprep form is itself, isn't, is empty, is over 127
    /// bytes, or that SASLprep refuses; and for random text; each with
    /// draws, and with none but the counter's, each way of
    /// `/EncryptMetadata`.
    #[test]
    fn new_files_dictionaries_keep_every_promise() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        };
        let long = "\u{2168}".repeat(64);
        let random: Vec<u8> = (0..40).map(|_| next()).collect();
        let texts: [&[u8]; 9] = [
            b"user",
            "p\u{e4}ssw\u{f6}rd".as_bytes(),
            "\u{2168}".as_bytes(),
            "I\u{AD}X".as_bytes(),
            "\u{AD}".as_bytes(),
            long.as_bytes(),
            b"\x07",
            b"",
            &random,
        ];
        for text in texts {
            for draws in [&[][..], &[7; 60]] {
                for metadata in [0, 1] {
                    let mut data = vec![0; 8 + 5 * 64 + 28];
                    data[1] = 254;
                    data[6] = metadata;
                    data.push(text.len() as u8);
                    data.extend_from_slice(text);
                    data.extend_from_slice(draws);
                    handler(&data);
                }
            }
        }
    }

    #[test]
    fn random_text_keeps_every_promise_of_its_forms() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for i in 0..2000 {
            let len = (next() % 400) as usize;
            let data: Vec<u8> = (0..len)
                .map(|_| match i % 4 {
                    // Mostly ASCII, as typed text is; a byte past it now
                    // and then, which the second and third readings make
                    // characters of other scripts.
                    0 => b"aZ9 \xC3\xA9\x01\xADx"[(next() % 9) as usize],
                    _ => next() as u8,
                })
                .collect();
            password(&data);
        }
        for text in [
            "I\u{AD}X",
            "\u{2168}",
            "e\u{301}",
            "\u{5D0}\u{FB44}",
            "\u{AD}",
        ] {
            password(text.as_bytes());
        }
        password("\u{2168}".repeat(64).as_bytes());
    }
}
