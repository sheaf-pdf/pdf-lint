//! LZW's and Flate's predictors (ISO 32000-2, 7.4.4.4), undone; and PNG's
//! done, for an encoder.

use crate::{Fault, Filter, Layout, Out, Predictor, Row, Unencodable};

/// The limit on what `filter` gives its predictor, for a `limit` on what
/// the predictor gives: a PNG predictor's tags besides, one a row.
pub(crate) fn before(limit: usize, filter: &Filter) -> usize {
    let (Filter::Lzw { predictor, .. } | Filter::Flate { predictor }) = filter else {
        return limit;
    };
    match predictor {
        Predictor::Png(layout) => match Rows::of(*layout) {
            Some(rows) => limit.saturating_add(limit / rows.len + 1),
            None => limit,
        },
        _ => limit,
    }
}

/// The filter's output in `out`, past what was there before it, with
/// `predictor` undone, row by row, in place, so that it takes no memory
/// but its own; a last row that's short is read with zeros for what it
/// lacks, as qpdf reads it, unless the data was `cut` off by a fault, when
/// it isn't read.
pub(crate) fn undo(predictor: Predictor, cut: bool, out: &mut Out) {
    let (layout, png) = match predictor {
        Predictor::None => return,
        Predictor::Tiff(layout) => (layout, false),
        Predictor::Png(layout) => (layout, true),
    };
    let start = out.start;
    let Some(rows) = Rows::of(layout) else {
        out.data.truncate(start);
        out.fault(Fault::Parameters);
        return;
    };
    // PNG's rows begin with a byte saying how they're predicted.
    let stride = rows.len.saturating_add(usize::from(png));
    let len = out.data.len() - start;
    let count = match cut {
        true => len / stride,
        false => len.div_ceil(stride),
    };
    // As many rows as the limit allows.
    let undone = count.min(out.limit / rows.len);
    if undone < count {
        out.fault(Fault::Limit);
    } else if count * stride > len {
        out.fault(Fault::PartialRow {
            missing: count * stride - len,
        });
        out.data.resize(start + count * stride, 0);
    }
    let data = &mut out.data[start..];
    let mut tagged = None;
    for i in 0..undone {
        // Each row moved to just past the rows before it: that's over its
        // own tag, so the tag's read first, and never over a row after it.
        let (from, to) = (i * stride, i * rows.len);
        let tag = data[from];
        data.copy_within(from + usize::from(png)..from + stride, to);
        let (before, row) = data.split_at_mut(to);
        let row = &mut row[..rows.len];
        match png {
            true => {
                let above = (i > 0).then(|| &before[to - rows.len..]);
                if !undo_png(tag, row, above, rows.pixel) && tagged.is_none() {
                    tagged = Some(Fault::RowTag { row: i, tag });
                }
            }
            false => undo_tiff(row, layout),
        }
    }
    out.data.truncate(start + undone * rows.len);
    if let Some(fault) = tagged {
        out.fault(fault);
    }
}

/// `data`, whole rows of `layout`, each predicted by PNG's `row` filter
/// and its tag put before it, as [`undo`] undoes.
pub(crate) fn png(data: &[u8], layout: Layout, row: Row) -> Result<Vec<u8>, Unencodable> {
    let rows = Rows::of(layout).ok_or(Unencodable::Parameters)?;
    let over = data.len() % rows.len;
    if over > 0 {
        return Err(Unencodable::PartialRow {
            missing: rows.len - over,
        });
    }
    let mut out = Vec::with_capacity(data.len() + data.len() / rows.len);
    // The row above, none for the first, as if all zeros.
    let mut above: Option<&[u8]> = None;
    for bytes in data.chunks_exact(rows.len) {
        let up = |i: usize| above.map_or(0, |above| above[i]);
        let left = |i: usize| i.checked_sub(rows.pixel).map_or(0, |j| bytes[j]);
        out.push(row.tag());
        match row {
            Row::None => out.extend_from_slice(bytes),
            Row::Sub => out.extend((0..bytes.len()).map(|i| bytes[i].wrapping_sub(left(i)))),
            Row::Up => match above {
                Some(above) => {
                    let less = bytes.iter().zip(above);
                    out.extend(less.map(|(byte, up)| byte.wrapping_sub(*up)));
                }
                None => out.extend_from_slice(bytes),
            },
            Row::Average => {
                out.extend((0..bytes.len()).map(|i| {
                    let mean = (u16::from(left(i)) + u16::from(up(i))) / 2;
                    bytes[i].wrapping_sub(mean as u8)
                }));
            }
            Row::Paeth => {
                out.extend((0..bytes.len()).map(|i| {
                    let up_left = i.checked_sub(rows.pixel).map_or(0, up);
                    bytes[i].wrapping_sub(paeth(left(i), up(i), up_left))
                }));
            }
        }
        above = Some(bytes);
    }
    Ok(out)
}

/// The bytes of a predictor's rows, and of a pixel, at least one.
struct Rows {
    len: usize,
    pixel: usize,
}

impl Rows {
    /// `layout`'s rows, unless it isn't a layout; a length too long to
    /// count is counted as the longest there is.
    fn of(layout: Layout) -> Option<Rows> {
        let Layout {
            colors,
            bits_per_component: bits,
            columns,
        } = layout;
        if colors == 0 || columns == 0 || !matches!(bits, 1 | 2 | 4 | 8 | 16) {
            return None;
        }
        let pixel = u64::from(colors) * u64::from(bits);
        let bytes = |bits: u64| usize::try_from(bits.div_ceil(8)).unwrap_or(usize::MAX);
        Some(Rows {
            len: bytes(pixel.saturating_mul(u64::from(columns))),
            pixel: bytes(pixel),
        })
    }
}

/// `row` with PNG's filter `tag` undone, given the row `above` it (none
/// for the first, as if all zeros) and the bytes of a pixel; false for a
/// tag that isn't one, the row left as it is.
fn undo_png(tag: u8, row: &mut [u8], above: Option<&[u8]>, pixel: usize) -> bool {
    let above = |i: usize| above.map_or(0, |above| above[i]);
    for i in 0..row.len() {
        let left = i.checked_sub(pixel).map_or(0, |j| row[j]);
        let up = above(i);
        let predicted = match tag {
            0 => return true,
            1 => left,
            2 => up,
            3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
            4 => paeth(left, up, i.checked_sub(pixel).map_or(0, above)),
            _ => return false,
        };
        row[i] = row[i].wrapping_add(predicted);
    }
    true
}

/// PNG's Paeth predictor: whichever of left, up and up-left is nearest
/// left + up − up-left, in that order on a tie.
fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let (a, b, c) = (i16::from(left), i16::from(up), i16::from(up_left));
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    match () {
        _ if pa <= pb && pa <= pc => left,
        _ if pb <= pc => up,
        _ => up_left,
    }
}

/// `row` with TIFF's predictor undone: each sample plus the one `colors`
/// samples before it, modulo its bits; bits past the last sample are left
/// 0, as qpdf leaves them.
fn undo_tiff(row: &mut [u8], layout: Layout) {
    let colors = layout.colors as usize;
    match layout.bits_per_component {
        8 => {
            for i in colors..row.len() {
                row[i] = row[i].wrapping_add(row[i - colors]);
            }
        }
        16 => {
            for i in colors..row.len() / 2 {
                let sample = |j: usize| u16::from_be_bytes([row[2 * j], row[2 * j + 1]]);
                let sum = sample(i).wrapping_add(sample(i - colors));
                row[2 * i..2 * i + 2].copy_from_slice(&sum.to_be_bytes());
            }
        }
        bits => {
            let bits = bits as usize;
            let mask = (1u8 << bits) - 1;
            let samples = colors.saturating_mul(layout.columns as usize);
            // Sample `i`'s byte, and its shift from the byte's low end.
            let at = |i: usize| (i * bits / 8, 8 - bits - i * bits % 8);
            for i in colors..samples {
                let ((byte, shift), (before, before_shift)) = (at(i), at(i - colors));
                let sum = (row[byte] >> shift).wrapping_add(row[before] >> before_shift) & mask;
                row[byte] = row[byte] & !(mask << shift) | sum << shift;
            }
            let used = samples * bits;
            if used % 8 != 0 {
                row[used / 8] &= !(0xFF >> (used % 8));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::png;
    use crate::fuzz::{backends, zlib};
    use crate::{
        Encoder, Fault, Filter, Layout, Level, Prediction, Predictor, Row, Unencodable, decode,
    };

    fn layout(colors: u32, bits_per_component: u32, columns: u32) -> Layout {
        Layout {
            colors,
            bits_per_component,
            columns,
        }
    }

    /// `data` compressed, and decoded through Flate and `predictor`, the
    /// same on each backend.
    fn undone(data: &[u8], predictor: Predictor) -> Result<Vec<u8>, crate::Error> {
        let mut each = backends().into_iter().map(|backend| {
            let compressed = zlib(data, Level::DEFAULT, backend);
            decode(
                &compressed,
                &[Filter::Flate { predictor }],
                1 << 20,
                backend,
            )
        });
        let first = each.next().unwrap();
        each.for_each(|other| assert_eq!(other, first, "another backend"));
        first
    }

    #[test]
    fn png_rows_each_undo_their_own_filter() {
        let rows = [
            0, 9, 9, 9, // none
            1, 10, 5, 3, // sub: 10, 15, 18
            3, 1, 2, 3, // average of left and up: 6, 12, 18
            4, 1, 1, 1, // Paeth: 7, 13, 19
            2, 1, 1, 1, // up: 8, 14, 20
        ];
        let png = Predictor::Png(layout(1, 8, 3));
        assert_eq!(
            undone(&rows, png).unwrap(),
            [9, 9, 9, 10, 15, 18, 6, 12, 18, 7, 13, 19, 8, 14, 20]
        );
    }

    #[test]
    fn paeth_takes_up_left_when_its_nearest() {
        let rows = [0, 6, 2, 4, 4, 1];
        let png = Predictor::Png(layout(1, 8, 2));
        // The second sample: left 10, up 2, up-left 6, so 6.
        assert_eq!(undone(&rows, png).unwrap(), [6, 2, 10, 7]);
    }

    #[test]
    fn png_predicts_from_the_pixel_before_not_the_byte() {
        let png = Predictor::Png(layout(3, 8, 2));
        assert_eq!(
            undone(&[1, 1, 2, 3, 1, 1, 1], png).unwrap(),
            [1, 2, 3, 2, 3, 4]
        );
        // A 16-bit sample's pixel is two bytes; bits under 8 make one.
        let png = Predictor::Png(layout(1, 16, 2));
        assert_eq!(undone(&[1, 1, 0xff, 0, 1], png).unwrap(), [1, 0xff, 1, 0]);
        let png = Predictor::Png(layout(2, 2, 4));
        assert_eq!(undone(&[1, 0x11, 0x01], png).unwrap(), [0x11, 0x12]);
    }

    #[test]
    fn a_cross_reference_streams_rows_undo_up() {
        // /W [1 2 1], /Columns 4, /Predictor 12: each row its difference
        // from the row above.
        let rows = [2, 1, 0, 0x10, 0, 2, 0, 0, 0x25, 0, 2, 1, 0, 0x20, 0];
        let png = Predictor::Png(layout(1, 8, 4));
        assert_eq!(
            undone(&rows, png).unwrap(),
            [1, 0, 0x10, 0, 1, 0, 0x35, 0, 2, 0, 0x55, 0]
        );
    }

    #[test]
    fn a_tag_that_isnt_one_leaves_its_row_as_it_is() {
        let png = Predictor::Png(layout(1, 8, 2));
        let error = undone(&[1, 1, 1, 7, 5, 5, 2, 1, 1], png).unwrap_err();
        assert_eq!(error.data, [1, 2, 5, 5, 6, 6]);
        assert_eq!(error.faults, [(0, Fault::RowTag { row: 1, tag: 7 })]);
    }

    #[test]
    fn a_short_last_row_is_read_with_zeros() {
        let png = Predictor::Png(layout(1, 8, 3));
        let error = undone(&[0, 1, 2, 3, 2, 1], png).unwrap_err();
        assert_eq!(error.data, [1, 2, 3, 2, 2, 3]);
        assert_eq!(error.faults, [(0, Fault::PartialRow { missing: 2 })]);
        let tiff = Predictor::Tiff(layout(1, 8, 3));
        let error = undone(&[1, 1, 1, 5], tiff).unwrap_err();
        assert_eq!(error.data, [1, 2, 3, 5, 5, 5]);
        assert_eq!(error.faults, [(0, Fault::PartialRow { missing: 2 })]);
    }

    #[test]
    fn a_layout_that_isnt_one_decodes_nothing() {
        for (colors, bits, columns) in [(0, 8, 1), (1, 3, 1), (1, 8, 0), (1, 32, 1)] {
            let png = Predictor::Png(layout(colors, bits, columns));
            let error = undone(&[0, 1], png).unwrap_err();
            assert_eq!(
                (error.data, error.faults),
                (Vec::new(), [(0, Fault::Parameters)].to_vec())
            );
        }
    }

    #[test]
    fn the_limit_is_on_what_the_predictor_gives_tags_aside() {
        let png = Predictor::Png(layout(1, 8, 2));
        let flate = [Filter::Flate { predictor: png }];
        for backend in backends() {
            let compressed = zlib(&[1, 1, 1, 2, 1, 1, 0, 9, 9], Level::DEFAULT, backend);
            let decoded = decode(&compressed, &flate, 6, backend);
            assert_eq!(decoded.unwrap(), [1, 2, 2, 3, 9, 9]);
            // Two rows of two bytes, no more.
            let error = decode(&compressed, &flate, 5, backend).unwrap_err();
            assert_eq!(error.data, [1, 2, 2, 3]);
            assert_eq!(error.faults, [(0, Fault::Limit)]);
        }
    }

    #[test]
    fn a_row_a_fault_cut_off_isnt_read() {
        // Seven bytes in a stored block, then a block of the reserved
        // type: the third row, cut off, isn't read as a short one.
        let mut data = vec![0x78, 0x01, 0x00, 7, 0, !7, 0xff, 1, 1, 1, 2, 1, 1, 0];
        data.push(0x07);
        let png = Predictor::Png(layout(1, 8, 2));
        for backend in backends() {
            let flate = [Filter::Flate { predictor: png }];
            let error = decode(&data, &flate, 100, backend).unwrap_err();
            assert_eq!(error.data, [1, 2, 2, 3], "{backend:?}");
            assert!(
                matches!(error.faults[..], [(0, Fault::Corrupt { .. })]),
                "{backend:?}: {error:?}"
            );
        }
    }

    #[test]
    fn rows_longer_than_the_limit_decode_nothing() {
        for backend in backends() {
            let compressed = zlib(&[0, 1, 2], Level::DEFAULT, backend);
            for (colors, columns) in [(1000, 1_000_000), (u32::MAX, u32::MAX)] {
                let predictor = Predictor::Png(layout(colors, 16, columns));
                let flate = [Filter::Flate { predictor }];
                let error = decode(&compressed, &flate, 1 << 20, backend).unwrap_err();
                assert_eq!(
                    (error.data, error.faults),
                    (Vec::new(), [(0, Fault::Limit)].to_vec())
                );
            }
        }
    }

    #[test]
    fn tiff_adds_the_sample_a_pixel_before() {
        let tiff = Predictor::Tiff(layout(3, 8, 2));
        assert_eq!(
            undone(&[1, 2, 3, 1, 1, 1, 9, 9, 9, 0, 0, 1], tiff).unwrap(),
            [1, 2, 3, 2, 3, 4, 9, 9, 9, 9, 9, 10]
        );
        let tiff = Predictor::Tiff(layout(1, 16, 2));
        assert_eq!(undone(&[0, 0xff, 0, 2], tiff).unwrap(), [0, 0xff, 1, 1]);
        // 4 bits: 1, 15, 2 is 1, 0, 2, modulo 16, the pad bits cleared.
        let tiff = Predictor::Tiff(layout(1, 4, 3));
        assert_eq!(undone(&[0x1f, 0x2f], tiff).unwrap(), [0x10, 0x20]);
        let tiff = Predictor::Tiff(layout(1, 1, 8));
        assert_eq!(undone(&[0b1010_0000], tiff).unwrap(), [0b1100_0000]);
        let tiff = Predictor::Tiff(layout(2, 2, 3));
        assert_eq!(
            undone(&[0b0110_0101, 0b1100_0000], tiff).unwrap(),
            [0b0110_1011, 0b0111_0000]
        );
    }

    /// Each of PNG's filters predicts each row as `png_rows_each_undo_their_own_filter`'s
    /// rows were predicted, each by its own: the inverse of the undo. Each
    /// row begins with the filter's tag, and the `/Predictor` written for it
    /// is 10 plus the tag (1 for no prediction).
    #[test]
    fn each_row_filter_predicts_as_its_undo_undoes() {
        let rows = [9, 9, 9, 10, 15, 18, 6, 12, 18, 7, 13, 19, 8, 14, 20];
        let cases = [
            (Row::None, 0, [0, 9, 9, 9]),
            (Row::Sub, 1, [1, 10, 5, 3]),
            (Row::Average, 2, [3, 1, 2, 3]),
            (Row::Paeth, 3, [4, 1, 1, 1]),
            (Row::Up, 4, [2, 1, 1, 1]),
        ];
        for (row, i, predicted) in cases {
            let all = png(&rows, layout(1, 8, 3), row).unwrap();
            assert_eq!(all.len(), rows.len() + 5, "{row:?}");
            assert_eq!(all[i * 4..i * 4 + 4], predicted, "{row:?}");
            assert!(all.chunks(4).all(|r| r[0] == row.tag()), "{row:?}");
            let prediction = Prediction::Png(layout(1, 8, 3), row);
            assert_eq!(prediction.predictor(), 10 + predicted[0], "{row:?}");
        }
        assert_eq!(Prediction::Png(layout(1, 8, 3), Row::Up).predictor(), 12);
        assert_eq!(Prediction::None.predictor(), 1);
    }

    /// `rows`, each `columns` bytes, under PNG's Up, as a cross-reference
    /// stream's writer puts them: each row tagged 2 and less the row above
    /// it, byte by byte, the first less zeros.
    fn up(rows: &[u8], columns: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(rows.len() + rows.len() / columns);
        let zeros = vec![0; columns];
        let mut above = zeros.as_slice();
        for row in rows.chunks(columns) {
            out.push(2);
            out.extend(row.iter().zip(above).map(|(b, a)| b.wrapping_sub(*a)));
            above = row;
        }
        out
    }

    /// Up's rows are a cross-reference stream writer's: the rows of
    /// `a_cross_reference_streams_rows_undo_up`, and a section of 202
    /// entries, the first free and the rest in use, at offsets past 2¹⁶
    /// (`/W [1 3 2]`).
    #[test]
    fn up_predicts_a_cross_reference_streams_rows_as_its_writer_does() {
        let rows = [1, 0, 0x10, 0, 1, 0, 0x35, 0, 2, 0, 0x55, 0];
        let predicted = png(&rows, layout(1, 8, 4), Row::Up).unwrap();
        assert_eq!(predicted, up(&rows, 4));
        assert_eq!(
            predicted,
            [2, 1, 0, 0x10, 0, 2, 0, 0, 0x25, 0, 2, 1, 0, 0x20, 0]
        );
        let mut section = vec![0, 0, 0, 0, 0xff, 0xff];
        for n in 1..202u32 {
            let offset = 60_000 + n * 977;
            section.push(1);
            section.extend_from_slice(&offset.to_be_bytes()[1..]);
            section.extend_from_slice(&[0, 0]);
        }
        let predicted = png(&section, layout(1, 8, 6), Row::Up).unwrap();
        assert_eq!(predicted, up(&section, 6));
        for backend in backends() {
            let mut out = Vec::new();
            let encoder = Encoder::Flate {
                level: Level::DEFAULT,
                predictor: Prediction::Png(layout(1, 8, 6), Row::Up),
            };
            crate::encode(&section, &[encoder], backend, &mut out).unwrap();
            assert_eq!(out, zlib(&up(&section, 6), Level::DEFAULT, backend));
            let decoded = decode(&out, &[encoder.filter()], 1 << 20, backend);
            assert_eq!(decoded.unwrap(), section, "{backend:?}");
        }
    }

    /// Rows of every layout, under each filter, at each level and on each
    /// backend, decode to themselves.
    #[test]
    fn each_row_filter_round_trips_on_every_layout() {
        let rows = [Row::None, Row::Sub, Row::Up, Row::Average, Row::Paeth];
        let layouts = [
            layout(1, 8, 3),
            layout(3, 8, 2),
            layout(1, 16, 2),
            layout(2, 2, 4),
            layout(1, 1, 8),
            layout(4, 16, 5),
        ];
        for layout in layouts {
            let len = (layout.colors * layout.bits_per_component * layout.columns).div_ceil(8);
            let data: Vec<u8> = (0..len as usize * 40)
                .map(|i| ((i * 37) ^ (i >> 3)) as u8)
                .collect();
            for row in rows {
                for level in [
                    Level::NONE,
                    Level::new(1).unwrap(),
                    Level::DEFAULT,
                    Level::BEST,
                ] {
                    for backend in backends() {
                        let encoder = Encoder::Flate {
                            level,
                            predictor: Prediction::Png(layout, row),
                        };
                        let mut out = Vec::new();
                        crate::encode(&data, &[encoder], backend, &mut out).unwrap();
                        assert_eq!(
                            encoder.filter(),
                            Filter::Flate {
                                predictor: Predictor::Png(layout)
                            }
                        );
                        for inflater in backends() {
                            let decoded = decode(&out, &[encoder.filter()], 1 << 20, inflater);
                            assert_eq!(decoded.unwrap(), data, "{layout:?}, {row:?}, {level:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rows_that_arent_whole_or_a_layout_that_isnt_one_dont_predict() {
        let short = png(&[1, 2, 3, 4], layout(1, 8, 3), Row::Up);
        assert_eq!(short, Err(Unencodable::PartialRow { missing: 2 }));
        for (colors, bits, columns) in [(0, 8, 1), (1, 3, 1), (1, 8, 0), (1, 32, 1)] {
            let predicted = png(&[0, 1], layout(colors, bits, columns), Row::Sub);
            assert_eq!(predicted, Err(Unencodable::Parameters));
        }
        // Rows too long to count are as long as can be: data is never
        // whole rows of them, but no data is, and its rows aren't made.
        let huge = layout(u32::MAX, 16, u32::MAX);
        assert_eq!(png(&[], huge, Row::Up), Ok(Vec::new()));
        assert!(matches!(
            png(&[1], huge, Row::Up),
            Err(Unencodable::PartialRow { .. })
        ));
    }
}
