//! What the fuzz target checks, which unit tests check too: the promises
//! [`decode`](crate::decode), [`decode_into`](crate::decode_into) and
//! [`encode`](crate::encode) make, on any input, any filters and any
//! encoder, on each backend compiled in. It's compiled only for tests and
//! under cargo-fuzz's `--cfg fuzzing`.

use crate::{
    Backend, Encoder, Error, Fault, Faults, Filter, Layout, Level, Prediction, Predictor, Row,
    Unencodable, decode, decode_into, encode,
};

/// The limit the target decodes to, small, so that a bomb ends quickly.
pub const LIMIT: usize = 1 << 16;

/// The backends this build has, each its feature's.
pub fn backends() -> Vec<Backend> {
    vec![
        #[cfg(feature = "miniz_oxide")]
        Backend::MinizOxide,
        #[cfg(feature = "zlib-rs")]
        Backend::ZlibRs,
    ]
}

/// The filters and predictors `data`'s first bytes choose, over the rest,
/// decoded twice on each backend, and into a buffer that holds something
/// already. Panics on a broken promise.
pub fn filters(data: &[u8]) {
    let (filters, input) = chosen(data);
    let mut clean: Option<Vec<u8>> = None;
    for backend in backends() {
        let decoded = decode(input, &filters, LIMIT, backend);
        assert_eq!(
            decoded,
            decode(input, &filters, LIMIT, backend),
            "the same twice"
        );
        let mut out = b"held".to_vec();
        let into = decode_into(input, &filters, LIMIT, backend, &mut out);
        assert_eq!(&out[..4], b"held", "what the buffer held, kept");
        match (&decoded, into) {
            (Ok(data), Ok(())) => assert_eq!(&out[4..], data, "into a buffer, the same"),
            (Err(error), Err(Faults(faults))) => {
                assert_eq!((&out[4..], &faults), (&error.data[..], &error.faults))
            }
            (decoded, into) => panic!("into a buffer, otherwise: {decoded:?}, {into:?}"),
        }
        // On clean data, every backend decodes the same.
        if let Ok(data) = &decoded {
            match &clean {
                Some(other) => assert_eq!(data, other, "the same on {backend:?}"),
                None => clean = Some(data.clone()),
            }
        }
        let Err(Error { data, faults }) = decoded else {
            continue;
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
                    Fault::Byte { at, .. }
                    | Fault::Corrupt { at }
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
}

/// The Flate encoder `data`'s first bytes choose (its level, and a PNG
/// predictor's layout and row, which may be one that isn't, or none),
/// over the rest, cut to whole rows or not: encoded on each backend into
/// a buffer that holds something already, twice, and decoded on each by
/// the encoder's filter, under a limit of the input's length, gives the
/// input; or, where it can't be encoded, the buffer as it was, for the
/// reason the encoder gives.
pub fn encoders(data: &[u8]) {
    let mut bytes = data.iter().copied();
    let mut next = || bytes.next().unwrap_or(0);
    let level = Level::new(next() % 10).expect("0 to 9");
    let row = [Row::None, Row::Sub, Row::Up, Row::Average, Row::Paeth][usize::from(next() % 5)];
    let (predictor, whole) = match next() {
        n if n % 3 == 0 => (Prediction::None, false),
        n => {
            let layout = Layout {
                colors: u32::from(next() % 5),
                bits_per_component: [1, 2, 4, 8, 16, 3][usize::from(next() % 6)],
                columns: u32::from(next()),
            };
            (Prediction::Png(layout, row), n % 3 == 1)
        }
    };
    let used = data.len() - bytes.len();
    let mut input = &data[used..];
    // A layout's rows: none for one that isn't.
    let rows = match predictor {
        Prediction::Png(layout, _) => {
            let Layout {
                colors,
                bits_per_component: bits,
                columns,
            } = layout;
            let fits = colors > 0 && columns > 0 && matches!(bits, 1 | 2 | 4 | 8 | 16);
            fits.then(|| (colors * bits * columns).div_ceil(8) as usize)
        }
        _ => Some(1),
    };
    if let (true, Some(len)) = (whole, rows) {
        input = &input[..input.len() - input.len() % len];
    }
    let encoder = Encoder::Flate { level, predictor };
    for backend in backends() {
        let mut out = b"held".to_vec();
        let encoded = encode(input, &[encoder], backend, &mut out);
        let mut again = b"held".to_vec();
        assert_eq!(
            (encode(input, &[encoder], backend, &mut again), &again),
            (encoded, &out),
            "the same twice"
        );
        let want = match rows {
            None => Err(Unencodable::Parameters),
            Some(len) if input.len() % len > 0 => Err(Unencodable::PartialRow {
                missing: len - input.len() % len,
            }),
            Some(_) => Ok(()),
        };
        assert_eq!(encoded, want, "{encoder:?}, {} bytes", input.len());
        if encoded.is_err() {
            assert_eq!(out, b"held", "the buffer as it was");
            continue;
        }
        assert_eq!(&out[..4], b"held", "what the buffer held, kept");
        for decoder in backends() {
            let decoded = decode(&out[4..], &[encoder.filter()], input.len(), decoder);
            assert_eq!(
                decoded.as_deref(),
                Ok(input),
                "{encoder:?} on {backend:?}, by {decoder:?}"
            );
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

/// `data` deflated at `level` by `backend`, as a test compresses it.
pub fn zlib(data: &[u8], level: Level, backend: Backend) -> Vec<u8> {
    let flate = Encoder::Flate {
        level,
        predictor: Prediction::None,
    };
    let mut out = Vec::new();
    encode(data, &[flate], backend, &mut out).expect("Flate without a predictor encodes anything");
    out
}

#[cfg(test)]
mod tests {
    use super::{backends, encoders, filters, zlib};
    use crate::Level;

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
            // Compressed by each backend in turn.
            let backend = backends()[i % backends().len()];
            let mut input = vec![3, 3, 0];
            input.extend(zlib(&plain, Level::DEFAULT, backend));
            for _ in 0..1 + i % 4 {
                let at = noise(&mut seed, 1)[0] as usize % input.len();
                input[at] ^= 1 << (i % 8);
            }
            filters(&input);
        }
    }

    #[test]
    fn random_inputs_encode_and_decode_back() {
        let mut seed = 0x6a09_e667_f3bc_c908;
        for i in 0..3_000 {
            let mut input = noise(&mut seed, (i % 400) + 1);
            // Data that compresses, now and then: runs of a byte.
            if i % 3 == 0 {
                for at in (8..input.len()).step_by(7) {
                    input[at] = input[at - 8];
                }
            }
            encoders(&input);
        }
    }
}
