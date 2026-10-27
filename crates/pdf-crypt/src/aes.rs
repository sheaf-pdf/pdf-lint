//! AES (FIPS 197) with 128- and 256-bit keys, in CBC and ECB (SP
//! 800-38A), which revisions 4 to 6 of the standard security handler
//! encrypt with. Encrypting too, as revision 6's password hash
//! (Algorithm 2.B) does.

const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// The S-box inverted.
const INVERSE: [u8; 256] = {
    let mut inverse = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        inverse[SBOX[i] as usize] = i as u8;
        i += 1;
    }
    inverse
};

/// A key expanded into its rounds' keys: 11 for AES-128, 15 for AES-256.
pub(crate) struct Aes {
    rounds: Vec<[u8; 16]>,
}

/// `a` times `b` in AES's field, GF(2⁸) by x⁸ + x⁴ + x³ + x + 1.
fn times(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        a = (a << 1) ^ if a & 0x80 != 0 { 0x1b } else { 0 };
        b >>= 1;
    }
    product
}

impl Aes {
    /// The cipher keyed by `key`, 16 or 32 bytes; none for another length.
    pub(crate) fn new(key: &[u8]) -> Option<Aes> {
        let nk = match key.len() {
            16 => 4,
            32 => 8,
            _ => return None,
        };
        let total = 4 * (nk + 7);
        let mut words: Vec<[u8; 4]> = key
            .chunks_exact(4)
            .map(|w| [w[0], w[1], w[2], w[3]])
            .collect();
        let mut rcon = 1u8;
        for i in nk..total {
            let mut t = words[i - 1];
            if i % nk == 0 {
                t = [
                    SBOX[t[1] as usize] ^ rcon,
                    SBOX[t[2] as usize],
                    SBOX[t[3] as usize],
                    SBOX[t[0] as usize],
                ];
                rcon = times(rcon, 2);
            } else if nk > 6 && i % nk == 4 {
                t = t.map(|b| SBOX[b as usize]);
            }
            let before = words[i - nk];
            words.push([
                before[0] ^ t[0],
                before[1] ^ t[1],
                before[2] ^ t[2],
                before[3] ^ t[3],
            ]);
        }
        let rounds = (words.chunks_exact(4))
            .map(|w| {
                let mut key = [0u8; 16];
                for (out, word) in key.chunks_exact_mut(4).zip(w) {
                    out.copy_from_slice(word);
                }
                key
            })
            .collect();
        Some(Aes { rounds })
    }

    /// One block encrypted.
    pub(crate) fn encrypt(&self, block: &mut [u8; 16]) {
        let last = self.rounds.len() - 1;
        add(block, &self.rounds[0]);
        for round in 1..=last {
            for b in block.iter_mut() {
                *b = SBOX[*b as usize];
            }
            shift(block, |r| r);
            if round != last {
                mix(block, [2, 3, 1, 1]);
            }
            add(block, &self.rounds[round]);
        }
    }

    /// One block decrypted.
    pub(crate) fn decrypt(&self, block: &mut [u8; 16]) {
        let last = self.rounds.len() - 1;
        add(block, &self.rounds[last]);
        for round in (0..last).rev() {
            shift(block, |r| (4 - r) % 4);
            for b in block.iter_mut() {
                *b = INVERSE[*b as usize];
            }
            add(block, &self.rounds[round]);
            if round != 0 {
                mix(block, [14, 11, 13, 9]);
            }
        }
    }

    /// `data`, whole blocks, decrypted in CBC from `iv`.
    pub(crate) fn decrypt_cbc(&self, iv: [u8; 16], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut before = iv;
        for block in data.chunks_exact(16) {
            let mut this: [u8; 16] = block.try_into().expect("a block");
            self.decrypt(&mut this);
            for (b, x) in this.iter_mut().zip(before) {
                *b ^= x;
            }
            out.extend_from_slice(&this);
            before = block.try_into().expect("a block");
        }
        out
    }

    /// `data`, whole blocks, encrypted in CBC from `iv`.
    pub(crate) fn encrypt_cbc(&self, iv: [u8; 16], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut before = iv;
        for block in data.chunks_exact(16) {
            let mut this: [u8; 16] = block.try_into().expect("a block");
            for (b, x) in this.iter_mut().zip(before) {
                *b ^= x;
            }
            self.encrypt(&mut this);
            out.extend_from_slice(&this);
            before = this;
        }
        out
    }
}

/// A round's key added.
fn add(block: &mut [u8; 16], key: &[u8; 16]) {
    for (b, k) in block.iter_mut().zip(key) {
        *b ^= k;
    }
}

/// Each row `r` of the state, column by column, rotated left by `by(r)`.
fn shift(block: &mut [u8; 16], by: impl Fn(usize) -> usize) {
    let state = *block;
    for r in 1..4 {
        for c in 0..4 {
            block[4 * c + r] = state[4 * ((c + by(r)) % 4) + r];
        }
    }
}

/// Each column multiplied by the circulant matrix of `row`.
fn mix(block: &mut [u8; 16], row: [u8; 4]) {
    for column in block.chunks_exact_mut(4) {
        let c = [column[0], column[1], column[2], column[3]];
        for (r, out) in column.iter_mut().enumerate() {
            *out = (0..4).fold(0, |x, i| x ^ times(row[(4 + i - r) % 4], c[i]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Aes;

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// FIPS 197, Appendix C: one block, each way, with each key size.
    #[test]
    fn fips_197s_examples() {
        let plain = bytes("00112233445566778899aabbccddeeff");
        let examples = [
            (
                "000102030405060708090a0b0c0d0e0f",
                "69c4e0d86a7b0430d8cdb78070b4c55a",
            ),
            (
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                "8ea2b7ca516745bfeafc49904b496089",
            ),
        ];
        for (key, cipher) in examples {
            let aes = Aes::new(&bytes(key)).unwrap();
            let mut block: [u8; 16] = plain.clone().try_into().unwrap();
            aes.encrypt(&mut block);
            assert_eq!(block.to_vec(), bytes(cipher), "{key}");
            aes.decrypt(&mut block);
            assert_eq!(block.to_vec(), plain, "{key}");
        }
        assert!(Aes::new(&[0; 24]).is_none());
    }

    /// SP 800-38A, F.1: ECB, a block at a time, with AES-128 and AES-256.
    #[test]
    fn sp_800_38as_ecb_examples() {
        let plain = bytes(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710",
        ));
        let examples = [
            (
                "2b7e151628aed2a6abf7158809cf4f3c",
                concat!(
                    "3ad77bb40d7a3660a89ecaf32466ef97",
                    "f5d3d58503b9699de785895a96fdbaaf",
                    "43b1cd7f598ece23881b00e3ed030688",
                    "7b0c785e27e8ad3f8223207104725dd4",
                ),
            ),
            (
                "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
                concat!(
                    "f3eed1bdb5d2a03c064b5a7e3db181f8",
                    "591ccb10d410ed26dc5ba74a31362870",
                    "b6ed21b99ca6f4f9f153e7b1beafed1d",
                    "23304b7a39f9f3ff067d8d8f9e24ecc7",
                ),
            ),
        ];
        for (key, cipher) in examples {
            let aes = Aes::new(&bytes(key)).unwrap();
            let cipher = bytes(cipher);
            for (p, c) in plain.chunks_exact(16).zip(cipher.chunks_exact(16)) {
                let mut block: [u8; 16] = p.try_into().unwrap();
                aes.encrypt(&mut block);
                assert_eq!(&block[..], c, "{key}");
                aes.decrypt(&mut block);
                assert_eq!(&block[..], p, "{key}");
            }
        }
    }

    /// SP 800-38A, F.2: CBC with AES-128 and AES-256, four blocks each way.
    #[test]
    fn sp_800_38as_cbc_examples() {
        let plain = bytes(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710",
        ));
        let iv: [u8; 16] = bytes("000102030405060708090a0b0c0d0e0f")
            .try_into()
            .unwrap();
        let examples = [
            (
                "2b7e151628aed2a6abf7158809cf4f3c",
                concat!(
                    "7649abac8119b246cee98e9b12e9197d",
                    "5086cb9b507219ee95db113a917678b2",
                    "73bed6b8e3c1743b7116e69e22229516",
                    "3ff1caa1681fac09120eca307586e1a7",
                ),
            ),
            (
                "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
                concat!(
                    "f58c4c04d6e5f1ba779eabfb5f7bfbd6",
                    "9cfc4e967edb808d679f777bc6702c7d",
                    "39f23369a9d9bacfa530e26304231461",
                    "b2eb05e2c39be9fcda6c19078c6a9d1b",
                ),
            ),
        ];
        for (key, cipher) in examples {
            let aes = Aes::new(&bytes(key)).unwrap();
            assert_eq!(aes.encrypt_cbc(iv, &plain), bytes(cipher), "{key}");
            assert_eq!(aes.decrypt_cbc(iv, &bytes(cipher)), plain, "{key}");
        }
    }
}
