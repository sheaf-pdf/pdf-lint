//! LZW's and Flate's predictors (ISO 32000-2, 7.4.4.4), undone.

use crate::{Fault, Layout, Out, Predictor};

/// `data` with `predictor` undone, row by row, in place, so that it takes
/// no memory but `data`'s; a last row that's short is read with zeros for
/// what it lacks, as qpdf reads it.
pub(crate) fn undo(mut data: Vec<u8>, predictor: Predictor, out: &mut Out) {
    let (layout, png) = match predictor {
        Predictor::None => {
            out.data = data;
            return;
        }
        Predictor::Tiff(layout) => (layout, false),
        Predictor::Png(layout) => (layout, true),
    };
    let Some(rows) = Rows::of(layout) else {
        out.fault(Fault::Parameters);
        return;
    };
    // PNG's rows begin with a byte saying how they're predicted.
    let stride = rows.len.saturating_add(usize::from(png));
    let count = data.len().div_ceil(stride);
    // As many rows as the limit allows.
    let undone = count.min(out.limit / rows.len);
    if undone < count {
        out.fault(Fault::Limit);
    } else if count * stride > data.len() {
        out.fault(Fault::PartialRow {
            missing: count * stride - data.len(),
        });
        data.resize(count * stride, 0);
    }
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
                if !undo_png(tag, row, above, rows.pixel) {
                    out.fault(Fault::RowTag { row: i, tag });
                }
            }
            false => undo_tiff(row, layout),
        }
    }
    data.truncate(undone * rows.len);
    out.data = data;
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
            let samples = colors * layout.columns as usize;
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
    use crate::{Fault, Filter, Layout, Predictor, decode};

    fn layout(colors: u32, bits_per_component: u32, columns: u32) -> Layout {
        Layout {
            colors,
            bits_per_component,
            columns,
        }
    }

    /// `data` compressed, and decoded through Flate and `predictor`.
    fn undone(data: &[u8], predictor: Predictor) -> Result<Vec<u8>, crate::Error> {
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(data, 6);
        decode(&compressed, &[Filter::Flate { predictor }], 1 << 20)
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
    fn the_limit_keeps_the_rows_it_allows() {
        // The limit is on what Flate gives the predictor too, tags and
        // all: two rows of three bytes.
        let png = Predictor::Png(layout(1, 8, 2));
        let compressed =
            miniz_oxide::deflate::compress_to_vec_zlib(&[1, 1, 1, 2, 1, 1, 0, 9, 9], 6);
        let error = decode(&compressed, &[Filter::Flate { predictor: png }], 6).unwrap_err();
        assert_eq!(error.data, [1, 2, 2, 3]);
        assert_eq!(error.faults, [(0, Fault::Limit)]);
    }

    #[test]
    fn rows_longer_than_the_limit_decode_nothing() {
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&[0, 1, 2], 6);
        for (colors, columns) in [(1000, 1_000_000), (u32::MAX, u32::MAX)] {
            let predictor = Predictor::Png(layout(colors, 16, columns));
            let error = decode(&compressed, &[Filter::Flate { predictor }], 1 << 20).unwrap_err();
            assert_eq!(
                (error.data, error.faults),
                (Vec::new(), [(0, Fault::Limit)].to_vec())
            );
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
}
