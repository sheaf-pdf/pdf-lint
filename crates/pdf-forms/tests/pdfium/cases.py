"""pdf-forms's PDFium oracle cases: `forms`, how PDFium reads the field tree,
and `forms-ap`, the appearance PDFium makes for a text field that has none.

Written using Python's standard library alone; `oracle/pdfium.py cases --write`
materializes `cases/*.pdf` and `cases.tsv`, and `oracle/pdfium.py record`
records `chromium-8152.txt`. The embedded TrueType cases read pdf-fonts' own
program, `../../../pdf-fonts/tests/pdfium/programs/truetype` from here (which
pdf-fonts' `the_programs_are_the_builders` holds to its builder)."""

import os
import re

ROWS = []

WIDGET = b"/Type /Annot /Subtype /Widget /P 3 0 R"


def rect(row):
    return b"/Rect [10 %d 110 %d]" % (10 + 30 * row, 30 + 30 * row)


def build_doc(acroform, annots, objs):
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R" + (b" /AcroForm " + acroform if acroform else b"") + b" >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots " + annots + b" >>",
    }
    objects.update(objs)

    out = bytearray(b"%PDF-1.7\n")
    offsets = {}
    for number in sorted(objects):
        offsets[number] = len(out)
        out += b"%d 0 obj\n" % number + objects[number] + b"\nendobj\n"
    xref = len(out)
    size = max(objects) + 1
    out += b"xref\n0 %d\n0000000000 65535 f \n" % size
    for number in range(1, size):
        if number in offsets:
            out += b"%010d 00000 n \n" % offsets[number]
        else:
            out += b"0000000000 65535 f \n"
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (size, xref)
    return bytes(out)


def add(name, fields, annots, objs, acroform_extra=b""):
    """A file whose form lists `fields` and whose one page lists `annots`."""
    acroform = b"<< /Fields " + fields + (b" " + acroform_extra if acroform_extra else b"") + b" >>"
    ROWS.append((name, "forms", {"pdf": build_doc(acroform, annots, objs)}))


def merged(name, rest, row=0):
    return b"<< /T (" + name + b") " + rest + b" " + WIDGET + b" " + rect(row) + b" >>"


def widget(parent, row=0, rest=b""):
    return b"<< /Parent %d 0 R " % parent + rest + (b" " if rest else b"") + WIDGET + b" " + rect(row) + b" >>"


def cases():
    del ROWS[:]

    # What a field inherits: its type, flags, value and maximum length come
    # down to a kid that is a field and to a kid that is only a widget.
    add(
        "forms-inherit",
        b"[5 0 R]",
        b"[6 0 R 7 0 R]",
        {
            5: b"<< /T (top) /FT /Tx /Ff 4096 /V (from top) /MaxLen 7 /Kids [6 0 R 7 0 R] >>",
            6: b"<< /T (kid) /Parent 5 0 R " + WIDGET + b" " + rect(0) + b" >>",
            7: widget(5, 1),
        },
    )
    add(
        "forms-own-over-inherited",
        b"[5 0 R]",
        b"[6 0 R 7 0 R]",
        {
            5: b"<< /T (top) /FT /Tx /Ff 4096 /V (from top) /Kids [6 0 R 7 0 R] >>",
            6: b"<< /T (choice) /Parent 5 0 R /FT /Ch /Ff 131072 /V (mine) " + WIDGET + b" " + rect(0) + b" >>",
            7: b"<< /T (text) /Parent 5 0 R /Ff 0 /V (own) " + WIDGET + b" " + rect(1) + b" >>",
        },
    )
    add(
        "forms-three-deep",
        b"[5 0 R]",
        b"[8 0 R]",
        {
            5: b"<< /T (a) /FT /Tx /Ff 8192 /Kids [6 0 R] >>",
            6: b"<< /T (b) /Parent 5 0 R /V (deep) /Kids [7 0 R] >>",
            7: b"<< /T (c) /Parent 6 0 R /Kids [8 0 R] >>",
            8: widget(7),
        },
    )

    # Kinds, by the type and the flags.
    add(
        "forms-button-kinds",
        b"[5 0 R 6 0 R 7 0 R 8 0 R]",
        b"[5 0 R 6 0 R 7 0 R 8 0 R]",
        {
            5: merged(b"check", b"/FT /Btn", 0),
            6: merged(b"radio", b"/FT /Btn /Ff 32768", 1),
            7: merged(b"push", b"/FT /Btn /Ff 65536", 2),
            8: merged(b"both", b"/FT /Btn /Ff 98304", 3),
        },
    )
    add(
        "forms-choice-kinds",
        b"[5 0 R 6 0 R 7 0 R]",
        b"[5 0 R 6 0 R 7 0 R]",
        {
            5: merged(b"list", b"/FT /Ch /Opt [(a) (b)]", 0),
            6: merged(b"combo", b"/FT /Ch /Ff 131072 /Opt [(a) (b)]", 1),
            7: merged(b"edit", b"/FT /Ch /Ff 393216 /Opt [(a) (b)]", 2),
        },
    )
    add(
        "forms-signature",
        b"[5 0 R]",
        b"[5 0 R]",
        {5: merged(b"sig", b"/FT /Sig", 0)},
    )
    add(
        "forms-type-missing-or-unknown",
        b"[5 0 R 6 0 R]",
        b"[5 0 R 6 0 R]",
        {
            5: merged(b"untyped", b"/V (v)", 0),
            6: merged(b"unknown", b"/FT /Zz /V (v)", 1),
        },
    )

    # Names.
    add(
        "forms-nameless-middle",
        b"[5 0 R]",
        b"[7 0 R]",
        {
            5: b"<< /T (a) /FT /Tx /Kids [6 0 R] >>",
            6: b"<< /Parent 5 0 R /Kids [7 0 R] >>",
            7: b"<< /T (c) /Parent 6 0 R " + WIDGET + b" " + rect(0) + b" >>",
        },
    )
    add(
        "forms-name-with-a-period",
        b"[5 0 R]",
        b"[6 0 R]",
        {
            5: b"<< /T (a.b) /FT /Tx /Kids [6 0 R] >>",
            6: b"<< /T (c) /Parent 5 0 R " + WIDGET + b" " + rect(0) + b" >>",
        },
    )
    add(
        "forms-name-and-value-in-utf16",
        b"[5 0 R]",
        b"[5 0 R]",
        {5: b"<< /T <FEFF0141> /FT /Tx /V <FEFF00D6006C> " + WIDGET + b" " + rect(0) + b" >>"},
    )
    add(
        "forms-nameless-at-the-top",
        b"[5 0 R]",
        b"[5 0 R]",
        {5: b"<< /FT /Tx /V (v) " + WIDGET + b" " + rect(0) + b" >>"},
    )

    # Trees that aren't trees, and odd ways to write one.
    add(
        "forms-kids-loop",
        b"[5 0 R]",
        b"[7 0 R]",
        {
            5: b"<< /T (a) /FT /Tx /Kids [6 0 R] >>",
            6: b"<< /T (b) /Parent 5 0 R /Kids [5 0 R 7 0 R] >>",
            7: widget(6),
        },
    )
    add(
        "forms-field-listed-twice",
        b"[5 0 R 5 0 R]",
        b"[5 0 R]",
        {5: merged(b"twice", b"/FT /Tx /V (v)", 0)},
    )
    add(
        "forms-direct-field",
        b"[<< /T (direct) /FT /Tx /V (v) /Kids [5 0 R] >>]",
        b"[5 0 R]",
        {5: b"<< " + WIDGET + b" " + rect(0) + b" >>"},
    )
    add(
        "forms-fields-by-reference",
        b"6 0 R",
        b"[5 0 R]",
        {
            5: merged(b"f", b"/FT /Tx /V 7 0 R", 0),
            6: b"[8 0 R]",
            7: b"(by reference)",
            8: b"5 0 R",
        },
    )
    add(
        "forms-real-flags",
        b"[5 0 R]",
        b"[5 0 R]",
        {5: merged(b"real", b"/FT /Tx /Ff 4096.0 /V (v)", 0)},
    )
    add(
        "forms-value-not-a-string",
        b"[5 0 R 6 0 R]",
        b"[5 0 R 6 0 R]",
        {
            5: merged(b"name", b"/FT /Tx /V /Nm", 0),
            6: merged(b"number", b"/FT /Tx /V 12", 1),
        },
    )

    # Annotations that aren't a tree's widgets.
    add(
        "forms-widget-outside-the-tree",
        b"[5 0 R]",
        b"[5 0 R 6 0 R]",
        {
            5: merged(b"inside", b"/FT /Tx /V (v)", 0),
            6: merged(b"outside", b"/FT /Tx /V (o)", 1),
        },
    )
    add(
        "forms-not-a-widget",
        b"[5 0 R]",
        b"[6 0 R 5 0 R]",
        {
            5: merged(b"f", b"/FT /Tx /V (v)", 0),
            6: b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /Contents (note) >>",
        },
    )
    ROWS.append((
        "forms-no-form",
        "forms",
        {"pdf": build_doc(None, b"[5 0 R]", {5: merged(b"lone", b"/FT /Tx /V (v)", 0)})},
    ))

    ap_cases()
    return list(ROWS)


# `forms-ap`: a text field with a value and no /AP, whose appearance PDFium
# makes under the form-fill environment. A case's objects: 1 the catalog with
# the form, 2 the pages, 3 the page, 5 on its widgets (each a field of its
# own unless the case says), 50 on what else it names, 100 on the fonts of
# its /DR. Each widget is 100 by 20 at (10, 10) unless it says.

BOX = b"[10 10 110 30]"


def std(base, encoding=b" /Encoding /WinAnsiEncoding"):
    return b"<< /Type /Font /Subtype /Type1 /BaseFont /" + base + encoding + b" >>"


def stream(dictionary, data):
    return b"<< " + dictionary + b"/Length %d >>\nstream\n" % len(data) + data + b"\nendstream"


def truetype_program():
    here = os.path.dirname(os.path.abspath(__file__))
    path = os.path.join(here, "..", "..", "..", "pdf-fonts", "tests", "pdfium", "programs", "truetype")
    with open(path, "rb") as f:
        return f.read()


def truetype(ascent, descent):
    """pdf-fonts' TrueType program (A, B, C, g and the space), embedded under
    WinAnsi with its own advances as /Widths and the descriptor's heights
    given: (font, {number: object})."""
    def number(base):
        widths = [b"500"] * 72
        for code, width in ((32, b"250"), (65, b"600"), (66, b"450"), (67, b"700"), (103, b"500")):
            widths[code - 32] = width
        return (b"<< /Type /Font /Subtype /TrueType /BaseFont /Fix /FirstChar 32 /LastChar 103 /Widths ["
                + b" ".join(widths) + b"] /Encoding /WinAnsiEncoding /FontDescriptor %d 0 R >>" % (base + 1),
                {base + 1: b"<< /Type /FontDescriptor /FontName /Fix /Flags 32 /FontBBox [0 -200 700 800]"
                           b" /ItalicAngle 0 /Ascent %d /Descent %d /CapHeight 700 /StemV 80 /FontFile2 %d 0 R >>"
                           % (ascent, descent, base + 2),
                 base + 2: stream(b"", truetype_program())})
    return number


def described(ascent, descent):
    """A font that isn't one of the standard 14 and isn't embedded: its
    /Widths and its descriptor's heights are all PDFium measures it by."""
    def number(base):
        return (b"<< /Type /Font /Subtype /Type1 /BaseFont /Foo /FirstChar 32 /LastChar 126 /Widths ["
                + b" ".join([b"500"] * 95) + b"] /Encoding /WinAnsiEncoding /FontDescriptor %d 0 R >>" % (base + 1),
                {base + 1: b"<< /Type /FontDescriptor /FontName /Foo /Flags 32 /FontBBox [0 -200 1000 800]"
                           b" /ItalicAngle 0 /Ascent %d /Descent %d /CapHeight 700 /StemV 80 >>" % (ascent, descent)})
    return number


# The fonts a case's /DR may hold, by the name a /DA gives, each its object
# number and its dictionary (or what makes it and the objects it names).
FONTS = {
    b"Helv": (100, std(b"Helvetica")),
    b"HeBo": (101, std(b"Helvetica-Bold")),
    b"HeOb": (102, std(b"Helvetica-Oblique")),
    b"HeBO": (103, std(b"Helvetica-BoldOblique")),
    b"Cour": (104, std(b"Courier")),
    b"CoBo": (105, std(b"Courier-Bold")),
    b"CoOb": (106, std(b"Courier-Oblique")),
    b"CoBO": (107, std(b"Courier-BoldOblique")),
    b"TiRo": (108, std(b"Times-Roman")),
    b"TiBo": (109, std(b"Times-Bold")),
    b"TiIt": (110, std(b"Times-Italic")),
    b"TiBI": (111, std(b"Times-BoldItalic")),
    b"Symb": (112, std(b"Symbol", b"")),
    b"ZaDb": (113, std(b"ZapfDingbats", b"")),
    b"HeMac": (114, std(b"Helvetica", b" /Encoding /MacRomanEncoding")),
    # The layout forms give /Helv: a built-in encoding with PDFDocEncoding's
    # punctuation laid over it.
    b"HeDif": (115, std(b"Helvetica", b" /Encoding << /Type /Encoding /Differences [128 /bullet /dagger"
                                     b" /daggerdbl /ellipsis /emdash /endash 141 /quotedblleft /quotedblright"
                                     b" 160 /Euro] >>")),
    b"Desc": (116, described(800, -200)),
    b"DeLo": (119, described(600, -150)),
    b"TT": (122, truetype(800, -200)),
    b"TTZero": (126, truetype(0, 0)),
}


def lit(text):
    """A literal string of `text` as written, its escapes its own."""
    return b"(" + text + b")"


def utf16(text):
    return b"<FEFF" + text.encode("utf-16-be").hex().upper().encode("ascii") + b">"


def tx(rest=b"", value=b"Hello", da=b"/Helv 0 Tf 0 g", rect=BOX):
    """A text field's merged widget: `value` its /V as written (a literal of
    it where it is plain bytes, none where None), `da` its /DA (none where
    None), `rest` its other entries."""
    entries = b"/FT /Tx"
    if value is not None:
        entries += b" /V " + (value if value[:1] in b"<[/" or value.endswith(b" R") else lit(value))
    if da is not None:
        entries += b" /DA (" + da + b")"
    if rest:
        entries += b" " + rest
    return entries + b" " + WIDGET + b" /Rect " + rect


def ap(name, widgets, fonts=None, form=b"", objs=None, fields=None):
    """A `forms-ap` case: each of `widgets` (a field's entries, or a whole
    object where it begins `<<`) an object from 5 on, all the page's
    annotations; the form's /Fields `fields`, else every widget; its /DR the
    `fonts` named, else every font a /DA in the case names; `form` the form's
    other entries."""
    objects = {}
    for i, widget in enumerate(widgets):
        objects[5 + i] = widget if widget.startswith(b"<<") else b"<< /T (f%d) " % i + widget + b" >>"
    assert len(widgets) < 45 and all(50 <= n < 100 for n in objs or {}), name
    objects.update(objs or {})
    if fonts is None:
        named = set()
        for body in list(objects.values()) + [form]:
            named.update(re.findall(rb"/DA \(/(\w+)", body))
        fonts = [n for n in FONTS if n in named]
    entries = []
    for font in fonts:
        number, made = FONTS[font]
        if callable(made):
            made, more = made(number)
            objects.update(more)
        objects[number] = made
        entries.append(b"/" + font + b" %d 0 R" % number)
    refs = b" ".join(b"%d 0 R" % (5 + i) for i in range(len(widgets)))
    acroform = (b"<< /Fields [" + (fields if fields is not None else refs) + b"] /DR << /Font << "
                + b" ".join(entries) + b" >> >>" + (b" " + form if form else b"") + b" >>")
    ROWS.append((name, "forms-ap", {"pdf": build_doc(acroform, b"[" + refs + b"]", objects)}))


def bs(width, style=b"S", more=b""):
    return b"/BS << /W %s /S /%s%s >>" % (width, style, more)


LONG = b"Hello this is a very long line of text for a small box"
PARAGRAPH = (b"one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen"
             b" seventeen")
MULTILINE, PASSWORD, DO_NOT_SCROLL, COMB = b"/Ff 4096", b"/Ff 8192", b"/Ff 8388608", b"/Ff 16777216"


def ap_cases():
    # Alignment, at a fixed size and auto; /Q past 2.
    for q, word in ((0, "left"), (1, "centre"), (2, "right")):
        ap("ap-%s-auto" % word, [tx(b"/Q %d" % q)])
        ap("ap-%s-12" % word, [tx(b"/Q %d" % q, da=b"/Helv 12 Tf 0 g")])
    ap("ap-quadding-past-2", [tx(b"/Q 3", da=b"/Helv 12 Tf 0 g")])

    # Auto size: each of the table's entries by the box's height (no border,
    # so the text's box is the widget's: 10 less than its top), past its ends,
    # at a line height exactly (Helvetica's is 1.169 a point) and just under;
    # long text, so its width decides; a multiline field's sizes.
    def tall(top):
        return b"[10 10 410 %s]" % top

    tops = [b"13", b"18", b"20", b"21", b"21.68", b"21.69", b"22", b"25", b"27", b"32", b"34", b"40", b"46", b"51",
            b"57", b"63", b"69", b"75", b"81", b"92", b"104", b"116", b"127", b"139", b"151", b"162", b"179", b"210"]
    ap("ap-auto-sizes", [tx(bs(b"0"), value=b"Hi", rect=tall(top)) for top in tops])
    ap("ap-auto-sizes-multiline", [tx(MULTILINE + b" " + bs(b"0"), value=b"Hi", rect=tall(top))
                                   for top in (b"13", b"15", b"18", b"20", b"21", b"22", b"25", b"60", b"210")])
    ap("ap-auto-long", [tx(value=LONG)])
    ap("ap-auto-tall-long", [tx(value=b"Hello this is long", rect=b"[10 10 110 110]")])
    ap("ap-auto-empty-box", [tx(value=b"Hi", rect=b"[10 10 110 10]")])

    # Each face's heights: a single line at 100 points gives their sum, a
    # multiline field's first line the ascent. Symbol and ZapfDingbats show
    # ASCII: a character only their own encodings hold (alpha, a check mark)
    # PDFium draws in a font it adds, or not at all.
    for font, words, value in (
            (b"Helv", "helvetica", b"Hg"), (b"HeBo", "helvetica-bold", b"Hg"),
            (b"HeOb", "helvetica-oblique", b"Hg"), (b"HeBO", "helvetica-boldoblique", b"Hg"),
            (b"Cour", "courier", b"Hg"), (b"CoBo", "courier-bold", b"Hg"),
            (b"CoOb", "courier-oblique", b"Hg"), (b"CoBO", "courier-boldoblique", b"Hg"),
            (b"TiRo", "times-roman", b"Hg"), (b"TiBo", "times-bold", b"Hg"),
            (b"TiIt", "times-italic", b"Hg"), (b"TiBI", "times-bolditalic", b"Hg"),
            (b"Symb", "symbol", b"ab"), (b"ZaDb", "zapfdingbats", b"34")):
        da = b"/" + font + b" 100 Tf 0 g"
        ap("ap-face-" + words, [tx(bs(b"0"), value=value, da=da, rect=b"[10 10 410 310]"),
                                tx(MULTILINE + b" " + bs(b"0"), value=value, da=da, rect=b"[10 320 410 620]")])

    # Fonts that aren't standard, a /DA font /DR lacks, and /DA that names none.
    ap("ap-font-described", [tx(da=b"/Desc 10 Tf 0 g"), tx(MULTILINE, value=b"Hello\\rworld", da=b"/Desc 10 Tf 0 g",
                                                          rect=b"[10 40 110 90]")])
    ap("ap-font-described-low", [tx(da=b"/DeLo 10 Tf 0 g"), tx(MULTILINE, value=b"Hello\\rworld",
                                                              da=b"/DeLo 10 Tf 0 g", rect=b"[10 40 110 90]")])
    ap("ap-font-truetype", [tx(value=b"ABC gAB", da=b"/TT 12 Tf 0 g")])
    ap("ap-font-truetype-multiline", [tx(MULTILINE, value=b"ABC gAB CAB gg BAC ABC CCC BB", da=b"/TT 10 Tf 0 g",
                                         rect=b"[10 10 70 70]")])
    ap("ap-font-truetype-zero-heights", [tx(value=b"ABC gAB", da=b"/TTZero 12 Tf 0 g"),
                                         tx(MULTILINE, value=b"AB\\rgC", da=b"/TTZero 12 Tf 0 g",
                                            rect=b"[10 40 110 90]")])
    for name, missing in (("helv", b"Helv"), ("arial", b"Arial"), ("times", b"TiRo"), ("courier", b"Cour"),
                          ("bold", b"HelveticaLTStd-Bold")):
        ap("ap-font-missing-" + name, [tx(da=b"/" + missing + b" 10 Tf 0 g")], fonts=[b"Symb"])
    ap("ap-font-no-tf", [tx(da=b"0 0 1 rg")])
    ap("ap-font-no-da", [tx(da=None)], fonts=[b"Helv"])
    ap("ap-font-da-on-the-form", [tx(da=None)], form=b"/DA (/Cour 10 Tf 0 0 1 rg) /Q 2")
    # Characters past ASCII under each encoding, none of them General
    # Punctuation's (U+2000 to U+206F: dashes, curly quotes), which PDFium
    # draws in a font it adds from the machine's own whatever the font holds.
    ap("ap-font-mac-roman", [tx(value=utf16("caf\u00e9 \u00fc\u00df"), da=b"/HeMac 10 Tf 0 g")])
    ap("ap-font-differences", [tx(value=utf16("a\u20acb x"), da=b"/HeDif 10 Tf 0 g")])
    ap("ap-font-winansi", [tx(value=utf16("caf\u00e9 \u20ac \u00fc\u00df"), da=b"/Helv 10 Tf 0 g")])
    # /DA's other text-state operators, one a widget.
    ap("ap-da-text-state", [tx(b"/Q 2", da=b"/Helv 10 Tf " + op + b" 0 g") for op in
                            (b"2 Tc", b"3 Tw", b"50 Tz", b"14 TL", b"3 Tr", b"2 Ts")])

    # Borders: each style at widths 0 to 3 with a border and a background
    # color, then at 2 with one or the other or neither.
    for style, word in ((b"S", "solid"), (b"D", "dashed"), (b"B", "beveled"), (b"I", "inset"),
                        (b"U", "underline")):
        widgets = [tx(bs(w, style) + b" /MK << /BC [0 0 1] /BG [0.9] >>") for w in (b"0", b"1", b"2", b"3")]
        widgets += [tx(bs(b"2", style) + mk) for mk in (b" /MK << /BG [0.9] >>", b" /MK << /BC [0 0 1] >>", b"")]
        ap("ap-border-" + word, widgets)
    ap("ap-border-dash-array", [tx(bs(b"2", b"D", b" /D [3 2]") + b" /MK << /BC [1 0 0] >>")])
    ap("ap-border-array", [tx(b"/Border [0 0 3] /MK << /BC [0] >>"),
                           tx(b"/Border [0 0 2 [3 2]] /MK << /BC [0] >>"),
                           tx(b"/Border [0 0 1] " + bs(b"3", b"B") + b" /MK << /BC [0] >>")])
    ap("ap-border-odd", [tx(b"/BS << /W 1.5 >> /MK << /BC [0] >>"), tx(bs(b"2", b"X") + b" /MK << /BC [0] >>"),
                         tx(b"/BS << /S /D >> /MK << /BC [0] >>")])

    # Colors: /DA's in each space and none; /MK's.
    for name, color in (("gray", b"0.5 g"), ("rgb", b"1 0 0 rg"), ("cmyk", b"0.1 0.2 0.3 0.4 k"), ("none", b""),
                        ("navy", b"0 0 0.502 rg")):
        ap("ap-color-da-" + name, [tx(da=b"/Helv 10 Tf" + (b" " + color if color else b""))])
    ap("ap-color-mk", [tx(b"/MK << /BC [0.25] /BG [0.75] >>"), tx(b"/MK << /BC [0 1 0] /BG [1 1 0] >>"),
                       tx(b"/MK << /BC [0 0 0 1] /BG [0 0 0 0.2] >>"), tx(b"/MK << /BC [] /BG [] >>"),
                       tx(b"/MK << /BC [0 1] /BG [0.1 0.2] >>")])

    # Comb: /MaxLen cells in each alignment, fixed and auto; fewer, as many
    # and more characters than cells; the dividers in each border style; with
    # the flags it doesn't go with; with no /MaxLen; in a font not standard.
    for q, word in ((0, "left"), (1, "centre"), (2, "right")):
        ap("ap-comb-%s-12" % word, [tx(COMB + b" /MaxLen 5 /Q %d" % q, value=b"123", da=b"/Helv 12 Tf 0 g")])
        ap("ap-comb-%s-auto" % word, [tx(COMB + b" /MaxLen 5 /Q %d" % q, value=b"123")])
        ap("ap-comb-%s-over" % word, [tx(COMB + b" /MaxLen 5 /Q %d" % q, value=b"1234567", da=b"/Helv 12 Tf 0 g")])
    ap("ap-comb-counts", [tx(COMB + b" /MaxLen 5 /Q 1", value=v, da=b"/Helv 12 Tf 0 g")
                          for v in (b"1", b"12", b"12345")])
    for style, word in ((b"S", "solid"), (b"D", "dashed"), (b"B", "beveled"), (b"I", "inset"),
                        (b"U", "underline")):
        ap("ap-comb-border-" + word, [tx(COMB + b" /MaxLen 5 " + bs(w, style) + b" /MK << /BC [0 0 1] >>",
                                         value=b"123", da=b"/Helv 12 Tf 0 g") for w in (b"1", b"2")])
    ap("ap-comb-multiline", [tx(b"/Ff 16781312 /MaxLen 5", value=b"123", da=b"/Helv 12 Tf 0 g")])
    ap("ap-comb-password", [tx(b"/Ff 16785408 /MaxLen 5", value=b"123", da=b"/Helv 12 Tf 0 g")])
    ap("ap-comb-no-maxlen", [tx(COMB, value=b"123", da=b"/Helv 12 Tf 0 g")])
    ap("ap-comb-described-auto", [tx(COMB + b" /MaxLen 5", value=b"123", da=b"/Desc 0 Tf 0 g")])

    # Multiline: wrapped in each alignment; line ends; an empty line; a word
    # longer than the line; hyphens; leading and doubled spaces; auto; past
    # the box; an inset border.
    for q, word in ((0, "left"), (1, "centre"), (2, "right")):
        ap("ap-multiline-%s" % word, [tx(MULTILINE + b" /Q %d" % q, value=PARAGRAPH, da=b"/Helv 10 Tf 0 g",
                                         rect=b"[10 10 110 110]")])
    ap("ap-multiline-line-ends", [tx(MULTILINE, value=b"a\\rb\\r\\nc\\nd", da=b"/Helv 8 Tf 0 g",
                                     rect=b"[10 10 110 60]")])
    ap("ap-multiline-empty-line", [tx(MULTILINE, value=b"a\\r\\rb\\r", da=b"/Helv 8 Tf 0 g",
                                      rect=b"[10 10 110 60]")])
    ap("ap-multiline-long-word", [tx(MULTILINE, value=b"Supercalifragilistic expi-ali docious",
                                     da=b"/Helv 10 Tf 0 g", rect=b"[10 10 60 60]")])
    ap("ap-multiline-hyphens", [tx(MULTILINE, value=b"well-known state-of-the-art half-baked ideas",
                                   da=b"/Helv 10 Tf 0 g", rect=b"[10 10 70 90]")])
    ap("ap-multiline-spaces", [tx(MULTILINE + b" /Q 1", value=b"  two leading and  doubled  spaces  in it",
                                  da=b"/Helv 10 Tf 0 g", rect=b"[10 10 80 90]")])
    ap("ap-multiline-auto-short", [tx(MULTILINE, value=b"Hi", rect=b"[10 10 110 60]")])
    ap("ap-multiline-auto-long", [tx(MULTILINE, value=PARAGRAPH, rect=b"[10 10 110 60]")])
    ap("ap-multiline-auto-past", [tx(MULTILINE, value=PARAGRAPH + b" " + PARAGRAPH)])
    ap("ap-multiline-past", [tx(MULTILINE, value=PARAGRAPH, da=b"/Helv 10 Tf 0 g", rect=b"[10 10 110 40]")])
    ap("ap-multiline-inset-centre", [tx(MULTILINE + b" /Q 1 " + bs(b"2", b"I") + b" /MK << /BC [0] >>",
                                        value=b"The quick brown fox jumps over the lazy dog",
                                        da=b"/Helv 10 Tf 0 g", rect=b"[10 10 110 60]")])

    # A single line: line ends in the value, past the box at a fixed size in
    # each alignment, DoNotScroll, a password, /MaxLen without comb, no
    # text, a size taller than the box, a box smaller than its border.
    ap("ap-single-line-ends", [tx(value=v, da=b"/Helv 8 Tf 0 g") for v in (b"a\\rb", b"a\\nb", b"a\\r\\nb")])
    for q, word in ((0, "left"), (1, "centre"), (2, "right")):
        ap("ap-single-past-%s" % word, [tx(b"/Q %d" % q, value=LONG, da=b"/Helv 12 Tf 0 g")])
    ap("ap-do-not-scroll", [tx(DO_NOT_SCROLL, value=LONG, da=b"/Helv 12 Tf 0 g")])
    ap("ap-password", [tx(PASSWORD, value=b"secret"), tx(PASSWORD + b" /Q 2", value=b"s3cr3t pass",
                                                          da=b"/Helv 10 Tf 0 g")])
    ap("ap-max-len", [tx(b"/MaxLen 3", da=b"/Helv 10 Tf 0 g"), tx(b"/MaxLen 0", da=b"/Helv 10 Tf 0 g")])
    ap("ap-empty", [tx(b"/MK << /BG [1] >>", value=b""), tx(b"/MK << /BC [0] >>", value=None),
                    tx(b"/DV (default)", value=None)])
    ap("ap-size-taller-than-the-box", [tx(da=b"/Helv 30 Tf 0 g")])
    ap("ap-box-under-its-border", [tx(bs(b"3") + b" /MK << /BC [0] /BG [0.5] >>", rect=b"[10 10 14 14]")])

    # Rotation: each quarter turn of a wide and a tall widget, a multiline
    # field turned, and turns that aren't quarters or are past one.
    for r in (0, 90, 180, 270):
        ap("ap-rotate-%d" % r, [tx(b"/MK << /R %d /BC [0] >>" % r)])
    for r in (90, 180, 270):
        ap("ap-rotate-%d-tall" % r, [tx(b"/Q 2 /MK << /R %d /BG [1 1 0] >>" % r, rect=b"[10 10 30 110]")])
    ap("ap-rotate-90-multiline", [tx(MULTILINE + b" /MK << /R 90 >>", value=b"one two three four five six",
                                     da=b"/Helv 10 Tf 0 g", rect=b"[10 10 60 110]")])
    ap("ap-rotate-odd", [tx(b"/MK << /R %s >>" % r, da=b"/Helv 10 Tf 0 g")
                         for r in (b"45", b"-90", b"450", b"-270", b"90.0")])

    # What the convenience reads: two widgets of one field, one with its own
    # /DA and /Q; /DA, /Q, /MaxLen and /Ff handed down one level and two; the
    # value a stream, a name, an array.
    ap("ap-kids-own-da-and-q", [b"<< /Parent 50 0 R " + WIDGET + b" /Rect " + BOX + b" >>",
                                b"<< /Parent 50 0 R /DA (/Cour 12 Tf 1 0 0 rg) /Q 2 " + WIDGET
                                + b" /Rect [10 40 110 60] >>"],
       fonts=[b"Helv", b"Cour"], fields=b"50 0 R",
       objs={50: b"<< /T (top) /FT /Tx /V (Hello) /DA (/Helv 10 Tf 0 g) /Q 1 /Kids [5 0 R 6 0 R] >>"})
    ap("ap-inherited-one-level", [b"<< /T (kid) /Parent 50 0 R /V (123) " + WIDGET + b" /Rect " + BOX + b" >>"],
       fonts=[b"Helv"], fields=b"50 0 R",
       objs={50: b"<< /T (top) /FT /Tx " + COMB + b" /MaxLen 5 /DA (/Helv 12 Tf 0 g) /Q 2 /Kids [5 0 R] >>"})
    ap("ap-inherited-two-levels", [b"<< /T (kid) /Parent 51 0 R /V (123) " + WIDGET + b" /Rect " + BOX + b" >>",
                                   b"<< /T (near) /Parent 50 0 R /V (near) " + WIDGET + b" /Rect [10 40 110 60] >>"],
       fonts=[b"Helv"], fields=b"50 0 R",
       objs={50: b"<< /T (top) /FT /Tx /MaxLen 5 /DA (/Helv 12 Tf 0 g) /Q 2 /Kids [51 0 R 6 0 R] >>",
             51: b"<< /T (mid) /Parent 50 0 R /Kids [5 0 R] >>"})
    ap("ap-value-kinds", [tx(value=b"50 0 R", da=b"/Helv 10 Tf 0 g"), tx(value=b"/Name", da=b"/Helv 10 Tf 0 g"),
                          tx(value=b"[(a) (b)]", da=b"/Helv 10 Tf 0 g"), tx(value=b"51 0 R", da=b"/Helv 10 Tf 0 g")],
       objs={50: stream(b"", b"from a stream"), 51: b"(by reference)"})
