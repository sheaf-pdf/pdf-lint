"""pdf-fonts' cases, each a file whose page shows one string in one font: what
PDFium's text page gives of it is how it read the font (oracle/README.md,
`fonts`, `fonts-heights`). Written from literals; oracle/pdfium.py's `cases`
writes them into cases/ and cases.tsv.

A case's objects are its own: 1 a catalog, 2 the pages, 3 the page, 4 its
content, 10 the font, and any others as it says. A case whose name begins
`predefined-` wants a predefined CMap, so the feature of that name.

Each reader's value is written each way its accessor in PDFium tells apart: a
number, a real, true, what's no number, past 32 bits, by a reference, a chain and
a reference to nothing. What's asked of a font with no /Widths is its widths by an
own face only for the standard 14; nothing here rests on the system's fonts."""

ROWS = []


def build(font, extra, content):
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200000 20000] /Contents 4 0 R"
           b" /Resources << /Font << /F0 10 0 R >> >> >>",
        4: stream(b"", content),
        10: font,
    }
    objects.update(extra)
    out = bytearray(b"%PDF-1.7\n")
    offsets = {}
    for number in sorted(objects):
        offsets[number] = len(out)
        out += b"%d 0 obj\n" % number + objects[number] + b"\nendobj\n"
    xref = len(out)
    size = max(objects) + 1
    out += b"xref\n0 %d\n0000000000 65535 f \n" % size
    for number in range(1, size):
        out += b"%010d 00000 n \n" % offsets[number] if number in offsets else b"0000000000 65535 f \n"
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (size, xref)
    return bytes(out)


def stream(dict_, data):
    return b"<< " + dict_ + b" /Length %d >>\nstream\n" % len(data) + data + b"\nendstream"


def case(name, font, extra=None, s=b"(ABC)", kind="fonts", before=b""):
    """One string shown in the font: the origin it starts from is (0, 100)."""
    content = b"BT /F0 1000 Tf 0 100 Td " + before + s + b" Tj ET"
    ROWS.append((name, kind, {"pdf": build(font, extra or {}, content)}))


H = b"/Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 65 /LastChar 66 "
HX = b"/Type /Font /Subtype /Type1 /BaseFont /Helvetica "


def widths(name, w, head=H, extra=None, s=b"(ABC)"):
    case(name, b"<< " + head + b"/Widths " + w + b" >>", extra, s)


def simple():
    # /Widths' elements, each by GetIntegerAt: a number's integer, true, what's
    # no number, a reference followed once, and 16 bits to hold it.
    for name, w, extra in [
        ("base", b"[500 600]", None),
        ("real", b"[500.9 600.9]", None),
        ("true", b"[true 600]", None),
        ("false", b"[false 600]", None),
        ("string", b"[(x) 600]", None),
        ("name", b"[/x 600]", None),
        ("null", b"[null 600]", None),
        ("array", b"[[700] 600]", None),
        ("dict", b"[<< >> 600]", None),
        ("past-32", b"[4294967296 600]", None),
        ("past-i32", b"[2147483648 600]", None),
        ("past-i32-plus", b"[2147484148 600]", None),
        ("u32-most", b"[4294967295 600]", None),
        ("past-64", b"[99999999999999999999 600]", None),
        ("neg", b"[-500 600]", None),
        ("neg-real", b"[-500.7 600]", None),
        ("neg-one", b"[-1 600]", None),
        ("bits-65535", b"[65535 600]", None),
        ("bits-65536", b"[65536 600]", None),
        ("bits-70000", b"[70000 600]", None),
        ("bits-32768", b"[32768 600]", None),
        ("plus", b"[+500 600]", None),
        ("exp", b"[1e12 600]", None),
        ("huge-real", b"[99999999999.0 600]", None),
        ("neg-huge-real", b"[-99999999999.0 600]", None),
        ("float-cut", b"[16777217.0 600]", None),
        ("elem-ref", b"[20 0 R 600]", {20: b"700"}),
        ("elem-chain", b"[20 0 R 600]", {20: b"21 0 R", 21: b"700"}),
        ("elem-chain3", b"[20 0 R 600]", {20: b"21 0 R", 21: b"22 0 R", 22: b"700"}),
        ("elem-dangling", b"[99 0 R 600]", None),
        ("elem-ref-true", b"[20 0 R 600]", {20: b"true"}),
        ("elem-ref-null", b"[20 0 R 600]", {20: b"null"}),
        # /Widths itself, by GetArrayFor: one reference, no further.
        ("widths-ref", b"20 0 R", {20: b"[710 720]"}),
        ("widths-chain", b"20 0 R", {20: b"21 0 R", 21: b"[710 720]"}),
        ("widths-chain3", b"20 0 R", {20: b"21 0 R", 21: b"22 0 R", 22: b"[710 720]"}),
        ("widths-dangling", b"99 0 R", None),
        ("widths-dict", b"<< >>", None),
        ("widths-name", b"/x", None),
        ("widths-null", b"null", None),
        ("widths-number", b"500", None),
    ]:
        widths(name, w, extra=extra)

    # /FirstChar and /LastChar, by GetIntegerFor into a size_t.
    for name, keys, extra in [
        ("first-real", b"/FirstChar 65.9 /LastChar 66 ", None),
        ("first-true", b"/FirstChar true /LastChar 66 ", None),
        ("first-300", b"/FirstChar 300 /LastChar 66 ", None),
        ("first-255", b"/FirstChar 255 /LastChar 256 ", None),
        ("first-256", b"/FirstChar 256 /LastChar 257 ", None),
        ("first-neg", b"/FirstChar -1 /LastChar 66 ", None),
        ("first-past-32", b"/FirstChar 4294967361 /LastChar 66 ", None),
        ("first-past-i32", b"/FirstChar 4294967295 /LastChar 66 ", None),
        ("first-missing", b"/LastChar 66 ", None),
        ("first-string", b"/FirstChar (x) /LastChar 66 ", None),
        ("first-ref", b"/FirstChar 20 0 R /LastChar 66 ", {20: b"65"}),
        ("first-chain", b"/FirstChar 20 0 R /LastChar 66 ", {20: b"21 0 R", 21: b"65"}),
        ("first-dangling", b"/FirstChar 99 0 R /LastChar 66 ", None),
        ("last-65", b"/FirstChar 65 /LastChar 65 ", None),
        ("last-0", b"/FirstChar 65 /LastChar 0 ", None),
        ("last-missing", b"/FirstChar 65 ", None),
        ("last-true", b"/FirstChar 65 /LastChar true ", None),
        ("last-neg", b"/FirstChar 65 /LastChar -1 ", None),
        ("last-real", b"/FirstChar 65 /LastChar 65.9 ", None),
        ("last-before", b"/FirstChar 65 /LastChar 60 ", None),
        ("last-past", b"/FirstChar 65 /LastChar 70 ", None),
        ("last-300", b"/FirstChar 65 /LastChar 300 ", None),
        ("last-ref", b"/FirstChar 65 /LastChar 20 0 R ", {20: b"65"}),
        ("last-chain", b"/FirstChar 65 /LastChar 20 0 R ", {20: b"21 0 R", 21: b"65"}),
        ("last-string", b"/FirstChar 65 /LastChar (x) ", None),
    ]:
        widths(name, b"[500 600]", HX + keys, extra)
    widths("first-zero-long", b"[" + b" ".join(b"%d" % (100 + i) for i in range(300)) + b"]", HX + b"/FirstChar 0 ")

    # /MissingWidth, by GetIntegerFor where the key is there; and no /Widths, none.
    descriptor = b"/FontDescriptor 20 0 R "
    for name, value, extra in [
        ("missing-width", b"777", None),
        ("missing-width-real", b"777.9", None),
        ("missing-true", b"true", None),
        ("missing-string", b"(x)", None),
        ("missing-null", b"null", None),
        ("missing-ref", b"21 0 R", {21: b"777"}),
        ("missing-chain", b"21 0 R", {21: b"22 0 R", 22: b"777"}),
        ("missing-dangling", b"99 0 R", None),
        ("missing-neg", b"-500", None),
        ("missing-bits-65535", b"65535", None),
        ("missing-bits-70000", b"70000", None),
    ]:
        all_extra = {20: b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 /MissingWidth " + value + b" >>"}
        all_extra.update(extra or {})
        widths(name, b"[500]", HX + b"/FirstChar 65 /LastChar 65 " + descriptor, all_extra)
    miss = {20: b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 /MissingWidth 777 >>"}
    case("missing-no-widths", b"<< " + HX + descriptor + b">>", miss)
    widths("missing-empty-first-zero", b"[]", HX + descriptor, miss)
    widths("missing-empty-first-char", b"[]", HX + b"/FirstChar 65 " + descriptor, miss)
    widths("missing-first-300", b"[500]", HX + b"/FirstChar 300 " + descriptor, miss)

    # A standard fixed-pitch font's widths start at 600, /Widths' or not.
    cour = b"/Type /Font /Subtype /Type1 /BaseFont /Courier "
    widths("courier-widths", b"[500]", cour + b"/FirstChar 65 /LastChar 65 ")
    widths("courier-first-300", b"[500]", cour + b"/FirstChar 300 ")
    widths("courier-missing", b"[500]", cour + b"/FirstChar 65 " + descriptor, miss)
    widths("courier-truetype", b"[500]", b"/Type /Font /Subtype /TrueType /BaseFont /Courier /FirstChar 65 /LastChar 65 ")
    widths("courier-new", b"[500]", b"/Type /Font /Subtype /Type1 /BaseFont /CourierNew /FirstChar 65 /LastChar 65 ")
    widths("courier-mm", b"[500]", b"/Type /Font /Subtype /MMType1 /BaseFont /Courier /FirstChar 65 /LastChar 65 ")
    case("courier-none", b"<< " + cour + b">>")

    # A descriptor's /Flags: all caps' lower case takes its capital's width.
    def flags(value, extra=None):
        out = {20: b"<< /Type /FontDescriptor /FontName /Helvetica /Flags " + value + b" >>"}
        out.update(extra or {})
        return out
    lower = HX + b"/FirstChar 65 /LastChar 98 " + descriptor
    fill = b"[" + b" ".join(b"%d" % (300 + i) for i in range(34)) + b"]"
    widths("allcap", fill, lower, flags(b"65568"), s=b"(aAb)")
    widths("allcap-ref", fill, lower, flags(b"21 0 R", {21: b"65568"}), s=b"(aAb)")
    widths("allcap-chain", fill, lower, flags(b"21 0 R", {21: b"22 0 R", 22: b"65568"}), s=b"(aAb)")
    widths("allcap-real", fill, lower, flags(b"65568.9"), s=b"(aAb)")
    widths("allcap-none", fill, lower, flags(b"32"), s=b"(aAb)")
    widths("allcap-zero", b"[0" + fill[4:], lower, flags(b"65568"), s=b"(aAb)")
    widths("allcap-range", b"[500]", HX + b"/FirstChar 192 " + descriptor, flags(b"65568"), s=b"<e0c0d7f7>")
    case("allcap-no-widths", b"<< " + HX + descriptor + b">>", flags(b"65568"), s=b"(aAb)")
    case("allcap-truetype", b"<< /Type /Font /Subtype /TrueType /BaseFont /Helvetica /FirstChar 65 /LastChar 98 " + descriptor + b"/Widths " + fill + b" >>", flags(b"65568"), s=b"(aAb)")
    widths("flags-true", b"[500 600]", H + descriptor, flags(b"true"))
    widths("flags-symbolic", b"[500 600]", b"/Type /Font /Subtype /TrueType /BaseFont /Foo /FirstChar 65 /LastChar 66 " + descriptor, flags(b"4"), s=b"(AB\\200)")

    # /FontDescriptor, by GetDictFor: a stream's dictionary, and two references.
    miss_key = b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 /MissingWidth 777 >>"
    for name, value, extra in [
        ("desc-ref", b"20 0 R", {20: miss_key}),
        ("desc-direct", miss_key, None),
        ("desc-chain", b"20 0 R", {20: b"21 0 R", 21: miss_key}),
        ("desc-chain3", b"20 0 R", {20: b"21 0 R", 21: b"22 0 R", 22: miss_key}),
        ("desc-stream", b"20 0 R", {20: stream(b"/MissingWidth 777", b"")}),
        ("desc-stream-chain", b"20 0 R", {20: b"21 0 R", 21: stream(b"/MissingWidth 777", b"")}),
        ("desc-dangling", b"99 0 R", None),
        ("desc-name", b"/x", None),
    ]:
        widths(name, b"[500]", HX + b"/FirstChar 65 /LastChar 65 /FontDescriptor " + value + b" ", extra)

    # /Encoding, by GetDirectObjectFor: one reference, a name or a dictionary.
    def encoded(name, enc, extra=None, s=b"(ABC)", base=b"/Helvetica"):
        case(name, b"<< /Type /Font /Subtype /Type1 /BaseFont " + base
             + b" /FirstChar 65 /LastChar 66 /Widths [500 600] /Encoding " + enc + b" >>", extra, s)
    probe = b"(\\047\\140\\200\\251A)"
    encoded("winansi-a0", b"/WinAnsiEncoding", s=b"(\\240\\255\\200)")
    encoded("standard-27", b"/StandardEncoding", s=b"(\\047\\140\\251)")
    encoded("macexpert", b"/MacExpertEncoding", s=b"(AB\\047\\140\\200)")
    encoded("enc-win", b"/WinAnsiEncoding", s=probe)
    encoded("enc-mac", b"/MacRomanEncoding", s=probe)
    encoded("enc-pdfdoc", b"/PDFDocEncoding", s=probe)
    encoded("enc-unknown", b"/FooEncoding", s=probe)
    encoded("enc-ref", b"20 0 R", {20: b"/WinAnsiEncoding"}, s=probe)
    encoded("enc-chain", b"20 0 R", {20: b"21 0 R", 21: b"/WinAnsiEncoding"}, s=probe)
    encoded("enc-dangling", b"99 0 R", s=probe)
    encoded("enc-string", b"(WinAnsiEncoding)", s=probe)
    encoded("enc-number", b"7", s=probe)
    encoded("enc-true", b"true", s=probe)
    encoded("enc-null", b"null", s=probe)
    encoded("enc-stream", b"20 0 R", {20: stream(b"/BaseEncoding /WinAnsiEncoding", b"")}, s=probe)
    encoded("enc-dict-ref", b"20 0 R", {20: b"<< /BaseEncoding /WinAnsiEncoding >>"}, s=probe)
    encoded("enc-dict-chain", b"20 0 R", {20: b"21 0 R", 21: b"<< /BaseEncoding /WinAnsiEncoding >>"}, s=probe)
    encoded("base-win", b"<< /BaseEncoding /WinAnsiEncoding >>", s=probe)
    encoded("base-mac", b"<< /BaseEncoding /MacRomanEncoding >>", s=probe)
    encoded("base-standard", b"<< /BaseEncoding /StandardEncoding >>", s=probe)
    encoded("base-macexpert", b"<< /BaseEncoding /MacExpertEncoding >>", s=probe)
    encoded("base-pdfdoc", b"<< /BaseEncoding /PDFDocEncoding >>", s=probe)
    encoded("base-unknown", b"<< /BaseEncoding /FooEncoding >>", s=probe)
    encoded("base-string", b"<< /BaseEncoding (WinAnsiEncoding) >>", s=probe)
    encoded("base-ref", b"<< /BaseEncoding 20 0 R >>", {20: b"/WinAnsiEncoding"}, s=probe)
    encoded("base-chain", b"<< /BaseEncoding 20 0 R >>", {20: b"21 0 R", 21: b"/WinAnsiEncoding"}, s=probe)
    encoded("base-number", b"<< /BaseEncoding 7 >>", s=probe)
    encoded("symbol-win", b"/WinAnsiEncoding", s=b"(abc)", base=b"/Symbol")
    encoded("symbol-base", b"<< /BaseEncoding /WinAnsiEncoding >>", s=b"(abc)", base=b"/Symbol")
    encoded("zapf-win", b"/WinAnsiEncoding", s=b"(abc)", base=b"/ZapfDingbats")
    encoded("zapf-diff", b"<< /Differences [97 /a1 /a2] >>", s=b"(abc)", base=b"/ZapfDingbats")

    # MacRoman's codes where WinAnsi's differ: 0x7f has no glyph, 0xf0 the Apple logo.
    W256 = b"/FirstChar 0 /LastChar 255 /Widths [" + b"500 " * 256 + b"] "
    mac = b"(\\176\\177\\360A)"
    case("enc-mac-7f", b"<< " + HX + W256 + b"/Encoding /MacRomanEncoding >>", s=mac)
    case("base-mac-7f", b"<< " + HX + W256 + b"/Encoding << /BaseEncoding /MacRomanEncoding >> >>", s=mac)
    case("diff-mac-7f", b"<< " + HX + W256 + b"/Encoding << /BaseEncoding /MacRomanEncoding /Differences [127 /A] >> >>", s=mac)
    case("win-7f-kept", b"<< " + HX + W256 + b"/Encoding /WinAnsiEncoding >>", s=mac)
    case("enc-mac-none-widths", b"<< " + HX + b"/Encoding /MacRomanEncoding >>", s=b"(A\\177B\\360C)")

    # /BaseFont's Symbol by /Subtype and /Flags: no /Widths, so each glyph's own width.
    sym = b"/Type /Font /BaseFont /Symbol /Subtype "
    case("symbol-none", b"<< " + sym + b"/Type1 >>", s=b"(abc\\042)")
    case("symbol-tt-none", b"<< " + sym + b"/TrueType >>", s=b"(abc\\042)")
    case("symbol-tt-win", b"<< " + sym + b"/TrueType /Encoding /WinAnsiEncoding >>", s=b"(abc\\042)")
    case("symbol-tt-symbolic", b"<< " + sym + b"/TrueType /FontDescriptor 20 0 R >>", {20: b"<< /Flags 4 >>"}, s=b"(abc\\042)")
    greek = b"(ABC\\104\\127\\155)"
    symbol_w = b"/FirstChar 0 /LastChar 255 /Widths [" + b"500 " * 256 + b"] "
    for name, keys, flags in [
        ("symbol-tt-greek", b"", b"4"),
        ("symbol-tt-greek-win", b"/Encoding /WinAnsiEncoding ", b"4"),
        ("symbol-tt-greek-dict", b"/Encoding << /Differences [65 /B] >> ", b"4"),
        ("symbol-tt-greek-latin", b"", b"32"),
    ]:
        case(name, b"<< " + sym + b"/TrueType " + symbol_w + keys + b"/FontDescriptor 20 0 R >>",
             {20: b"<< /Flags " + flags + b" >>"}, s=greek)
    case("symbol-tt-greek-no-descriptor", b"<< " + sym + b"/TrueType " + symbol_w + b">>", s=greek)
    case("symbol-tt-greek-no-flags", b"<< " + sym + b"/TrueType " + symbol_w + b"/FontDescriptor 20 0 R >>", {20: b"<< /Flags 0 >>"}, s=greek)
    case("symbol-tt-greek-both", b"<< " + sym + b"/TrueType " + symbol_w + b"/FontDescriptor 20 0 R >>", {20: b"<< /Flags 36 >>"}, s=greek)
    case("symbol-type1-greek", b"<< " + sym + b"/Type1 " + symbol_w + b">>", s=greek)
    case("symbol-tagged-tt-greek", b"<< /Type /Font /BaseFont /ABCDEF+Symbol /Subtype /TrueType " + symbol_w + b"/FontDescriptor 20 0 R >>",
         {20: b"<< /Flags 4 >>"}, s=greek)
    case("symbol-flags-latin", b"<< " + sym + b"/Type1 /FontDescriptor 20 0 R >>", {20: b"<< /Flags 32 >>"}, s=b"(abc\\042)")
    case("symbol-flags-latin-win", b"<< " + sym + b"/Type1 /Encoding /WinAnsiEncoding /FontDescriptor 20 0 R >>", {20: b"<< /Flags 32 >>"}, s=b"(abc\\042)")
    case("zapf-flags-latin", b"<< /Type /Font /BaseFont /ZapfDingbats /Subtype /Type1 /FontDescriptor 20 0 R >>", {20: b"<< /Flags 32 >>"}, s=b"(abc)")
    case("helvetica-flags-symbolic", b"<< " + HX + b"/FontDescriptor 20 0 R >>", {20: b"<< /Flags 4 >>"}, s=b"(abc\\047)")
    case("helvetica-flags-symbolic-unknown", b"<< " + HX + b"/Encoding /Foo /FontDescriptor 20 0 R >>", {20: b"<< /Flags 4 >>"}, s=b"(abc\\047)")
    case("foo-tt-none", b"<< /Type /Font /Subtype /TrueType /BaseFont /Foo /FirstChar 39 /Widths [500] >>", s=b"(\\047A)")
    case("foo-t1-none", b"<< /Type /Font /Subtype /Type1 /BaseFont /Foo /FirstChar 39 /Widths [500] >>", s=b"(\\047A)")
    case("foo-t1-symbolic", b"<< /Type /Font /Subtype /Type1 /BaseFont /Foo /FirstChar 39 /Widths [500] /FontDescriptor 20 0 R >>", {20: b"<< /Flags 4 >>"}, s=b"(\\047A)")

    # /Differences, by GetArrayFor, each element by GetDirectObjectAt, a name or
    # else GetInteger.
    for name, value, extra in [
        ("diff-base", b"[65 /C]", None),
        ("diff-real", b"[65.9 /C]", None),
        ("diff-true", b"[true /C]", None),
        ("diff-false", b"[false /C]", None),
        ("diff-string", b"[66 (s) /C]", None),
        ("diff-null", b"[66 null /C]", None),
        ("diff-array", b"[66 [1] /C]", None),
        ("diff-dict", b"[66 << >> /C]", None),
        ("diff-neg", b"[-1 /C /D]", None),
        ("diff-neg-two", b"[-2 /C /D /E]", None),
        ("diff-past-255", b"[255 /C /D]", None),
        ("diff-neg-comes-round", b"[-1 /C /D 66 /E]", None),
        ("diff-past-32", b"[4294967361 /C]", None),
        ("diff-past-i32", b"[4294967295 /C /D]", None),
        ("diff-twice", b"[65 /C 65 /D]", None),
        ("diff-run", b"[65 /C /D /E]", None),
        ("diff-empty-name", b"[65 / /D]", None),
        ("diff-uni", b"[65 /uni0416]", None),
        ("diff-u", b"[65 /u1F600]", None),
        ("diff-lig", b"[65 /f_i]", None),
        ("diff-variant", b"[65 /a.alt]", None),
        ("diff-dingbat", b"[65 /a1]", None),
        ("diff-notdef", b"[65 /.notdef]", None),
        ("diff-unknown", b"[65 /g123]", None),
        ("diff-ref-code", b"[20 0 R /C]", {20: b"66"}),
        ("diff-ref-name", b"[65 20 0 R]", {20: b"/C"}),
        ("diff-chain-code", b"[20 0 R /C]", {20: b"21 0 R", 21: b"66"}),
        ("diff-chain-name", b"[66 20 0 R /D]", {20: b"21 0 R", 21: b"/C"}),
        ("diff-chain3-code", b"[20 0 R /C]", {20: b"21 0 R", 21: b"22 0 R", 22: b"66"}),
        ("diff-dangling", b"[66 99 0 R /C]", None),
        ("diff-ref", b"20 0 R", {20: b"[65 /C]"}),
        ("diff-whole-chain", b"20 0 R", {20: b"21 0 R", 21: b"[65 /C]"}),
        ("diff-whole-dict", b"<< >>", None),
    ]:
        encoded(name, b"<< /Differences " + value + b" >>", extra)
    encoded("diff-chain", b"<< /Differences [65 /C] /BaseEncoding /WinAnsiEncoding >>")
    encoded("diff-neg-seen", b"<< /Differences [-1 /C /D] >>", s=b"(\\000AB)")
    case("diff-notdef-truetype", b"<< /Type /Font /Subtype /TrueType /BaseFont /Helvetica /FirstChar 65 /LastChar 66 "
         b"/Widths [500 600] /Encoding << /Differences [65 /.notdef] >> >>")
    case("diff-notdef-symbolic", b"<< /Type /Font /Subtype /Type1 /BaseFont /Foo /FirstChar 65 /LastChar 66 /Widths [500 600] "
         b"/FontDescriptor 20 0 R /Encoding << /Differences [65 /.notdef] >> >>", {20: b"<< /Flags 4 >>"})
    encoded("diff-over-base", b"<< /BaseEncoding /WinAnsiEncoding /Differences [128 /A] >>", s=b"(\\200\\201A)")

    # /Subtype and /BaseFont, by GetByteStringFor: a string reads as a name does,
    # through one reference.
    w = b"/Widths [500] /FirstChar 65 /LastChar 65 "
    for name, value, extra in [
        ("sub-none", b"", None),
        ("sub-unknown", b"/Subtype /Foo ", None),
        ("sub-number", b"/Subtype 7 ", None),
        ("sub-string-truetype", b"/Subtype (TrueType) ", None),
        ("sub-ref-truetype", b"/Subtype 20 0 R ", {20: b"/TrueType"}),
        ("sub-chain-truetype", b"/Subtype 20 0 R ", {20: b"21 0 R", 21: b"/TrueType"}),
        ("sub-mm", b"/Subtype /MMType1 ", None),
        ("sub-cid", b"/Subtype /CIDFontType2 ", None),
    ]:
        # Courier by /Type1 starts at 600, and doesn't by /TrueType.
        case(name, b"<< /Type /Font /BaseFont /Courier " + value + w + b">>", extra)
    case("sub-string-type0", b"<< /Type /Font /Subtype (Type0) /BaseFont /Foo /Encoding /Identity-H /DescendantFonts [20 0 R] >>",
         {20: b"<< /Type /Font /Subtype /CIDFontType2 /W [65 [510 610]] >>"}, s=b"<004100420043>")
    case("sub-string-type3", b"<< /Type /Font /Subtype (Type3) /FontBBox [0 0 1 1] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /Widths [510 610] /Encoding << >> >>")
    for name, value, extra in [
        ("bf-name", b"/Courier", None),
        ("bf-string", b"(Courier)", None),
        ("bf-ref", b"20 0 R", {20: b"/Courier"}),
        ("bf-chain", b"20 0 R", {20: b"21 0 R", 21: b"/Courier"}),
        ("bf-dangling", b"99 0 R", None),
        ("bf-number", b"7", None),
        ("bf-subset", b"/ABCDEF+Courier", None),
        ("bf-alternate", b"/CourierNewPSMT", None),
        ("bf-case", b"/courier", None),
    ]:
        case(name, b"<< /Type /Font /Subtype /Type1 /BaseFont " + value + b" " + w + b">>", extra)

    # No /Widths: what's drawn by a subset's tag, by an alternate name, by its case.
    for name, value in [("none-subset", b"/ABCDEF+Helvetica"), ("none-alternate", b"/Arial"), ("none-case", b"/helvetica"),
                        ("none-comma", b"/Arial,Bold"), ("none-string", b"(Helvetica)")]:
        case(name, b"<< /Type /Font /Subtype /Type1 /BaseFont " + value + b" >>", s=b"(ABCi)")
        case(name + "-tt", b"<< /Type /Font /Subtype /TrueType /BaseFont " + value + b" >>", s=b"(ABCi)")

    # WinAnsiEncoding's unused codes draw the bullet, and each has its width.
    for code in [0x7f, 0x81, 0x8d, 0x8f, 0x90, 0x9d]:
        for font, tag in [(b"Helvetica", b"h"), (b"Times-Roman", b"t")]:
            case("win-%02x-%s" % (code, tag.decode()),
                 b"<< /Type /Font /Subtype /Type1 /BaseFont /" + font + b" /Encoding /WinAnsiEncoding >>",
                 s=b"<%02x41>" % code)
    case("win-a0-widths", b"<< " + HX + b"/Encoding /WinAnsiEncoding >>", s=b"<a041>")
    case("win-ad-widths", b"<< " + HX + b"/Encoding /WinAnsiEncoding >>", s=b"<ad41>")
    case("win-none-widths", b"<< " + HX + b"/Encoding /WinAnsiEncoding >>", s=b"<8041>")
    case("std-none-widths", b"<< " + HX + b">>", s=b"<0141>")

    # /ToUnicode, by GetStreamFor: a reference to a stream, no further.
    one = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap 1 begincodespacerange <00> <ff> endcodespacerange "
    two = b"endcmap end end"
    map_a = stream(b"", one + b"1 beginbfchar <41> <0416> endbfchar " + two)
    tu = b"<< " + H + b"/Widths [500 600] /ToUnicode "
    case("tu-ref", tu + b"20 0 R >>", {20: map_a})
    case("tu-chain", tu + b"20 0 R >>", {20: b"21 0 R", 21: map_a})
    case("tu-dangling", tu + b"99 0 R >>")
    case("tu-name", tu + b"/Identity-H >>")
    case("tu-dict", tu + b"<< >> >>")
    case("tu-lig", tu + b"20 0 R >>", {20: stream(b"", one + b"1 beginbfchar <41> <00660069> endbfchar " + two)})
    case("tu-range", tu + b"20 0 R >>", {20: stream(b"", one + b"1 beginbfrange <41> <42> <0416> endbfrange " + two)})
    case("tu-over-diff", b"<< " + H + b"/Widths [500 600] /Encoding << /Differences [65 /C] >> /ToUnicode 20 0 R >>", {20: map_a})


D = b"<< /Type /Font /Subtype /TrueType /BaseFont /Foo /FirstChar 32 /LastChar 65 /Widths [" + b"500 " * 34 + b"] /FontDescriptor 20 0 R >>"


def heights():
    # A descriptor's numbers, by GetIntegerFor and GetIntegerAt. Each is asked of
    # the space, whose box is none, so that the font's heights show.
    def d(name, keys, extra=None):
        objects = {20: b"<< /Type /FontDescriptor /FontName /Foo /Flags 32 " + keys + b" >>"}
        objects.update(extra or {})
        case(name, D, objects, s=b"( A)", kind="fonts-heights")
    box = b"/FontBBox [0 -300 1000 900] "
    d("h-base", box + b"/Ascent 800 /Descent -200")
    d("h-real", box + b"/Ascent 800.9 /Descent -200.9")
    d("h-true", box + b"/Ascent true /Descent true")
    d("h-ascent-true", box + b"/Ascent true /Descent -200")
    d("h-descent-true", box + b"/Ascent 800 /Descent true")
    d("h-ascent-string", box + b"/Ascent (x) /Descent -200")
    d("h-descent-string", box + b"/Ascent 800 /Descent (x)")
    d("h-ascent-ref", box + b"/Ascent 21 0 R /Descent -200", {21: b"800"})
    d("h-ascent-chain", box + b"/Ascent 21 0 R /Descent -200", {21: b"22 0 R", 22: b"800"})
    d("h-ascent-dangling", box + b"/Ascent 99 0 R /Descent -200")
    d("h-descent-ref", box + b"/Ascent 800 /Descent 21 0 R", {21: b"-200"})
    d("h-descent-chain", box + b"/Ascent 800 /Descent 21 0 R", {21: b"22 0 R", 22: b"-200"})
    d("h-desc-pos", box + b"/Ascent 800 /Descent 200")
    d("h-desc-11", box + b"/Ascent 800 /Descent 11")
    d("h-desc-10", box + b"/Ascent 800 /Descent 10")
    d("h-desc-9", box + b"/Ascent 800 /Descent 9")
    d("h-desc-neg", box + b"/Ascent 800 /Descent -10")
    d("h-ascent-neg", box + b"/Ascent -100 /Descent -200")
    d("h-ascent-only", box + b"/Ascent 800")
    d("h-descent-only", box + b"/Descent -200")
    d("h-past-32", box + b"/Ascent 4294967296 /Descent -200")
    d("h-past-i32", box + b"/Ascent 800 /Descent 2147483648")
    d("h-clamp", b"/FontBBox [0 -100 1000 500] /Ascent 800 /Descent -200")
    d("h-clamp-top", b"/FontBBox [0 -300 1000 500] /Ascent 800 /Descent -200")
    d("h-clamp-bottom", b"/FontBBox [0 -100 1000 900] /Ascent 800 /Descent -200")
    d("h-box-flipped", b"/FontBBox [0 900 1000 -300] /Ascent 1000 /Descent -500")
    d("h-box-flat", b"/FontBBox [5 100 6 100] /Ascent 800 /Descent -200")
    d("h-box-short", b"/FontBBox [0 -300 1000] /Ascent 800 /Descent -200")
    d("h-box-long", b"/FontBBox [0 -300 1000 900 5 5] /Ascent 800 /Descent -200")
    d("h-box-real", b"/FontBBox [0 -300.9 1000 500.9] /Ascent 800 /Descent -400")
    d("h-box-true", b"/FontBBox [0 -300 1000 true] /Ascent 800 /Descent -200")
    d("h-box-string", b"/FontBBox [0 (x) 1000 500] /Ascent 800 /Descent -200")
    d("h-box-ref", b"/FontBBox 21 0 R /Ascent 800 /Descent -200", {21: b"[0 -100 1000 500]"})
    d("h-box-elem-ref", b"/FontBBox [0 -100 1000 21 0 R] /Ascent 800 /Descent -200", {21: b"500"})
    d("h-box-elem-chain", b"/FontBBox [0 -100 1000 21 0 R] /Ascent 800 /Descent -200", {21: b"22 0 R", 22: b"500"})
    d("h-box-elem-dangling", b"/FontBBox [0 -100 1000 99 0 R] /Ascent 800 /Descent -200")
    # What the box's absence leaves to the face isn't asked: it's the system's.


def type0():
    Z = b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-H /DescendantFonts [20 0 R] >>"
    info = b"/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> "
    s = b"<004100420043>"

    def c(name, keys, extra=None, s=s, font=Z):
        objects = {20: b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo " + info + keys + b" >>"}
        objects.update(extra or {})
        case(name, font, objects, s)
    # /W, by GetArrayFor; its elements by GetDirectObjectAt, then an array or
    # GetInteger; an array's by GetIntegerAt.
    for name, value, extra in [
        ("w-base", b"[65 [500 600]]", None),
        ("w-real", b"[65 [500.9 600.9]]", None),
        ("w-range", b"[65 66 600]", None),
        ("w-range-real", b"[65 66 600.9]", None),
        ("w-true", b"[65 [true 600]]", None),
        ("w-first-real", b"[65.9 [500 600]]", None),
        ("w-range-true", b"[65 66 true]", None),
        ("w-range-last-true", b"[65 true 700]", None),
        ("w-range-string", b"[65 66 (x)]", None),
        ("w-str", b"[65 [(x) 600]]", None),
        ("w-past-32", b"[65 [4294967296 600]]", None),
        ("w-past-i32", b"[65 [2147483648 600]]", None),
        ("w-16-bits", b"[65 [70000 600]]", None),
        ("w-neg", b"[65 [-500 600]]", None),
        ("w-overlap-first", b"[65 66 500 65 66 600]", None),
        ("w-overlap-array", b"[65 [510] 65 66 600]", None),
        ("w-neg-first", b"[-5 66 650]", None),
        ("w-neg-last", b"[-5 -1 650 65 [500]]", None),
        ("w-array-at-last", b"[65 66 [500] 65 66 700]", None),
        ("w-array-first", b"[[500] 65 66 700]", None),
        ("w-array-at-value", b"[65 66 500 [600]]", None),
        ("w-short-range", b"[65 66]", None),
        ("w-backward", b"[66 65 500]", None),
        ("w-int-most", b"[2147483647 [100 200] 65 [500]]", None),
        ("w-inner-ref", b"[65 [21 0 R 600]]", {21: b"500"}),
        ("w-inner-chain", b"[65 [21 0 R 600]]", {21: b"22 0 R", 22: b"500"}),
        ("w-inner-dangling", b"[65 [99 0 R 600]]", None),
        ("w-outer-ref", b"[21 0 R [500 600]]", {21: b"65"}),
        ("w-outer-chain", b"[21 0 R [500 600]]", {21: b"22 0 R", 22: b"65"}),
        ("w-outer-chain3", b"[21 0 R [500 600]]", {21: b"22 0 R", 22: b"23 0 R", 23: b"65"}),
        ("w-outer-dangling", b"[99 0 R 65 [500 600]]", None),
        ("w-array-ref", b"[65 21 0 R]", {21: b"[500 600]"}),
        ("w-array-chain", b"[65 21 0 R]", {21: b"22 0 R", 22: b"[500 600]"}),
        ("w-ref", b"21 0 R", {21: b"[65 [810 820]]"}),
        ("w-chain", b"21 0 R", {21: b"22 0 R", 22: b"[65 [810 820]]"}),
        ("w-dict", b"<< >>", None),
    ]:
        c(name, b"/W " + value, extra)
    low = b"<000000010043>"
    c("w-first-true", b"/W [true [100 200]]", s=b"<000100020043>")
    c("w-first-string", b"/W [(x) [300 400]]", s=low)
    c("w-neg-array", b"/W [-2 [100 200 300 400]]", s=low)
    c("w-elem-dict", b"/W [21 0 R [500 600]]", {21: b"<< >>"}, s=low)
    # /DW, by GetIntegerFor into 16 bits.
    for name, value, extra in [
        ("dw", b"700", None),
        ("dw-real", b"700.9", None),
        ("dw-true", b"true", None),
        ("dw-string", b"(x)", None),
        ("dw-null", b"null", None),
        ("dw-ref", b"21 0 R", {21: b"700"}),
        ("dw-chain", b"21 0 R", {21: b"22 0 R", 22: b"700"}),
        ("dw-dangling", b"99 0 R", None),
        ("dw-neg", b"-700", None),
        ("dw-16-bits", b"70000", None),
        ("dw-32768", b"32768", None),
        ("dw-past-32", b"4294967996", None),
    ]:
        c(name, b"/DW " + value, extra)
    c("dw-none", b"")

    # /DescendantFonts, by GetArrayFor, one of it by GetDictAt. A font that doesn't
    # load draws nothing.
    cid = b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo /W [65 [510 610]] >>"

    def df(name, value, extra=None):
        font = b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-H /DescendantFonts " + value + b" >>"
        case(name, font, extra, s)
    df("df-ref", b"21 0 R", {21: b"[20 0 R]", 20: cid})
    df("df-chain", b"21 0 R", {21: b"22 0 R", 22: b"[20 0 R]", 20: cid})
    df("df-direct", b"[" + cid + b"]")
    df("df-elem-chain", b"[21 0 R]", {21: b"20 0 R", 20: cid})
    df("df-elem-stream", b"[20 0 R]", {20: stream(b"/Subtype /CIDFontType2 /W [65 [510 610]]", b"")})
    df("df-elem-dangling", b"[99 0 R]")
    df("df-elem-number", b"[7]")
    df("df-two", b"[20 0 R 20 0 R]", {20: cid})
    df("df-empty", b"[]")
    df("df-name", b"/x")
    case("df-none", b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-H >>", None, s)

    # /Encoding, by GetDirectObjectFor: a name or a stream, or the font doesn't load.
    cmap = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap "
            b"1 begincodespacerange <00> <ff> endcodespacerange "
            b"1 begincidrange <41> <43> 65 endcidrange endcmap end end")

    def e(name, value, extra=None, s=s):
        objects = {20: cid}
        objects.update(extra or {})
        case(name, b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /DescendantFonts [20 0 R] /Encoding " + value + b" >>", objects, s)
    e("e0-ref", b"21 0 R", {21: b"/Identity-H"})
    e("e0-chain", b"21 0 R", {21: b"22 0 R", 22: b"/Identity-H"})
    e("e0-dangling", b"99 0 R")
    e("e0-number", b"7")
    e("e0-string", b"(Identity-H)")
    e("e0-dict", b"<< >>")
    e("e0-stream", b"21 0 R", {21: stream(b"", cmap)}, b"(ABC)")
    e("e0-stream-chain", b"21 0 R", {21: b"22 0 R", 22: stream(b"", cmap)}, b"(ABC)")
    e("e0-unknown", b"/Foo-H")
    e("e0-cid-16-bits", b"21 0 R", {21: stream(b"", b"begincmap 1 begincodespacerange <00> <ff> endcodespacerange "
                                                  b"1 begincidrange <41> <43> 65601 endcidrange endcmap")}, b"(ABC)")
    case("e0-none", b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /DescendantFonts [20 0 R] >>", {20: cid}, s)
    e("codes-short", b"/Identity-H", s=b"<0041004200>")

    # An embedded CMap's words: what it's read by, and where that isn't PDFium's.
    def cmap_case(name, body, s=b"(ABC)"):
        e(name, b"21 0 R", {21: stream(b"", b"begincmap " + body + b" endcmap")}, s)
    space_one = b"1 begincodespacerange <00> <ff> endcodespacerange "
    cmap_case("cmap-cidchar", space_one + b"2 begincidchar <41> 66 <42> 65 endcidchar")
    cmap_case("cmap-wmode", b"/WMode 1 def " + space_one + b"1 begincidrange <41> <43> 65 endcidrange")
    cmap_case("cmap-mixed", b"2 begincodespacerange <00> <7f> <8000> <ffff> endcodespacerange "
                            b"2 begincidrange <41> <43> 65 <8041> <8043> 66 endcidrange", b"<41804143>")
    cmap_case("cmap-two", b"1 begincodespacerange <0000> <ffff> endcodespacerange "
                          b"1 begincidrange <0041> <0043> 65 endcidrange", b"<004100420043>")
    cmap_case("cmap-none-mapped", space_one, b"(ABC)")
    cmap_case("cmap-literal", space_one + b"1 begincidrange (A) (C) 65 endcidrange")
    cmap_case("cmap-hex-odd", space_one + b"1 begincidrange <4> <4> 65 endcidrange", b"(@AB)")
    cmap_case("cmap-usecmap", b"/Identity-H usecmap")
    cmap_case("cmap-junk-word", space_one + b"1 begincidrange <41> <43> 65 endcidrange ) <41> <41> 66 >> 1 begincidchar <42> 65 endcidchar")
    tu_map = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 65 /LastChar 66 /Widths [500 600] /ToUnicode 20 0 R >>"
    def tu_case(name, body):
        case(name, tu_map, {20: stream(b"", b"begincmap " + body + b" endcmap")})
    tu_case("tu-literal-code", space_one + b"1 beginbfchar (A) <0416> endbfchar")
    tu_case("tu-name-dest", space_one + b"1 beginbfchar <41> /Zhe endbfchar")
    tu_case("tu-one-byte-dest", space_one + b"1 beginbfchar <41> <5a> endbfchar")
    tu_case("tu-array-range", space_one + b"1 beginbfrange <41> <42> [<0416> <0417>] endbfrange")
    # Mappings over mappings: a code's last, by a map of codes.
    tu_case("tu-overlap", space_one + b"1 beginbfrange <00> <ff> <0416> endbfrange 1 beginbfrange <41> <41> <0420> endbfrange")
    tu_case("tu-overlap-first-wider", space_one + b"1 beginbfrange <41> <43> <0416> endbfrange 1 beginbfrange <42> <42> <0420> endbfrange")
    tu_case("tu-char-after-range", space_one + b"1 beginbfrange <41> <43> <0416> endbfrange 1 beginbfchar <42> <0420> endbfchar")
    tu_case("tu-range-after-char", space_one + b"1 beginbfchar <42> <0420> endbfchar 1 beginbfrange <41> <43> <0416> endbfrange")
    tu_case("tu-char-twice", space_one + b"1 beginbfchar <41> <0416> endbfchar 1 beginbfchar <41> <0420> endbfchar")
    # A block's count, what's before its begin; a block whose count is otherwise is lost whole.
    tu_case("tu-count-more", space_one + b"2 beginbfchar <41> <0416> endbfchar")
    tu_case("tu-count-less", space_one + b"1 beginbfchar <41> <0416> <42> <0417> endbfchar")
    tu_case("tu-count-zero", space_one + b"0 beginbfchar <41> <0416> endbfchar")
    tu_case("tu-count-word", space_one + b"beginbfchar <41> <0416> endbfchar")
    tu_case("tu-count-range-more", space_one + b"99 beginbfrange <41> <41> <0416> endbfrange")
    tu_case("tu-count-range-less", space_one + b"1 beginbfrange <41> <41> <0416> <42> <42> <0417> endbfrange")
    tu_case("tu-count-kept-apart", space_one + b"9 beginbfchar <41> <0416> endbfchar 1 beginbfchar <42> <0417> endbfchar")
    tu_case("tu-word-in-block", space_one + b"1 beginbfchar <41> <0416> x endbfchar")
    tu_case("tu-word-before-mapping", space_one + b"1 beginbfchar <41> <0416> x <42> <0417> endbfchar")
    tu_case("tu-range-code-empty", space_one + b"2 beginbfrange <> <41> <0417> <41> <41> <0416> endbfrange")
    tu_case("tu-end-unbegun", space_one + b"<41> <0416> endbfchar")
    # Codes: 16 bits at most, a range's last by its low byte.
    tu_case("tu-code-past-16", space_one + b"1 beginbfchar <010041> <0416> endbfchar")
    tu_case("tu-code-past-16-lost", space_one + b"2 beginbfchar <41> <0416> <010042> <0417> endbfchar")
    tu_case("tu-code-long", space_one + b"1 beginbfchar <0000000041> <0416> endbfchar")
    tu_case("tu-code-empty", space_one + b"2 beginbfchar <> <0416> <41> <0417> endbfchar")
    tu_case("tu-past-low-byte", space_one + b"1 beginbfrange <0041> <0142> <0416> endbfrange")
    tu_case("tu-past-low-byte-array", space_one + b"1 beginbfrange <0041> <0142> [<0416> <0417>] endbfrange")
    tu_case("tu-range-low-byte-down", space_one + b"1 beginbfrange <0141> <0042> <0416> endbfrange")
    tu_case("tu-range-low-byte-past-16", space_one + b"1 beginbfrange <0041> <010042> <0416> endbfrange")
    tu_case("tu-range-down", space_one + b"2 beginbfrange <41> <40> <0416> <42> <42> <0417> endbfrange")
    tu_case("tu-range-past-16", space_one + b"1 beginbfrange <010041> <010042> <0416> endbfrange")
    # Destinations: what's no text, and 16-bit units.
    tu_case("tu-ffff", space_one + b"1 beginbfchar <41> <ffff> endbfchar")
    tu_case("tu-ffff-range", space_one + b"1 beginbfrange <41> <43> <ffff> endbfrange")
    tu_case("tu-ffff-over", space_one + b"1 beginbfchar <41> <0416> endbfchar 1 beginbfchar <41> <ffff> endbfchar")
    tu_case("tu-ffff-past", space_one + b"1 beginbfrange <41> <43> <fffe> endbfrange")
    tu_case("tu-ffff-first-many", space_one + b"2 beginbfchar <41> <ffff> <42> <00660069> endbfchar")
    tu_case("tu-empty", space_one + b"1 beginbfchar <41> <> endbfchar")
    tu_case("tu-empty-kept", space_one + b"1 beginbfchar <41> <0416> endbfchar 1 beginbfchar <41> <> endbfchar")
    tu_case("tu-empty-range-first", space_one + b"1 beginbfchar <41> <0416> endbfchar 1 beginbfrange <41> <41> <> endbfrange")
    tu_case("tu-empty-range", space_one + b"1 beginbfrange <41> <42> <> endbfrange")
    tu_case("tu-name-range", space_one + b"1 beginbfrange <41> <42> /B endbfrange")
    tu_case("tu-name-dest-listed", space_one + b"1 beginbfchar <41> /B endbfchar")
    tu_case("tu-lone-surrogate", space_one + b"1 beginbfchar <41> <dd00> endbfchar")
    tu_case("tu-lone-surrogate-range", space_one + b"1 beginbfrange <41> <43> <dd00> endbfrange")
    tu_case("tu-pair", space_one + b"1 beginbfchar <41> <d83dde00> endbfchar")
    tu_case("tu-many-range", space_one + b"1 beginbfrange <41> <43> <00660069> endbfrange")
    tu_case("tu-pair-range", space_one + b"1 beginbfrange <41> <43> <d83dde00> endbfrange")
    tu_case("tu-odd-byte-dest", space_one + b"1 beginbfchar <41> <004100> endbfchar")
    tu_case("tu-one-byte-range", space_one + b"1 beginbfrange <41> <42> <5a> endbfrange")
    tu_case("tu-array-short", space_one + b"1 beginbfrange <41> <43> [<0416>] endbfrange")
    tu_case("tu-array-long", space_one + b"1 beginbfrange <41> <41> [<0416> <0417>] endbfrange")
    tu_case("tu-array-empty", space_one + b"1 beginbfrange <41> <42> [<> <0417>] endbfrange")
    tu_case("tu-array-name", space_one + b"1 beginbfrange <41> <42> [/B <0417>] endbfrange")
    tu_case("tu-no-end", space_one + b"1 beginbfchar <41> <0416> ")
    # A collection's UCS2 CMap named: what none maps is a CID of it, its text Adobe's.
    tu_case("predefined-tu-base-usecmap", space_one + b"/Adobe-GB1-UCS2 usecmap 1 beginbfchar <41> <0416> endbfchar")
    tu_case("predefined-tu-base-named", b"/CMapName /Adobe-Japan1-UCS2 def " + space_one + b"1 beginbfchar <41> <0416> endbfchar")
    # The file's reader: a stream's dictionary held in another, what's no name read as a key.
    case("tu-stream-dict-in-dict", tu_map, {20: b"<< << /Length 0 >>\nstream\n" + space_one + b"1 beginbfchar <41> <0416> endbfchar\nendstream"})
    # A Type 0 font's code its /ToUnicode has none of: its CID's text, or the code's.
    miss = stream(b"", b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfchar <0041> <0416> endbfchar endcmap")
    def t0_miss(name, encoding, ordering, s, extra=None):
        objects = {
            20: b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo /DW 500 /CIDSystemInfo << /Registry (Adobe) /Ordering (" + ordering + b") /Supplement 0 >> >>",
            21: miss,
        }
        objects.update(extra or {})
        case(name, b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding " + encoding + b" /DescendantFonts [20 0 R] /ToUnicode 21 0 R >>", objects, s)
    t0_miss("t0-miss-identity", b"/Identity-H", b"Identity", b"<004100420043>")
    t0_miss("predefined-t0-miss-gb1", b"/Identity-H", b"GB1", b"<004100420043>")
    t0_miss("predefined-t0-miss-japan1", b"/Identity-H", b"Japan1", b"<004100420043>")
    t0_miss("predefined-t0-miss-ucs2", b"/UniGB-UCS2-H", b"GB1", b"<004100420416>")
    t0_miss("predefined-t0-miss-gbk", b"/GBK-EUC-H", b"GB1", b"(\\000AB)")
    t0_miss("t0-miss-embedded", b"22 0 R", b"GB1", b"<004100420043>", {22: stream(b"", b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange 1 begincidrange <0000> <ffff> 0 endcidrange endcmap")})
    t0_miss("t0-miss-unmapped-gb1-cid0", b"/Identity-H", b"Identity", b"<00000041>")
    # A range past its low byte: read whole here, by the code's own text there.
    whole = stream(b"", b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfrange <0000> <ffff> <0000> endbfrange endcmap")
    moved = stream(b"", b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfrange <00ff> <01ff> <0416> endbfrange endcmap")
    crossing = stream(b"", b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange 2 beginbfrange <0041> <0041> <0420> <00ff> <0100> [<0416> <0417>] endbfrange endcmap")
    for name, to in [("t0-past-low-byte-identity", whole), ("t0-past-low-byte", moved), ("t0-past-low-byte-array", crossing)]:
        t0_miss(name, b"/Identity-H", b"Identity", b"<004101000416>", {21: to})
    # A CID of the predefined CMaps: the bundle's, Adobe's current ones.
    case("predefined-cid-uni-gb-a0", b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /UniGB-UTF16-H /DescendantFonts [20 0 R] >>",
         {20: b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo /DW 500 /W [1 [700]] /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> >>"},
         s=b"<00a000410042>")

    # /Tw after a code 32 sized a byte.
    tw = b"100 Tw "
    case("ws-simple", b"<< " + H + b"/Widths [500 600] >>", s=b"(A B)", before=tw)
    case("ws-identity", Z, {20: cid}, s=b"<002000410042>", before=tw)
    one_byte = stream(b"", b"begincmap 1 begincodespacerange <00> <ff> endcodespacerange "
                           b"1 begincidrange <00> <ff> 0 endcidrange endcmap")
    two_byte = stream(b"", b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange "
                           b"1 begincidrange <0000> <ffff> 0 endcidrange endcmap")
    e("ws-one-byte", b"21 0 R", {21: one_byte}, b"( AB)")
    case("ws-cmap-one", b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /DescendantFonts [20 0 R] /Encoding 21 0 R >>",
         {20: cid, 21: one_byte}, s=b"( AB)", before=tw)
    case("ws-cmap-two", b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /DescendantFonts [20 0 R] /Encoding 21 0 R >>",
         {20: cid, 21: two_byte}, s=b"<002000410042>", before=tw)

    # Vertical writing: /W2's and /DW2's numbers, each in 16 bits.
    V = b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-V /DescendantFonts [20 0 R] >>"

    def v(name, keys, extra=None):
        c(name, keys, extra, font=V)
    v("v-default", b"/W [65 [500 600]]")
    v("v-dw-odd", b"/DW 701")
    v("v-w-odd", b"/W [65 [501 601]]")
    v("v-w-16-bits", b"/W [65 [70000 600]]")
    v("v-w-neg", b"/W [65 [-501 600]]")
    v("v-dw2", b"/DW2 [700 -900]")
    v("v-dw2-one", b"/DW2 [700]")
    v("v-dw2-none", b"/DW2 []")
    v("v-dw2-true", b"/DW2 [true -900]")
    v("v-dw2-real", b"/DW2 [700.9 -900.9]")
    v("v-dw2-string", b"/DW2 [(x) -900]")
    v("v-dw2-ref", b"/DW2 21 0 R", {21: b"[700 -900]"})
    v("v-dw2-chain", b"/DW2 21 0 R", {21: b"22 0 R", 22: b"[700 -900]"})
    v("v-dw2-elem-ref", b"/DW2 [21 0 R -900]", {21: b"700"})
    v("v-dw2-elem-chain", b"/DW2 [21 0 R -900]", {21: b"22 0 R", 22: b"700"})
    v("v-dw2-16-bits", b"/DW2 [70000 -70000]")
    v("v-w2", b"/W2 [65 [-800 250 700 -900 300 750]]")
    v("v-w2-range", b"/W2 [65 66 -800 250 700]")
    v("v-w2-short", b"/W2 [65 [-800 250]]")
    v("v-w2-range-short", b"/W2 [65 66 -800 250]")
    v("v-w2-true", b"/W2 [65 [true true true]]")
    v("v-w2-16-bits", b"/W2 [65 [40000 70000 -40000]]")
    v("v-w2-ref", b"/W2 21 0 R", {21: b"[65 [-800 250 700]]"})
    v("v-w2-chain", b"/W2 21 0 R", {21: b"22 0 R", 22: b"[65 [-800 250 700]]"})
    v("v-w2-at-last", b"/W2 [65 66 [1 2 3] 65 66 -800 250 700]")

    # /CIDSystemInfo's /Ordering, for a collection's text; GetDictFor, then
    # GetByteStringFor.
    a = b"<1d0e>"
    for name, keys, extra in [
        ("predefined-order", b"/CIDSystemInfo << /Ordering (Japan1) >> ", None),
        ("predefined-order-name", b"/CIDSystemInfo << /Ordering /Japan1 >> ", None),
        ("predefined-order-ref", b"/CIDSystemInfo << /Ordering 21 0 R >> ", {21: b"(Japan1)"}),
        ("predefined-order-chain", b"/CIDSystemInfo << /Ordering 21 0 R >> ", {21: b"22 0 R", 22: b"(Japan1)"}),
        ("predefined-info-ref", b"/CIDSystemInfo 21 0 R ", {21: b"<< /Ordering (Japan1) >>"}),
        ("predefined-info-chain", b"/CIDSystemInfo 21 0 R ", {21: b"22 0 R", 22: b"<< /Ordering (Japan1) >>"}),
        ("predefined-info-chain3", b"/CIDSystemInfo 21 0 R ", {21: b"22 0 R", 22: b"23 0 R", 23: b"<< /Ordering (Japan1) >>"}),
        ("predefined-info-stream", b"/CIDSystemInfo 21 0 R ", {21: stream(b"/Ordering (Japan1)", b"")}),
    ]:
        objects = {20: b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo " + keys + b">>"}
        objects.update(extra or {})
        case(name, Z, objects, s=b"<0022" + a[1:])
    for name in [b"UniJIS-UTF16-H", b"GBK-EUC-H", b"90ms-RKSJ-H", b"UniGB-UCS2-H", b"KSCms-UHC-H", b"ETen-B5-H", b"H", b"GB-EUC-H"]:
        lower = name.decode().lower()
        font = b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /DescendantFonts [20 0 R] /Encoding /" + name + b" >>"
        case("predefined-" + lower, font, {20: cid}, s=b"<00410042>")
        case("predefined-low-" + lower, font, {20: cid}, s=b"<000100021f204142>")

    # A TrueType font PDFium reads as GBK, by the first four bytes of its name.
    gb = b"/#CB#CE#CC#E5"
    gb_font = b"<< /Type /Font /Subtype /TrueType /BaseFont " + gb + b" /FirstChar 65 /Widths [510 610] "
    case("predefined-gb-none", gb_font + b">>", s=b"(AB\\260\\241C)")
    case("predefined-gb-descriptor", gb_font + b"/FontDescriptor 20 0 R >>", {20: b"<< /Flags 32 >>"}, s=b"(AB\\260\\241C)")
    case("gb-program", gb_font + b"/FontDescriptor 20 0 R >>", {20: b"<< /Flags 32 /FontFile2 99 0 R >>"}, s=b"(ABC)")
    case("gb-type1", b"<< /Type /Font /Subtype /Type1 /BaseFont " + gb + b" /FirstChar 65 /Widths [510 610] >>", s=b"(ABC)")


def type3():
    head = b"/Type /Font /Subtype /Type3 "
    matrix = b"/FontMatrix [0.001 0 0 0.001 0 0] "
    bbox = b"/FontBBox [0 -200 1000 800] "
    enc = b"/Encoding << /Differences [65 /A /B /C] >> "
    procs = b"/CharProcs << /A 20 0 R /B 21 0 R /C 22 0 R >> "
    w = b"/FirstChar 65 /LastChar 67 /Widths [500 600 700] "
    bodies = {20: stream(b"", b"410 0 d0"), 21: stream(b"", b"420 0 0 0 100 100 d1"), 22: stream(b"", b"430 0 d0")}

    def t(name, keys, extra=None, s=b"(ABC)", kind="fonts"):
        objects = dict(bodies)
        objects.update(extra or {})
        case(name, b"<< " + head + keys + b">>", objects, s, kind)

    def no_widths(name, a_proc=None, extra=None):
        objects = {20: stream(b"", a_proc)} if a_proc is not None else {}
        objects.update(extra or {})
        t(name, matrix + bbox + enc + procs, objects)

    t("t3-base", matrix + bbox + enc + procs + w)
    no_widths("t3-no-widths")
    # /Widths, by GetFloatAt, through the matrix's a and rounded.
    for name, value, extra in [
        ("t3-w-real", b"[500.6 600.4 700]", None),
        ("t3-w-half", b"[500.5 600.5 701.5]", None),
        ("t3-w-true", b"[true 600 700]", None),
        ("t3-w-string", b"[(x) 600 700]", None),
        ("t3-w-null", b"[null 600 700]", None),
        ("t3-w-zero", b"[0 600 700]", None),
        ("t3-w-neg", b"[-500 600 700]", None),
        ("t3-w-ref", b"[23 0 R 600 700]", {23: b"500"}),
        ("t3-w-chain", b"[23 0 R 600 700]", {23: b"24 0 R", 24: b"500"}),
        ("t3-w-dangling", b"[99 0 R 600 700]", None),
        ("t3-w-short", b"[500]", None),
        ("t3-w-past-32", b"[4294967296 600 700]", None),
        ("t3-w-past-i32", b"[2147483648 600 700]", None),
        ("t3-w-many-digits", b"[500.49999999999 600 700]", None),
        ("t3-widths-ref", b"23 0 R", {23: b"[500 600 700]"}),
        ("t3-widths-chain", b"23 0 R", {23: b"24 0 R", 24: b"[500 600 700]"}),
        ("t3-widths-dict", b"<< >>", None),
    ]:
        t(name, matrix + bbox + enc + procs + b"/FirstChar 65 /Widths " + value + b" ", extra)
    # /FirstChar, by GetIntegerFor, in an int.
    for name, value, extra in [
        ("t3-first-real", b"65.9", None),
        ("t3-first-true", b"true", None),
        ("t3-first-string", b"(x)", None),
        ("t3-first-neg", b"-1", None),
        ("t3-first-300", b"300", None),
        ("t3-first-255", b"255", None),
        ("t3-first-past-32", b"4294967361", None),
        ("t3-first-ref", b"23 0 R", {23: b"65"}),
        ("t3-first-chain", b"23 0 R", {23: b"24 0 R", 24: b"65"}),
    ]:
        t(name, matrix + bbox + enc + procs + b"/FirstChar " + value + b" /Widths [500 600 700] ", extra)
    t("t3-first-none", matrix + bbox + enc + procs + b"/Widths [500 600 700] ")
    # /FontMatrix, by GetArrayFor and GetMatrix: six, each by GetFloatAt.
    for name, value, extra in [
        ("t3-matrix-scale", b"[0.002 0 0 0.001 0 0]", None),
        ("t3-matrix-rotate", b"[0 0.001 -0.001 0 0 0]", None),
        ("t3-matrix-skew", b"[0.0006 0.0008 0 0.001 0 0]", None),
        ("t3-matrix-move", b"[0.001 0 0 0.001 5 5]", None),
        ("t3-matrix-flip", b"[-0.001 0 0 -0.001 0 0]", None),
        ("t3-matrix-ref", b"23 0 R", {23: b"[0.002 0 0 0.002 0 0]"}),
        ("t3-matrix-chain", b"23 0 R", {23: b"24 0 R", 24: b"[0.002 0 0 0.002 0 0]"}),
        ("t3-matrix-short", b"[0.002 0 0 0.002 0]", None),
        ("t3-matrix-long", b"[0.002 0 0 0.002 0 0 0]", None),
        ("t3-matrix-true", b"[true 0 0 0.002 0 0]", None),
        ("t3-matrix-string", b"[(x) 0 0 0.002 0 0]", None),
        ("t3-matrix-elem-ref", b"[23 0 R 0 0 0.002 0 0]", {23: b"0.002"}),
        ("t3-matrix-elem-chain", b"[23 0 R 0 0 0.002 0 0]", {23: b"24 0 R", 24: b"0.002"}),
        ("t3-matrix-int", b"[1 0 0 1 0 0]", None),
    ]:
        t(name, b"/FontMatrix " + value + b" " + bbox + enc + procs + w, extra)
        t(name + "-procs", b"/FontMatrix " + value + b" " + bbox + enc + procs, extra)
    t("t3-matrix-none", bbox + enc + procs + w)
    # /FontBBox, by GetFloatAt, scaled, cut.
    for name, value, extra in [
        ("t3-h-box", b"[0 -200 1000 800]", None),
        ("t3-h-box-real", b"[0 -200.9 1000 800.9]", None),
        ("t3-h-box-short", b"[0 -200 1000]", None),
        ("t3-h-box-long", b"[0 -200 1000 800 1 1]", None),
        ("t3-h-box-true", b"[0 -200 1000 true]", None),
        ("t3-h-box-ref", b"23 0 R", {23: b"[0 -200 1000 800]"}),
        ("t3-h-box-elem-ref", b"[0 -200 1000 23 0 R]", {23: b"800"}),
        ("t3-h-box-elem-chain", b"[0 -200 1000 23 0 R]", {23: b"24 0 R", 24: b"800"}),
    ]:
        t(name, matrix + b"/FontBBox " + value + b" " + enc + b"/CharProcs << /B 21 0 R >> " + w, extra, s=b"(B)", kind="fonts-heights")
    # /CharProcs, by GetDictFor; each by GetDirectObjectFor, a stream.
    procs_only = {20: stream(b"", b"410 0 d0")}
    charprocs = b"<< /A 20 0 R >>"
    for name, value, extra in [
        ("t3-procs-ref", b"23 0 R", {23: charprocs}),
        ("t3-procs-chain", b"23 0 R", {23: b"24 0 R", 24: charprocs}),
        ("t3-procs-chain3", b"23 0 R", {23: b"24 0 R", 24: b"25 0 R", 25: charprocs}),
        ("t3-procs-stream", b"23 0 R", {23: stream(b"/A 20 0 R", b"")}),
        ("t3-procs-dangling", b"99 0 R", None),
        ("t3-proc-chain", b"<< /A 23 0 R >>", {23: b"20 0 R"}),
        ("t3-proc-dangling", b"<< /A 99 0 R >>", None),
        ("t3-proc-number", b"<< /A 7 >>", None),
        ("t3-proc-dict", b"<< /A 23 0 R >>", {23: b"<< >>"}),
    ]:
        t(name, matrix + bbox + enc + b"/CharProcs " + value + b" ", {**procs_only, **(extra or {})})
    # A CharProc's numbers, by GetNumber: what's no number is 0, and its last d0
    # or d1 is what's kept.
    for name, proc in [
        ("t3-d0-real", b"410.6 0 d0"),
        ("t3-d0-half", b"0.5005 0 d0"),
        ("t3-d0-name", b"/x 0 d0"),
        ("t3-d0-string", b"(x) 0 d0"),
        ("t3-d0-array", b"[1] 0 d0"),
        ("t3-d0-short", b"410 d0"),
        ("t3-d0-none", b"d0"),
        ("t3-d0-long", b"7 8 410 0 d0"),
        ("t3-d0-late", b"q 410 0 d0 Q"),
        ("t3-d0-twice", b"100 0 d0 410 0 d0"),
        ("t3-d0-then-d1", b"100 0 d0 410 0 0 0 100 100 d1"),
        ("t3-d1-then-d0", b"100 0 0 0 100 100 d1 410 0 d0"),
        ("t3-d0-exp", b"4e2 0 d0"),
        ("t3-d0-neg", b"-410 0 d0"),
        ("t3-d0-past-32", b"4294967296 0 d0"),
        ("t3-d1-base", b"410 0 0 0 100 100 d1"),
        ("t3-d1-name", b"/x 0 0 0 100 100 d1"),
        ("t3-d1-short", b"410 0 0 100 100 d1"),
        ("t3-d1-long", b"7 410 0 0 0 100 100 d1"),
        ("t3-proc-empty", b""),
        ("t3-proc-other", b"q Q"),
        ("t3-proc-junk", b"41 0 obj"),
    ]:
        no_widths(name, proc)
    # /Encoding: none, none of the glyphs is named.
    t("t3-enc-none", matrix + bbox + procs)
    t("t3-enc-empty", matrix + bbox + b"/Encoding << >> " + procs)
    t("t3-enc-win", matrix + bbox + b"/Encoding /WinAnsiEncoding " + procs)
    t("t3-enc-ref", matrix + bbox + b"/Encoding 23 0 R " + procs, {23: b"<< /Differences [65 /A /B /C] >>"})
    t("t3-enc-chain", matrix + bbox + b"/Encoding 23 0 R " + procs, {23: b"24 0 R", 24: b"<< /Differences [65 /A /B /C] >>"})
    t("t3-enc-other-names", matrix + bbox + b"/Encoding << /Differences [65 /B /A /C] >> " + procs)
    t("t3-enc-diff-real", matrix + bbox + b"/Encoding << /Differences [65.9 /A /B /C] >> " + procs)
    t("t3-text", matrix + bbox + b"/Encoding << /Differences [65 /a /b /c] >> " + b"/CharProcs << /a 20 0 R /b 21 0 R /c 22 0 R >> ")
    # A real PDFium parses to another float than the reader's, seen at a scale.
    case("t3-w-rounds-otherwise", b"<< " + head + b"/FontMatrix [10000 0 0 1 0 0] " + bbox + enc + procs
         + b"/FirstChar 65 /Widths [1.00000005960464478 1.5 1] >>", bodies)
    # A Type 3 font's heights: `A`'s top, `g`'s bottom, or the box's.
    ag = b"/CharProcs << /A 20 0 R /g 21 0 R >> /Encoding << /Differences [65 /A 103 /g] >> "
    for name, a, g, keys in [
        ("t3-h-a-g", b"500 0 0 0 100 700 d1", b"500 0 0 -150 100 100 d1", bbox),
        ("t3-h-a-past-box", b"500 0 0 0 100 900 d1", b"500 0 0 -300 100 100 d1", bbox),
        ("t3-h-a-only", b"500 0 0 0 100 700 d1", None, bbox),
        ("t3-h-flip", b"500 0 0 0 100 700 d1", b"500 0 0 -150 100 100 d1", b"/FontBBox [0 800 1000 -200] "),
    ]:
        objects = {20: stream(b"", a)}
        objects[21] = stream(b"", g if g is not None else b"")
        keys_ag = ag if g is not None else b"/CharProcs << /A 20 0 R >> /Encoding << /Differences [65 /A] >> "
        t(name, matrix + keys + keys_ag, objects, s=b"(Az)", kind="fonts-heights")


def cases():
    del ROWS[:]
    simple()
    heights()
    type0()
    type3()
    return list(ROWS)
