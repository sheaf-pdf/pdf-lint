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


def cases():
    return CASES
