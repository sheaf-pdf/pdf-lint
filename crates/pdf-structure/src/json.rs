//! A reader of qpdf's JSON, and of base64, without a dependency. It's
//! lenient where qpdf's output isn't JSON: qpdf copies a real as the file
//! wrote it, and `+.5` isn't a JSON number, so a number here is any run of
//! signs, digits, points and exponents, kept as its text.

/// A JSON value. An object keeps its members in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    /// As written.
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// How deeply arrays and objects may nest: qpdf's nest as deep as the
/// file's objects do, which the bytes reader caps at 256 too. It's read
/// recursively, so the cap also bounds the stack.
const DEPTH: usize = 256;

impl Json {
    /// An object's member `key`.
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// An array's element `i`.
    pub(crate) fn at(&self, i: usize) -> Option<&Json> {
        match self {
            Json::Array(elements) => elements.get(i),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }
}

/// `text`, one JSON value and white-space around it.
pub(crate) fn parse(text: &[u8]) -> Result<Json, String> {
    let mut p = Parser { text, at: 0 };
    let value = p.value(0)?;
    p.space();
    if p.at < text.len() {
        return Err(format!("JSON goes on after its value, at byte {}", p.at));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self
            .text
            .get(self.at)
            .is_some_and(|b| b" \t\r\n".contains(b))
        {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn eat(&mut self, word: &[u8]) -> bool {
        let ate = self.text[self.at..].starts_with(word);
        if ate {
            self.at += word.len();
        }
        ate
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > DEPTH {
            return Err(format!("JSON nested deeper than {DEPTH}"));
        }
        self.space();
        let at = self.at;
        match self.peek().ok_or("JSON ends where a value should be")? {
            b'{' => {
                self.at += 1;
                let mut members = Vec::new();
                self.space();
                if self.eat(b"}") {
                    return Ok(Json::Object(members));
                }
                loop {
                    self.space();
                    if self.peek() != Some(b'"') {
                        return Err(format!("a JSON key expected at byte {}", self.at));
                    }
                    let key = self.string()?;
                    self.space();
                    if !self.eat(b":") {
                        return Err(format!("a `:` expected at byte {}", self.at));
                    }
                    let value = self.value(depth + 1)?;
                    members.push((key, value));
                    self.space();
                    if self.eat(b"}") {
                        return Ok(Json::Object(members));
                    }
                    if !self.eat(b",") {
                        return Err(format!("a `,` or `}}` expected at byte {}", self.at));
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut elements = Vec::new();
                self.space();
                if self.eat(b"]") {
                    return Ok(Json::Array(elements));
                }
                loop {
                    elements.push(self.value(depth + 1)?);
                    self.space();
                    if self.eat(b"]") {
                        return Ok(Json::Array(elements));
                    }
                    if !self.eat(b",") {
                        return Err(format!("a `,` or `]` expected at byte {}", self.at));
                    }
                }
            }
            b'"' => self.string().map(Json::String),
            b't' if self.eat(b"true") => Ok(Json::Bool(true)),
            b'f' if self.eat(b"false") => Ok(Json::Bool(false)),
            b'n' if self.eat(b"null") => Ok(Json::Null),
            b'+' | b'-' | b'.' | b'0'..=b'9' => {
                let len = (self.text[at..].iter())
                    .take_while(|b| matches!(b, b'+' | b'-' | b'.' | b'e' | b'E' | b'0'..=b'9'))
                    .count();
                self.at += len;
                let number = String::from_utf8_lossy(&self.text[at..self.at]).into_owned();
                Ok(Json::Number(number))
            }
            _ => Err(format!("no JSON value at byte {at}")),
        }
    }

    /// A string, at its opening quote.
    fn string(&mut self) -> Result<String, String> {
        let start = self.at;
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let b = self
                .peek()
                .ok_or_else(|| format!("a JSON string at byte {start} that doesn't end"))?;
            self.at += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let escaped = self.peek().ok_or("JSON ends in an escape")?;
                    self.at += 1;
                    match escaped {
                        b'"' | b'\\' | b'/' => out.push(escaped),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut unit = self.unit()?;
                            // A high surrogate, then its low one.
                            if (0xD800..0xDC00).contains(&unit) && self.eat(b"\\u") {
                                let low = self.unit()?;
                                unit = 0x10000
                                    + ((unit - 0xD800) << 10)
                                    + (low.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            let c = char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER);
                            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        other => return Err(format!("`\\{}` isn't a JSON escape", other as char)),
                    }
                }
                _ => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| format!("a JSON string at byte {start} isn't UTF-8"))
    }

    /// The four hex digits of a `\u` escape.
    fn unit(&mut self) -> Result<u32, String> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .ok_or("a \\u escape cut short")?;
        let digits = std::str::from_utf8(digits).map_err(|_| "a \\u escape that isn't hex")?;
        let unit = u32::from_str_radix(digits, 16).map_err(|_| format!("\\u{digits} isn't hex"))?;
        self.at += 4;
        Ok(unit)
    }
}

/// `text`, base64 (RFC 4648, with `=` padding), decoded.
pub(crate) fn base64(text: &str) -> Result<Vec<u8>, String> {
    let value = |b: u8| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let text = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut held) = (0u32, 0);
    for &b in text {
        let v = value(b).ok_or_else(|| format!("{:?} isn't base64", b as char))?;
        bits = (bits << 6) | u32::from(v);
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push((bits >> held) as u8);
            bits &= (1 << held) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_reads_as_qpdf_writes_it() {
        let json = parse(br#" {"a": [1, -0.5, +.5, 4.0, true, false, null], "b": {"c": "x\"\n\u00e9\ud83d\ude00"}} "#)
            .unwrap();
        let numbers: Vec<_> = (0..4)
            .map(|i| match json.get("a").and_then(|a| a.at(i)) {
                Some(Json::Number(n)) => n.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            numbers,
            ["1", "-0.5", "+.5", "4.0"],
            "+.5, as qpdf writes it"
        );
        assert_eq!(json.get("a").and_then(|a| a.at(4)), Some(&Json::Bool(true)));
        assert_eq!(json.get("a").and_then(|a| a.at(6)), Some(&Json::Null));
        let c = json
            .get("b")
            .and_then(|b| b.get("c"))
            .and_then(Json::as_str);
        assert_eq!(c, Some("x\"\né😀"));
        assert_eq!(parse(b"{}").unwrap(), Json::Object(Vec::new()));
        for bad in [
            &b"{"[..],
            b"[1,]",
            b"{\"a\" 1}",
            b"\"open",
            b"[1] 2",
            b"nul",
            b"\"\\q\"",
        ] {
            assert!(parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        let deep = format!("{}{}", "[".repeat(2000), "]".repeat(2000));
        assert!(parse(deep.as_bytes()).is_err(), "too deep");
    }

    #[test]
    fn base64_decodes() {
        assert_eq!(base64("").unwrap(), b"");
        assert_eq!(base64("Zg==").unwrap(), b"f");
        assert_eq!(base64("Zm8=").unwrap(), b"fo");
        assert_eq!(base64("Zm9v").unwrap(), b"foo");
        assert_eq!(base64("MyAwIDQgNDQK").unwrap(), b"3 0 4 44\n");
        assert_eq!(base64("AP8=").unwrap(), [0x00, 0xFF]);
        assert!(base64("Zm9v!").is_err());
    }
}
