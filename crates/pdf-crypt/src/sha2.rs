//! SHA-256, SHA-384 and SHA-512 (FIPS 180-4), which revisions 5 and 6 of
//! the standard security handler derive keys with: RustCrypto's `sha2`.

use ::sha2::{Digest, Sha256, Sha384, Sha512};

/// `data`'s SHA-256 digest.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// `data`'s SHA-512 digest.
pub(crate) fn sha512(data: &[u8]) -> [u8; 64] {
    Sha512::digest(data).into()
}

/// `data`'s SHA-384 digest.
pub(crate) fn sha384(data: &[u8]) -> [u8; 48] {
    Sha384::digest(data).into()
}

#[cfg(test)]
mod tests {
    use super::{sha256, sha384, sha512};

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// FIPS 180-4's examples (the NIST "Examples with Intermediate
    /// Values"), and the empty message.
    #[test]
    fn the_standards_examples() {
        let two = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        let four = "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";
        let sha256s = [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                two,
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ];
        for (message, digest) in sha256s {
            assert_eq!(hex(&sha256(message.as_bytes())), digest, "{message:?}");
        }
        let sha512s = [
            (
                "abc",
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
            (
                four,
                "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909",
            ),
        ];
        for (message, digest) in sha512s {
            assert_eq!(hex(&sha512(message.as_bytes())), digest, "{message:?}");
        }
        let sha384s = [
            (
                "abc",
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            ),
            (
                four,
                "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039",
            ),
        ];
        for (message, digest) in sha384s {
            assert_eq!(hex(&sha384(message.as_bytes())), digest, "{message:?}");
        }
    }

    #[test]
    fn a_million_as() {
        let message = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha256(&message)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
}
