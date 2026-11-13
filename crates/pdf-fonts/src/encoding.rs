//! Simple fonts' encodings, code to glyph name, and glyph names to text
//! through the Adobe Glyph List and the ITC Zapf Dingbats Glyph List
//! (`assets/glyphlist.txt` and `assets/zapfdingbats.txt`, each with its licence
//! in its header).

use std::collections::HashMap;
use std::sync::OnceLock;

/// A simple font's encoding: which glyph each one-byte code draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Encoding {
    /// StandardEncoding, the Latin fonts' own (ISO 32000-2, D.2).
    Standard,
    /// Windows-1252's characters, by the names PDF gives them, and the bullet at
    /// the codes above the space it leaves empty (ISO 32000-2, D.2).
    WinAnsi,
    /// MacRomanEncoding (ISO 32000-2, D.2).
    MacRoman,
    /// Symbol's own (ISO 32000-2, D.5).
    Symbol,
    /// ZapfDingbats' own (ISO 32000-2, D.6).
    ZapfDingbats,
}

impl Encoding {
    /// The base encoding a font's `/Encoding` or `/BaseEncoding` names.
    /// `MacExpertEncoding`'s small capitals and old-style figures aren't
    /// read: it's None, as an unknown name is.
    #[must_use]
    pub const fn named(name: &[u8]) -> Option<Encoding> {
        match name {
            b"StandardEncoding" => Some(Encoding::Standard),
            b"WinAnsiEncoding" => Some(Encoding::WinAnsi),
            b"MacRomanEncoding" => Some(Encoding::MacRoman),
            _ => None,
        }
    }

    /// The glyph `code` draws.
    #[must_use]
    pub fn glyph(self, code: u8) -> Option<&'static str> {
        match self {
            Encoding::Standard => tables().standard_names[code as usize],
            Encoding::WinAnsi => tables().win_ansi_names[code as usize],
            Encoding::MacRoman => tables().mac_roman_names[code as usize],
            Encoding::Symbol => tables().symbol_names[code as usize],
            Encoding::ZapfDingbats => tables().zapf_dingbats_names[code as usize],
        }
    }

    /// The text `code` draws. WinAnsi's is Windows-1252's where that has one,
    /// so its second space and hyphen, 0xA0 and 0xAD, are U+00A0 and U+00AD,
    /// and else its glyph's: the bullet of a code it has no other use for.
    #[must_use]
    pub fn text(self, code: u8) -> Option<String> {
        match (self, cp1252(code)) {
            (Encoding::WinAnsi, Some(c)) => Some(String::from(c)),
            _ => glyph_text(self.glyph(code)?, self == Encoding::ZapfDingbats),
        }
    }
}

/// Windows-1252's character for `code`, where WinAnsiEncoding draws one:
/// none for the control codes or the five codes it leaves empty.
#[must_use]
pub const fn cp1252(code: u8) -> Option<char> {
    match code {
        0x20..=0x7e | 0xa0..=0xff => Some(code as char),
        0x80..=0x9f => match CP1252_HIGH[code as usize - 0x80] {
            '\0' => None,
            c => Some(c),
        },
        _ => None,
    }
}

/// The text glyph `name` stands for, by the AGL specification's rules:
/// what follows a period is passed over, the parts an underscore joins
/// are each read, and a part is read from the ITC Zapf Dingbats list (in
/// a ZapfDingbats font), the Adobe Glyph List, `uniXXXX…` or `uXXXX` to
/// `uXXXXXX`. None when no part reads as anything.
#[must_use]
pub fn glyph_text(name: &str, dingbats: bool) -> Option<String> {
    let name = name.split('.').next().unwrap_or_default();
    let mut text = String::new();
    for part in name.split('_') {
        let listed = dingbats
            .then(|| tables().dingbats.get(part))
            .flatten()
            .or_else(|| tables().agl.get(part));
        match listed {
            Some(t) => text.push_str(t),
            None => text.extend(uni(part).or_else(|| u(part)).into_iter().flatten()),
        }
    }
    (!text.is_empty()).then_some(text)
}

/// The code point glyph `name` stands for as FreeType's glyph name list
/// reads it, which PDFium copies (`FXFT_unicode_from_adobe_name`): `uni`
/// and exactly four upper-case hex digits, or `u` and four to six, each
/// at the end or before a period; else the Adobe Glyph List's code point
/// for what's before a period that isn't the first character, where it
/// gives exactly one. With whether a period followed, as a variant's name
/// has one. None where it's none of those.
#[must_use]
pub fn name_code_point(name: &[u8]) -> Option<(u32, bool)> {
    // Up to `most` upper-case hex digits after `prefix`, at least `least`
    // of them, and what follows them.
    let hex = |prefix: &[u8], least: usize, most: usize| {
        let rest = name.strip_prefix(prefix)?;
        let digits = rest
            .iter()
            .take(most)
            .take_while(|c| matches!(c, b'0'..=b'9' | b'A'..=b'F'));
        let n = digits.count();
        let value =
            (rest[..n].iter()).fold(0, |v, &c| v << 4 | (c as char).to_digit(16).unwrap_or(0));
        match rest.get(n) {
            _ if n < least => None,
            None => Some((value, false)),
            Some(b'.') => Some((value, true)),
            Some(_) => None,
        }
    };
    if let Some(read) = hex(b"uni", 4, 4).or_else(|| hex(b"u", 4, 6)) {
        return Some(read);
    }
    let dot = name.iter().skip(1).position(|&c| c == b'.').map(|i| i + 1);
    let base = std::str::from_utf8(&name[..dot.unwrap_or(name.len())]).ok()?;
    // The list FreeType keeps has only the names of one code point.
    let mut text = tables().agl.get(base)?.chars();
    let one = text.next().filter(|_| text.next().is_none())?;
    Some((u32::from(one), dot.is_some()))
}

/// `uniXXXX…`: groups of four upper-case hex digits, none a surrogate. Anything
/// but ASCII is none, and four bytes of it might not be whole characters.
fn uni(part: &str) -> Option<Vec<char>> {
    let hex = part.strip_prefix("uni")?;
    if hex.is_empty() || hex.len() % 4 != 0 || !hex.is_ascii() {
        return None;
    }
    (0..hex.len())
        .step_by(4)
        .map(|i| scalar(&hex[i..i + 4]))
        .collect()
}

/// `uXXXX` to `uXXXXXX`: one scalar value, in upper-case hex digits.
fn u(part: &str) -> Option<Vec<char>> {
    let hex = part.strip_prefix('u')?;
    (4..=6).contains(&hex.len()).then_some(())?;
    Some(vec![scalar(hex)?])
}

/// Upper-case hex digits as a Unicode scalar value.
fn scalar(hex: &str) -> Option<char> {
    if !hex.bytes().all(|c| matches!(c, b'0'..=b'9' | b'A'..=b'F')) {
        return None;
    }
    char::from_u32(u32::from_str_radix(hex, 16).ok()?)
}

/// The glyph lists and the encodings' tables, read once.
struct Tables {
    agl: HashMap<&'static str, String>,
    dingbats: HashMap<&'static str, String>,
    standard_names: [Option<&'static str>; 256],
    win_ansi_names: [Option<&'static str>; 256],
    mac_roman_names: [Option<&'static str>; 256],
    symbol_names: [Option<&'static str>; 256],
    zapf_dingbats_names: [Option<&'static str>; 256],
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let table = |from, words| {
            let mut table = [None; 256];
            names(&mut table, from, words);
            table
        };
        let win_ansi_names = table(0x20, WIN_ANSI);
        let mut mac_roman_names = win_ansi_names;
        // WinAnsi's bullet, which draws where it has no glyph, isn't MacRoman's.
        mac_roman_names[0x7f] = None;
        names(&mut mac_roman_names, 0x80, MAC_ROMAN_HIGH);
        Tables {
            agl: list(include_str!("../assets/glyphlist.txt")),
            dingbats: list(include_str!("../assets/zapfdingbats.txt")),
            standard_names: table(0x20, STANDARD),
            win_ansi_names,
            mac_roman_names,
            symbol_names: table(0x20, SYMBOL),
            zapf_dingbats_names: table(0x20, ZAPF_DINGBATS),
        }
    })
}

/// A glyph list's `name;XXXX[ XXXX…]` lines.
fn list(text: &'static str) -> HashMap<&'static str, String> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let (name, values) = l.split_once(';')?;
            let text = values.split(' ').map(scalar).collect::<Option<String>>()?;
            Some((name, text))
        })
        .collect()
}

/// `table`'s codes from `from` on, named by `names`' words in turn; `-`
/// is no glyph.
fn names(table: &mut [Option<&'static str>; 256], from: u8, names: &'static str) {
    let slots = table.iter_mut().skip(from as usize);
    for (slot, name) in slots.zip(names.split_whitespace()) {
        *slot = Some(name).filter(|&n| n != "-");
    }
}

/// StandardEncoding's glyphs (ISO 32000-2, D.2) from 0x20, sixteen a line.
const STANDARD: &str = "
    space exclam quotedbl numbersign dollar percent ampersand quoteright
        parenleft parenright asterisk plus comma hyphen period slash
    zero one two three four five six seven
        eight nine colon semicolon less equal greater question
    at A B C D E F G
        H I J K L M N O
    P Q R S T U V W
        X Y Z bracketleft backslash bracketright asciicircum underscore
    quoteleft a b c d e f g
        h i j k l m n o
    p q r s t u v w
        x y z braceleft bar braceright asciitilde -
    - - - - - - - -
        - - - - - - - -
    - - - - - - - -
        - - - - - - - -
    - exclamdown cent sterling fraction yen florin section
        currency quotesingle quotedblleft guillemotleft guilsinglleft guilsinglright fi fl
    - endash dagger daggerdbl periodcentered - paragraph bullet
        quotesinglbase quotedblbase quotedblright guillemotright ellipsis perthousand - questiondown
    - grave acute circumflex tilde macron breve dotaccent
        dieresis - ring cedilla - hungarumlaut ogonek caron
    emdash - - - - - - -
        - - - - - - - -
    - AE - ordfeminine - - - -
        Lslash Oslash OE ordmasculine - - - -
    - ae - - - dotlessi - -
        lslash oslash oe germandbls - - - -
";

/// WinAnsiEncoding's glyphs (ISO 32000-2, D.2) from 0x20, sixteen a
/// line.
const WIN_ANSI: &str = "
    space exclam quotedbl numbersign dollar percent ampersand quotesingle
        parenleft parenright asterisk plus comma hyphen period slash
    zero one two three four five six seven eight nine colon semicolon less equal greater question
    at A B C D E F G H I J K L M N O
    P Q R S T U V W X Y Z bracketleft backslash bracketright asciicircum underscore
    grave a b c d e f g h i j k l m n o
    p q r s t u v w x y z braceleft bar braceright asciitilde bullet
    Euro bullet quotesinglbase florin quotedblbase ellipsis dagger daggerdbl
        circumflex perthousand Scaron guilsinglleft OE bullet Zcaron bullet
    bullet quoteleft quoteright quotedblleft quotedblright bullet endash emdash
        tilde trademark scaron guilsinglright oe bullet zcaron Ydieresis
    space exclamdown cent sterling currency yen brokenbar section
        dieresis copyright ordfeminine guillemotleft logicalnot hyphen registered macron
    degree plusminus twosuperior threesuperior acute mu paragraph periodcentered
        cedilla onesuperior ordmasculine guillemotright onequarter onehalf threequarters questiondown
    Agrave Aacute Acircumflex Atilde Adieresis Aring AE Ccedilla
        Egrave Eacute Ecircumflex Edieresis Igrave Iacute Icircumflex Idieresis
    Eth Ntilde Ograve Oacute Ocircumflex Otilde Odieresis multiply
        Oslash Ugrave Uacute Ucircumflex Udieresis Yacute Thorn germandbls
    agrave aacute acircumflex atilde adieresis aring ae ccedilla
        egrave eacute ecircumflex edieresis igrave iacute icircumflex idieresis
    eth ntilde ograve oacute ocircumflex otilde odieresis divide
        oslash ugrave uacute ucircumflex udieresis yacute thorn ydieresis
";

/// MacRomanEncoding's glyphs from 0x80, sixteen a line, with the Mac OS
/// symbols Annex D leaves out, the Apple logo among them, as PDFium's table
/// has it; below, it's WinAnsi's.
const MAC_ROMAN_HIGH: &str = "
    Adieresis Aring Ccedilla Eacute Ntilde Odieresis Udieresis aacute
        agrave acircumflex adieresis atilde aring ccedilla eacute egrave
    ecircumflex edieresis iacute igrave icircumflex idieresis ntilde oacute
        ograve ocircumflex odieresis otilde uacute ugrave ucircumflex udieresis
    dagger degree cent sterling section bullet paragraph germandbls
        registered copyright trademark acute dieresis notequal AE Oslash
    infinity plusminus lessequal greaterequal yen mu partialdiff summation
        product pi integral ordfeminine ordmasculine Omega ae oslash
    questiondown exclamdown logicalnot radical florin approxequal Delta guillemotleft
        guillemotright ellipsis space Agrave Atilde Otilde OE oe
    endash emdash quotedblleft quotedblright quoteleft quoteright divide lozenge
        ydieresis Ydieresis fraction currency guilsinglleft guilsinglright fi fl
    daggerdbl periodcentered quotesinglbase quotedblbase perthousand Acircumflex Ecircumflex Aacute
        Edieresis Egrave Iacute Icircumflex Idieresis Igrave Oacute Ocircumflex
    apple Ograve Uacute Ucircumflex Ugrave dotlessi circumflex tilde
        macron breve dotaccent ring cedilla hungarumlaut ogonek caron
";

/// Symbol's own glyphs (ISO 32000-2, D.5) from 0x20, sixteen a line.
const SYMBOL: &str = "
    space exclam universal numbersign existential percent ampersand suchthat
        parenleft parenright asteriskmath plus comma minus period slash
    zero one two three four five six seven
        eight nine colon semicolon less equal greater question
    congruent Alpha Beta Chi Delta Epsilon Phi Gamma
        Eta Iota theta1 Kappa Lambda Mu Nu Omicron
    Pi Theta Rho Sigma Tau Upsilon sigma1 Omega
        Xi Psi Zeta bracketleft therefore bracketright perpendicular underscore
    radicalex alpha beta chi delta epsilon phi gamma
        eta iota phi1 kappa lambda mu nu omicron
    pi theta rho sigma tau upsilon omega1 omega
        xi psi zeta braceleft bar braceright similar -
    - - - - - - - -
        - - - - - - - -
    - - - - - - - -
        - - - - - - - -
    Euro Upsilon1 minute lessequal fraction infinity florin club
        diamond heart spade arrowboth arrowleft arrowup arrowright arrowdown
    degree plusminus second greaterequal multiply proportional partialdiff bullet
        divide notequal equivalence approxequal ellipsis arrowvertex arrowhorizex carriagereturn
    aleph Ifraktur Rfraktur weierstrass circlemultiply circleplus emptyset intersection
        union propersuperset reflexsuperset notsubset propersubset reflexsubset element notelement
    angle gradient registerserif copyrightserif trademarkserif product radical dotmath
        logicalnot logicaland logicalor arrowdblboth arrowdblleft arrowdblup arrowdblright arrowdbldown
    lozenge angleleft registersans copyrightsans trademarksans summation parenlefttp parenleftex
        parenleftbt bracketlefttp bracketleftex bracketleftbt bracelefttp braceleftmid braceleftbt braceex
    - angleright integral integraltp integralex integralbt parenrighttp parenrightex
        parenrightbt bracketrighttp bracketrightex bracketrightbt bracerighttp bracerightmid bracerightbt -
";

/// ZapfDingbats' own glyphs (ISO 32000-2, D.6) from 0x20, sixteen a line.
const ZAPF_DINGBATS: &str = "
    space a1 a2 a202 a3 a4 a5 a119
        a118 a117 a11 a12 a13 a14 a15 a16
    a105 a17 a18 a19 a20 a21 a22 a23
        a24 a25 a26 a27 a28 a6 a7 a8
    a9 a10 a29 a30 a31 a32 a33 a34
        a35 a36 a37 a38 a39 a40 a41 a42
    a43 a44 a45 a46 a47 a48 a49 a50
        a51 a52 a53 a54 a55 a56 a57 a58
    a59 a60 a61 a62 a63 a64 a65 a66
        a67 a68 a69 a70 a71 a72 a73 a74
    a203 a75 a204 a76 a77 a78 a79 a81
        a82 a83 a84 a97 a98 a99 a100 -
    a89 a90 a93 a94 a91 a92 a205 a85
        a206 a86 a87 a88 a95 a96 - -
    - - - - - - - -
        - - - - - - - -
    - a101 a102 a103 a104 a106 a107 a108
        a112 a111 a110 a109 a120 a121 a122 a123
    a124 a125 a126 a127 a128 a129 a130 a131
        a132 a133 a134 a135 a136 a137 a138 a139
    a140 a141 a142 a143 a144 a145 a146 a147
        a148 a149 a150 a151 a152 a153 a154 a155
    a156 a157 a158 a159 a160 a161 a163 a164
        a196 a165 a192 a166 a167 a168 a169 a170
    a171 a172 a173 a162 a174 a175 a176 a177
        a178 a179 a193 a180 a199 a181 a200 a182
    - a201 a183 a184 a197 a185 a194 a198
        a186 a195 a187 a188 a189 a190 a191 -
";

/// Windows-1252's characters from 0x80 to 0x9F; `\0` for its five empty
/// codes.
const CP1252_HIGH: [char; 32] = [
    '€', '\0', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\0', 'Ž', '\0', '\0', '‘',
    '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\0', 'ž', 'Ÿ',
];
