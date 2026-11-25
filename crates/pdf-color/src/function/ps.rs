//! Bounded, recursion-free ISO 32000-2 §7.10.5 (Table 42) PostScript calculator parser and
//! stack-machine evaluator.

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

impl Val {
    #[inline]
    fn as_f32(self) -> Option<f32> {
        match self {
            Val::Int(i) => Some(i as f32),
            Val::Real(r) => Some(r),
            Val::Bool(_) | Val::Proc { .. } => None,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct Program {
    code: Vec<Op>,
}

impl Program {
    pub(super) fn len(&self) -> usize {
        self.code.len()
    }

    pub(super) fn parse(bytes: &[u8], limits: &Limits) -> Result<Program, PsParseError> {
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

        Ok(Program { code })
    }

    pub(super) fn exec(&self, inputs: &[f32], outputs: &mut [f32], limits: &Limits) -> bool {
        if inputs.len() > limits.postscript_stack || outputs.len() > limits.postscript_stack {
            return false;
        }
        let mut stack = Vec::with_capacity(inputs.len().max(outputs.len()).min(128));
        for &x in inputs {
            stack.push(Val::Real(sanitize_f32(x)));
        }

        let mut frames: Vec<(usize, usize)> = Vec::with_capacity(8);
        frames.push((0, self.code.len()));
        let mut steps = 0_usize;

        while let Some((pc, end)) = frames.last_mut() {
            if *pc >= *end {
                frames.pop();
                continue;
            }
            if steps >= limits.postscript_ops {
                return false;
            }
            steps += 1;

            let op = self.code[*pc];
            *pc += 1;

            let push = |stack: &mut Vec<Val>, v: Val| -> bool {
                if stack.len() >= limits.postscript_stack {
                    false
                } else {
                    stack.push(v);
                    true
                }
            };

            match op {
                Op::Int(i) => {
                    if !push(&mut stack, Val::Int(i)) {
                        return false;
                    }
                }
                Op::Real(r) => {
                    if !push(&mut stack, Val::Real(r)) {
                        return false;
                    }
                }
                Op::Bool(b) => {
                    if !push(&mut stack, Val::Bool(b)) {
                        return false;
                    }
                }
                Op::Proc { end_ip } => {
                    let proc_start = *pc;
                    *pc = end_ip;
                    if !push(
                        &mut stack,
                        Val::Proc {
                            start: proc_start,
                            end: end_ip,
                        },
                    ) {
                        return false;
                    }
                }
                Op::Abs => match stack.pop() {
                    Some(Val::Int(i)) => {
                        let v = i
                            .checked_abs()
                            .map_or_else(|| Val::Real(-(i as f32)), Val::Int);
                        stack.push(v);
                    }
                    Some(Val::Real(r)) => stack.push(Val::Real(sanitize_f32(r.abs()))),
                    _ => return false,
                },
                Op::Neg => match stack.pop() {
                    Some(Val::Int(i)) => {
                        let v = i
                            .checked_neg()
                            .map_or_else(|| Val::Real(-(i as f32)), Val::Int);
                        stack.push(v);
                    }
                    Some(Val::Real(r)) => stack.push(Val::Real(sanitize_f32(-r))),
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
                            stack.push(v);
                        }
                        _ => {
                            let (Some(fa), Some(fb)) = (a.as_f32(), b.as_f32()) else {
                                return false;
                            };
                            stack.push(Val::Real(sanitize_f32(fa + fb)));
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
                            stack.push(v);
                        }
                        _ => {
                            let (Some(fa), Some(fb)) = (a.as_f32(), b.as_f32()) else {
                                return false;
                            };
                            stack.push(Val::Real(sanitize_f32(fa - fb)));
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
                            stack.push(v);
                        }
                        _ => {
                            let (Some(fa), Some(fb)) = (a.as_f32(), b.as_f32()) else {
                                return false;
                            };
                            stack.push(Val::Real(sanitize_f32(fa * fb)));
                        }
                    }
                }
                Op::Div => {
                    let (Some(fb), Some(fa)) = (
                        stack.pop().and_then(Val::as_f32),
                        stack.pop().and_then(Val::as_f32),
                    ) else {
                        return false;
                    };
                    if fb == 0.0 {
                        return false;
                    }
                    stack.push(Val::Real(sanitize_f32(fa / fb)));
                }
                Op::Idiv => {
                    let (Some(Val::Int(ib)), Some(Val::Int(ia))) = (stack.pop(), stack.pop())
                    else {
                        return false;
                    };
                    if ib == 0 {
                        return false;
                    }
                    stack.push(Val::Int(ia.wrapping_div(ib)));
                }
                Op::Mod => {
                    let (Some(Val::Int(ib)), Some(Val::Int(ia))) = (stack.pop(), stack.pop())
                    else {
                        return false;
                    };
                    if ib == 0 {
                        return false;
                    }
                    stack.push(Val::Int(ia.wrapping_rem(ib)));
                }
                Op::Atan => {
                    let (Some(den), Some(num)) = (
                        stack.pop().and_then(Val::as_f32),
                        stack.pop().and_then(Val::as_f32),
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
                    stack.push(Val::Real(sanitize_f32(deg)));
                }
                Op::Ceiling => match stack.pop() {
                    Some(Val::Int(i)) => stack.push(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push(Val::Real(sanitize_f32(r.ceil()))),
                    _ => return false,
                },
                Op::Floor => match stack.pop() {
                    Some(Val::Int(i)) => stack.push(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push(Val::Real(sanitize_f32(r.floor()))),
                    _ => return false,
                },
                Op::Round => match stack.pop() {
                    Some(Val::Int(i)) => stack.push(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push(Val::Real(sanitize_f32((r + 0.5).floor()))),
                    _ => return false,
                },
                Op::Truncate => match stack.pop() {
                    Some(Val::Int(i)) => stack.push(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push(Val::Real(sanitize_f32(r.trunc()))),
                    _ => return false,
                },
                Op::Cvi => match stack.pop() {
                    Some(Val::Int(i)) => stack.push(Val::Int(i)),
                    Some(Val::Real(r)) => stack.push(Val::Int(r.trunc() as i32)),
                    _ => return false,
                },
                Op::Cvr => {
                    let Some(r) = stack.pop().and_then(Val::as_f32) else {
                        return false;
                    };
                    stack.push(Val::Real(sanitize_f32(r)));
                }
                Op::Cos => {
                    let Some(deg) = stack.pop().and_then(Val::as_f32) else {
                        return false;
                    };
                    stack.push(Val::Real(sanitize_f32(deg.to_radians().cos())));
                }
                Op::Sin => {
                    let Some(deg) = stack.pop().and_then(Val::as_f32) else {
                        return false;
                    };
                    stack.push(Val::Real(sanitize_f32(deg.to_radians().sin())));
                }
                Op::Exp => {
                    let (Some(exp), Some(base)) = (
                        stack.pop().and_then(Val::as_f32),
                        stack.pop().and_then(Val::as_f32),
                    ) else {
                        return false;
                    };
                    let val = base.powf(exp);
                    if !val.is_finite() {
                        return false;
                    }
                    stack.push(Val::Real(val));
                }
                Op::Ln => {
                    let Some(x) = stack.pop().and_then(Val::as_f32) else {
                        return false;
                    };
                    if x <= 0.0 {
                        return false;
                    }
                    stack.push(Val::Real(sanitize_f32(x.ln())));
                }
                Op::Log => {
                    let Some(x) = stack.pop().and_then(Val::as_f32) else {
                        return false;
                    };
                    if x <= 0.0 {
                        return false;
                    }
                    stack.push(Val::Real(sanitize_f32(x.log10())));
                }
                Op::Sqrt => {
                    let Some(x) = stack.pop().and_then(Val::as_f32) else {
                        return false;
                    };
                    if x < 0.0 {
                        return false;
                    }
                    stack.push(Val::Real(sanitize_f32(x.sqrt())));
                }
                Op::And => match (stack.pop(), stack.pop()) {
                    (Some(Val::Bool(b)), Some(Val::Bool(a))) => stack.push(Val::Bool(a && b)),
                    (Some(Val::Int(b)), Some(Val::Int(a))) => stack.push(Val::Int(a & b)),
                    _ => return false,
                },
                Op::Or => match (stack.pop(), stack.pop()) {
                    (Some(Val::Bool(b)), Some(Val::Bool(a))) => stack.push(Val::Bool(a || b)),
                    (Some(Val::Int(b)), Some(Val::Int(a))) => stack.push(Val::Int(a | b)),
                    _ => return false,
                },
                Op::Xor => match (stack.pop(), stack.pop()) {
                    (Some(Val::Bool(b)), Some(Val::Bool(a))) => stack.push(Val::Bool(a ^ b)),
                    (Some(Val::Int(b)), Some(Val::Int(a))) => stack.push(Val::Int(a ^ b)),
                    _ => return false,
                },
                Op::Not => match stack.pop() {
                    Some(Val::Bool(b)) => stack.push(Val::Bool(!b)),
                    Some(Val::Int(i)) => stack.push(Val::Int(!i)),
                    _ => return false,
                },
                Op::Bitshift => {
                    let (Some(Val::Int(shift)), Some(Val::Int(val))) = (stack.pop(), stack.pop())
                    else {
                        return false;
                    };
                    let u = val as u32;
                    let shifted = if shift >= 0 {
                        if shift >= 32 { 0 } else { u << (shift as u32) }
                    } else {
                        let r = shift.unsigned_abs();
                        if r >= 32 { 0 } else { u >> r }
                    };
                    stack.push(Val::Int(shifted as i32));
                }
                Op::Eq | Op::Ne => {
                    let (Some(b), Some(a)) = (stack.pop(), stack.pop()) else {
                        return false;
                    };
                    let eq = match (a, b) {
                        (Val::Bool(ba), Val::Bool(bb)) => ba == bb,
                        (Val::Int(ia), Val::Int(ib)) => ia == ib,
                        _ => match (a.as_f32(), b.as_f32()) {
                            (Some(fa), Some(fb)) => fa == fb,
                            _ => return false,
                        },
                    };
                    stack.push(Val::Bool(if matches!(op, Op::Eq) { eq } else { !eq }));
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
                            let (Some(fa), Some(fb)) = (a.as_f32(), b.as_f32()) else {
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
                    stack.push(Val::Bool(res));
                }
                Op::If => {
                    let (Some(Val::Proc { start, end }), Some(Val::Bool(cond))) =
                        (stack.pop(), stack.pop())
                    else {
                        return false;
                    };
                    if cond {
                        if frames.len() > limits.space_depth {
                            return false;
                        }
                        frames.push((start, end));
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
                        Some(Val::Bool(cond)),
                    ) = (stack.pop(), stack.pop(), stack.pop())
                    else {
                        return false;
                    };
                    if frames.len() > limits.space_depth {
                        return false;
                    }
                    if cond {
                        frames.push((t_start, t_end));
                    } else {
                        frames.push((f_start, f_end));
                    }
                }
                Op::Pop => {
                    if stack.pop().is_none() {
                        return false;
                    }
                }
                Op::Dup => {
                    let Some(&top) = stack.last() else {
                        return false;
                    };
                    if !push(&mut stack, top) {
                        return false;
                    }
                }
                Op::Exch => {
                    let len = stack.len();
                    if len < 2 {
                        return false;
                    }
                    stack.swap(len - 1, len - 2);
                }
                Op::Copy => {
                    let Some(Val::Int(n)) = stack.pop() else {
                        return false;
                    };
                    let Ok(n) = usize::try_from(n) else {
                        return false;
                    };
                    if n > stack.len() || stack.len() + n > limits.postscript_stack {
                        return false;
                    }
                    let start = stack.len() - n;
                    stack.extend_from_within(start..);
                }
                Op::Index => {
                    let Some(Val::Int(n)) = stack.pop() else {
                        return false;
                    };
                    let Ok(n) = usize::try_from(n) else {
                        return false;
                    };
                    if n >= stack.len() {
                        return false;
                    }
                    let val = stack[stack.len() - 1 - n];
                    if !push(&mut stack, val) {
                        return false;
                    }
                }
                Op::Roll => {
                    let (Some(Val::Int(j)), Some(Val::Int(n))) = (stack.pop(), stack.pop()) else {
                        return false;
                    };
                    if n < 0 {
                        return false;
                    }
                    let n_usize = n as usize;
                    if n_usize > stack.len() {
                        return false;
                    }
                    if n_usize > 1 {
                        let k = j.rem_euclid(n) as usize;
                        let start = stack.len() - n_usize;
                        stack[start..].rotate_right(k);
                    }
                }
            }
        }

        if stack.len() < outputs.len() {
            return false;
        }
        // ISO 32000-2 §7.10.5: after execution, the top `n` values (or exact `n` values) represent
        // `y_0 .. y_{n-1}` from bottom to top; PDFium pops `n` values from the top of the stack in
        // reverse (`outputs[n - 1]` from top down to `outputs[0]`).
        let base = stack.len() - outputs.len();
        for (out, &val) in outputs.iter_mut().zip(&stack[base..]) {
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
