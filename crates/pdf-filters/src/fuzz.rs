//! What the fuzz target checks, which unit tests check too: the promises
//! [`decode`](crate::decode), [`decode_into`](crate::decode_into),
//! [`decode_prefix_into`](crate::decode_prefix_into) and
//! [`encode`](crate::encode) make, on any input, any filters and one
//! encoder or two, on each backend compiled in. It's compiled only for
//! tests and under cargo-fuzz's `--cfg fuzzing`.

use crate::{
    Backend, Encoder, Error, Fault, Faults, Filter, Layout, Level, Prediction, Predictor, Prefix,
    Row, Unencodable, decode, decode_into, decode_prefix_into, encode,
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

/// `data` decoded by `filter` as data that may run on, by `backend`, into
/// a buffer that holds something already: what it read, what it added and
/// its faults.
pub fn prefix(data: &[u8], filter: Filter, backend: Backend) -> (Prefix, Vec<u8>, Vec<Fault>) {
    let mut out = b"held".to_vec();
    let (prefix, decoded) = decode_prefix_into(data, filter, LIMIT, backend, &mut out);
    assert_eq!(&out[..4], b"held", "what the buffer held, kept");
    let faults = match decoded {
        Ok(()) => Vec::new(),
        Err(Faults(faults)) => {
            assert!(!faults.is_empty(), "an error has a fault");
            assert!(faults.iter().all(|&(i, _)| i == 0), "one filter's");
            faults.into_iter().map(|(_, fault)| fault).collect()
        }
    };
    (prefix, out.split_off(4), faults)
}

/// The promises [`decode_prefix_into`] makes of `input` decoded by
/// `filter` on `backend`: what [`decode_into`] gives by a list of that
/// filter alone; never past the input; the data up to what it read
/// decoding alike, with the same faults but a missing end's; where its data
/// ended, bytes after it changing nothing.
fn prefixes(input: &[u8], filter: Filter, backend: Backend) {
    let (read, added, faults) = prefix(input, filter, backend);
    let mut out = Vec::new();
    let into = decode_into(input, &[filter], LIMIT, backend, &mut out);
    let listed: Vec<Fault> = match into {
        Ok(()) => Vec::new(),
        Err(Faults(faults)) => faults.into_iter().map(|(_, fault)| fault).collect(),
    };
    assert_eq!(
        (&added, &faults),
        (&out, &listed),
        "as decode_into decodes it"
    );
    assert!(read.read <= input.len(), "never past the input: {read:?}");
    if read.ended {
        let missing = |f: &Fault| {
            matches!(
                f,
                Fault::NoEnd | Fault::Truncated | Fault::BrokenEnd { .. } | Fault::Header
            )
        };
        assert!(
            !faults.iter().any(missing),
            "ended, its end there: {faults:?}"
        );
    }
    // What it read, alone: the same, but where its data didn't end, a
    // fault for the end it then lacks. Where it read nothing (a header
    // zlib counts none of), nothing came of it, and nothing decodes to
    // nothing.
    if read.read == 0 {
        assert!(added.is_empty(), "nothing read, nothing added");
        return;
    }
    let (again, cut, cut_faults) = prefix(&input[..read.read], filter, backend);
    assert_eq!(cut, added, "what it read decodes alike: {read:?}");
    let lacking = |f: &&Fault| !matches!(f, Fault::NoEnd | Fault::Truncated);
    match read.ended {
        true => assert_eq!((again, &cut_faults), (read, &faults), "ended, the same"),
        false => {
            let (a, b): (Vec<_>, Vec<_>) = (
                cut_faults.iter().filter(lacking).collect(),
                faults.iter().filter(lacking).collect(),
            );
            assert_eq!(a, b, "what it read, the same faults but the end's");
        }
    }
    // Bytes after an end change nothing: an inline image's `EI`, and the
    // input itself again.
    if read.ended {
        for tail in [&b" EI Q"[..], input] {
            let longer = [&input[..read.read], tail].concat();
            assert_eq!(
                prefix(&longer, filter, backend),
                (read, added.clone(), faults.clone()),
                "ended, bytes after it change nothing"
            );
        }
    }
}

/// The filters and predictors `data`'s first bytes choose, over the rest,
/// decoded twice on each backend, and into a buffer that holds something
/// already; and by the first of them as data that may run on. Panics on a
/// broken promise.
pub fn filters(data: &[u8]) {
    let (filters, input) = chosen(data);
    let mut clean: Option<Vec<u8>> = None;
    for backend in backends() {
        prefixes(input, filters[0], backend);
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
///
/// Where the first byte's top bit is set, a second encoder is listed
/// before that one, as a stream's `/Filter` would list it, so applied
/// last: Flate with no predictor, at the level the next byte gives. The
/// two then encode the input as the second alone encodes what the first
/// gave, which decoded by their filters in the list's order, under a limit
/// of the input's length or the first's output's, whichever is more, gives
/// the input; or, where the first can't encode it, the buffer as it was,
/// for the same reason.
pub fn encoders(data: &[u8]) {
    let mut bytes = data.iter().copied();
    let mut next = || bytes.next().unwrap_or(0);
    let first = next();
    let level = Level::new(first % 10).expect("0 to 9");
    let outer = (first & 0x80 != 0).then(|| Encoder::Flate {
        level: Level::new(next() % 10).expect("0 to 9"),
        predictor: Prediction::None,
    });
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
            Some(len) if !input.len().is_multiple_of(len) => Err(Unencodable::PartialRow {
                missing: len - input.len() % len,
            }),
            Some(_) => Ok(()),
        };
        assert_eq!(encoded, want, "{encoder:?}, {} bytes", input.len());
        match encoded {
            Err(_) => assert_eq!(out, b"held", "the buffer as it was"),
            Ok(()) => {
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
        if let Some(outer) = outer {
            listed_after(input, outer, encoder, backend, &out[4..], encoded);
        }
    }
}

/// The promises [`encode`] makes of `input` encoded by `inner` listed
/// after `outer`, Flate with no predictor, on `backend`, given what
/// `inner` alone gave it, `alone`, and whether it could, `encoded`: the
/// buffer as it was where `inner` can't encode it, for the same reason;
/// otherwise `alone` encoded by `outer` alone, which decoded on each
/// backend by their filters, in the list's order, gives the input.
fn listed_after(
    input: &[u8],
    outer: Encoder,
    inner: Encoder,
    backend: Backend,
    alone: &[u8],
    encoded: Result<(), Unencodable>,
) {
    let mut out = b"held".to_vec();
    let both = encode(input, &[outer, inner], backend, &mut out);
    assert_eq!(both, encoded, "{outer:?}, {inner:?}: as {inner:?} alone");
    if encoded.is_err() {
        assert_eq!(out, b"held", "the buffer as it was, with two");
        return;
    }
    let mut outer_alone = b"held".to_vec();
    encode(alone, &[outer], backend, &mut outer_alone)
        .expect("Flate without a predictor encodes anything");
    assert_eq!(
        out, outer_alone,
        "{outer:?} applied last, to what {inner:?} gave, on {backend:?}"
    );
    let filters = [outer.filter(), inner.filter()];
    let limit = input.len().max(alone.len());
    for decoder in backends() {
        let decoded = decode(&out[4..], &filters, limit, decoder);
        assert_eq!(
            decoded.as_deref(),
            Ok(input),
            "{outer:?}, {inner:?} on {backend:?}, by {decoder:?}"
        );
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
            // Data that compresses, now and then: runs of a byte, eight
            // long, after the bytes that choose the encoders, so that what
            // the first gives can be shorter than the input or longer.
            if i % 3 == 0 {
                for at in (8..input.len()).filter(|at| at % 8 > 0) {
                    input[at] = input[at - 1];
                }
            }
            encoders(&input);
        }
    }
}
