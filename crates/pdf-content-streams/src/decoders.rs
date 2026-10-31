//! With the `image-ends` feature: where an inline image's DCT and CCITTFax
//! data ends, as PDFium's decoders find it (its `DecodeInlineStream`, which
//! decodes every row of the image and takes where its decoder's source
//! then stands), the data decoded by zune-jpeg and hayro-ccitt, which say
//! how far they read. What's decoded isn't kept.

use std::cell::Cell;

use hayro_ccitt::{DecodeError, DecodeSettings, DecoderContext, EncodingMode};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::{ZByteIoError, ZByteReaderTrait, ZSeekFrom};
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// What decoding an image's data found of its end, and what it cost.
pub(crate) struct Decoded {
    /// How it ends.
    pub(crate) end: Found,
    /// What decoding made, in bytes, whatever comes of it.
    pub(crate) made: usize,
}

/// How an image's data ends, as its decoder found it.
pub(crate) enum Found {
    /// `end` bytes from the data's start, the decoder having read `read`,
    /// which is read in vain if no `EI` comes after.
    At { end: usize, read: usize },
    /// Nowhere: PDFium refuses the data, which is read on from its start
    /// to the next `EI`, with no stream; what was read, in vain.
    Refused(usize),
    /// The decoder couldn't read it, where PDFium's reads on: what was
    /// read, in vain, the end the stand-in's.
    Undecoded(usize),
}

/// The largest width and height of a JPEG, as its header writes them in 16
/// bits; zune-jpeg's own caps are lower by default.
const JPEG_MAX: usize = 65535;

/// Where `data`, an inline image's DCT data and the content after it, ends
/// as PDFium's JPEG decoder finds it, `soi` where its first start of image
/// (FF D8) is, or 0 where there's none, `width` its `/W` as PDFium reads
/// it; what decoding holds, under `limit`, decoded into `out`, which is
/// cleared after. None past `limit`.
///
/// As PDFium (its `LibjpegScanlineDecoder` on libjpeg-turbo, PDFium's Rust
/// JPEG decoder not built): the JPEG from its first start of image on, or
/// from the data's start where there's none; the content's last two bytes
/// read as an end of image (FF D9) where they aren't one. Refused where
/// its header doesn't read, or where it's narrower than `width`, or its
/// rows of all its components, a byte each, are none or past 32 bits.
/// Otherwise its end is where the decoder's source stands once every row
/// is decoded, counted from the start of image, not the data's start, as
/// PDFium counts it, so where bytes come before the start of image the end
/// is short by as many. A JPEG of several scans (progressive, or its
/// first scan without all its components) libjpeg-turbo reads whole
/// before its first row, to its end of image, and zune-jpeg reads it so
/// too. A JPEG of one scan it reads as its rows ask, its reader taking the
/// coded data's bits as they're needed, and the marker after them as the
/// bits run out, a restart (FF D0 to FF D7) read past: there it ends,
/// where zune-jpeg reads on to the end of image, through any segment
/// between (a comment, an application's). Where its last bits are read
/// before it runs into that marker, as its reader holds as many as 64
/// ahead, libjpeg-turbo stops short of the marker, which isn't followed,
/// as zune-jpeg says nothing of where its last row's bits end. And where
/// no marker follows the coded data, as where a JPEG is cut short, the
/// end runs to the content's end where libjpeg-turbo stops at the bits its
/// last row needed. Where zune-jpeg can't decode what libjpeg-turbo reads
/// past (damaged data, which libjpeg-turbo reads on through), the end is
/// left to the stand-in; and so where it stops short of the end of image
/// without an error, as it does at a progressive scan with no coded data,
/// where libjpeg-turbo reads on through every scan to that end. A header
/// zune-jpeg doesn't read, as one of 12-bit samples, which libjpeg-turbo
/// does, is read as refused. What the decoder read, counted from the
/// data's start, runs to just past the end of image, where the data ends
/// as ISO 32000-2 reads it.
///
/// What decoding holds is its output, a byte a sample of each component,
/// and, decoding several scans, the coefficients of all of them, two bytes
/// a sample of each component, dimensions padded to 32; one scan, a row of
/// them, 32 samples high.
pub(crate) fn dct(
    data: &[u8],
    soi: usize,
    width: u32,
    limit: usize,
    out: &mut Vec<u8>,
) -> Option<Decoded> {
    let span = &data[soi..];
    if span.len() < 2 {
        return Some(Decoded {
            end: Found::Refused(0),
            made: 0,
        });
    }
    let at = Cell::new(0);
    let options = DecoderOptions::default()
        .set_max_width(JPEG_MAX)
        .set_max_height(JPEG_MAX);
    let mut decoder = JpegDecoder::new_with_options(Jpeg::new(span, &at), options);
    let refused = || {
        Some(Decoded {
            end: Found::Refused(soi + at.get()),
            made: 0,
        })
    };
    let (Ok(()), Some(info)) = (decoder.decode_headers(), decoder.info()) else {
        return refused();
    };
    // Its first scan's coded data starts where its header ends.
    let scan = at.get();
    let (w, h, components) = (
        u32::from(info.width),
        u32::from(info.height),
        u32::from(info.components),
    );
    // Gray samples decoded as they are, not as RGB.
    if components == 1 {
        decoder.set_options(options.jpeg_set_out_colorspace(ColorSpace::Luma));
    }
    let Some(size) = decoder.output_buffer_size() else {
        return refused();
    };
    // The rows PDFium decodes, a byte for each sample.
    let rows = w.checked_mul(components).and_then(|row| row.checked_mul(h));
    if w < width || matches!(rows, None | Some(0)) {
        return refused();
    }
    let several = info.sof.is_progressive()
        || scan_components(span, scan).is_none_or(|n| u32::from(n) < components);
    let padded = |n: u32| (n as usize).div_ceil(32) * 32;
    let coefficients = match several {
        true => padded(h),
        false => 32,
    };
    let made = (padded(w).saturating_mul(coefficients))
        .saturating_mul(components as usize * 2)
        .saturating_add(size);
    if made > limit {
        return None;
    }
    out.clear();
    out.resize(size, 0);
    let decoded = decoder.decode_into(out);
    out.clear();
    let read = soi + at.get();
    // A whole decode stands just past the end of image it read last; one
    // of one scan, decoded, may stand past the marker after it instead.
    let whole = at.get() >= 2 && [at.get() - 2, at.get() - 1].map(|i| as_read(span, i)) == EOI;
    let end = match several {
        true => whole.then_some(at.get()),
        false => after_scan(span, scan).or(whole.then_some(at.get())),
    };
    let end = match (decoded, end) {
        (Ok(()), Some(end)) if i32::try_from(end).is_ok() => Found::At { end, read },
        (Ok(()), Some(_)) => Found::Refused(read),
        _ => Found::Undecoded(read),
    };
    Some(Decoded { end, made })
}

/// How many components the first scan of `span`, a JPEG's bytes as
/// zune-jpeg read them, holds, its header ending at `at`, where zune-jpeg
/// stands once it's read the headers: the header found back from there,
/// its marker (FF DA), its length, 6 and two bytes for each component,
/// and the count, 1 to 4. None where no count, or more than one, makes
/// such a header.
fn scan_components(span: &[u8], at: usize) -> Option<u8> {
    let mut found = None;
    for n in 1..=4u8 {
        let length = 6 + 2 * usize::from(n);
        let Some(start) = at.checked_sub(length + 1) else {
            continue;
        };
        let header = [0, 1, 2, 3].map(|i| as_read(span, start + i));
        if header == [0xDA, 0, length as u8, n] {
            if found.is_some() {
                return None;
            }
            found = Some(n);
        }
    }
    found
}

/// Where libjpeg-turbo's reader stands once it runs into the marker after
/// a scan's coded data, from `from` in `span`, a JPEG's bytes as zune-jpeg
/// read them: just past the first marker that isn't a restart (FF D0 to FF
/// D7), the FF bytes that fill before it read with it, a stuffed FF (FF
/// 00) being data. None where there's none.
fn after_scan(span: &[u8], from: usize) -> Option<usize> {
    let (n, patched) = (span.len(), patched(span));
    let mut i = from;
    while i < n {
        // The next FF: in the bytes as written, or the end of image read in
        // place of the last two.
        i = match memchr::memchr(0xFF, span.get(i..patched).unwrap_or_default()) {
            Some(k) => i + k,
            None => i.max(patched),
        };
        let mut j = i + 1;
        while j < n && as_read(span, j) == 0xFF {
            j += 1;
        }
        match (j < n).then(|| as_read(span, j))? {
            0x00 | 0xD0..=0xD7 => i = j + 1,
            _ => return Some(j + 1),
        }
    }
    None
}

/// A JPEG's end of image.
const EOI: [u8; 2] = [0xFF, 0xD9];

/// Where the bytes of `span`, a JPEG's from its start of image to the
/// content's end, read as an end of image start: its last two, as PDFium
/// patches them where they aren't one, or its end where they're one
/// already.
fn patched(span: &[u8]) -> usize {
    let n = span.len();
    if span[n - 2..] == EOI { n } else { n - 2 }
}

/// The byte at `i` of `span` as the decoder reads it ([`patched`]).
fn as_read(span: &[u8], i: usize) -> u8 {
    match i.checked_sub(patched(span)) {
        Some(k) => EOI[k],
        None => span[i],
    }
}

/// A JPEG's bytes as zune-jpeg reads them, from its start of image to the
/// content's end, the last two read as an end of image, as PDFium patches
/// them where they aren't one; where it stands kept in a cell the caller
/// reads once the decoder's done.
struct Jpeg<'a, 'c> {
    span: &'a [u8],
    /// Where the bytes read as an end of image start ([`patched`]).
    patched: usize,
    at: &'c Cell<usize>,
}

impl<'a, 'c> Jpeg<'a, 'c> {
    fn new(span: &'a [u8], at: &'c Cell<usize>) -> Jpeg<'a, 'c> {
        let patched = patched(span);
        Jpeg { span, patched, at }
    }

    /// The bytes from where it stands into `buf`, as many as there are: how
    /// many.
    fn copy(&self, buf: &mut [u8]) -> usize {
        let from = self.at.get();
        let n = buf.len().min(self.span.len() - from);
        buf[..n].copy_from_slice(&self.span[from..from + n]);
        for (i, b) in buf[..n].iter_mut().enumerate() {
            if from + i >= self.patched {
                *b = EOI[from + i - self.patched];
            }
        }
        n
    }
}

impl ZByteReaderTrait for Jpeg<'_, '_> {
    fn read_byte_no_error(&mut self) -> u8 {
        let mut b = [0];
        let n = self.copy(&mut b);
        self.at.set(self.at.get() + n);
        b[0]
    }

    fn read_exact_bytes(&mut self, buf: &mut [u8]) -> Result<(), ZByteIoError> {
        self.peek_exact_bytes(buf)?;
        self.at.set(self.at.get() + buf.len());
        Ok(())
    }

    fn read_bytes(&mut self, buf: &mut [u8]) -> Result<usize, ZByteIoError> {
        let n = self.copy(buf);
        self.at.set(self.at.get() + n);
        Ok(n)
    }

    fn peek_bytes(&mut self, buf: &mut [u8]) -> Result<usize, ZByteIoError> {
        Ok(self.copy(buf))
    }

    fn peek_exact_bytes(&mut self, buf: &mut [u8]) -> Result<(), ZByteIoError> {
        let left = self.span.len() - self.at.get();
        if buf.len() > left {
            return Err(ZByteIoError::NotEnoughBytes(buf.len(), left));
        }
        self.copy(buf);
        Ok(())
    }

    fn z_seek(&mut self, from: ZSeekFrom) -> Result<u64, ZByteIoError> {
        let (base, by) = match from {
            ZSeekFrom::Start(n) => (0, i64::try_from(n).unwrap_or(i64::MAX)),
            ZSeekFrom::End(n) => (self.span.len(), n),
            ZSeekFrom::Current(n) => (self.at.get(), n),
        };
        let n = usize::try_from(by.unsigned_abs()).unwrap_or(usize::MAX);
        let to = match by {
            0.. => base.saturating_add(n),
            _ => base.saturating_sub(n),
        };
        self.at.set(to.min(self.span.len()));
        Ok(self.at.get() as u64)
    }

    fn is_eof(&mut self) -> Result<bool, ZByteIoError> {
        Ok(self.at.get() >= self.span.len())
    }

    fn z_position(&mut self) -> Result<u64, ZByteIoError> {
        Ok(self.at.get() as u64)
    }

    fn read_remaining(&mut self, sink: &mut Vec<u8>) -> Result<usize, ZByteIoError> {
        let mut rest = vec![0; self.span.len() - self.at.get()];
        let n = self.read_bytes(&mut rest)?;
        sink.extend_from_slice(&rest);
        Ok(n)
    }
}

/// What an image's `/DecodeParms` say to PDFium's fax decoder, as PDFium
/// reads them (its `CreateFaxDecoder`), each as it reads an integer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fax {
    /// `/K`: below 0, Group 4 coding; 0, Group 3's one-dimensional; above,
    /// Group 3's mixed, each row tagged.
    pub(crate) k: i32,
    /// `/EndOfLine`, where it isn't 0.
    pub(crate) end_of_line: bool,
    /// `/EncodedByteAlign`, where it isn't 0.
    pub(crate) byte_align: bool,
    /// `/Columns`, 1728 where it isn't written, the image's width where 0.
    pub(crate) columns: i32,
    /// `/Rows`, the image's height where it's 0 or past 65,535.
    pub(crate) rows: i32,
    /// `/EndOfBlock`, as ISO 32000-2 reads it, true unless it's `false`
    /// (7.4.6), which PDFium doesn't read: where the data ends as ISO
    /// reads it ([`ccitt_iso`]).
    pub(crate) end_of_block: bool,
}

impl Fax {
    /// What's read where there are none.
    pub(crate) const DEFAULT: Fax = Fax {
        k: 0,
        end_of_line: false,
        byte_align: false,
        columns: 1728,
        rows: 0,
        end_of_block: true,
    };
}

/// The largest width and height PDFium's fax decoder takes.
const FAX_MAX: i32 = 65535;

/// How much of what follows fax data its decoder is first given
/// ([`ccitt`]), in bytes, before the first byte from there whose last bit
/// is 1.
const FAX_WINDOW: usize = 4096;

/// Where `data`, an inline image's CCITTFax data and the content after it,
/// ends as PDFium's fax decoder finds it, `width` and `height` its `/W` and
/// `/H` as PDFium reads them, `fax` its parameters.
///
/// As PDFium: its rows `/Columns` wide, or `width` where that's 0, and
/// `/Rows` of them, or `height` where that's 0; refused where either is
/// none or past 65,535. Its end is where the decoder stands once every row
/// is decoded, rounded up to a byte. hayro-ccitt decodes it as PDFium's
/// decoder does where the data is as T.4 and T.6 code it, but for the
/// end-of-line codes after its last row of Group 3 coding, which it reads
/// on through, where PDFium stops before them, or after the first where
/// `/EndOfLine` is set: the end is then past them all, and the image's
/// stream the same. It makes no output here. Where it runs into the
/// content's end, so does the data; where it can't decode the data, where
/// PDFium's decoder reads on as it can, the end is left to the stand-in.
///
/// Where it can't decode the data, it says nothing of how far it read, so
/// it's given a window of `data` ([`window`]), from [`FAX_WINDOW`] bytes,
/// doubled while it runs into the window's end: a window it decodes
/// within, or can't decode, it reads as it reads all of `data`, and what
/// it can't decode counts as read to the window's end, about twice what
/// it read at most.
pub(crate) fn ccitt(data: &[u8], width: i32, height: i32, fax: Fax) -> Found {
    let Some(settings) = settings(width, height, fax) else {
        return Found::Refused(0);
    };
    let mut first = FAX_WINDOW;
    loop {
        let given = window(data, first);
        let all = given == data.len();
        match decode(&data[..given], settings) {
            // Its rows decoded before the window's end, or in all of it.
            Ok(end) if end < given || all => {
                return match i32::try_from(end) {
                    Ok(_) => Found::At { end, read: end },
                    Err(_) => Found::Refused(end),
                };
            }
            Err(DecodeError::UnexpectedEof) if all => {
                return Found::At {
                    end: data.len(),
                    read: data.len(),
                };
            }
            // It ran into the window's end: twice as much.
            Ok(_) | Err(DecodeError::UnexpectedEof) => first = given.saturating_mul(2),
            Err(_) => return Found::Undecoded(given),
        }
    }
}

/// How much of `data` is given to hayro-ccitt in a window of `first` bytes
/// or more: to the first byte from there whose last bit is 1, or all of
/// it. Before such a byte, every look ahead the decoder takes for zeros,
/// an end-of-line code's and the fill bits before it, finds a 1 or a code
/// within the window, and every bit it reads is there, or it runs into the
/// window's end, so it reads the window as it reads all of `data` but where
/// it runs into the window's end or stands there.
fn window(data: &[u8], first: usize) -> usize {
    if first >= data.len() {
        return data.len();
    }
    let from = first.max(1) - 1;
    (data[from..].iter())
        .position(|b| b & 1 == 1)
        .map_or(data.len(), |k| from + k + 1)
}

/// Where `data`, an inline image's CCITTFax data and the content after it,
/// ends as ISO 32000-2 reads it, `width`, `height` and `fax` as [`ccitt`]
/// takes them: at an end-of-block pattern, Group 4's EOFB or Group 3's
/// RTC, which ISO's filter expects to end the data unless `/EndOfBlock` is
/// false, whatever `/Rows` says (7.4.6), and which PDFium's decoder,
/// counting rows, doesn't read; past it where it comes after the rows
/// PDFium counts, and otherwise where PDFium ends it. None where
/// `/EndOfBlock` is false, where that's PDFium's end, or where the data
/// doesn't decode so.
pub(crate) fn ccitt_iso(data: &[u8], width: i32, height: i32, fax: Fax) -> Option<usize> {
    if !fax.end_of_block {
        return None;
    }
    let settings = DecodeSettings {
        end_of_block: true,
        ..settings(width, height, fax)?
    };
    decode(data, settings).ok()
}

/// What `fax` says to hayro-ccitt, as PDFium's decoder reads it: its rows
/// `/Columns` wide, or `width` where that's 0, and `/Rows` of them, or
/// `height` where that's 0; none where either is none or past 65,535.
fn settings(width: i32, height: i32, fax: Fax) -> Option<DecodeSettings> {
    let columns = if fax.columns != 0 { fax.columns } else { width };
    let rows = if fax.rows != 0 { fax.rows } else { height };
    if !(1..=FAX_MAX).contains(&columns) || !(1..=FAX_MAX).contains(&rows) {
        return None;
    }
    Some(DecodeSettings {
        columns: columns as u32,
        rows: rows as u32,
        // PDFium reads no end of block: it decodes the rows it counts.
        end_of_block: false,
        end_of_line: fax.end_of_line,
        rows_are_byte_aligned: fax.byte_align,
        encoding: match fax.k {
            k if k < 0 => EncodingMode::Group4,
            0 => EncodingMode::Group3_1D,
            k => EncodingMode::Group3_2D { k: k as u32 },
        },
        invert_black: false,
    })
}

/// Where hayro-ccitt, decoding `data` by `settings`, stands once it's
/// done, rounded up to a byte and never past the data's end, as PDFium
/// holds its decoder's end.
fn decode(data: &[u8], settings: DecodeSettings) -> Result<usize, DecodeError> {
    let mut context = DecoderContext::new(settings);
    hayro_ccitt::decode(data, &mut Pixels, &mut context).map(|end| end.min(data.len()))
}

/// A fax image's pixels, decoded and let go.
struct Pixels;

impl hayro_ccitt::Decoder for Pixels {
    fn push_pixel(&mut self, _white: bool) {}

    fn push_pixel_chunk(&mut self, _white: bool, _chunk_count: u32) {}

    fn next_line(&mut self) {}
}
