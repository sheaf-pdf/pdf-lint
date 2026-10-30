//! Operands read when asked: a name, its `#` escapes decoded, and any
//! operand as a pdf-objects value.

use std::borrow::Cow;

use pdf_objects::{Dict, Form, Object, Str, Value};

use crate::scan::{hex_string, is_number, literal_string, skip_space, word_end};

/// The name an operand is, its `#` escapes decoded (ISO 32000-2, 7.3.5);
/// None when it isn't one. A `#` without two hex digits after it stays as
/// written. Borrowed where there's no `#` to decode.
///
/// ```
/// use pdf_content_streams::name;
///
/// assert_eq!(name(b"/F#311").as_deref(), Some(&b"F11"[..]));
/// assert_eq!(name(b"12"), None);
/// ```
pub fn name(operand: &[u8]) -> Option<Cow<'_, [u8]>> {
    let raw = operand.strip_prefix(b"/")?;
    if !raw.contains(&b'#') {
        return Some(Cow::Borrowed(raw));
    }
    let digit = |c: Option<&u8>| (*c? as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match (raw[i], digit(raw.get(i + 1)), digit(raw.get(i + 2))) {
            (b'#', Some(high), Some(low)) => {
                out.push(high << 4 | low);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(Cow::Owned(out))
}

/// An operand as a PDF object: a name's escapes decoded, a string's
/// escapes or hex digits read, and an array's or a dictionary's elements
/// each read so. None for what isn't an object, such as a stray closing
/// delimiter or a word of signs, and for one that doesn't end or nests
/// more than `depth` deep. A dictionary keeps every entry, a key written
/// twice too, as pdf-objects' [`Dict`] does, so a lookup takes its last;
/// an entry whose key isn't a name is passed over. A string keeps its
/// [`Form`]. Nothing read has a place ([`Object::at`] is none): an
/// operand's place is in the content, where its token's range is. It's
/// read in one pass, so a deep operand costs what a flat one of its size
/// does.
///
/// ```
/// use pdf_content_streams::object;
/// use pdf_objects::Object;
///
/// let read = object(b"<</MCID 3 /Lang (en)>>", 512).unwrap();
/// let mcid = read.as_dict().and_then(|d| d.get(b"MCID"));
/// assert_eq!(mcid.and_then(Object::as_integer), Some(3));
/// assert_eq!(object(b"--5", 512), None);
/// ```
pub fn object(operand: &[u8], depth: usize) -> Option<Object> {
    match item(operand, &mut 0, depth, None)? {
        Item::Value(object) => object,
        Item::Close => None,
    }
}

/// What's next in an operand read as an object.
enum Item {
    /// An object, or None for what isn't one.
    Value(Option<Object>),
    /// The delimiter that ends the array or dictionary being read.
    Close,
}

/// The item from `*at` in `s`, `*at` moved past it, inside the array or
/// dictionary that `close` ends, if one's open, `depth` more of which may
/// nest. None at the end, and for a string, array or dictionary that
/// doesn't end, or one nested too deep.
fn item(s: &[u8], at: &mut usize, depth: usize, close: Option<&[u8]>) -> Option<Item> {
    let start = skip_space(s, *at);
    let rest = s.get(start..)?;
    let closes = |delimiter: &[u8]| close == Some(delimiter);
    let (end, value) = match *rest.first()? {
        b'(' => {
            let end = literal_string(s, start).ok()?;
            let string = Value::String(Str::Bytes(literal(&s[start..end])), Some(Form::Literal));
            (end, Some(string))
        }
        b'<' if rest.starts_with(b"<<") => {
            *at = start + 2;
            return Some(Item::Value(Some(dictionary(s, at, depth.checked_sub(1)?)?)));
        }
        b'<' => {
            let end = hex_string(s, start).ok()?;
            let string = Value::String(Str::Bytes(hex(&s[start..end])), Some(Form::Hex));
            (end, Some(string))
        }
        b'[' => {
            *at = start + 1;
            return Some(Item::Value(Some(array(s, at, depth.checked_sub(1)?)?)));
        }
        b']' if closes(b"]") => {
            *at = start + 1;
            return Some(Item::Close);
        }
        b'>' if rest.starts_with(b">>") => {
            *at = start + 2;
            return Some(if closes(b">>") {
                Item::Close
            } else {
                Item::Value(None)
            });
        }
        b')' | b'>' | b']' | b'{' | b'}' => (start + 1, None),
        b'/' => {
            let end = word_end(s, start + 1);
            (end, Some(Value::Name(name(&s[start..end])?.into_owned())))
        }
        _ => {
            let end = word_end(s, start);
            (end, word(&s[start..end]))
        }
    };
    *at = end;
    Some(Item::Value(value.map(Object::new)))
}

/// The array whose `[` ends at `*at`: its elements read, to its `]`.
fn array(s: &[u8], at: &mut usize, depth: usize) -> Option<Object> {
    let mut elements = Vec::new();
    loop {
        match item(s, at, depth, Some(b"]")) {
            Some(Item::Value(element)) => elements.extend(element),
            Some(Item::Close) => return Some(Object::new(Value::Array(elements))),
            None => return None,
        }
    }
}

/// The dictionary whose `<<` ends at `*at`: its entries read, to its
/// `>>`.
fn dictionary(s: &[u8], at: &mut usize, depth: usize) -> Option<Object> {
    let mut dict = Dict::new();
    let next = |at: &mut usize| match item(s, at, depth, Some(b">>")) {
        Some(Item::Value(value)) => Some(Ok(value)),
        Some(Item::Close) => Some(Err(())),
        None => None,
    };
    loop {
        let Ok(key) = next(at)? else {
            return Some(Object::new(Value::Dict(dict)));
        };
        let Some(key) = key.and_then(|key| match key.into_value() {
            Value::Name(key) => Some(key),
            _ => None,
        }) else {
            continue;
        };
        match next(at)? {
            Ok(Some(value)) => dict.push(key, value),
            Ok(None) => {}
            Err(()) => return Some(Object::new(Value::Dict(dict))),
        }
    }
}

/// A word as an object: a number, `true`, `false` or `null`.
fn word(word: &[u8]) -> Option<Value> {
    Some(match word {
        b"true" => Value::Bool(true),
        b"false" => Value::Bool(false),
        b"null" => Value::Null,
        n if !is_number(n) => return None,
        n => {
            let n = std::str::from_utf8(n).ok()?;
            if n.contains('.') {
                Value::Real(n.parse().ok()?)
            } else {
                Value::Integer(n.parse().ok()?)
            }
        }
    })
}

/// A literal string's bytes, its escapes read: `(…)` as lexed.
fn literal(string: &[u8]) -> Vec<u8> {
    let inner = &string[1..string.len().saturating_sub(1).max(1)];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while let Some(&c) = inner.get(i) {
        i += 1;
        match c {
            b'\\' => {
                let Some(&e) = inner.get(i) else { break };
                i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'0'..=b'7' => {
                        let mut v = u32::from(e - b'0');
                        for _ in 0..2 {
                            match inner.get(i) {
                                Some(&d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as u8);
                    }
                    // A line continued: the end of line is no part of it.
                    b'\r' => i += usize::from(inner.get(i) == Some(&b'\n')),
                    b'\n' => {}
                    e => out.push(e),
                }
            }
            // Any end of line reads as a line feed.
            b'\r' => {
                i += usize::from(inner.get(i) == Some(&b'\n'));
                out.push(b'\n');
            }
            c => out.push(c),
        }
    }
    out
}

/// A hex string's bytes: `<…>` as lexed, what isn't a hex digit passed
/// over, an odd last digit followed by 0.
fn hex(string: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = string
        .iter()
        .filter_map(|&c| (c as char).to_digit(16).map(|d| d as u8))
        .collect();
    digits
        .chunks(2)
        .map(|pair| pair[0] << 4 | pair.get(1).copied().unwrap_or(0))
        .collect()
}
