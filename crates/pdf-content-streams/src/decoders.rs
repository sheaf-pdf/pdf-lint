//! With the `image-ends` feature: where an inline image's DCT and CCITTFax
//! data ends, as PDFium's decoders find it (its `DecodeInlineStream`, which
//! decodes every row of the image and takes where its decoder's source
//! then stands), the data decoded by zune-jpeg and hayro-ccitt, which say
//! how far they read. What's decoded isn't kept.

use std::cell::Cell;

use hayro_ccitt::{DecodeError, DecodeSettings, DecoderContext, EncodingMode};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::{ZByteIoError, ZByteReaderTrait, ZSeekFrom};
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
/// it; what decoding makes, and its coefficients, under `limit`, decoded
/// into `out`, which is cleared after. None past `limit`.
///
/// As PDFium (its `LibjpegScanlineDecoder` on libjpeg-turbo): the JPEG
/// from its first start of image on, or from the data's start where
/// there's none; the content's last two bytes read as an end of image (FF
/// D9) where they aren't one. Refused where its header doesn't read, or
/// where it's narrower than `width`, or its rows of all its components, a
/// byte each, are none or past 32 bits. Otherwise its end is
/// where the decoder's source stands once every row is decoded, counted
/// from the start of image, not the data's start, as PDFium counts it, so
/// where bytes come before the start of image the end is short by as many.
/// zune-jpeg reads on to the marker after the last scan and past it, which
/// libjpeg-turbo's reader takes too as it fills its bits, at the end of
/// image; where none comes after the entropy-coded data, as where a JPEG
/// is cut short, zune-jpeg reads on to the content's end where
/// libjpeg-turbo stops at the bits its last row needed. Where zune-jpeg
/// can't decode what libjpeg-turbo reads past (damaged data, which
/// libjpeg-turbo reads on through), the end is left to the stand-in; and
/// so where it stops short of the end of image without an error, as it
/// does at a scan with no coded data, where libjpeg-turbo reads on through
/// every scan to that end. A header zune-jpeg doesn't read, as one of
/// 12-bit samples, which libjpeg-turbo does, is read as refused. What the
/// decoder read, counted from the data's start, runs to just past the end
/// of image, where the data ends as ISO 32000-2 reads it.
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
    let (Ok(()), Some(info), Some(size)) = (
        decoder.decode_headers(),
        decoder.info(),
        decoder.output_buffer_size(),
    ) else {
        return refused();
    };
    let (w, h, components) = (
        u32::from(info.width),
        u32::from(info.height),
        u32::from(info.components),
    );
    // The rows PDFium decodes, a byte for each sample.
    let rows = w.checked_mul(components).and_then(|row| row.checked_mul(h));
    if w < width || matches!(rows, None | Some(0)) {
        return refused();
    }
    // What decoding holds: its output, and the coefficients a decoder of
    // several scans keeps, two bytes for each sample of each component,
    // its blocks whole however the components are sampled.
    let padded = |n: u32| (n as usize).div_ceil(32) * 32;
    let made = (padded(w).saturating_mul(padded(h)))
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
    // A whole decode stands just past the end of image it read last.
    let whole = at.get() >= 2 && [at.get() - 2, at.get() - 1].map(|i| as_read(span, i)) == EOI;
    let end = match decoded {
        Ok(()) if !whole => Found::Undecoded(read),
        Ok(()) if i32::try_from(at.get()).is_ok() => Found::At {
            end: at.get(),
            read,
        },
        Ok(()) => Found::Refused(read),
        Err(_) => Found::Undecoded(read),
    };
    Some(Decoded { end, made })
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
/// It reads no more than the data, but where it can't decode it, it says
/// nothing of how far it read, so all of it counts as read.
pub(crate) fn ccitt(data: &[u8], width: i32, height: i32, fax: Fax) -> Found {
    let Some(settings) = settings(width, height, fax) else {
        return Found::Refused(0);
    };
    match decode(data, settings) {
        Ok(end) if i32::try_from(end).is_ok() => Found::At { end, read: end },
        Ok(end) => Found::Refused(end),
        Err(DecodeError::UnexpectedEof) => Found::At {
            end: data.len(),
            read: data.len(),
        },
        Err(_) => Found::Undecoded(data.len()),
    }
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
