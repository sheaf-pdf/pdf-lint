"""pdf-objects' PDFium cases: objects as PDFium's object parser reads
them, by the kinds oracle/README.md describes: a /MediaBox's value, an
/Info dictionary's entry, a page tree's /Kids. Every case's bytes are
written here, from literals; oracle/pdfium.py's `cases` writes them into
cases/ and cases.tsv."""


def mediabox(name, value):
    return ("mediabox-" + name, "mediabox", {"obj": value})


def info(name, dictionary, tag):
    return ("info-" + name, "info", {"obj": dictionary, "tag": tag})


def kids(name, value):
    return ("kids-" + name, "kids", {"obj": value})


# Number words in a /MediaBox's first corner: its width is 100 less the
# word's value, where it's a number; and the box emptied.
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
    ("minus-5", b"-5"),
    ("20-nines", b"9" * 20),
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

# A word PDFium's object parser keeps 256 bytes of: 0s and a 5.
WORDS = [("word-%d-bytes" % n, b"0" * (n - 1) + b"5") for n in (255, 256, 257, 258)] + [
    ("real-257-bytes", b"0." + b"0" * 254 + b"5"),
    ("real-258-bytes-point-at-257", b"0" * 255 + b"7.5"),
    ("real-258-bytes-point-at-256", b"0" * 256 + b".5"),
]

# Hex strings in /Title: PDFium's runs to the next `>` over any byte.
TITLE_HEX = [
    ("space", b"<41 42>"),
    ("letter-between", b"<41G42>"),
    ("letter-inside", b"<4G1>"),
    ("letter-last", b"<4G>"),
    ("open-paren", b"<41(42>"),
    ("slash", b"<41/42>"),
    ("percent", b"<41%42>"),
    ("byte-80", b"<41\x8042>"),
    ("odd-after-space", b"<41 4>"),
    ("odd", b"<414>"),
]

# Literal strings in /Title: a CR or CR LF written in one, and escaped.
TITLE_LITERAL = [
    ("cr-and-crlf", b"(a\rb\r\nc)"),
    ("lf", b"(a\nb)"),
    ("escaped-cr", b"(a\\\rb)"),
    ("escaped-crlf", b"(a\\\r\nb)"),
    ("escaped-lf", b"(a\\\nb)"),
    ("octal-cr", b"(a\\015b)"),
    ("two-cr", b"(a\r\rb)"),
    ("lf-cr", b"(a\n\rb)"),
]

# /Info keys with `#` in them, and the tag asked: a `#` and the two bytes
# after it make a byte, one that's no hex digit read as 0.
KEYS = [
    ("hash-5G", b"/#5Groducer", b"Producer"),
    ("hash-50", b"/#50roducer", b"Producer"),
    ("hash-72-last", b"/Produce#72", b"Producer"),
    ("hash-last", b"/Producer#", b"Producer#"),
    ("hash-one-digit-as-written", b"/Producer#7", b"Producer#7"),
    ("hash-one-digit-as-byte", b"/Producer#7", b"Producerp"),
    ("hash-one-digit-short", b"/Produce#7", b"Produce#7"),
    ("hash-g4", b"/P#g4", b"P\x04"),
    ("hash-hash-41", b"/##41", b"\x041"),
    ("hash-7z", b"/P#7z", b"Pp"),
    ("hash-z7", b"/P#z7", b"P\x07"),
    ("hash-3-short", b"/F#3", b"F#3"),
    ("hash-20", b"/A#20B", b"A B"),
]

# References in a page tree's /Kids: object 3 is its page, 50 by 60.
REFERENCES = [
    ("plain", b"3 0 R"),
    ("plus", b"+3 0 R"),
    ("real-number", b"3.9 0 R"),
    ("two-minus-number", b"--3 0 R"),
    ("negative-generation", b"3 -1 R"),
    ("generation-past-32-bits", b"3 99999999999 R"),
    ("two-minus-generation", b"3 --1 R"),
    ("keyword-generation", b"3 x R"),
    ("number-past-32-bits", b"4294967299 0 R"),
    ("real-generation", b"3 0.5 R"),
    ("generation-past-16-bits", b"3 65536 R"),
    ("negative-number", b"-3 0 R"),
    ("two-minus-and-plus", b"--3 + R"),
    ("plus-generation", b"3 + R"),
]


def cases():
    out = [mediabox(n, b"[" + w + b" 0 100 100]") for n, w in NUMBERS + WORDS]
    out += [
        mediabox("emptied", b"[0 0 1.2.3 --5]"),
        mediabox("byte-80-between", b"[0\x800 100 100]"),
        mediabox("byte-ff-between", b"[0\xff0 100 100]"),
        mediabox("byte-0b-between", b"[0\x0b0 100 100]"),
        mediabox("byte-85-between", b"[0\x850 100 100]"),
        mediabox("byte-7f-between", b"[0\x7f0 100 100]"),
        mediabox("byte-a0-between", b"[0\xa00 100 100]"),
    ]
    out += [info("title-hex-" + n, b"<< /Title " + v + b" >>", b"Title") for n, v in TITLE_HEX]
    out += [info("title-literal-" + n, b"<< /Title " + v + b" >>", b"Title") for n, v in TITLE_LITERAL]
    out += [info("key-" + n, b"<< " + k + b" (x) >>", t) for n, k, t in KEYS]
    for n in (255, 256, 257):
        body = b"P" * (n - 1)
        out.append(info("key-%d-bytes" % n, b"<< /" + body + b" (x) >>", body))
        out.append(info("key-%d-bytes-as-255" % n, b"<< /" + body + b" (x) >>", b"P" * 255))
    for n in (32767, 32768, 40000):
        out.append(info("title-literal-%d-bytes" % n, b"<< /Title (" + b"a" * n + b") >>", b"Title"))
    for name, b in (("80", b"\x80"), ("ff", b"\xff"), ("0b", b"\x0b"), ("85", b"\x85"), ("7f", b"\x7f"),
                    ("a0", b"\xa0")):
        out.append(info("title-byte-%s-before-value" % name, b"<< /Title" + b + b"(x) >>", b"Title"))
        out.append(info("key-byte-%s-inside" % name, b"<< /Ti" + b + b"tle (x) >>", b"Ti"))
    out += [kids(n, r) for n, r in REFERENCES]
    return out
