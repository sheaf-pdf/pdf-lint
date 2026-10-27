//! The handler on files qpdf wrote, each way it encrypts, and on what the
//! writer writes; and its faults and refusals.

use crate::writer::{self, Kind, Settings};
use crate::{CryptFilter, Dictionary, Error, Fault, Handler, Method, Refused};

/// A file qpdf 12.3 encrypted (`qpdf --encrypt`, the vector's name its
/// options): its encryption dictionary and first `/ID`, its passwords, the
/// key qpdf gives (`--show-encryption-key`), and its `/Info`'s `/Title`,
/// `A plain title`, as written, with its object's number.
struct Vector {
    name: &'static str,
    dictionary: Dictionary,
    id: &'static [u8],
    user: &'static [u8],
    owner: &'static [u8],
    key: &'static str,
    title: (u32, &'static [u8]),
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn vectors() -> Vec<Vector> {
    vec![
    Vector {
        name: "rc4_40",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 1,
            r: 2,
            length: Some(40),
            o: Some(b"\x94\xe8\x09\x44\x19\x66\x2a\x77\x44\x42\xfb\x07\x2e\x3d\x9f\x19\xe9\xd1\x30\xec\x09\xa4\xd0\x06\x1e\x78\xfe\x92\x0f\x7a\xb6\x2f".to_vec()),
            u: Some(b"\xbf\x39\x6e\x5d\x5d\xbc\x01\x24\x7e\x85\xf6\x2c\x5f\x4a\x7e\x41\x49\x2c\x45\x3e\x10\x00\xb8\x78\x51\x40\x7b\xc9\x29\xa6\x54\x76".to_vec()),
            oe: None,
            ue: None,
            perms: None,
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![],
            stm_f: None,
            str_f: None,
        },
        id: b"\x88\x3e\x20\xa9\x41\x92\xfc\xed\x32\xd5\x9f\x47\x41\x4e\x85\xdc",
        user: b"\x75\x73\x65\x72",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "75bbd4d82e",
        title: (2, b"\xe8\xd6\x58\xff\x70\x5c\x07\x2c\x53\xe5\x18\xfc\x52"),
    },
    Vector {
        name: "rc4_128",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 2,
            r: 3,
            length: Some(128),
            o: Some(b"\x0b\xa3\x83\x5f\x88\xf9\x03\x88\xe7\x4e\x54\x58\x41\x25\xce\x14\x2b\xe0\xde\x24\xc6\xb0\xd3\x77\x46\xe0\x75\xb8\x91\x75\x66\x71".to_vec()),
            u: Some(b"\xeb\x4c\x43\x85\x72\xc4\xdb\x5d\x96\x59\x3d\x00\x6c\xe3\xb9\x5e\x00\x21\x44\x69\x90\xb9\xe4\x11\x40\x71\xa4\xd9\x10\x49\x84\xc1".to_vec()),
            oe: None,
            ue: None,
            perms: None,
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![],
            stm_f: None,
            str_f: None,
        },
        id: b"\x3f\xaa\x4e\x99\xd7\xa1\x35\x4d\x36\xbc\x61\x4e\x30\x56\x23\xde",
        user: b"\x75\x73\x65\x72",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "955705db0e0c61345f8302d0b34f4585",
        title: (2, b"\x28\x3d\x75\xbe\x26\x4a\x48\xce\xc6\xa3\x16\x47\x9c"),
    },
    Vector {
        name: "rc4_128_v4",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 4,
            r: 4,
            length: Some(128),
            o: Some(b"\x0b\xa3\x83\x5f\x88\xf9\x03\x88\xe7\x4e\x54\x58\x41\x25\xce\x14\x2b\xe0\xde\x24\xc6\xb0\xd3\x77\x46\xe0\x75\xb8\x91\x75\x66\x71".to_vec()),
            u: Some(b"\x77\xe0\x71\xba\xf3\x19\x56\x2a\x71\x9c\x67\xed\x4d\x08\x6d\x28\x00\x21\x44\x69\x90\xb9\xe4\x11\x40\x71\xa4\xd9\x10\x49\x84\xc1".to_vec()),
            oe: None,
            ue: None,
            perms: None,
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"V2".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\xda\xd6\x9b\xf5\x32\x1a\x12\x0d\x6b\x5e\x14\x42\xc6\x45\x9d\x1c",
        user: b"\x75\x73\x65\x72",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "d9ae3a6556f03bf8824b3e19090d4f7a",
        title: (2, b"\xd9\xa5\x49\xc3\x0a\x2f\x56\x6d\xc2\x87\x0c\xce\xfb"),
    },
    Vector {
        name: "aes_128",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 4,
            r: 4,
            length: Some(128),
            o: Some(b"\x0b\xa3\x83\x5f\x88\xf9\x03\x88\xe7\x4e\x54\x58\x41\x25\xce\x14\x2b\xe0\xde\x24\xc6\xb0\xd3\x77\x46\xe0\x75\xb8\x91\x75\x66\x71".to_vec()),
            u: Some(b"\xcc\x59\xe7\x65\x00\xfb\x66\xd8\x49\xe5\x23\x45\x01\x94\x7c\x29\x00\x21\x44\x69\x90\xb9\xe4\x11\x40\x71\xa4\xd9\x10\x49\x84\xc1".to_vec()),
            oe: None,
            ue: None,
            perms: None,
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"AESV2".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\x59\x97\xda\x7a\xe0\x51\xae\x9e\xc7\xf4\x3b\x3f\x16\xe9\x54\x09",
        user: b"\x75\x73\x65\x72",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "57c38843adcf37899e7b913b029ad693",
        title: (2, b"\x7e\x9e\xd2\x2e\xd6\x2b\x60\x49\x31\x8f\x21\xc5\xa9\x2f\x0b\x10\x9f\xa9\xa2\x93\x61\xd3\xae\x06\x12\x16\x0a\xd9\x67\x68\xf8\x3f"),
    },
    Vector {
        name: "aes_128_cleartext_metadata",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 4,
            r: 4,
            length: Some(128),
            o: Some(b"\x0b\xa3\x83\x5f\x88\xf9\x03\x88\xe7\x4e\x54\x58\x41\x25\xce\x14\x2b\xe0\xde\x24\xc6\xb0\xd3\x77\x46\xe0\x75\xb8\x91\x75\x66\x71".to_vec()),
            u: Some(b"\xcd\xbb\x5e\xc7\x36\x73\xb9\x1d\xde\x50\x62\xcb\xaf\x86\x20\x89\x00\x21\x44\x69\x90\xb9\xe4\x11\x40\x71\xa4\xd9\x10\x49\x84\xc1".to_vec()),
            oe: None,
            ue: None,
            perms: None,
            p: Some(-4),
            encrypt_metadata: Some(false),
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"AESV2".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\x3e\xff\x7c\xba\xce\xde\xa5\x5a\xff\x15\xd4\x79\xa4\x8b\xbb\x8e",
        user: b"\x75\x73\x65\x72",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "7809443bdacf4f7c858df90e42520e8e",
        title: (2, b"\x1e\x75\x93\xe7\xdd\x48\x96\xf1\x84\xe2\x0f\x3c\xad\x34\x85\xa6\x73\xc9\xa7\xd7\xda\x0b\xd7\xee\xe4\x64\x2b\x5a\x3a\x28\xed\x45"),
    },
    Vector {
        name: "aes_256_r5",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 5,
            r: 5,
            length: Some(256),
            o: Some(b"\x6b\x12\xaf\x84\x00\xd0\x17\x39\xb4\x9a\x71\x56\x37\xd3\x37\xf7\xbf\xb3\x3c\xdb\x5f\x7f\x4b\x6e\x44\x00\x3c\x23\x0c\xf7\xf0\xd4\x73\x88\x8e\xf2\xe7\x5c\x62\x5b\xe1\xd2\xff\x26\x67\xb0\xe5\x7b".to_vec()),
            u: Some(b"\x8c\x80\x61\xf7\x83\x9f\x4b\x31\xda\x26\x3e\x72\xf7\xfc\x0e\x6b\x0c\xac\xfc\x57\x97\xdc\x8e\x67\x56\x5a\x63\x6d\x8d\xb7\xe7\xe2\xc8\x24\x43\x7c\x41\x48\xaa\xbc\x3c\xbc\x18\x96\x15\x8f\xa7\x6d".to_vec()),
            oe: Some(b"\x52\x97\x7f\x95\xab\xe8\x32\x06\x31\x54\x24\x52\x27\x59\x9c\x9c\x8b\x44\xee\x87\x40\xcd\xe1\x8f\xb3\xa6\xfd\x12\x59\x58\xa8\x65".to_vec()),
            ue: Some(b"\x03\x98\x51\x2c\x9b\xc1\x28\x7d\x72\x35\x13\x37\xa0\x99\x49\x59\x3e\x38\x10\xf9\xcb\x8b\x7e\xd8\x21\xd1\xa2\x9c\x23\x37\xc6\x65".to_vec()),
            perms: Some(b"\x7d\x23\x74\x47\xac\xb4\x91\x24\xba\xeb\x3f\x83\x01\x89\xc7\x5e".to_vec()),
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"AESV3".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\x41\x36\x6a\xd7\x05\x21\x6b\xf6\xa6\x47\x70\x64\xc7\x5d\x9a\x7d",
        user: b"\x75\x73\x65\x72",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "e77cbcb621c387a474253796581ac9ad1511f81a667642c007d0afa695bc987d",
        title: (2, b"\x06\xd0\xe9\x68\xcf\x5f\xa4\x63\x3c\x33\x94\xcc\x59\xdd\x9b\x86\x33\x40\x1e\xe6\x01\xf5\xb6\xf8\xd7\x03\x01\x8d\x44\x50\xca\xde"),
    },
    Vector {
        name: "aes_256",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 5,
            r: 6,
            length: Some(256),
            o: Some(b"\x32\x41\xc6\x84\xf2\x7d\xe5\xe9\xae\xee\x56\xd8\xbb\xc8\x8a\x53\x3b\x6b\xe6\x6e\xb9\x9b\xa2\x54\x4d\xef\x4e\x53\x62\xee\x0d\xf0\x81\xde\x64\xa8\x2c\x63\xa7\xfe\xd4\x10\x1c\x0c\xd8\xce\x79\x77".to_vec()),
            u: Some(b"\x8c\x99\x99\xed\xf5\x8a\x98\x9b\xa8\x89\x34\x77\x2f\x97\x74\x8f\x38\xcf\xc6\x5f\x64\xd6\xfe\x07\xd2\x96\x89\x89\xc8\xa6\x11\x3a\x9e\xec\xc2\x83\xc2\xc2\x74\x8e\x3c\x74\x34\xa4\xff\xd9\xdd\x3d".to_vec()),
            oe: Some(b"\xfe\x02\xf7\x97\x50\x91\x57\xbd\xea\x8c\x6c\x80\x24\xec\x5b\xe7\xe3\x7d\xfc\xb6\x91\x4b\xdf\x2b\xc3\x1f\x7f\x41\x0e\x11\x22\x73".to_vec()),
            ue: Some(b"\x78\x19\xee\x47\x48\x00\x8e\xa0\xbc\x09\xad\x93\xe1\x7a\xf7\x8f\xea\x4d\x07\x01\xaa\xa0\xfc\x7c\xb5\xb7\xd3\xd5\x9c\xd3\xb9\x84".to_vec()),
            perms: Some(b"\x00\x62\x7a\x5a\x17\xb0\x03\x73\xc3\x21\xc2\xbc\x06\x11\x08\x2b".to_vec()),
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"AESV3".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\x04\xa4\xb5\x97\x18\x8e\xef\xa2\x70\x3c\x15\x51\x2f\xa7\xeb\xca",
        user: b"\x75\x73\x65\x72",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "f1919acc4fe4850a97575f94188ea587c8de7b85a16d7a348315e958e0ea957f",
        title: (2, b"\xed\xde\xb1\x65\xcd\xb2\xbc\x12\x61\xe9\xde\x12\xeb\x3f\xf1\xdc\x73\x26\x58\x79\x02\x20\x1b\xd8\xd6\xe7\xfe\xbc\x93\x73\x26\xcc"),
    },
    Vector {
        name: "aes_256_cleartext_metadata",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 5,
            r: 6,
            length: Some(256),
            o: Some(b"\x63\xd6\xff\x35\xfb\x83\x33\x69\xbb\x68\xdd\x47\x2d\xbb\xd4\x17\xa0\xc3\xae\x46\x99\xe0\xf1\x66\x3e\xc6\x46\x29\x36\xa3\xb2\x0a\x14\xf0\x24\xae\xa5\xcd\x67\x6d\x8a\xa9\x49\x82\xfd\xd8\x28\x89".to_vec()),
            u: Some(b"\x7f\xfd\xc4\x41\x90\xff\xf7\xab\xb7\x91\xef\x8b\x22\xc3\x01\x00\x9b\xbc\x0f\x5e\x8c\x89\xaf\xf4\x85\xf5\xf3\x45\x73\xe1\x68\x5f\x60\x9c\xa0\xa7\x3b\x4e\x50\x55\xf7\x89\x88\xdc\xf9\x89\xf7\xf2".to_vec()),
            oe: Some(b"\x87\x84\xa3\xc6\x62\xe4\xc2\xc7\x26\x28\xde\x45\x1a\x05\x5c\xa4\x72\xa8\x55\xba\xdf\xa2\x7a\xe1\x46\xf0\xd4\x8b\xc9\xa5\x98\xba".to_vec()),
            ue: Some(b"\xae\x59\xb0\xad\x1c\x2e\x0f\x8b\x99\x67\x00\x6b\xed\xfe\x3e\x5b\x32\x62\xc5\x80\x83\xeb\xec\xe5\x07\x43\x5a\x17\x13\x4c\x6b\xe7".to_vec()),
            perms: Some(b"\x4f\x56\x54\xc6\x82\x8b\x83\x5c\xa5\x5e\x5c\xeb\xe9\x2c\x9c\x6a".to_vec()),
            p: Some(-4),
            encrypt_metadata: Some(false),
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"AESV3".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\xed\x0c\xba\x4d\xc7\x34\xf0\x01\x62\xac\x0d\x0d\xaf\xfc\xee\x1a",
        user: b"\x75\x73\x65\x72",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "2e753b9ca222d45aff6d53b49602d7ff51aeae4af4a5d925bd86525605784d17",
        title: (2, b"\xbc\x4a\xfb\xa4\x63\x70\xd8\xc2\xe2\x1d\x5b\x92\x25\x46\x6a\x8d\xd3\x96\x81\x75\xe7\x82\x11\xe4\xfa\xae\xe3\xb6\x42\xe9\xee\x25"),
    },
    Vector {
        name: "aes_256_empty_user",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 5,
            r: 6,
            length: Some(256),
            o: Some(b"\xe1\x6e\xae\x6d\x54\xbd\xf4\xfe\xde\x01\xa7\x8b\xc1\x0d\x21\xd2\xba\x19\x2b\x2a\x58\x56\xc7\xe4\x41\xf7\x10\xd3\xc7\xe8\xc1\xeb\x4b\x9c\xcb\x5c\x57\x45\xed\x5a\x8e\xd1\x18\x46\x53\x91\x6f\x86".to_vec()),
            u: Some(b"\xcd\xff\x70\x53\xdd\xc9\x8c\x59\x1d\xb9\x2f\xe9\x87\x5b\x27\x6d\x42\x4a\x0a\x43\xcd\xa0\xc2\x18\x30\x06\x70\xe7\x2d\x81\x4e\x08\x54\x68\xb4\xd8\xa5\x69\xd5\x27\x5c\xbe\x9d\x35\x06\xa8\x89\xff".to_vec()),
            oe: Some(b"\x47\x3a\x17\xbb\xb6\x28\xb1\x05\x8e\xc0\xcc\x4a\x61\x89\xd8\x17\x0f\xf5\x19\x52\x08\x34\x69\x3f\x1f\x60\x19\xcf\x1d\x8c\xa4\x9e".to_vec()),
            ue: Some(b"\xd5\x65\xb2\xba\x4b\xec\xbe\xee\x52\x6f\x20\xb4\x33\x7d\x7a\x18\x9a\x59\xa7\x43\x3f\x53\xc4\x3b\xa1\xb3\x46\x72\x68\x4c\x2a\xda".to_vec()),
            perms: Some(b"\x90\xa4\x8b\x81\x98\xc4\x22\xa1\xb8\x5d\x0b\x81\xd1\x37\xa5\xf6".to_vec()),
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"AESV3".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\x9d\x5c\x2c\xe6\x9e\xc4\x03\x09\xee\x26\x10\x60\x2b\xd1\x48\x7e",
        user: b"",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "40e35a3f141103e49039410f91037f99ad4f484dfdaeca236cfa16754dac35f0",
        title: (2, b"\x26\x67\x51\x0c\x09\x3a\xaa\xde\x88\xca\x7e\xef\x44\x23\x21\x40\xc7\x6b\x9c\x83\x91\x06\xdd\xf9\x24\x48\x5f\x55\x5b\x12\xa7\xd6"),
    },
    Vector {
        name: "aes_256_non_ascii",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 5,
            r: 6,
            length: Some(256),
            o: Some(b"\x3e\xbc\xca\x7b\x9f\x23\x93\xdb\xba\x17\x7a\x29\xb6\x37\xc3\xad\x80\xf7\xd0\xbe\x9a\x7f\x37\xe7\xb8\x9d\x1a\x32\x68\x9e\x33\x96\x82\x8c\xec\x03\x7f\xd9\x51\x6b\xc3\xef\x57\x62\xcd\x43\xde\x9e".to_vec()),
            u: Some(b"\xfc\x96\xee\xe6\x60\x3e\xb9\x43\xce\xf8\x59\x84\x0b\x45\x9c\x10\x1e\x87\x9e\x68\x20\x3e\xe7\xff\x9e\x7b\x0c\x4f\x11\x8c\xb4\xdf\xda\xba\xb3\x38\xbe\x6e\x89\x7a\x73\x52\xd2\xde\x08\x84\x30\x28".to_vec()),
            oe: Some(b"\x1a\x3f\xc6\x76\x0c\x34\x4c\xb1\xc8\xeb\x07\x6d\x87\x17\x6e\x48\xcc\x4c\xbc\xb1\x12\x52\xf9\x44\xd1\x45\x50\x3e\xbe\x64\x1e\xbf".to_vec()),
            ue: Some(b"\x0b\xcc\x8b\x5d\xa7\x67\xfc\xc6\x8e\x56\x12\xf8\x98\x14\x88\xd5\x4f\xe0\x9a\x55\xf8\xcf\x1b\xb4\x46\x20\x7d\xf7\x85\x79\xf4\xe3".to_vec()),
            perms: Some(b"\x77\x9b\x3f\xf1\xdc\x60\xff\x16\x1e\x81\x2d\x66\x98\x4e\x2a\x8d".to_vec()),
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"AESV3".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\x34\x6b\xae\xa2\x16\xf0\x20\x07\x5c\x92\x0b\x36\x29\xc5\x72\xa1",
        user: b"\x70\xc3\xa4\x73\x73\x77\xc3\xb6\x72\x64",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "a8a0cbc66856fb5fd05a9493dc64c461999591d8772adbbb5a1261dbc2c523bc",
        title: (2, b"\xa1\x81\x54\x02\x0a\x0c\x67\x69\xaf\xbd\xdb\x81\xc1\x2c\xaf\x2c\x7e\x50\xf7\xc1\x6b\xf8\x5f\xd3\x0a\x29\x50\xef\xa3\x4b\x55\x66"),
    },
    Vector {
        name: "aes_256_127",
        dictionary: Dictionary {
            filter: b"Standard".to_vec(),
            v: 5,
            r: 6,
            length: Some(256),
            o: Some(b"\x38\x85\xe5\x6a\x8e\x29\x46\x34\x53\x35\xb1\x99\xbc\x2e\x85\x47\xb1\xfa\x80\x77\x42\x0b\xb6\x29\xc7\x35\x05\xd7\x7d\xd2\xe4\xa8\xfd\x31\x3a\x53\xf2\x7d\x58\xbf\x78\x2d\x8f\x89\xb0\x53\x5a\x0b".to_vec()),
            u: Some(b"\xe1\xe6\x8d\x25\x23\xd1\x0f\x0f\x67\x1a\xde\x4c\x40\xda\xd0\x00\xac\xbb\x09\x0d\x43\xde\x33\x8d\x67\xcd\xf3\xc2\x5a\xb6\x53\x77\x50\x54\x0f\x63\x4f\xf2\x9b\x29\x5a\x5f\xd4\x3d\x86\x55\x6e\x7e".to_vec()),
            oe: Some(b"\x07\xb0\x59\xf5\xed\xa8\x4d\x2c\x15\x51\xcc\x45\xa4\x0e\xa4\xe2\x26\x60\xbf\x94\xfe\xc8\x03\x5c\xe8\xed\x4e\x30\xed\x54\xc4\xbc".to_vec()),
            ue: Some(b"\xa8\x5e\xa2\xfe\x80\xf4\x29\x2f\x4d\x4d\x7f\x1c\x5f\xba\xd1\x0c\xe4\x36\xe7\x5e\x8c\x55\xf2\xe6\x16\x4d\x4a\x39\xd7\x2a\x03\x70".to_vec()),
            perms: Some(b"\x2f\x81\x3a\x43\x7c\x63\x6e\x61\xea\xaf\x25\x4e\x21\x2f\x72\x34".to_vec()),
            p: Some(-4),
            encrypt_metadata: None,
            crypt_filters: vec![CryptFilter { name: b"StdCF".to_vec(), method: Some(b"AESV3".to_vec()) }],
            stm_f: Some(b"StdCF".to_vec()),
            str_f: Some(b"StdCF".to_vec()),
        },
        id: b"\x1b\x20\xbe\xa8\x1f\x29\x27\x9b\x27\x1d\xc3\x57\x2b\x18\xfd\x7a",
        user: b"\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70\x70",
        owner: b"\x6f\x77\x6e\x65\x72",
        key: "2821de0cd65ef22c3902d69e3e9a4e9c8f05d2e81e5043a68633e228bf584aa9",
        title: (2, b"\x3f\x6a\x1d\x2a\x65\x4d\xa0\x7f\xf5\xe3\x3b\xc3\x5b\xc5\xbc\x74\x42\x1c\xe2\x08\x6e\xdf\x05\x1e\x30\xb9\x91\x5c\x5e\x07\x3b\xd7"),
    },

    ]
}

#[test]
fn files_qpdf_encrypted_open_by_either_password_with_qpdfs_key() {
    for v in vectors() {
        let name = v.name;
        let user =
            Handler::open(&v.dictionary, v.id, &[v.user]).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(hex(user.key()), v.key, "{name}");
        assert!(!user.owner(), "{name}");
        assert_eq!(user.faults(), [], "{name}");
        let (n, written) = v.title;
        assert_eq!(
            user.decrypt_string((n, 0), written).unwrap(),
            b"A plain title",
            "{name}"
        );
        let owner = Handler::open(&v.dictionary, v.id, &[v.owner])
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(hex(owner.key()), v.key, "{name}");
        assert!(owner.owner(), "{name}");
        let wrong = Handler::open(&v.dictionary, v.id, &[b"wrong"]);
        assert_eq!(wrong.unwrap_err(), Refused::Password, "{name}");
        // The empty password, as no password is, only when it's the user's.
        let none = Handler::open(&v.dictionary, v.id, &[]);
        assert_eq!(none.is_ok(), v.user.is_empty(), "{name}");
    }
}

#[test]
fn the_first_password_that_opens_it_does() {
    let v = &vectors()[6];
    let handler = Handler::open(&v.dictionary, v.id, &[b"wrong", v.user, v.owner]).unwrap();
    assert!(!handler.owner());
}

#[test]
fn a_perms_that_doesnt_match_is_a_fault_and_the_file_opens() {
    let v = &vectors()[6];
    let mut dictionary = v.dictionary.clone();
    dictionary.p = dictionary.p.map(|p| p ^ 4);
    // /P is in /Perms, and from revision 5 not in the key.
    let handler = Handler::open(&dictionary, v.id, &[v.user]).unwrap();
    assert_eq!(handler.faults(), [Fault::Perms]);
    assert_eq!(hex(handler.key()), v.key);
}

#[test]
fn a_key_perms_doesnt_confirm_is_a_fault_unless_another_route_gives_one() {
    let v = &vectors()[6];
    let mut dictionary = v.dictionary.clone();
    dictionary.ue.as_mut().unwrap()[0] ^= 1;
    // The user's password, which unwraps /UE: a key /Perms doesn't
    // confirm, and no other to take, so it opens by it, saying so.
    let handler = Handler::open(&dictionary, v.id, &[v.user]).unwrap();
    assert_eq!(handler.faults(), [Fault::Unconfirmed]);
    assert_ne!(hex(handler.key()), v.key);
    // The owner's too: its key, which /Perms confirms, is taken.
    let handler = Handler::open(&dictionary, v.id, &[v.user, v.owner]).unwrap();
    assert_eq!(handler.faults(), []);
    assert_eq!(hex(handler.key()), v.key);
    assert!(handler.owner());
}

#[test]
fn revision_2_compares_all_of_u() {
    let v = &vectors()[0];
    assert_eq!(v.dictionary.r, 2);
    let mut dictionary = v.dictionary.clone();
    dictionary.u.as_mut().unwrap()[31] ^= 1;
    for password in [v.user, v.owner] {
        let opened = Handler::open(&dictionary, v.id, &[password]);
        assert_eq!(opened.unwrap_err(), Refused::Password);
    }
}

#[test]
fn what_the_writer_writes_opens_and_decrypts() {
    let kinds = [
        Kind::Rc4By40,
        Kind::Rc4By128,
        Kind::Aes128,
        Kind::Aes256R5,
        Kind::Aes256,
    ];
    for kind in kinds {
        for encrypt_metadata in [true, false] {
            let settings = Settings {
                kind,
                user: b"user",
                owner: b"owner",
                p: -3904,
                id: b"0123456789abcdef",
                encrypt_metadata,
            };
            let dictionary = writer::dictionary(&settings);
            let user = Handler::open(&dictionary, settings.id, &[b"user"])
                .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert_eq!(user.faults(), [], "{kind:?}");
            let owner = Handler::open(&dictionary, settings.id, &[b"owner"]).unwrap();
            assert!(owner.owner() && !user.owner(), "{kind:?}");
            assert_eq!(owner.key(), user.key(), "{kind:?}");
            assert_eq!(
                Handler::open(&dictionary, settings.id, &[b"wrong"]).unwrap_err(),
                Refused::Password
            );
            for data in [
                &b""[..],
                b"x",
                b"a string sixteen",
                b"a string of more than sixteen bytes",
            ] {
                let string = writer::encrypt_string(&user, (7, 0), data, [9; 16]).unwrap();
                assert_eq!(
                    owner.decrypt_string((7, 0), &string).unwrap(),
                    data,
                    "{kind:?}"
                );
                let stream = writer::encrypt_stream(&user, (8, 0), data, None, [3; 16]).unwrap();
                assert_eq!(
                    owner.decrypt_stream((8, 0), &stream, None).unwrap(),
                    data,
                    "{kind:?}"
                );
            }
            assert_eq!(
                user.encrypts_metadata(),
                encrypt_metadata || matches!(kind, Kind::Rc4By40 | Kind::Rc4By128),
                "{kind:?}"
            );
        }
    }
}

/// An AES-128 handler, and a string encrypted by it.
fn aes() -> (Handler, Vec<u8>) {
    let settings = Settings {
        kind: Kind::Aes128,
        user: b"",
        owner: b"owner",
        p: -4,
        id: b"an id",
        encrypt_metadata: true,
    };
    let handler = Handler::open(&writer::dictionary(&settings), settings.id, &[]).unwrap();
    let string = writer::encrypt_string(
        &handler,
        (5, 0),
        b"thirty-two bytes, two AES blocks",
        [1; 16],
    )
    .unwrap();
    (handler, string)
}

#[test]
fn aes_ciphertext_that_isnt_says_why() {
    let (handler, string) = aes();
    let decrypt = |data: &[u8]| handler.decrypt_string((5, 0), data);
    assert_eq!(
        decrypt(&string).unwrap(),
        b"thirty-two bytes, two AES blocks"
    );
    // Empty, it's empty; shorter than an IV, it stays; just an IV, it's
    // empty, said.
    assert_eq!(decrypt(b""), Ok(Vec::new()));
    let short = |len, data| Error {
        data,
        fault: Fault::Short { len },
    };
    assert_eq!(decrypt(&string[..10]), Err(short(10, None)));
    assert_eq!(decrypt(&string[..16]), Err(short(16, Some(Vec::new()))));
    // A part block past the whole ones is said and left off, and the
    // padding kept, as the last whole block needn't be the last.
    let padded = [&b"thirty-two bytes, two AES blocks"[..], &[16; 16]].concat();
    let error = decrypt(&[&string[..], b"xy"].concat()).unwrap_err();
    assert_eq!(
        (error.data, error.fault),
        (Some(padded.clone()), Fault::PartBlock { extra: 2 })
    );
    let error = decrypt(&string[..20]).unwrap_err();
    assert_eq!(
        (error.data, error.fault),
        (Some(Vec::new()), Fault::PartBlock { extra: 4 })
    );
    // A string's length is exact: white-space past its whole blocks is a
    // part block.
    let error = decrypt(&[&string[..], b"\r\n"].concat()).unwrap_err();
    assert_eq!(error.fault, Fault::PartBlock { extra: 2 });
    // A stream's white-space past its whole blocks, as a recovered /Length
    // leaves, is left off, however much; white-space alone is empty.
    let stream = |data: &[u8]| handler.decrypt_stream((5, 0), data, None);
    let plain = b"thirty-two bytes, two AES blocks".to_vec();
    assert_eq!(stream(&[&string[..], b"\r\n"].concat()), Ok(plain.clone()));
    assert_eq!(stream(&[&string[..], &[b' '; 40]].concat()), Ok(plain));
    assert_eq!(stream(b"\r\n"), Ok(Vec::new()));
    // But not past what the whole blocks need, nor said as less than it is.
    let error = stream(&[&string[..40], b"\n"].concat()).unwrap_err();
    assert_eq!(error.fault, Fault::PartBlock { extra: 8 });
    let error = stream(&[&string[..10], b"\r\n"].concat()).unwrap_err();
    assert_eq!(error, short(12, None));
    // A last block that isn't padded is kept whole.
    let error = decrypt(&string[..string.len() - 16]).unwrap_err();
    assert_eq!(error.fault, Fault::Padding);
    assert_eq!(error.data.unwrap(), b"thirty-two bytes, two AES blocks");
}

#[test]
fn crypt_filters_by_name_and_method() {
    let (handler, string) = aes();
    assert_eq!(handler.named(b"Identity"), Method::Identity);
    assert_eq!(handler.named(b"StdCF"), Method::Aes128);
    assert_eq!(handler.named(b"Other"), Method::Unknown(b"Other".to_vec()));
    // A stream by its own crypt filter: /Identity, as it's written.
    assert_eq!(
        handler
            .decrypt_stream((5, 0), &string, Some(&Method::Identity))
            .unwrap(),
        string
    );
    let error = handler
        .decrypt_stream((5, 0), &string, Some(&Method::Unknown(b"X".to_vec())))
        .unwrap_err();
    assert_eq!(
        error,
        Error {
            data: None,
            fault: Fault::Method {
                name: "X".to_string()
            }
        }
    );
    // AES-256 in a file of AES-128, by a stream's own crypt filter: its
    // key is AES-128's, and AES-256 is only of /V 5.
    let error = handler
        .decrypt_stream((5, 0), &string, Some(&Method::Aes256))
        .unwrap_err();
    assert_eq!(error.fault, Fault::Key { len: 16 });
    // AES in a file of RC4's 40 bits: neither its key nor an object's is
    // AES's. And below /V 4, a stream's own /Crypt is as streams are.
    let rc4 = Settings {
        kind: Kind::Rc4By40,
        user: b"",
        owner: b"o",
        p: -4,
        id: b"id",
        encrypt_metadata: true,
    };
    let forty = Handler::open(&writer::dictionary(&rc4), b"id", &[]).unwrap();
    let error = forty
        .decrypt_stream((5, 0), &string, Some(&Method::Aes256))
        .unwrap_err();
    assert_eq!(
        error,
        Error {
            data: None,
            fault: Fault::Key { len: 5 }
        }
    );
    let error = forty
        .decrypt_stream((5, 0), &string, Some(&Method::Aes128))
        .unwrap_err();
    assert_eq!(error.fault, Fault::Key { len: 5 });
    assert_eq!(
        writer::encrypt_stream(&forty, (5, 0), b"x", Some(&Method::Aes128), [0; 16]),
        Err(Fault::Key { len: 5 })
    );
    assert_eq!(forty.named(b"StdCF"), Method::Rc4);
    assert_eq!(forty.named(b"Identity"), Method::Rc4);
    // A crypt filter's method, from /CFM: none is identity, /None as
    // written, and one that isn't, unknown.
    let settings = Settings {
        kind: Kind::Aes128,
        user: b"",
        owner: b"o",
        p: -4,
        id: b"id",
        encrypt_metadata: true,
    };
    let mut dictionary = writer::dictionary(&settings);
    let filter = |name: &str, method: Option<&str>| CryptFilter {
        name: name.as_bytes().to_vec(),
        method: method.map(|m| m.as_bytes().to_vec()),
    };
    dictionary.crypt_filters.extend([
        filter("Bare", None),
        filter("Out", Some("None")),
        filter("Odd", Some("ROT13")),
    ]);
    let handler = Handler::open(&dictionary, b"id", &[]).unwrap();
    assert_eq!(handler.named(b"Bare"), Method::Identity);
    assert_eq!(handler.named(b"Out"), Method::None);
    assert_eq!(handler.named(b"Odd"), Method::Unknown(b"ROT13".to_vec()));
    // Without /StmF and /StrF, nothing is encrypted.
    dictionary.stm_f = None;
    dictionary.str_f = None;
    let handler = Handler::open(&dictionary, b"id", &[]).unwrap();
    assert_eq!(
        (handler.strings(), handler.streams()),
        (&Method::Identity, &Method::Identity)
    );
}

#[test]
fn a_dictionary_the_standard_handler_doesnt_take_is_refused() {
    let base = vectors()[1].dictionary.clone();
    let refused = |change: fn(&mut Dictionary)| {
        let mut dictionary = base.clone();
        change(&mut dictionary);
        match Handler::open(&dictionary, b"id", &[]) {
            Err(Refused::Unsupported(why)) => why,
            other => panic!("{other:?}"),
        }
    };
    assert!(
        refused(|d| d.filter = b"Adobe.PubSec".to_vec()).contains("/Adobe.PubSec, not /Standard")
    );
    assert!(refused(|d| d.v = 3).contains("/V 3 /R 3"));
    assert!(refused(|d| d.r = 7).contains("/V 2 /R 7"));
    assert_eq!(refused(|d| d.o = None), "its /O isn't a string");
    assert_eq!(
        refused(|d| (d.u, d.p) = (None, None)),
        "its /U isn't a string, /P isn't an integer"
    );
    assert!(refused(|d| d.u.as_mut().unwrap().extend([0; 8])).contains("over 32 bytes"));
    // Of /V 5, /OE, /UE and /Perms too, as qpdf must have them.
    let mut aes = vectors()[6].dictionary.clone();
    aes.perms = None;
    match Handler::open(&aes, b"id", &[]) {
        Err(Refused::Unsupported(why)) => assert_eq!(why, "its /Perms isn't a string"),
        other => panic!("{other:?}"),
    }
    // Short ones, empty ones too, are filled out, as qpdf fills them.
    for len in [16, 0] {
        let mut short = base.clone();
        short.u.as_mut().unwrap().truncate(len);
        assert!(Handler::open(&short, b"id", &[]).is_err_and(|e| e == Refused::Password));
    }
}
