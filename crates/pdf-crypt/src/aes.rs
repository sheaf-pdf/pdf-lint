//! AES with 128- and 256-bit keys, which revisions 4 to 6 of the standard
//! security handler encrypt with: RustCrypto's `aes`, a block at a time
//! (ECB) and in CBC by its `cbc`. Encrypting too, as revision 6's password
//! hash (Algorithm 2.B) does.

#[cfg(any(test, feature = "writer"))]
use ::aes::cipher::BlockCipherEncrypt;
use ::aes::cipher::array::Array;
use ::aes::cipher::{BlockCipherDecrypt, BlockModeDecrypt, BlockModeEncrypt, KeyInit, KeyIvInit};
use ::aes::{Aes128, Aes256, Block};

/// A key, 128 or 256 bits.
pub(crate) enum Aes {
    By128([u8; 16]),
    By256([u8; 32]),
}

impl Aes {
    /// The cipher keyed by `key`, 16 or 32 bytes; none for another length.
    pub(crate) fn new(key: &[u8]) -> Option<Aes> {
        match key.len() {
            16 => key.try_into().ok().map(Aes::By128),
            32 => key.try_into().ok().map(Aes::By256),
            _ => None,
        }
    }

    /// One block encrypted, as the writer encrypts `/Perms`.
    #[cfg(any(test, feature = "writer"))]
    pub(crate) fn encrypt(&self, block: &mut [u8; 16]) {
        let block: &mut Block = block.into();
        match self {
            Aes::By128(key) => Aes128::new(key.into()).encrypt_block(block),
            Aes::By256(key) => Aes256::new(key.into()).encrypt_block(block),
        }
    }

    /// One block decrypted.
    pub(crate) fn decrypt(&self, block: &mut [u8; 16]) {
        let block: &mut Block = block.into();
        match self {
            Aes::By128(key) => Aes128::new(key.into()).decrypt_block(block),
            Aes::By256(key) => Aes256::new(key.into()).decrypt_block(block),
        }
    }

    /// `data`'s whole blocks decrypted in CBC from `iv`.
    pub(crate) fn decrypt_cbc(&self, iv: [u8; 16], data: &[u8]) -> Vec<u8> {
        let mut out = data[..data.len() - data.len() % 16].to_vec();
        let (blocks, _): (&mut [Block], _) = Array::slice_as_chunks_mut(&mut out);
        let iv = (&iv).into();
        match self {
            Aes::By128(key) => cbc::Decryptor::<Aes128>::new(key.into(), iv).decrypt_blocks(blocks),
            Aes::By256(key) => cbc::Decryptor::<Aes256>::new(key.into(), iv).decrypt_blocks(blocks),
        }
        out
    }

    /// `data`'s whole blocks encrypted in CBC from `iv`.
    pub(crate) fn encrypt_cbc(&self, iv: [u8; 16], data: &[u8]) -> Vec<u8> {
        let mut out = data[..data.len() - data.len() % 16].to_vec();
        let (blocks, _): (&mut [Block], _) = Array::slice_as_chunks_mut(&mut out);
        let iv = (&iv).into();
        match self {
            Aes::By128(key) => cbc::Encryptor::<Aes128>::new(key.into(), iv).encrypt_blocks(blocks),
            Aes::By256(key) => cbc::Encryptor::<Aes256>::new(key.into(), iv).encrypt_blocks(blocks),
        }
        out
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
