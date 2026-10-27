//! RC4, which revisions 2 to 4 of the standard security handler encrypt
//! with (the cipher's name was ARCFOUR when it was published): RustCrypto's
//! `rc4`.

use ::rc4::{KeyInit, Rc4, StreamCipher};

/// `data` through RC4 keyed by `key`, which the handler makes 5 to 16
/// bytes, or 32, a version 5 file's key under a `/V2` crypt filter (the
/// cipher takes 1 to 256).
pub(crate) fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    Rc4::new_from_slice(key)
        .expect("an RC4 key of 1 to 256 bytes")
        .apply_keystream(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::rc4;

    /// RFC 6229's test vectors: the key stream at offsets 0 and 16, 240 and
    /// 256, 4080 and 4096, for keys of 40, 128 and 256 bits.
    #[test]
    fn rfc_6229s_key_streams() {
        let stream = |key: &[u8], at: usize| {
            rc4(key, &vec![0u8; at + 16])[at..]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let forty = [0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(stream(&forty, 0), "b2396305f03dc027ccc3524a0a1118a8");
        assert_eq!(stream(&forty, 16), "6982944f18fc82d589c403a47a0d0919");
        assert_eq!(stream(&forty, 240), "28cb1132c96ce286421dcaadb8b69eae");
        assert_eq!(stream(&forty, 4096), "ff25b58995996707e51fbdf08b34d875");
        let sixteen: Vec<u8> = (1..=16).collect();
        assert_eq!(stream(&sixteen, 0), "9ac7cc9a609d1ef7b2932899cde41b97");
        assert_eq!(stream(&sixteen, 16), "5248c4959014126a6e8a84f11d1a9e1c");
        assert_eq!(stream(&sixteen, 4096), "a36a4c301ae8ac13610ccbc12256cacc");
        let thirty_two: Vec<u8> = (1..=32).collect();
        assert_eq!(stream(&thirty_two, 0), "eaa6bd25880bf93d3f5d1e4ca2611d91");
        assert_eq!(stream(&thirty_two, 16), "cfa45c9f7e714b54bdfa80027cb14380");
    }

    #[test]
    fn decrypting_is_encrypting_again() {
        let encrypted = rc4(b"Key", b"Plaintext");
        assert_eq!(
            encrypted,
            [0xbb, 0xf3, 0x16, 0xe8, 0xd9, 0x40, 0xaf, 0x0a, 0xd3]
        );
        assert_eq!(rc4(b"Key", &encrypted), b"Plaintext");
    }
}
