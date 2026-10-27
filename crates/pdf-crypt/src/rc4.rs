//! RC4, which revisions 2 to 4 of the standard security handler encrypt
//! with (the cipher's name was ARCFOUR when it was published).

/// RC4's state, keyed.
pub(crate) struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    /// The cipher keyed by `key`, 1 to 256 bytes.
    pub(crate) fn new(key: &[u8]) -> Rc4 {
        let mut s = [0u8; 256];
        for (i, s) in s.iter_mut().enumerate() {
            *s = i as u8;
        }
        if !key.is_empty() {
            let mut j = 0u8;
            for i in 0..256 {
                j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
                s.swap(i, usize::from(j));
            }
        }
        Rc4 { s, i: 0, j: 0 }
    }

    /// `data` encrypted, or decrypted, which is the same, in place.
    pub(crate) fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[usize::from(self.i)]);
            self.s.swap(usize::from(self.i), usize::from(self.j));
            let k = self.s[usize::from(
                self.s[usize::from(self.i)].wrapping_add(self.s[usize::from(self.j)]),
            )];
            *byte ^= k;
        }
    }
}

/// `data` through RC4 keyed by `key`.
pub(crate) fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    Rc4::new(key).apply(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::{Rc4, rc4};

    /// RFC 6229's test vectors: the key stream at offsets 0 and 16, 240 and
    /// 256, 4080 and 4096, for keys of 40 and 128 bits.
    #[test]
    fn rfc_6229s_key_streams() {
        let stream = |key: &[u8], at: usize| {
            let mut data = vec![0u8; at + 16];
            Rc4::new(key).apply(&mut data);
            data[at..]
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
