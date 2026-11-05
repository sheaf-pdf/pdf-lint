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
# Text strings, in /Title as hex strings so that every byte is as written:
# each encoding by its mark, what isn't one, a surrogate alone, language
# escapes, bad UTF-8. A surrogate alone can't be held in a Rust string: it's
# U+FFFD here, and the oracle decodes its answer with `replace`, so the two
# agree and no line of differ.tsv names one.
TEXT = [
    ("be", b"\xfe\xff\x00A\x00\xe9"),
    ("be-empty", b"\xfe\xff"),
    ("be-odd", b"\xfe\xff\x00A\x00"),
    ("be-nul", b"\xfe\xff\x00A\x00\x00\x00B"),
    ("be-mark-inside", b"\xfe\xff\xfe\xff\x00A\xff\xfe\x00B"),
    ("be-pair", b"\xfe\xff\xd8\x3d\xde\x00"),
    ("be-lone-high", b"\xfe\xff\xd8\x00\x00A"),
    ("be-lone-high-last", b"\xfe\xff\x00A\xd8\x00"),
    ("be-lone-low", b"\xfe\xff\xdc\x00\x00A"),
    ("be-high-high-low", b"\xfe\xff\xd8\x00\xd8\x00\xdc\x00"),
    ("le", b"\xff\xfeA\x00\xe9\x00"),
    ("le-empty", b"\xff\xfe"),
    ("le-odd", b"\xff\xfeA\x00\x00"),
    ("le-pair", b"\xff\xfe\x3d\xd8\x00\xde"),
    ("le-lone-high", b"\xff\xfe\x00\xd8A\x00"),
    ("utf8", b"\xef\xbb\xbfcaf\xc3\xa9"),
    ("utf8-empty", b"\xef\xbb\xbf"),
    ("utf8-four-bytes", b"\xef\xbb\xbf\xf0\x9f\x98\x80"),
    ("no-mark-fe", b"\xfe"),
    ("no-mark-ff", b"\xff"),
    ("no-mark-ef-bb", b"\xef\xbb"),
    ("no-mark-fe-fe", b"\xfe\xfe\x41"),
    ("no-mark-ff-ff", b"\xff\xff\x41"),
    ("lang-be", b"\xfe\xff\x00\x1b\x00e\x00n\x00\x1b\x00A"),
    ("lang-be-between", b"\xfe\xff\x00A\x00\x1b\x00e\x00n\x00\x1b\x00B"),
    ("lang-be-two", b"\xfe\xff\x00\x1b\x00e\x00\x1b\x00A\x00\x1b\x00f\x00\x1b\x00B"),
    ("lang-be-empty", b"\xfe\xff\x00A\x00\x1b\x00\x1b\x00B"),
    ("lang-be-unclosed", b"\xfe\xff\x00A\x00\x1b\x00e\x00n\x00B"),
    ("lang-be-last", b"\xfe\xff\x00A\x00\x1b"),
    ("lang-be-surrogates-parted", b"\xfe\xff\xd8\x3d\x00\x1b\x00\x1b\xde\x00"),
    ("lang-le", b"\xff\xfe\x1b\x00e\x00n\x00\x1b\x00A\x00"),
    ("lang-le-unclosed", b"\xff\xfeA\x00\x1b\x00e\x00"),
    ("lang-utf8", b"\xef\xbb\xbf\x1benUS\x1bA"),
    ("lang-utf8-unclosed", b"\xef\xbb\xbfA\x1benUS"),
    ("lang-doc", b"a\x1benUS\x1bb"),
    ("bad-utf8-ff", b"\xef\xbb\xbfa\xffb"),
    ("bad-utf8-fe", b"\xef\xbb\xbfa\xfeb"),
    ("bad-utf8-continuation", b"\xef\xbb\xbfa\x80b"),
    ("bad-utf8-two-short", b"\xef\xbb\xbf\xc3A"),
    ("bad-utf8-two-short-last", b"\xef\xbb\xbfA\xc3"),
    ("bad-utf8-three-short", b"\xef\xbb\xbf\xe2\x82A"),
    ("bad-utf8-new-start", b"\xef\xbb\xbf\xc3\xc3\xa9"),
    ("bad-utf8-continuation-after-char", b"\xef\xbb\xbf\xc3\xa9\xa9"),
    ("bad-utf8-short-then-ff", b"\xef\xbb\xbf\xe2\x82\xff\x82A"),
    ("bad-utf8-overlong-nul", b"\xef\xbb\xbfa\xc0\x80b"),
    ("bad-utf8-overlong-a", b"\xef\xbb\xbf\xc1\x81"),
    ("bad-utf8-overlong-three", b"\xef\xbb\xbf\xe0\x81\x81"),
    ("bad-utf8-overlong-escape", b"\xef\xbb\xbfa\xc0\x9benUS\xc0\x9bb"),
    ("bad-utf8-surrogate", b"\xef\xbb\xbf\xed\xa0\x80A"),
    ("bad-utf8-surrogates-paired", b"\xef\xbb\xbf\xed\xa0\xbd\xed\xb8\x80"),
    ("bad-utf8-past-unicode", b"\xef\xbb\xbf\xf4\x90\x80\x81"),
    ("bad-utf8-past-unicode-escape", b"\xef\xbb\xbf\xf4\x90\x80\x9bA"),
    ("bad-utf8-four-top", b"\xef\xbb\xbf\xf7\xbf\xbf\xbf"),
    ("bad-utf8-five", b"\xef\xbb\xbf\xf8\x88\x80\x80\x81"),
    ("bad-utf8-six", b"\xef\xbb\xbf\xfc\x84\x80\x80\x80\x81"),
    ("bad-utf8-six-top", b"\xef\xbb\xbf\xfd\xbf\xbf\xbf\xbf\xbf"),
    ("bad-utf8-four-overlong", b"\xef\xbb\xbf\xf0\x80\x81\x81"),
    ("bad-utf8-short-then-f8", b"\xef\xbb\xbf\xe2\x82\xf8\x82A"),
    ("bad-utf8-five-overlong", b"\xef\xbb\xbf\xf8\x80\x80\x81\x81"),
    ("bad-utf8-six-overlong", b"\xef\xbb\xbf\xfc\x80\x80\x80\x81\x81"),
    ("bad-utf8-short-then-a-then-continuation", b"\xef\xbb\xbf\xe2\x82A\x82"),
    ("bad-utf8-three-new-start", b"\xef\xbb\xbf\xe2\xc3\xa9"),
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
    out += [info("text-" + n, b"<< /Title <" + v.hex().encode() + b"> >>", b"Title") for n, v in TEXT]
    # Every byte of PDFDocEncoding, sixteen to a case, the marks' first
    # bytes after another.
    for high in range(16):
        row = bytes(high * 16 + low for low in range(16))
        out.append(info("text-doc-%xx" % high, b"<< /Title <" + (b"a" + row).hex().encode() + b"> >>", b"Title"))
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
