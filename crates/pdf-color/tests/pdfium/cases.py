"""pdf-color's cases, each a file whose page draws an inline image in a color
space its resources name: what PDFium sized the image by is what it counted
(oracle/README.md, `spaces`). Written from literals; oracle/pdfium.py's `cases`
writes them into cases/ and cases.tsv.

A case's objects are its own: 1 a catalog, 2 the pages, 3 the page, 4 its
content, and any others from 5 on, in the order given."""

# Bytes for an image's data: enough for 20 components of 8 samples.
PAD = b"a" * 160

F2_1 = b"<< /FunctionType 2 /Domain [0 1] /N 1 >>"
F2_3 = b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0] /C1 [1 1 1] >>"
F2_4 = b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0 0] /C1 [1 1 1 1] >>"
F2_2I = b"<< /FunctionType 2 /Domain [0 1 0 1] /N 1 /C0 [0 0] /C1 [1 1] >>"
F2_RANGE = b"<< /FunctionType 2 /Domain [0 1] /N 1 /Range [0 1 0 1 0 1 0 1] >>"
F2_NO_N = b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] >>"
F3 = b"<< /FunctionType 3 /Domain [0 1] /Functions [" + F2_4 + b"] /Bounds [] /Encode [0 1] >>"
F_BAD = b"<< /FunctionType 1 /Domain [0 1] >>"
CAL = b"<< /WhitePoint [0.95 1 1.08] >>"
WP1 = b"<< /WhitePoint [1 1 1] >>"
F0_HEAD = b"/FunctionType 0 /Domain [0 1] /Range [0 1] "
F0_2IN4OUT = b"/FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1] /Size [2 2] /BitsPerSample 8"
F4_2IN4OUT = b"/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1]"
A = b"[/A /B]"


def build(res, content, extra=()):
    page = (b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources "
            + res + b" >>")
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               page, b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream"]
    objects += list(extra)
    out = bytearray(b"%PDF-1.7\n")
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % number + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    for offset in offsets:
        out += b"%010d 00000 n \n" % offset
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    return bytes(out)


def image(name):
    return b"BI /W 8 /H 1 /BPC 8 /CS " + name + b" ID " + PAD + b" EI 5 5 m 7 7 l S"


def stream(dict_, data=b""):
    return b"<< " + dict_ + b" /Length %d >>\nstream\n" % len(data) + data + b"\nendstream"


def chain(n, end):
    """Objects from 5 on: n - 1 references, each to the next, then `end`."""
    return [b"%d 0 R" % (6 + i) for i in range(n - 1)] + [end]


def cases():
    out = []

    def res_case(name, res, extra=(), content=None):
        pdf = build(res, content if content is not None else image(b"/X"), extra)
        out.append((name, "spaces", {"pdf": pdf}))

    def named(name, value, extra=()):
        res_case(name, b"<< /ColorSpace << /X " + value + b" /Y /DeviceCMYK >> >>", extra)

    # A name alone: a stock space, or no space.
    for name in [b"DeviceGray", b"DeviceRGB", b"DeviceCMYK", b"G", b"RGB", b"CMYK", b"Pattern",
                 b"CalGray", b"CalRGB", b"Lab", b"Indexed", b"I", b"Separation", b"DeviceN",
                 b"ICCBased", b"Foo", b"X", b"Y", b"DeviceRGBX", b"PatternX"]:
        named("name-" + name.decode().lower(), b"/" + name)

    # What's no name and no array.
    for label, value in [("number", b"7"), ("string", b"(DeviceRGB)"), ("null", b"null"),
                         ("true", b"true"), ("dict", b"<< >>"), ("empty-array", b"[]")]:
        named("value-" + label, value)
    named("value-dangling", b"99 0 R")
    named("value-object-zero", b"0 0 R")
    named("value-wrong-generation", b"5 1 R", [b"/DeviceCMYK"])

    # A reference is followed once.
    named("ref-name", b"5 0 R", [b"/DeviceCMYK"])
    named("ref-ref-name", b"5 0 R", [b"6 0 R", b"/DeviceCMYK"])
    named("ref-array", b"5 0 R", [b"[/Pattern /DeviceRGB]"])
    named("ref-null", b"5 0 R", [b"null"])
    named("ref-stream", b"5 0 R", [stream(b"/A /DeviceCMYK")])

    # An array of one.
    for label, value in [("devicegray", b"[/DeviceGray]"), ("devicergb", b"[/DeviceRGB]"),
                         ("cmyk", b"[/CMYK]"), ("pattern", b"[/Pattern]"), ("indexed", b"[/Indexed]"),
                         ("calrgb", b"[/CalRGB]"), ("lab", b"[/Lab]"), ("foo", b"[/Foo]"),
                         ("nested", b"[[/DeviceCMYK]]"), ("string", b"[(DeviceCMYK)]"),
                         ("number", b"[7]"), ("null", b"[null]"), ("nested-3", b"[[[/G]]]"),
                         ("dict", b"[<< >>]"), ("nested-pattern", b"[[/Pattern]]")]:
        named("one-" + label, value)
    named("one-ref-name", b"[5 0 R]", [b"/DeviceCMYK"])
    named("one-ref-ref-name", b"[5 0 R]", [b"6 0 R", b"/DeviceCMYK"])
    named("one-dangling", b"[99 0 R]")
    named("one-ref-stream", b"[5 0 R]", [stream(b"/A /DeviceCMYK")])
    named("one-nested-40", b"[" * 40 + b"/DeviceCMYK" + b"]" * 40)

    # An array of more, its family a device name, or no family.
    for label, value in [("devicegray", b"[/DeviceGray 1]"), ("devicergb", b"[/DeviceRGB 1]"),
                         ("devicecmyk", b"[/DeviceCMYK 1]"), ("g", b"[/G 1]"), ("rgb", b"[/RGB 1]"),
                         ("cmyk", b"[/CMYK 1]"), ("foo", b"[/Foo 1]"), ("cmyk-null", b"[/DeviceCMYK null]"),
                         ("as-device-n", b"[/DeviceCMYK [/A] /DeviceGray " + F2_1 + b"]"),
                         ("null-first", b"[null /DeviceRGB]"), ("number-first", b"[7 /DeviceRGB]")]:
        named("more-" + label, value)

    # The CIE families.
    for family in [b"CalGray", b"CalRGB", b"Lab"]:
        low = family.decode().lower()
        named(low + "-ok", b"[/" + family + b" " + CAL + b"]")
        named(low + "-number", b"[/" + family + b" 7]")
        named(low + "-no-white-point", b"[/" + family + b" << >>]")
        named(low + "-white-point-short", b"[/" + family + b" << /WhitePoint [1 1] >>]")
        named(low + "-y-not-1", b"[/" + family + b" << /WhitePoint [0.9 0.9 0.9] >>]")
        named(low + "-x-0", b"[/" + family + b" << /WhitePoint [0 1 1] >>]")
        named(low + "-stream", b"[/" + family + b" 5 0 R]", [stream(b"/WhitePoint [1 1 1]")])
        named(low + "-string-family", b"[(" + family + b") " + CAL + b"]")
        named(low + "-longer-family", b"[/" + family + b"X " + CAL + b"]")
        named(low + "-ref-ref", b"[/" + family + b" 5 0 R]", [b"6 0 R", WP1])

    # A white point's numbers.
    for label, point in [("y-0-9999", b"[1 0.9999 1]"), ("y-0-9999999", b"[1 0.9999999 1]"),
                         ("y-0-99999999", b"[1 0.99999999 1]"), ("y-0-99999999999", b"[1 0.99999999999 1]"),
                         ("y-1-0000000001", b"[1 1.0000000001 1]"), ("y-1-00000001", b"[1 1.00000001 1]"),
                         ("y-1-0000001", b"[1 1.0000001 1]"), ("y-1-000001", b"[1 1.000001 1]"),
                         ("y-1-0", b"[1 1.0 1]"), ("y-1-dot", b"[1 1. 1]"),
                         ("y-1-many-zeros", b"[1 1.00000000000000000000 1]"), ("y-plus-1", b"[1 +1 1]"),
                         ("y-true", b"[1 true 1]"), ("y-string", b"[1 (1) 1]"),
                         ("y-exponent", b"[1 1000000000000e-12 1]"),
                         ("x-tiny", b"[0.0000000000000000000000000000000000000000000000001 1 1]"),
                         ("x-exponent", b"[1e5 1 1]"), ("x-minus-0", b"[-0 1 1]"), ("x-true", b"[true 1 1]"),
                         ("x-string", b"[(1) 1 1]"), ("x-negative", b"[-1 1 1]"),
                         ("x-past-32-bits", b"[4294967296 1 1]"), ("x-32-bits", b"[4294967295 1 1]"),
                         ("x-past-31-bits", b"[2147483648 1 1]"),
                         ("y-past-32-bits", b"[1 4294967297 1]"), ("z-0", b"[1 1 0]"),
                         ("y-rounds-up-from-half", b"[1 1.00000005960464478 1]"),
                         ("y-rounds-down-from-half", b"[1 1.00000005960464477 1]"),
                         ("x-plus-past-31-bits", b"[+3000000000 1 1]"),
                         ("x-past-64-bits", b"[99999999999999999999 1 1]"),
                         ("z-true", b"[1 1 true]"), ("long-4", b"[1 1 1 1]"), ("long-5", b"[1 1 1 7 7]")]:
        named("white-point-" + label, b"[/CalGray << /WhitePoint " + point + b" >>]")
    named("white-point-rgb-y-0-99999999999", b"[/CalRGB << /WhitePoint [1 0.99999999999 1] >>]")
    named("white-point-rgb-y-0-9999", b"[/CalRGB << /WhitePoint [1 0.9999 1] >>]")
    named("white-point-ref", b"[/CalGray << /WhitePoint 5 0 R >>]", [b"[1 1 1]"])
    named("white-point-ref-ref", b"[/CalGray << /WhitePoint 5 0 R >>]", [b"6 0 R", b"[1 1 1]"])
    named("white-point-element-ref", b"[/CalGray << /WhitePoint [5 0 R 1 1] >>]", [b"1"])
    named("white-point-element-ref-ref", b"[/CalGray << /WhitePoint [5 0 R 1 1] >>]", [b"6 0 R", b"1"])
    named("white-point-y-ref", b"[/CalGray << /WhitePoint [1 5 0 R 1] >>]", [b"1"])
    named("white-point-y-ref-ref", b"[/CalGray << /WhitePoint [1 5 0 R 1] >>]", [b"6 0 R", b"1"])
    named("white-point-y-ref-true", b"[/CalGray << /WhitePoint [1 5 0 R 1] >>]", [b"true"])
    named("white-point-real-y", b"[/CalGray << /WhitePoint [1 1.0 1] >>]")

    # A family by its first four bytes. Lab counts 3 whether it loads or not,
    # so it's told apart under a Pattern space.
    named("family-labz", b"[/Labz " + WP1 + b"]")
    named("family-lab-nul", b"[/Lab#00zz " + WP1 + b"]")
    named("family-lab-nul-number", b"[/Lab#00z 7]")
    named("family-lab-number", b"[/Lab 7]")
    named("family-under-pattern-lab", b"[/Pattern [/Lab " + WP1 + b"]]")
    named("family-under-pattern-labz", b"[/Pattern [/Labz " + WP1 + b"]]")
    named("family-under-pattern-lab-nul", b"[/Pattern [/Lab#00zz " + WP1 + b"]]")
    named("family-under-pattern-lab-number", b"[/Pattern [/Lab 7]]")
    named("family-i-nul-z", b"[/I#00z /RGB 1 <00>]")
    named("family-i-nul", b"[/I#00 /RGB 1 <00>]")
    named("family-iz", b"[/Iz /RGB 1 <00>]")
    named("family-string-i", b"[(I) /RGB 1 <00>]")
    named("family-string-pattern", b"[(Pattern) /DeviceRGB]")
    named("family-hex-pattern", b"[<50617474> /DeviceRGB]")
    named("family-ref-stream", b"[5 0 R /DeviceRGB]", [stream(b"/A /Pattern")])
    named("family-cal", b"[/Cal " + WP1 + b"]")
    named("family-calg", b"[/CalG " + WP1 + b"]")
    named("family-ref", b"[5 0 R /DeviceRGB]", [b"/Pattern"])
    named("family-ref-string", b"[5 0 R /DeviceRGB]", [b"(Pattern)"])
    for n in (2, 3, 4, 8, 31, 32, 33, 40):
        named("family-hops-%d" % n, b"[5 0 R /DeviceRGB]", chain(n, b"/Pattern"))
    named("family-element-ref", b"[5 0 R 1]", [b"/Pattern"])
    named("family-element-ref-ref", b"[5 0 R /DeviceRGB]", [b"6 0 R", b"/Pattern"])
    named("family-dangling", b"[99 0 R /DeviceRGB]")

    # ICCBased, and /N as the integer accessor reads it.
    for label, n in [("1", b"1"), ("2", b"2"), ("3", b"3"), ("4", b"4"), ("5", b"5"), ("0", b"0"),
                     ("3-0", b"3.0"), ("3-7", b"3.7"), ("4-7", b"4.7"), ("1-5", b"1.5"), ("string-3", b"(3)"),
                     ("string-4", b"(4)"), ("name-4", b"/4"), ("minus-1", b"-1"), ("minus-3", b"-3"),
                     ("true", b"true"), ("false", b"false"), ("1-99999999999", b"1.99999999999"),
                     ("3-9999999999999", b"3.9999999999999"), ("4294967299", b"4294967299"),
                     ("4294967300", b"4294967300"), ("4294967297", b"4294967297"),
                     ("2147483651", b"2147483651"), ("2147483652", b"2147483652"),
                     ("past-64-bits", b"99999999999999999999"), ("minus-past-31-bits", b"-4294967295"),
                     ("real-past-32-bits", b"4294967300.0")]:
        named("icc-n-" + label, b"[/ICCBased 5 0 R]", [stream(b"/N " + n)])
    named("icc-n-ref", b"[/ICCBased 5 0 R]", [stream(b"/N 6 0 R"), b"4"])
    named("icc-n-ref-true", b"[/ICCBased 5 0 R]", [stream(b"/N 6 0 R"), b"true"])
    named("icc-n-ref-real", b"[/ICCBased 5 0 R]", [stream(b"/N 6 0 R"), b"4.7"])
    named("icc-n-ref-ref", b"[/ICCBased 5 0 R]", [stream(b"/N 6 0 R"), b"7 0 R", b"4"])
    named("icc-n-written-twice", b"[/ICCBased 5 0 R]", [stream(b"/N 1 /N 4")])
    named("icc-no-n", b"[/ICCBased 5 0 R]", [stream(b"")])
    named("icc-dict", b"[/ICCBased << /N 4 >>]")
    named("icc-number", b"[/ICCBased 7]")
    named("icc-alternate-rgb", b"[/ICCBased 5 0 R]", [stream(b"/N 4 /Alternate /DeviceRGB")])
    named("icc-alternate-foo", b"[/ICCBased 5 0 R]", [stream(b"/N 4 /Alternate /Foo")])
    named("icc-alternate-self", b"[/ICCBased 5 0 R]", [stream(b"/N 4 /Alternate [/ICCBased 5 0 R]")])
    named("icc-longer-family", b"[/ICCBasedX 5 0 R]", [stream(b"/N 4")])
    named("icc-extra-elements", b"[/ICCBased 5 0 R 7 7]", [stream(b"/N 4")])
    named("icc-ref-ref", b"[/ICCBased 5 0 R]", [b"6 0 R", stream(b"/N 4")])
    named("icc-data-no-filter-reads", b"[/ICCBased 5 0 R]",
          [stream(b"/N 4 /Filter /FlateDecode", b"not flate")])
    named("icc-wrong-generation", b"[/ICCBased 5 1 R]", [stream(b"/N 4")])

    # Indexed.
    for label, value in [("ok", b"[/Indexed /DeviceRGB 1 <00>]"), ("i-rgb", b"[/I /RGB 1 <00>]"),
                         ("three", b"[/Indexed /DeviceRGB 1]"), ("two", b"[/Indexed /DeviceRGB]"),
                         ("base-foo", b"[/Indexed /Foo 1 <00>]"), ("base-pattern", b"[/Indexed /Pattern 1 <00>]"),
                         ("base-indexed", b"[/Indexed [/Indexed /G 1 <00>] 1 <00>]"),
                         ("base-one", b"[/Indexed [/G] 1 <00>]"), ("base-one-one", b"[/Indexed [[/G]] 1 <00>]"),
                         ("base-one-pattern", b"[/Indexed [/Pattern] 1 <00>]"),
                         ("base-separation", b"[/Indexed [/Separation /X /DeviceCMYK " + F2_4 + b"] 1 <00>]"),
                         ("hival-999", b"[/Indexed /DeviceRGB 999 <00>]"), ("hival-minus-1", b"[/Indexed /DeviceRGB -1 <00>]"),
                         ("hival-string", b"[/Indexed /DeviceRGB (x) <00>]"), ("table-number", b"[/Indexed /DeviceRGB 1 7]"),
                         ("table-null", b"[/Indexed /DeviceRGB 1 null]"), ("table-dangling", b"[/Indexed /DeviceRGB 1 99 0 R]"),
                         ("longer-family", b"[/IndexedX /RGB 1 <00>]"), ("four-bytes", b"[/Inde /RGB 1 <00>]"),
                         ("three-bytes", b"[/Ind /RGB 1 <00>]")]:
        named("indexed-" + label, value)
    named("indexed-base-stream", b"[/Indexed 5 0 R 1 <00>]", [stream(b"/A /DeviceCMYK")])
    named("indexed-base-self", b"[/Indexed 5 0 R 1 <00>]", [b"[/Indexed 5 0 R 1 <00>]"])
    named("indexed-base-ref-ref", b"[/Indexed 5 0 R 1 <00>]", [b"6 0 R", b"/DeviceRGB"])
    named("indexed-table-ref-ref", b"[/Indexed /G 1 5 0 R]", [b"6 0 R", b"<00>"])
    named("indexed-table-ref-null", b"[/Indexed /G 1 5 0 R]", [b"null"])
    named("indexed-table-stream", b"[/Indexed /G 1 5 0 R]", [stream(b"", b"\0\1")])
    named("indexed-hival-ref", b"[/Indexed /G 5 0 R <00>]", [b"1"])

    # Separation.
    for label, value in [("none", b"[/Separation /None]"), ("none-string", b"[/Separation (None) 7]"),
                         ("no-alternate", b"[/Separation /X]"), ("alternate-foo", b"[/Separation /X /Foo]"),
                         ("no-function", b"[/Separation /X /DeviceCMYK]"),
                         ("ok", b"[/Separation /X /DeviceCMYK " + F2_4 + b"]"),
                         ("function-bad", b"[/Separation /X /DeviceCMYK " + F_BAD + b"]"),
                         ("alternate-indexed", b"[/Separation /X [/Indexed /G 1 <00>] " + F2_1 + b"]"),
                         ("alternate-pattern", b"[/Separation /X /Pattern " + F2_1 + b"]"),
                         ("all", b"[/Separation /All /DeviceCMYK " + F2_4 + b"]"), ("name-number", b"[/Separation 7 /DeviceGray]"),
                         ("longer-family", b"[/SeparationX /X /DeviceCMYK]"),
                         ("alternate-separation", b"[/Separation /X [/Separation /Y /G]]"),
                         ("alternate-one-string", b"[/Separation /X [(DeviceCMYK)]]"),
                         ("alternate-one-foo", b"[/Separation /X [/Foo]]"),
                         ("alternate-one-one", b"[/Separation /X [[/G]]]"),
                         ("alternate-one-pattern", b"[/Separation /X [/Pattern]]")]:
        named("separation-" + label, value)
    named("separation-alternate-stream", b"[/Separation /X 5 0 R]", [stream(b"/A /DeviceCMYK")])
    named("separation-alternate-stream-foo", b"[/Separation /X 5 0 R]", [stream(b"/A /Foo")])
    named("separation-alternate-stream-pattern", b"[/Separation /X 5 0 R]", [stream(b"/A /Pattern")])
    named("separation-alternate-ref-ref", b"[/Separation /A 5 0 R]", [b"6 0 R", b"/DeviceRGB"])
    named("separation-alternate-one-ref-stream", b"[/Separation /A [5 0 R]]", [stream(b"/A /DeviceRGB")])
    named("separation-none-hex", b"[/Separation <4E6F6E65>]")
    named("separation-none-ref", b"[/Separation 5 0 R]", [b"/None"])
    named("separation-none-ref-ref", b"[/Separation 5 0 R]", [b"6 0 R", b"/None"])
    named("separation-stream-key-order", b"[/Separation /A 5 0 R " + F2_1 + b"]", [stream(b"/Z /DeviceGray /A /DeviceCMYK")])
    named("separation-stream-key-c0", b"[/Separation /A 5 0 R << /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0 0] /C1 [0 0 0 0]>>]",
          [stream(b"/Z /DeviceGray /A /DeviceCMYK")])

    # A stream as an alternate: which of its values, told by the outputs of a
    # function of one.
    device_n_1 = b"[/DeviceN [/A] 5 0 R " + F2_1 + b"]"
    for label, keys in [("one", b"/Z /DeviceGray"), ("first-by-key", b"/Z /DeviceGray /A /DeviceCMYK"),
                        ("first-written-not-kept", b"/A /DeviceCMYK /Z /DeviceGray"),
                        ("last-of-a-key", b"/A /DeviceCMYK /A /DeviceGray"),
                        ("last-of-a-key-no-space", b"/A /DeviceGray /A /Foo"), ("no-space-passed", b"/A /Foo /Z /DeviceGray"),
                        ("empty-key", b"/ /DeviceCMYK /Z /DeviceGray"), ("by-reference", b"/A 6 0 R /Z /DeviceGray"),
                        ("abbreviation", b"/A /G"), ("key-order-bytes", b"/a /DeviceCMYK /Z /DeviceGray")]:
        extra = [stream(keys)] + ([b"/DeviceCMYK"] if b"6 0 R" in keys else [])
        named("stream-" + label, device_n_1, extra)

    # DeviceN against the outputs of its function.
    def device_n(name, function, extra=()):
        named("device-n-" + name, b"[/DeviceN " + A + b" /DeviceCMYK" + (b" " + function if function else b"") + b"]", extra)

    for label, function in [("f2-4", F2_4), ("f2-3", F2_3), ("f2-1", F2_1), ("f2-2-inputs", F2_2I),
                            ("f2-range-4", F2_RANGE), ("f2-no-n", F2_NO_N), ("f3", F3), ("f-bad", F_BAD),
                            ("no-function", None), ("function-name", b"/Identity"), ("function-null", b"null"),
                            ("f0-dict", b"<< " + F0_2IN4OUT + b" >>"), ("f4-dict", b"<< " + F4_2IN4OUT + b" >>")]:
        device_n(label, function)
    device_n("f0-ok", b"5 0 R", [stream(F0_2IN4OUT, b"x" * 16)])
    device_n("f0-short", b"5 0 R", [stream(F0_2IN4OUT, b"x" * 3)])
    device_n("f4-ok", b"5 0 R", [stream(F4_2IN4OUT, b"{ pop pop 0 0 0 0 }")])
    device_n("f4-bad", b"5 0 R", [stream(F4_2IN4OUT, b"{ nonsense")])
    device_n("f4-no-range", b"5 0 R", [stream(b"/FunctionType 4 /Domain [0 1 0 1]", b"{ pop pop 0 0 0 0 }")])
    for label, value, extra in [
            ("names-empty", b"[/DeviceN [] /DeviceGray " + F2_1 + b"]", ()),
            ("names-name", b"[/DeviceN /A /DeviceGray " + F2_1 + b"]", ()),
            ("names-ref", b"[/DeviceN 5 0 R /DeviceGray " + F2_1 + b"]", [A]),
            ("names-ref-ref", b"[/DeviceN 5 0 R /DeviceGray " + F2_1 + b"]", [b"6 0 R", A]),
            ("names-9", b"[/DeviceN [/A /B /C /D /E /F /G /H /I] /DeviceGray " + F2_1 + b"]", ()),
            ("alternate-foo", b"[/DeviceN " + A + b" /Foo " + F2_1 + b"]", ()),
            ("alternate-pattern", b"[/DeviceN " + A + b" /Pattern " + F2_1 + b"]", ()),
            ("alternate-indexed", b"[/DeviceN " + A + b" [/Indexed /G 1 <00>] " + F2_1 + b"]", ()),
            ("alternate-one", b"[/DeviceN [/A] [/DeviceGray] " + F2_1 + b"]", ()),
            ("alternate-one-one", b"[/DeviceN [/A] [[/DeviceGray]] " + F2_1 + b"]", ()),
            ("alternate-icc-3", b"[/DeviceN [/A] [/ICCBased 5 0 R] " + F2_3 + b"]", [stream(b"/N 3")]),
            ("alternate-icc-4", b"[/DeviceN [/A] [/ICCBased 5 0 R] " + F2_3 + b"]", [stream(b"/N 4")]),
            ("attributes", b"[/DeviceN " + A + b" /DeviceGray " + F2_1 + b" << /Subtype /NChannel >>]", ()),
            ("names-not-names", b"[/DeviceN [7 (x)] /DeviceGray " + F2_1 + b"]", ()),
            ("function-ref-ref", b"[/DeviceN [/A] /DeviceGray 5 0 R]", [b"6 0 R", F2_1]),
            ("function-shares-a-stream", b"[/DeviceN [/A] [/ICCBased 5 0 R] 5 0 R]",
             [stream(b"/N 3 /FunctionType 2 /Domain [0 1] /C0 [0 0 0]")]),
            ("stream-is-alternate-and-function", b"[/DeviceN [/A] 5 0 R 5 0 R]",
             [stream(b"/FunctionType 2 /Domain [0 1] /N 1 /Z /DeviceGray")])]:
        named("device-n-" + label, value, extra)

    # A function's dictionary.
    def function_case(name, dict_, data=b"x" * 400, extra=()):
        res_case(name, b"<< /ColorSpace << /X [/DeviceN [/A /B] /DeviceGray 5 0 R] >> >>", [stream(dict_, data)] + list(extra))

    def dict_function(name, dict_, extra=()):
        res_case(name, b"<< /ColorSpace << /X [/DeviceN [/A /B] /DeviceGray 5 0 R] >> >>", [b"<< " + dict_ + b" >>"] + list(extra))

    for label, more in [("ok", b"/Size [2] /BitsPerSample 8"), ("no-size", b"/BitsPerSample 8"),
                        ("size-empty", b"/Size [] /BitsPerSample 8"), ("size-0", b"/Size [0] /BitsPerSample 8"),
                        ("size-minus-1", b"/Size [-1] /BitsPerSample 8"), ("size-number", b"/Size 2 /BitsPerSample 8"),
                        ("bits-7", b"/Size [2] /BitsPerSample 7"), ("bits-3", b"/Size [2] /BitsPerSample 3"),
                        ("bits-64", b"/Size [2] /BitsPerSample 64"), ("bits-minus-8", b"/Size [2] /BitsPerSample -8"),
                        ("no-bits", b"/Size [2]"), ("bits-true", b"/Size [2] /BitsPerSample true"),
                        ("bits-8-9", b"/Size [2] /BitsPerSample 8.9"), ("bits-1", b"/Size [2] /BitsPerSample 1"),
                        ("bits-2", b"/Size [2] /BitsPerSample 2"), ("bits-4", b"/Size [2] /BitsPerSample 4"),
                        ("bits-12", b"/Size [2] /BitsPerSample 12"), ("bits-16", b"/Size [2] /BitsPerSample 16"),
                        ("bits-24", b"/Size [2] /BitsPerSample 24"), ("bits-32", b"/Size [2] /BitsPerSample 32"),
                        ("size-real", b"/Size [2.7] /BitsPerSample 8"), ("size-true", b"/Size [true] /BitsPerSample 8"),
                        ("size-string", b"/Size [(2)] /BitsPerSample 8"), ("overflow", b"/Size [536870912] /BitsPerSample 32")]:
        function_case("f0-" + label, F0_HEAD + more)
    function_case("f0-two-inputs-size-1", b"/FunctionType 0 /Domain [0 1 0 1] /Range [0 1] /Size [2] /BitsPerSample 8")
    function_case("f0-two-inputs-size-2", b"/FunctionType 0 /Domain [0 1 0 1] /Range [0 1] /Size [2 2] /BitsPerSample 8")
    function_case("f0-no-range", b"/FunctionType 0 /Domain [0 1] /Size [2] /BitsPerSample 8")
    function_case("f0-no-type", b"/Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8")
    function_case("f0-type-string", b"/FunctionType (2) /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8")
    function_case("f0-type-false", b"/FunctionType false /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8")
    function_case("f0-size-ref", F0_HEAD + b"/Size 6 0 R /BitsPerSample 8", extra=[b"[2]"])
    function_case("f0-size-ref-ref", F0_HEAD + b"/Size 6 0 R /BitsPerSample 8", extra=[b"7 0 R", b"[2]"])
    function_case("f0-bits-ref", F0_HEAD + b"/Size [2] /BitsPerSample 6 0 R", extra=[b"8"])
    function_case("f0-bits-ref-ref", F0_HEAD + b"/Size [2] /BitsPerSample 6 0 R", extra=[b"7 0 R", b"8"])
    function_case("f0-size-element-ref", F0_HEAD + b"/Size [6 0 R] /BitsPerSample 8", extra=[b"2"])
    function_case("f0-size-element-ref-ref", F0_HEAD + b"/Size [6 0 R] /BitsPerSample 8", extra=[b"7 0 R", b"2"])
    for label, dict_ in [("type-2-5", b"/FunctionType 2.5 /Domain [0 1] /N 1"), ("type-true", b"/FunctionType true /Domain [0 1] /N 1"),
                         ("type-missing", b"/Domain [0 1] /N 1"), ("type-1", b"/FunctionType 1 /Domain [0 1] /N 1"),
                         ("type-5", b"/FunctionType 5 /Domain [0 1] /N 1"), ("domain-odd-3", b"/FunctionType 2 /Domain [0 1 0] /N 1"),
                         ("domain-1", b"/FunctionType 2 /Domain [0] /N 1"), ("domain-empty", b"/FunctionType 2 /Domain [] /N 1"),
                         ("no-domain", b"/FunctionType 2 /N 1"), ("domain-number", b"/FunctionType 2 /Domain 7 /N 1"),
                         ("f2-n-real", b"/FunctionType 2 /Domain [0 1] /N 1.5"), ("f2-n-true", b"/FunctionType 2 /Domain [0 1] /N true"),
                         ("f2-n-string", b"/FunctionType 2 /Domain [0 1] /N (1)"), ("f2-no-n", b"/FunctionType 2 /Domain [0 1]"),
                         ("f4-parses-no-n", b"/FunctionType 4 /Domain [0 1] /Range [0 1]")]:
        data = b"{ }" if b"FunctionType 4" in dict_ else b"x" * 400
        function_case("fn-" + label, dict_, data)
    dict_function("fn-domain-ref", b"/FunctionType 2 /N 1 /Domain 6 0 R", [b"[0 1]"])
    dict_function("fn-domain-ref-ref", b"/FunctionType 2 /N 1 /Domain 6 0 R", [b"7 0 R", b"[0 1]"])
    dict_function("fn-type-ref", b"/FunctionType 6 0 R /N 1 /Domain [0 1]", [b"2"])
    dict_function("fn-type-ref-ref", b"/FunctionType 6 0 R /N 1 /Domain [0 1]", [b"7 0 R", b"2"])
    dict_function("fn-f2-n-ref", b"/FunctionType 2 /Domain [0 1] /N 6 0 R", [b"1"])
    named("fn-domain-ref-ref-direct", b"[/DeviceN [/A] /DeviceGray << /FunctionType 2 /N 1 /Domain 5 0 R >>]", [b"6 0 R", b"[0 1]"])

    # A Type 2's outputs, told by an alternate of four.
    def in_cmyk(name, function, extra=()):
        res_case(name, b"<< /ColorSpace << /X [/DeviceN [/A /B] /DeviceCMYK " + function + b"] >> >>", extra)

    in_cmyk("f2-outputs-range-before-c0", b"<< /FunctionType 2 /Domain [0 1] /N 1 /Range [0 1] /C0 [0 0 0 0] >>")
    in_cmyk("f2-outputs-c0-empty", b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [] >>")
    in_cmyk("f2-outputs-c0-ref", b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 5 0 R >>", [b"[0 0 0 0]"])
    in_cmyk("f2-outputs-c0-ref-ref", b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 5 0 R >>", [b"6 0 R", b"[0 0 0 0]"])
    in_cmyk("f2-outputs-c1-not-asked", b"<< /FunctionType 2 /Domain [0 1] /N 1 /C1 [1 1 1 1] >>")
    in_cmyk("f2-outputs-c0-number", b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 0 >>")
    in_cmyk("f2-outputs-range-odd", b"<< /FunctionType 2 /Domain [0 1] /N 1 /Range [0 1 0 1 0 1 0 1 0] >>")
    in_cmyk("f2-outputs-range-ref", b"<< /FunctionType 2 /Domain [0 1] /N 1 /Range 5 0 R >>", [b"[0 1 0 1 0 1 0 1]"])
    in_cmyk("f2-outputs-range-empty-c0", b"<< /FunctionType 2 /Domain [0 1] /N 1 /Range [] /C0 [0 0 0 0] >>")
    in_cmyk("f2-outputs-2-inputs-1", b"<< /FunctionType 2 /Domain [0 1 0 1] /N 1 >>")
    in_cmyk("f2-outputs-4-inputs-1", b"<< /FunctionType 2 /Domain [0 1 0 1 0 1 0 1] /N 1 >>")

    # A stitching function.
    def stitched(domain, kids, bounds, encode, more=b""):
        return (b"<< /FunctionType 3 /Domain " + domain + b" /Functions " + kids + b" /Bounds " + bounds
                + b" /Encode " + encode + more + b" >>")

    F1_2I = b"<< /FunctionType 2 /Domain [0 1 0 1] /N 1 >>"
    one, two = b"[" + F2_1 + b"]", b"[" + F2_1 + b" " + F2_1 + b"]"
    for label, function in [("one", stitched(b"[0 1]", one, b"[]", b"[0 1]")),
                            ("two", stitched(b"[0 1]", two, b"[0.5]", b"[0 1 0 1]")),
                            ("two-inputs", stitched(b"[0 1 0 1]", one, b"[]", b"[0 1]")),
                            ("no-kids", stitched(b"[0 1]", b"[]", b"[]", b"[]")),
                            ("kids-number", stitched(b"[0 1]", b"7", b"[]", b"[0 1]")),
                            ("kid-two-inputs", stitched(b"[0 1]", b"[" + F1_2I + b"]", b"[]", b"[0 1]")),
                            ("kids-unalike", stitched(b"[0 1]", b"[" + F2_1 + b" " + F2_3 + b"]", b"[0]", b"[0 1 0 1]")),
                            ("bounds-short", stitched(b"[0 1]", two, b"[]", b"[0 1 0 1]")),
                            ("encode-short", stitched(b"[0 1]", two, b"[0.5]", b"[0 1]")),
                            ("more-than-enough", stitched(b"[0 1]", two, b"[0 0 0]", b"[0 1 0 1 0 1]")),
                            ("bounds-number", stitched(b"[0 1]", one, b"7", b"[0 1]")),
                            ("encode-number", stitched(b"[0 1]", one, b"[]", b"7")),
                            ("kid-bad", stitched(b"[0 1]", b"[" + F_BAD + b"]", b"[]", b"[0 1]"))]:
        res_case("f3-" + label, b"<< /ColorSpace << /X [/DeviceN [/A /B] /DeviceGray " + function + b"] >> >>")
    in_cmyk("f3-range-not-its-outputs", stitched(b"[0 1]", one, b"[]", b"[0 1]", b" /Range [0 1 0 1 0 1 0 1]"))
    in_cmyk("f3-outputs-its-kids", stitched(b"[0 1]", b"[" + F2_4 + b"]", b"[]", b"[0 1]"))
    selfish = stitched(b"[0 1]", b"[5 0 R]", b"[]", b"[0 1]")
    other = stitched(b"[0 1]", b"[6 0 R]", b"[]", b"[0 1]")
    dict_function("f3-itself", selfish[3:-3])
    dict_function("f3-by-another", other[3:-3], [selfish])
    dict_function("f3-kid-twice", stitched(b"[0 1]", b"[6 0 R 6 0 R]", b"[0]", b"[0 1 0 1]")[3:-3], [F2_1])
    dict_function("f3-kid-ref", other[3:-3], [F2_1])
    dict_function("f3-kid-ref-ref", other[3:-3], [b"7 0 R", F2_1])

    # A Pattern space.
    for label, value in [("rgb", b"[/Pattern /DeviceRGB]"), ("cmyk", b"[/Pattern /DeviceCMYK]"), ("gray", b"[/Pattern /DeviceGray]"),
                         ("foo", b"[/Pattern /Foo]"), ("number", b"[/Pattern 7]"), ("null", b"[/Pattern null]"), ("dangling", b"[/Pattern 99 0 R]"),
                         ("pattern", b"[/Pattern /Pattern]"), ("one-pattern", b"[/Pattern [/Pattern]]"),
                         ("indexed", b"[/Pattern [/Indexed /G 1 <00>]]"), ("pattern-g", b"[/Pattern [/Pattern /G]]"),
                         ("device-n-17", b"[/Pattern [/DeviceN [/A /B /C /D /E /F /G /H /I /J /K /L /M /N /O /P /Q] /DeviceGray " + F2_1 + b"]]"),
                         ("device-n-16", b"[/Pattern [/DeviceN [/A /B /C /D /E /F /G /H /I /J /K /L /M /N /O /P] /DeviceGray " + F2_1 + b"]]"),
                         ("longer-family", b"[/PatternX /DeviceRGB]"), ("more-elements", b"[/Pattern /DeviceRGB 7 7 7]"),
                         ("one-rgb", b"[/Pattern [/RGB]]"), ("separation", b"[/Pattern [/Separation /X /G]]"),
                         ("cal-gray", b"[/Pattern [/CalGray " + WP1 + b"]]"), ("cal-gray-bad", b"[/Pattern [/CalGray 7]]")]:
        named("pattern-" + label, value)
    named("pattern-itself-in-object", b"[/Pattern 5 0 R]", [b"[/Pattern 5 0 R]"])
    named("pattern-stream", b"[/Pattern 5 0 R]", [stream(b"/A /DeviceCMYK")])
    named("pattern-icc", b"[/Pattern [/ICCBased 5 0 R]]", [stream(b"/N 4")])
    named("pattern-ref-null", b"[/Pattern 5 0 R]", [b"null"])

    # Loops, and an object met twice with no loop.
    F = F2_1
    named("loop-self-array", b"5 0 R", [b"[/Pattern 5 0 R]"])
    named("loop-self-one", b"5 0 R", [b"[5 0 R]"])
    named("loop-entry", b"5 0 R", [b"5 0 R"])
    named("loop-mutual-indexed", b"5 0 R", [b"[/Indexed 6 0 R 1 <00>]", b"[/Separation /A 5 0 R " + F + b"]"])
    named("loop-mutual-pattern", b"5 0 R", [b"[/Pattern 6 0 R]", b"[/Pattern 5 0 R]"])
    named("loop-pattern-by-one", b"5 0 R", [b"[/Pattern 6 0 R]", b"[5 0 R]"])
    named("loop-one-by-pattern", b"5 0 R", [b"[6 0 R]", b"[/Pattern 7 0 R]", b"[5 0 R]"])
    named("loop-one-by-separation", b"5 0 R", [b"[6 0 R]", b"[/Separation /A 5 0 R]"])
    named("diamond", b"[/Indexed 5 0 R 1 <00>]", [b"[/Separation /A 6 0 R " + F + b"]", b"[/ICCBased 7 0 R]", stream(b"/N 3 /Alternate 6 0 R")])

    # How deep: by reference PDFium has no cap but its stack.
    def array_chain(n):
        return [b"[%d 0 R]" % (6 + i) for i in range(n)] + [b"/DeviceCMYK"]

    for n in (10, 63, 64, 65, 70):
        named("depth-array-chain-%d" % n, b"5 0 R", array_chain(n))

    # /Resources and /ColorSpace, each through at most two references.
    CS4 = b"<< /ColorSpace << /X /DeviceCMYK >> >>"
    for n in (1, 2, 3, 4, 8, 31, 32, 33, 40):
        res_case("hops-res-%d" % n, b"5 0 R", chain(n, CS4))
        res_case("hops-cs-%d" % n, b"<< /ColorSpace 5 0 R >>", chain(n, b"<< /X /DeviceCMYK >>"))
        res_case("hops-entry-%d" % n, b"<< /ColorSpace << /X 5 0 R >> >>", chain(n, b"/DeviceCMYK"))
    res_case("res-stream", b"5 0 R", [stream(b"/ColorSpace << /X /DeviceCMYK >>")])
    res_case("res-ref-ref-stream", b"5 0 R", [b"6 0 R", stream(b"/ColorSpace << /X /DeviceCMYK >>")])
    res_case("res-null", b"null")
    res_case("res-name", b"/Foo")
    res_case("res-ref-null", b"5 0 R", [b"null"])
    res_case("res-dangling", b"99 0 R")
    res_case("res-loop", b"5 0 R", [b"5 0 R"])
    res_case("res-loop-2", b"5 0 R", [b"6 0 R", b"5 0 R"])
    res_case("cs-null", b"<< /ColorSpace null >>")
    res_case("cs-array", b"<< /ColorSpace [] >>")
    res_case("cs-stream", b"<< /ColorSpace 5 0 R >>", [stream(b"/X /DeviceCMYK")])
    res_case("cs-ref-ref-stream", b"<< /ColorSpace 5 0 R >>", [b"6 0 R", stream(b"/X /DeviceCMYK")])
    res_case("cs-dangling", b"<< /ColorSpace 99 0 R >>")
    res_case("cs-loop", b"<< /ColorSpace 5 0 R >>", [b"5 0 R"])
    res_case("cs-written-twice", b"<< /ColorSpace << /X /DeviceGray >> /ColorSpace << /X /DeviceCMYK >> >>")
    res_case("entry-written-twice", b"<< /ColorSpace << /X /DeviceGray /X /DeviceCMYK >> >>")
    res_case("entry-written-twice-last-null", b"<< /ColorSpace << /X /DeviceGray /X null >> >>")
    res_case("entry-missing", b"<< /ColorSpace << /Y /DeviceCMYK >> >>")
    res_case("resources-element-pad", b"<< /ColorSpace << /X /DeviceCMYK >> /Font << >> >>")

    # Keys as each reader reads them.
    def key_case(name, key, asked):
        res_case(name, b"<< /ColorSpace << " + key + b" /DeviceCMYK >> >>", content=image(asked))

    key_case("key-escape-asked-plain", b"/X#41", b"/XA")
    key_case("key-plain-asked-escape", b"/XA", b"/X#41")
    key_case("key-short-escape-itself", b"/X#4", b"/X#4")
    key_case("key-short-escape-plain", b"/X#4", b"/X")
    key_case("key-bad-escape-itself", b"/X#G1", b"/X#G1")
    key_case("key-bad-escape-plain", b"/X#G1", b"/X")
    key_case("key-bad-escape-by-its-value", b"/X#G1", b"/X#01")
    key_case("key-empty", b"/", b"/")

    # A form: its own /ColorSpace dictionary, and its page's only if it has none.
    def form_case(name, form_res, page_res, extra=()):
        form = stream(b"/Type /XObject /Subtype /Form /BBox [0 0 9 9] " + form_res, image(b"/X"))
        res_case(name, b"<< /XObject << /F 5 0 R >> " + page_res + b" >>", [form] + list(extra), content=b"/F Do")

    PAGE_HAS = b"/ColorSpace << /X /DeviceCMYK >>"
    form_case("form-no-resources", b"", PAGE_HAS)
    form_case("form-empty-resources", b"/Resources << >>", PAGE_HAS)
    form_case("form-own-space", b"/Resources << /ColorSpace << /X /DeviceGray >> >>", PAGE_HAS)
    form_case("form-other-names", b"/Resources << /ColorSpace << /Y /DeviceGray >> >>", PAGE_HAS)
    form_case("form-empty-color-spaces", b"/Resources << /ColorSpace << >> >>", PAGE_HAS)
    form_case("form-color-space-through-three", b"/Resources << /ColorSpace 6 0 R >>", PAGE_HAS,
              [b"7 0 R", b"8 0 R", b"<< /X /DeviceGray >>"])

    # A page naming several, drawn in turn: what a reader of its content has.
    several = (b"<< /ColorSpace << /Icc [/ICCBased 5 0 R] /Cmyk /DeviceCMYK /Gray 6 0 R"
               b" /Idx [/Indexed /DeviceRGB 1 (abcdef)]"
               b" /N2 [/DeviceN [/a /b] /DeviceRGB << /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0] >>]"
               b" /Lab [/Lab << >>] /Odd 7 >> >>")
    drawn = b" ".join(image(n) for n in [b"/Icc", b"/Cmyk", b"/Gray", b"/Idx", b"/N2", b"/Lab", b"/Odd", b"/Missing"])
    res_case("several-names", several, [stream(b"/N 4"), b"/DeviceGray"], content=drawn)

    return out
