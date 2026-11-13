"""pdf-content-streams' PDFium cases: content streams, each drawn on a
one-page PDF with the resources beside it, PDFium's answer the page's
objects (oracle/README.md, `content`). Every case's bytes are written here,
from literals and the standard library; oracle/pdfium.py's `cases` writes
them into cases/ and cases.tsv. A case whose content and resources an
earlier one has already is left out, the earlier one's name kept."""

import zlib

COURIER = b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>"
TIMES = b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /Encoding /WinAnsiEncoding >>"
HELVETICA = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"
FONTS = b"<< /Font << /F0 " + COURIER + b" >> >>"
NONE = b"<< >>"

CASES = []
SEEN = {}


def case(name, content, res=NONE):
    if (content, res) in SEEN:
        return
    SEEN[(content, res)] = name
    files = {"content": content}
    if res != NONE:
        files["res"] = res
    CASES.append((name, "content", files))


def fonts(*pairs):
    """Resources whose /Font has each key, as written, for each font."""
    return b"<< /Font << " + b" ".join(k + b" " + f for k, f in pairs) + b" >> >>"


# Inline images: what a path after one shows is where its data ended.
P = b" 5 5 m 7 7 l S"
T = b" EI 0 0 m 9 9 l S EI" + P
S = b"(x EI 0 0 m 9 9 l S) EI" + P
INNER = b"a EI 0 0 m 9 9 l S "
RES = b"<< /ColorSpace << /CS0 /DeviceGray /CS1 /DeviceRGB /Indexed /DeviceRGB >> >>"
G = b"/W 1 /H 1 /BPC 8 /CS /G"

# An image's dictionary: abbreviations and full names, written twice.
case("abbr-full-values", b"BI /W 1 /H 1 /BPC 8 /CS /DeviceGray /ColorSpace /DeviceRGB ID a" + T)
case("full-first", b"BI /W 1 /H 1 /BPC 8 /ColorSpace /DeviceRGB /CS /DeviceGray ID a" + T)
case("abbr-W-first", b"BI /W 1 /Width 3 /H 1 /BPC 8 /CS /DeviceGray ID a" + T)
case("full-W-first", b"BI /Width 3 /W 1 /H 1 /BPC 8 /CS /DeviceGray ID a" + T)
case("abbr-values-both", b"BI /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB ID a" + T)
case("abbr-value-then-full", b"BI /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /DeviceRGB ID a" + T)
case("full-value-then-abbr", b"BI /W 1 /H 1 /BPC 8 /CS /DeviceGray /ColorSpace /RGB ID a" + T)
case("same-meaning", b"BI /W 1 /H 1 /BPC 8 /CS /DeviceRGB /ColorSpace /RGB ID abc" + T)
case("escaped-key", b"BI /W 1 /H 1 /BPC 8 /C#53 /RGB ID a" + T)
case("escaped-value", b"BI /W 1 /H 1 /BPC 8 /CS /R#47B ID a" + T)
case("res-names", b"BI /W 1 /H 1 /BPC 8 /CS /CS0 /ColorSpace /CS1 ID a" + T, RES)
case("res-I", b"BI /W 1 /H 1 /BPC 8 /CS /I ID a" + T, RES)

# Filters, and sizes PDFium can't count.
case("unknown-L", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /XYZ /L %d ID " % len(INNER) + INNER + b"EI" + P)
case("unknown-string", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /XYZ ID " + S)
case("unknown-array", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [/XYZ /Fl] ID " + S)
case("jpx", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /JPXDecode ID " + S)
case("no-W-string", b"BI /H 1 /BPC 8 /CS /G ID " + S)
case("no-W-L", b"BI /H 1 /BPC 8 /CS /G /L %d ID " % len(INNER) + INNER + b"EI" + P)
case("no-W-plain", b"BI /H 1 /BPC 8 /CS /G ID xy EI" + P)
case("zero-W", b"BI /W 0 /H 1 /BPC 8 /CS /G ID " + S)
case("a85-tilde", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID 9jqo~ EI 0 0 m 9 9 l S ~> EI" + P)
case("a85-end", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID 9jqo^~> EI" + P)
case("a85-first-tilde", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID ~> EI" + P)
case("a85-v", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID 9jqov EI 0 0 m 9 9 l S ~> EI" + P)
case("a85-array", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [/A85 /Fl] ID 9jqo~ EI 0 0 m 9 9 l S ~> EI" + P)
case("ahx", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41> EI" + P)
# A stored deflate block holding ` EI `: zlib at level 0 writes no
# compressed data, so its bytes are the same from any zlib.
case("flate-EI", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /Fl ID " + zlib.compress(b"a EI 0 0 m 9 9 l S b", 0) + b" EI" + P)
case("flate-L", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /Fl /L 6 ID x EI y EI" + P)
case("lzw-plain", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /LZW ID xy EI" + P)
case("string-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F (Fl) ID " + S)
case("number-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F 5 ID a" + T)
case("string-ahx-full", b"BI /W 1 /H 1 /BPC 8 /CS /G /F (ASCIIHexDecode) ID 41> EI" + P)
case("string-ahx-abbr", b"BI /W 1 /H 1 /BPC 8 /CS /G /F (AHx) ID " + S)
case("dict-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F <<>> ID a" + T)
case("null-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F null ID a" + T)
case("true-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F true ID " + S)
case("empty-array-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [] ID a" + T)
case("nested-array-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [[/AHx]] ID a" + T)
case("array-string-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [(XYZ)] ID " + S)
case("escaped-filter-key", b"BI /W 1 /H 1 /BPC 8 /CS /G /#46 /XYZ ID " + S)
case("escaped-filter-value", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /A#48x ID 41> EI" + P)
case("filter-abbr-both", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /XYZ /Filter /AHx ID 41> EI" + P)
case("filter-abbr-both-2", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx /Filter /XYZ ID 41> EI" + P)
case("nested-array-filter-string", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [[/AHx]] ID " + S)
case("number-filter-string", b"BI /W 1 /H 1 /BPC 8 /CS /G /F 5 ID " + S)
case("null-filter-string", b"BI /W 1 /H 1 /BPC 8 /CS /G /F null ID " + S)
case("dict-filter-string", b"BI /W 1 /H 1 /BPC 8 /CS /G /F <<>> ID " + S)
case("array-string-ahx", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [(ASCIIHexDecode)] ID " + S)
case("array-null", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [null] ID " + S)
case("array-dict", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [<<>>] ID " + S)
case("array-number", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [5] ID " + S)
case("array-true", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [true] ID " + S)
case("hex-string-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F <41> ID " + S)
case("closer-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F ) ID " + S)
case("keyword-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F foo ID " + S)
case("array-string-ahx-data", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [(ASCIIHexDecode)] ID 41> EI" + P)
case("dp-array-ahx", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [/AHx] /DP [null] ID 41> EI" + P)
case("ahx-no-end", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 0E EI" + P)
case("ahx-no-end-string", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID " + S)
case("a85-to-end", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID 9jqo EI" + P)
case("rl-no-end", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /RL ID \x05abc EI" + P)
case("unfiltered-past-end", b"BI /W 99 /H 1 /BPC 8 /CS /G ID abc EI" + P)

# What ends an image's dictionary: a keyword in key place makes the BI no
# image; an object that's no name there ends it, the data after it.
case("keyword-in-key-place", b"BI /W 1 /H 1 foo /BPC 8 /CS /G ID a" + T)
case("number-in-key-place", b"BI 7 /W 1 /H 1 /BPC 8 /CS /G ID " + S)
case("true-in-key-place", b"BI true " + G + b" ID " + S)
case("false-in-key-place", b"BI false " + G + b" ID " + S)
case("null-in-key-place", b"BI null " + G + b" ID " + S)
case("string-in-key-place", b"BI " + G + b" (k) xy EI" + P)
case("string-in-key-place-id", b"BI " + G + b" (k) ID ab EI" + P)
case("hex-in-key-place", b"BI " + G + b" <6B> xy EI" + P)
case("array-in-key-place", b"BI " + G + b" [1 2] xy EI" + P)
case("dict-in-key-place", b"BI " + G + b" <</A 1>> xy EI" + P)
case("number-after-entries", b"BI /W 2 /H 1 /BPC 8 /CS /G 7 xyz EI" + P)
case("real-in-key-place", b"BI " + G + b" 1.5 xy EI" + P)
case("pdfium-number-in-key-place", b"BI " + G + b" --5 xy EI" + P)
case("closer-in-key-place", b"BI " + G + b" ) xy EI" + P)
case("bracket-in-key-place", b"BI " + G + b" ] xy EI" + P)
case("dictclose-in-key-place", b"BI " + G + b" >> xy EI" + P)
case("gt-in-key-place", b"BI " + G + b" > xy EI" + P)
case("brace-in-key-place", b"BI " + G + b" { xy EI" + P)
case("closebrace-in-key-place", b"BI " + G + b" } xy EI" + P)
case("object-no-space", b"BI " + G + b" (k)xy EI" + P)
case("object-then-id-data", b"BI " + G + b" 7 ID x EI" + P)
case("ahx-then-object", b"BI " + G + b" /F /AHx 7 41> EI" + P)
case("object-then-ahx", b"BI " + G + b" 7 /F /AHx 41> EI" + P)
case("a85-then-object", b"BI " + G + b" /F /A85 (k) 9jqo^~> EI" + P)
case("rgb-size-then-object", b"BI /W 1 /H 1 /BPC 8 /CS /RGB [0] abcd EI" + P)
case("unknown-filter-then-object", b"BI " + G + b" /F /XYZ 7 " + S)
case("keyword-first", b"BI foo 0 0 m 9 9 l S" + P)
case("keyword-after-value", b"BI " + G + b" foo ID x EI" + P)
case("ei-in-key-place", b"BI EI" + P)
case("bi-in-key-place", b"BI BI " + G + b" ID x EI" + P)
case("bi-after-entry", b"BI /W 1 BI " + G + b" ID x EI" + P)
case("lone-lone", b"BI BI foo" + P)
case("comment-then-keyword", b"BI /W 1 % c ID\nfoo" + P)
case("keyword-tf-text", b"BT BI /F1 24 Tf 10 10 Td (Hi) Tj ET", b"<< /Font << /F1 " + HELVETICA + b" >> >>")
case("operands-before-lone", b"10 20 30 40 BI re f")
case("operands-before-lone-2", b"10 20 30 40 BI foo re f")
case("lone-then-image", b"BI foo BI " + G + b" ID x EI" + P)
case("lone-at-end", b"0 0 m 9 9 l S BI foo")

# A value PDFium reads as no object takes its key out.
case("id-in-value-place", b"BI " + G + b" /F ID x EI" + P)
case("id-in-value-place-cs", b"BI /W 1 /H 1 /BPC 8 /CS ID x EI" + P)
case("id-in-value-then-object", b"BI /W 1 /H 1 /BPC 8 /CS ID 7 x EI" + P)
case("id-in-value-then-id", b"BI /W 1 /H 1 /BPC 8 /CS ID ID abc EI" + P)
case("id-in-value-then-name", b"BI /CS ID /W 1 /H 1 /BPC 8 ID x EI" + P)
case("keyword-in-value-cs", b"BI /W 1 /H 1 /BPC 8 /CS foo ID abc EI" + P)
case("keyword-in-value-erases", b"BI /W 3 /H 1 /BPC 8 /CS /G /CS foo ID abcdefghij EI" + P)
case("keyword-in-value-w", b"BI /W foo /H 1 /BPC 8 /CS /G ID abc EI" + P)
case("closer-in-value-cs", b"BI /W 1 /H 1 /BPC 8 /CS ) ID abc EI" + P)
case("bracket-in-value-cs", b"BI /W 1 /H 1 /BPC 8 /CS ] ID abc EI" + P)
case("dictclose-in-value-cs", b"BI /W 1 /H 1 /BPC 8 /CS >> ID abc EI" + P)
case("brace-in-value-cs", b"BI /W 1 /H 1 /BPC 8 /CS { ID abc EI" + P)
case("bi-in-value-place", b"BI /W 1 /H 1 /BPC 8 /CS BI ID abc EI" + P)
case("true-in-value-cs", b"BI /W 1 /H 1 /BPC 8 /CS true ID abc EI" + P)
case("filter-erased", b"BI " + G + b" /F /AHx /F foo ID 41> EI" + P)

# Nested operands: PDFium ends an array inside an array, outside a
# dictionary, at the inner `]`, and reads a dictionary whose key is no
# name as no object.
case("bad-dict-in-key-place", b"BI " + G + b" << 1 2 >> xy EI" + P)
case("nested-array-in-key-place", b"BI " + G + b" [[1] 2] xy EI" + P)
case("nested-array-value", b"BI /W 1 /H 1 /BPC 8 /CS [[/G]] ID abc EI" + P)
case("nested-array-operand", b"[[1] 2] 3 4 re f")
case("flat-array-operand", b"[1 2] 3 4 re f")
case("bad-dict-operand", b"<< 1 2 >> 3 4 re f")
case("good-dict-operand", b"<< /A 2 >> 3 4 re f")

# How far PDFium reads a nested operand: a dash array's numbers, the
# operands `re` takes and a marked-content property list's /MCID show
# where it ended and what it held.
DASH = b" 0 d 0 0 m 10 10 l S"
MC = b" BDC 0 0 10 10 re f EMC"
END = b"0 0 m 9 9 l S "
case("array-bad-dict", b"[<< 1 2 >> 3]" + DASH)
case("array-dict-ended-by-bracket", b"[<< /A ] 5 >> 6] 3 4 re f")
case("dict-string-key", b"<< (x) 1 >> 3 4 re f")
case("dict-hex-key", b"<< <41> 1 >> 3 4 re f")
case("dict-nested-arrays", b"/Span << /A [[1] 2] /MCID 5 >>" + MC)
case("triple-array", b"1 2 [[[1]]] 5 6 re f")
case("nested-array-dash", b"[1 [2] 0 d 0 0 m 10 10 l S")
case("dict-no-value", b"/Span << /A >>" + MC)
case("dict-array-dict", b"/Span << /A [<< /B 1 >>] /MCID 3 >>" + MC)
case("dict-array-bad-dict", b"/Span << /A [<< /B ] /MCID 3 >>" + MC)
case("dict-array-dict-key", b"/Span << /A [<< 1 >> 2] /MCID 3 >>" + MC)
case("bdc-broken", b"/Span <</MCID 0 1 /Alt>>" + MC)
case("bdc-broken-in-bmc", b"/Span BMC /Span <</MCID 0 1 /Alt>> BDC 0 0 10 10 re f EMC 0 0 5 5 re f EMC")
case("bdc-keyword-value", b"/Span <</MCID 4 /A foo>>" + MC)
case("bdc-string-key-then-dict", b"/Span <</A 1 (/MCID 7) <</MCID 7>>" + MC)
case("image-decode-nested", b"BI /W 1 /H 1 /BPC 8 /CS /G /D [[0 1]] ID a" + T)
case("image-dp-bad-dict", b"BI /W 1 /H 1 /BPC 8 /CS /G /DP << 1 >> ID a" + T)
case("nested-array-filter-id", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [[/AHx]] ID 41> EI" + P)
case("nested-array-filter-hex", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [[/AHx]] 41> EI" + P)
case("keyword-first-in-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [foo /AHx] ID 41> EI" + P)
case("closer-first-in-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [) /AHx] ID 41> EI" + P)
case("devicen-nested-names", b"BI /W 1 /H 1 /BPC 8 /CS [/DeviceN [/A /B] /DeviceGray null] ID abc EI" + P)
case("cs-keyword-first", b"BI /W 3 /H 1 /BPC 8 /CS [foo /DeviceGray] ID abcdefghi EI" + P)
case("cs-erased-by-dict-none", b"BI /W 3 /H 1 /BPC 8 /CS /G /CS << /A >> ID abcdefghi EI" + P)
case("cs-empty-array", b"BI /W 1 /H 1 /BPC 8 /CS [] ID abc EI" + P)
# Each at the content's end, whole and cut short.
for name, x in [("array-bad-dict", b"[<< 1 2 >> 3]"), ("array-dict-ended-by-bracket", b"[<< /A ] 5 >> 6]"),
                ("dict-string-key", b"<< (x) 1 >>"), ("dict-nested-arrays", b"<< /A [[1] 2] >>"),
                ("triple-array", b"[[[1]]]"), ("dict-no-value", b"<< /A >>"),
                ("dict-array-dict", b"<< /A [<< /B 1 >>] >>"), ("bdc-broken", b"/Span <</MCID 0 1 /Alt>> BDC"),
                ("image-cs-nested", b"BI /W 1 /H 1 /BPC 8 /CS [[/G]]"),
                ("image-decode-nested", b"BI /W 1 /H 1 /BPC 8 /CS /G /D [[0 1]]"),
                ("image-dp-bad-dict", b"BI /W 1 /H 1 /BPC 8 /CS /G /DP << 1 >>"),
                ("nested-array-filter", b"BI /W 1 /H 1 /BPC 8 /CS /G /F [[/AHx]] ID")]:
    case(name + "-at-end", END + x)
for name, x in [("array-bad-dict", b"[<< 1 2"), ("array-dict-ended-by-bracket", b"[<< /A"),
                ("dict-string-key", b"<< (x"), ("dict-nested-arrays", b"<< /A [[1]"), ("triple-array", b"[[[1"),
                ("dict-no-value", b"<< /A"), ("dict-array-dict", b"<< /A [<< /B"),
                ("bdc-broken", b"/Span <</MCID 0 1"), ("image-cs-nested", b"BI /W 1 /H 1 /BPC 8 /CS [[/G"),
                ("image-dp-bad-dict", b"BI /W 1 /H 1 /BPC 8 /CS /G /DP << 1")]:
    case(name + "-cut-at-end", END + x)
# How deep PDFium reads: a property list whose last value is 64 levels in,
# and 65, where PDFium's chromium/8076 and 8125 read none (chromium/7881,
# 512); one with an empty dictionary or array 64 levels in, and an array
# there with an element.
DEEP = b"/Span <</MCID 3 /A "
case("deep-64", DEEP + b"<</A " * 63 + b"1" + b" >>" * 64 + MC)
case("deep-65", DEEP + b"<</A " * 64 + b"1" + b" >>" * 65 + MC)
case("deep-64-empty-dict", DEEP + b"<</A " * 63 + b"<< >>" + b" >>" * 64 + MC)
case("deep-64-empty-array", DEEP + b"<</A " * 63 + b"[ ]" + b" >>" * 64 + MC)
case("deep-64-array-element", DEEP + b"<</A " * 63 + b"[1]" + b" >>" * 64 + MC)

# White-space: 0x80 and 0xFF are PDFium's too; 0x0B, 0x85, 0x7F and 0xA0
# aren't.
for name, b in (("80", b"\x80"), ("ff", b"\xff"), ("0b", b"\x0b"), ("85", b"\x85"), ("7f", b"\x7f"),
                ("a0", b"\xa0")):
    case("byte-%s-before-re" % name, b"1 2 3 4" + b + b"re f")
    case("byte-%s-after-tf-name" % name, b"BT /F0" + b + b"10 Tf 0 0 Td (ab) Tj ET", FONTS)
    case("byte-%s-after-id" % name, b"BI /W 1 /H 1 /BPC 8 /CS /G ID" + b + b"a EI 1 2 3 4 re f")
for name, b in (("80", b"\x80"), ("ff", b"\xff"), ("space", b" "), ("0b", b"\x0b"), ("85", b"\x85")):
    case("byte-%s-after-id-2-bytes" % name, b"BI /W 2 /H 1 /BPC 8 /CS /G ID" + b + b"xy EI 1 2 3 4 re f")
    case("byte-%s-after-id-1-byte" % name, b"BI /W 1 /H 1 /BPC 8 /CS /G ID" + b + b"x EI 1 2 3 4 re f")
    case("byte-%s-before-walked-ei" % name, b"BI /F /Xyz ID x y" + b + b"EI 1 2 3 4 re f")
    case("byte-%s-before-walked-ei-sized" % name, b"BI /W 1 /H 1 /BPC 8 /CS /G /F /Xyz ID x y" + b + b"EI 1 2 3 4 re f")

# What's between a filter's end, or an image's size, and `EI`.
case("ahx-end-then-ei", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41> EI 1 2 3 4 re f")
case("ahx-end-tokens-ei", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41> zz 7 EI 1 2 3 4 re f")
case("ahx-end-spaces-ei", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41>   EI 1 2 3 4 re f")
case("unfiltered-spaces-before-ei", b"BI /W 1 /H 1 /BPC 8 /CS /G ID x   EI 1 2 3 4 re f")
case("unfiltered-tokens-before-ei", b"BI /W 1 /H 1 /BPC 8 /CS /G ID x zz EI 1 2 3 4 re f")

# Caps on what PDFium keeps: a word's 255 bytes, a name's, a string's
# 32,767.
for n in (254, 255, 256, 257, 300):
    case("word-%d-bytes-as-cm-a" % n, b"q " + b"0" * (n - 1) + b"2 0 0 1 0 0 cm 0 0 10 10 re f Q")
for n in (255, 256, 257):
    res = b"<< /Font << /" + b"F" * 254 + b" " + COURIER + b" >> >>"
    case("name-%d-bytes-in-tf" % n, b"BT /" + b"F" * 254 + b"X" * (n - 255) + b" 10 Tf 0 0 Td (ab) Tj ET", res)
for n in (32766, 32767, 32768, 32769, 40000):
    case("literal-%d-bytes" % n, b"BT /F0 1 Tf 0 0 Td (" + b"a" * n + b") Tj ET", FONTS)
for n in (20000, 32767, 32768, 40000):
    case("hex-%d-bytes" % n, b"BT /F0 1 Tf 0 0 Td <" + b"41" * n + b"> Tj ET", FONTS)
case("name-300-bytes-in-tf", b"BT /F1" + b"x" * 298 + b" 1 Tf 0 0 Td (ab) Tj ET",
     b"<< /Font << /F1" + b"x" * 253 + b" " + HELVETICA + b" >> >>")

# Operands that aren't numbers read as 0: `cm`'s f.
for name, w in [("true", b"true"), ("name", b"/N"), ("string", b"(5)"), ("hex", b"<35>"),
                ("array", b"[5]"), ("null", b"null"), ("false", b"false")]:
    case("cm-f-" + name, b"q 2 0 0 2 0 " + w + b" cm 0 0 10 10 re f Q")
for n in (254, 255, 256):
    case("cm-f-%d-zeros-7.5" % n, b"q 2 0 0 2 0 " + b"0" * n + b"7.5 cm 0 0 10 10 re f Q")
case("cm-f-signed-past", b"q 2 0 0 2 0 +2147483648 cm 0 0 10 10 re f Q")
case("cm-f-unsigned-past", b"q 2 0 0 2 0 4294967296 cm 0 0 10 10 re f Q")
case("cm-f-unsigned-31", b"q 2 0 0 2 0 2147483648 cm 0 0 10 10 re f Q")
for name, x in [("p1", b"0.1"), ("pi", b"3.14159265358979"), ("2^24+1", b"16777217")]:
    case("cm-a-" + name, b"q " + x + b" 0 0 1 0 0 cm 0 0 10 10 re f Q")

# Numbers as `re`'s width: the f32 PDFium holds.
for name, x in [("f32-max", b"340282346638528859811704183484516925440.0"),
                ("f32-below-mid", b"340282356000000000000000000000000000000.0"),
                ("f32-above-mid", b"340282357000000000000000000000000000000.0"),
                ("past-f32", b"340290000000000000000000000000000000000.0"),
                ("past-f32-neg", b"-340290000000000000000000000000000000000.0"),
                ("p1", b"0.1"), ("1.00000005", b"1.00000005"), ("1.0000001", b"1.0000001"),
                ("2^24+1", b"16777217"), ("2^24+3", b"16777219"), ("2^24+1-real", b"16777217.0"),
                ("micro", b"0.000001"), ("pi", b"3.14159265358979"), ("123456.789", b"123456.789"),
                ("denormal", b"0." + b"0" * 43 + b"1"), ("tiny", b"0." + b"0" * 50 + b"1"),
                ("-0", b"-0"), ("-.5", b"-.5"), ("5.", b"5."), (".5.5", b".5.5"), ("+.5", b"+.5"),
                ("i32-max", b"2147483647"), ("i32-min", b"-2147483648"), ("i32-min-1", b"-2147483649"),
                ("u32-max", b"4294967295"), ("u32-max+1", b"4294967296"), ("+u32", b"+4294967295"),
                ("many-digits-real", b"1." + b"1" * 300), ("lead-zeros-real", b"0" * 200 + b"1.5"),
                ("sign-mid", b"5-3"), ("plus-minus", b"+-5"), ("dots", b"..5")]:
    case("re-w-" + name, b"0 0 " + x + b" 10 re f")

# An inline image's /W, /H and /BPC as PDFium reads an integer from each:
# the data its size counts, unfiltered.
D = b" ID xyzw EI" + P
for name, w in [("1.9", b"1.9"), ("--3", b"--3"), ("-1", b"-1"), ("true", b"true"), ("false", b"false"),
                ("string", b"(3)"), ("name", b"/N"), ("4294967295", b"4294967295"),
                ("4294967297", b"4294967297"), ("2147483648", b"2147483648"), ("+2", b"+2"),
                ("2.9", b"2.9"), ("5e9-real", b"5000000000.0"), ("-0.5", b"-0.5"),
                ("long-255", b"0" * 254 + b"2"), ("long-256", b"0" * 255 + b"2")]:
    case("w-" + name, b"BI /W " + w + b" /H 1 /BPC 8 /CS /G" + D)
for name, h in [("1.9", b"1.9"), ("-1-w1", b"-1"), ("true", b"true"), ("4294967295-w1", b"4294967295")]:
    case("h-" + name, b"BI /W 1 /H " + h + b" /BPC 8 /CS /G" + D)
case("h--1-w2", b"BI /W 2 /H -1 /BPC 8 /CS /G" + D)
for name, bpc in [("true", b"true"), ("2.9", b"2.9"), ("-8", b"-8"), ("--8", b"--8"), ("16", b"16")]:
    case("bpc-" + name, b"BI /W 2 /H 1 /BPC " + bpc + b" /CS /G" + D)
case("bpc--8-rgb", b"BI /W 1 /H 1 /BPC -8 /CS /RGB" + D)
case("w-5e9-bpc1", b"BI /W 5000000000.0 /H 1 /BPC 1 /CS /G" + D)
case("w-5e9-mask", b"BI /IM true /W 5000000000.0 /H 1" + D)
case("w--1-bpc0", b"BI /W -1 /H 1 /BPC 0 /CS /G" + D)
case("w--1-bpc1", b"BI /W -1 /H 1 /BPC 1 /CS /G" + D)
case("w-4294967295-bpc1", b"BI /W 4294967295 /H 1 /BPC 1 /CS /G" + D)
# A row of 2^32 - 1 bits, and of 2^32 - 8: rounded up to a byte, it fits
# 32 bits; adding 7 first, the first doesn't.
case("row-2^32-1", b"BI /W 1431655765 /H 1 /BPC 3 /CS /G" + D)
case("row-2^32-8", b"BI /W 1431655764 /H 1 /BPC 3 /CS /G" + D)
case("mask-w--1", b"BI /IM true /W -1 /H 1 ID xyzw EI" + P)
case("w3-h2-bpc1", b"BI /W 3 /H 2 /BPC 1 /CS /G ID xyzw EI" + P)
case("w9-h1-bpc1", b"BI /W 9 /H 1 /BPC 1 /CS /G ID xyzw EI" + P)
case("w2-cmyk-bpc4", b"BI /W 2 /H 1 /BPC 4 /CS /CMYK ID abcdefgh EI" + P)
case("h-2^32", b"BI /W 1 /H 4294967296 /BPC 8 /CS /G ID xyzw EI" + P)
case("h-2^31", b"BI /W 1 /H 2147483648 /BPC 8 /CS /G ID xyzw EI" + P)
case("w-true-h-true-bpc-true", b"BI /W true /H true /BPC true /CS /G ID xyzw EI" + P)
case("bpc-0", b"BI /W 4 /H 1 /BPC 0 /CS /G ID xyzw EI" + P)
case("w-long-hash-key", b"BI /W#20 2 /W 1 /H 1 /BPC 8 /CS /G ID xyzw EI" + P)

# A filtered image whose size PDFium can't count: no data, read on to the
# first EI from the data's start, not the filter's end.
case("ahx-w--1", b"BI /W -1 /H 1 /BPC 8 /CS /G /F /AHx ID 41 EI 1 1 m 2 2 l S 42> EI" + P)
case("ahx-w-1", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41 EI 1 1 m 2 2 l S 42> EI" + P)
case("ahx-w-2147483648", b"BI /W 2147483648 /H 1 /BPC 8 /CS /G /F /AHx ID 41 EI 1 1 m 2 2 l S 42> EI" + P)
case("ahx-no-size", b"BI /F /AHx ID 41 EI 1 1 m 2 2 l S 42> EI" + P)
case("rl-h-2^32-1-w2", b"BI /W 2 /H 4294967295 /BPC 8 /CS /G /F /RL ID \x01ab\x80 EI 1 1 m 2 2 l S \x80 EI" + P)
case("a85-w--1", b"BI /W -1 /H 1 /BPC 8 /CS /G /F /A85 ID 9 EI 1 1 m 2 2 l S ~> EI" + P)
case("dct-w--1", b"BI /W -1 /H 1 /BPC 8 /CS /G /F /DCT ID 41 EI 1 1 m 2 2 l S 42 EI" + P)
case("fl-w--1", b"BI /W -1 /H 1 /BPC 8 /CS /G /F /Fl ID 41 EI 1 1 m 2 2 l S 42 EI" + P)
case("unknown-w--1", b"BI /W -1 /H 1 /BPC 8 /CS /G /F /XYZ ID 41 EI 1 1 m 2 2 l S 42 EI" + P)

# An image's color space and filter by their names as PDFium decodes them.
CP = b"<< /ColorSpace << /CP /DeviceRGB >> >>"
case("cs-hash-lax", b"BI /W 2 /H 1 /BPC 8 /CS /C#5G ID abcdefgh EI" + P, CP)
case("cs-hash-hex", b"BI /W 2 /H 1 /BPC 8 /CS /C#50 ID abcdefgh EI" + P, CP)
case("cs-plain", b"BI /W 2 /H 1 /BPC 8 /CS /CP ID abcdefgh EI" + P, CP)
case("cs-unknown", b"BI /W 2 /H 1 /BPC 8 /CS /CQ ID abcdefgh EI" + P, CP)
case("cs-array-hash", b"BI /W 2 /H 1 /BPC 8 /CS [/C#5G] ID abcdefgh EI" + P, CP)
case("f-hash-hex", b"BI /W 1 /H 1 /BPC 8 /CS /G /F /A#48x ID 41> EI" + P)
case("f-string-abbr", b"BI /W 1 /H 1 /BPC 8 /CS /G /F (AHx) ID 41> EI" + P)
case("key-hash-lax", b"BI /W 8 /H 1 /B#5GC 8 /CS /G" + D)
case("key-hash-hex", b"BI /W 8 /H 1 /B#50C 8 /CS /G" + D)

# An image's color space as an array in its own dictionary, counted as the same
# array a resources dictionary names would be, the data an image of 8 samples
# in a row; and arrays and names in the resources, read by `ResCx::read`. What
# PDFium ends an array at decides what it holds: a `[` inside it opens none, and
# its first `]` ends it. Nothing here refers to an object, as content can't.
CS_DATA = b"a" * 160
F2_1 = b"<< /FunctionType 2 /Domain [0 1] /N 1 >>"
F2_3 = b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0] /C1 [1 1 1] >>"
F2_4 = b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0 0] /C1 [1 1 1 1] >>"
CAL = b"<< /WhitePoint [0.95 1 1.08] >>"


def cs_image(value, before=b"/W 8 /H 1 /BPC 8 /CS "):
    return b"BI " + before + value + b" ID " + CS_DATA + b" EI" + P


for label, value in [
    ("value-number", b'7'),
    ("value-string", b'(DeviceRGB)'),
    ("value-null", b'null'),
    ("value-true", b'true'),
    ("value-dict", b'<< >>'),
    ("value-empty-array", b'[]'),
    ("one-devicegray", b'[/DeviceGray]'),
    ("one-devicergb", b'[/DeviceRGB]'),
    ("one-cmyk", b'[/CMYK]'),
    ("one-pattern", b'[/Pattern]'),
    ("one-indexed", b'[/Indexed]'),
    ("one-calrgb", b'[/CalRGB]'),
    ("one-lab", b'[/Lab]'),
    ("one-foo", b'[/Foo]'),
    ("one-nested", b'[[/DeviceCMYK]]'),
    ("one-string", b'[(DeviceCMYK)]'),
    ("one-number", b'[7]'),
    ("one-null", b'[null]'),
    ("one-nested-3", b'[[[/G]]]'),
    ("one-dict", b'[<< >>]'),
    ("more-devicegray", b'[/DeviceGray 1]'),
    ("more-devicergb", b'[/DeviceRGB 1]'),
    ("more-devicecmyk", b'[/DeviceCMYK 1]'),
    ("more-g", b'[/G 1]'),
    ("more-rgb", b'[/RGB 1]'),
    ("more-cmyk", b'[/CMYK 1]'),
    ("more-foo", b'[/Foo 1]'),
    ("more-cmyk-null", b'[/DeviceCMYK null]'),
    ("more-as-device-n", b'[/DeviceCMYK [/A] /DeviceGray ' + F2_1 + b']'),
    ("more-null-first", b'[null /DeviceRGB]'),
    ("more-number-first", b'[7 /DeviceRGB]'),
    ("calgray-ok", b'[/CalGray ' + CAL + b']'),
    ("calgray-number", b'[/CalGray 7]'),
    ("calgray-no-white-point", b'[/CalGray << >>]'),
    ("calgray-white-point-short", b'[/CalGray << /WhitePoint [1 1] >>]'),
    ("calgray-y-not-1", b'[/CalGray << /WhitePoint [0.9 0.9 0.9] >>]'),
    ("calgray-x-0", b'[/CalGray << /WhitePoint [0 1 1] >>]'),
    ("calgray-string-family", b'[(CalGray) ' + CAL + b']'),
    ("calgray-longer-family", b'[/CalGrayX ' + CAL + b']'),
    ("calrgb-ok", b'[/CalRGB ' + CAL + b']'),
    ("calrgb-number", b'[/CalRGB 7]'),
    ("calrgb-no-white-point", b'[/CalRGB << >>]'),
    ("calrgb-white-point-short", b'[/CalRGB << /WhitePoint [1 1] >>]'),
    ("calrgb-y-not-1", b'[/CalRGB << /WhitePoint [0.9 0.9 0.9] >>]'),
    ("calrgb-x-0", b'[/CalRGB << /WhitePoint [0 1 1] >>]'),
    ("calrgb-string-family", b'[(CalRGB) ' + CAL + b']'),
    ("calrgb-longer-family", b'[/CalRGBX ' + CAL + b']'),
    ("lab-ok", b'[/Lab ' + CAL + b']'),
    ("lab-number", b'[/Lab 7]'),
    ("lab-no-white-point", b'[/Lab << >>]'),
    ("lab-white-point-short", b'[/Lab << /WhitePoint [1 1] >>]'),
    ("lab-y-not-1", b'[/Lab << /WhitePoint [0.9 0.9 0.9] >>]'),
    ("lab-x-0", b'[/Lab << /WhitePoint [0 1 1] >>]'),
    ("lab-string-family", b'[(Lab) ' + CAL + b']'),
    ("lab-longer-family", b'[/LabX ' + CAL + b']'),
    ("icc-dict", b'[/ICCBased << /N 4 >>]'),
    ("icc-number", b'[/ICCBased 7]'),
    ("indexed-ok", b'[/Indexed /DeviceRGB 1 <00>]'),
    ("indexed-i-rgb", b'[/I /RGB 1 <00>]'),
    ("indexed-three", b'[/Indexed /DeviceRGB 1]'),
    ("indexed-two", b'[/Indexed /DeviceRGB]'),
    ("indexed-base-foo", b'[/Indexed /Foo 1 <00>]'),
    ("indexed-base-pattern", b'[/Indexed /Pattern 1 <00>]'),
    ("indexed-base-indexed", b'[/Indexed [/Indexed /G 1 <00>] 1 <00>]'),
    ("indexed-base-one", b'[/Indexed [/G] 1 <00>]'),
    ("indexed-base-separation", b'[/Indexed [/Separation /X /DeviceCMYK ' + F2_4 + b'] 1 <00>]'),
    ("indexed-hival-999", b'[/Indexed /DeviceRGB 999 <00>]'),
    ("indexed-hival-minus-1", b'[/Indexed /DeviceRGB -1 <00>]'),
    ("indexed-hival-string", b'[/Indexed /DeviceRGB (x) <00>]'),
    ("indexed-table-number", b'[/Indexed /DeviceRGB 1 7]'),
    ("indexed-table-null", b'[/Indexed /DeviceRGB 1 null]'),
    ("indexed-longer-family", b'[/IndexedX /RGB 1 <00>]'),
    ("indexed-four-bytes", b'[/Inde /RGB 1 <00>]'),
    ("indexed-three-bytes", b'[/Ind /RGB 1 <00>]'),
    ("separation-none", b'[/Separation /None]'),
    ("separation-none-string", b'[/Separation (None) 7]'),
    ("separation-no-alternate", b'[/Separation /X]'),
    ("separation-alternate-foo", b'[/Separation /X /Foo]'),
    ("separation-no-function", b'[/Separation /X /DeviceCMYK]'),
    ("separation-ok", b'[/Separation /X /DeviceCMYK ' + F2_4 + b']'),
    ("separation-function-bad", b'[/Separation /X /DeviceCMYK << /FunctionType 1 /Domain [0 1] >>]'),
    ("separation-alternate-indexed", b'[/Separation /X [/Indexed /G 1 <00>] ' + F2_1 + b']'),
    ("separation-alternate-pattern", b'[/Separation /X /Pattern ' + F2_1 + b']'),
    ("separation-all", b'[/Separation /All /DeviceCMYK ' + F2_4 + b']'),
    ("separation-name-number", b'[/Separation 7 /DeviceGray]'),
    ("separation-longer-family", b'[/SeparationX /X /DeviceCMYK]'),
    ("device-n-f2-4", b'[/DeviceN [/A /B] /DeviceCMYK ' + F2_4 + b']'),
    ("device-n-f2-3", b'[/DeviceN [/A /B] /DeviceCMYK ' + F2_3 + b']'),
    ("device-n-f2-1", b'[/DeviceN [/A /B] /DeviceCMYK ' + F2_1 + b']'),
    ("device-n-f2-2-inputs", b'[/DeviceN [/A /B] /DeviceCMYK << /FunctionType 2 /Domain [0 1 0 1] /N 1 /C0 [0 0] /C1 [1 1] >>]'),
    ("device-n-f2-range-4", b'[/DeviceN [/A /B] /DeviceCMYK << /FunctionType 2 /Domain [0 1] /N 1 /Range [0 1 0 1 0 1 0 1] >>]'),
    ("device-n-f2-no-n", b'[/DeviceN [/A /B] /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] >>]'),
    ("device-n-f3", b'[/DeviceN [/A /B] /DeviceCMYK << /FunctionType 3 /Domain [0 1] /Functions [' + F2_4 + b'] /Bounds [] /Encode [0 1] >>]'),
    ("device-n-f-bad", b'[/DeviceN [/A /B] /DeviceCMYK << /FunctionType 1 /Domain [0 1] >>]'),
    ("device-n-no-function", b'[/DeviceN [/A /B] /DeviceCMYK]'),
    ("device-n-function-name", b'[/DeviceN [/A /B] /DeviceCMYK /Identity]'),
    ("device-n-function-null", b'[/DeviceN [/A /B] /DeviceCMYK null]'),
    ("device-n-f0-dict", b'[/DeviceN [/A /B] /DeviceCMYK << /FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1] /Size [2 2] /BitsPerSample 8 >>]'),
    ("device-n-f4-dict", b'[/DeviceN [/A /B] /DeviceCMYK << /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1] >>]'),
    ("device-n-names-empty", b'[/DeviceN [] /DeviceGray ' + F2_1 + b']'),
    ("device-n-names-name", b'[/DeviceN /A /DeviceGray ' + F2_1 + b']'),
    ("device-n-names-9", b'[/DeviceN [/A /B /C /D /E /F /G /H /I] /DeviceGray ' + F2_1 + b']'),
    ("device-n-alternate-foo", b'[/DeviceN [/A /B] /Foo ' + F2_1 + b']'),
    ("device-n-alternate-pattern", b'[/DeviceN [/A /B] /Pattern ' + F2_1 + b']'),
    ("device-n-alternate-indexed", b'[/DeviceN [/A /B] [/Indexed /G 1 <00>] ' + F2_1 + b']'),
    ("device-n-attributes", b'[/DeviceN [/A /B] /DeviceGray ' + F2_1 + b' << /Subtype /NChannel >>]'),
    ("device-n-names-not-names", b'[/DeviceN [7 (x)] /DeviceGray ' + F2_1 + b']'),
    ("pattern-rgb", b'[/Pattern /DeviceRGB]'),
    ("pattern-cmyk", b'[/Pattern /DeviceCMYK]'),
    ("pattern-gray", b'[/Pattern /DeviceGray]'),
    ("pattern-foo", b'[/Pattern /Foo]'),
    ("pattern-number", b'[/Pattern 7]'),
    ("pattern-null", b'[/Pattern null]'),
    ("pattern-pattern", b'[/Pattern /Pattern]'),
    ("pattern-one-pattern", b'[/Pattern [/Pattern]]'),
    ("pattern-indexed", b'[/Pattern [/Indexed /G 1 <00>]]'),
    ("pattern-pattern-g", b'[/Pattern [/Pattern /G]]'),
    ("pattern-device-n-17", b'[/Pattern [/DeviceN [/A /B /C /D /E /F /G /H /I /J /K /L /M /N /O /P /Q] /DeviceGray ' + F2_1 + b']]'),
    ("pattern-device-n-16", b'[/Pattern [/DeviceN [/A /B /C /D /E /F /G /H /I /J /K /L /M /N /O /P] /DeviceGray ' + F2_1 + b']]'),
    ("pattern-longer-family", b'[/PatternX /DeviceRGB]'),
    ("pattern-more-elements", b'[/Pattern /DeviceRGB 7 7 7]'),
    ("one-i", b'[/I]'),
    ("one-g", b'[/G]'),
    ("lab-point-1", b'[/Lab <</WhitePoint [1 1 1]>>]'),
    ("icc-reference", b'[/ICCBased 5 0 R]'),
    ("pattern-abbreviated-rgb", b'[/Pattern /RGB]'),
    ("pattern-abbreviated-g", b'[/Pattern /G]'),
    ("pattern-one-rgb", b'[/Pattern [/RGB]]'),
    ("indexed-base-one-rgb", b'[/Indexed [/RGB] 1 <00>]'),
    ("calgray-point-1", b'[/CalGray << /WhitePoint [1 1 1] >>]'),
    ("calgray-point-open", b'[/CalGray << /WhitePoint [1 1 1 >>]'),
    # A family by its first four bytes, however it's written.
    ("family-string-pattern", b'[(Pattern) /DeviceRGB]'),
    ("family-hex-pattern", b'[<50617474> /DeviceRGB]'),
    ("family-escaped-pattern", b'[/P#61ttern /DeviceRGB]'),
    ("family-lab-nul", b'[/Lab#00zz ' + CAL + b']'),
    ("family-i-nul", b'[/I#00 /RGB 1 <00>]'),
    # A word that's no object isn't in the array, so doesn't count.
    ("keyword-before-family", b'[foo /Pattern /DeviceRGB]'),
    ("keyword-after-family", b'[/Pattern foo /DeviceRGB]'),
    ("keyword-alone", b'[foo]'),
    ("pattern-reference", b'[/Pattern 5 0 R]'),
    # A white point's numbers, as PDFium's content parser reads them.
    ("white-point-y-0-99999999", b'[/CalGray << /WhitePoint [1 0.99999999 1] >>]'),
    ("white-point-y-1-0000001", b'[/CalGray << /WhitePoint [1 1.0000001 1] >>]'),
    ("white-point-y-rounds-up-from-half", b'[/CalGray << /WhitePoint [1 1.00000005960464478 1] >>]'),
    ("white-point-y-true", b'[/CalGray << /WhitePoint [1 true 1] >>]'),
    ("white-point-x-exponent", b'[/CalGray << /WhitePoint [1e5 1 1] >>]'),
    ("white-point-x-plus-past-31-bits", b'[/CalGray << /WhitePoint [+3000000000 1 1] >>]'),
    ("white-point-long-4", b'[/CalGray << /WhitePoint [1 1 1 1] >>]'),
]:
    case("cs-" + label, cs_image(value))
case("cs-full-key", cs_image(b"[/Pattern /DeviceRGB]", b"/W 8 /H 1 /BPC 8 /ColorSpace "))
case("cs-image-mask", cs_image(b"[/Pattern /DeviceRGB]", b"/IM true /W 8 /H 1 /BPC 8 /CS "))
case("cs-no-bpc", cs_image(b"[/Pattern /DeviceRGB]", b"/W 8 /H 1 /CS "))
for label, spaces in [
    ("pattern", b"/X [/Pattern /DeviceRGB]"),
    ("pattern-cmyk", b"/X [/Pattern /DeviceCMYK]"),
    ("indexed", b"/X [/Indexed /DeviceRGB 1 <00>]"),
    ("indexed-three", b"/X [/Indexed /DeviceRGB 1]"),
    ("separation", b"/X [/Separation /A /DeviceCMYK " + F2_4 + b"]"),
    ("device-n", b"/X [/DeviceN [/A /B /C] /DeviceGray " + F2_1 + b"]"),
    ("device-n-no-function", b"/X [/DeviceN [/A /B /C] /DeviceGray]"),
    ("device-n-nested", b"/X [/Pattern [/DeviceN [/A /B /C] /DeviceGray " + F2_1 + b"]]"),
    ("calgray", b"/X [/CalGray " + CAL + b"]"),
    ("calgray-number", b"/X [/CalGray 7]"),
    ("one-nested", b"/X [[/DeviceCMYK]]"),
    ("name-device", b"/X /DeviceCMYK"),
    ("name-family", b"/X /CalGray"),
    ("name-other", b"/X /Y /Y /DeviceCMYK"),
    ("number", b"/X 7"),
    ("null", b"/X null"),
    ("nothing-there", b"/X 99 0 R"),
    ("written-twice", b"/X /DeviceGray /X /DeviceCMYK"),
    ("escaped-key", b"/#58 /DeviceCMYK"),
]:
    case("cs-named-" + label, cs_image(b"/X"), b"<< /ColorSpace << " + spaces + b" >> >>")
case("cs-named-not-a-dictionary", cs_image(b"/X"), b"<< /ColorSpace [/X /DeviceCMYK] >>")
case("cs-named-reference-to-nothing", cs_image(b"/X"), b"<< /ColorSpace 99 0 R >>")

# Names and strings shown as text: a name's `#` finding the font, a
# string's CR and its cap.
case("tf-hash-lax", b"BT /F#3G 10 Tf 0 0 Td (ab) Tj ET", FONTS)
case("tf-hash-hex", b"BT /F#30 10 Tf 0 0 Td (ab) Tj ET", FONTS)
case("tf-hash-one-digit", b"BT /F#3 10 Tf 0 0 Td (ab) Tj ET", FONTS)
case("tf-hash-short", b"BT /F0#3 10 Tf 0 0 Td (ab) Tj ET", fonts((b"/F0#3", COURIER)))
case("tj-cr", b"BT /F0 10 Tf 0 0 Td (a\rb\r\nc) Tj ET", FONTS)
case("tj-array-cr", b"BT /F0 10 Tf 0 0 Td [(a\rb) -250 (c)] TJ ET", FONTS)
case("tj-hex-cap", b"BT /F0 1 Tf 0 0 Td <" + b"41" * 32768 + b"4> Tj ET", FONTS)
case("tj-literal-escapes-cap", b"BT /F0 1 Tf 0 0 Td (" + b"\\101" * 32768 + b") Tj ET", FONTS)
A = b"A"
case("name-straddle-253", b"BT /" + A * 252 + b"#41 10 Tf 0 0 Td (ab) Tj ET",
     fonts((b"/" + A * 252 + b"#4", COURIER), (b"/" + A * 253, TIMES)))
case("name-straddle-254", b"BT /" + A * 253 + b"#41 10 Tf 0 0 Td (ab) Tj ET",
     fonts((b"/" + A * 253 + b"#", COURIER), (b"/" + A * 254, TIMES)))
case("name-255-escape-inside", b"BT /" + A * 251 + b"#41 10 Tf 0 0 Td (ab) Tj ET",
     fonts((b"/" + A * 252, COURIER), (b"/" + A * 251 + b"#4", TIMES)))
case("name-lax-high-nibble", b"BT /F#G1 10 Tf 0 0 Td (ab) Tj ET", fonts((b"/F#01", COURIER), (b"/F#G1x", TIMES)))
case("name-lax-both", b"BT /F#zz 10 Tf 0 0 Td (ab) Tj ET", fonts((b"/F#00", COURIER)))
case("name-hash-23", b"BT /F#23 10 Tf 0 0 Td (ab) Tj ET", fonts((b"/F#23", COURIER)))
F0 = fonts((b"/F0", COURIER))
for name, s in [("bs-cr", b"(a\\\rb)"), ("bs-crlf", b"(a\\\r\nb)"), ("bs-lf", b"(a\\\nb)"), ("cr-at-end", b"(ab\r)"),
                ("cr-cr-lf", b"(a\r\r\nb)"), ("lf-cr", b"(a\n\rb)"), ("cr-escaped", b"(a\\rb)"),
                ("cr-before-paren", b"(a\r)"), ("octal-cr", b"(a\\015b)"), ("nested-cr", b"(a(b\rc)d)")]:
    case("tj-" + name, b"BT /F0 10 Tf 0 0 Td " + s + b" Tj ET", F0)
for name, x in [("1.5.5", b"1.5.5"), ("--3", b"--3"), ("+", b"+"), ("true", b"true"), ("long", b"0" * 254 + b"9")]:
    case("tf-size-" + name, b"BT /F0 " + x + b" Tf 0 0 Td (ab) Tj ET", F0)

# Numbers inside an operand's array or dictionary: a marked-content
# property list's /MCID, a dash array.
R = b" 0 0 10 10 re f EMC"
for name, w in [("7", b"7"), ("--5", b"--5"), ("1.2.3", b"1.2.3"), ("4294967296", b"4294967296"),
                ("+2147483648", b"+2147483648"), ("20-nines", b"9" * 20),
                ("20-nines--5", b"9" * 20 + b"--5"), ("-20-nines", b"-" + b"9" * 20),
                ("19-digits", b"9223372036854775807"), ("2.5", b"2.5")]:
    case("mcid-" + name, b"/Span << /MCID " + w + b" /ActualText (a) >> BDC" + R)
case("mcid-in-array", b"/Span << /K [--5] /MCID 3 >> BDC" + R)
L = b" 0 0 m 10 10 l S"
for name, a in [("plain", b"[2 3]"), ("--5", b"[--5 3]"), ("1.2.3", b"[1.2.3 4]"),
                ("4294967296", b"[4294967296 2]"), ("20-nines", b"[" + b"9" * 20 + b" 2]"),
                ("20-nines--5", b"[" + b"9" * 20 + b"--5 2]")]:
    case("dash-" + name, a + b" 0 d" + L)


# Flate's and LZW's ends, where PDFium decodes the data to find them: what
# zlib counts it read, and what PDFium's LZW decoder does, then on to `EI`;
# data a decoder or its predictor refuses, or whose parameters it refuses,
# read on from its start, and so an end with no `EI` after it, for every
# filter whose end PDFium's decoder finds. The deflate data is written
# here, stored blocks and fixed-Huffman blocks bit by bit, and one stream of
# dynamic blocks as its bytes; nothing is compressed by the zlib Python
# links, whose output differs between builds.


class Bits:
    """Bits packed as deflate packs them, the least significant first."""

    def __init__(self):
        self.out = bytearray()
        self.acc = 0
        self.n = 0

    def put(self, value, n):
        for i in range(n):
            self.acc |= ((value >> i) & 1) << self.n
            self.n += 1
            if self.n == 8:
                self.out.append(self.acc)
                self.acc, self.n = 0, 0

    def code(self, code, n):
        """A Huffman code, its most significant bit first."""
        for i in reversed(range(n)):
            self.put((code >> i) & 1, 1)

    def bytes(self):
        if self.n:
            self.out.append(self.acc)
            self.acc, self.n = 0, 0
        return bytes(self.out)


def symbol(b, sym):
    """A literal or length symbol in deflate's fixed code."""
    if sym < 144:
        b.code(0x30 + sym, 8)
    elif sym < 256:
        b.code(0x190 + sym - 144, 9)
    elif sym < 280:
        b.code(sym - 256, 7)
    else:
        b.code(0xC0 + sym - 280, 8)


def fixed(data, last=True, extra=()):
    """A fixed-Huffman block: `data` as literals, then `extra`, each a
    symbol or ("dist", a distance code), then its end."""
    b = Bits()
    b.put(1 if last else 0, 1)
    b.put(1, 2)
    for x in data:
        symbol(b, x)
    for e in extra:
        if isinstance(e, tuple):
            b.code(e[1], 5)
        else:
            symbol(b, e)
    symbol(b, 256)
    return b.bytes()


def stored(data, last=True):
    n = len(data)
    return bytes([1 if last else 0]) + n.to_bytes(2, "little") + (n ^ 0xFFFF).to_bytes(2, "little") + data


def zlib_data(blocks, data, header=b"\x78\x01", check=None):
    """A zlib stream: a header (deflate, no preset dictionary), the blocks,
    and the Adler-32 of `data`, or `check`."""
    return header + blocks + (zlib.adler32(data) if check is None else check).to_bytes(4, "big")


def lzw(codes, early=True):
    """LZW codes, the most significant bit first, each as wide as the
    decoder's table makes it then, one entry early with `early`."""
    bits = []
    width, entry, first = 9, 258, True
    for c in codes:
        assert c < 1 << width
        bits += [(c >> i) & 1 for i in reversed(range(width))]
        if c == 256:
            width, entry, first = 9, 258, True
        elif c == 257:
            pass
        elif first:
            first = False
        elif entry < 4096:
            if entry + early in (511, 1023, 2047):
                width += 1
            entry += 1
    bits += [0] * (-len(bits) % 8)
    return bytes(int("".join(map(str, bits[i:i + 8])), 2) for i in range(0, len(bits), 8))


FL = G + b" /F /Fl"
LZ = G + b" /F /LZW"
TEXT = b"hello, hello, hello, hello"
LONG = bytes((i * 7 + 13) % 251 for i in range(3000))


def image(name, d, data, tail=T):
    case(name, b"BI " + d + b" ID " + data + tail)


# Ended cleanly: its last byte at a byte's start, and in its middle; two
# blocks; `EI` at once after its checksum.
FIXED = zlib_data(fixed(TEXT), TEXT)
image("fl-stored", FL, zlib_data(stored(TEXT), TEXT))
image("fl-fixed", FL, FIXED)
image("fl-fixed-long", FL, zlib_data(fixed(LONG), LONG))
image("fl-two-blocks", FL, zlib_data(stored(b"abc", last=False) + fixed(b"def"), b"abcdef"))
image("fl-end-then-EI", FL, FIXED, T[1:])
# Its checksum missing, or cut short, and so read from what follows; one
# that doesn't match.
image("fl-no-checksum", FL, FIXED[:-4])
image("fl-short-checksum", FL, FIXED[:-2])
image("fl-bad-checksum", FL, zlib_data(fixed(TEXT), TEXT, check=0x01020304))
# Headers zlib refuses: a check that fails, a method not deflate, a window
# too large, a preset dictionary (`(4` names one, and reads as a string).
image("fl-header-check", FL, b"\x78\x02" + FIXED[2:])
image("fl-header-method", FL, b"\x79\x9d" + FIXED[2:])
image("fl-header-window", FL, b"\x88\x98" + FIXED[2:])
image("fl-header-dict", FL, b"\x78\x20abcd" + FIXED[2:])
image("fl-header-dict-paren", FL, b"(4abcdEI 0 0 m 9 9 l S EI" + P, b"")
# Corrupt: a block of type 3, a stored block's length and its complement
# that don't agree, a length symbol 286, a distance code 30, a distance
# past the start; and cut short, so read on into what follows.
image("fl-type-3", FL, b"\x78\x01\x07" + b"\x00" * 6)
image("fl-stored-nlen", FL, b"\x78\x01\x01\x05\x00\x00\x00abcde" + b"\x00" * 4)
image("fl-code-286", FL, zlib_data(fixed(TEXT, extra=[286]), TEXT))
image("fl-code-286-long", FL, zlib_data(fixed(LONG, extra=[286]), LONG))
image("fl-dist-30", FL, zlib_data(fixed(TEXT, extra=[257, ("dist", 30)]), TEXT))
far = Bits()
far.put(1, 1)
far.put(1, 2)
symbol(far, 257)
far.code(5, 5)
far.put(0, 1)
symbol(far, 256)
image("fl-too-far", FL, zlib_data(far.bytes(), b""))
image("fl-cut", FL, FIXED[:10])
image("fl-cut-header", FL, FIXED[:2])
image("fl-cut-long", FL, zlib_data(fixed(LONG), LONG)[:1500])
# Dynamic blocks: more length codes than there are; code lengths that make
# no code, where decoders part (zlib reads on, miniz_oxide refuses at once);
# and a stream of them, whole and with a byte flipped at places, each
# showing a kind of end: corrupt early, the checksum alone, corrupt later,
# and cut short by a length.
hlit = Bits()
for v, n in [(1, 1), (2, 2), (30, 5), (0, 5), (0, 4)]:
    hlit.put(v, n)
image("fl-dyn-hlit", FL, b"\x78\x01" + hlit.bytes() + b"\x00" * 8)
lengths = Bits()
for v, n in [(1, 1), (2, 2), (0, 5), (0, 5), (0, 4)] + [(0, 3)] * 4:
    lengths.put(v, n)
image("fl-dyn-lengths", FL, b"\x78\x01" + lengths.bytes() + b"\x00" * 8)
DYN = bytes.fromhex(
    "78daed90d915c3200c045ba184186cc095d17e66025d643f78123a56d2ac4f5957597759"
    "2f4e2fab3e6535eccdbf139e641b61dc7b94352a3e11a21dfb5a4d7a28c3ffc17fed5650"
    "5dfff80ff9490be5159589ff10aeb44c073984dc859db47725a869e42ffb504169523228"
    "1fda76de8999b3a6ef0e3b555049c5eb4c7092139dec066ee2466e36f79eeefbdbfb3a77"
    "f47d97f779a7f77a773f1ceaa6221d29b5cd4c76326c87a96cdf1f6892811ee8811ee881"
    "1ee8811ee8811ee8811ee8811ee8811ee8811ee8811ee8ff02fd0b4e83ae69")
image("fl-dyn", FL, DYN)
for at, flip in [(3, 0x80), (40, 0x01), (100, 0x80), (204, 0xFF)]:
    bad = bytearray(DYN)
    bad[at] ^= flip
    image("fl-dyn-%d-%02x" % (at, flip), FL, bytes(bad))

# Predictors and parameters, on six bytes stored, or none: PDFium refuses
# parameters negative or too large, a PNG predictor on rows of no bytes or
# on nothing, TIFF's on rows of no bytes, each read as an integer, the
# first of an array where the filter is one.
SIX = zlib_data(stored(b"abcdef"), b"abcdef")
NONE_DECODED = zlib_data(stored(b""), b"")
for name, dp, data in [
        ("fl-png", b"<< /Predictor 12 /Columns 1 >>", SIX),
        ("fl-png-short-row", b"<< /Predictor 12 /Columns 4 >>", SIX),
        ("fl-png-bad-tag", b"<< /Predictor 15 /Columns 1 >>", zlib_data(stored(b"\x07a\x07b"), b"\x07a\x07b")),
        ("fl-png-nothing", b"<< /Predictor 12 >>", NONE_DECODED),
        ("fl-tiff-nothing", b"<< /Predictor 2 >>", NONE_DECODED),
        ("fl-png-columns-0", b"<< /Predictor 10 /Columns 0 >>", SIX),
        ("fl-tiff", b"<< /Predictor 2 /Colors 3 /Columns 2 >>", SIX),
        ("fl-tiff-columns-0", b"<< /Predictor 2 /Columns 0 >>", SIX),
        ("fl-predictor-9-columns-0", b"<< /Predictor 9 /Columns 0 >>", SIX),
        ("fl-colors--1", b"<< /Colors -1 >>", SIX),
        ("fl-bpc--1", b"<< /BitsPerComponent -1 >>", SIX),
        ("fl-columns--1", b"<< /Columns -1 >>", SIX),
        ("fl-colors--1.5", b"<< /Colors -1.5 >>", SIX),
        ("fl-colors-true", b"<< /Predictor 12 /Colors true /Columns 1 >>", SIX),
        ("fl-colors-name", b"<< /Predictor 2 /Colors /G >>", SIX),
        ("fl-params-at-edge", b"<< /Columns 2147483640 /BitsPerComponent 1 >>", SIX),
        ("fl-params-past-edge", b"<< /Columns 2147483641 /BitsPerComponent 1 >>", SIX),
        ("fl-params-overflow-first", b"<< /Columns 1073741824 /Colors 4 /BitsPerComponent 0 >>", SIX),
        ("fl-bpc-abbr", b"<< /BPC -1 >>", SIX),
        ("fl-bpc-abbr-wins", b"<< /BitsPerComponent 8 /BPC -1 >>", SIX),
        ("fl-bpc-escaped", b"<< /B#50C -1 >>", SIX),
        ("fl-bpc-full-abbr-name", b"<< /BitsPerComponent /G /BPC -1 >>", SIX),
        ("fl-params-twice", b"<< /Colors -1 /Colors 1 >>", SIX),
        ("fl-params-array", b"[<< /Colors -1 >>]", SIX)]:
    image(name, FL + b" /DP " + dp, data)
FA = G + b" /F [/Fl]"
image("fl-array-params-array", FA + b" /DP [<< /Colors -1 >>]", SIX)
image("fl-array-params-dict", FA + b" /DP << /Colors -1 >>", SIX)
image("fl-array-params-null-first", FA + b" /DP [null << /Colors -1 >>]", SIX)
image("fl-array-params-keyword-first", FA + b" /DP [foo << /Colors -1 >>]", SIX)
image("fl-params-full-and-abbr", FL + b" /DecodeParms << /Colors -1 >> /DP << >>", SIX)

# LZW: ended by 257 at each code width, either early change, read on past
# a full table; codes PDFium's decoder refuses (one past the table first
# after a clear code or at the start, nothing decoded) and one it reads on
# past; no 257, and bytes that aren't codes; `/EarlyChange` as PDFium reads
# an integer from it.
A, B = 65, 66
image("lzw-end", LZ, lzw([256, A, B, 258, 257]))
image("lzw-end-0", LZ + b" /DP << /EarlyChange 0 >>", lzw([256, A, B, 258, 257], early=False))
image("lzw-end-then-EI", LZ, lzw([256, A, B, 258, 257]), T[1:])
for name, codes in [
        ("lzw-no-clear", [A, B, 258, 257]),
        ("lzw-kwkwk", [256, A, 258, 257]),
        ("lzw-nothing", [256, 257]),
        ("lzw-first-entry", [256, 300, A, 257]),
        ("lzw-start-entry", [300, A, 257]),
        ("lzw-cleared-entry", [256, A, B, 256, 300, A, 257]),
        ("lzw-bad-code", [256, A, B, 300, 257]),
        ("lzw-bad-code-then-entry", [256, A, B, 300, 258, A, 257]),
        ("lzw-bad-code-then-bytes", [256, A, B, 300, A, B, 257]),
        ("lzw-no-end", [256, A, B, 258])]:
    image(name, LZ, lzw(codes))
image("lzw-no-end-x", LZ, b"xy")
WIDE = [256] + [(i * 37) % 251 + 1 for i in range(260)] + [257]
image("lzw-wide", LZ, lzw(WIDE))
image("lzw-wide-0", LZ + b" /DP << /EarlyChange 0 >>", lzw(WIDE, early=False))
image("lzw-wide-early-2", LZ + b" /DP << /EarlyChange 2 >>", lzw(WIDE))
image("lzw-wide-early-null", LZ + b" /DP << /EarlyChange null >>", lzw(WIDE, early=False))
image("lzw-wide-0-read-early", LZ, lzw(WIDE, early=False))
FILL = [256] + [1 + (k * 37 % 255) for k in range(3839)]
image("lzw-full", LZ, lzw(FILL + [A, 300, 4095, 257]))
image("lzw-full-0", LZ + b" /DP << /EarlyChange 0 >>", lzw(FILL + [A, 300, 4095, 257], early=False))
image("lzw-png-columns-0", LZ + b" /DP << /Predictor 12 /Columns 0 >>", lzw([256, A, B, 258, 257]))

# Read on past a code past the table's next entry, but the first since a
# clear code, as PDFium's decoder reads it, as that entry: past it by one
# and by many, then 257; twice in a row, where a code from 258 on after
# one is the last read; twice, bytes between; the entry the code after it
# makes, which holds it, read while it's still past the table, and while
# it's that entry's own, a run of 4,000 bytes; at the 9-to-10-bit change,
# with and without early change; after a full table and a clear code; at
# the start, no clear code first.
C = 67
BYTES = [(i * 37) % 251 + 1 for i in range(253)]
for name, codes in [
        ("lzw-past-by-one", [256, A, B, 260, 257]),
        ("lzw-past-by-many", [256, A, B, 511, A, 257]),
        ("lzw-past-twice-in-a-row", [256, A, B, 300, 301, 257]),
        ("lzw-past-twice", [256, A, B, 300, A, 300, 257]),
        ("lzw-past-entry-after", [256, A, B, 300, C, 260, 257]),
        ("lzw-past-entry-run", [256, A, B, 260, C, 260, 257]),
        ("lzw-past-at-10-bits", [256] + BYTES + [511, A, 257]),
        ("lzw-past-after-full", FILL + [256, A, B, 300, 257]),
        ("lzw-past-at-start", [A, B, 300, A, 257])]:
    image(name, LZ, lzw(codes))
image("lzw-past-at-10-bits-0", LZ + b" /DP << /EarlyChange 0 >>", lzw([256] + BYTES + [511, A, 257], early=False))
image("lzw-past-after-full-0", LZ + b" /DP << /EarlyChange 0 >>", lzw(FILL + [256, A, B, 300, 257], early=False))
# Predictors PDFium refuses behind LZW and Flate, rows of no bits: TIFF's
# with no columns or colors, PNG's of no bits a sample; one it doesn't, of
# 3 bits a sample, which PDFium undoes and this crate's predictor doesn't,
# the data's end PDFium's all the same; PNG's behind LZW data that decodes
# to nothing, which PDFium's decoder refuses first.
image("lzw-tiff-columns-0", LZ + b" /DP << /Predictor 2 /Columns 0 >>", lzw([256, A, B, 258, 257]))
image("lzw-png-bpc-0", LZ + b" /DP << /Predictor 12 /BitsPerComponent 0 >>", lzw([256, A, B, 258, 257]))
image("fl-tiff-colors-0", FL + b" /DP << /Predictor 2 /Colors 0 >>", SIX)
image("fl-png-bpc-3", FL + b" /DP << /Predictor 12 /BitsPerComponent 3 /Columns 2 >>", SIX)
image("lzw-tiff-bpc-3", LZ + b" /DP << /Predictor 2 /BitsPerComponent 3 /Columns 2 >>", lzw([256, A, B, 258, 257]))
image("lzw-png-nothing", LZ + b" /DP << /Predictor 12 >>", lzw([256, 257]))
# One byte of data under TIFF's predictor on rows of 2^31 - 8 one-bit
# samples, then a Flate image: both read, and the path after them, the
# first's end where its decoder stops, whatever its rows would hold.
WIDE = b" /DP << /Predictor 2 /Columns 2147483640 /BitsPerComponent 1 >>"
THEN = b" EI BI " + FL + b" ID " + zlib_data(stored(b"xyz"), b"xyz") + b" EI" + P
image("fl-tiff-wide-then-fl", FL + WIDE, zlib_data(stored(b"\x01"), b"\x01"), THEN)
image("lzw-tiff-wide-then-fl", LZ + WIDE, lzw([256, 1, 257]), THEN)

# An end with no `EI` after it: no stream, read on from the data's start,
# for each filter whose end a decoder finds (a JPEG's by its end of image
# here, PDFium refusing this one).
IN = b"a EI 0 0 m 9 9 l S b"
image("ahx-end-no-EI", G + b" /F /AHx", b"41 EI 0 0 m 9 9 l S 42>", P)
image("a85-end-no-EI", G + b" /F /A85", b"9jqo EI 0 0 m 9 9 l S ~>", P)
image("rl-end-no-EI", G + b" /F /RL", b"\x04 EI 0\x80", P)
image("dct-end-no-EI", G + b" /F /DCT", b"\xff\xd8 EI 0 0 m 9 9 l S \xff\xd9", P)
image("fl-end-no-EI", FL, zlib_data(stored(IN), IN), P)
image("lzw-end-no-EI", LZ, lzw([256, A, B, 257]), P)
# LZW data whose codes' bits hold ` EI ` (97 97 4 169 72 97 97, nine bits
# each, after the clear code), its end with no `EI` after it: read on from
# its start, to that `EI`.
image("lzw-end-no-EI-in-data", LZ, lzw([256, 97, 97, 4, 169, 72, 97, 97, 257]), P)


class Msb:
    """Bits written most significant first, as JPEG and fax data are."""

    def __init__(self):
        self.bits = []

    def put(self, value, n):
        self.bits += [(value >> i) & 1 for i in reversed(range(n))]

    def code(self, text):
        self.bits += [int(c) for c in text]

    def bytes(self, pad=1, stuff=False):
        """The bits, the last byte filled with `pad`; with `stuff`, each
        0xFF followed by a 0, as a JPEG's coded data has it."""
        bits = self.bits + [pad] * (-len(self.bits) % 8)
        out = bytearray()
        for i in range(0, len(bits), 8):
            b = int("".join(map(str, bits[i:i + 8])), 2)
            out.append(b)
            if stuff and b == 0xFF:
                out.append(0)
        return bytes(out)


# JPEGs written by hand: each segment a marker and its length; quantizers
# all 1; two Huffman tables of 4-bit codes, one coding a DC difference's
# size, 0 to 11, as itself, the other an AC value's, a run of 0 and a size
# of 1 to 10 as the size, 0 the end of the block.
def segment(marker, body):
    return bytes([0xFF, marker]) + (len(body) + 2).to_bytes(2, "big") + body


QUANT = segment(0xDB, b"\x00" + b"\x01" * 64)


def huffman(table, symbols):
    counts = [0] * 16
    counts[3] = len(symbols)
    return segment(0xC4, bytes([table]) + bytes(counts) + bytes(symbols))


HUFFMAN = huffman(0x00, range(12)) + huffman(0x10, range(11))


def frame(marker, w, h, components):
    body = b"\x08" + h.to_bytes(2, "big") + w.to_bytes(2, "big") + bytes([len(components)])
    for c, sampling in components:
        body += bytes([c, sampling, 0])
    return segment(marker, body)


def scan(components, first=0, last=63):
    body = bytes([len(components)])
    for c in components:
        body += bytes([c, 0x00])
    return segment(0xDA, body + bytes([first, last, 0]))


def value(b, v):
    """A coefficient: its size as its code, then its bits."""
    size = abs(v).bit_length()
    b.put(size, 4)
    if size:
        b.put(v if v >= 0 else v + (1 << size) - 1, size)


def jpeg(w, h, blocks, components=((1, 0x11),), before=b""):
    """A baseline JPEG: `blocks` in the order its scan takes them, each a
    DC difference and the AC values that start it, then the end of the
    block; `before` the segments ahead of its tables."""
    b = Msb()
    for dc, acs in blocks:
        value(b, dc)
        for ac in acs:
            value(b, ac)
        b.put(0, 4)
    return (b"\xff\xd8" + before + QUANT + HUFFMAN + frame(0xC0, w, h, components)
            + scan([c for c, _ in components]) + b.bytes(stuff=True) + b"\xff\xd9")


def progressive(w, h, dcs, bands=((1, 63),), empty=()):
    """A progressive gray JPEG: a scan of the DC differences, then one of
    the AC values of each band, each block's band ended at once; the scans
    `empty` names (0 the DC scan's, 1 the first band's) with no coded
    data."""
    dc = Msb()
    for d in dcs:
        value(dc, d)
    ac = Msb()
    for _ in dcs:
        ac.put(0, 4)
    scans = [(0, 0, dc.bytes(stuff=True))] + [(a, b, ac.bytes(stuff=True)) for a, b in bands]
    return (b"\xff\xd8" + QUANT + HUFFMAN + frame(0xC2, w, h, [(1, 0x11)])
            + b"".join(scan([1], a, b) + (b"" if i in empty else coded)
                       for i, (a, b, coded) in enumerate(scans)) + b"\xff\xd9")


# DCT data: PDFium decodes every row of the JPEG and its data ends where
# libjpeg-turbo's reader then stands, past the end of image it reads as it
# fills its bits, or, with none after the coded data, at those bits; it
# refuses a JPEG whose header doesn't read, or one narrower than /W. A
# thumbnail in the metadata ends before the JPEG does; a comment segment
# holds ` EI `; a content of more than 512 bytes after a gray JPEG puts the
# decoder on its fast path. Then a JPEG cut short, with bytes before its
# start (its end counted from its start of image, so short by as many),
# damaged, with samples of 12 bits.
G8 = b"/W 8 /H 8 /BPC 8 /CS /G /F /DCT"
# Two spaces at the content's end, which chromium/7881 and 8076, writing an
# end of image over the content's own last two bytes where the JPEG's data
# runs to its end, write over, the last path left whole.
DT = T + b"  "


def jpeg_image(name, d, data, tail=DT):
    image(name, d, data, tail)


J8 = jpeg(8, 8, [(5, [3, -2, 1])])
LONG_TAIL = b" " + b"q Q " * 200
jpeg_image("dct-base", G8, J8)
jpeg_image("dct-base-then-EI", G8, J8, T[1:] + b"  ")
jpeg_image("dct-base-long", G8, J8, T + LONG_TAIL)
jpeg_image("dct-base-then-EI-long", G8, J8, T[1:] + LONG_TAIL)
jpeg_image("dct-dc-only", G8, jpeg(8, 8, [(0, [])]))
jpeg_image("dct-16", b"/W 16 /H 16 /BPC 8 /CS /G /F /DCT", jpeg(16, 16, [(i * 3 - 5, [i, -i, 2]) for i in range(4)]))
jpeg_image("dct-rgb", b"/W 8 /H 8 /BPC 8 /CS /RGB /F /DCT",
      jpeg(8, 8, [(4, [1]), (-3, []), (2, [2, 2])], components=((1, 0x11), (2, 0x11), (3, 0x11))))
jpeg_image("dct-subsampled", b"/W 16 /H 16 /BPC 8 /CS /RGB /F /DCT",
      jpeg(16, 16, [(1, [1])] + [(0, [])] * 3 + [(2, []), (-1, [])], components=((1, 0x22), (2, 0x11), (3, 0x11))))
jpeg_image("dct-progressive", G8, progressive(8, 8, [7]))
jpeg_image("dct-progressive-16", b"/W 16 /H 16 /BPC 8 /CS /G /F /DCT", progressive(16, 16, [7, -1, 2, 0]))
# A scan with no coded data, which libjpeg-turbo reads on through to the
# end of image; the next scan's header, of the band from 37 (0x25), holds a
# `%`.
jpeg_image("dct-progressive-empty-dc", G8, progressive(8, 8, [7], empty=[0]))
jpeg_image("dct-progressive-empty-scan", G8, progressive(8, 8, [7], ((1, 36), (37, 63)), empty=[1]))
COMMENT = segment(0xFE, b" EI 1 1 m 2 2 l S ")
THUMBNAIL = segment(0xE1, b"Exif\x00\x00" + jpeg(8, 8, [(0, [])]))
jpeg_image("dct-thumbnail", G8, jpeg(8, 8, [(5, [3])], before=THUMBNAIL + COMMENT))
jpeg_image("dct-comment-EI", G8, jpeg(8, 8, [(5, [3])], before=COMMENT))
jpeg_image("dct-then-bytes", G8, J8 + b" xyz")
jpeg_image("dct-narrower-than-W", b"/W 16 /H 8 /BPC 8 /CS /G /F /DCT", J8)
jpeg_image("dct-wider-than-W", b"/W 4 /H 8 /BPC 8 /CS /G /F /DCT", J8)
jpeg_image("dct-taller-than-H", b"/W 8 /H 4 /BPC 8 /CS /G /F /DCT", J8)
jpeg_image("dct-W-0", b"/W 0 /H 8 /BPC 8 /CS /G /F /DCT", J8)
jpeg_image("dct-cut", G8, J8[:-6])
# Cut short at the content's end, where PDFium reads its last two bytes as
# an end of image, which no `EI` comes after: read on from the data's
# start, to the comment's `EI`.
jpeg_image("dct-cut-at-content-end", G8, jpeg(8, 8, [(5, [3, -2, 1])], before=COMMENT)[:-3], b"")
# The same, cut in its second block, the content's last two bytes no codes
# of its tables: PDFium's end of image in their place ends the coded data.
J16 = jpeg(16, 16, [(i * 3 - 5, [i, -i, 2]) for i in range(4)], before=COMMENT)
cut = J16.index(b"\xff\xda") + 10 + 2
jpeg_image("dct-cut-in-codes-at-content-end", b"/W 16 /H 16 /BPC 8 /CS /G /F /DCT", J16[:cut] + b"\xfe\xfe", b"")
# The same, its last two bytes a marker's, a table's (FF C4) and a scan's
# (FF DA), which a decoder reading them reads on into as a segment's: PDFium
# reads an end of image in their place, and so must the decoder here.
for name, last in [("ffc4", b"\xff\xc4"), ("ffda", b"\xff\xda")]:
    jpeg_image("dct-cut-in-codes-at-content-end-" + name, b"/W 16 /H 16 /BPC 8 /CS /G /F /DCT", J16[:cut] + last, b"")
jpeg_image("dct-cut-header", G8, J8[:40])
jpeg_image("dct-no-eoi", G8, J8[:-2])
jpeg_image("dct-no-eoi-long", G8, J8[:-2], T + LONG_TAIL)
jpeg_image("dct-no-soi", G8, J8[2:])
jpeg_image("dct-soi-only", G8, b"\xff\xd8")
jpeg_image("dct-bytes-first", G8, b"ab" + J8)
jpeg_image("dct-bytes-first-EI", G8, b"a" * 170 + b" EI 1 1 m 2 2 l S " + J8)
damaged = bytearray(J8)
damaged[-4] ^= 0xF0
jpeg_image("dct-damaged", G8, bytes(damaged))
jpeg_image("dct-12-bit", G8, J8.replace(b"\xff\xc0\x00\x0b\x08", b"\xff\xc0\x00\x0b\x0c"))
jpeg_image("dct-array-params", b"/W 8 /H 8 /BPC 8 /CS /G /F [/DCT] /DP [<< /ColorTransform 0 >>]", J8)
# A JPEG of one scan is read as its rows need it: the reader stops past the
# marker it runs into after the coded data, restarts read past, so a
# segment between the scan and the end of image, holding `(`, puts the end
# inside it, the walk from there reading `( ... )` as one string. Where the
# reader holds the last row's bits before it runs into the marker, it
# stops short of it, `EI` straight after the end of image then no word of
# its own. A JPEG of several scans, progressive or one component a scan,
# is read whole first, to its end of image.
CT = b" EI 0 0 m 9 9 l S ) EI" + P + b"  "
OPEN = segment(0xFE, b"(")


def then_eoi(jpeg, segments):
    """`jpeg` with `segments` before its end of image."""
    return jpeg[:-2] + segments + b"\xff\xd9"


def restarted(w, blocks):
    """A baseline gray JPEG with a restart after each of its blocks but the
    last, each block's bits filled out to a byte."""
    coded = b""
    for i, (dc, acs) in enumerate(blocks):
        b = Msb()
        value(b, dc)
        for ac in acs:
            value(b, ac)
        b.put(0, 4)
        coded += b.bytes(stuff=True)
        if i < len(blocks) - 1:
            coded += bytes([0xFF, 0xD0 + i % 8])
    interval = b"\xff\xdd\x00\x04\x00\x01"
    return (b"\xff\xd8" + QUANT + HUFFMAN + interval + frame(0xC0, w, w, [(1, 0x11)])
            + scan([1]) + coded + b"\xff\xd9")


def scan_each(blocks, between=b""):
    """An 8 by 8 baseline JPEG of three components, a scan for each, its
    block in it, `between` after each scan but the last."""
    scans = []
    for c, (dc, acs) in zip((1, 2, 3), blocks):
        b = Msb()
        value(b, dc)
        for ac in acs:
            value(b, ac)
        b.put(0, 4)
        scans.append(scan([c]) + b.bytes(stuff=True))
    return (b"\xff\xd8" + QUANT + HUFFMAN + frame(0xC0, 8, 8, [(1, 0x11), (2, 0x11), (3, 0x11)])
            + between.join(scans) + b"\xff\xd9")


jpeg_image("dct-comment-after-scan", G8, then_eoi(J8, OPEN), CT)
jpeg_image("dct-app-after-scan", G8, then_eoi(J8, segment(0xE5, b"(")), CT)
jpeg_image("dct-thumbnail-comment-after-scan", G8,
           then_eoi(jpeg(8, 8, [(5, [3, -2, 1])], before=THUMBNAIL), OPEN), CT)
R16 = [(-50, []), (30, [18, 8, 2, 12]), (-13, []), (26, [-7, 14, -7, -15, -13])]
jpeg_image("dct-restarts-comment-after-scan", b"/W 16 /H 16 /BPC 8 /CS /G /F /DCT",
           then_eoi(restarted(16, R16), OPEN), CT)
RGB3 = [(4, [1]), (-3, []), (2, [2, 2])]
jpeg_image("dct-scan-each-comment-after", b"/W 8 /H 8 /BPC 8 /CS /RGB /F /DCT",
           then_eoi(scan_each(RGB3, OPEN), OPEN), CT)
jpeg_image("dct-progressive-comment-after", G8, then_eoi(progressive(8, 8, [7]), OPEN), CT)
jpeg_image("dct-EI-after-eoi", b"/W 16 /H 16 /BPC 8 /CS /G /F /DCT", jpeg(16, 16, R16), T[1:] + b"  ")
jpeg_image("dct-EI-after-eoi-unread", G8, jpeg(8, 8, [(31, [13, -9, 3, -13, 15, 13])]), T[1:] + b"  ")

# Fax data written by hand: the run codes these cases take, and the 2-D
# coding's modes, as ITU-T T.4 and T.6 code them.
WHITE_RUN = {0: "00110101", 1: "000111", 2: "0111", 3: "1000", 4: "1011", 5: "1100", 6: "1110",
             7: "1111", 8: "10011", 9: "10100", 10: "00111", 11: "01000", 12: "001000",
             13: "000011", 14: "110100", 15: "110101", 16: "101010", 17: "101011", 23: "0000100"}
BLACK_RUN = {0: "0000110111", 1: "010", 2: "11", 3: "10", 4: "011", 5: "0011", 6: "0010",
             7: "00011", 8: "000101", 9: "000100", 10: "0000100", 11: "0000101", 12: "0000111",
             13: "00000100"}
MAKEUP_64 = ("11011", "0000001111")
EOL = "000000000001"
VERTICAL = {0: "1", 1: "011", -1: "010", 2: "000011", -2: "000010", 3: "0000011", -3: "0000010"}


def runs(row):
    """A row's runs of one color, white first, so the first may be none."""
    out, color, n = [], 0, 0
    for p in row:
        if p != color:
            out.append(n)
            color, n = p, 0
        n += 1
    return out + [n]


def run(b, color, n):
    if n >= 64:
        b.code(MAKEUP_64[color])
        n -= 64
    b.code((BLACK_RUN if color else WHITE_RUN)[n])


def one_d(b, row):
    for i, n in enumerate(runs(row)):
        run(b, i % 2, n)


def two_d(b, row, ref):
    """`row` coded against `ref`, the row before it."""
    width = len(row)

    def changes(line):
        return [i for i in range(width) if line[i] != (line[i - 1] if i else 0)]

    def after(found, at):
        return next((x for x in found if x > at), width)

    here, there = changes(row), changes(ref)
    a0, color = -1, 0
    while a0 < width:
        a1 = after(here, a0)
        a2 = after(here, a1)
        b1 = next((x for x in there if x > a0 and ref[x] != color), width)
        b2 = after(there, b1)
        if b2 < a1:
            b.code("0001")
            a0 = b2
        elif abs(a1 - b1) <= 3:
            b.code(VERTICAL[a1 - b1])
            a0, color = a1, 1 - color
        else:
            b.code("001")
            run(b, color, a1 - max(a0, 0))
            run(b, 1 - color, a2 - a1)
            a0 = a2


def group_4(rows, end=False):
    b = Msb()
    ref = [0] * len(rows[0])
    for row in rows:
        two_d(b, row, ref)
        ref = row
    if end:
        b.code(EOL + EOL)
    return b.bytes(pad=0)


def group_3(rows, k=0, eol=False, align=False, rtc=False):
    """Group 3 coding: each row's runs, or with `k`, every k-th row so and
    the rest against the row before, each tagged; `eol` an end of line
    before each row, `align` each row padded to a byte, `rtc` six ends of
    line after the last."""
    b = Msb()
    ref = [0] * len(rows[0])
    for i, row in enumerate(rows):
        if eol:
            b.code(EOL)
        if k > 0:
            b.code("1" if i % k == 0 else "0")
        if k > 0 and i % k:
            two_d(b, row, ref)
        else:
            one_d(b, row)
        ref = row
        if align:
            b.bits += [0] * (-len(b.bits) % 8)
    if rtc:
        b.code(EOL * 6)
    return b.bytes(pad=0)


# CCITTFax data: PDFium decodes as many rows as /Rows, or /H, says, each
# /Columns wide, 1728 where it isn't written, /W where it's 0, by /K's
# coding, and its data ends at the byte its decoder then stands in; it
# reads no end of block, and it refuses a width or a height of none or
# past 65,535. A row whose codes hold an `EI` between white-space.
ROWS = [[0] * 16, [0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0],
        [0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0], [1] * 4 + [0] * 12]
F16 = b"/W 16 /H 4 /BPC 1 /CS /G /F /CCF /DP "
G4 = F16 + b"<< /K -1 /Columns 16 >>"
image("ccf-g4", G4, group_4(ROWS))
image("ccf-g4-then-EI", G4, group_4(ROWS), T[1:])
image("ccf-g4-end-of-block", G4, group_4(ROWS, end=True))
image("ccf-g4-white", b"/W 16 /H 2 /BPC 1 /CS /G /F /CCF /DP << /K -1 /Columns 16 >>", group_4([[0] * 16] * 2))
image("ccf-g4-rows-1", F16 + b"<< /K -1 /Columns 16 /Rows 1 >>", group_4(ROWS))
image("ccf-g4-H-2", b"/W 16 /H 2 /BPC 1 /CS /G /F /CCF /DP << /K -1 /Columns 16 >>", group_4(ROWS))
image("ccf-g4-rows-past-65535", F16 + b"<< /K -1 /Columns 16 /Rows 70000 >>", group_4(ROWS))
image("ccf-g4-columns-past-65535", F16 + b"<< /K -1 /Columns 70000 >>", group_4(ROWS))
image("ccf-g4-columns-0", F16 + b"<< /K -1 /Columns 0 >>", group_4(ROWS))
image("ccf-g4-no-columns", F16 + b"<< /K -1 >>", group_4(ROWS))
image("ccf-g4-cut", G4, group_4(ROWS)[:2])
damaged = bytearray(group_4(ROWS))
damaged[1] = 0x00
image("ccf-g4-damaged", G4, bytes(damaged))
image("ccf-g4-array-params", b"/W 16 /H 4 /BPC 1 /CS /G /F [/CCF] /DP [<< /K -1 /Columns 16 >>]", group_4(ROWS))
G3 = F16 + b"<< /Columns 16 >>"
image("ccf-g3", G3, group_3(ROWS))
image("ccf-g3-eol", G3, group_3(ROWS, eol=True))
image("ccf-g3-end-of-line", F16 + b"<< /Columns 16 /EndOfLine true >>", group_3(ROWS, eol=True))
image("ccf-g3-rtc", G3, group_3(ROWS, eol=True, rtc=True))
image("ccf-g3-byte-align", F16 + b"<< /Columns 16 /EncodedByteAlign true >>", group_3(ROWS, align=True))
image("ccf-g3-2d", F16 + b"<< /K 2 /Columns 16 >>", group_3(ROWS, k=2, eol=True))
image("ccf-g3-2d-no-eol", F16 + b"<< /K 2 /Columns 16 >>", group_3(ROWS, k=2))
image("ccf-no-params", b"/W 16 /H 4 /BPC 1 /CS /G /F /CCF", group_3(ROWS))
image("ccf-g3-no-columns", F16 + b"<< /K 0 >>", group_3(ROWS))
TAB_EI = [0] * 23 + [1] * 3 + [0] * 3 + [1] * 3 + [0] * 9 + [1] * 3 + [0] * 11 + [1] * 13
image("ccf-g3-codes-EI", b"/W 68 /H 2 /BPC 1 /CS /G /F /CCF /DP << /Columns 68 /EncodedByteAlign true >>",
      group_3([[0] * 68, TAB_EI], align=True))

# What taking out a range that draws and sets nothing would change, each
# before and after. A `BI` with a keyword where its dictionary's key should
# be begins no image, its would-be dictionary content: a marked range taken
# out of it makes it begin one. An ASCIIHex image whose first `>` is in a
# comment after it, with no `EI` after that, is no stream, its data read on
# to its first `EI`, the rectangle after it drawn: the range holding the
# comment taken out, a `>` after the rectangle is its end, and the image
# runs on over the rectangle to the `EI` after that.
case("splice-lone-bi", b"BI /A BMC EMC /W 1 /H 1 /CS /G /BPC 8 ID x EI Q")
case("splice-lone-bi-taken", b"BI  /W 1 /H 1 /CS /G /BPC 8 ID x EI Q")
case("splice-ahx-reach", b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 41 EI q % > (\n Q 0 0 9 9 re f > EI")
case("splice-ahx-reach-taken", b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 41 EI  0 0 9 9 re f > EI")

# Two lists, each written again alone leaving the content read as it was,
# written again together: an ASCII85 image whose data's end, `~>`, is past
# both, each list holding a `~` and a comment, is no stream, read to its
# first `EI`, the rectangle after it drawn; with one list written again,
# the other's `~` ends its data, its comment hiding the rest; with both,
# the `~>` past the rectangle is its end, and the image runs on over it.
A85_LISTS = b"BI /W 1 /H 1 /CS /G /BPC 8 /F /A85 ID 9jqo^ EI /A <<%s>> BDC EMC /C <<%s>> BDC EMC 0 0 9 9 re f ~> EI"
case("splice-a85-two-lists", A85_LISTS % (b"/Tool_Gen (~%)", b"/Tool_Gen (~%)"))
case("splice-a85-one-list-made", A85_LISTS % (b"", b"/Tool_Gen (~%)"))
case("splice-a85-two-lists-made", A85_LISTS % (b"", b""))


# A page's /Contents (the `contents` kind): its value as written, and the
# objects from 4 on, each written between its `n 0 obj` and `endobj`. A
# path begun in one stream (BEGUN) and stroked in another (STROKED) is drawn
# only where both are joined, in that order; what's drawn says which streams
# PDFium joined.
def contents(name, value, *objects):
    CASES.append((name, "contents", {
        "contents": value,
        "objects": b"".join(o + b"\nendobj\n" for o in objects),
    }))


def stream(data):
    return b"<< /Length %d >>\nstream\n" % len(data) + data + b"\nendstream"


BEGUN = stream(b"0 0 m 9 9 l")
STROKED = stream(b"S")
WHOLE = stream(b"5 5 m 7 7 l S")
OTHER = stream(b"1 1 m 3 3 l S")

# A stream, and an array of them, in order, one again.
contents("contents-stream", b"4 0 R", WHOLE)
contents("contents-array", b"[4 0 R 5 0 R]", BEGUN, STROKED)
contents("contents-array-one", b"[4 0 R]", WHOLE)
contents("contents-array-order", b"[5 0 R 4 0 R]", WHOLE, OTHER)
contents("contents-array-again", b"[4 0 R 4 0 R]", WHOLE)
# An element that isn't a stream, between two that are: no stream of its own.
contents("contents-element-dict", b"[4 0 R 6 0 R 5 0 R]", BEGUN, STROKED, b"<< /A 1 >>")
contents("contents-element-nothing", b"[4 0 R 9 0 R 5 0 R]", BEGUN, STROKED)
contents("contents-element-null", b"[4 0 R null 5 0 R]", BEGUN, STROKED)
contents("contents-element-number", b"[4 0 R 7 5 0 R]", BEGUN, STROKED)
contents("contents-element-in-place", b"[4 0 R << /Length 0 >> 5 0 R]", BEGUN, STROKED)
contents("contents-element-null-object", b"[4 0 R 6 0 R 5 0 R]", BEGUN, STROKED, b"null")
# An array in the array, its stream not joined.
contents("contents-nested-array", b"[4 0 R [6 0 R] 5 0 R]", BEGUN, STROKED, WHOLE)
contents("contents-nested-array-alone", b"[[4 0 R 5 0 R]]", BEGUN, STROKED)
contents("contents-element-array-by-reference", b"[4 0 R 6 0 R 5 0 R]", BEGUN, STROKED, b"[7 0 R]", WHOLE)
# The array by reference.
contents("contents-array-by-reference", b"6 0 R", BEGUN, STROKED, b"[4 0 R 5 0 R]")
# Nothing to join: no content.
contents("contents-empty-array", b"[]", WHOLE)
contents("contents-empty-array-by-reference", b"5 0 R", WHOLE, b"[]")
contents("contents-dict", b"5 0 R", WHOLE, b"<< /A 1 >>")
contents("contents-nothing", b"9 0 R", WHOLE)
contents("contents-null", b"null", WHOLE)
contents("contents-number", b"4", WHOLE)
# A reference to a reference: to a stream, to an array, and as an element.
contents("contents-chain", b"5 0 R", WHOLE, b"4 0 R")
contents("contents-chain-array", b"6 0 R", BEGUN, STROKED, b"7 0 R", b"[4 0 R 5 0 R]")
contents("contents-element-chain", b"[6 0 R 5 0 R]", BEGUN, STROKED, b"4 0 R")


def cases():
    return CASES
