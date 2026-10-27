//! Encrypting as the handler decrypts, to write files to test a reader
//! with: an encryption dictionary for a user's and an owner's password,
//! and strings and streams encrypted under it. Its salts, keys and IVs are
//! given, not random, so it's no way to protect a document.

use crate::aes::Aes;
use crate::handler::{File, Handler, Method, hash};
use crate::rc4::rc4;
use crate::sha2::sha256;
use crate::{CryptFilter, Dictionary, Fault};

/// The kind of encryption to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// RC4 with a 40-bit key: `/V 1 /R 2`.
    Rc4By40,
    /// RC4 with a 128-bit key: `/V 2 /R 3`.
    Rc4By128,
    /// AES-128 through a crypt filter: `/V 4 /R 4`, `/CFM /AESV2`.
    Aes128,
    /// AES-256 through a crypt filter: `/V 5 /R 5`, Adobe's Extension
    /// Level 3, `/CFM /AESV3`.
    Aes256R5,
    /// AES-256 through a crypt filter: `/V 5 /R 6`, ISO 32000-2,
    /// `/CFM /AESV3`.
    Aes256,
}

/// What to encrypt with.
#[derive(Clone, Copy, Debug)]
pub struct Settings<'a> {
    /// The kind.
    pub kind: Kind,
    /// The user's password.
    pub user: &'a [u8],
    /// The owner's password.
    pub owner: &'a [u8],
    /// `/P`, the permissions.
    pub p: i64,
    /// The file's first `/ID` element.
    pub id: &'a [u8],
    /// `/EncryptMetadata`, for `/V` 4 and 5.
    pub encrypt_metadata: bool,
}

/// The encryption dictionary `settings` describe.
pub fn dictionary(settings: &Settings) -> Dictionary {
    let Settings {
        kind,
        user,
        owner,
        p,
        id,
        encrypt_metadata,
    } = *settings;
    let (v, r, length, method) = match kind {
        Kind::Rc4By40 => (1, 2, Some(40), None),
        Kind::Rc4By128 => (2, 3, Some(128), None),
        Kind::Aes128 => (4, 4, Some(128), Some(&b"AESV2"[..])),
        Kind::Aes256R5 => (5, 5, Some(256), Some(&b"AESV3"[..])),
        Kind::Aes256 => (5, 6, Some(256), Some(&b"AESV3"[..])),
    };
    let mut dictionary = Dictionary {
        filter: b"Standard".to_vec(),
        v,
        r,
        length,
        p: Some(p),
        encrypt_metadata: (v >= 4).then_some(encrypt_metadata),
        ..Dictionary::default()
    };
    if let Some(method) = method {
        dictionary.crypt_filters = vec![CryptFilter {
            name: b"StdCF".to_vec(),
            method: Some(method.to_vec()),
        }];
        dictionary.stm_f = Some(b"StdCF".to_vec());
        dictionary.str_f = Some(b"StdCF".to_vec());
    }
    match r {
        2..=4 => {
            // Algorithm 3: /O, the user's password encrypted by the owner's.
            dictionary.o = Some(vec![0; 32]);
            dictionary.u = Some(vec![0; 32]);
            let file = File::of(&dictionary, id).expect("a dictionary the handler takes");
            let owner_key = file.owner_key(if owner.is_empty() { user } else { owner });
            let mut o = File::padded(user);
            for i in 0..file.rounds() {
                let round: Vec<u8> = owner_key.iter().map(|k| k ^ i as u8).collect();
                o = rc4(&round, &o);
            }
            dictionary.o = Some(o);
            // Algorithms 4 and 5: /U, from the file's key.
            let file = File::of(&dictionary, id).expect("a dictionary the handler takes");
            let mut u = file.u_value(&file.key(user));
            u.resize(32, 0);
            dictionary.u = Some(u);
        }
        _ => {
            // Algorithms 8, 9 and 10, with salts and a key from what's given.
            let key = sha256(&[id, user, owner].concat()).to_vec();
            let salt =
                |n: u8| -> Vec<u8> { (0..8).map(|i| n.wrapping_mul(31).wrapping_add(i)).collect() };
            let wrap = |intermediate: &[u8]| {
                Aes::new(&intermediate[..32])
                    .expect("32 bytes")
                    .encrypt_cbc([0; 16], &key)
            };
            let user = &user[..user.len().min(127)];
            let owner = &owner[..owner.len().min(127)];
            let (uv, uk, ov, ok) = (salt(1), salt(2), salt(3), salt(4));
            let u = [hash(r, user, &uv, b""), uv, uk.clone()].concat();
            dictionary.ue = Some(wrap(&hash(r, user, &uk, b"")));
            dictionary.o = Some([hash(r, owner, &ov, &u), ov, ok.clone()].concat());
            dictionary.oe = Some(wrap(&hash(r, owner, &ok, &u)));
            dictionary.u = Some(u);
            let metadata = if encrypt_metadata { b'T' } else { b'F' };
            let mut perms: [u8; 16] = [
                &(p as u32).to_le_bytes()[..],
                &[0xff; 4],
                &[metadata],
                b"adb",
                b"rand",
            ]
            .concat()
            .try_into()
            .expect("16 bytes");
            Aes::new(&key).expect("32 bytes").encrypt(&mut perms);
            dictionary.perms = Some(perms.to_vec());
        }
    }
    dictionary
}

/// String `data` of object `id` encrypted by `handler`, AES's from `iv`;
/// or [`Fault::Key`], when it's AES that the file's key isn't for.
pub fn encrypt_string(
    handler: &Handler,
    id: (u32, u16),
    data: &[u8],
    iv: [u8; 16],
) -> Result<Vec<u8>, Fault> {
    handler.encrypt(handler.strings(), id, data, iv)
}

/// Stream `id`'s `data` encrypted by `handler`, by `crypt` when it names
/// the stream's own crypt filter's method, AES's from `iv`; or
/// [`Fault::Key`], when it's AES that the file's key isn't for.
pub fn encrypt_stream(
    handler: &Handler,
    id: (u32, u16),
    data: &[u8],
    crypt: Option<&Method>,
    iv: [u8; 16],
) -> Result<Vec<u8>, Fault> {
    handler.encrypt(crypt.unwrap_or(handler.streams()), id, data, iv)
}
