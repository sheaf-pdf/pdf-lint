//! A JPEG file's bytes as an Image XObject, unchanged ([`wrap_jpeg`]): what
//! a `/DCTDecode` reader needs of its frame header written in the dictionary,
//! the bytes themselves the stream's data.

use pdf_objects::{Dict, Object, Stream, dict};

use crate::error::{EncodeError, JpegRefused};

/// What [`wrap_jpeg`] reads of a JPEG before its first scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    width: u16,
    height: u16,
    /// Each component's identifier, in the frame header's order.
    ids: [u8; 4],
    components: u8,
    /// A JFIF `APP0` segment before the scan.
    jfif: bool,
    /// An Adobe `APP14` segment before the scan, and its transform byte.
    adobe: Option<u8>,
}

/// The frame of the JPEG `bytes` hold, or why a PDF can't hold it as it is.
fn frame(bytes: &[u8]) -> Result<Frame, JpegRefused> {
    if bytes.len() < 2 || bytes[..2] != [0xFF, 0xD8] {
        return Err(JpegRefused::NoStart);
    }
    let mut frame: Option<Frame> = None;
    let (mut jfif, mut adobe) = (false, None);
    let mut i = 2;
    loop {
        // A marker: 0xFF, any fill bytes of 0xFF, then its code.
        if bytes.get(i) != Some(&0xFF) {
            return Err(JpegRefused::Malformed);
        }
        while bytes.get(i) == Some(&0xFF) {
            i += 1;
        }
        let Some(&marker) = bytes.get(i) else {
            return Err(JpegRefused::Malformed);
        };
        i += 1;
        match marker {
            // The first scan: the headers are read.
            0xDA => break,
            // The image's end before any scan.
            0xD9 => return Err(JpegRefused::NoScan),
            // Markers with no segment, which don't come before a scan.
            0x00 | 0x01 | 0xD0..=0xD8 => return Err(JpegRefused::Malformed),
            _ => {}
        }
        let Some(&[hi, lo]) = bytes.get(i..i + 2) else {
            return Err(JpegRefused::Malformed);
        };
        let len = usize::from(u16::from_be_bytes([hi, lo]));
        let Some(payload) = (len >= 2).then(|| bytes.get(i + 2..i + len)).flatten() else {
            return Err(JpegRefused::Malformed);
        };
        match marker {
            // Baseline, extended sequential and progressive DCT, Huffman
            // coded: what a `/DCTDecode` reader decodes.
            0xC0..=0xC2 => {
                if frame.is_some() {
                    return Err(JpegRefused::Process(marker));
                }
                let &[precision, h1, h0, w1, w0, components, ref specs @ ..] = payload else {
                    return Err(JpegRefused::Malformed);
                };
                if precision != 8 {
                    return Err(JpegRefused::Precision(precision));
                }
                if !matches!(components, 1 | 3 | 4) {
                    return Err(JpegRefused::Components(components));
                }
                if specs.len() != usize::from(components) * 3 {
                    return Err(JpegRefused::Malformed);
                }
                let mut ids = [0; 4];
                for (id, spec) in ids.iter_mut().zip(specs.as_chunks::<3>().0) {
                    *id = spec[0];
                }
                frame = Some(Frame {
                    width: u16::from_be_bytes([w1, w0]),
                    height: u16::from_be_bytes([h1, h0]),
                    ids,
                    components,
                    jfif: false,
                    adobe: None,
                });
            }
            // Lossless, differential and arithmetic-coded frames, the
            // reserved JPG extension, and a hierarchy of frames: no
            // `/DCTDecode` reader is asked to decode them.
            0xC3 | 0xC5..=0xC7 | 0xC8..=0xCB | 0xCD..=0xCF | 0xDE => {
                return Err(JpegRefused::Process(marker));
            }
            0xE0 if payload.starts_with(b"JFIF\0") => jfif = true,
            0xEE if payload.len() >= 12 && payload.starts_with(b"Adobe") => {
                adobe = Some(payload[11]);
            }
            _ => {}
        }
        i += len;
    }
    let frame = frame.ok_or(JpegRefused::NoFrame)?;
    Ok(Frame {
        jfif,
        adobe,
        ..frame
    })
}

/// Wraps a JPEG file's bytes as a PDF Image XObject [`Stream`] with no change
/// to them: `/Filter /DCTDecode`, and `/Width`, `/Height`, `/ColorSpace` and
/// `/BitsPerComponent` from the file's frame header. Nothing is decoded and
/// nothing encoded again, so a photograph or a scan goes onto a page at the
/// quality and size it has. What the file carries beside its image (EXIF,
/// XMP, ICC, a thumbnail, comments) stays in the bytes: for an image with
/// those taken out, decode it and use [`encode_jpeg`](crate::encode_jpeg).
///
/// A baseline, extended sequential or progressive JPEG of 8-bit samples and
/// one, three or four components is taken:
///
/// - **One component** is `/DeviceGray`.
/// - **Three** are `/DeviceRGB`. A reader takes them for YCbCr unless the
///   file's Adobe `APP14` segment says they're RGB as stored, which it reads
///   in the bytes. A file with neither a JFIF nor an Adobe segment whose
///   components are named `R`, `G` and `B` is RGB as stored by libjpeg's
///   rule, which a PDF reader doesn't apply: `/DecodeParms` then says so
///   (`/ColorTransform 0`).
/// - **Four** are `/DeviceCMYK`. A file with an Adobe `APP14` segment is
///   taken to hold them as Adobe's applications write them, inverted (0 for
///   full ink), and is given `/Decode [1 0 1 0 1 0 1 0]`; one without is taken
///   as stored. The file doesn't say which, so a caller that knows its
///   samples aren't inverted (they came from
///   [`encode_jpeg_bytes`](crate::encode_jpeg_bytes), which writes a PDF's
///   own polarity) removes `/Decode` from the stream's dictionary.
///
/// The dictionary is the caller's to add to (`/SMask`, `/Interpolate`, an
/// `/ICCBased` `/ColorSpace` made from the file's profile).
///
/// ```
/// use pdf_images::{JpegColorSpace, encode_jpeg_bytes, wrap_jpeg};
///
/// let file = encode_jpeg_bytes(&[200; 6 * 4 * 3], 6, 4, JpegColorSpace::Rgb, 90).unwrap();
/// let image = wrap_jpeg(file.clone()).unwrap();
/// assert_eq!(image.data.bytes(), Some(&file[..]));
/// let entry = |key: &str| image.dict.get(key).unwrap();
/// assert_eq!(entry("Filter").as_name(), Some(&b"DCTDecode"[..]));
/// assert_eq!(entry("ColorSpace").as_name(), Some(&b"DeviceRGB"[..]));
/// assert_eq!((entry("Width").as_integer(), entry("Height").as_integer()), (Some(6), Some(4)));
/// ```
///
/// # Errors
///
/// Returns [`EncodeError::Jpeg`] where the bytes aren't a JPEG a
/// `/DCTDecode` reader decodes as they are ([`JpegRefused`] says why: no JPEG
/// at their start, headers that don't read, a lossless, hierarchical or
/// arithmetic-coded process, samples that aren't 8-bit, two components or
/// more than four), or [`EncodeError::Dimensions`] where the frame header
/// gives a width or a height of zero (a height written after the scan, in a
/// `DNL` segment, among them).
pub fn wrap_jpeg(jpeg: Vec<u8>) -> Result<Stream, EncodeError> {
    let frame = frame(&jpeg)?;
    let (width, height) = (u32::from(frame.width), u32::from(frame.height));
    if width == 0 || height == 0 {
        return Err(EncodeError::Dimensions { width, height });
    }
    let mut d = Dict::new();
    d.set("Type", Object::name("XObject"));
    d.set("Subtype", Object::name("Image"));
    d.set("Width", i64::from(width));
    d.set("Height", i64::from(height));
    let space = match frame.components {
        1 => "DeviceGray",
        3 => "DeviceRGB",
        _ => "DeviceCMYK",
    };
    d.set("ColorSpace", Object::name(space));
    d.set("BitsPerComponent", 8_i64);
    if frame.components == 4 && frame.adobe.is_some() {
        let inverted: Vec<Object> = [1, 0, 1, 0, 1, 0, 1, 0].map(Object::from).into();
        d.set("Decode", inverted);
    }
    d.set("Filter", Object::name("DCTDecode"));
    let rgb_as_stored =
        frame.components == 3 && !frame.jfif && frame.adobe.is_none() && frame.ids[..3] == *b"RGB";
    if rgb_as_stored {
        d.set("DecodeParms", dict! { "ColorTransform" => 0 });
    }
    Ok(Stream::new(d, jpeg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JpegColorSpace, encode_jpeg, encode_jpeg_bytes};

    /// A JPEG's bytes up to its first scan, made of `segments` (a marker's
    /// code and its payload each), then a scan marker.
    fn headers(segments: &[(u8, &[u8])]) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        for (marker, payload) in segments {
            out.extend_from_slice(&[0xFF, *marker]);
            out.extend_from_slice(&(payload.len() as u16 + 2).to_be_bytes());
            out.extend_from_slice(payload);
        }
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        out
    }

    /// A frame header's payload: `precision`, 5 rows of 7 columns, and
    /// components named `ids`.
    fn sof(precision: u8, ids: &[u8]) -> Vec<u8> {
        let mut out = vec![precision, 0, 5, 0, 7, ids.len() as u8];
        for id in ids {
            out.extend_from_slice(&[*id, 0x11, 0]);
        }
        out
    }

    const JFIF: &[u8] = b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0";

    fn adobe(transform: u8) -> Vec<u8> {
        let mut out = b"Adobe\0\x64\0\0\0\0".to_vec();
        out.push(transform);
        out
    }

    fn refused(bytes: Vec<u8>) -> JpegRefused {
        match wrap_jpeg(bytes) {
            Err(EncodeError::Jpeg(why)) => why,
            other => panic!("not refused as a JPEG: {other:?}"),
        }
    }

    #[test]
    fn what_the_encoder_writes_wraps_as_the_encoder_wraps_it() {
        for (space, pixel) in [(JpegColorSpace::Gray, 1), (JpegColorSpace::Rgb, 3)] {
            let pixels = vec![90_u8; 9 * 5 * pixel];
            let file = encode_jpeg_bytes(&pixels, 9, 5, space, 85).unwrap();
            let wrapped = wrap_jpeg(file.clone()).unwrap();
            assert_eq!(wrapped, encode_jpeg(&pixels, 9, 5, space, 85).unwrap());
            assert_eq!(wrapped.data.bytes(), Some(&file[..]));
        }
        // CMYK with an Adobe segment is taken as Adobe's applications write
        // it, inverted: the one entry more than the encoder's own.
        let pixels = vec![40_u8; 9 * 5 * 4];
        let file = encode_jpeg_bytes(&pixels, 9, 5, JpegColorSpace::Cmyk, 85).unwrap();
        let mut wrapped = wrap_jpeg(file).unwrap();
        let decode = wrapped.dict.remove("Decode").unwrap();
        let numbers: Vec<i64> = decode
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_integer().unwrap())
            .collect();
        assert_eq!(numbers, [1, 0, 1, 0, 1, 0, 1, 0]);
        assert_eq!(
            wrapped,
            encode_jpeg(&pixels, 9, 5, JpegColorSpace::Cmyk, 85).unwrap()
        );
    }

    #[test]
    fn the_frame_header_gives_the_dictionary() {
        let entry = |stream: &Stream, key: &str| stream.dict.get(key).cloned();
        let name = |stream: &Stream, key: &str| {
            entry(stream, key).map(|n| String::from_utf8(n.as_name().unwrap().to_vec()).unwrap())
        };
        // Progressive and extended sequential frames are taken as baseline is.
        for marker in [0xC0, 0xC1, 0xC2] {
            let gray = wrap_jpeg(headers(&[(0xE0, JFIF), (marker, &sof(8, &[1]))])).unwrap();
            assert_eq!(name(&gray, "ColorSpace").as_deref(), Some("DeviceGray"));
            assert_eq!(entry(&gray, "Width").unwrap().as_integer(), Some(7));
            assert_eq!(entry(&gray, "Height").unwrap().as_integer(), Some(5));
            assert_eq!(
                entry(&gray, "BitsPerComponent").unwrap().as_integer(),
                Some(8)
            );
            assert_eq!(name(&gray, "Filter").as_deref(), Some("DCTDecode"));
            assert_eq!(name(&gray, "Subtype").as_deref(), Some("Image"));
            assert!(entry(&gray, "Decode").is_none() && entry(&gray, "DecodeParms").is_none());
        }
        // Fill bytes before a marker and segments it doesn't know are passed.
        let mut filled = vec![0xFF, 0xD8, 0xFF, 0xFF, 0xFF, 0xFE, 0x00, 0x04, b'h', b'i'];
        filled.extend_from_slice(&headers(&[(0xC0, &sof(8, &[1, 2, 3]))])[2..]);
        let rgb = wrap_jpeg(filled).unwrap();
        assert_eq!(name(&rgb, "ColorSpace").as_deref(), Some("DeviceRGB"));
        assert!(entry(&rgb, "DecodeParms").is_none());

        // Four components: inverted with an Adobe segment, as stored without.
        let marked = wrap_jpeg(headers(&[
            (0xEE, &adobe(2)),
            (0xC0, &sof(8, &[1, 2, 3, 4])),
        ]));
        let marked = marked.unwrap();
        assert_eq!(name(&marked, "ColorSpace").as_deref(), Some("DeviceCMYK"));
        assert!(entry(&marked, "Decode").is_some());
        let bare = wrap_jpeg(headers(&[(0xC0, &sof(8, &[1, 2, 3, 4]))])).unwrap();
        assert_eq!(name(&bare, "ColorSpace").as_deref(), Some("DeviceCMYK"));
        assert!(entry(&bare, "Decode").is_none());
    }

    #[test]
    fn components_named_rgb_with_no_jfif_or_adobe_segment_are_rgb_as_stored() {
        let transform = |segments: &[(u8, &[u8])]| {
            let stream = wrap_jpeg(headers(segments)).unwrap();
            let parms = stream.dict.get("DecodeParms")?.as_dict()?.clone();
            parms.get("ColorTransform")?.as_integer()
        };
        let rgb = sof(8, b"RGB");
        assert_eq!(transform(&[(0xC0, &rgb)]), Some(0));
        // A JFIF segment says YCbCr whatever the names; an Adobe one says
        // which itself, in the bytes; and other names are YCbCr.
        assert_eq!(transform(&[(0xE0, JFIF), (0xC0, &rgb)]), None);
        assert_eq!(transform(&[(0xEE, &adobe(0)), (0xC0, &rgb)]), None);
        assert_eq!(transform(&[(0xEE, &adobe(1)), (0xC0, &rgb)]), None);
        assert_eq!(transform(&[(0xC0, &sof(8, &[1, 2, 3]))]), None);
        assert_eq!(transform(&[(0xC0, &sof(8, b"BGR"))]), None);
        // An APP0 that isn't JFIF's (JFXX, say) doesn't say YCbCr.
        assert_eq!(transform(&[(0xE0, b"JFXX\0\x10"), (0xC0, &rgb)]), Some(0));
    }

    #[test]
    fn what_a_dct_reader_wont_decode_is_refused_and_why() {
        assert_eq!(refused(Vec::new()), JpegRefused::NoStart);
        assert_eq!(refused(b"\x89PNG\r\n\x1a\n".to_vec()), JpegRefused::NoStart);
        // Bytes before the JPEG's start aren't skipped: they'd be written.
        let mut led = vec![0];
        led.extend_from_slice(&headers(&[(0xC0, &sof(8, &[1]))]));
        assert_eq!(refused(led), JpegRefused::NoStart);

        // Lossless, differential, arithmetic and hierarchical processes.
        for marker in [
            0xC3, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF,
        ] {
            let why = refused(headers(&[(marker, &sof(8, &[1]))]));
            assert_eq!(why, JpegRefused::Process(marker));
        }
        let hierarchy = headers(&[(0xDE, &sof(8, &[1])), (0xC0, &sof(8, &[1]))]);
        assert_eq!(refused(hierarchy), JpegRefused::Process(0xDE));
        let two_frames = headers(&[(0xC0, &sof(8, &[1])), (0xC2, &sof(8, &[1]))]);
        assert_eq!(refused(two_frames), JpegRefused::Process(0xC2));

        assert_eq!(
            refused(headers(&[(0xC1, &sof(12, &[1]))])),
            JpegRefused::Precision(12)
        );
        assert_eq!(
            refused(headers(&[(0xC0, &sof(8, &[1, 2]))])),
            JpegRefused::Components(2)
        );
        let five = refused(headers(&[(0xC0, &sof(8, &[1, 2, 3, 4, 5]))]));
        assert_eq!(five, JpegRefused::Components(5));

        // No frame before the scan, no scan before the end, and headers cut
        // short or wrong in their lengths.
        assert_eq!(refused(headers(&[(0xE0, JFIF)])), JpegRefused::NoFrame);
        assert_eq!(refused(vec![0xFF, 0xD8, 0xFF, 0xD9]), JpegRefused::NoScan);
        let whole = headers(&[(0xE0, JFIF), (0xC0, &sof(8, &[1, 2, 3]))]);
        for cut in 2..whole.len() - 2 {
            let why = refused(whole[..cut].to_vec());
            assert_eq!(why, JpegRefused::Malformed, "cut at {cut}");
        }
        let short_frame = headers(&[(0xC0, &sof(8, &[1, 2, 3])[..10])]);
        assert_eq!(refused(short_frame), JpegRefused::Malformed);
        assert_eq!(
            refused(vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 1]),
            JpegRefused::Malformed
        );
        assert_eq!(
            refused(vec![0xFF, 0xD8, 0xFF, 0xD0, 0xFF, 0xDA]),
            JpegRefused::Malformed
        );
        assert_eq!(
            refused(vec![0xFF, 0xD8, 0x00, 0xFF, 0xDA]),
            JpegRefused::Malformed
        );
    }

    #[test]
    fn a_frame_with_no_width_or_height_is_refused_for_its_dimensions() {
        let mut zero_height = sof(8, &[1]);
        zero_height[1..3].copy_from_slice(&[0, 0]);
        assert_eq!(
            wrap_jpeg(headers(&[(0xC0, &zero_height)])),
            Err(EncodeError::Dimensions {
                width: 7,
                height: 0
            })
        );
        let mut zero_width = sof(8, &[1]);
        zero_width[3..5].copy_from_slice(&[0, 0]);
        assert_eq!(
            wrap_jpeg(headers(&[(0xC0, &zero_width)])),
            Err(EncodeError::Dimensions {
                width: 0,
                height: 5
            })
        );
    }
}
