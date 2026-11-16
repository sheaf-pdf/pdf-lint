//! In-process fuzzing entrypoints exercised by unit tests and `fuzz/fuzz_targets/gfx.rs`.
//!
//! Verifies on arbitrary byte inputs:
//! 1. Zero panics across `interpret`, `interpret_with`, and `encode`.
//! 2. Sink invariance: `interpret` (`&mut ()`) and `interpret_with` (`Vec`)
//!    produce identical `Result<Summary, Error>`.
//! 3. `Wants` invariance: `Wants::NONE` produces the exact same
//!    `Result<Summary, Error>` as `Wants::ALL`.
//! 4. Re-encode round-trip invariance: whenever `encode` succeeds, re-interpreting
//!    the encoded page and Form XObject streams reproduces the exact same
//!    non-`Op` geometry, text, clip, form, image, shading, and mark events.

use std::collections::BTreeMap;
use std::ops::ControlFlow;

use pdf_content_streams::{Concat, Join};
use pdf_fonts::Fonts;
use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{
    Dict, Object, ObjectId, Resolve, Stream, Streams, Unreadable, Value, Why, WithStreams,
};
use pdf_semantics::pages::{self, Inheritable};

use crate::encode::{Encoded, encode, encode_with};
use crate::geom::{Matrix, PageGeom, Rect};
use crate::interp::{Event, Forms, Kind, Opts, Wants, interpret, interpret_with};
use crate::limits::Limits;
use crate::path::Seg;

/// Built-in seed inputs exercising paths, text operators, Form XObjects, inline
/// images, and marked content for unit tests and `just fuzz gfx`.
pub const SEEDS: &[&[u8]] = &[
    b"",
    b"q 10 20 30 40 re f Q",
    b"BT /F1 12 Tf 10 20 Td (Hello) Tj 0 -14 TD (World) ' ET\xff<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>",
    b"0 0 50 50 re W n /Fm1 Do\xff<< /XObject << /Fm1 1 0 R >> >>\xff<< /Subtype /Form /BBox [0 0 40 40] >>\nstream\n0 0 20 20 re S",
    b"BI /W 1 /H 1 /BPC 8 /CS /G ID x EI",
    b"/Span << /MCID 0 /ActualText (Hi) >> BDC 1 w 1 J 5 5 m h S EMC",
];

/// Runs `interpret`, `interpret_with`, `Wants::NONE`, and `encode` round-trip
/// checks over an arbitrary byte slice split into page content, resources, and
/// indirect objects/streams.
pub fn check(data: &[u8]) {
    let mut parts = data.split(|&b| b == 0xFF);
    let page_bytes = parts.next().unwrap_or_default();
    let res_bytes = parts.next().unwrap_or_default();

    let mut parse_limits = pdf_objects::Limits::DEFAULT;
    parse_limits.nesting = 16;
    parse_limits.values = 512;

    let resources = Parser::at(res_bytes, 0)
        .rules(Rules::Pdfium)
        .limits(parse_limits)
        .value(&mut ())
        .ok();

    let mut map: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut stream_bytes: BTreeMap<ObjectId, Vec<u8>> = BTreeMap::new();
    for (idx, chunk) in parts.take(16).enumerate() {
        let id = ObjectId::new((idx + 1) as u32, 0);
        if let Some((dict_part, raw_stream)) = split_once_bytes(chunk, b"\nstream\n") {
            if let Ok(obj) = Parser::at(dict_part, 0)
                .rules(Rules::Pdfium)
                .limits(parse_limits)
                .value(&mut ())
                && let Value::Dict(dict) = obj.into_value()
            {
                stream_bytes.insert(id, raw_stream.to_vec());
                let s = Stream::new(dict, raw_stream.to_vec());
                map.insert(id, Object::from(s));
            }
        } else if let Ok(obj) = Parser::at(chunk, 0)
            .rules(Rules::Pdfium)
            .limits(parse_limits)
            .value(&mut ())
        {
            map.insert(id, obj);
        }
    }

    let limits = Limits {
        form_depth: 6,
        states: 32,
        operators: 4_096,
        glyphs: 4_096,
        segments: 4_096,
        marked: 32,
        interpreted: 64 << 10,
        decompressed: 128 << 10,
        ..Limits::DEFAULT
    };

    let streams = WithStreams::new(&map, |id: ObjectId, cap: usize| {
        match stream_bytes.get(&id) {
            Some(bytes) if bytes.len() <= cap => Ok(bytes.clone()),
            Some(_) => Err(Unreadable::new(id, Why::Limit, "past budget")),
            None => Err(Unreadable::new(id, Why::Damaged, "missing stream")),
        }
    });
    let concat = Concat::new(&[page_bytes]);
    let fonts = Fonts::default();

    let mut events_before = Vec::new();
    let res_all = interpret(
        streams,
        &concat,
        resources.as_ref(),
        &limits,
        &fonts,
        Opts::default(),
        |ev| {
            if let Some(snap) = snapshot(&ev) {
                events_before.push(snap);
            }
            ControlFlow::Continue(())
        },
    );

    let mut faults = Vec::new();
    let res_sink = interpret_with(
        streams,
        &concat,
        resources.as_ref(),
        &limits,
        &fonts,
        Opts::default(),
        &mut faults,
        |_| ControlFlow::Continue(()),
    );
    assert_eq!(res_all, res_sink);
    for (_, fault) in &faults {
        let _ = format!("{fault:?}");
    }

    let res_none = interpret(
        streams,
        &concat,
        resources.as_ref(),
        &limits,
        &fonts,
        Opts {
            forms: Forms::Descend,
            wants: Wants::NONE,
        },
        |_| ControlFlow::Continue(()),
    );
    assert_eq!(res_all, res_none);

    if res_all.is_ok()
        && let Ok(Encoded {
            content: enc_page,
            forms: enc_forms,
        }) = encode(streams, &concat, resources.as_ref(), &limits, &fonts)
    {
        let mut map2 = map.clone();
        let mut sb2 = stream_bytes.clone();
        for (fid, bytes) in enc_forms {
            let dict = map2
                .get(&fid)
                .and_then(Object::as_stream)
                .map_or_else(Dict::new, |s| s.dict.clone());
            sb2.insert(fid, bytes.clone());
            map2.insert(fid, Object::from(Stream::new(dict, bytes)));
        }
        let streams2 = WithStreams::new(&map2, |id: ObjectId, cap: usize| match sb2.get(&id) {
            Some(bytes) if bytes.len() <= cap => Ok(bytes.clone()),
            Some(_) => Err(Unreadable::new(id, Why::Limit, "past budget")),
            None => Err(Unreadable::new(id, Why::Damaged, "missing stream")),
        });
        let concat2 = Concat::new(&[&enc_page]);
        let mut events_after = Vec::new();
        let again = interpret(
            streams2,
            &concat2,
            resources.as_ref(),
            &limits,
            &fonts,
            Opts::default(),
            |ev| {
                if let Some(snap) = snapshot(&ev) {
                    events_after.push(snap);
                }
                ControlFlow::Continue(())
            },
        );
        if again.is_ok() {
            assert_eq!(events_before.len(), events_after.len());
            for (a, b) in events_before.iter().zip(&events_after) {
                assert_eq!(a.tag, b.tag);
                assert_eq!(a.nums.len(), b.nums.len());
                for (&x, &y) in a.nums.iter().zip(&b.nums) {
                    assert!(x == y || (x.is_nan() && y.is_nan()));
                }
            }
        }
    }
}

fn split_once_bytes<'a>(haystack: &'a [u8], needle: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let pos = haystack.windows(needle.len()).position(|w| w == needle)?;
    Some((&haystack[..pos], &haystack[pos + needle.len()..]))
}

#[derive(Debug)]
struct Snap {
    tag: String,
    nums: Vec<f32>,
}

fn push_matrix(nums: &mut Vec<f32>, m: Matrix) {
    nums.extend(m.to_array());
}

fn push_rect(nums: &mut Vec<f32>, r: Rect) {
    nums.extend([r.x0, r.y0, r.x1, r.y1]);
}

fn snapshot(ev: &Event<'_>) -> Option<Snap> {
    let mut nums = Vec::new();
    let tag = match &ev.kind {
        Kind::Op => return None,
        Kind::Text(show) => {
            push_matrix(&mut nums, show.start);
            nums.push(show.size);
            let mut count = 0usize;
            for g in show.glyphs() {
                count += 1;
                nums.extend([g.origin.x, g.origin.y, g.advance]);
                for p in g.quad.0 {
                    nums.extend([p.x, p.y]);
                }
            }
            format!(
                "text:{:?}:{}:{}:{count}",
                show.string, show.stock, show.mode as u8
            )
        }
        Kind::FormBegin(u) => {
            push_matrix(&mut nums, u.matrix);
            if let Some(b) = u.bbox {
                push_rect(&mut nums, b);
            }
            format!("form_begin:{}:{}", u.form, u.id.depth())
        }
        Kind::FormEnd(u) => format!("form_end:{}:{}", u.form, u.id.depth()),
        Kind::FormRefused { form, why } => format!("form_refused:{form}:{why:?}"),
        Kind::Path(p) => {
            push_rect(&mut nums, p.bounds);
            let mut kinds = String::new();
            for s in p.segments {
                match *s {
                    Seg::Move(pt) => {
                        kinds.push('m');
                        nums.extend([pt.x, pt.y]);
                    }
                    Seg::Line(pt) => {
                        kinds.push('l');
                        nums.extend([pt.x, pt.y]);
                    }
                    Seg::Curve(a, b, c) => {
                        kinds.push('c');
                        nums.extend([a.x, a.y, b.x, b.y, c.x, c.y]);
                    }
                    Seg::Close => kinds.push('h'),
                }
            }
            format!("path:{kinds}:{:?}:{:?}", p.paint, p.clip)
        }
        Kind::Image(img) => {
            for p in img.quad.0 {
                nums.extend([p.x, p.y]);
            }
            format!(
                "image:{:?}:{:?}:{:?}:{}",
                img.width, img.height, img.bits_per_component, img.stencil
            )
        }
        Kind::Shading(sh) => {
            if let Some(b) = sh.bounds {
                push_rect(&mut nums, b);
            }
            format!("shading:{:?}:{}", sh.shading, sh.bounds.is_some())
        }
        Kind::Ignored(id) => format!("ignored:{id}"),
        Kind::MarkedBegin(m) => format!("mb:{:?}:{:?}", m.tag, m.mcid),
        Kind::MarkedEnd(m) => format!("me:{:?}:{:?}", m.tag, m.mcid),
        Kind::MarkedPoint(m) => format!("mp:{:?}:{:?}", m.tag, m.mcid),
    };
    push_matrix(&mut nums, ev.state.ctm);
    if let Some(cb) = ev.state.clip_bounds() {
        push_rect(&mut nums, cb);
    }
    Some(Snap { tag, nums })
}

/// Runs `PageGeom::from_page`, `interpret`, `interpret_with`, and `encode` over
/// the first pages of a parsed PDF trailer and object/stream store.
pub fn over<'a, S>(streams: S, trailer: &'a Dict)
where
    S: Resolve<'a> + Streams + Copy,
{
    let Ok(Some(cat)) = pdf_semantics::catalog(streams, trailer) else {
        return;
    };
    let mut sem_limits = pdf_semantics::Limits::DEFAULT;
    sem_limits.tree_depth = 16;
    sem_limits.pages = 16;
    let Ok(table) = pages::pages(streams, cat, Rules::Pdfium, &sem_limits, &mut ()) else {
        return;
    };
    let limits = Limits {
        form_depth: 6,
        states: 32,
        operators: 4_096,
        glyphs: 4_096,
        segments: 4_096,
        marked: 32,
        interpreted: 64 << 10,
        decompressed: 128 << 10,
        ..Limits::DEFAULT
    };
    let fonts = Fonts::default();
    for page in table.iter().take(4) {
        let geom = PageGeom::from_page(page);
        let _ = geom.to_view();
        let _ = geom.from_view();
        let Some(page_dict) = page.dict else {
            continue;
        };
        let mut join = Join::new(&limits.content);
        if let Some(contents) = page_dict.get(b"Contents")
            && join.push_contents(streams, contents).is_err()
        {
            continue;
        }
        let content = join.finish();
        let resources = page.attribute(Inheritable::Resources).map(|r| r.written);
        let r1 = interpret(
            streams,
            &content,
            resources,
            &limits,
            &fonts,
            Opts::default(),
            |_| ControlFlow::Continue(()),
        );
        let mut faults = Vec::new();
        let r2 = interpret_with(
            streams,
            &content,
            resources,
            &limits,
            &fonts,
            Opts::default(),
            &mut faults,
            |_| ControlFlow::Continue(()),
        );
        assert_eq!(r1, r2);
        let e1 = encode(streams, &content, resources, &limits, &fonts);
        let mut efaults = Vec::new();
        let e2 = encode_with(streams, &content, resources, &limits, &fonts, &mut efaults);
        assert_eq!(e1, e2);
    }
}

#[cfg(test)]
mod tests {
    use super::SEEDS;

    /// `PDF_GFX_FUZZ_SEEDS` names a folder: writes the seed inputs there for
    /// `just fuzz gfx` to begin from.
    #[test]
    #[ignore = "needs PDF_GFX_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_GFX_FUZZ_SEEDS").expect("PDF_GFX_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in SEEDS.iter().enumerate() {
            std::fs::write(dir.join(format!("seed-{i}")), seed).unwrap();
        }
    }
}
