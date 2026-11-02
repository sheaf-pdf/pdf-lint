//! Font programs built by hand, for the tests and the fuzz seeds: what a program
//! needs of each format and no more.

/// A Type 1 program (`/FontFile`): clear text, then its private dictionary
/// and charstrings encrypted (`eexec`, `lenIV` 4): `.notdef`, `space`, `A` and
/// `g`, `g`'s side bearing 20, in the standard encoding.
pub fn type1_program() -> Vec<u8> {
    let charstring = |sbw: [i32; 2], ops: Vec<u8>| -> Vec<u8> {
        let plain = [vec![0; 4], cs(&sbw), vec![13], ops, vec![14]].concat();
        encrypt(&plain, 4330)
    };
    let glyphs = [
        (".notdef", charstring([0, 500], Vec::new())),
        ("space", charstring([0, 250], Vec::new())),
        (
            "A",
            charstring(
                [0, 600],
                [
                    cs(&[0, 0]),
                    vec![21],
                    cs(&[300, 750]),
                    vec![5],
                    cs(&[300, -750]),
                    vec![5, 9],
                ]
                .concat(),
            ),
        ),
        (
            "g",
            charstring(
                [20, 500],
                [
                    cs(&[0, -200]),
                    vec![21],
                    cs(&[460, 0]),
                    vec![5],
                    cs(&[0, 700]),
                    vec![5],
                    cs(&[-100, 60, -130, 0, -130, -60]),
                    vec![8, 9],
                ]
                .concat(),
            ),
        ),
    ];
    let mut private = b"dup /Private 8 dict dup begin\n\
        /RD{string currentfile exch readstring pop}executeonly def\n\
        /ND{noaccess def}executeonly def\n\
        /NP{noaccess put}executeonly def\n\
        /BlueValues [] def\n/MinFeature {16 16} def\n/password 5839 def\n/lenIV 4 def\n\
        2 index /CharStrings 4 dict dup begin\n"
        .to_vec();
    for (name, data) in glyphs {
        private.extend(format!("/{name} {} RD ", data.len()).bytes());
        private.extend(data);
        private.extend(b" ND\n");
    }
    private.extend(b"end\nend\nreadonly put\nnoaccess put\ndup/FontName get exch definefont pop\nmark currentfile closefile\n");
    let clear = b"%!PS-AdobeFont-1.0: Fix 001.000\n\
        11 dict begin\n/FontName /Fix def\n/PaintType 0 def\n/FontType 1 def\n\
        /FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/Encoding StandardEncoding def\n\
        /FontBBox {0 -200 600 750} readonly def\ncurrentdict end\ncurrentfile eexec\n";
    [
        &clear[..],
        &encrypt(&[&[0u8; 4][..], &private].concat(), 55665),
    ]
    .concat()
}

/// [`type1_program`] with `n` integers in a row in its clear text, which
/// read-fonts reads ahead of one by one.
pub fn type1_integers_program(n: usize) -> Vec<u8> {
    let program = type1_program();
    let at = program.windows(4).position(|w| w == b"/Pai").unwrap();
    [&program[..at], "1 ".repeat(n).as_bytes(), &program[at..]].concat()
}

/// `plain` encrypted as Type 1 encrypts it, from `key`.
fn encrypt(plain: &[u8], mut key: u16) -> Vec<u8> {
    plain
        .iter()
        .map(|&p| {
            let c = p ^ (key >> 8) as u8;
            key = (u16::from(c).wrapping_add(key))
                .wrapping_mul(52845)
                .wrapping_add(22719);
            c
        })
        .collect()
}

/// A charstring's numbers, encoded as Type 1 and Type 2 charstrings share:
/// a byte, or two, for -1131 to 1131.
fn cs(values: &[i32]) -> Vec<u8> {
    let mut out = Vec::new();
    for &v in values {
        match v {
            -107..=107 => out.push((v + 139) as u8),
            108..=1131 => out.extend([((v - 108) / 256 + 247) as u8, ((v - 108) % 256) as u8]),
            -1131..=-108 => out.extend([((-v - 108) / 256 + 251) as u8, ((-v - 108) % 256) as u8]),
            _ => panic!("a fixture's number fits two bytes"),
        }
    }
    out
}
