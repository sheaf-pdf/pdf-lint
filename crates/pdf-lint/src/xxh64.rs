//! XXH64, the 64-bit xxHash (<https://github.com/Cyan4973/xxHash>, its
//! `doc/xxhash_spec.md`), which names a model by its tables' bytes.

const P1: u64 = 0x9E37_79B1_85EB_CA87;
const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const P3: u64 = 0x1656_67B1_9E37_79F9;
const P4: u64 = 0x85EB_CA77_C2B2_AE63;
const P5: u64 = 0x27D4_EB2F_1656_67C5;

/// XXH64 of `data` with `seed`.
pub(crate) fn xxh64(data: &[u8], seed: u64) -> u64 {
    let lane = |b: &[u8]| u64::from_le_bytes(b[..8].try_into().expect("8 bytes"));
    let mut rest = data;
    let mut h = if data.len() >= 32 {
        let mut v = [
            seed.wrapping_add(P1).wrapping_add(P2),
            seed.wrapping_add(P2),
            seed,
            seed.wrapping_sub(P1),
        ];
        while rest.len() >= 32 {
            for (i, v) in v.iter_mut().enumerate() {
                *v = round(*v, lane(&rest[i * 8..]));
            }
            rest = &rest[32..];
        }
        let h = (v[0].rotate_left(1))
            .wrapping_add(v[1].rotate_left(7))
            .wrapping_add(v[2].rotate_left(12))
            .wrapping_add(v[3].rotate_left(18));
        v.iter().fold(h, |h, &v| merge(h, v))
    } else {
        seed.wrapping_add(P5)
    };
    h = h.wrapping_add(data.len() as u64);
    while rest.len() >= 8 {
        h ^= round(0, lane(rest));
        h = h.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        rest = &rest[8..];
    }
    if rest.len() >= 4 {
        let word = u32::from_le_bytes(rest[..4].try_into().expect("4 bytes"));
        h ^= u64::from(word).wrapping_mul(P1);
        h = h.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
        rest = &rest[4..];
    }
    for &b in rest {
        h ^= u64::from(b).wrapping_mul(P5);
        h = h.rotate_left(11).wrapping_mul(P1);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(P2);
    h ^= h >> 29;
    h = h.wrapping_mul(P3);
    h ^ (h >> 32)
}

fn round(acc: u64, lane: u64) -> u64 {
    acc.wrapping_add(lane.wrapping_mul(P2))
        .rotate_left(31)
        .wrapping_mul(P1)
}

fn merge(acc: u64, v: u64) -> u64 {
    (acc ^ round(0, v)).wrapping_mul(P1).wrapping_add(P4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As xxHash 0.8.2's `xxhash.h` has them, through each path: under 32
    /// bytes, stripes of 32, and tails of 8, 4 and 1.
    #[test]
    fn as_the_reference_has_it() {
        let big: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let cases: [(&[u8], u64, u64); 12] = [
            (b"", 0, 0xef46db3751d8e999),
            (b"a", 0, 0xd24ec4f1a98c6e5b),
            (b"abc", 0, 0x44bc2cf5ad770999),
            (b"abc", 1, 0xbea9ca8199328908),
            (&big[..31], 0, 0xa2aa5f33cc4a6119),
            (&big[..32], 0, 0x23c3c17ef790fd97),
            (&big[..33], 0, 0x50a7cfc7ba588784),
            (&big[..63], 0, 0x5e3e54b431c7493c),
            (&big[..64], 0, 0x0eb64b3ef6eeb01f),
            (&big[..65], 0, 0xa383b724b2bd12f1),
            (&big[..100], 0, 0xa61f8d4c170fe531),
            (&big[..1000], 0, 0x5f235fa033f1a3fb),
        ];
        for (data, seed, want) in cases {
            assert_eq!(xxh64(data, seed), want, "{} bytes, seed {seed}", data.len());
        }
    }
}
