"""pdf-tokenizer's PDFium cases: what PDFium reads a word as, by the kinds
oracle/README.md describes. Every case's bytes are written here, from
literals; oracle/pdfium.py's `cases` writes them into cases/ and
cases.tsv."""


def word(kind, name, w):
    return ("%s-%s" % (kind, name), kind, {"word": w})


# Number words: digits, signs and points in any order, a word PDFium reads
# as a number; beside them words it doesn't, and the edges of its 32-bit
# integers and its f32 reals.
NUMBERS = [
    ("two-points", b"1.2.3"),
    ("two-minus-real", b"--5.0"),
    ("plus-minus-point", b"+-.5"),
    ("trailing-point", b"5."),
    ("point", b"."),
    ("minus", b"-"),
    ("plus", b"+"),
    ("leading-zeros", b"007"),
    ("u32-max", b"4294967295"),
    ("u32-max-plus-1", b"4294967296"),
    ("plus-i32-max-plus-1", b"+2147483648"),
    ("i32-min", b"-2147483648"),
    ("i32-max-plus-1", b"2147483648"),
    ("i32-min-minus-1", b"-2147483649"),
    ("minus-inside", b"5-3"),
    ("two-minus", b"--5"),
    ("exponent", b"1e5"),
    ("minus-point-5", b"-.5"),
    ("plus-point", b"+."),
    ("minus-point", b"-."),
    ("point-5-point", b".5."),
    ("two-points-together", b"1..2"),
    ("plus-5", b"+5"),
    ("minus-0", b"-0"),
    ("41-nines-point-0", b"9" * 41 + b".0"),
    ("50-nines", b"9" * 50),
    ("1e-45", b"0." + b"0" * 44 + b"1"),
    ("f32-max-exponent", b"3.4028236e38"),
    ("f32-max", b"340282346638528859811704183484516925440.0"),
    ("f32-max-rounding-down", b"340282356779733661637539395458142568447.9"),
    ("f32-max-rounding-up", b"340282356779733661637539395458142568448.0"),
    ("minus-f32-max-rounding-up", b"-340282356779733661637539395458142568448.0"),
    ("point-1", b"0.1"),
    ("2^24-plus-1-real", b"16777217.0"),
    ("2^24-plus-1", b"16777217"),
    ("plus-2^24-plus-1", b"+16777217"),
    ("halfway-to-even", b"2.00000011920928955078125"),
    ("past-halfway", b"2.000000119209289550781251"),
]

# Hex strings: PDFium's runs to the next `>` over any byte, pairing the
# hex digits in it.
HEX = [
    ("space", b"<41 42>"),
    ("letter-between", b"<41G42>"),
    ("letter-inside", b"<4G1>"),
    ("letter-last", b"<4G>"),
    ("open-paren", b"<41(42>"),
    ("slash", b"<41/42>"),
    ("percent", b"<41%42>\n"),
    ("bracket", b"<41]42>"),
    ("byte-80", b"<41\x8042>"),
    ("close-brace", b"<41}42>"),
    ("open-brace", b"<41{42>"),
    ("space-inside", b"<4 1>"),
    ("odd-after-space", b"<41 4>"),
    ("odd", b"<414>"),
    ("empty", b"<>"),
]

# Names in `Tf`: `/F0` is the font; a `#` and the two bytes after it make a
# byte, one that's no hex digit read as 0.
NAMES = [
    ("plain", b"/F0"),
    ("hash-30", b"/F#30"),
    ("hash-3-upper-G", b"/F#3G"),
    ("hash-3-lower-g", b"/F#3g"),
    ("hash-G0", b"/F#G0"),
    ("hash-46-first", b"/#460"),
    ("hash-one-digit", b"/F#3"),
    ("hash-last", b"/F0#"),
    ("hash-2-last", b"/F0#2"),
    ("hash-300", b"/F#300"),
    ("hash-twice", b"/#F#30"),
    ("hash-then-word", b"/F#3 0"),
]

# Literal strings: a CR or CR LF written in one, and escaped.
LITERALS = [
    ("cr-and-crlf", b"(a\rb\r\nc)"),
    ("lf", b"(a\nb)"),
    ("escaped-cr", b"(a\\\rb)"),
    ("escaped-crlf", b"(a\\\r\nb)"),
    ("escaped-lf", b"(a\\\nb)"),
    ("octal-cr", b"(a\\015b)"),
    ("two-cr", b"(a\r\rb)"),
    ("lf-cr", b"(a\n\rb)"),
]


def cases():
    return ([word("number", n, w) for n, w in NUMBERS]
            + [word("hex", n, w) for n, w in HEX]
            + [word("name", n, w) for n, w in NAMES]
            + [word("literal", n, w) for n, w in LITERALS])
