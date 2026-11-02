//! A Type 1 program read as far as its charstrings (ISO 32000-2, 9.6.2.1): its
//! tokens as read-fonts' `Type1Font` reads them (`RawDicts`, its `Parser`),
//! counted, and its charstrings by glyph name, which read-fonts keeps to itself.
//!
//! read-fonts reads a program's tokens in time that grows as the program does, but
//! for its `/Encoding`, each of whose names it looks for among all the glyphs, one by
//! one, however many it's given. The scan counts that with its own reading
//! ([`Limits::type1_work`]), so that a reader of programs need hand it only a program
//! within that. The charstrings found can be evaluated through read-fonts'
//! interpreter within fuel of the caller's own, and a multiple master font's
//! `/BuildCharArray` is read where FreeType reads it, the private dictionary, for it.

use std::collections::HashMap;

use crate::Limits;

/// A Type 1 program's charstrings by glyph name, each decrypted and past
/// its `lenIV` bytes, as read-fonts reads them (`read_charstrings`): a name
/// that isn't UTF-8 by its bytes as Latin-1 (`decode_name`), None for a name
/// the program gives twice, whose glyph can't be told from another's by
/// name. A program without `.notdef` has the one read-fonts makes up.
pub type Charstrings = HashMap<Vec<u8>, Option<Vec<u8>>>;

/// `data`, a Type 1 program, scanned as read-fonts reads it: its
/// charstrings, where reading it costs within [`Limits::type1_work`]; None where
/// not, or where read-fonts wouldn't read it either: no header, no `eexec`, no
/// `/CharStrings`.
pub fn scan(data: &[u8], limits: &Limits) -> Option<Charstrings> {
    read(data, limits).map(|scanned| scanned.charstrings)
}

/// What [`scan`] reads of a program.
pub(crate) struct Scanned {
    pub(crate) charstrings: Charstrings,
    /// How long its private dictionary's `/BuildCharArray` is, as FreeType reads it
    /// (`parse_buildchar`): read-fonts looks for it in the base dictionary, where
    /// FreeType doesn't.
    #[cfg_attr(not(feature = "programs"), allow(dead_code))]
    pub(crate) build_char_len: usize,
}

/// `data` scanned, as [`scan`] says.
pub(crate) fn read(data: &[u8], limits: &Limits) -> Option<Scanned> {
    let fuel = (limits.type1_work).saturating_mul(data.len().max(1 << 14));
    let mut parser = Parser::new(data, fuel);
    let (base, private) = dicts(data, &mut parser)?;
    let mut parser = Parser::new(base, parser.fuel);
    let mut encodings = Vec::new();
    while let Some(token) = parser.next().ok()? {
        if token == Token::Name(b"Encoding") {
            encodings.push(parser.pos);
        }
    }
    let mut parser = Parser::new(&private, parser.fuel);
    let mut len_iv = 4;
    let mut found = None;
    let mut build_char_len = 0;
    loop {
        match parser.next().ok()? {
            None => break,
            Some(Token::Name(b"lenIV")) => match parser.next().ok()? {
                Some(Token::Int(n)) => len_iv = n,
                _ => return None,
            },
            Some(Token::Name(b"CharStrings")) if found.is_none() => {
                found = Some(parser.charstrings(len_iv)?);
            }
            Some(Token::Name(b"BuildCharArray")) => build_char_len = parser.array_len().ok()?,
            Some(_) => {}
        }
    }
    let (found, glyphs) = found?;
    // Which of them read-fonts reads rests on what it takes for a value before it: each
    // is counted, as a program has one or two.
    let mut fuel = parser.fuel;
    for at in encodings {
        let mut parser = Parser::new(base.get(at..)?, fuel);
        let names = parser.encoding_names().ok()?;
        fuel = parser.fuel.checked_sub(names.saturating_mul(glyphs))?;
    }
    Some(Scanned {
        charstrings: found,
        build_char_len,
    })
}

/// The program's base dictionary and its private one decrypted, as
/// read-fonts finds them (`RawDicts::new`): a PFB's segments, or the clear
/// text and what follows `eexec`, hex or binary.
fn dicts<'a>(data: &'a [u8], parser: &mut Parser<'a>) -> Option<(&'a [u8], Vec<u8>)> {
    let header = |d: &[u8]| d.starts_with(b"%!PS-AdobeFont") || d.starts_with(b"%!FontType");
    if let Some((TEXT, size)) = pfb_tag(data, 0) {
        let data = data.get(6..)?;
        header(data).then_some(())?;
        let (base, rest) = data.split_at_checked(size as usize)?;
        let mut binary = Vec::new();
        let mut at = 0;
        // The binary segments that follow, each whole.
        while let Some((BINARY, len)) = pfb_tag(rest, at) {
            let end = (at + 6).checked_add(len as usize);
            let Some(segment) = end.and_then(|end| rest.get(at + 6..end)) else {
                break;
            };
            binary.extend_from_slice(segment);
            at += 6 + segment.len();
        }
        return Some((base, decrypt(&binary, EEXEC_SEED, 4)));
    }
    header(data).then_some(())?;
    let start = parser.eexec()?;
    let (base, rest) = data.split_at_checked(start)?;
    let private = match rest.len() > 3 && rest[..4].iter().all(u8::is_ascii_hexdigit) {
        true => decrypt(&hex(rest), EEXEC_SEED, 4),
        false => decrypt(rest, EEXEC_SEED, 4),
    };
    Some((base, private))
}

const TEXT: u16 = 0x8001;
const BINARY: u16 = 0x8002;

/// A PFB segment's tag and size, at `at`.
fn pfb_tag(data: &[u8], at: usize) -> Option<(u16, u32)> {
    let header = data.get(at..at.checked_add(6)?)?;
    let tag = u16::from_be_bytes([header[0], header[1]]);
    let size = u32::from_le_bytes([header[2], header[3], header[4], header[5]]);
    matches!(tag, TEXT | BINARY).then_some((tag, size))
}

const EEXEC_SEED: u32 = 55665;
const CHARSTRING_SEED: u32 = 4330;

/// `data` decrypted from `seed`, its first `skip` bytes dropped.
fn decrypt(data: &[u8], mut seed: u32, skip: usize) -> Vec<u8> {
    let plain = data.iter().map(|&b| {
        let b = u32::from(b);
        let plain = b ^ (seed >> 8);
        seed = b.wrapping_add(seed).wrapping_mul(52845).wrapping_add(22719) & 0xFFFF;
        plain as u8
    });
    plain.skip(skip).collect()
}

/// Hex digits as bytes, whitespace passed over, ending at anything else;
/// a digit left over is a byte's high half.
fn hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut high = None;
    for &c in data {
        if is_whitespace(c) {
            continue;
        }
        let Some(digit) = (c as char).to_digit(16) else {
            break;
        };
        match high.take() {
            None => high = Some(digit as u8),
            Some(h) => out.push(h << 4 | digit as u8),
        }
    }
    out.extend(high.map(|h| h << 4));
    out
}

/// A token, as read-fonts' Type 1 parser reads one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Token<'a> {
    Int(i64),
    Binary(&'a [u8]),
    Name(&'a [u8]),
    Raw(&'a [u8]),
    /// A procedure, a literal string or a hex string.
    Other,
}

/// Out of fuel.
struct Refused;

#[derive(Clone)]
struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
    /// Bytes it may yet read as tokens, and names an encoding may yet look up.
    fuel: usize,
}

impl<'a> Parser<'a> {
    fn new(data: &'a [u8], fuel: usize) -> Parser<'a> {
        Parser { data, pos: 0, fuel }
    }

    /// The next token, read as read-fonts reads it: None at the end, or where
    /// read-fonts would stop.
    fn next(&mut self) -> Result<Option<Token<'a>>, Refused> {
        loop {
            if !self.skip_whitespace()? {
                return Ok(None);
            }
            let start = self.pos;
            let Some(c) = self.next_byte()? else {
                return Ok(None);
            };
            match c {
                b'%' => self.skip_line()?,
                b'{' => return self.proc(),
                b'(' => return self.string(),
                b'<' if self.peek_byte() == Some(b'<') => self.pos += 1,
                b'<' => return self.hex_string(),
                b'>' => match self.next_byte()? {
                    Some(b'>') => {}
                    _ => return Ok(None),
                },
                b'/' => match self.peek_byte() {
                    Some(c) if is_whitespace(c) || is_special(c) => {
                        self.pos += usize::from(!is_special(c));
                        return Ok(Some(Token::Name(&[])));
                    }
                    Some(_) => {
                        let count = self.skip_until(|c| is_whitespace(c) || is_special(c))?;
                        return Ok(self.data.get(start + 1..start + count).map(Token::Name));
                    }
                    None => {}
                },
                b'[' | b']' => return Ok(Some(Token::Raw(&self.data[start..start + 1]))),
                _ => {
                    let count = self.skip_until(|c| is_whitespace(c) || is_special(c))?;
                    let Some(content) = self.data.get(start..start + count) else {
                        return Ok(None);
                    };
                    let numeric = c.is_ascii_digit() || c == b'-';
                    let int = (numeric && !content.contains(&b'.'))
                        .then(|| int(content))
                        .flatten();
                    let Some(int) = int else {
                        return Ok(Some(Token::Raw(content)));
                    };
                    // An integer followed by `RD` or `-|` is the length of
                    // the binary blob after them and a space.
                    if self.blob_start()? {
                        self.pos += 1;
                        let end = self.pos.checked_add(int as usize);
                        let Some(blob) = end.and_then(|end| self.data.get(self.pos..end)) else {
                            return Ok(None);
                        };
                        self.burn(blob.len())?;
                        self.pos += blob.len();
                        return Ok(Some(Token::Binary(blob)));
                    }
                    return Ok(Some(Token::Int(int)));
                }
            }
        }
    }

    /// Whether what follows is `RD` or `-|`, passed over if so, as read-fonts
    /// looks ahead (`accept_blob_start`): by its bytes, past whitespace alone, so
    /// that a comment there ends the integer.
    fn blob_start(&mut self) -> Result<bool, Refused> {
        let start_at = self.pos;
        self.skip_whitespace()?;
        let start = self.pos;
        self.skip_until(|c| is_whitespace(c) || is_special(c))?;
        if matches!(self.data.get(start..self.pos), Some(b"RD" | b"-|")) {
            return Ok(true);
        }
        self.pos = start_at;
        Ok(false)
    }

    /// A procedure's bytes to its `}`, read as read-fonts reads them (`read_proc`):
    /// its braces counted, past comments and literal strings; none where it's not
    /// ended.
    fn proc(&mut self) -> Result<Option<Token<'a>>, Refused> {
        let mut depth = 1usize;
        while let Some(c) = self.next_byte()? {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Some(Token::Other));
                    }
                }
                b'%' => self.skip_line()?,
                b'(' if self.string()?.is_none() => return Ok(None),
                _ => {}
            }
        }
        Ok(None)
    }

    /// A literal string's bytes, its parentheses balanced, a backslash
    /// taking the byte after it.
    fn string(&mut self) -> Result<Option<Token<'a>>, Refused> {
        let mut depth = 1;
        while let Some(c) = self.next_byte()? {
            match c {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Some(Token::Other));
                    }
                }
                b'\\' if self.next_byte()?.is_none() => return Ok(None),
                _ => {}
            }
        }
        Ok(None)
    }

    /// A hex string's digits and whitespace, to its `>`.
    fn hex_string(&mut self) -> Result<Option<Token<'a>>, Refused> {
        while let Some(c) = self.next_byte()? {
            if !is_whitespace(c) && !c.is_ascii_hexdigit() {
                return Ok((c == b'>').then_some(Token::Other));
            }
        }
        Ok(None)
    }

    /// Where the data after `eexec` starts, as read-fonts finds it
    /// (`find_eexec_data`): past the whitespace after the token, to a
    /// carriage return that no line feed follows before it.
    fn eexec(&mut self) -> Option<usize> {
        while let Some(token) = self.next().ok()? {
            if token != Token::Raw(b"eexec") {
                continue;
            }
            let data = self.data;
            let mut start = self.pos;
            let mut linefeed = None;
            while start < data.len() {
                match data[start] {
                    b' ' | b'\t' => {}
                    b'\n' => linefeed = Some(start),
                    b'\r' => {
                        let next = || {
                            data[start..]
                                .iter()
                                .position(|&b| b == b'\n')
                                .map_or(0, |p| p + start)
                        };
                        if *linefeed.get_or_insert_with(next) < start {
                            break;
                        }
                    }
                    _ => break,
                }
                start += 1;
            }
            return (start < data.len()).then_some(start);
        }
        None
    }

    /// `/CharStrings`' entries, as read-fonts reads them
    /// (`read_charstrings`), decrypted with `len_iv`; and how many it gave, a
    /// name given twice twice.
    fn charstrings(&mut self, len_iv: i64) -> Option<(Charstrings, usize)> {
        let Some(Token::Int(count)) = self.next().ok()? else {
            return None;
        };
        usize::try_from(count).ok()?;
        let mut found = Charstrings::new();
        let mut glyphs = 0usize;
        let ends = |p: &Parser<'_>| {
            p.peek_byte()
                .is_some_and(|c| is_whitespace(c) || is_special(c))
        };
        while let Some(token) = self.next().ok()? {
            match token {
                Token::Raw(b"end") if ends(self) => break,
                Token::Raw(b"def") if ends(self) && !found.is_empty() => break,
                Token::Name(name) => {
                    let Some(Token::Binary(data)) = self.next().ok()? else {
                        return None;
                    };
                    glyphs += 1;
                    let plain = match usize::try_from(len_iv) {
                        Ok(skip) => decrypt(data, CHARSTRING_SEED, skip),
                        Err(_) => data.to_vec(),
                    };
                    found
                        .entry(decoded_name(name))
                        .and_modify(|twice| *twice = None)
                        .or_insert(Some(plain));
                }
                _ => {}
            }
        }
        // read-fonts makes up a `.notdef` where there's none: `0 333 hsbw
        // endchar`.
        found
            .entry(b".notdef".to_vec())
            .or_insert(Some(vec![0x8B, 0xF7, 0xE1, 0x0D, 0x0E]));
        Some((found, glyphs + 1))
    }

    /// How many tokens an array holds, it being next, to its `]`: 0 for what's no array
    /// or isn't ended, which is left where it stops.
    fn array_len(&mut self) -> Result<usize, Refused> {
        let mut ahead = self.clone();
        if ahead.next()? != Some(Token::Raw(b"[")) {
            return Ok(0);
        }
        let mut len = 0;
        loop {
            match ahead.next()? {
                Some(Token::Raw(b"]")) => {
                    *self = ahead;
                    return Ok(len);
                }
                Some(_) => len += 1,
                None => return Ok(0),
            }
        }
    }

    /// How many names an `/Encoding`'s value has read-fonts look for, it being next
    /// (`read_encoding`): an array's, to its end or what's no name, or a map's, code and
    /// name, to its `def`, whether or not they're codes it has.
    fn encoding_names(&mut self) -> Result<usize, Refused> {
        let mut names = 0usize;
        match self.next()? {
            Some(Token::Raw(b"[")) => {
                let mut ahead = self.clone();
                if ahead.next()? == Some(Token::Raw(b"[")) {
                    *self = ahead;
                }
                while let Some(Token::Name(_)) = self.next()? {
                    names += 1;
                }
                Ok(names)
            }
            Some(Token::Int(_)) => loop {
                match self.next()? {
                    None | Some(Token::Raw(b"def")) => return Ok(names),
                    Some(Token::Int(_)) => {
                        if let Some(Token::Name(_)) = self.next()? {
                            names += 1;
                        }
                    }
                    Some(_) => {}
                }
            },
            _ => Ok(0),
        }
    }

    fn burn(&mut self, n: usize) -> Result<(), Refused> {
        self.fuel = self.fuel.checked_sub(n).ok_or(Refused)?;
        Ok(())
    }

    fn next_byte(&mut self) -> Result<Option<u8>, Refused> {
        let Some(&byte) = self.data.get(self.pos) else {
            return Ok(None);
        };
        self.burn(1)?;
        self.pos += 1;
        Ok(Some(byte))
    }

    fn peek_byte(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Whitespace passed over: false at the end.
    fn skip_whitespace(&mut self) -> Result<bool, Refused> {
        while let Some(&c) = self.data.get(self.pos) {
            if !is_whitespace(c) {
                return Ok(true);
            }
            self.burn(1)?;
            self.pos += 1;
        }
        Ok(false)
    }

    fn skip_line(&mut self) -> Result<(), Refused> {
        while let Some(c) = self.next_byte()? {
            if c == b'\n' || c == b'\r' {
                break;
            }
        }
        Ok(())
    }

    /// Bytes passed over until `stop` holds, one more than there are.
    fn skip_until(&mut self, stop: impl Fn(u8) -> bool) -> Result<usize, Refused> {
        let mut count = 0;
        while let Some(byte) = self.peek_byte() {
            if stop(byte) {
                break;
            }
            self.burn(1)?;
            self.pos += 1;
            count += 1;
        }
        Ok(count + 1)
    }
}

/// A glyph's name as read-fonts keeps it (`decode_name`): a name that isn't UTF-8 by
/// its bytes as Latin-1.
pub(crate) fn decoded_name(name: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(name) {
        Ok(_) => name.to_vec(),
        Err(_) => name
            .iter()
            .copied()
            .map(char::from)
            .collect::<String>()
            .into_bytes(),
    }
}

/// An integer as read-fonts reads one (`decode_int`): decimal, or a radix
/// of 2 to 36 before `#`.
fn int(bytes: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(bytes).ok()?;
    match s.find('#') {
        Some(at @ (1 | 2)) => {
            let radix = s[..at]
                .parse::<u32>()
                .ok()
                .filter(|r| (2..=36).contains(r))?;
            i64::from_str_radix(&s[at + 1..], radix).ok()
        }
        _ => s.parse().ok(),
    }
}

fn is_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\0' | 0x0C)
}

fn is_special(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}
