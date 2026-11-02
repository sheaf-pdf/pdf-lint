//! A function's shape, as PDFium loads one (ISO 32000-2, 7.10): how many inputs
//! and outputs it has, or that it doesn't load, from its dictionary alone. A
//! DeviceN space doesn't load without a function with enough outputs, so that's
//! all a count needs of it. Nothing here evaluates a function or reads a
//! stream's data: a sampled function shorter than its sizes say and a
//! PostScript function that doesn't parse, which PDFium refuses, load here.

use pdf_objects::{Dict, Object, Resolve, Unreadable, Value};

use crate::count::{Hop, Walk};

/// A function's inputs and outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    pub(crate) inputs: u32,
    pub(crate) outputs: u32,
}

/// The shape of the function `at`, none where it doesn't load. None for `at` is
/// nothing there.
pub(crate) fn shape<'o, R: Resolve<'o>>(
    walk: &mut Walk<'_, R>,
    at: Option<Hop<'o>>,
    depth: usize,
) -> Result<Option<Shape>, Unreadable> {
    match at {
        Some(at) => walk.on_path(at, depth, load),
        None => Ok(None),
    }
}

/// A count of an array's elements or pairs, as PDFium holds it in 32 bits.
fn count(elements: usize) -> Option<u32> {
    u32::try_from(elements).ok()
}

/// What every kind asks: a dictionary or a stream's, whose `/FunctionType`,
/// by the integer accessor, is one of four, 0 where it isn't written; a
/// `/Domain` of at least one pair; and a `/Range`, whose pairs are its outputs
/// until its kind says otherwise, and which a sampled or PostScript function
/// must have. Then what its kind asks.
fn load<'o, R: Resolve<'o>>(
    walk: &mut Walk<'_, R>,
    at: Hop<'o>,
    depth: usize,
) -> Result<Option<Shape>, Unreadable> {
    let (dict, is_stream) = match at.object.value() {
        Value::Stream(stream) => (&stream.dict, true),
        Value::Dict(dict) => (dict, false),
        _ => return Ok(None),
    };
    let kind = walk.integer(dict.get(b"FunctionType"))?;
    if !matches!(kind, 0 | 2 | 3 | 4) {
        return Ok(None);
    }
    let Some(domain) = walk.array(dict.get(b"Domain"))? else {
        return Ok(None);
    };
    let Some(inputs) = count(domain.len() / 2).filter(|inputs| *inputs > 0) else {
        return Ok(None);
    };
    let range = walk.array(dict.get(b"Range"))?;
    let Some(outputs) = count(range.map_or(0, |range| range.len() / 2)) else {
        return Ok(None);
    };
    if matches!(kind, 0 | 4) && (outputs == 0 || !is_stream) {
        return Ok(None);
    }
    let shape = |outputs| Shape { inputs, outputs };
    Ok(match kind {
        0 => sampled(walk, dict, inputs, outputs)?.map(|()| shape(outputs)),
        2 => exponential(walk, dict, inputs, outputs)?.map(shape),
        3 => stitching(walk, dict, inputs, depth)?.map(shape),
        _ => Some(shape(outputs)),
    })
}

/// A sampled function's dictionary (7.10.2): a `/Size` with an element above 0
/// for each input, a `/BitsPerSample` of those Table 39 lists, by the integer
/// accessor each, and all the samples' bits to be counted in 32 of them. Whether
/// the stream holds them is what isn't read.
fn sampled<'o, R: Resolve<'o>>(
    walk: &mut Walk<'_, R>,
    dict: &'o Dict,
    inputs: u32,
    outputs: u32,
) -> Result<Option<()>, Unreadable> {
    let Some(size) = walk.array(dict.get(b"Size"))?.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let bits = walk.integer(dict.get(b"BitsPerSample"))?;
    if !matches!(bits, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
        return Ok(None);
    }
    let mut total = (bits as u32).checked_mul(outputs);
    for input in 0..inputs as usize {
        let samples = walk.integer(size.get(input))?;
        if samples <= 0 {
            return Ok(None);
        }
        total = total.and_then(|total| total.checked_mul(samples as u32));
    }
    let bytes = total.and_then(|total| total.checked_add(7)).map(|n| n / 8);
    Ok(bytes.filter(|bytes| *bytes > 0).map(|_| ()))
}

/// An exponential function's dictionary (7.10.3): an `/N` that's a number where
/// it's written, a reference being none; its outputs its `/Range`'s, or its
/// `/C0`'s, or 1, for each input.
fn exponential<'o, R: Resolve<'o>>(
    walk: &mut Walk<'_, R>,
    dict: &'o Dict,
    inputs: u32,
    outputs: u32,
) -> Result<Option<u32>, Unreadable> {
    let number = |object: &Object| matches!(object.value(), Value::Integer(_) | Value::Real(_));
    if !dict.get(b"N").is_some_and(number) {
        return Ok(None);
    }
    let start = walk.array(dict.get(b"C0"))?;
    let outputs = match outputs {
        0 => start
            .and_then(|start| count(start.len()))
            .unwrap_or(0)
            .max(1),
        outputs => outputs,
    };
    Ok(outputs.checked_mul(inputs))
}

/// A stitching function's dictionary (7.10.4): one input, at least one
/// subfunction, `/Bounds` and `/Encode` long enough, and each subfunction, itself
/// not among them, of one input and with as many outputs as the others, which
/// are its own. One written in place spends a visit as one by reference does, so
/// that whatever's walked is bounded by the same cap.
fn stitching<'o, R: Resolve<'o>>(
    walk: &mut Walk<'_, R>,
    dict: &'o Dict,
    inputs: u32,
    depth: usize,
) -> Result<Option<u32>, Unreadable> {
    if inputs != 1 {
        return Ok(None);
    }
    let Some(kids) = walk
        .array(dict.get(b"Functions"))?
        .filter(|f| !f.is_empty())
    else {
        return Ok(None);
    };
    let Some(bounds) = walk.array(dict.get(b"Bounds"))? else {
        return Ok(None);
    };
    let Some(encode) = walk.array(dict.get(b"Encode"))? else {
        return Ok(None);
    };
    let enough = kids
        .len()
        .checked_mul(2)
        .is_some_and(|two| two <= encode.len());
    if bounds.len() < kids.len() - 1 || !enough {
        return Ok(None);
    }
    let mut alike = None;
    for kid in kids {
        if kid.as_reference().is_none() && !walk.spend() {
            return Ok(None);
        }
        let at = walk.hop(kid)?;
        let Some(kid) = shape(walk, at, depth + 1)? else {
            return Ok(None);
        };
        if kid.inputs != 1 || kid.outputs == 0 || alike.is_some_and(|alike| alike != kid.outputs) {
            return Ok(None);
        }
        alike = Some(kid.outputs);
    }
    Ok(alike)
}
