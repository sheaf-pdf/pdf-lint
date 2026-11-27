//! Bounded, recursion-free ISO 32000-2 §7.10.5 (Table 42) PostScript calculator parser and
//! stack-machine evaluator.

use pdf_objects::Rules;

use crate::limits::Limits;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum PsParseError {
    Syntax,
    TooDeep,
    TooManyOps,
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Int(i32),
    Real(f32),
    Bool(bool),
    /// Pushes `Val::Proc { start: pc + 1, end: end_ip }` onto the operand stack and advances `pc`
    /// to `end_ip`.
    Proc {
        end_ip: usize,
    },
    // Arithmetic (21)
    Abs,
    Add,
    Atan,
    Ceiling,
    Cos,
    Cvi,
    Cvr,
    Div,
    Exp,
    Floor,
    Idiv,
    Ln,
    Log,
    Mod,
    Mul,
    Neg,
    Round,
    Sin,
    Sqrt,
    Sub,
    Truncate,
    // Relational, boolean, and bitwise (11 ops + true/false literals above = 13)
    And,
    Bitshift,
    Eq,
    Ge,
    Gt,
    Le,
    Lt,
    Ne,
    Not,
    Or,
    Xor,
    // Conditional (2)
    If,
    IfElse,
    // Stack (6)
    Copy,
    Dup,
    Exch,
    Index,
    Pop,
    Roll,
}

#[derive(Clone, Copy)]
enum Val {
    Int(i32),
    Real(f32),
    Bool(bool),
    Proc { start: usize, end: usize },
}

impl Default for Val {
    fn default() -> Self {
        Val::Int(0)
    }
}

impl Val {
    #[inline]
    fn as_num(self, pdfium: bool) -> Option<f32> {
        match self {
            Val::Int(i) => Some(i as f32),
            Val::Real(r) => Some(r),
            Val::Bool(b) if pdfium => Some(if b { 1.0 } else { 0.0 }),
            Val::Bool(_) | Val::Proc { .. } => None,
        }
    }

    #[inline]
    fn as_int(self, pdfium: bool) -> Option<i32> {
        match self {
            Val::Int(i) => Some(i),
            Val::Real(r) if pdfium => Some(if r.is_finite() { r.trunc() as i32 } else { 0 }),
            Val::Bool(b) if pdfium => Some(i32::from(b)),
            Val::Real(_) | Val::Bool(_) | Val::Proc { .. } => None,
        }
    }

    #[inline]
    fn as_cond(self, pdfium: bool) -> Option<bool> {
        match self {
            Val::Bool(b) => Some(b),
            Val::Int(i) if pdfium => Some(i != 0),
            Val::Real(r) if pdfium => Some(r != 0.0 && !r.is_nan()),
            Val::Int(_) | Val::Real(_) | Val::Proc { .. } => None,
        }
    }
}

/// Stack-resident buffer that spills to a heap `Vec` only when length exceeds `N`, while always
/// exposing a single contiguous slice.
struct SmallStack<T: Copy, const N: usize> {
    inline: [T; N],
    len: usize,
    spilled: Vec<T>,
}

impl<T: Copy + Default, const N: usize> SmallStack<T, N> {
    #[inline]
    fn new() -> Self {
        Self {
            inline: [T::default(); N],
            len: 0,
            spilled: Vec::new(),
        }
    }

    #[inline]
    fn len(&self) -> usize {
        if self.spilled.is_empty() {
            self.len
        } else {
            self.spilled.len()
        }
    }

    #[inline]
    fn push(&mut self, val: T, cap: usize) -> bool {
        let cur = self.len();
        if cur >= cap {
            return false;
        }
        if !self.spilled.is_empty() {
            self.spilled.push(val);
        } else if cur < N {
            self.inline[cur] = val;
            self.len = cur + 1;
        } else {
            self.spilled.reserve(cap.max(N + 1));
            self.spilled.extend_from_slice(&self.inline[..N]);
            self.spilled.push(val);
        }
        true
    }

    /// Pushes `val` when replacing an already-popped operand (so length cannot exceed prior cap).
    #[inline]
    fn push_within(&mut self, val: T) {
        if !self.spilled.is_empty() {
            self.spilled.push(val);
        } else {
            self.inline[self.len] = val;
            self.len += 1;
        }
    }

    #[inline]
    fn pop(&mut self) -> Option<T> {
        if !self.spilled.is_empty() {
            self.spilled.pop()
        } else if self.len > 0 {
            self.len -= 1;
            Some(self.inline[self.len])
        } else {
            None
        }
    }

    #[inline]
    fn as_slice(&self) -> &[T] {
        if self.spilled.is_empty() {
            &self.inline[..self.len]
        } else {
            &self.spilled
        }
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [T] {
        if self.spilled.is_empty() {
            &mut self.inline[..self.len]
        } else {
            &mut self.spilled
        }
    }

    #[inline]
    fn extend_from_within(&mut self, start: usize, cap: usize) -> bool {
        let cur = self.len();
        let n = cur - start;
        if cur + n > cap {
            return false;
        }
        if !self.spilled.is_empty() {
            self.spilled.extend_from_within(start..);
        } else if cur + n <= N {
            self.inline.copy_within(start..cur, cur);
            self.len = cur + n;
        } else {
            self.spilled.reserve(cap.max(cur + n));
            self.spilled.extend_from_slice(&self.inline[..cur]);
            self.spilled.extend_from_within(start..);
        }
        true
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct Program {
    code: Vec<Op>,
    max_ops: usize,
    max_stack: usize,
    max_depth: usize,
    rules: Rules,
}

impl Program {
    pub(super) fn len(&self) -> usize {
        self.code.len()
    }

    pub(super) fn parse(
        bytes: &[u8],
        rules: Rules,
        limits: &Limits,
    ) -> Result<Program, PsParseError> {
        let mut cursor = skip_ws_and_comments(bytes, 0);
        if bytes.get(cursor) != Some(&b'{') {
            return Err(PsParseError::Syntax);
        }
        cursor += 1;

        let mut code = Vec::new();
        // `None` marks the outermost `{ ... }`; `Some(ip)` marks nested `{ ... }` procedures.
        let mut braces: Vec<Option<usize>> = vec![None];
        let mut closed_outer = false;

        while cursor < bytes.len() {
            cursor = skip_ws_and_comments(bytes, cursor);
            if cursor >= bytes.len() {
                break;
            }
            let b = bytes[cursor];
            if b == b'{' {
                if braces.len() > limits.space_depth {
                    return Err(PsParseError::TooDeep);
                }
                if code.len() >= limits.postscript_ops {
                    return Err(PsParseError::TooManyOps);
                }
                let ip = code.len();
                code.push(Op::Proc { end_ip: 0 });
                braces.push(Some(ip));
                cursor += 1;
                continue;
            }
            if b == b'}' {
                cursor += 1;
                let Some(entry) = braces.pop() else {
                    return Err(PsParseError::Syntax);
                };
                match entry {
                    Some(ip) => {
                        let end_ip = code.len();
                        code[ip] = Op::Proc { end_ip };
                    }
                    None => {
                        closed_outer = true;
                        break;
                    }
                }
                continue;
            }
            if is_ps_delimiter(b) {
                return Err(PsParseError::Syntax);
            }
            let start = cursor;
            while cursor < bytes.len()
                && !is_ps_whitespace(bytes[cursor])
                && !is_ps_delimiter(bytes[cursor])
            {
                cursor += 1;
            }
            let token = &bytes[start..cursor];
            if code.len() >= limits.postscript_ops {
                return Err(PsParseError::TooManyOps);
            }
            let op = parse_token(token).ok_or(PsParseError::Syntax)?;
            code.push(op);
        }

        if !closed_outer || !braces.is_empty() {
            return Err(PsParseError::Syntax);
        }
        // Only whitespace and comments may trail the outer `}`.
        if skip_ws_and_comments(bytes, cursor) != bytes.len() {
            return Err(PsParseError::Syntax);
        }

        Ok(Program {
            code,
            max_ops: limits.postscript_ops,
            max_stack: limits.postscript_stack,
            max_depth: limits.space_depth,
            rules,
        })
    }

    pub(super) fn exec(
        &self,
        inputs: &[f32],
        outputs: &mut [f32],
        rules_override: Option<Rules>,
    ) -> bool {
        let max_stack = self.max_stack;
        if inputs.len() > max_stack || outputs.len() > max_stack {
            return false;
        }
        let pdfium = rules_override.unwrap_or(self.rules) != Rules::Iso;

        let mut stack: SmallStack<Val, 128> = SmallStack::new();
        for &x in inputs {
            if !stack.push(Val::Real(sanitize_f32(x)), max_stack) {
                return false;
            }
        }

        let frame_cap = self.max_depth.saturating_add(2);
        let mut frames: SmallStack<(usize, usize), 66> = SmallStack::new();
        let _ = frames.push((0, self.code.len()), frame_cap.max(2));
        let mut steps = 0_usize;

        while let Some((pc, end)) = frames.as_mut_slice().last_mut() {
            if *pc >= *end {
                frames.pop();
                continue;
            }
            if steps >= self.max_ops {
                return false;
            }
            steps += 1;

            let op = self.code[*pc];
            *pc += 1;

            match op {
                Op::Int(i) => {
                    if !stack.push(Val::Int(i), max_stack) {
                        return false;
                    }
                }
                Op::Real(r) => {
                    if !stack.push(Val::Real(r), max_stack) {
                        return false;
                    }
                }
                Op::Bool(b) => {
                    if !stack.push(Val::Bool(b), max_stack) {
                        return false;
                    }
                }
                Op::Proc { end_ip } => {
                    let proc_start = *pc;
                    *pc = end_ip;
                    if !stack.push(
                        Val::Proc {
                            start: proc_start,
                            end: end_ip,
                        },
                        max_stack,
                    ) {
                        return false;
                    }
                }
                Op::Abs => match stack.pop() {
                    Some(Val::Int(i)) => {
                        let v = i
                            .checked_abs()
                            .map_or_else(|| Val::Real(-(i as f32)), Val::Int);
                        stack.push_within(v);
                    }
                    Some(Val::Real(r)) => stack.push_within(Val::Real(sanitize_f32(r.abs()))),
                    Some(Val::Bool(b)) if pdfium => stack.push_within(Val::Int(i32::from(b))),
                    _ => return false,
                },
                Op::Neg => match stack.pop() {
                    Some(Val::Int(i)) => {
                        let v = i
                            .checked_neg()
                            .map_or_else(|| Val::Real(-(i as f32)), Val::Int);
                        stack.push_within(v);
                    }
                    Some(Val::Real(r)) => stack.push_within(Val::Real(sanitize_f32(-r))),
                    Some(Val::Bool(b)) if pdfium => stack.push_within(Val::Int(-i32::from(b))),
                    _ => return false,
                },
                Op::Add => {
                    let (Some(b), Some(a)) = (stack.pop(), stack.pop()) else {
                        return false;
                    };
                    match (a, b) {
                        (Val::Int(ia), Val::Int(ib)) => {
                            let v = ia
                                .checked_add(ib)
                                .map_or_else(|| Val::Real((ia as f32) + (ib as f32)), Val::Int);
                            stack.push_within(v);
                        }
                        _ => {
                            let (Some(fa), Some(fb)) = (a.as_num(pdfium), b.as_num(pdfium)) else {
                                return false;
                            };
                            stack.push_within(Val::Real(sanitize_f32(fa + fb)));
                        }
                    }
                }
                Op::Sub => {
                    let (Some(b), Some(a)) = (stack.pop(), stack.pop()) else {
                        return false;
                    };
                    match (a, b) {
                        (Val::Int(ia), Val::Int(ib)) => {
                            let v = ia
                                .checked_sub(ib)
                                .map_or_else(|| Val::Real((ia as f32) - (ib as f32)), Val::Int);
                            stack.push_within(v);
                        }
                        _ => {
                            let (Some(fa), Some(fb)) = (a.as_num(pdfium), b.as_num(pdfium)) else {
                                return false;
                            };
                            stack.push_within(Val::Real(sanitize_f32(fa - fb)));
                        }
                    }
                }
                Op::Mul => {
                    let (Some(b), Some(a)) = (stack.pop(), stack.pop()) else {
                        return false;
                    };
                    match (a, b) {
                        (Val::Int(ia), Val::Int(ib)) => {
                            let v = ia
                                .checked_mul(ib)
                                .map_or_else(|| Val::Real((ia as f32) * (ib as f32)), Val::Int);
                            stack.push_within(v);
                        }
                        _ => {
                            let (Some(fa), Some(fb)) = (a.as_num(pdfium), b.as_num(pdfium)) else {
                                return false;
                            };
                            stack.push_within(Val::Real(sanitize_f32(fa * fb)));
                        }
                    }
                }
                Op::Div => {
                    let (Some(fb), Some(fa)) = (
                        stack.pop().and_then(|v| v.as_num(pdfium)),
                        stack.pop().and_then(|v| v.as_num(pdfium)),
                    ) else {
                        return false;
                    };
                    if fb == 0.0 {
                        return false;
                    }
                    stack.push_within(Val::Real(sanitize_f32(fa / fb)));
                }
                Op::Idiv => {
                    let (Some(ib), Some(ia)) = (
                        stack.pop().and_then(|v| v.as_int(pdfium)),
                        stack.pop().and_then(|v| v.as_int(pdfium)),
                    ) else {
                        return false;
                    };
                    if ib == 0 {
                        return false;
                    }
                    stack.push_within(Val::Int(ia.wrapping_div(ib)));
                }
                Op::Mod => {
                    let (Some(ib), Some(ia)) = (
                        stack.pop().and_then(|v| v.as_int(pdfium)),
                        stack.pop().and_then(|v| v.as_int(pdfium)),
                    ) else {
                        return false;
                    };
                    if ib == 0 {
                        return false;
                    }
                    stack.push_within(Val::Int(ia.wrapping_rem(ib)));
                }
                Op::Atan => {
                    let (Some(den), Some(num)) = (
                        stack.pop().and_then(|v| v.as_num(pdfium)),
                        stack.pop().and_then(|v| v.as_num(pdfium)),
                    ) else {
                        return false;
                    };
                    if num == 0.0 && den == 0.0 {
                        return false;
                    }
                    let mut deg = num.atan2(den).to_degrees();
                    if deg < 0.0 {
                        deg += 360.0;
                    }
                    if deg >= 360.0 {
                        deg -= 360.0;
                    }
                    stack.push_within(Val::Real(sanitize_f32(deg)));
                }
                Op::Ceiling => match stack.pop() {
                    Some(Val::Int(i)) => stack.push_within(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push_within(Val::Real(sanitize_f32(r.ceil()))),
                    Some(Val::Bool(b)) if pdfium => stack.push_within(Val::Int(i32::from(b))),
                    _ => return false,
                },
                Op::Floor => match stack.pop() {
                    Some(Val::Int(i)) => stack.push_within(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push_within(Val::Real(sanitize_f32(r.floor()))),
                    Some(Val::Bool(b)) if pdfium => stack.push_within(Val::Int(i32::from(b))),
                    _ => return false,
                },
                Op::Round => match stack.pop() {
                    Some(Val::Int(i)) => stack.push_within(Val::Int(i)),
                    Some(Val::Real(r)) => {
                        stack.push_within(Val::Real(sanitize_f32((r + 0.5).floor())));
                    }
                    Some(Val::Bool(b)) if pdfium => stack.push_within(Val::Int(i32::from(b))),
                    _ => return false,
                },
                Op::Truncate => match stack.pop() {
                    Some(Val::Int(i)) => stack.push_within(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push_within(Val::Real(sanitize_f32(r.trunc()))),
                    Some(Val::Bool(b)) if pdfium => stack.push_within(Val::Int(i32::from(b))),
                    _ => return false,
                },
                Op::Cvi => match stack.pop() {
                    Some(Val::Int(i)) => stack.push_within(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push_within(Val::Int(r.trunc() as i32)),
                    Some(Val::Bool(b)) if pdfium => stack.push_within(Val::Int(i32::from(b))),
                    _ => return false,
                },
                Op::Cvr => {
                    let Some(r) = stack.pop().and_then(|v| v.as_num(pdfium)) else {
                        return false;
                    };
                    stack.push_within(Val::Real(sanitize_f32(r)));
                }
                Op::Cos => {
                    let Some(deg) = stack.pop().and_then(|v| v.as_num(pdfium)) else {
                        return false;
                    };
                    stack.push_within(Val::Real(sanitize_f32(deg.to_radians().cos())));
                }
                Op::Sin => {
                    let Some(deg) = stack.pop().and_then(|v| v.as_num(pdfium)) else {
                        return false;
                    };
                    stack.push_within(Val::Real(sanitize_f32(deg.to_radians().sin())));
                }
                Op::Exp => {
                    let (Some(exp), Some(base)) = (
                        stack.pop().and_then(|v| v.as_num(pdfium)),
                        stack.pop().and_then(|v| v.as_num(pdfium)),
                    ) else {
                        return false;
                    };
                    let val = base.powf(exp);
                    if !val.is_finite() {
                        return false;
                    }
                    stack.push_within(Val::Real(val));
                }
                Op::Ln => {
                    let Some(x) = stack.pop().and_then(|v| v.as_num(pdfium)) else {
                        return false;
                    };
                    if x <= 0.0 {
                        return false;
                    }
                    stack.push_within(Val::Real(sanitize_f32(x.ln())));
                }
                Op::Log => {
                    let Some(x) = stack.pop().and_then(|v| v.as_num(pdfium)) else {
                        return false;
                    };
                    if x <= 0.0 {
                        return false;
                    }
                    stack.push_within(Val::Real(sanitize_f32(x.log10())));
                }
                Op::Sqrt => {
                    let Some(x) = stack.pop().and_then(|v| v.as_num(pdfium)) else {
                        return false;
                    };
                    if x < 0.0 {
                        return false;
                    }
                    stack.push_within(Val::Real(sanitize_f32(x.sqrt())));
                }
                Op::And => match (stack.pop(), stack.pop()) {
                    (Some(Val::Bool(b)), Some(Val::Bool(a))) => {
                        stack.push_within(Val::Bool(a && b));
                    }
                    (Some(Val::Int(b)), Some(Val::Int(a))) => {
                        stack.push_within(Val::Int(a & b));
                    }
                    (Some(b), Some(a)) if pdfium => {
                        let (Some(ib), Some(ia)) = (b.as_int(true), a.as_int(true)) else {
                            return false;
                        };
                        stack.push_within(Val::Int(ia & ib));
                    }
                    _ => return false,
                },
                Op::Or => match (stack.pop(), stack.pop()) {
                    (Some(Val::Bool(b)), Some(Val::Bool(a))) => {
                        stack.push_within(Val::Bool(a || b));
                    }
                    (Some(Val::Int(b)), Some(Val::Int(a))) => {
                        stack.push_within(Val::Int(a | b));
                    }
                    (Some(b), Some(a)) if pdfium => {
                        let (Some(ib), Some(ia)) = (b.as_int(true), a.as_int(true)) else {
                            return false;
                        };
                        stack.push_within(Val::Int(ia | ib));
                    }
                    _ => return false,
                },
                Op::Xor => match (stack.pop(), stack.pop()) {
                    (Some(Val::Bool(b)), Some(Val::Bool(a))) => {
                        stack.push_within(Val::Bool(a ^ b));
                    }
                    (Some(Val::Int(b)), Some(Val::Int(a))) => {
                        stack.push_within(Val::Int(a ^ b));
                    }
                    (Some(b), Some(a)) if pdfium => {
                        let (Some(ib), Some(ia)) = (b.as_int(true), a.as_int(true)) else {
                            return false;
                        };
                        stack.push_within(Val::Int(ia ^ ib));
                    }
                    _ => return false,
                },
                Op::Not => match stack.pop() {
                    Some(Val::Bool(b)) => stack.push_within(Val::Bool(!b)),
                    Some(Val::Int(i)) => stack.push_within(Val::Int(!i)),
                    // PDFium (`CPDF_PSEngine::DoOperator(PSOP_NOT)`) pushes `!PopInt()`, so `0.0`
                    // becomes `1` (true) and any non-zero truncated integer becomes `0` (false).
                    Some(Val::Real(r)) if pdfium => {
                        let i = if r.is_finite() { r.trunc() as i32 } else { 0 };
                        stack.push_within(Val::Bool(i == 0));
                    }
                    _ => return false,
                },
                Op::Bitshift => {
                    let (Some(shift), Some(val)) = (
                        stack.pop().and_then(|v| v.as_int(pdfium)),
                        stack.pop().and_then(|v| v.as_int(pdfium)),
                    ) else {
                        return false;
                    };
                    let u = val as u32;
                    let shifted = if shift >= 0 {
                        if shift >= 32 { 0 } else { u << (shift as u32) }
                    } else {
                        let r = shift.unsigned_abs();
                        if r >= 32 { 0 } else { u >> r }
                    };
                    stack.push_within(Val::Int(shifted as i32));
                }
                Op::Eq | Op::Ne => {
                    let (Some(b), Some(a)) = (stack.pop(), stack.pop()) else {
                        return false;
                    };
                    let eq = match (a, b) {
                        (Val::Bool(ba), Val::Bool(bb)) => ba == bb,
                        (Val::Int(ia), Val::Int(ib)) => ia == ib,
                        _ => match (a.as_num(pdfium), b.as_num(pdfium)) {
                            (Some(fa), Some(fb)) => fa == fb,
                            _ => return false,
                        },
                    };
                    stack.push_within(Val::Bool(if matches!(op, Op::Eq) { eq } else { !eq }));
                }
                Op::Gt | Op::Ge | Op::Lt | Op::Le => {
                    let (Some(b), Some(a)) = (stack.pop(), stack.pop()) else {
                        return false;
                    };
                    let res = match (a, b) {
                        (Val::Int(ia), Val::Int(ib)) => match op {
                            Op::Gt => ia > ib,
                            Op::Ge => ia >= ib,
                            Op::Lt => ia < ib,
                            _ => ia <= ib,
                        },
                        _ => {
                            let (Some(fa), Some(fb)) = (a.as_num(pdfium), b.as_num(pdfium)) else {
                                return false;
                            };
                            match op {
                                Op::Gt => fa > fb,
                                Op::Ge => fa >= fb,
                                Op::Lt => fa < fb,
                                _ => fa <= fb,
                            }
                        }
                    };
                    stack.push_within(Val::Bool(res));
                }
                Op::If => {
                    let (Some(Val::Proc { start, end }), Some(cond)) =
                        (stack.pop(), stack.pop().and_then(|v| v.as_cond(pdfium)))
                    else {
                        return false;
                    };
                    if cond {
                        if frames.len() > self.max_depth {
                            return false;
                        }
                        if !frames.push((start, end), frame_cap.max(2)) {
                            return false;
                        }
                    }
                }
                Op::IfElse => {
                    let (
                        Some(Val::Proc {
                            start: f_start,
                            end: f_end,
                        }),
                        Some(Val::Proc {
                            start: t_start,
                            end: t_end,
                        }),
                        Some(cond),
                    ) = (
                        stack.pop(),
                        stack.pop(),
                        stack.pop().and_then(|v| v.as_cond(pdfium)),
                    )
                    else {
                        return false;
                    };
                    if frames.len() > self.max_depth {
                        return false;
                    }
                    let target = if cond {
                        (t_start, t_end)
                    } else {
                        (f_start, f_end)
                    };
                    if !frames.push(target, frame_cap.max(2)) {
                        return false;
                    }
                }
                Op::Pop => {
                    if stack.pop().is_none() {
                        return false;
                    }
                }
                Op::Dup => {
                    let Some(&top) = stack.as_slice().last() else {
                        return false;
                    };
                    if !stack.push(top, max_stack) {
                        return false;
                    }
                }
                Op::Exch => {
                    let slice = stack.as_mut_slice();
                    let len = slice.len();
                    if len < 2 {
                        return false;
                    }
                    slice.swap(len - 1, len - 2);
                }
                Op::Copy => {
                    let Some(n) = stack.pop().and_then(|v| v.as_int(pdfium)) else {
                        return false;
                    };
                    let Ok(n) = usize::try_from(n) else {
                        return false;
                    };
                    if n > stack.len() {
                        return false;
                    }
                    let start = stack.len() - n;
                    if !stack.extend_from_within(start, max_stack) {
                        return false;
                    }
                }
                Op::Index => {
                    let Some(n) = stack.pop().and_then(|v| v.as_int(pdfium)) else {
                        return false;
                    };
                    let Ok(n) = usize::try_from(n) else {
                        return false;
                    };
                    let slice = stack.as_slice();
                    if n >= slice.len() {
                        return false;
                    }
                    let val = slice[slice.len() - 1 - n];
                    if !stack.push(val, max_stack) {
                        return false;
                    }
                }
                Op::Roll => {
                    let (Some(j), Some(n)) = (
                        stack.pop().and_then(|v| v.as_int(pdfium)),
                        stack.pop().and_then(|v| v.as_int(pdfium)),
                    ) else {
                        return false;
                    };
                    if n < 0 {
                        return false;
                    }
                    let n_usize = n as usize;
                    let slice = stack.as_mut_slice();
                    if n_usize > slice.len() {
                        return false;
                    }
                    if n_usize > 1 {
                        let k = j.rem_euclid(n) as usize;
                        let start = slice.len() - n_usize;
                        slice[start..].rotate_right(k);
                    }
                }
            }
        }

        let slice = stack.as_slice();
        if slice.len() < outputs.len() {
            return false;
        }
        // ISO 32000-2 §7.10.5: after execution, the top `n` values represent `y_0 .. y_{n-1}` from
        // bottom to top; PDFium pops `n` values from the top of the stack in reverse
        // (`outputs[n - 1]` from top down to `outputs[0]`).
        let base = slice.len() - outputs.len();
        for (out, &val) in outputs.iter_mut().zip(&slice[base..]) {
            match val {
                Val::Int(i) => *out = i as f32,
                Val::Real(r) => *out = sanitize_f32(r),
                Val::Bool(b) => *out = if b { 1.0 } else { 0.0 },
                Val::Proc { .. } => return false,
            }
        }
        true
    }
}

#[inline]
fn sanitize_f32(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

#[inline]
fn is_ps_whitespace(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

#[inline]
fn is_ps_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn skip_ws_and_comments(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        if is_ps_whitespace(bytes[i]) {
            i += 1;
        } else if bytes[i] == b'%' {
            while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                i += 1;
            }
        } else {
            break;
        }
    }
    i
}

fn parse_token(tok: &[u8]) -> Option<Op> {
    Some(match tok {
        b"true" => Op::Bool(true),
        b"false" => Op::Bool(false),
        b"abs" => Op::Abs,
        b"add" => Op::Add,
        b"atan" => Op::Atan,
        b"ceiling" => Op::Ceiling,
        b"cos" => Op::Cos,
        b"cvi" => Op::Cvi,
        b"cvr" => Op::Cvr,
        b"div" => Op::Div,
        b"exp" => Op::Exp,
        b"floor" => Op::Floor,
        b"idiv" => Op::Idiv,
        b"ln" => Op::Ln,
        b"log" => Op::Log,
        b"mod" => Op::Mod,
        b"mul" => Op::Mul,
        b"neg" => Op::Neg,
        b"round" => Op::Round,
        b"sin" => Op::Sin,
        b"sqrt" => Op::Sqrt,
        b"sub" => Op::Sub,
        b"truncate" => Op::Truncate,
        b"and" => Op::And,
        b"bitshift" => Op::Bitshift,
        b"eq" => Op::Eq,
        b"ge" => Op::Ge,
        b"gt" => Op::Gt,
        b"le" => Op::Le,
        b"lt" => Op::Lt,
        b"ne" => Op::Ne,
        b"not" => Op::Not,
        b"or" => Op::Or,
        b"xor" => Op::Xor,
        b"if" => Op::If,
        b"ifelse" => Op::IfElse,
        b"copy" => Op::Copy,
        b"dup" => Op::Dup,
        b"exch" => Op::Exch,
        b"index" => Op::Index,
        b"pop" => Op::Pop,
        b"roll" => Op::Roll,
        _ => return parse_number(tok),
    })
}

fn parse_number(tok: &[u8]) -> Option<Op> {
    let s = std::str::from_utf8(tok).ok()?;
    // PostScript radix integer `base#digits` (`2..=36`).
    if let Some((radix_s, digits)) = s.split_once('#') {
        let radix: u32 = radix_s.parse().ok()?;
        if !(2..=36).contains(&radix) || digits.is_empty() {
            return None;
        }
        let val = u32::from_str_radix(digits, radix).ok()?;
        return Some(Op::Int(val as i32));
    }
    if !s.contains(['.', 'e', 'E'])
        && let Ok(i) = s.parse::<i32>()
    {
        return Some(Op::Int(i));
    }
    let f: f32 = s.parse().ok()?;
    f.is_finite().then_some(Op::Real(f))
}
