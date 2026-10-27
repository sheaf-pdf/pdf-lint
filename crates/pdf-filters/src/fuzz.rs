//! What the fuzz target checks, which a unit test checks too: the promises
//! [`decode`](crate::decode) makes, on any input and any filters. It's
//! compiled only for tests and under cargo-fuzz's `--cfg fuzzing`.

use crate::{Error, Fault, Filter, Layout, Predictor, decode};

/// The limit the target decodes to, small, so that a bomb ends quickly.
pub const LIMIT: usize = 1 << 16;

/// The fuzz target's body: the filters and predictors `data`'s first bytes
/// choose, over the rest, decoded twice. Panics on a broken promise.
pub fn filters(data: &[u8]) {
    let (filters, input) = chosen(data);
    let decoded = decode(input, &filters, LIMIT);
    assert_eq!(decoded, decode(input, &filters, LIMIT), "the same twice");
    let Err(Error { data, faults }) = decoded else {
        return;
    };
    assert!(data.len() <= LIMIT, "within the limit");
    assert!(!faults.is_empty(), "an error has a fault");
    assert!(
        faults.windows(2).all(|w| w[0].0 <= w[1].0),
        "in the filters' order"
    );
    for (i, &(filter, fault)) in faults.iter().enumerate() {
        assert!(filter < filters.len(), "a filter that's listed");
        let kind = std::mem::discriminant(&fault);
        let again = faults[..i]
            .iter()
            .any(|&(f, earlier)| f == filter && std::mem::discriminant(&earlier) == kind);
        assert!(!again, "one of each kind a filter: {faults:?}");
        // Places in the first filter's input, which is the input.
        if filter == 0 {
            match fault {
                Fault::Corrupt { at } => assert!(at <= input.len()),
                Fault::Byte { at, .. }
                | Fault::BrokenEnd { at }
                | Fault::InGroup { at }
                | Fault::Overflow { at }
                | Fault::LoneByte { at }
                | Fault::Code { at }
                | Fault::TableFull { at } => assert!(at < input.len(), "{fault:?}"),
                _ => {}
            }
        }
    }
}

/// The filters `data`'s first bytes choose, and the input after them: one
/// to three, each a byte, LZW's and Flate's with a predictor from four
/// more, which may be one that isn't.
fn chosen(data: &[u8]) -> (Vec<Filter>, &[u8]) {
    let mut bytes = data.iter().copied();
    let mut next = || bytes.next().unwrap_or(0);
    let mut filters = Vec::new();
    for _ in 0..1 + next() % 3 {
        let kind = next();
        let mut predictor = || match next() % 3 {
            0 => Predictor::None,
            p => {
                let layout = Layout {
                    colors: u32::from(next() % 5),
                    bits_per_component: [1, 2, 4, 8, 16, 3][usize::from(next() % 6)],
                    columns: u32::from(next()),
                };
                match p {
                    1 => Predictor::Tiff(layout),
                    _ => Predictor::Png(layout),
                }
            }
        };
        filters.push(match kind % 5 {
            0 => Filter::AsciiHex,
            1 => Filter::Ascii85,
            2 => Filter::Lzw {
                early_change: kind & 0x80 == 0,
                predictor: predictor(),
            },
            3 => Filter::Flate {
                predictor: predictor(),
            },
            _ => Filter::RunLength,
        });
    }
    let used = data.len() - bytes.len();
    (filters, &data[used..])
}

#[cfg(test)]
mod tests {
    use super::filters;

    /// Bytes of no pattern, from `seed`.
    fn noise(seed: &mut u64, len: usize) -> Vec<u8> {
        (0..len)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                *seed as u8
            })
            .collect()
    }

    #[test]
    fn random_inputs_keep_every_promise() {
        let mut seed = 0x2545_f491_4f6c_dd1d;
        for i in 0..20_000 {
            let len = (i % 300) + 1;
            filters(&noise(&mut seed, len));
        }
    }

    #[test]
    fn damaged_compressed_inputs_keep_every_promise() {
        let mut seed = 0x9e37_79b9_7f4a_7c15;
        for i in 0..2_000 {
            let plain = noise(&mut seed, (i % 500) + 1);
            let mut input = vec![3, 3, 0];
            input.extend(miniz_oxide::deflate::compress_to_vec_zlib(&plain, 6));
            for _ in 0..1 + i % 4 {
                let at = noise(&mut seed, 1)[0] as usize % input.len();
                input[at] ^= 1 << (i % 8);
            }
            filters(&input);
        }
    }
}
