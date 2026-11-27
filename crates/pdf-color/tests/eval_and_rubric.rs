//! Integration tests for `pdf-color` §7.10 function evaluation, §8.6 color space conversion,
//! `/Decode` & lookup helpers, and optional `icc` (`moxcms`) profiles.

#[cfg(feature = "icc")]
use std::collections::BTreeMap;

use pdf_color::pdf_objects::{
    Object, ObjectId, Resolve, Rules, Stream, Streams, Unreadable, Why, WithStreams, dict,
    resolve_fn,
};
use pdf_color::{
    EvalOptions, Fault, Function, FunctionKind, Limits, LookupError, Paint, PreparedSpace, Space,
    Spaces,
};

fn empty_streams<'o>() -> impl Resolve<'o> + Streams + Copy {
    WithStreams::new(
        resolve_fn(|_: ObjectId| -> Option<&'o Object> { None }),
        |id: ObjectId, _: usize| Err(Unreadable::new(id, Why::Damaged, "no stream")),
    )
}

#[test]
fn type0_sampled_functions_unpack_all_bit_depths_and_interpolate() {
    // 1D 8-bps sampled ramp `[0.0, 0.5, 1.0]` -> `[1.0, 0.2, 0.0]` and `[0.0, 0.6, 1.0]`.
    let dict8 = dict! {
        "FunctionType" => 0,
        "Domain" => vec![Object::from(0.0), Object::from(1.0)],
        "Range" => vec![Object::from(0.0), Object::from(1.0), Object::from(0.0), Object::from(1.0)],
        "Size" => vec![Object::from(3)],
        "BitsPerSample" => 8,
    };
    let stream8 = Object::from(Stream::new(dict8, vec![255, 0, 51, 153, 0, 255]));
    let mut faults = Vec::new();
    let f8 = Function::read(
        empty_streams(),
        &stream8,
        Rules::Iso,
        &Limits::DEFAULT,
        &mut faults,
    )
    .unwrap()
    .unwrap();
    assert!(faults.is_empty(), "{faults:?}");
    assert_eq!(f8.kind(), FunctionKind::Sampled);
    assert_eq!(f8.inputs(), 1);
    assert_eq!(f8.outputs(), 2);

    let mid = f8.eval_vec(&[0.25]).unwrap();
    assert!(
        (mid[0] - 0.6).abs() < 0.01 && (mid[1] - 0.3).abs() < 0.01,
        "{mid:?}"
    );

    // 2D bilinear 4-bps sampled function (`Size [2 2]`, 1 output):
    // Grid corners (x, y): (0,0)->0/15, (1,0)->5/15, (0,1)->10/15, (1,1)->15/15.
    // Packed 4-bit MSB-first: `0x05, 0xAF`.
    let dict4 = dict! {
        "FunctionType" => 0,
        "Domain" => vec![Object::from(0.0), Object::from(1.0), Object::from(0.0), Object::from(1.0)],
        "Range" => vec![Object::from(0.0), Object::from(1.0)],
        "Size" => vec![Object::from(2), Object::from(2)],
        "BitsPerSample" => 4,
    };
    let stream4 = Object::from(Stream::new(dict4, vec![0x05, 0xAF]));
    let f4 = Function::read(
        empty_streams(),
        &stream4,
        Rules::Iso,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    let center = f4.eval_vec(&[0.5, 0.5]).unwrap();
    assert!((center[0] - 0.5).abs() < 1e-5, "{center:?}");

    // 16-bps and 1-bps unpacking checks.
    for (bps, bytes, want_hi) in [
        (1, vec![0b0100_0000], 1.0_f32),
        (2, vec![0b0011_0000], 1.0_f32),
        (12, vec![0x00, 0x0F, 0xFF], 1.0_f32),
        (16, vec![0x00, 0x00, 0xFF, 0xFF], 1.0_f32),
        (24, vec![0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF], 1.0_f32),
        (32, vec![0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF], 1.0_f32),
    ] {
        let d = dict! {
            "FunctionType" => 0,
            "Domain" => vec![Object::from(0), Object::from(1)],
            "Range" => vec![Object::from(0), Object::from(1)],
            "Size" => vec![Object::from(2)],
            "BitsPerSample" => bps,
        };
        let s = Object::from(Stream::new(d, bytes));
        let func = Function::read(empty_streams(), &s, Rules::Iso, &Limits::DEFAULT, &mut ())
            .unwrap()
            .unwrap();
        assert_eq!(func.eval_vec(&[0.0]), Some(vec![0.0]));
        assert!((func.eval_vec(&[1.0]).unwrap()[0] - want_hi).abs() < 1e-6);
    }

    // Short stream reports `Fault::SampleDataLength` and returns `None`.
    let short_dict = dict! {
        "FunctionType" => 0,
        "Domain" => vec![Object::from(0), Object::from(1)],
        "Range" => vec![Object::from(0), Object::from(1)],
        "Size" => vec![Object::from(4)],
        "BitsPerSample" => 8,
    };
    let short_stream = Object::from(Stream::new(short_dict, vec![1, 2]));
    let mut faults = Vec::new();
    assert!(
        Function::read(
            empty_streams(),
            &short_stream,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut faults
        )
        .unwrap()
        .is_none()
    );
    assert!(faults.iter().any(|(_, f)| *f == Fault::SampleDataLength));
}

#[test]
fn type2_and_type3_exponential_and_stitching_functions() {
    let f_low = Object::from(dict! {
        "FunctionType" => 2,
        "Domain" => vec![Object::from(0.0), Object::from(1.0)],
        "C0" => vec![Object::from(0.0), Object::from(1.0)],
        "C1" => vec![Object::from(0.5), Object::from(0.5)],
        "N" => 1.0,
    });
    let f_high = Object::from(dict! {
        "FunctionType" => 2,
        "Domain" => vec![Object::from(0.0), Object::from(1.0)],
        "C0" => vec![Object::from(0.5), Object::from(0.5)],
        "C1" => vec![Object::from(1.0), Object::from(0.0)],
        "N" => 2.0,
    });
    let stitch = Object::from(dict! {
        "FunctionType" => 3,
        "Domain" => vec![Object::from(0.0), Object::from(1.0)],
        "Functions" => vec![f_low, f_high],
        "Bounds" => vec![Object::from(0.5)],
        "Encode" => vec![
            Object::from(0.0), Object::from(1.0),
            Object::from(0.0), Object::from(1.0),
        ],
    });

    let mut faults = Vec::new();
    let f = Function::read(
        empty_streams(),
        &stitch,
        Rules::Iso,
        &Limits::DEFAULT,
        &mut faults,
    )
    .unwrap()
    .unwrap();
    assert!(faults.is_empty(), "{faults:?}");
    assert_eq!(f.kind(), FunctionKind::Stitching);
    assert_eq!(f.eval_vec(&[0.25]), Some(vec![0.25, 0.75]));
    // At boundary x == 0.5, selects second subfunction at encoded 0.0 -> `[0.5, 0.5]`.
    assert_eq!(f.eval_vec(&[0.5]), Some(vec![0.5, 0.5]));
    // At x == 0.75, encoded = 0.5 in `f_high` (`N = 2.0` -> `0.5^2 = 0.25`) -> `0.5 + 0.25 * 0.5 = 0.625`.
    let hi = f.eval_vec(&[0.75]).unwrap();
    assert!(
        (hi[0] - 0.625).abs() < 1e-6 && (hi[1] - 0.375).abs() < 1e-6,
        "{hi:?}"
    );
}

#[test]
fn type4_postscript_calculator_covers_table_42_and_enforces_caps() {
    let eval_ps = |code: &[u8], domain: &[(f32, f32)], range: &[(f32, f32)], x: &[f32]| {
        let dom: Vec<Object> = domain
            .iter()
            .flat_map(|&(a, b)| [Object::from(f64::from(a)), Object::from(f64::from(b))])
            .collect();
        let rng: Vec<Object> = range
            .iter()
            .flat_map(|&(a, b)| [Object::from(f64::from(a)), Object::from(f64::from(b))])
            .collect();
        let d = dict! {
            "FunctionType" => 4,
            "Domain" => dom,
            "Range" => rng,
        };
        let s = Object::from(Stream::new(d, code.to_vec()));
        let mut faults = Vec::new();
        let f = Function::read(
            empty_streams(),
            &s,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap()
        .unwrap();
        assert!(faults.is_empty(), "{faults:?}");
        f.eval_vec(x)
    };

    // Trig in degrees (`sin(90) = 1`, `cos(180) = -1`, `-1 0 atan = 270`).
    let trig = eval_ps(
        b"{ % comment\n 90 sin 180 cos add -1 0 atan }",
        &[(0.0, 1.0)],
        &[(-10.0, 10.0), (-10.0, 10.0), (0.0, 360.0)],
        &[0.5],
    )
    .unwrap();
    assert!((trig[0] - 0.5).abs() < 1e-5);
    assert!(trig[1].abs() < 1e-5);
    assert!((trig[2] - 270.0).abs() < 1e-4);

    // Conditionals, stack ops (`dup`, `exch`, `index`, `roll`, `copy`), bitwise, radix ints.
    let stack_cond = eval_ps(
        b"{ dup 0.5 gt { pop 16#FF -4 bitshift cvr 15.0 div } { 1.0 exch sub } ifelse }",
        &[(0.0, 1.0)],
        &[(0.0, 1.0)],
        &[0.8],
    )
    .unwrap();
    assert!((stack_cond[0] - 1.0).abs() < 1e-5, "{stack_cond:?}");

    let stack_roll = eval_ps(
        b"{ 10 20 30 3 1 roll pop exch pop add }",
        &[(0.0, 100.0)],
        &[(0.0, 100.0)],
        &[5.0],
    )
    .unwrap();
    // Initial `[5]`, push `10 20 30` -> `[5, 10, 20, 30]`, `3 1 roll` -> `[5, 30, 10, 20]`,
    // `pop` -> `[5, 30, 10]`, `exch pop` -> `[5, 10]`, `add` -> `[15]`.
    assert_eq!(stack_roll, vec![15.0]);

    // Division by zero returns `None` cleanly without panicking.
    assert_eq!(
        eval_ps(b"{ 0 div }", &[(0.0, 1.0)], &[(0.0, 1.0)], &[0.5]),
        None
    );

    // Invalid operator is rejected at parse time with `Fault::PostScriptSyntax`.
    let bad = Object::from(Stream::new(
        dict! {
            "FunctionType" => 4,
            "Domain" => vec![Object::from(0), Object::from(1)],
            "Range" => vec![Object::from(0), Object::from(1)],
        },
        b"{ def }".to_vec(),
    ));
    let mut faults = Vec::new();
    assert!(
        Function::read(
            empty_streams(),
            &bad,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut faults
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(faults[0].1, Fault::PostScriptSyntax);
}

#[test]
fn default_decode_and_lookup_extraction_helpers() {
    let lab = Object::from(vec![
        Object::name("Lab"),
        Object::from(dict! {
            "WhitePoint" => vec![Object::from(0.9505), Object::from(1.0), Object::from(1.089)],
            "Range" => vec![Object::from(-80.0), Object::from(80.0), Object::from(-50.0), Object::from(90.0)],
        }),
    ]);
    let indexed = Object::from(vec![
        Object::name("Indexed"),
        Object::name("DeviceRGB"),
        Object::from(1),
        Object::bytes(*b"\x10\x20\x30\x40\x50\x60"),
    ]);

    let lab_sp = Space::read(empty_streams(), &lab, &Limits::DEFAULT, &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(
        lab_sp
            .default_decode(empty_streams(), 8, &Limits::DEFAULT)
            .unwrap(),
        Some(vec![(0.0, 100.0), (-80.0, 80.0), (-50.0, 90.0)])
    );

    let store_idx = empty_streams();
    let idx_sp = Space::read(store_idx, &indexed, &Limits::DEFAULT, &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(
        idx_sp
            .default_decode(store_idx, 4, &Limits::DEFAULT)
            .unwrap(),
        Some(vec![(0.0, 15.0)])
    );
    if let Space::Indexed(idx) = &idx_sp {
        assert_eq!(
            idx.lookup_bytes(&store_idx, 64).unwrap().as_deref(),
            Some(&b"\x10\x20\x30\x40\x50\x60"[..])
        );
        assert_eq!(idx.lookup_bytes(&store_idx, 3), Err(LookupError::Limit));
    } else {
        panic!("expected Space::Indexed");
    }
}

#[test]
fn iso_vs_pdfium_color_policies_and_special_colorants() {
    // 1. CalGray with Gamma 1.0: linear 0.5 -> sRGB ~188 under ISO, vs direct 128 under PDFium.
    let cal_gray = Object::from(vec![
        Object::name("CalGray"),
        Object::from(dict! {
            "WhitePoint" => vec![Object::from(0.95047), Object::from(1.0), Object::from(1.08883)],
            "Gamma" => 1.0,
        }),
    ]);
    let iso_cg = PreparedSpace::prepare(
        empty_streams(),
        &cal_gray,
        &EvalOptions::ISO,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    let pdfium_cg = PreparedSpace::prepare(
        empty_streams(),
        &cal_gray,
        &EvalOptions::PDFIUM,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert_eq!(iso_cg.eval_srgb8(&[0.5]), Some([188, 188, 188]));
    assert_eq!(pdfium_cg.eval_srgb8(&[0.5]), Some([128, 128, 128]));

    // 2. `/DefaultGray` substitution in `Rules::Iso` (`EvalOptions::from_spaces`).
    let resources = Object::from(dict! {
        "ColorSpace" => dict! {
            "DefaultGray" => cal_gray.clone(),
        },
    });
    let spaces = Spaces::read(empty_streams(), Some(&resources)).unwrap();
    let iso_with_defaults = EvalOptions::from_spaces(Rules::Iso, spaces);
    let dev_gray = Object::name("DeviceGray");
    let prep_def_gray = PreparedSpace::prepare(
        empty_streams(),
        &dev_gray,
        &iso_with_defaults,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert_eq!(prep_def_gray.eval_srgb8(&[0.5]), Some([188, 188, 188]));
    assert_eq!(
        prep_def_gray
            .eval_with_rules(&[0.5], Rules::Pdfium)
            .unwrap()
            .srgb8(),
        Some([128, 128, 128])
    );

    // 3. `Separation /None` returns `Paint::None`; `Separation /All` returns `1 - t` under ISO vs
    //    tint transform under PDFium (and `eval_with_rules` switches cleanly between both!).
    let tint_red = Object::from(dict! {
        "FunctionType" => 2,
        "Domain" => vec![Object::from(0.0), Object::from(1.0)],
        "N" => 1.0,
        "C0" => vec![Object::from(1.0), Object::from(1.0), Object::from(1.0)],
        "C1" => vec![Object::from(1.0), Object::from(0.0), Object::from(0.0)],
    });
    let sep_none = Object::from(vec![
        Object::name("Separation"),
        Object::name("None"),
        Object::name("DeviceRGB"),
        tint_red.clone(),
    ]);
    let prep_none = PreparedSpace::prepare(
        empty_streams(),
        &sep_none,
        &EvalOptions::PDFIUM,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert_eq!(prep_none.eval(&[0.7]), Some(Paint::None));
    assert_eq!(
        prep_none.eval(&[0.7]).unwrap().srgb8_over([51, 102, 153]),
        [51, 102, 153]
    );

    let sep_all = Object::from(vec![
        Object::name("Separation"),
        Object::name("All"),
        Object::name("DeviceRGB"),
        tint_red,
    ]);
    let prep_all_iso = PreparedSpace::prepare(
        empty_streams(),
        &sep_all,
        &EvalOptions::ISO,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    let prep_all_pdfium = PreparedSpace::prepare(
        empty_streams(),
        &sep_all,
        &EvalOptions::PDFIUM,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert_eq!(prep_all_iso.eval_srgb8(&[0.7]), Some([77, 77, 77]));
    assert_eq!(prep_all_pdfium.eval_srgb8(&[0.7]), Some([255, 77, 77]));
    assert_eq!(
        prep_all_iso
            .eval_with_rules(&[0.7], Rules::Pdfium)
            .unwrap()
            .srgb8(),
        Some([255, 77, 77])
    );
    assert_eq!(
        prep_all_pdfium
            .eval_with_rules(&[0.7], Rules::Iso)
            .unwrap()
            .srgb8(),
        Some([77, 77, 77])
    );
}

#[cfg(feature = "icc")]
#[test]
fn icc_profiles_convert_via_moxcms_and_corrupt_profiles_fall_back_with_fault() {
    // Synthesize genuine Display P3 and Gray-gamma-1.8 ICC profiles via `moxcms`.
    let p3_bytes = moxcms::ColorProfile::new_display_p3().encode().unwrap();
    let gray_bytes = moxcms::ColorProfile::new_gray_with_gamma(1.8)
        .encode()
        .unwrap();

    let mut objs = BTreeMap::new();
    objs.insert(
        ObjectId::new(1, 0),
        Object::stream(dict! { "N" => 3, "Alternate" => Object::name("DeviceRGB") }),
    );
    objs.insert(
        ObjectId::new(2, 0),
        Object::stream(dict! { "N" => 1, "Alternate" => Object::name("DeviceGray") }),
    );
    objs.insert(
        ObjectId::new(3, 0),
        Object::stream(dict! { "N" => 3, "Alternate" => Object::name("DeviceRGB") }),
    );

    let store = WithStreams::new(&objs, |id: ObjectId, _: usize| match id.number {
        1 => Ok(p3_bytes.clone()),
        2 => Ok(gray_bytes.clone()),
        3 => Ok(b"NOT_AN_ICC_PROFILE".to_vec()),
        _ => Err(Unreadable::new(id, Why::Damaged, "missing")),
    });

    // Saturated Display P3 green `[0.0, 1.0, 0.0]` maps to clipped/adapted sRGB green (noticeably
    // different from `[0, 255, 0]` or matching Display P3 -> sRGB primaries `[0, 255, 0]` / red `[1, 0, 0]`).
    // Display P3 `[0.8, 0.2, 0.1]` has wider red chroma than sRGB `[204, 51, 26]`.
    let icc_p3 = Object::from(vec![Object::name("ICCBased"), Object::reference(1)]);
    let mut faults = Vec::new();
    let prep_p3 = PreparedSpace::prepare(
        store,
        &icc_p3,
        &EvalOptions::ISO,
        &Limits::DEFAULT,
        &mut faults,
    )
    .unwrap()
    .unwrap();
    assert!(faults.is_empty(), "{faults:?}");
    let p3_rgb = prep_p3.eval_srgb8(&[0.8, 0.2, 0.1]).unwrap();
    assert!(
        p3_rgb[0] > 210 && p3_rgb[1] < 45,
        "Display P3 wider red gamut: {p3_rgb:?}"
    );

    // Gray gamma 1.8 at 0.5 (`0.5^1.8` linear -> sRGB ~145, distinct from DeviceGray 128).
    let icc_gray = Object::from(vec![Object::name("ICCBased"), Object::reference(2)]);
    let prep_gray = PreparedSpace::prepare(
        store,
        &icc_gray,
        &EvalOptions::ISO,
        &Limits::DEFAULT,
        &mut faults,
    )
    .unwrap()
    .unwrap();
    assert!(faults.is_empty(), "{faults:?}");
    let gray_rgb = prep_gray.eval_srgb8(&[0.5]).unwrap();
    assert!((i16::from(gray_rgb[0]) - 145).abs() <= 2, "{gray_rgb:?}");

    // Corrupt ICC profile emits `Fault::IccProfile` and falls back to `/Alternate` (`DeviceRGB`).
    let icc_bad = Object::from(vec![Object::name("ICCBased"), Object::reference(3)]);
    let prep_bad = PreparedSpace::prepare(
        store,
        &icc_bad,
        &EvalOptions::ISO,
        &Limits::DEFAULT,
        &mut faults,
    )
    .unwrap()
    .unwrap();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].1, Fault::IccProfile);
    assert_eq!(prep_bad.eval_srgb8(&[0.2, 0.4, 0.8]), Some([51, 102, 204]));

    // `/OutputIntents` destination profile overrides unprofiled `DeviceRGB` under `Rules::Iso`,
    // while `eval_with_rules(..., Rules::Pdfium)` evaluates unprofiled `DeviceRGB`.
    let mut opts_oi = EvalOptions::ISO;
    opts_oi.output_intent_profile = Some(&p3_bytes);
    let dev_rgb = Object::name("DeviceRGB");
    let prep_oi = PreparedSpace::prepare(store, &dev_rgb, &opts_oi, &Limits::DEFAULT, &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(prep_oi.eval_srgb8(&[0.8, 0.2, 0.1]), Some(p3_rgb));
    assert_eq!(
        prep_oi
            .eval_with_rules(&[0.8, 0.2, 0.1], Rules::Pdfium)
            .unwrap()
            .srgb8(),
        Some([204, 51, 26])
    );

    // Direct `from_icc_bytes` and batched `eval_pixels_srgb8` match per-pixel evaluation.
    let from_bytes =
        PreparedSpace::from_icc_bytes(&p3_bytes, 3, &EvalOptions::ISO, &Limits::DEFAULT, &mut ())
            .unwrap();
    assert_eq!(from_bytes.eval_srgb8(&[0.8, 0.2, 0.1]), Some(p3_rgb));
    let mut batch_out = [[0_u8; 3]; 2];
    assert!(from_bytes.eval_pixels_srgb8(&[0.8, 0.2, 0.1, 0.0, 0.0, 0.0], &mut batch_out, [0; 3]));
    assert_eq!(batch_out, [p3_rgb, [0, 0, 0]]);
}

#[test]
fn icc_based_at_space_depth_limit_never_panics_and_stitching_shares_sample_budget() {
    use std::collections::BTreeMap;

    // 1. `ICCBased` with `limits.space_depth = 0` must gracefully return `Ok(None)`,
    //    and an `ICCBased` space at `depth == limits.space_depth` (e.g. via `/DefaultRGB`
    //    or a 70-deep `/Alternate` chain) must fall back to the unprofiled device space
    //    without panicking!
    let mut store: BTreeMap<ObjectId, Object> = BTreeMap::new();
    store.insert(
        ObjectId::new(1, 0),
        Object::from(Stream::new(
            dict! { "N" => 3, "Alternate" => Object::name("DeviceRGB") },
            b"not-a-valid-icc-profile".to_vec(),
        )),
    );
    let icc_root = Object::from(vec![Object::name("ICCBased"), Object::reference(1)]);

    let mut zero_depth = Limits::DEFAULT;
    zero_depth.space_depth = 0;
    let store_streams = WithStreams::new(&store, |id, limit| {
        let Some(s) = store.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "missing"));
        };
        let bytes = s.data.bytes().unwrap();
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(bytes.to_vec())
    });
    let prep_zero = PreparedSpace::prepare(
        store_streams,
        &icc_root,
        &EvalOptions::ISO,
        &zero_depth,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert_eq!(prep_zero.eval_srgb8(&[1.0, 0.0, 0.0]), Some([255, 0, 0]));

    // `/DefaultRGB` pointing to `ICCBased` with `space_depth = 1` reaches `prepare_icc`
    // at `depth == 1 == limits.space_depth`, where `depth + 1 > space_depth`: must not panic!
    let mut depth_one = Limits::DEFAULT;
    depth_one.space_depth = 1;
    let mut def_opts = EvalOptions::ISO;
    def_opts.default_rgb = Some(&icc_root);
    let prep_def = PreparedSpace::prepare(
        store_streams,
        &Object::name("DeviceRGB"),
        &def_opts,
        &depth_one,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert_eq!(prep_def.eval_srgb8(&[1.0, 0.0, 0.0]), Some([255, 0, 0]));

    // Build a 70-deep chain of `ICCBased` `/Alternate`s exceeding `Limits::DEFAULT.space_depth` (64).
    let mut chain_store: BTreeMap<ObjectId, Object> = BTreeMap::new();
    chain_store.insert(
        ObjectId::new(100, 0),
        Object::from(Stream::new(
            dict! { "N" => 3, "Alternate" => Object::name("DeviceRGB") },
            b"bad-icc".to_vec(),
        )),
    );
    for idx in (1..70_u32).rev() {
        let next_ref = Object::from(vec![
            Object::name("ICCBased"),
            Object::from(ObjectId::new(idx + 1 + 99, 0)),
        ]);
        chain_store.insert(
            ObjectId::new(idx + 99, 0),
            Object::from(Stream::new(
                dict! { "N" => 3, "Alternate" => next_ref },
                b"bad-icc".to_vec(),
            )),
        );
    }
    let chain_head = Object::from(vec![
        Object::name("ICCBased"),
        Object::from(ObjectId::new(100, 0)),
    ]);
    let chain_streams = WithStreams::new(&chain_store, |id, limit| {
        let Some(s) = chain_store.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "missing"));
        };
        let bytes = s.data.bytes().unwrap();
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(bytes.to_vec())
    });
    let prep_chain = PreparedSpace::prepare(
        chain_streams,
        &chain_head,
        &EvalOptions::ISO,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    // `Space::read` or `PreparedSpace::prepare` caps the deep chain cleanly without panicking;
    // if prepared, evaluating must also succeed without panicking.
    if let Some(p) = prep_chain {
        assert_eq!(p.eval_srgb8(&[0.0, 1.0, 0.0]), Some([0, 255, 0]));
    }

    // 2. Stitching (Type 3) function referencing a sampled (Type 0) stream 100 times
    //    shares a single reader-wide `sample_values` budget instead of multiplying 100x.
    let mut fn_store: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let sub_samples = 1_024_usize;
    fn_store.insert(
        ObjectId::new(10, 0),
        Object::from(Stream::new(
            dict! {
                "FunctionType" => 0,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "Range" => vec![Object::from(0.0), Object::from(1.0)],
                "Size" => vec![Object::from(sub_samples as i64)],
                "BitsPerSample" => 8,
            },
            vec![128_u8; sub_samples],
        )),
    );
    let count = 100_usize;
    let funcs: Vec<Object> = (0..count).map(|_| Object::reference(10)).collect();
    let bounds: Vec<Object> = (1..count)
        .map(|i| Object::from((i as f64) / (count as f64)))
        .collect();
    let mut encode = Vec::with_capacity(count * 2);
    for _ in 0..count {
        encode.push(Object::from(0.0));
        encode.push(Object::from(1.0));
    }
    let stitch_dict = Object::from(dict! {
        "FunctionType" => 3,
        "Domain" => vec![Object::from(0.0), Object::from(1.0)],
        "Functions" => funcs,
        "Bounds" => bounds,
        "Encode" => encode,
    });
    let fn_streams = WithStreams::new(&fn_store, |id, limit| {
        let Some(s) = fn_store.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "missing"));
        };
        let bytes = s.data.bytes().unwrap();
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(bytes.to_vec())
    });

    let mut capped_fn_limits = Limits::DEFAULT;
    // Allow 10 copies (`10_240` samples), far below 100 copies (`102_400` samples).
    capped_fn_limits.sample_values = 10_240;
    let mut fn_faults = Vec::new();
    let read_res = Function::read(
        fn_streams,
        &stitch_dict,
        Rules::Iso,
        &capped_fn_limits,
        &mut fn_faults,
    )
    .unwrap();
    assert!(read_res.is_none());
    assert!(
        fn_faults.iter().any(|&(_, f)| f == Fault::TooManySamples),
        "expected TooManySamples fault on Type 3 sample amplification: {fn_faults:?}"
    );
}

#[test]
fn tint_surplus_outputs_default_space_families_tiny_domains_and_ps_edges() {
    use std::collections::BTreeMap;

    // 1. Tint transform with MORE outputs than the alternate space (`[/Separation /Spot /DeviceRGB f]`
    //    where `f` has 5 outputs) evaluates using the first 3 outputs rather than failing!
    let mut store: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let tint_5_out = Object::from(dict! {
        "FunctionType" => 2,
        "Domain" => vec![Object::from(0.0), Object::from(1.0)],
        "Range" => vec![
            Object::from(0.0), Object::from(1.0),
            Object::from(0.0), Object::from(1.0),
            Object::from(0.0), Object::from(1.0),
            Object::from(0.0), Object::from(1.0),
            Object::from(0.0), Object::from(1.0),
        ],
        "C0" => vec![
            Object::from(0.0), Object::from(0.0), Object::from(0.0),
            Object::from(0.9), Object::from(0.9),
        ],
        "C1" => vec![
            Object::from(1.0), Object::from(0.5), Object::from(0.0),
            Object::from(0.2), Object::from(0.3),
        ],
        "N" => 1,
    });
    let sep = Object::from(vec![
        Object::name("Separation"),
        Object::name("SpotOrange"),
        Object::name("DeviceRGB"),
        tint_5_out,
    ]);
    let streams = WithStreams::new(&store, |id, _| {
        Err(Unreadable::new(id, Why::Damaged, "none"))
    });
    let prep_sep =
        PreparedSpace::prepare(streams, &sep, &EvalOptions::ISO, &Limits::DEFAULT, &mut ())
            .unwrap()
            .unwrap();
    assert_eq!(prep_sep.eval_srgb8(&[1.0]), Some([255, 128, 0]));

    // 2. ISO 32000-2 §8.6.5.6 `/Default*` substitution:
    //    - `Lab` is FORBIDDEN as `/DefaultRGB` (falls back to unprofiled `DeviceRGB`).
    //    - `Separation` IS ALLOWED as `/DefaultGray` (1 component).
    let lab_space = Object::from(vec![
        Object::name("Lab"),
        Object::from(dict! {
            "WhitePoint" => vec![Object::from(0.9505), Object::from(1.0), Object::from(1.089)],
        }),
    ]);
    let mut opts_lab = EvalOptions::ISO;
    opts_lab.default_rgb = Some(&lab_space);
    let prep_rgb = PreparedSpace::prepare(
        streams,
        &Object::name("DeviceRGB"),
        &opts_lab,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    // Unprofiled DeviceRGB `[0.0, 1.0, 0.0]` -> `[0, 255, 0]` (whereas Lab `[0, 1, 0]` would be near black!).
    assert_eq!(prep_rgb.eval_srgb8(&[0.0, 1.0, 0.0]), Some([0, 255, 0]));

    let sep_gray = Object::from(vec![
        Object::name("Separation"),
        Object::name("BlackInk"),
        Object::name("DeviceRGB"),
        Object::from(dict! {
            "FunctionType" => 2,
            "Domain" => vec![Object::from(0.0), Object::from(1.0)],
            "C0" => vec![Object::from(0.0), Object::from(0.0), Object::from(1.0)],
            "C1" => vec![Object::from(1.0), Object::from(0.0), Object::from(0.0)],
            "N" => 1,
        }),
    ]);
    let mut opts_sep = EvalOptions::ISO;
    opts_sep.default_gray = Some(&sep_gray);
    let prep_def_gray = PreparedSpace::prepare(
        streams,
        &Object::name("DeviceGray"),
        &opts_sep,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert_eq!(prep_def_gray.eval_srgb8(&[1.0]), Some([255, 0, 0]));

    // 3. Tiny non-zero domain (`[0.0, 1e-8]`) and stitching boundaries (`x == Bounds[i]` and `x == Domain[1]`),
    //    plus indirect `ObjectId` stream decoding on Type 0 and Type 4 functions.
    store.insert(
        ObjectId::new(20, 0),
        Object::from(Stream::new(
            dict! {
                "FunctionType" => 0,
                "Domain" => vec![Object::from(0.0), Object::from(1.0e-8)],
                "Range" => vec![Object::from(0.0), Object::from(1.0)],
                "Size" => vec![Object::from(2)],
                "BitsPerSample" => 8,
            },
            vec![0_u8, 255_u8],
        )),
    );
    store.insert(
        ObjectId::new(21, 0),
        Object::from(Stream::new(
            dict! {
                "FunctionType" => 4,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "Range" => vec![Object::from(-1.0e38), Object::from(1.0e38)],
            },
            // Exercise `idiv` of `i32::MIN` by `-1`? Wait: test both valid int overflow promotion
            // (`2147483647 1 add`), negative `roll` (`10 20 30 3 -1 roll pop pop`), and `Bounds`!
            b"{ pop 2147483647 1 add 10 20 30 3 -1 roll pop pop add }".to_vec(),
        )),
    );
    store.insert(
        ObjectId::new(22, 0),
        Object::from(Stream::new(
            dict! {
                "FunctionType" => 4,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "Range" => vec![Object::from(-1.0e38), Object::from(1.0e38)],
            },
            // `i32::MIN` (-2147483648) `idiv` by `-1` never panics!
            b"{ pop -2147483647 1 sub -1 idiv }".to_vec(),
        )),
    );
    store.insert(
        ObjectId::new(23, 0),
        Object::from(Stream::new(
            dict! {
                "FunctionType" => 4,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "Range" => vec![Object::from(0.0), Object::from(1.0)],
            },
            // Runaway self-recursion through `dup true exch if` is cleanly stopped by caps!
            b"{ pop { dup true exch if } dup true exch if }".to_vec(),
        )),
    );
    let by_id_streams = WithStreams::new(&store, |id, limit| {
        let Some(s) = store.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "missing"));
        };
        let bytes = s.data.bytes().unwrap();
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(bytes.to_vec())
    });

    let tiny_fn = Function::read(
        by_id_streams,
        &Object::reference(20),
        Rules::Iso,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    let mid = tiny_fn.eval_vec(&[0.5e-8]).unwrap()[0];
    assert!(
        (mid - 0.5).abs() < 0.02,
        "tiny domain [0, 1e-8] must interpolate to ~0.5 at midpoint, got {mid}"
    );

    // Stitching at `x == Bounds[0]` (`0.5` selects subfunction 1 per ISO §7.10.4 `[Bounds[0], Domain[1]]`)
    // and at `x == Domain[1]` (`1.0`).
    let stitch_bounds = Object::from(dict! {
        "FunctionType" => 3,
        "Domain" => vec![Object::from(0.0), Object::from(1.0)],
        "Functions" => vec![
            Object::from(dict! {
                "FunctionType" => 2,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "C0" => vec![Object::from(0.1)],
                "C1" => vec![Object::from(0.2)],
                "N" => 1,
            }),
            Object::from(dict! {
                "FunctionType" => 2,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "C0" => vec![Object::from(0.7)],
                "C1" => vec![Object::from(0.9)],
                "N" => 1,
            }),
        ],
        "Bounds" => vec![Object::from(0.5)],
        "Encode" => vec![
            Object::from(0.0), Object::from(1.0),
            Object::from(0.0), Object::from(1.0),
        ],
    });
    let st_fn = Function::read(
        by_id_streams,
        &stitch_bounds,
        Rules::Iso,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert!((st_fn.eval_vec(&[0.5]).unwrap()[0] - 0.7).abs() < 1e-5);
    assert!((st_fn.eval_vec(&[1.0]).unwrap()[0] - 0.9).abs() < 1e-5);

    let ps_ok = Function::read(
        by_id_streams,
        &Object::reference(21),
        Rules::Iso,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    let ps_val = ps_ok.eval_vec(&[0.0]).unwrap()[0];
    assert!(ps_val > 2.0e9);

    let ps_min_idiv = Function::read(
        by_id_streams,
        &Object::reference(22),
        Rules::Iso,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    let _ = ps_min_idiv.eval_vec(&[0.0]);

    let ps_rec = Function::read(
        by_id_streams,
        &Object::reference(23),
        Rules::Iso,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert!(ps_rec.eval_vec(&[0.0]).is_none());

    // 4. `Rules::Pdfium` vs `Rules::Iso` on Type 4 PostScript float-stack coercion:
    //    Input `7.8` is pushed as a real; `{ 2 idiv }` and `{ 3.0 mod }` and `{ 1.0 { 10 } { 20 } ifelse }`
    //    fail strict Table 42 type checks under `Rules::Iso` and coerce truncated ints / truthy floats
    //    under `Rules::Pdfium` (including via `PreparedSpace::eval_with_rules`!).
    let ps_float_int = Object::from(Stream::new(
        dict! {
            "FunctionType" => 4,
            "Domain" => vec![Object::from(0.0), Object::from(10.0)],
            "Range" => vec![Object::from(0.0), Object::from(10.0)],
        },
        b"{ dup 2 idiv exch 3.0 mod add 1.0 { 0.5 add } { 9.0 } ifelse }".to_vec(),
    ));
    let fn_iso = Function::read(
        empty_streams(),
        &ps_float_int,
        Rules::Iso,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    let fn_pdfium = Function::read(
        empty_streams(),
        &ps_float_int,
        Rules::Pdfium,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    assert_eq!(fn_iso.eval_vec(&[7.8]), None);
    // `7.8`: `7 / 2 = 3`, `7 % 3 = 1`, `3 + 1 + 0.5 = 4.5`.
    assert_eq!(fn_pdfium.eval_vec(&[7.8]), Some(vec![4.5]));

    // `DeviceN [/None]` with a Type 4 tint that uses float `idiv`:
    let ps_tint = Object::from(Stream::new(
        dict! {
            "FunctionType" => 4,
            "Domain" => vec![Object::from(0.0), Object::from(1.0)],
            "Range" => vec![Object::from(0.0), Object::from(1.0)],
        },
        b"{ 10.0 mul 2 idiv 5.0 div }".to_vec(),
    ));
    let dn_none = Object::from(vec![
        Object::name("DeviceN"),
        Object::from(vec![Object::name("None")]),
        Object::name("DeviceGray"),
        ps_tint,
    ]);
    let prep_dn = PreparedSpace::prepare(
        empty_streams(),
        &dn_none,
        &EvalOptions::PDFIUM,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap()
    .unwrap();
    // Under PDFium: evaluates tint (`0.7 * 10 = 7.0`, `7 idiv 2 = 3`, `3 / 5 = 0.6` -> 153).
    assert_eq!(prep_dn.eval_srgb8(&[0.7]), Some([153, 153, 153]));
    // Overriding to `Rules::Iso` on the same `PreparedSpace` yields `Paint::None`!
    assert_eq!(
        prep_dn.eval_with_rules(&[0.7], Rules::Iso),
        Some(Paint::None)
    );
}
