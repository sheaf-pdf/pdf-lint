//! Fuzzing entry points and seed corpus generator for `pdf-images`.

use std::collections::BTreeMap;

use pdf_content_streams::{ResCx, operations};
use pdf_filters::{Level, Reading as FilterReading};
use pdf_objects::fault::Place;
use pdf_objects::streams::DecodedStream;
use pdf_objects::{
    Dict, Object, ObjectId, Resolve, Rules, Stream, Streams, Unreadable, Value, Why, dict,
};

use crate::inline_chain::decode_inline_standard;
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
        let ds = self.decoded_standard(id, limit)?;
        if ds.tail.is_some() {
            return Err(Unreadable::new(id, Why::Filter, "non-standard tail"));
        }
        Ok(ds.data)
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
        decode_inline_standard(
            *self,
            &stream.dict,
            bytes,
            limit,
            FilterReading::Pdfium,
            Some(Place::new(Some(id), None)),
            &mut (),
        )
        .map_err(|e| match e {
            crate::Error::Limit { .. } => Unreadable::new(id, Why::Limit, "past limit"),
            crate::Error::UnsupportedFilter { .. } => {
                Unreadable::new(id, Why::Filter, "unsupported filter")
            }
            crate::Error::Unreadable(u) => u,
            _ => Unreadable::new(id, Why::Damaged, "damaged stream"),
        })
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

/// Exercises all `pdf-images` codecs, standard filter chains, `/Mask`, `/SMask` + `/Matte`,
/// `/Decode`, inline images, scrubbing, re-encoders, and fault-sink parity invariants on arbitrary
/// `data` without panicking.
pub fn images(data: &[u8]) {
    if data.len() < 6 {
        return;
    }
    let lim = tight_limits();
    let mode = data[0];
    let w = i64::from((data[1] % 32).max(1));
    let h = i64::from((data[2] % 32).max(1));
    let bpc_choice = [1_i64, 2, 4, 8, 16][(data[3] as usize) % 5];
    let flags = data[4];
    let payload = &data[5..];

    let mut d = Dict::new();
    d.set("Subtype", Object::name("Image"));
    d.set("Width", w);
    d.set("Height", h);

    match mode % 7 {
        0 => {
            // Raw or Flate/ASCIIHex-filtered samples
            let (cs, comps) = match flags % 4 {
                0 => (Object::name("DeviceGray"), 1_usize),
                1 => (Object::name("DeviceRGB"), 3),
                2 => (Object::name("DeviceCMYK"), 4),
                _ => (
                    Object::from(vec![
                        Object::name("Indexed"),
                        Object::name("DeviceRGB"),
                        Object::from(3_i64),
                        Object::bytes(&b"\x00\x00\x00\xff\x00\x00\x00\xff\x00\x00\x00\xff"[..]),
                    ]),
                    1,
                ),
            };
            d.set("ColorSpace", cs);
            d.set("BitsPerComponent", bpc_choice);
            if flags & 0x04 != 0 {
                d.set("Filter", Object::name("FlateDecode"));
            } else if flags & 0x08 != 0 {
                d.set("Filter", Object::name("ASCIIHexDecode"));
            }
            if flags & 0x10 != 0 {
                let mut dec = Vec::with_capacity(comps * 2);
                for c in 0..comps {
                    if c & 1 == 0 {
                        dec.push(Object::from(1.0_f64));
                        dec.push(Object::from(0.0_f64));
                    } else {
                        dec.push(Object::from(-0.25_f64));
                        dec.push(Object::from(1.25_f64));
                    }
                }
                d.set("Decode", dec);
            }
        }
        1 => {
            d.set("ImageMask", true);
            d.set("BitsPerComponent", 1_i64);
            if flags & 1 != 0 {
                d.set("Decode", vec![Object::from(1_i64), Object::from(0_i64)]);
            }
        }
        2 => {
            d.set(
                "ColorSpace",
                Object::name(if flags & 1 == 0 {
                    "DeviceRGB"
                } else {
                    "DeviceCMYK"
                }),
            );
            d.set("BitsPerComponent", 8_i64);
            d.set("Filter", Object::name("DCTDecode"));
            d.set(
                "DecodeParms",
                dict! { "ColorTransform" => i64::from(flags & 1) },
            );
        }
        3 => {
            d.set("ColorSpace", Object::name("DeviceGray"));
            d.set("BitsPerComponent", 1_i64);
            d.set("Filter", Object::name("CCITTFaxDecode"));
            let k = match flags % 3 {
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
                    "BlackIs1" => (flags & 4) != 0,
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
            if flags & 1 == 0 {
                d.set("ColorSpace", Object::name("DeviceRGB"));
            }
            d.set("Filter", Object::name("JPXDecode"));
            d.set("SMaskInData", i64::from(flags % 3));
        }
        _ => {
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

    // Exercise `/SMask` (+ optional `/Matte`), explicit stencil `/Mask`, and color-key `/Mask`.
    if mode % 7 != 1 {
        match (flags >> 5) & 0x03 {
            1 => {
                let mut sm_dict = dict! {
                    "Subtype" => Object::name("Image"),
                    "Width" => (w / 2).max(1),
                    "Height" => (h / 2).max(1),
                    "ColorSpace" => Object::name("DeviceGray"),
                    "BitsPerComponent" => 8_i64,
                };
                if flags & 0x80 != 0 {
                    sm_dict.set(
                        "Matte",
                        vec![
                            Object::from(0.2_f64),
                            Object::from(0.5_f64),
                            Object::from(0.8_f64),
                        ],
                    );
                }
                store.objects.insert(
                    ObjectId::new(3, 0),
                    Object::from(Stream::new(sm_dict, payload[..mid].to_vec())),
                );
                d.set("SMask", ObjectId::new(3, 0));
            }
            2 => {
                let mask_dict = dict! {
                    "Subtype" => Object::name("Image"),
                    "Width" => (w / 2).max(1),
                    "Height" => (h / 2).max(1),
                    "ImageMask" => true,
                    "BitsPerComponent" => 1_i64,
                };
                store.objects.insert(
                    ObjectId::new(4, 0),
                    Object::from(Stream::new(mask_dict, payload[..mid].to_vec())),
                );
                d.set("Mask", ObjectId::new(4, 0));
            }
            3 => {
                d.set(
                    "Mask",
                    vec![
                        Object::from(0_i64),
                        Object::from(2_i64),
                        Object::from(0_i64),
                        Object::from(2_i64),
                        Object::from(0_i64),
                        Object::from(2_i64),
                        Object::from(0_i64),
                        Object::from(2_i64),
                    ],
                );
            }
            _ => {}
        }
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

        let mut faults_raw: Vec<(Place, Fault)> = Vec::new();
        let raw_silent = decode_xobject_raw(&store, id, None, &opts, &lim);
        let raw_loud = decode_xobject_raw_with(&store, id, None, &opts, &lim, &mut faults_raw);
        assert_eq!(raw_silent.is_ok(), raw_loud.is_ok());

        let mut faults_rgba: Vec<(Place, Fault)> = Vec::new();
        let rgba_silent = decode_xobject_with(&store, id, None, &opts, &lim, &mut ());
        let rgba_loud = decode_xobject_with(&store, id, None, &opts, &lim, &mut faults_rgba);
        assert_eq!(rgba_silent, rgba_loud);

        if let (Ok(mut raw), Ok(rgba)) = (raw_silent, rgba_silent) {
            let fill = [0_u16; 16];
            let _ = raw.fill_rect_raw(0, 0, 2, 2, &fill);
            let dec_fill = [0.5_f32; 16];
            let _ = raw.fill_rect_decoded(1, 1, 2, 2, &dec_fill);
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
                let _ = rgba.to_png().unwrap();
            }
        }
    }

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

/// Walks indirect objects reachable from `trailer` in any `Resolve + Streams` provider, decoding
/// up to 32 Image XObjects under both `Rules::Pdfium` and `Rules::Iso` and checking sink parity.
pub fn over<'o, O>(objects: O, trailer: &'o Dict)
where
    O: Resolve<'o> + Streams + Copy,
{
    let lim = tight_limits();
    let mut stack: Vec<ObjectId> = trailer.iter().flat_map(|(_, v)| v.references()).collect();
    let mut visited = std::collections::BTreeSet::new();
    let mut images_checked = 0_usize;

    while let Some(id) = stack.pop() {
        if visited.len() >= 128 || !visited.insert(id) {
            continue;
        }
        let Ok(Some(obj)) = objects.try_resolve(id) else {
            continue;
        };
        if let Value::Stream(stream) = obj.value()
            && stream.dict.get(b"Subtype").and_then(Object::as_name) == Some(b"Image")
        {
            for rules in [Rules::Pdfium, Rules::Iso] {
                let opts = DecodeOptions {
                    color: pdf_color::EvalOptions::from_spaces(rules, None),
                    stencil_color: [0, 0, 0, 255],
                    apply_masks: true,
                };
                let mut faults: Vec<(Place, Fault)> = Vec::new();
                let silent = decode_xobject_with(objects, id, None, &opts, &lim, &mut ());
                let loud = decode_xobject_with(objects, id, None, &opts, &lim, &mut faults);
                assert_eq!(silent, loud);
            }
            images_checked += 1;
            if images_checked >= 32 {
                break;
            }
        }
        stack.extend(obj.references().take(32));
    }
}

/// Returns deterministic seed inputs covering raw, stencil, DCT, CCITT Group 4, JBIG2, JPX,
/// inline, and soft-masked streams.
#[must_use]
pub fn seeds() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    // 1. Raw RGB 4x4 with inverted /Decode
    let mut s0 = vec![0, 4, 4, 3, 0x11];
    s0.extend((0..48_u8).map(|i| i.wrapping_mul(5)));
    out.push(s0);

    // 2. Stencil 8x4
    out.push(vec![1, 8, 4, 0, 1, 0xAA, 0x55, 0xF0, 0x0F]);

    // 3. Synthetic RGB JPEG 4x4
    let jpeg_stream = encode_jpeg(&[120_u8; 48], 4, 4, JpegColorSpace::Rgb, 85).unwrap();
    let mut s2 = vec![2, 4, 4, 3, 0];
    s2.extend_from_slice(jpeg_stream.data.bytes().unwrap());
    out.push(s2);

    // 4. CCITT Group 4 (`K = -1`) 8x2 with EOFB (`000000000001000000000001`)
    let mut s3 = vec![3, 8, 2, 0, 0];
    s3.extend_from_slice(&[0x00, 0x10, 0x01, 0x00]);
    out.push(s3);

    // 5. Minimal JBIG2 embedded generic region header stub
    let mut s4 = vec![4, 8, 2, 0, 0];
    s4.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x00, 0x30, 0x00, 0x01, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x00, 0x00, 0x02,
    ]);
    out.push(s4);

    // 6. Minimal JPX codestream SOC + SIZ header stub
    let mut s5 = vec![5, 4, 4, 3, 0];
    s5.extend_from_slice(&[
        0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00,
        0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x07, 0x01, 0x01,
        0x07, 0x01, 0x01, 0x07, 0x01, 0x01, 0xFF, 0xD9,
    ]);
    out.push(s5);

    // 7. Raw RGB 4x4 with `/SMask` + `/Matte` (`flags = 0xA1`)
    let mut s_smask = vec![0, 4, 4, 3, 0xA1];
    s_smask.extend((0..64_u8).map(|i| i.wrapping_mul(7)));
    out.push(s_smask);

    // 8. Inline ASCIIHex grayscale image
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
    fn seeds_mutations_and_random_inputs_keep_every_promise() {
        for seed in seeds() {
            images(&seed);
            for idx in 0..seed.len().min(32) {
                let mut mutated = seed.clone();
                mutated[idx] ^= 0x5A;
                images(&mutated);
            }
        }
        // Deterministic pseudo-random inputs + `over` pass on a minimal in-memory PDF.
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for len in [8_usize, 24, 64, 160] {
            let mut buf = vec![0_u8; len];
            for b in &mut buf {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *b = (state >> 24) as u8;
            }
            images(&buf);
        }

        let pdf_bytes = pdf_fixtures::fixture::file(
            "1.7",
            &[
                (
                    1,
                    pdf_fixtures::fixture::Body::Object(Object::from(dict! {
                        "Type" => Object::name("Catalog"),
                        "Im" => Object::reference(2),
                    })),
                ),
                (
                    2,
                    pdf_fixtures::fixture::Body::Stream(
                        dict! {
                            "Type" => Object::name("XObject"),
                            "Subtype" => Object::name("Image"),
                            "Width" => 2,
                            "Height" => 2,
                            "ColorSpace" => Object::name("DeviceGray"),
                            "BitsPerComponent" => 8,
                        },
                        vec![0, 85, 170, 255],
                    ),
                ),
            ],
            dict! { "Root" => Object::reference(1) },
            pdf_fixtures::fixture::Xref::Table,
        );
        let reading =
            pdf_structure::Reading::open(&pdf_bytes, &pdf_structure::Options::default()).unwrap();
        over(&reading, reading.trailer().dict());
    }
}
