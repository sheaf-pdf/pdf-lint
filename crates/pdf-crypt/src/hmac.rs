//! HMAC-SHA256 (RFC 2104, FIPS 198-1), which the handler derives the IVs
//! it encrypts under with: RustCrypto's `hmac` over `sha2`.

use ::hmac::{Hmac, KeyInit, Mac};
use ::sha2::Sha256;

/// HMAC-SHA256 keyed once, its keyed state cloned for each message. Its
/// state is wiped when it's dropped (`hmac`'s and `sha2`'s `zeroize`).
#[derive(Clone)]
pub(crate) struct HmacSha256(Hmac<Sha256>);

impl HmacSha256 {
    /// HMAC-SHA256 keyed by `key`, of any length.
    pub(crate) fn new(key: &[u8]) -> HmacSha256 {
        HmacSha256(Hmac::new_from_slice(key).expect("HMAC takes a key of any length"))
    }

    /// The MAC of `parts`, one after the other.
    pub(crate) fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut mac = self.0.clone();
        for part in parts {
            mac.update(part);
        }
        mac.finalize().into_bytes().into()
    }
}

#[cfg(test)]
mod tests {
    use super::HmacSha256;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// RFC 4231's HMAC-SHA256 test cases (4.2 to 4.8), case 5's output cut
    /// to 128 bits as the RFC has it; and each message in two parts, which
    /// is the message.
    #[test]
    fn rfc_4231s_test_cases() {
        let larger = [0xaa; 131];
        let cases: [(&[u8], &[u8], &str); 7] = [
            (
                &[0x0b; 20],
                b"Hi There",
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                b"Jefe",
                b"what do ya want for nothing?",
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                &[0xaa; 20],
                &[0xdd; 50],
                "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
            ),
            (
                &[
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                    0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19,
                ],
                &[0xcd; 50],
                "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
            ),
            (
                &[0x0c; 20],
                b"Test With Truncation",
                "a3b6167473100ee06e0c796c2955552b",
            ),
            (
                &larger,
                b"Test Using Larger Than Block-Size Key - Hash Key First",
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
            (
                &larger,
                b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.",
                "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
            ),
        ];
        for (i, (key, data, mac)) in cases.into_iter().enumerate() {
            let keyed = HmacSha256::new(key);
            let len = mac.len() / 2;
            assert_eq!(hex(&keyed.mac(&[data])[..len]), mac, "case {}", i + 1);
            let (a, b) = data.split_at(data.len() / 3);
            assert_eq!(hex(&keyed.mac(&[a, b])[..len]), mac, "case {}", i + 1);
        }
    }

    /// Keyed once and used again, it gives each message what one keyed
    /// anew does: a MAC doesn't carry one message into the next.
    #[test]
    fn keyed_once_it_gives_what_one_keyed_anew_does() {
        let keyed = HmacSha256::new(&[7; 32]);
        for message in [&b"a"[..], b"another", b""] {
            assert_eq!(
                keyed.mac(&[message]),
                HmacSha256::new(&[7; 32]).mac(&[message])
            );
        }
    }
}
