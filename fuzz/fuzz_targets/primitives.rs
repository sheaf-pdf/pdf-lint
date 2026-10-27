//! pdf-crypt's primitives beside RustCrypto's, on any input: MD5, SHA-256,
//! SHA-384 and SHA-512 of it; RC4 with a 5- and a 16-byte key from it; AES
//! with a 16- and a 32-byte key from it, in CBC each way.

#![no_main]

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use md5::Digest;
use pdf_crypt::fuzz;
use rc4::StreamCipher;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    assert_eq!(fuzz::md5(data)[..], md5::Md5::digest(data)[..]);
    assert_eq!(fuzz::sha256(data)[..], sha2::Sha256::digest(data)[..]);
    assert_eq!(fuzz::sha384(data)[..], sha2::Sha384::digest(data)[..]);
    assert_eq!(fuzz::sha512(data)[..], sha2::Sha512::digest(data)[..]);
    let key = |n: usize| -> Vec<u8> { (0..n).map(|i| data.get(i).copied().unwrap_or(i as u8)).collect() };
    let mut five = data.to_vec();
    rc4::Rc4::<rc4::consts::U5>::new(GenericArray::from_slice(&key(5))).apply_keystream(&mut five);
    assert_eq!(fuzz::rc4(&key(5), data), five);
    let mut sixteen = data.to_vec();
    rc4::Rc4::<rc4::consts::U16>::new(GenericArray::from_slice(&key(16))).apply_keystream(&mut sixteen);
    assert_eq!(fuzz::rc4(&key(16), data), sixteen);
    let iv: [u8; 16] = std::array::from_fn(|i| data.get(data.len().saturating_sub(16) + i).copied().unwrap_or(0));
    let blocks = &data[..data.len() / 16 * 16];
    for n in [16, 32] {
        let k = key(n);
        // CBC by RustCrypto's block cipher, chained here.
        let (mut encrypted, mut decrypted) = (Vec::new(), Vec::new());
        let (mut before_e, mut before_d) = (iv, iv);
        for block in blocks.chunks_exact(16) {
            let mut e = GenericArray::clone_from_slice(block);
            for (b, x) in e.iter_mut().zip(before_e) {
                *b ^= x;
            }
            let mut d = GenericArray::clone_from_slice(block);
            match n {
                16 => {
                    let aes = aes::Aes128::new(GenericArray::from_slice(&k));
                    aes.encrypt_block(&mut e);
                    aes.decrypt_block(&mut d);
                }
                _ => {
                    let aes = aes::Aes256::new(GenericArray::from_slice(&k));
                    aes.encrypt_block(&mut e);
                    aes.decrypt_block(&mut d);
                }
            }
            for (b, x) in d.iter_mut().zip(before_d) {
                *b ^= x;
            }
            before_e.copy_from_slice(&e);
            before_d.copy_from_slice(block);
            encrypted.extend_from_slice(&e);
            decrypted.extend_from_slice(&d);
        }
        assert_eq!(fuzz::aes_cbc_encrypt(&k, iv, blocks), Some(encrypted), "AES-{} encrypt", n * 8);
        assert_eq!(fuzz::aes_cbc_decrypt(&k, iv, blocks), Some(decrypted), "AES-{} decrypt", n * 8);
    }
});
