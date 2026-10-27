//! The standard security handler (ISO 32000-2, 7.6.4): a file's key from
//! its encryption dictionary and a password, and its strings and streams
//! decrypted with it. Revision 5, which ISO 32000-2 leaves out, is read as
//! Adobe's Extension Level 3 has it, as qpdf reads it.

use crate::aes::Aes;
use crate::md5::md5;
use crate::rc4::rc4;
use crate::sha2::{sha256, sha384, sha512};
use crate::{CryptFilter, Dictionary, Error, Fault, Refused};

/// The padding a password of revisions 2 to 4 is filled out with
/// (Algorithm 2, step a).
pub(crate) const PADDING: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];

/// How a string or a stream is encrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    /// Not at all: `/Identity`, or a crypt filter without `/CFM`.
    Identity,
    /// `/CFM /None`: by something not the standard security handler,
    /// which leaves it as it is.
    None,
    /// RC4 (`/CFM /V2`, and everything in a file of `/V` 1 or 2).
    Rc4,
    /// AES-128, the key each object's (`/CFM /AESV2`).
    Aes128,
    /// AES-256, the file's key (`/CFM /AESV3`).
    Aes256,
    /// A crypt filter's method that isn't one, or a crypt filter the
    /// dictionary doesn't have, by this name.
    Unknown(Vec<u8>),
}

/// A file's security handler, opened: its key, and how its strings and
/// streams are encrypted.
#[derive(Clone, Debug)]
pub struct Handler {
    key: Vec<u8>,
    v: i64,
    owner: bool,
    strings: Method,
    streams: Method,
    filters: Vec<(Vec<u8>, Method)>,
    encrypt_metadata: bool,
    faults: Vec<Fault>,
}

impl Handler {
    /// The handler `dictionary` describes, for the file whose first `/ID`
    /// element is `id`, opened by the first of `passwords` that opens it,
    /// each tried as the owner's and then the user's; with none, the empty
    /// password.
    pub fn open(
        dictionary: &Dictionary,
        id: &[u8],
        passwords: &[&[u8]],
    ) -> Result<Handler, Refused> {
        let file = File::of(dictionary, id)?;
        let empty: [&[u8]; 1] = [b""];
        let passwords = match passwords.is_empty() {
            true => &empty[..],
            false => passwords,
        };
        let mut faults = Vec::new();
        let opened = passwords.iter().find_map(|password| {
            if let Some(key) = file.owner(password, &mut faults) {
                return Some((key, true));
            }
            file.user(password, &mut faults).map(|key| (key, false))
        });
        let Some((key, owner)) = opened else {
            return Err(Refused::Password);
        };
        let (strings, streams, filters) = methods(dictionary, file.v);
        Ok(Handler {
            key,
            v: file.v,
            owner,
            strings,
            streams,
            filters,
            encrypt_metadata: file.encrypt_metadata,
            faults,
        })
    }

    /// Whether it was opened by the owner's password, not the user's.
    pub fn owner(&self) -> bool {
        self.owner
    }

    /// The file's key.
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// How strings are encrypted (`/StrF`).
    pub fn strings(&self) -> &Method {
        &self.strings
    }

    /// How streams are encrypted (`/StmF`).
    pub fn streams(&self) -> &Method {
        &self.streams
    }

    /// Whether the document's metadata stream is encrypted
    /// (`/EncryptMetadata`, of a dictionary of `/V` 4 or later).
    pub fn encrypts_metadata(&self) -> bool {
        self.encrypt_metadata
    }

    /// What was wrong with the dictionary though it opened: `/Perms` that
    /// doesn't match, as a file of revision 5 or 6 can have.
    pub fn faults(&self) -> &[Fault] {
        &self.faults
    }

    /// How a stream whose own `/Crypt` filter is named `name` is
    /// encrypted: `/Identity` by name, else the dictionary's crypt filter
    /// of that name.
    pub fn named(&self, name: &[u8]) -> Method {
        match name {
            b"Identity" => Method::Identity,
            _ => (self.filters.iter())
                .find(|(n, _)| n == name)
                .map_or_else(|| Method::Unknown(name.to_vec()), |(_, m)| m.clone()),
        }
    }

    /// The string `data` of object `id`, decrypted.
    pub fn decrypt_string(&self, id: (u32, u16), data: &[u8]) -> Result<Vec<u8>, Error> {
        self.decrypt(&self.strings, id, data)
    }

    /// The data of stream `id`, decrypted: by its own `/Crypt` filter's
    /// method when it has one (`crypt`, see [`Handler::named`]), else as
    /// streams are.
    pub fn decrypt_stream(
        &self,
        id: (u32, u16),
        data: &[u8],
        crypt: Option<&Method>,
    ) -> Result<Vec<u8>, Error> {
        self.decrypt(crypt.unwrap_or(&self.streams), id, data)
    }

    /// `data` of object `id` encrypted by `method`, AES's from `iv`, for
    /// the writer.
    #[cfg(any(test, feature = "writer"))]
    pub(crate) fn encrypt(
        &self,
        method: &Method,
        id: (u32, u16),
        data: &[u8],
        iv: [u8; 16],
    ) -> Vec<u8> {
        let aes = |key: &[u8]| {
            let pad = 16 - data.len() % 16;
            let padded = [data, &vec![pad as u8; pad]].concat();
            let cipher = Aes::new(key).expect("a key of 16 or 32 bytes");
            [&iv[..], &cipher.encrypt_cbc(iv, &padded)].concat()
        };
        match method {
            Method::Identity | Method::None | Method::Unknown(_) => data.to_vec(),
            Method::Rc4 => rc4(&self.object_key(id, false), data),
            Method::Aes128 => aes(&self.object_key(id, true)),
            Method::Aes256 => aes(&self.key),
        }
    }

    /// `data` of object `id` decrypted by `method`.
    fn decrypt(&self, method: &Method, id: (u32, u16), data: &[u8]) -> Result<Vec<u8>, Error> {
        match method {
            Method::Identity | Method::None => Ok(data.to_vec()),
            Method::Unknown(name) => Err(Error {
                data: None,
                fault: Fault::Method {
                    name: String::from_utf8_lossy(name).into_owned(),
                },
            }),
            Method::Rc4 => Ok(rc4(&self.object_key(id, false), data)),
            Method::Aes128 => aes(&self.object_key(id, true), data),
            Method::Aes256 => aes(&self.key, data),
        }
    }

    /// Object `id`'s key (Algorithm 1): the file's key, its number and
    /// generation, and for AES `sAlT`, through MD5, the first n + 5 bytes
    /// up to 16. Revision 5 and later use the file's key (Algorithm 1.A).
    fn object_key(&self, (n, g): (u32, u16), aes: bool) -> Vec<u8> {
        if self.v >= 5 {
            return self.key.clone();
        }
        let mut input = self.key.clone();
        input.extend_from_slice(&n.to_le_bytes()[..3]);
        input.extend_from_slice(&g.to_le_bytes());
        if aes {
            input.extend_from_slice(b"sAlT");
        }
        md5(&input)[..(self.key.len() + 5).min(16)].to_vec()
    }
}

/// AES-CBC ciphertext `data` decrypted: its first 16 bytes the IV, its
/// padding taken off. Empty, it's empty; without a whole block past the
/// IV, it stays ciphertext; white-space past its whole blocks, as a stream
/// whose `/Length` was recovered has, is left off, and another part block
/// too, said; padding that isn't, kept, said.
fn aes(key: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut data = data;
    while data.len() % 16 != 0 && data.last().is_some_and(|b| b" \t\r\n\x0c\0".contains(b)) {
        data = &data[..data.len() - 1];
    }
    if data.len() < 32 {
        return Err(Error {
            data: None,
            fault: Fault::Short { len: data.len() },
        });
    }
    let extra = data.len() % 16;
    let whole = &data[..data.len() - extra];
    // A crypt filter's AES with a key of another length, as AESV3 in a file
    // whose key is RC4's is.
    let Some(cipher) = Aes::new(key) else {
        return Err(Error {
            data: None,
            fault: Fault::Key { len: key.len() },
        });
    };
    let iv: [u8; 16] = whole[..16].try_into().expect("a block");
    let mut plain = cipher.decrypt_cbc(iv, &whole[16..]);
    let pad = usize::from(*plain.last().expect("a block"));
    let padded = (1..=16).contains(&pad)
        && plain[plain.len() - pad..]
            .iter()
            .all(|&b| usize::from(b) == pad);
    let fault = match (extra, padded) {
        (0, true) => None,
        (0, false) => Some(Fault::Padding),
        (extra, _) => Some(Fault::PartBlock { extra }),
    };
    if padded {
        plain.truncate(plain.len() - pad);
    }
    match fault {
        None => Ok(plain),
        Some(fault) => Err(Error {
            data: Some(plain),
            fault,
        }),
    }
}

/// What opening a file's handler takes: its dictionary's values, checked
/// and filled out.
pub(crate) struct File<'a> {
    v: i64,
    r: i64,
    /// The key's length in bytes.
    n: usize,
    o: Vec<u8>,
    u: Vec<u8>,
    oe: Vec<u8>,
    ue: Vec<u8>,
    perms: Vec<u8>,
    p: u32,
    id: &'a [u8],
    encrypt_metadata: bool,
}

impl<'a> File<'a> {
    /// `dictionary`'s values, unless they're not ones the standard
    /// security handler takes. Short ones are filled out with zeros, as
    /// qpdf does, and revision 5's and 6's long ones cut short.
    pub(crate) fn of(dictionary: &Dictionary, id: &'a [u8]) -> Result<File<'a>, Refused> {
        let refuse = |why: &str| Err(Refused::Unsupported(why.to_string()));
        if dictionary.filter != b"Standard" {
            let filter = String::from_utf8_lossy(&dictionary.filter).into_owned();
            return Err(Refused::Unsupported(format!(
                "its security handler is /{filter}, not /Standard"
            )));
        }
        let (v, r) = (dictionary.v, dictionary.r);
        match (v, r) {
            (1 | 2 | 4, 2..=4) | (5, 5 | 6) => {}
            _ => {
                return Err(Refused::Unsupported(format!(
                    "it's /V {v} /R {r}, which the standard security handler isn't"
                )));
            }
        }
        if dictionary.o.is_empty() || dictionary.u.is_empty() {
            return refuse("it has no /O or /U");
        }
        let fill = |value: &[u8], len: usize| {
            let mut value = value[..value.len().min(len)].to_vec();
            value.resize(len, 0);
            value
        };
        let (o, u) = match v {
            5 => (fill(&dictionary.o, 48), fill(&dictionary.u, 48)),
            _ if dictionary.o.len() > 32 || dictionary.u.len() > 32 => {
                return refuse("its /O or /U is over 32 bytes");
            }
            _ => (fill(&dictionary.o, 32), fill(&dictionary.u, 32)),
        };
        let n = match (v, r) {
            (1, _) | (_, 2) => 5,
            (2, _) => match dictionary.length {
                Some(bits @ 40..=128) if bits % 8 == 0 => (bits / 8) as usize,
                _ => 16,
            },
            (4, _) => 16,
            _ => 32,
        };
        Ok(File {
            v,
            r,
            n,
            o,
            u,
            oe: fill(&dictionary.oe, 32),
            ue: fill(&dictionary.ue, 32),
            perms: fill(&dictionary.perms, 16),
            p: dictionary.p as u32,
            id,
            encrypt_metadata: v < 4 || dictionary.encrypt_metadata.unwrap_or(true),
        })
    }

    /// The file's key, when `password` is the user's.
    fn user(&self, password: &[u8], faults: &mut Vec<Fault>) -> Option<Vec<u8>> {
        match self.r {
            2..=4 => {
                let key = self.key(password);
                (self.u_value(&key)[..self.compared()] == self.u[..self.compared()]).then_some(key)
            }
            _ => {
                let password = &password[..password.len().min(127)];
                if self.hash(password, &self.u[32..40], b"") != self.u[..32] {
                    return None;
                }
                let intermediate = self.hash(password, &self.u[40..48], b"");
                Some(self.unwrapped(&intermediate, &self.ue, faults))
            }
        }
    }

    /// The file's key, when `password` is the owner's.
    fn owner(&self, password: &[u8], faults: &mut Vec<Fault>) -> Option<Vec<u8>> {
        match self.r {
            // Algorithm 7: the owner's key decrypts /O to the user's
            // password, padded, which opens the file as the user's does.
            2..=4 => {
                let key = self.owner_key(password);
                let mut user = self.o.clone();
                for i in (0..self.rounds()).rev() {
                    let round: Vec<u8> = key.iter().map(|k| k ^ i as u8).collect();
                    user = rc4(&round, &user);
                }
                let key = self.key(&user);
                (self.u_value(&key)[..self.compared()] == self.u[..self.compared()]).then_some(key)
            }
            _ => {
                let password = &password[..password.len().min(127)];
                if self.hash(password, &self.o[32..40], &self.u[..48]) != self.o[..32] {
                    return None;
                }
                let intermediate = self.hash(password, &self.o[40..48], &self.u[..48]);
                Some(self.unwrapped(&intermediate, &self.oe, faults))
            }
        }
    }

    /// How many bytes of /U a password's own is compared with: 16 from
    /// revision 3, else the key's, as qpdf compares.
    fn compared(&self) -> usize {
        match self.r {
            2 => self.n,
            _ => 16,
        }
    }

    /// RC4's rounds over /O and /U: 20 from revision 3, else 1.
    pub(crate) fn rounds(&self) -> usize {
        match self.r {
            2 => 1,
            _ => 20,
        }
    }

    /// `password` padded or cut to 32 bytes (Algorithm 2, step a).
    pub(crate) fn padded(password: &[u8]) -> Vec<u8> {
        let password = &password[..password.len().min(32)];
        [password, &PADDING[..32 - password.len()]].concat()
    }

    /// MD5 repeated 50 times over its first n bytes from revision 3,
    /// those n bytes.
    fn iterated(&self, mut digest: [u8; 16]) -> Vec<u8> {
        if self.r >= 3 {
            for _ in 0..50 {
                digest = md5(&digest[..self.n.min(16)]);
            }
        }
        digest[..self.n.min(16)].to_vec()
    }

    /// The file's key from the user's `password` (Algorithm 2).
    pub(crate) fn key(&self, password: &[u8]) -> Vec<u8> {
        let mut input = File::padded(password);
        input.extend_from_slice(&self.o);
        input.extend_from_slice(&self.p.to_le_bytes());
        input.extend_from_slice(self.id);
        if self.r >= 4 && !self.encrypt_metadata {
            input.extend_from_slice(&[0xff; 4]);
        }
        self.iterated(md5(&input))
    }

    /// The RC4 key /O is encrypted by, from the owner's `password`
    /// (Algorithm 3, steps a to d).
    pub(crate) fn owner_key(&self, password: &[u8]) -> Vec<u8> {
        self.iterated(md5(&File::padded(password)))
    }

    /// What /U is for the file's `key` (Algorithms 4 and 5): its first 16
    /// bytes from revision 3, as only they're compared.
    pub(crate) fn u_value(&self, key: &[u8]) -> Vec<u8> {
        match self.r {
            2 => rc4(key, &PADDING),
            _ => {
                let mut value = md5(&[&PADDING[..], self.id].concat()).to_vec();
                for i in 0..20u8 {
                    let round: Vec<u8> = key.iter().map(|k| k ^ i).collect();
                    value = rc4(&round, &value);
                }
                value
            }
        }
    }

    /// Revision 5's and 6's hash of `password`, `salt` and `udata`
    /// (Algorithm 2.B; revision 5's, SHA-256 alone).
    fn hash(&self, password: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
        hash(self.r, password, salt, udata)
    }

    /// The file's key, `wrapped` (/OE or /UE) by AES-256 in CBC from a zero
    /// IV, with no padding, under `intermediate`; and /Perms checked
    /// against it.
    fn unwrapped(&self, intermediate: &[u8], wrapped: &[u8], faults: &mut Vec<Fault>) -> Vec<u8> {
        let cipher = Aes::new(&intermediate[..32]).expect("32 bytes");
        let key = cipher.decrypt_cbc([0; 16], wrapped);
        let mut perms: [u8; 16] = self.perms.clone().try_into().expect("16 bytes");
        Aes::new(&key).expect("32 bytes").decrypt(&mut perms);
        let metadata = if self.encrypt_metadata { b'T' } else { b'F' };
        let expected = [&self.p.to_le_bytes()[..], &[0xff; 4], &[metadata], b"adb"].concat();
        if perms[..12] != expected[..] && !faults.contains(&Fault::Perms) {
            faults.push(Fault::Perms);
        }
        key
    }
}

/// Revision 5's and 6's hash (Algorithm 2.B): SHA-256 of `password`,
/// `salt` and `udata`; and from revision 6, rounds of AES-128 and SHA-2
/// over it until at least 64, and the last byte of the round's AES no more
/// than its number less 32, the starting hash round 0.
pub(crate) fn hash(r: i64, password: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
    let mut k = sha256(&[password, salt, udata].concat()).to_vec();
    if r < 6 {
        return k;
    }
    let mut round = 0u32;
    loop {
        round += 1;
        let block = [password, &k, udata].concat();
        let k1 = block.repeat(64);
        let cipher = Aes::new(&k[..16]).expect("16 bytes");
        let iv: [u8; 16] = k[16..32].try_into().expect("16 bytes");
        let e = cipher.encrypt_cbc(iv, &k1);
        let which = e[..16].iter().map(|&b| u32::from(b)).sum::<u32>() % 3;
        k = match which {
            0 => sha256(&e).to_vec(),
            1 => sha384(&e).to_vec(),
            _ => sha512(&e).to_vec(),
        };
        if round >= 64 && u32::from(*e.last().expect("a block")) <= round - 32 {
            break;
        }
    }
    k.truncate(32);
    k
}

/// How strings and streams are encrypted, and the crypt filters by name,
/// for a dictionary of version `v`.
fn methods(dictionary: &Dictionary, v: i64) -> (Method, Method, Vec<(Vec<u8>, Method)>) {
    if v < 4 {
        return (Method::Rc4, Method::Rc4, Vec::new());
    }
    let filters: Vec<(Vec<u8>, Method)> = (dictionary.crypt_filters.iter())
        .map(|CryptFilter { name, method, .. }| {
            let method = match method.as_deref() {
                None => Method::Identity,
                Some(b"None") => Method::None,
                Some(b"V2") => Method::Rc4,
                Some(b"AESV2") => Method::Aes128,
                Some(b"AESV3") => Method::Aes256,
                Some(other) => Method::Unknown(other.to_vec()),
            };
            (name.clone(), method)
        })
        .collect();
    let by = |name: &Option<Vec<u8>>| match name.as_deref() {
        None | Some(b"Identity") => Method::Identity,
        Some(name) => (filters.iter())
            .find(|(n, _)| n == name)
            .map_or_else(|| Method::Unknown(name.to_vec()), |(_, m)| m.clone()),
    };
    (by(&dictionary.str_f), by(&dictionary.stm_f), filters)
}
