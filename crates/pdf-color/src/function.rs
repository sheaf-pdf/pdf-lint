//! PDF functions (ISO 32000-2, §7.10): dictionary shape inspection (`shape`, used for
//! component counting and `Space::read` checks) and full bounded loading and evaluation
//! ([`Function`], [`FunctionKind`]) for `Type 0` (sampled), `Type 2` (exponential), `Type 3`
//! (stitching), and `Type 4` (PostScript calculator) functions.

use std::borrow::Cow;
use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{
    Dict, Object, ObjectId, Resolve, Rules, Span, Stream, Streams, Unreadable, Value, Why,
};

use crate::count::{Hop, Walk, float};
use crate::fault::Fault;
use crate::limits::Limits;

use ps::{Program, PsParseError};

mod ps;

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

// ---------------------------------------------------------------------------
// Public bounded §7.10 Function reader and evaluator
// ---------------------------------------------------------------------------

/// The kind of a PDF function (`/FunctionType`; ISO 32000-2, §7.10.1, Table 38).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FunctionKind {
    /// `Type 0`: Sampled multilinear lookup table (§7.10.2).
    Sampled,
    /// `Type 2`: Exponential interpolation (§7.10.3).
    Exponential,
    /// `Type 3`: 1-input stitching of subfunctions (§7.10.4).
    Stitching,
    /// `Type 4`: PostScript calculator stack machine (§7.10.5).
    PostScript,
}

/// A parsed, ready-to-evaluate ISO 32000-2 §7.10 PDF function.
///
/// Constructed via [`Function::read`] under explicit [`Limits`] and [`Rules`], and evaluated via
/// [`Function::eval`] or [`Function::eval_vec`] with zero panics on any input.
#[derive(Clone, PartialEq)]
pub struct Function {
    domain: Vec<(f32, f32)>,
    range: Option<Vec<(f32, f32)>>,
    outputs: u32,
    inner: FunctionInner,
}

#[derive(Clone, PartialEq)]
enum FunctionInner {
    Sampled(SampledData),
    Exponential(ExpData),
    Stitching(StitchData),
    PostScript(Program),
}

#[derive(Clone, PartialEq)]
struct SampledData {
    sizes: Vec<usize>,
    encode: Vec<(f32, f32)>,
    samples: Vec<f32>,
}

#[derive(Clone, PartialEq)]
struct ExpData {
    c0: Vec<f32>,
    c1: Vec<f32>,
    exponent: f32,
}

#[derive(Clone, PartialEq)]
struct StitchData {
    functions: Vec<Function>,
    bounds: Vec<f32>,
    encode: Vec<(f32, f32)>,
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Function");
        s.field("kind", &self.kind())
            .field("inputs", &self.inputs())
            .field("outputs", &self.outputs)
            .field("domain", &self.domain)
            .field("range", &self.range);
        match &self.inner {
            FunctionInner::Sampled(d) => {
                s.field("sizes", &d.sizes)
                    .field("sample_count", &d.samples.len());
            }
            FunctionInner::Exponential(d) => {
                s.field("exponent", &d.exponent);
            }
            FunctionInner::Stitching(d) => {
                s.field("subfunctions", &d.functions.len())
                    .field("bounds", &d.bounds);
            }
            FunctionInner::PostScript(p) => {
                s.field("ops", &p.len());
            }
        }
        s.finish()
    }
}

impl Function {
    /// Reads and prepares the PDF function at `value` (`Type 0`, `Type 2`, `Type 3`, or `Type 4`).
    ///
    /// Returns `Ok(None)` when `value` cannot be loaded as a function under `rules` and `limits`,
    /// reporting spec departures to `sink` when `sink.wants()` is `true`. Passing `&mut ()`
    /// produces the exact same `Result<Option<Function>, Unreadable>` without constructing fault
    /// diagnostics.
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] for non-recoverable reader failures (`Why::Capped`, `Why::Locked`,
    /// `Why::Source`, `Why::Detached`). Damaged objects/streams (`Why::Damaged`, `Why::Filter`)
    /// and `Limits` caps (`Why::Limit`) report a [`Fault`] and return `Ok(None)`.
    pub fn read<'o, O, S>(
        objects: O,
        value: &'o Object,
        rules: Rules,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Option<Function>, Unreadable>
    where
        O: Resolve<'o> + Streams,
        S: Sink<Fault> + ?Sized,
    {
        FuncReader::new(objects, rules, limits, sink).func(value, 0)
    }

    /// Function type (`Sampled`, `Exponential`, `Stitching`, or `PostScript`).
    #[must_use]
    pub const fn kind(&self) -> FunctionKind {
        match &self.inner {
            FunctionInner::Sampled(_) => FunctionKind::Sampled,
            FunctionInner::Exponential(_) => FunctionKind::Exponential,
            FunctionInner::Stitching(_) => FunctionKind::Stitching,
            FunctionInner::PostScript(_) => FunctionKind::PostScript,
        }
    }

    /// Number of input variables $m \ge 1$.
    #[must_use]
    pub fn inputs(&self) -> u32 {
        self.domain.len() as u32
    }

    /// Number of output values $n \ge 1$.
    #[must_use]
    pub const fn outputs(&self) -> u32 {
        self.outputs
    }

    /// Clamping domain `(min, max)` per input variable.
    #[must_use]
    pub fn domain(&self) -> &[(f32, f32)] {
        &self.domain
    }

    /// Optional clamping range `(min, max)` per output variable.
    #[must_use]
    pub fn range(&self) -> Option<&[(f32, f32)]> {
        self.range.as_deref()
    }

    /// Evaluates the function on `inputs[..self.inputs()]`, writing into
    /// `outputs[..self.outputs()]`.
    ///
    /// Returns `false` if `inputs` or `outputs` is shorter than required, or if a `Type 4`
    /// calculator program fails at runtime (stack underflow/overflow, division by zero, or
    /// exceeding `limits.postscript_ops`).
    pub fn eval(&self, inputs: &[f32], outputs: &mut [f32], limits: &Limits) -> bool {
        let m = self.domain.len();
        let n = self.outputs as usize;
        if inputs.len() < m || outputs.len() < n {
            return false;
        }

        // Clamp inputs to `/Domain`.
        let mut clamped_stack = [0.0_f32; 16];
        let mut clamped_heap = Vec::new();
        let x: &mut [f32] = if m <= clamped_stack.len() {
            &mut clamped_stack[..m]
        } else {
            clamped_heap.resize(m, 0.0);
            &mut clamped_heap
        };
        for (i, &(d0, d1)) in self.domain.iter().enumerate() {
            x[i] = clamp_pair(inputs[i], d0, d1);
        }

        let out = &mut outputs[..n];
        let ok = match &self.inner {
            FunctionInner::Sampled(data) => {
                data.eval_with_domain(&self.domain, x, out);
                true
            }
            FunctionInner::Exponential(data) => {
                eval_exponential(data, x, out);
                true
            }
            FunctionInner::Stitching(data) => {
                eval_stitching(data, self.domain[0], x[0], out, limits)
            }
            FunctionInner::PostScript(prog) => prog.exec(x, out, limits),
        };
        if !ok {
            return false;
        }

        if let Some(range) = &self.range {
            for (y, &(r0, r1)) in out.iter_mut().zip(range) {
                *y = clamp_pair(*y, r0, r1);
            }
        }
        true
    }

    /// Evaluates the function on `inputs` and returns a `Vec<f32>` of length [`Self::outputs`],
    /// or `None` if evaluation fails.
    #[must_use]
    pub fn eval_vec(&self, inputs: &[f32], limits: &Limits) -> Option<Vec<f32>> {
        let mut out = vec![0.0_f32; self.outputs as usize];
        self.eval(inputs, &mut out, limits).then_some(out)
    }
}

#[inline]
pub(crate) fn clamp_pair(v: f32, a: f32, b: f32) -> f32 {
    if !v.is_finite() {
        return if a.is_finite() { a } else { 0.0 };
    }
    // Matches PDFium's `std::min(std::max(v, a), b)` when `a <= b`, and never panics when `a > b`.
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    v.max(lo).min(hi)
}

#[inline]
fn interpolate(x: f32, x_min: f32, x_max: f32, y_min: f32, y_max: f32) -> f32 {
    let denom = x_max - x_min;
    if denom.abs() <= f32::EPSILON {
        y_min
    } else {
        y_min + (x - x_min) * (y_max - y_min) / denom
    }
}

fn eval_exponential(data: &ExpData, x: &[f32], out: &mut [f32]) {
    let base_n = data.c0.len();
    let mut pos = 0;
    for &xi in x {
        let factor = if data.exponent == 1.0 {
            xi
        } else if xi <= 0.0 && data.exponent.fract() != 0.0 {
            0.0
        } else {
            let p = xi.powf(data.exponent);
            if p.is_finite() { p } else { 0.0 }
        };
        for j in 0..base_n {
            if let Some(dst) = out.get_mut(pos) {
                let c0 = data.c0[j];
                let c1 = data.c1[j];
                let v = c0 + factor * (c1 - c0);
                *dst = if v.is_finite() { v } else { c0 };
                pos += 1;
            }
        }
    }
}

fn eval_stitching(
    data: &StitchData,
    domain0: (f32, f32),
    x: f32,
    out: &mut [f32],
    limits: &Limits,
) -> bool {
    let k = data.functions.len();
    if k == 0 {
        return false;
    }
    let idx = if k == 1 {
        0
    } else {
        data.bounds.iter().position(|&b| x < b).unwrap_or(k - 1)
    };
    let sub_lo = if idx == 0 {
        domain0.0
    } else {
        data.bounds[idx - 1]
    };
    let sub_hi = if idx + 1 == k {
        domain0.1
    } else {
        data.bounds[idx]
    };
    let (e0, e1) = data.encode[idx];
    let mapped = interpolate(x, sub_lo, sub_hi, e0, e1);
    data.functions[idx].eval(&[mapped], out, limits)
}

// Let's wire `SampledData` with `domain` cleanly so `eval_sampled` maps `domain[i] -> encode[i]`:
struct FuncReader<'l, O, S: ?Sized> {
    objects: O,
    rules: Rules,
    limits: &'l Limits,
    sink: &'l mut S,
    visits: usize,
    visits_told: bool,
    path: Vec<ObjectId>,
}

enum Reached<'o> {
    At(&'o Object),
    Nothing,
    Stopped,
}

impl<'l, 'o, O, S> FuncReader<'l, O, S>
where
    O: Resolve<'o> + Streams,
    S: Sink<Fault> + ?Sized,
{
    fn new(objects: O, rules: Rules, limits: &'l Limits, sink: &'l mut S) -> Self {
        Self {
            objects,
            rules,
            limits,
            sink,
            visits: 0,
            visits_told: false,
            path: Vec::new(),
        }
    }

    fn say(&mut self, at: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            let obj = self.path.last().copied();
            self.sink.fault(Place::new(obj, at), fault);
        }
    }

    fn spend(&mut self, at: Option<Span>) -> bool {
        if self.visits < self.limits.space_visits {
            self.visits += 1;
            return true;
        }
        if !std::mem::replace(&mut self.visits_told, true) {
            self.say(at, Fault::TooManyVisits);
        }
        false
    }

    fn through(&mut self, written: &'o Object) -> Result<Reached<'o>, Unreadable> {
        let mut object = written;
        let mut followed = 0;
        while let Some(id) = object.as_reference() {
            followed += 1;
            if followed == 2 {
                self.say(object.at(), Fault::Chained);
                if self.rules == Rules::Pdfium {
                    return Ok(Reached::Nothing);
                }
            }
            if self.path.contains(&id) {
                self.say(object.at(), Fault::Loop);
                return Ok(Reached::Stopped);
            }
            if self.path.len() > self.limits.space_depth {
                self.say(object.at(), Fault::TooDeep);
                return Ok(Reached::Stopped);
            }
            if !self.spend(object.at()) {
                return Ok(Reached::Stopped);
            }
            match self.objects.try_resolve(id) {
                Ok(Some(next)) => {
                    self.path.push(id);
                    object = next;
                }
                Ok(None) => return Ok(Reached::Nothing),
                Err(e) if e.why == Why::Damaged => {
                    self.say(object.at(), Fault::Damaged { id });
                    return Ok(Reached::Stopped);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Reached::At(object))
    }

    fn entered<T>(
        &mut self,
        written: &'o Object,
        nothing: Option<Fault>,
        f: impl FnOnce(&mut Self, &'o Object) -> Result<Option<T>, Unreadable>,
    ) -> Result<Option<T>, Unreadable> {
        let mark = self.path.len();
        let res = match self.through(written) {
            Ok(Reached::At(obj)) => f(self, obj),
            Ok(Reached::Nothing) => {
                self.path.truncate(mark);
                if let Some(fault) = nothing {
                    self.say(written.at(), fault);
                }
                Ok(None)
            }
            Ok(Reached::Stopped) => Ok(None),
            Err(e) => Err(e),
        };
        self.path.truncate(mark);
        res
    }

    fn num_f32(&mut self, written: &'o Object, fault: Fault) -> Result<Option<f32>, Unreadable> {
        self.entered(written, Some(fault), |reader, obj| match reader.rules {
            Rules::Iso => match obj.as_number().filter(|v| v.is_finite()) {
                Some(v) => Ok(Some(v as f32)),
                None => {
                    reader.say(obj.at(), fault);
                    Ok(None)
                }
            },
            _ => {
                if obj.as_number().is_none() {
                    reader.say(obj.at(), fault);
                }
                Ok(Some(float(obj)))
            }
        })
    }

    fn pairs(
        &mut self,
        written: &'o Object,
        fault: Fault,
        expected_pairs: Option<usize>,
    ) -> Result<Option<Vec<(f32, f32)>>, Unreadable> {
        self.entered(written, Some(fault), |reader, obj| {
            let Some(arr) = obj.as_array() else {
                reader.say(obj.at(), fault);
                return Ok(None);
            };
            let pair_count = arr.len() / 2;
            if pair_count == 0
                || (reader.rules == Rules::Iso && arr.len() % 2 != 0)
                || expected_pairs.is_some_and(|want| match reader.rules {
                    Rules::Iso => arr.len() != want * 2,
                    _ => pair_count < want,
                })
            {
                reader.say(obj.at(), fault);
                return Ok(None);
            }
            let take_pairs = expected_pairs.unwrap_or(pair_count);
            let mut out = Vec::with_capacity(take_pairs);
            for i in 0..take_pairs {
                let Some(lo) = reader.num_f32(&arr[2 * i], fault)? else {
                    return Ok(None);
                };
                let Some(hi) = reader.num_f32(&arr[2 * i + 1], fault)? else {
                    return Ok(None);
                };
                if lo > hi {
                    reader.say(arr[2 * i].at(), fault);
                }
                out.push((lo, hi));
            }
            Ok(Some(out))
        })
    }

    fn stream_bytes(
        &mut self,
        stream: &'o Stream,
        stream_id: Option<ObjectId>,
        at: Option<Span>,
        cap_fault: Fault,
        corrupt_fault: Fault,
    ) -> Result<Option<Cow<'o, [u8]>>, Unreadable> {
        let limit = self.limits.stream_bytes;
        if let Some(id) = stream_id {
            let res = self.objects.decoded(id, limit).map(|b| b.into_owned());
            match res {
                Ok(bytes) => Ok(Some(Cow::Owned(bytes))),
                Err(e) if e.why == Why::Limit => {
                    self.say(at, cap_fault);
                    Ok(None)
                }
                Err(e) if matches!(e.why, Why::Damaged | Why::Filter) => {
                    self.say(at, corrupt_fault);
                    Ok(None)
                }
                Err(e) => Err(e),
            }
        } else {
            if stream.dict.has(b"Filter") {
                self.say(at, corrupt_fault);
                return Ok(None);
            }
            let Some(bytes) = stream.data.bytes() else {
                self.say(at, corrupt_fault);
                return Ok(None);
            };
            if bytes.len() > limit {
                self.say(at, cap_fault);
                return Ok(None);
            }
            Ok(Some(Cow::Borrowed(bytes)))
        }
    }

    fn func(&mut self, written: &'o Object, depth: usize) -> Result<Option<Function>, Unreadable> {
        if depth > self.limits.space_depth {
            self.say(written.at(), Fault::TooDeep);
            return Ok(None);
        }
        self.entered(written, Some(Fault::NotAFunction), |reader, obj| {
            let stream_id = written.as_reference().and(reader.path.last().copied());
            let (dict, stream) = match obj.value() {
                Value::Stream(s) => (&s.dict, Some(s)),
                Value::Dict(d) => (d, None),
                _ => {
                    reader.say(obj.at(), Fault::NotAFunction);
                    return Ok(None);
                }
            };

            let kind_num = match dict.get(b"FunctionType").filter(|v| !v.is_null()) {
                Some(ft) => {
                    let v =
                        reader.entered(ft, Some(Fault::FunctionType), |r, ko| match r.rules {
                            Rules::Iso => match ko.as_integer() {
                                Some(k @ (0 | 2 | 3 | 4)) => Ok(Some(k as i32)),
                                _ => {
                                    r.say(ko.at(), Fault::FunctionType);
                                    Ok(None)
                                }
                            },
                            _ => {
                                let k = pdfium_int(ko);
                                if !matches!(k, 0 | 2 | 3 | 4) {
                                    r.say(ko.at(), Fault::FunctionType);
                                    Ok(None)
                                } else {
                                    if ko.as_integer().is_none() {
                                        r.say(ko.at(), Fault::FunctionType);
                                    }
                                    Ok(Some(k))
                                }
                            }
                        })?;
                    let Some(k) = v else {
                        return Ok(None);
                    };
                    k
                }
                None => match reader.rules {
                    Rules::Iso => {
                        reader.say(obj.at(), Fault::FunctionType);
                        return Ok(None);
                    }
                    _ => {
                        reader.say(obj.at(), Fault::FunctionType);
                        0
                    }
                },
            };

            let Some(domain_obj) = dict.get(b"Domain").filter(|v| !v.is_null()) else {
                reader.say(obj.at(), Fault::FunctionDomain);
                return Ok(None);
            };
            let Some(domain) = reader.pairs(domain_obj, Fault::FunctionDomain, None)? else {
                return Ok(None);
            };

            let range = match dict.get(b"Range").filter(|v| !v.is_null()) {
                Some(r_obj) => match reader.pairs(r_obj, Fault::FunctionRange, None)? {
                    Some(r) => Some(r),
                    None if matches!(kind_num, 0 | 4) => return Ok(None),
                    None => None,
                },
                None => {
                    if matches!(kind_num, 0 | 4) {
                        reader.say(obj.at(), Fault::FunctionRange);
                        return Ok(None);
                    }
                    None
                }
            };

            match kind_num {
                0 => {
                    let Some(stream) = stream else {
                        reader.say(obj.at(), Fault::FunctionNotStream);
                        return Ok(None);
                    };
                    let Some(range_ref) = range.as_ref() else {
                        return Ok(None);
                    };
                    let outputs = range_ref.len() as u32;
                    let Some(sampled) = reader.read_sampled(
                        dict,
                        stream,
                        stream_id,
                        obj.at(),
                        &domain,
                        range_ref,
                    )?
                    else {
                        return Ok(None);
                    };
                    Ok(Some(Function {
                        domain,
                        range,
                        outputs,
                        inner: FunctionInner::Sampled(sampled),
                    }))
                }
                2 => {
                    if stream.is_some() {
                        reader.say(obj.at(), Fault::FunctionStream);
                    }
                    reader.read_exponential(dict, obj.at(), domain, range)
                }
                3 => {
                    if stream.is_some() {
                        reader.say(obj.at(), Fault::FunctionStream);
                    }
                    reader.read_stitching(dict, obj.at(), domain, range, depth)
                }
                4 => {
                    let Some(stream) = stream else {
                        reader.say(obj.at(), Fault::FunctionNotStream);
                        return Ok(None);
                    };
                    let outputs = range.as_ref().map_or(0, Vec::len) as u32;
                    let Some(prog) = reader.read_postscript(stream, stream_id, obj.at())? else {
                        return Ok(None);
                    };
                    Ok(Some(Function {
                        domain,
                        range,
                        outputs,
                        inner: FunctionInner::PostScript(prog),
                    }))
                }
                _ => Ok(None),
            }
        })
    }

    fn read_sampled(
        &mut self,
        dict: &'o Dict,
        stream: &'o Stream,
        stream_id: Option<ObjectId>,
        at: Option<Span>,
        domain: &[(f32, f32)],
        range: &[(f32, f32)],
    ) -> Result<Option<SampledData>, Unreadable> {
        let m = domain.len();
        let n = range.len();

        let Some(size_obj) = dict.get(b"Size").filter(|v| !v.is_null()) else {
            self.say(at, Fault::SampleSize);
            return Ok(None);
        };
        let Some(sizes) = self.entered(size_obj, Some(Fault::SampleSize), |r, so| {
            let Some(arr) = so.as_array() else {
                r.say(so.at(), Fault::SampleSize);
                return Ok(None);
            };
            if (r.rules == Rules::Iso && arr.len() != m) || arr.len() < m {
                r.say(so.at(), Fault::SampleSize);
                return Ok(None);
            }
            let mut out = Vec::with_capacity(m);
            for item in &arr[..m] {
                let val = r.entered(item, Some(Fault::SampleSize), |r2, io| {
                    let v = match r2.rules {
                        Rules::Iso => io.as_integer().and_then(|v| usize::try_from(v).ok()),
                        _ => usize::try_from(pdfium_int(io)).ok(),
                    };
                    match v.filter(|&s| s >= 1) {
                        Some(s) => Ok(Some(s)),
                        None => {
                            r2.say(io.at(), Fault::SampleSize);
                            Ok(None)
                        }
                    }
                })?;
                let Some(s) = val else {
                    return Ok(None);
                };
                out.push(s);
            }
            Ok(Some(out))
        })?
        else {
            return Ok(None);
        };

        let Some(bps_obj) = dict.get(b"BitsPerSample").filter(|v| !v.is_null()) else {
            self.say(at, Fault::BitsPerSample);
            return Ok(None);
        };
        let Some(bps) = self.entered(bps_obj, Some(Fault::BitsPerSample), |r, bo| {
            let b = match r.rules {
                Rules::Iso => bo.as_integer(),
                _ => Some(i64::from(pdfium_int(bo))),
            };
            match b {
                Some(b @ (1 | 2 | 4 | 8 | 12 | 16 | 24 | 32)) => Ok(Some(b as u32)),
                _ => {
                    r.say(bo.at(), Fault::BitsPerSample);
                    Ok(None)
                }
            }
        })?
        else {
            return Ok(None);
        };

        if let Some(order_obj) = dict.get(b"Order").filter(|v| !v.is_null()) {
            self.entered(order_obj, Some(Fault::SampleOrder), |r, oo| {
                if !matches!(oo.as_integer(), Some(1 | 3)) {
                    r.say(oo.at(), Fault::SampleOrder);
                }
                Ok(Some(()))
            })?;
        }

        let mut total_samples = n;
        for &s in &sizes {
            let Some(prod) = total_samples
                .checked_mul(s)
                .filter(|&p| p <= self.limits.sample_values)
            else {
                self.say(at, Fault::TooManySamples);
                return Ok(None);
            };
            total_samples = prod;
        }
        let Some(total_bits) = total_samples.checked_mul(bps as usize) else {
            self.say(at, Fault::TooManySamples);
            return Ok(None);
        };
        let needed_bytes = total_bits.div_ceil(8);

        let encode = match dict.get(b"Encode").filter(|v| !v.is_null()) {
            Some(enc_obj) => match self.pairs(enc_obj, Fault::FunctionEncode, Some(m))? {
                Some(p) => p,
                None if self.rules == Rules::Iso => return Ok(None),
                None => sizes
                    .iter()
                    .map(|&s| (0.0, (s.saturating_sub(1)) as f32))
                    .collect(),
            },
            None => sizes
                .iter()
                .map(|&s| (0.0, (s.saturating_sub(1)) as f32))
                .collect(),
        };

        let decode = match dict.get(b"Decode").filter(|v| !v.is_null()) {
            Some(dec_obj) => match self.pairs(dec_obj, Fault::SampleDecode, Some(n))? {
                Some(p) => p,
                None if self.rules == Rules::Iso => return Ok(None),
                None => range.to_vec(),
            },
            None => range.to_vec(),
        };

        let Some(raw) = self.stream_bytes(
            stream,
            stream_id,
            at,
            Fault::TooManySamples,
            Fault::SampleDataLength,
        )?
        else {
            return Ok(None);
        };
        if raw.len() < needed_bytes {
            self.say(at, Fault::SampleDataLength);
            return Ok(None);
        }

        let samples = unpack_samples(&raw, total_samples, bps, n, &decode);
        Ok(Some(SampledData {
            sizes,
            encode,
            samples,
        }))
    }

    fn read_exponential(
        &mut self,
        dict: &'o Dict,
        at: Option<Span>,
        domain: Vec<(f32, f32)>,
        range: Option<Vec<(f32, f32)>>,
    ) -> Result<Option<Function>, Unreadable> {
        let m = domain.len();
        if m != 1 {
            self.say(at, Fault::FunctionDomain);
            if self.rules == Rules::Iso {
                return Ok(None);
            }
        }

        let Some(n_obj) = dict.get(b"N").filter(|v| !v.is_null()) else {
            self.say(at, Fault::Exponent);
            return Ok(None);
        };
        let Some(exponent) = self.num_f32(n_obj, Fault::Exponent)? else {
            return Ok(None);
        };
        // Validate mathematical domain constraints from ISO 32000-2 Table 40.
        for &(d0, d1) in &domain {
            let lo = d0.min(d1);
            let hi = d0.max(d1);
            if (lo < 0.0 && exponent.fract() != 0.0) || (exponent < 0.0 && lo <= 0.0 && hi >= 0.0) {
                self.say(n_obj.at(), Fault::Exponent);
            }
        }

        let read_endpoint =
            |reader: &mut Self, key: &[u8]| -> Result<Option<Vec<f32>>, Unreadable> {
                let Some(written) = dict.get(key).filter(|v| !v.is_null()) else {
                    return Ok(None);
                };
                reader.entered(written, Some(Fault::ExponentialEndpoints), |r, ao| {
                    let Some(arr) = ao.as_array().filter(|a| !a.is_empty()) else {
                        r.say(ao.at(), Fault::ExponentialEndpoints);
                        return Ok(None);
                    };
                    let mut vals = Vec::with_capacity(arr.len());
                    for item in arr {
                        let Some(v) = r.num_f32(item, Fault::ExponentialEndpoints)? else {
                            return Ok(None);
                        };
                        vals.push(v);
                    }
                    Ok(Some(vals))
                })
            };

        let c0_opt = read_endpoint(self, b"C0")?;
        let c1_opt = read_endpoint(self, b"C1")?;
        if let (Some(c0), Some(c1)) = (&c0_opt, &c1_opt)
            && c0.len() != c1.len()
        {
            self.say(at, Fault::ExponentialEndpoints);
            if self.rules == Rules::Iso {
                return Ok(None);
            }
        }

        let base_n = match (&range, &c0_opt, &c1_opt) {
            (Some(r), _, _) => {
                if let Some(c0) = &c0_opt
                    && c0.len() != r.len()
                {
                    self.say(at, Fault::ExponentialEndpoints);
                }
                if let Some(c1) = &c1_opt
                    && c1.len() != r.len()
                {
                    self.say(at, Fault::ExponentialEndpoints);
                }
                r.len()
            }
            (None, Some(c0), _) => c0.len(),
            (None, None, Some(c1)) => c1.len(),
            (None, None, None) => 1,
        };

        let mut c0 = c0_opt.unwrap_or_else(|| vec![0.0; base_n]);
        c0.resize(base_n, 0.0);
        let mut c1 = c1_opt.unwrap_or_else(|| vec![1.0; base_n]);
        c1.resize(base_n, 1.0);

        let Some(outputs) = (base_n as u32).checked_mul(m as u32).filter(|&o| o > 0) else {
            return Ok(None);
        };

        Ok(Some(Function {
            domain,
            range,
            outputs,
            inner: FunctionInner::Exponential(ExpData { c0, c1, exponent }),
        }))
    }

    fn read_stitching(
        &mut self,
        dict: &'o Dict,
        at: Option<Span>,
        domain: Vec<(f32, f32)>,
        range: Option<Vec<(f32, f32)>>,
        depth: usize,
    ) -> Result<Option<Function>, Unreadable> {
        if domain.len() != 1 {
            self.say(at, Fault::FunctionDomain);
            return Ok(None);
        }
        let Some(fns_obj) = dict.get(b"Functions").filter(|v| !v.is_null()) else {
            self.say(at, Fault::StitchFunctions);
            return Ok(None);
        };
        let Some(functions) = self.entered(fns_obj, Some(Fault::StitchFunctions), |r, fo| {
            let Some(arr) = fo.as_array().filter(|a| !a.is_empty()) else {
                r.say(fo.at(), Fault::StitchFunctions);
                return Ok(None);
            };
            let mut subs = Vec::with_capacity(arr.len());
            let mut expected_out: Option<u32> = None;
            for item in arr {
                if item.as_reference().is_none() && !r.spend(item.at()) {
                    return Ok(None);
                }
                let Some(sub) = r.func(item, depth + 1)? else {
                    r.say(item.at(), Fault::StitchFunctions);
                    return Ok(None);
                };
                if sub.inputs() != 1
                    || sub.outputs() == 0
                    || expected_out.is_some_and(|want| want != sub.outputs())
                {
                    r.say(item.at(), Fault::StitchFunctions);
                    return Ok(None);
                }
                expected_out = Some(sub.outputs());
                subs.push(sub);
            }
            Ok(Some(subs))
        })?
        else {
            return Ok(None);
        };

        let k = functions.len();
        let outputs = functions[0].outputs();
        if let Some(r) = &range
            && r.len() != outputs as usize
        {
            self.say(at, Fault::FunctionRange);
        }

        let Some(bounds_obj) = dict.get(b"Bounds").filter(|v| !v.is_null()) else {
            self.say(at, Fault::StitchBounds);
            return Ok(None);
        };
        let Some(bounds) = self.entered(bounds_obj, Some(Fault::StitchBounds), |r, bo| {
            let Some(arr) = bo.as_array() else {
                r.say(bo.at(), Fault::StitchBounds);
                return Ok(None);
            };
            let want = k - 1;
            if (r.rules == Rules::Iso && arr.len() != want) || arr.len() < want {
                r.say(bo.at(), Fault::StitchBounds);
                return Ok(None);
            }
            let mut vals = Vec::with_capacity(want);
            let (d0, d1) = domain[0];
            let mut prev = d0;
            for (idx, item) in arr[..want].iter().enumerate() {
                let Some(v) = r.num_f32(item, Fault::StitchBounds)? else {
                    return Ok(None);
                };
                let ordered = if idx == 0 {
                    d0 <= v && v <= d1 && (d0 == d1 || d0 < v)
                } else {
                    prev < v && v <= d1
                };
                if !ordered || (idx + 1 == want && d0 < d1 && v >= d1) {
                    r.say(item.at(), Fault::StitchBounds);
                }
                prev = v;
                vals.push(v);
            }
            Ok(Some(vals))
        })?
        else {
            return Ok(None);
        };

        let Some(encode_obj) = dict.get(b"Encode").filter(|v| !v.is_null()) else {
            self.say(at, Fault::FunctionEncode);
            return Ok(None);
        };
        let Some(encode) = self.pairs(encode_obj, Fault::FunctionEncode, Some(k))? else {
            return Ok(None);
        };

        Ok(Some(Function {
            domain,
            range,
            outputs,
            inner: FunctionInner::Stitching(StitchData {
                functions,
                bounds,
                encode,
            }),
        }))
    }

    fn read_postscript(
        &mut self,
        stream: &'o Stream,
        stream_id: Option<ObjectId>,
        at: Option<Span>,
    ) -> Result<Option<Program>, Unreadable> {
        let Some(bytes) = self.stream_bytes(
            stream,
            stream_id,
            at,
            Fault::TooManyOps,
            Fault::PostScriptSyntax,
        )?
        else {
            return Ok(None);
        };
        match Program::parse(&bytes, self.limits) {
            Ok(prog) => Ok(Some(prog)),
            Err(PsParseError::TooDeep) => {
                self.say(at, Fault::TooDeep);
                Ok(None)
            }
            Err(PsParseError::TooManyOps) => {
                self.say(at, Fault::TooManyOps);
                Ok(None)
            }
            Err(PsParseError::Syntax) => {
                self.say(at, Fault::PostScriptSyntax);
                Ok(None)
            }
        }
    }
}

fn pdfium_int(obj: &Object) -> i32 {
    match obj.value() {
        Value::Integer(n) => {
            if let Ok(u) = u32::try_from(*n) {
                u as i32
            } else {
                i32::try_from(*n).unwrap_or(0)
            }
        }
        Value::Real(r) => *r as f32 as i32,
        Value::Bool(true) => 1,
        _ => 0,
    }
}

fn unpack_samples(
    bytes: &[u8],
    total_samples: usize,
    bps: u32,
    n_outputs: usize,
    decode: &[(f32, f32)],
) -> Vec<f32> {
    let mut out = Vec::with_capacity(total_samples);
    let max_val = if bps == 32 {
        f64::from(u32::MAX)
    } else {
        ((1_u64 << bps) - 1) as f64
    };
    let mut bit_pos = 0_usize;
    for idx in 0..total_samples {
        let raw = read_bits_msb(bytes, bit_pos, bps);
        bit_pos += bps as usize;
        let t = (f64::from(raw) / max_val) as f32;
        let (d0, d1) = decode[idx % n_outputs];
        out.push(d0 + t * (d1 - d0));
    }
    out
}

#[inline]
fn read_bits_msb(bytes: &[u8], bit_offset: usize, bps: u32) -> u32 {
    match bps {
        8 => u32::from(bytes[bit_offset >> 3]),
        16 => {
            let i = bit_offset >> 3;
            u32::from(u16::from_be_bytes([bytes[i], bytes[i + 1]]))
        }
        24 => {
            let i = bit_offset >> 3;
            (u32::from(bytes[i]) << 16) | (u32::from(bytes[i + 1]) << 8) | u32::from(bytes[i + 2])
        }
        32 => {
            let i = bit_offset >> 3;
            u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
        }
        _ => {
            // 1, 2, 4, or 12 bits packed MSB-first.
            let mut acc = 0_u32;
            for b in 0..bps as usize {
                let pos = bit_offset + b;
                let byte = bytes[pos >> 3];
                let shift = 7 - (pos & 7);
                acc = (acc << 1) | u32::from((byte >> shift) & 1);
            }
            acc
        }
    }
}

// Complete `Function::eval` sampled interpolation with `self.domain`:
impl SampledData {
    fn eval_with_domain(&self, domain: &[(f32, f32)], x: &[f32], out: &mut [f32]) {
        let m = self.sizes.len();
        let n = out.len();
        out.fill(0.0);

        let mut base_offset = 0_usize;
        let mut active_strides = [0_usize; 12];
        let mut active_fracs = [0.0_f32; 12];
        let mut active_len = 0_usize;
        let mut stride = n;

        for i in 0..m {
            let s = self.sizes[i];
            let (d0, d1) = domain[i];
            let (e0, e1) = self.encode[i];
            let e = interpolate(x[i], d0, d1, e0, e1);
            let e_clamped = clamp_pair(e, 0.0, (s.saturating_sub(1)) as f32);

            if s <= 1 {
                stride *= s.max(1);
                continue;
            }

            let idx0 = (e_clamped.floor() as usize).min(s - 2);
            let frac = (e_clamped - (idx0 as f32)).clamp(0.0, 1.0);

            if frac <= 0.0 {
                base_offset += idx0 * stride;
            } else if frac >= 1.0 {
                base_offset += (idx0 + 1) * stride;
            } else if active_len < active_strides.len() {
                base_offset += idx0 * stride;
                active_strides[active_len] = stride;
                active_fracs[active_len] = frac;
                active_len += 1;
            } else {
                // Beyond 12 simultaneous fractional axes, snap nearest so evaluation stays O(1).
                let snapped = if frac >= 0.5 { idx0 + 1 } else { idx0 };
                base_offset += snapped * stride;
            }
            stride *= s;
        }

        let corners = 1_usize << active_len;
        for mask in 0..corners {
            let mut w = 1.0_f32;
            let mut off = base_offset;
            for a in 0..active_len {
                if (mask & (1 << a)) != 0 {
                    w *= active_fracs[a];
                    off += active_strides[a];
                } else {
                    w *= 1.0 - active_fracs[a];
                }
            }
            let slice = &self.samples[off..off + n];
            for (dst, &sv) in out.iter_mut().zip(slice) {
                *dst += w * sv;
            }
        }
    }
}
