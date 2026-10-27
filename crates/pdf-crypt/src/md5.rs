//! MD5 (RFC 1321), which revisions 2 to 4 of the standard security handler
//! derive keys with: RustCrypto's `md-5`.

use ::md5::{Digest, Md5};

/// `data`'s MD5 digest.
pub(crate) fn md5(data: &[u8]) -> [u8; 16] {
    Md5::digest(data).into()
}

#[cfg(test)]
mod tests {
    use super::md5;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn rfc_1321s_test_suite() {
        let suite = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (message, digest) in suite {
            assert_eq!(hex(&md5(message.as_bytes())), digest, "{message:?}");
        }
    }
}
