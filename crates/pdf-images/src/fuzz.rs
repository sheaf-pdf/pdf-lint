//! Fuzzing entry points and seed corpus generator for `pdf-images`.

use std::collections::BTreeMap;

use pdf_content_streams::{ResCx, operations};
use pdf_filters::Level;
use pdf_objects::fault::Place;
use pdf_objects::streams::{DecodedStream, FilterStep, FilterTail};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Rules, Stream, Streams, Unreadable, Why, dict};

use crate::{
    DecodeOptions, Fault, JpegColorSpace, Limits, decode_inline_with, decode_stream_with,
    decode_xobject_raw, decode_xobject_raw_with, decode_xobject_with, encode_jpeg,
    encode_raw_flate, encode_rgba_flate,
};

struct FuzzStore {
    objects: BTreeMap<ObjectId, Object>,
}

impl<'o> Resolve<'o> for &'o FuzzStore {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.get(&id)
    }
}

impl Streams for &FuzzStore {
    fn decoded(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<std::borrow::Cow<'_, [u8]>, Unreadable> {
        let Some(stream) = self.objects.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "not a stream"));
        };
        let Some(bytes) = stream.data.bytes() else {
            return Err(Unreadable::new(id, Why::Detached, "detached"));
        };
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(std::borrow::Cow::Borrowed(bytes))
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        let Some(stream) = self.objects.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "not a stream"));
        };
        let Some(bytes) = stream.data.bytes() else {
            return Err(Unreadable::new(id, Why::Detached, "detached"));
        };
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        let tail = stream
            .dict
            .get(b"Filter")
            .and_then(Object::as_name)
            .filter(|n| {
                matches!(
                    *n,
                    b"DCTDecode" | b"CCITTFaxDecode" | b"JBIG2Decode" | b"JPXDecode"
                )
            })
            .map(|name| {
                let parms = stream
                    .dict
                    .get(b"DecodeParms")
                    .and_then(Object::as_dict)
                    .cloned();
                FilterTail::new(FilterStep::new(name, parms), Vec::new())
            });
        Ok(DecodedStream::new(std::borrow::Cow::Borrowed(bytes), tail))
    }
}

fn tight_limits() -> Limits {
    let mut lim = Limits::DEFAULT;
    lim.max_width = 256;
    lim.max_height = 256;
    lim.max_pixels = 65_536;
    lim.max_sample_bytes = 512 * 1024;
    lim.max_encoded_bytes = 256 * 1024;
    lim
}

/// Exercises all `pdf-images` codecs, mask paths, inline-image chains, scrubbing, re-encoders, and
/// fault-sink parity invariants on arbitrary `data` without panicking.
pub fn images(data: &[u8]) {
    if data.len() < 6 {
        return;
    }
    let lim = tight_limits();
    let mode = data[0];
    let w = i64::from((data[1] % 32).max(1));
    let h = i64::from((data[2] % 32).max(1));
    let bpc_choice = [1_i64, 2, 4, 8, 16][(data[3] as usize) % 5];
    let payload = &data[5..];

    let mut d = Dict::new();
    d.set("Subtype", Object::name("Image"));
    d.set("Width", w);
    d.set("Height", h);

    match mode % 7 {
        0 => {
            // Raw / packed samples
            let cs = match data[4] % 4 {
                0 => Object::name("DeviceGray"),
                1 => Object::name("DeviceRGB"),
                2 => Object::name("DeviceCMYK"),
                _ => Object::from(vec![
                    Object::name("Indexed"),
                    Object::name("DeviceRGB"),
                    Object::from(3_i64),
                    Object::bytes(&b"\x00\x00\x00\xff\x00\x00\x00\xff\x00\x00\x00\xff"[..]),
                ]),
            };
            d.set("ColorSpace", cs);
            d.set("BitsPerComponent", bpc_choice);
        }
        1 => {
            d.set("ImageMask", true);
            d.set("BitsPerComponent", 1_i64);
            if data[4] & 1 != 0 {
                d.set("Decode", vec![Object::from(1_i64), Object::from(0_i64)]);
            }
        }
        2 => {
            d.set(
                "ColorSpace",
                Object::name(if data[4] & 1 == 0 {
                    "DeviceRGB"
                } else {
                    "DeviceCMYK"
                }),
            );
            d.set("BitsPerComponent", 8_i64);
            d.set("Filter", Object::name("DCTDecode"));
            d.set(
                "DecodeParms",
                dict! { "ColorTransform" => i64::from(data[4] & 1) },
            );
        }
        3 => {
            d.set("ColorSpace", Object::name("DeviceGray"));
            d.set("BitsPerComponent", 1_i64);
            d.set("Filter", Object::name("CCITTFaxDecode"));
            let k = match data[4] % 3 {
                0 => -1_i64,
                1 => 0_i64,
                _ => 1_i64,
            };
            d.set(
                "DecodeParms",
                dict! {
                    "K" => k,
                    "Columns" => w,
                    "Rows" => h,
                    "BlackIs1" => (data[4] & 4) != 0,
                },
            );
        }
        4 => {
            d.set("ColorSpace", Object::name("DeviceGray"));
            d.set("BitsPerComponent", 1_i64);
            d.set("Filter", Object::name("JBIG2Decode"));
            d.set(
                "DecodeParms",
                dict! { "JBIG2Globals" => ObjectId::new(2, 0) },
            );
        }
        5 => {
            if data[4] & 1 == 0 {
                d.set("ColorSpace", Object::name("DeviceRGB"));
            }
            d.set("Filter", Object::name("JPXDecode"));
            d.set("SMaskInData", i64::from(data[4] % 3));
        }
        _ => {
            // Inline image path
            let res = ResCx::none();
            let mut ops = operations(payload, &res, &lim.content);
            let empty_store = FuzzStore {
                objects: BTreeMap::new(),
            };
            while let Some(Ok(op)) = ops.next() {
                if let Some(inline) = op.inline_image() {
                    let mut faults: Vec<(Place, Fault)> = Vec::new();
                    let r1 = decode_inline_with(
                        &empty_store,
                        &inline,
                        payload,
                        None,
                        &DecodeOptions::PDFIUM,
                        &lim,
                        &mut (),
                    );
                    let r2 = decode_inline_with(
                        &empty_store,
                        &inline,
                        payload,
                        None,
                        &DecodeOptions::PDFIUM,
                        &lim,
                        &mut faults,
                    );
                    assert_eq!(r1, r2);
                }
            }
            return;
        }
    }

    let mid = payload.len() / 2;
    let mut store = FuzzStore {
        objects: BTreeMap::new(),
    };
    if mode % 7 == 4 {
        store.objects.insert(
            ObjectId::new(2, 0),
            Object::from(Stream::new(Dict::new(), payload[..mid].to_vec())),
        );
    }
    // Optional soft mask on object 3
    if data[4] & 0x80 != 0 {
        let sm_dict = dict! {
            "Subtype" => Object::name("Image"),
            "Width" => w,
            "Height" => h,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 8_i64,
        };
        store.objects.insert(
            ObjectId::new(3, 0),
            Object::from(Stream::new(sm_dict, payload[..mid].to_vec())),
        );
        d.set("SMask", ObjectId::new(3, 0));
    }

    let id = ObjectId::new(1, 0);
    store
        .objects
        .insert(id, Object::from(Stream::new(d, payload.to_vec())));

    for rules in [Rules::Pdfium, Rules::Iso] {
        let opts = DecodeOptions {
            color: pdf_color::EvalOptions::from_spaces(rules, None),
            stencil_color: [20, 40, 60, 255],
            apply_masks: true,
        };

        // Fault sink parity check on raw and RGBA8 paths.
        let mut faults_raw: Vec<(Place, Fault)> = Vec::new();
        let raw_silent = decode_xobject_raw(&store, id, None, &opts, &lim);
        let raw_loud = decode_xobject_raw_with(&store, id, None, &opts, &lim, &mut faults_raw);
        assert_eq!(raw_silent.is_ok(), raw_loud.is_ok());

        let mut faults_rgba: Vec<(Place, Fault)> = Vec::new();
        let rgba_silent = decode_xobject_with(&store, id, None, &opts, &lim, &mut ());
        let rgba_loud = decode_xobject_with(&store, id, None, &opts, &lim, &mut faults_rgba);
        assert_eq!(rgba_silent, rgba_loud);

        if let (Ok(mut raw), Ok(rgba)) = (raw_silent, rgba_silent) {
            // Scrubbing and Flate round-trip on small images.
            let fill = [0_u16; 16];
            let _ = raw.fill_rect_raw(0, 0, 2, 2, &fill);
            if raw.width() <= 32
                && raw.height() <= 32
                && matches!(
                    raw.meta().color_space,
                    None | Some(
                        pdf_color::Family::DeviceGray
                            | pdf_color::Family::DeviceRGB
                            | pdf_color::Family::DeviceCMYK
                    )
                )
                && let Ok(enc) = encode_raw_flate(&raw, None, Level::NONE)
            {
                let _ = decode_stream_with(&store, &enc.stream, None, &opts, &lim, &mut ());
            }
            if rgba.width() <= 16 && rgba.height() <= 16 {
                let enc = encode_rgba_flate(&rgba, Level::NONE).unwrap();
                let _ = decode_stream_with(&store, &enc.stream, None, &opts, &lim, &mut ());
            }
        }
    }

    // Exercise baseline JPEG encoder + decoder round-trip on small inputs.
    if payload.len() >= 12 {
        let jw = u32::from((data[1] % 8).max(1));
        let jh = u32::from((data[2] % 8).max(1));
        let jspace = match data[3] % 3 {
            0 => JpegColorSpace::Gray,
            1 => JpegColorSpace::Rgb,
            _ => JpegColorSpace::Cmyk,
        };
        let need = (jw as usize) * (jh as usize) * jspace.components();
        let mut pix = vec![0_u8; need];
        for (i, slot) in pix.iter_mut().enumerate() {
            *slot = payload[i % payload.len()];
        }
        let stream = encode_jpeg(&pix, jw, jh, jspace, data[4]).unwrap();
        let empty = FuzzStore {
            objects: BTreeMap::new(),
        };
        let decoded =
            decode_stream_with(&empty, &stream, None, &DecodeOptions::PDFIUM, &lim, &mut ())
                .unwrap();
        assert_eq!(decoded.width(), jw);
        assert_eq!(decoded.height(), jh);
    }
}

/// Returns deterministic seed inputs covering raw, stencil, DCT, CCITT, inline, and soft-masked
/// streams.
#[must_use]
pub fn seeds() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    // 1. Raw RGB 4x4
    let mut s0 = vec![0, 4, 4, 3, 1];
    s0.extend((0..48_u8).map(|i| i.wrapping_mul(5)));
    out.push(s0);

    // 2. Stencil 8x4
    out.push(vec![1, 8, 4, 0, 1, 0xAA, 0x55, 0xF0, 0x0F]);

    // 3. Synthetic RGB JPEG 4x4
    let jpeg_stream = encode_jpeg(&[120_u8; 48], 4, 4, JpegColorSpace::Rgb, 85).unwrap();
    let mut s2 = vec![2, 4, 4, 3, 0];
    s2.extend_from_slice(jpeg_stream.data.bytes().unwrap());
    out.push(s2);

    // 4. Inline ASCIIHex grayscale image
    let mut s6 = vec![6, 2, 2, 3, 0];
    s6.extend_from_slice(b"BI /W 2 /H 2 /BPC 8 /CS /G /F /AHx ID 004080ff> EI");
    out.push(s6);

    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    #[ignore = "writes fuzz seeds when PDF_IMAGES_FUZZ_SEEDS is set"]
    fn the_seeds_are_written() {
        let dir = std::env::var("PDF_IMAGES_FUZZ_SEEDS").expect("PDF_IMAGES_FUZZ_SEEDS");
        std::fs::create_dir_all(&dir).unwrap();
        for (i, seed) in seeds().into_iter().enumerate() {
            let path = std::path::Path::new(&dir).join(format!("seed-{i:03}"));
            std::fs::write(path, seed).unwrap();
        }
    }

    #[test]
    fn seeds_and_mutations_keep_every_promise() {
        for seed in seeds() {
            images(&seed);
            for idx in 0..seed.len().min(32) {
                let mut mutated = seed.clone();
                mutated[idx] ^= 0x5A;
                images(&mutated);
            }
        }
    }
}
