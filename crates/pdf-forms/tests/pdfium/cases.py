"""pdf-forms's PDFium oracle cases: `forms`, how PDFium reads the field tree;
`forms-ap`, the appearance PDFium makes for a text field that has none; and
`forms-ap-all` and `forms-press`, every appearance PDFium makes for a button
or a choice field that has none, and its widgets' states after a press.

Written using Python's standard library alone; `oracle/pdfium.py cases --write`
materializes `cases/*.pdf` and `cases.tsv`, and `oracle/pdfium.py record`
records `chromium-8152.txt`. The embedded TrueType cases read pdf-fonts' own
program, `../../../pdf-fonts/tests/pdfium/programs/truetype` from here (which
pdf-fonts' `the_programs_are_the_builders` holds to its builder). `written()`
reads the files the crate's tests write into written/, of fonts pdf-fonts
embeds: `the_written_cases_are_written` writes them, and
`the_written_cases_are_the_maker` holds them to what makes them; `ap-*` are
`forms-ap` cases, and `back-*` appearances made in Helvetica and a fallback
program, drawn on their page, which PDFium's text page reads back (`fonts`)."""

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

    # A kid's value that reaches nothing: a reference to no object, a null,
    # and a reference to a null, each under a parent with a value.
    for name, value, more in (
        ("forms-value-dangling", b"99 0 R", {}),
        ("forms-value-null", b"null", {}),
        ("forms-value-null-by-reference", b"60 0 R", {60: b"null"}),
    ):
        objs = {
            5: b"<< /T (p) /FT /Tx /V (parent) /Kids [6 0 R] >>",
            6: b"<< /T (k) /Parent 5 0 R /V " + value + b" " + WIDGET + b" " + rect(0) + b" >>",
        }
        objs.update(more)
        add(name, b"[5 0 R]", b"[6 0 R]", objs)

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
    written()
    button_choice_cases()
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


def described_truetype(ascent, descent, box):
    """A TrueType font that isn't embedded, its descriptor's heights as given
    and its box `box`, which they may pass."""
    def number(base):
        return (b"<< /Type /Font /Subtype /TrueType /BaseFont /Foo /FirstChar 32 /LastChar 126 /Widths ["
                + b" ".join([b"500"] * 95) + b"] /Encoding /WinAnsiEncoding /FontDescriptor %d 0 R >>" % (base + 1),
                {base + 1: b"<< /Type /FontDescriptor /FontName /Foo /Flags 32 /FontBBox " + box
                           + b" /ItalicAngle 0 /Ascent %d /Descent %d /CapHeight 700 /StemV 80 >>" % (ascent, descent)})
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
    b"TTBox": (130, described_truetype(1079, -250, b"[-431 -250 1483 1050]")),
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
        entries += b" /V " + (value if value[:1] and value[:1] in b"<[/" or value.endswith(b" R") else lit(value))
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


# `forms-ap-all`: a button or a choice field with no /AP, every appearance
# PDFium makes for it; `forms-press`: the same with its widgets' states and
# its value after a press, the pressed widget's index in the case's `.press`
# file. Each is laid out as `ap` lays out a `forms-ap` case.
RADIO, PUSH, COMBO = b"/Ff 49152", b"/Ff 65536", b"/Ff 131072"


def widget_of(entries, rect=b"[10 10 24 24]"):
    """A field's merged widget: `entries` its own, at `rect`."""
    return entries + b" " + WIDGET + b" /Rect " + rect


def as_kind(kind, **more):
    """The case `ap` added last, made a case of `kind`, with the files
    `more` gives beside its own."""
    name, _, files = ROWS.pop()
    files.update(more)
    ROWS.append((name, kind, files))


def button_choice_cases():
    # A check box and a radio button, each its mark named by /MK /CA in
    # ZapfDingbats (a check, a circle), a border and a background.
    ap("btn-check", [widget_of(b"/FT /Btn /DA (/ZaDb 0 Tf 0 g) /MK << /CA (4) /BC [0] /BG [1] >>")])
    as_kind("forms-ap-all")
    ap("btn-radio", [widget_of(b"/FT /Btn " + RADIO + b" /DA (/ZaDb 0 Tf 0 g) /MK << /CA (l) /BC [0] /BG [1] >>")])
    as_kind("forms-ap-all")
    # A push button whose highlighting is a push: its rollover and down
    # appearances beside its normal one, the down one with a caption of its
    # own.
    ap("push-caption", [widget_of(b"/FT /Btn " + PUSH + b" /H /P /DA (/Helv 10 Tf 0 g) /MK << /CA (Push) /AC (Down)"
                                  b" /BC [0] /BG [0.75] >> " + bs(b"1", b"B"), b"[10 10 70 30]")])
    as_kind("forms-ap-all")
    # A combo box showing the label of the option its value is; a list box
    # with its value's option highlighted, more options than it shows.
    ap("combo-label", [widget_of(b"/FT /Ch " + COMBO + b" /DA (/Helv 10 Tf 0 g) /Opt [[(e1) (Label one)]"
                                 b" [(e2) (Label two)]] /V (e2) /MK << /BC [0] >>", BOX)])
    as_kind("forms-ap-all")
    ap("list-select", [widget_of(b"/FT /Ch /DA (/Helv 10 Tf 0 g) /Opt [(alpha) (bravo) (charlie) (delta)]"
                                 b" /V (bravo) /MK << /BC [0] >>", b"[10 10 90 50]")])
    as_kind("forms-ap-all")
    # A radio group of three widgets, each its own state, the first on;
    # the second pressed.
    on = stream(b"/Type /XObject /Subtype /Form /BBox [0 0 20 20] ", b"0 g 5 5 10 10 re f")
    off = stream(b"/Type /XObject /Subtype /Form /BBox [0 0 20 20] ", b"")
    ap("press-radio",
       [b"<< /Parent 50 0 R /AS /%s /AP << /N << /%d 51 0 R /Off 52 0 R >> >> /MK << /BC [0] >> "
        % (b"1" if i == 0 else b"Off", i + 1) + WIDGET + b" /Rect [%d 10 %d 30] >>" % (10 + 30 * i, 30 + 30 * i)
        for i in range(3)],
       fonts=[], fields=b"50 0 R",
       objs={50: b"<< /T (group) /FT /Btn " + RADIO + b" /V /1 /Kids [5 0 R 6 0 R 7 0 R] >>", 51: on, 52: off})
    as_kind("forms-press", press=b"1")
    press_cases()
    toggle_cases()
    push_cases()
    choice_cases()


def press(name, ff, kids, value=b"", opt=b"", presses=b"0"):
    """A `forms-press` case laid out as `press-radio`: a button field of /Ff
    `ff`, object 50, with `value` and `opt` its own entries, and a widget for
    each of `kids`, its /AS and the name of its normal appearance's on state
    (None for no /AP), from the left; `presses` the widgets pressed in
    turn."""
    on = stream(b"/Type /XObject /Subtype /Form /BBox [0 0 20 20] ", b"0 g 5 5 10 10 re f")
    off = stream(b"/Type /XObject /Subtype /Form /BBox [0 0 20 20] ", b"")
    widgets = []
    for i, (state, own) in enumerate(kids):
        states = b" /AP << /N << /" + own + b" 51 0 R /Off 52 0 R >> >>" if own else b""
        widgets.append(b"<< /Parent 50 0 R /AS /" + state + states + b" /MK << /BC [0] >> " + WIDGET
                       + b" /Rect [%d 10 %d 30] >>" % (10 + 30 * i, 30 + 30 * i))
    refs = b" ".join(b"%d 0 R" % (5 + i) for i in range(len(kids)))
    field = b"<< /T (group) /FT /Btn /Ff %d " % ff + value + opt + b"/Kids [" + refs + b"] >>"
    ap(name, widgets, fonts=[], fields=b"50 0 R", objs={50: field, 51: on, 52: off})
    as_kind("forms-press", press=presses)


def press_cases():
    radio, no_toggle, unison = 32768, 16384, 33554432
    # A radio button pressed while on, with NoToggleToOff and without; and
    # one pressed while off, without.
    three = [(b"1", b"1"), (b"Off", b"2"), (b"Off", b"3")]
    press("press-radio-on", radio | no_toggle, three, b"/V /1 ", presses=b"0")
    press("press-radio-on-toggles", radio, three, b"/V /1 ", presses=b"0")
    press("press-radio-off-toggles", radio, three, b"/V /1 ", presses=b"1")
    # Radio buttons of one on state, in unison and not.
    same = [(b"Off", b"1"), (b"Off", b"2"), (b"Off", b"1")]
    press("press-radio-unison", radio | unison, same, presses=b"2")
    press("press-radio-not-unison", radio, same, presses=b"2")
    # Radio buttons of on states named On, spouse and Choice1, two pressed.
    named = [(b"On", b"On"), (b"Off", b"spouse"), (b"Off", b"Choice1")]
    press("press-named-states", radio, named, b"/V /On ", presses=b"2 1")
    # Check boxes of one on state go together, on and off; check boxes of
    # distinct on states act as radio buttons, and may all be off; one
    # pressed twice; two with no /AP, whose states PDFium makes as Yes.
    press("press-check-together", 0, [(b"Off", b"Yes"), (b"Off", b"Yes")], presses=b"0")
    press("press-check-together-off", 0, [(b"Yes", b"Yes"), (b"Yes", b"Yes")], b"/V /Yes ", presses=b"1")
    press("press-check-distinct", 0, [(b"a", b"a"), (b"Off", b"b"), (b"Off", b"c")], b"/V /a ", presses=b"1")
    press("press-check-distinct-off", 0, [(b"a", b"a"), (b"Off", b"b")], b"/V /a ", presses=b"0")
    press("press-check-twice", 0, [(b"Off", b"Yes")], presses=b"0 0")
    press("press-no-ap", 0, [(b"Off", None), (b"Off", None)], presses=b"0")
    # /Opt's export values: check boxes of two widgets of one export value,
    # told apart by their indexes, one turned on and one off; radio buttons,
    # in unison and not; check boxes whose states have names of their own.
    opt = b"/Opt [(apple) (pear) (apple)] "
    by_index = [(b"Off", b"0"), (b"Off", b"1"), (b"Off", b"2")]
    first_on = [(b"0", b"0"), (b"Off", b"1"), (b"Off", b"2")]
    press("press-opt-check", 0, by_index, opt=opt, presses=b"0")
    press("press-opt-check-off", 0, first_on, b"/V /0 ", opt, presses=b"0")
    press("press-opt-radio", radio, first_on, b"/V /0 ", opt, presses=b"1")
    press("press-opt-radio-unison", radio | unison, by_index, opt=opt, presses=b"0")
    press("press-opt-own-names", 0, [(b"Off", b"x"), (b"Off", b"y")], opt=b"/Opt [(a) (b)] ", presses=b"1")
    # An /Opt shorter than its widgets: two check boxes of one on state, the
    # second with no entry, which don't go together, either pressed; and an
    # empty /Opt over two with no /AP, whose states PDFium makes by index.
    short = [(b"Off", b"a"), (b"Off", b"a")]
    press("press-opt-short", 0, short, opt=b"/Opt [(a)] ", presses=b"0")
    press("press-opt-short-past", 0, short, opt=b"/Opt [(a)] ", presses=b"1")
    press("press-opt-empty", 0, [(b"Off", None), (b"Off", None)], opt=b"/Opt [] ", presses=b"0")


def at(i, w=12, h=12, cols=8):
    """The `/Rect` of the `i`th widget of a grid, `w` by `h`, 60 apart."""
    x, y = 10 + 60 * (i % cols), 10 + 60 * (i // cols)
    return b"[%d %d %s %s]" % (x, y, b"%g" % (x + w), b"%g" % (y + h))


def toggle(ff, rest, i, w=12, h=12):
    """A check box's or radio button's merged widget: /Ff `ff`, `rest` its
    other entries, the `i`th of a grid."""
    return widget_of(b"/FT /Btn /Ff %d " % ff + rest, at(i, w, h))


def toggle_cases():
    check, radio = 0, 49152
    # Each mark a caption names, and none (the default) and 6 (a glyph
    # PDFium has no shape for), in a check box and a radio button, at three
    # boxes, a black border of 1 on white.
    captions = [None, b"4", b"8", b"H", b"l", b"n", b"u", b"6"]
    boxes = [(12, 12), (20, 20), (14, 20)]
    for word, ff in (("check", check), ("radio", radio)):
        widgets = []
        for caption in captions:
            for w, h in boxes:
                ca = b"/CA (" + caption + b") " if caption else b""
                widgets.append(toggle(ff, b"/DA (/ZaDb 0 Tf 0 g) /MK << " + ca + b"/BC [0] /BG [1] >>",
                                      len(widgets), w, h))
        ap("btn-marks-" + word, widgets)
        as_kind("forms-ap-all")
    # Each border style, with a background and without: none (a width of 0),
    # solid, beveled, inset, dashed, underline; with a check mark, with the
    # default mark (a radio button's round frame), and with none but the
    # frame drawn (no border color).
    styles = [(None, None), (b"S", b"1"), (b"B", b"2"), (b"I", b"1"), (b"D", b"1"), (b"U", b"1"), (b"B", b"1")]
    for word, ff, captions in (("check", check, [b"4", None]), ("radio", radio, [None, b"4"])):
        widgets = []
        for caption in captions:
            for style, width in styles:
                for bg in (b"", b"/BG [0.9] "):
                    ca = b"/CA (" + caption + b") " if caption else b""
                    bc = b"/BC [0] " if style else b""
                    bs = b" /BS << /W %s /S /%s >>" % (width, style) if style else b" /BS << /W 0 >>"
                    widgets.append(toggle(ff, b"/DA (/ZaDb 0 Tf 0 g) /MK << " + ca + bc + bg + b">>" + bs,
                                          len(widgets), 16, 16))
        # Beveled and inset with no border color: the bands alone.
        for style in (b"B", b"I"):
            widgets.append(toggle(ff, b"/DA (/ZaDb 0 Tf 0 g) /MK << /BG [0.5] >> /BS << /W 1 /S /%s >>" % style,
                                  len(widgets), 16, 16))
        ap("btn-borders-" + word, widgets)
        as_kind("forms-ap-all")
    # Turned: a check, a circle and a cross, a quarter, a half and three
    # quarters, in a box 14 by 20.
    widgets = []
    for ff, caption in ((check, b"4"), (radio, b"l"), (check, b"8")):
        for r in (b"90", b"180", b"270"):
            widgets.append(toggle(ff, b"/DA (/ZaDb 0 Tf 0 g) /MK << /CA (" + caption + b") /R " + r
                                  + b" /BC [0] /BG [1] >>", len(widgets), 14, 20))
    ap("btn-rotate", widgets)
    as_kind("forms-ap-all")
    # /DA's colors (gray, RGB, CMYK, none), and its size: 8 (the mark drawn
    # at it), 0, and no /DA at all; a background in RGB and CMYK, whose down
    # appearance is each component less 0.25.
    widgets = []
    for ff in (check, radio):
        for da, mk in ((b"/DA (/ZaDb 0 Tf 0.5 g)", b"/BG [1 0 0]"), (b"/DA (/ZaDb 0 Tf 0 0 1 rg)", b"/BG [0.1 0.2 0.3 0.4]"),
                       (b"/DA (/ZaDb 0 Tf 0 0 0 1 k)", b""), (b"/DA (/ZaDb 8 Tf 0 g)", b""), (b"", b"")):
            widgets.append(toggle(ff, da + b" /MK << /CA (n) /BC [0] " + mk + b" >>", len(widgets), 20, 20))
    ap("btn-da", widgets)
    as_kind("forms-ap-all")
    # The on state's name a widget has, under /NeedAppearances, which makes
    # PDFium make every widget's appearances anew: /1, /Yes, /On, /spouse,
    # /Choice1, none of them with an Off of its own, the first on; with no
    # colors, so nothing is drawn when they're off.
    on = stream(b"/Type /XObject /Subtype /Form /BBox [0 0 12 12] ", b"0 g 2 2 8 8 re f")
    widgets = []
    for i, name in enumerate((b"1", b"Yes", b"On", b"spouse", b"Choice1")):
        state = name if i == 0 else b"Off"
        widgets.append(toggle(check if i % 2 == 0 else radio, b"/DA (/ZaDb 0 Tf 0 g) /AS /" + state
                              + b" /AP << /N << /" + name + b" 50 0 R >> >>", i))
    ap("btn-states", widgets, form=b"/NeedAppearances true", objs={50: on})
    as_kind("forms-ap-all")
    # /Opt: check boxes and radio buttons of a field each, its kids with no
    # /AP, named by their indexes; and under /NeedAppearances a check box
    # field whose first kid has a name of its own, which PDFium names by its
    # index all the same.
    for word, ff in (("check", check), ("radio", radio)):
        kids = [b"<< /Parent 50 0 R /MK << /CA (4) /BC [0] >> " + WIDGET + b" /Rect " + at(i) + b" >>"
                for i in range(3)]
        field = (b"<< /T (g) /FT /Btn /Ff %d /DA (/ZaDb 0 Tf 0 g) /Opt [(apple) (pear) (apple)]"
                 b" /Kids [5 0 R 6 0 R 7 0 R] >>" % ff)
        ap("btn-opt-" + word, kids, fields=b"50 0 R", objs={50: field})
        as_kind("forms-ap-all")
    kids = [b"<< /Parent 50 0 R /AS /Off /AP << /N << /mine 51 0 R >> >> /MK << /BC [0] >> " + WIDGET
            + b" /Rect " + at(0) + b" >>",
            b"<< /Parent 50 0 R /MK << /BC [0] >> " + WIDGET + b" /Rect " + at(1) + b" >>"]
    field = b"<< /T (g) /FT /Btn /DA (/ZaDb 0 Tf 0 g) /Opt [(apple) (pear)] /Kids [5 0 R 6 0 R] >>"
    ap("btn-opt-own-name", kids, fields=b"50 0 R", objs={50: field, 51: on}, form=b"/NeedAppearances true")
    as_kind("forms-ap-all")
    # The default resources' ZapfDingbats with an encoding of its own, whose
    # code for a check mark draws a circle, and a /DA naming Helvetica: the
    # mark drawn in ZapfDingbats as it's written by name all the same.
    objects = {
        5: b"<< /T (f0) " + toggle(check, b"/DA (/ZaEnc 0 Tf 0 g) /MK << /CA (4) /BC [0] >>", 0) + b" >>",
        6: b"<< /T (f1) " + toggle(radio, b"/DA (/Helv 0 Tf 0 g) /MK << /BC [0] >>", 1) + b" >>",
        60: std(b"ZapfDingbats", b" /Encoding << /Differences [52 /a71] >>"),
        100: FONTS[b"Helv"][1],
    }
    acroform = b"<< /Fields [5 0 R 6 0 R] /DR << /Font << /ZaEnc 60 0 R /Helv 100 0 R >> >> >>"
    ROWS.append(("btn-fonts", "forms-ap-all", {"pdf": build_doc(acroform, b"[5 0 R 6 0 R]", objects)}))


def push(rest, i, w=50, h=20):
    """A push button's merged widget, `rest` its other entries, the `i`th of
    a grid of four a row."""
    return widget_of(b"/FT /Btn /Ff 65536 " + rest, at(i, w, h, cols=4))


def push_cases():
    # /DA's sizes: automatic, 9, 12 in red, none at all (12 in a font PDFium
    # adds), a color alone (automatic, the same font), Times; a caption wider
    # than its box at 10 and automatic; no caption; the icon alone with no
    # icon; turned a quarter.
    widgets = []
    for da in (b"/DA (/Helv 0 Tf 0 g)", b"/DA (/Helv 9 Tf 0 g)", b"/DA (/Helv 12 Tf 1 0 0 rg)", b"",
               b"/DA (0 0 1 rg)", b"/DA (/TiRo 10 Tf 0 g)"):
        widgets.append(push(da + b" /MK << /CA (Push me) /BC [0] /BG [0.75] >> /BS << /W 1 /S /B >>",
                            len(widgets)))
    for size in (b"10", b"0"):
        widgets.append(push(b"/DA (/Helv " + size + b" Tf 0 g) /MK << /CA (A caption much wider than its box)"
                            b" /BC [0] >>", len(widgets)))
    widgets.append(push(b"/DA (/Helv 10 Tf 0 g) /MK << /BC [0] /BG [1 1 0] >>", len(widgets)))
    widgets.append(push(b"/DA (/Helv 10 Tf 0 g) /MK << /CA (Hidden) /TP 1 /BC [0] >>", len(widgets)))
    widgets.append(push(b"/DA (/Helv 10 Tf 0 g) /MK << /CA (Turned) /R 90 /BC [0] >>", len(widgets), 20, 50))
    widgets.append(push(b"/DA (/Helv 10 Tf 0 g) /MK << /CA (Tall) >>", len(widgets), 50, 60))
    ap("push-sizes", widgets)
    as_kind("forms-ap-all")
    # Highlighting: none, invert, outline, push and toggle; push with its
    # rollover and down captions and without; each border style; a
    # background in gray, RGB, CMYK and none, whose down appearance is each
    # component less 0.25.
    widgets = []
    for h in (b"N", b"I", b"O", b"P", b"T"):
        widgets.append(push(b"/DA (/Helv 10 Tf 0 g) /H /" + h + b" /MK << /CA (Go) /BC [0] /BG [0.75] >>",
                            len(widgets)))
    widgets.append(push(b"/DA (/Helv 10 Tf 0 g) /H /P /MK << /CA (Go) /RC (Over) /AC (Down) /BC [0] >>",
                        len(widgets)))
    widgets.append(push(b"/DA (/Helv 10 Tf 0 g) /H /P /MK << /CA (Go) /RC () /BC [0] >>", len(widgets)))
    for style, bg in ((b"B", b"[0.75]"), (b"I", b"[1 0 0]"), (b"S", b"[0.1 0.2 0.3 0.4]"), (b"D", None),
                      (b"U", b"[0.5]"), (b"B", None)):
        mk = b"/BG " + bg + b" " if bg else b""
        widgets.append(push(b"/DA (/Helv 10 Tf 0 g) /H /P /MK << /CA (Press) /BC [0 0 1] " + mk
                            + b">> /BS << /W 1 /S /" + style + b" >>", len(widgets)))
    ap("push-states", widgets)
    as_kind("forms-ap-all")
    # A /DA whose size is below 0, beside one of 0: PDFium sets no font and
    # a size of 0 for it.
    ap("push-size-negative", [push(b"/DA (/Helv " + size + b" Tf 0 g) /MK << /CA (Neg) /BC [0] >>", i)
                              for i, size in enumerate((b"-10", b"0"))])
    as_kind("forms-ap-all")


def place(i, w, h, cols=4):
    """The `/Rect` of the `i`th choice widget of a grid, `w` by `h`, its
    cells 120 by 90."""
    x, y = 10 + 120 * (i % cols), 10 + 90 * (i // cols)
    return b"[%d %d %s %s]" % (x, y, b"%g" % (x + w), b"%g" % (y + h))


def choice_cases():
    edit, multiple = 262144, 2097152
    # Combo boxes, 100 by 20 unless said: a value among pairs, its label
    # shown; an editable one's value outside them; an automatic size; /I
    # against /V and with it; /Q 1 and 2, which PDFium doesn't apply; each
    # border style; no value; a label wider than its text box; a value
    # outside the options of one that isn't editable; a /DA with no font;
    # a value that is an array, its first item's option shown.
    pairs = b"/Opt [[(e1) (Label one)] [(e2) (Label two)] [(e3) (Three)]]"
    ab = b"/Opt [(a) (b)] /V (b)"
    combos = [
        (0, b"/DA (/Helv 10 Tf 0 g) " + pairs + b" /V (e2) /MK << /BC [0] >>"),
        (edit, b"/DA (/Helv 10 Tf 0 g) " + pairs + b" /V (typed) /MK << /BC [0] >>"),
        (0, b"/DA (/Helv 0 Tf 0 g) " + pairs + b" /V (e2) /MK << /BC [0] >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + pairs + b" /V (e2) /I [0]"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + pairs + b" /V (e1) /I [0]"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + ab + b" /Q 1"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + ab + b" /Q 2"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + ab + b" /MK << /BC [0] /BG [0.9] >> /BS << /W 1 /S /B >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + ab + b" /MK << /BC [1 0 0] >> /BS << /W 2 /S /I >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + ab + b" /MK << /BC [0] >> /BS << /W 1 /S /D >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + ab + b" /MK << /BC [0] >> /BS << /W 1 /S /U >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) /Opt [(a) (b)] /MK << /BC [0] >>"),
        (0, b"/DA (/Helv 10 Tf 0 0 1 rg) /Opt [(A long label that passes the box)]"
            b" /V (A long label that passes the box)"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + pairs + b" /V (outside)"),
        (0, b"/DA (0 0 1 rg) " + pairs + b" /V (e3) /MK << /BG [1 1 0] >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + pairs + b" /V [(e2) (e1)]"),
    ]
    ap("combo-values", [widget_of(b"/FT /Ch /Ff %d " % (131072 | ff) + rest, place(i, 100, 20))
                        for i, (ff, rest) in enumerate(combos)])
    as_kind("forms-ap-all")
    # Combo boxes narrower than their drop button, which then lies over
    # the text box's left edge, the text box turned the right way round; too
    # low for the triangle, and for the button's bands; turned a quarter, a
    # half, and three quarters, over gray, RGB and CMYK backgrounds, beveled
    # and inset.
    boxes = [(12, 20, b""), (10, 20, b""), (40, 7, b""), (40, 5, b""), (5, 5, b""),
             (20, 100, b"/MK << /R 90 /BC [0] /BG [0.75] >>"),
             (100, 20, b"/MK << /R 180 /BC [0 0 1] /BG [1 0.9 0.9] >> /BS << /W 1 /S /B >>"),
             (20, 100, b"/MK << /R 270 /BC [0] /BG [0.1 0.2 0.3 0.4] >> /BS << /W 1 /S /I >>")]
    ap("combo-boxes", [widget_of(b"/FT /Ch /Ff 131072 /DA (/Helv 8 Tf 0 g) /Opt [(a) (Turned)] /V (%s) " % (
        b"Turned" if more else b"a") + more, place(i, w, h)) for i, (w, h, more) in enumerate(boxes)])
    as_kind("forms-ap-all")
    # List boxes, 80 by 40 unless said, alpha to echo: one selected, its
    # last lines below the box; two, by /I and /V and by /V alone; /I
    # against /V; /TI 2 and 0, the selected line partly below; at size 0,
    # which is 12; at 9 in blue, /Q 2; pairs; no options; beveled over gray;
    # /Q 1; thirty options; a label wider than the box; an empty label; and
    # turned a quarter.
    abc = b"/Opt [(alpha) (bravo) (charlie) (delta) (echo)]"
    lists = [
        (0, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V (bravo)"),
        (multiple, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V [(alpha) (charlie)] /I [0 2]"),
        (multiple, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V [(alpha) (charlie)]"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V (charlie) /I [1]"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V (bravo) /TI 2"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V (delta) /TI 0"),
        (0, b"/DA (/Helv 0 Tf 0 g) " + abc + b" /V (bravo)"),
        (0, b"/DA (/Helv 9 Tf 0 0 1 rg) " + abc + b" /V (bravo) /Q 2"),
        (0, b"/DA (/Helv 10 Tf 0 g) /Opt [[(x1) (Ex one)] [(x2) (Ex two)]] /V (x2)"),
        (0, b"/DA (/Helv 10 Tf 0 g) /Opt [] /MK << /BC [0] >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V (bravo) /MK << /BC [0] /BG [0.9] >> /BS << /W 1 /S /B >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V (bravo) /Q 1 /MK << /BC [0] >>"),
        (0, b"/DA (/Helv 10 Tf 0 g) /Opt [" + b" ".join(b"(item %d)" % n for n in range(30)) + b"] /V (item 1)"),
        (0, b"/DA (/Helv 10 Tf 0 g) /Opt [(A label much wider than its box) (b)] /V (b)"),
        (0, b"/DA (/Helv 10 Tf 0 g) /Opt [(alpha) () (charlie)] /V (charlie)"),
        (0, b"/DA (/Helv 10 Tf 0 g) " + abc + b" /V (charlie) /MK << /R 90 /BC [0] >>"),
    ]
    ap("list-values", [widget_of(b"/FT /Ch /Ff %d " % ff + rest, place(i, 40 if i == 15 else 80, 80 if i == 15 else 40))
                       for i, (ff, rest) in enumerate(lists)])
    as_kind("forms-ap-all")
    # List boxes with no value, or none that is text: /I alone, one index
    # and two; a number in /I's place; /I beside a /V that is a name, and
    # one that is an empty array; a default value alone.
    ten = b"/DA (/Helv 10 Tf 0 g) "
    lists = [
        (0, ten + abc + b" /I [1]"),
        (multiple, ten + abc + b" /I [0 2]"),
        (0, ten + abc + b" /I 1"),
        (0, ten + abc + b" /V /bravo /I [1]"),
        (multiple, ten + abc + b" /V [] /I [1]"),
        (0, ten + abc + b" /DV (bravo)"),
    ]
    ap("list-no-value", [widget_of(b"/FT /Ch /Ff %d " % ff + rest, place(i, 80, 40))
                         for i, (ff, rest) in enumerate(lists)])
    as_kind("forms-ap-all")
    # A top index below 0, and past the options; a /DA size below 0.
    lists = [
        ten + abc + b" /V (bravo) /TI -1",
        ten + abc + b" /V (bravo) /TI 9",
        b"/DA (/Helv -10 Tf 0 g) " + abc + b" /V (bravo)",
    ]
    ap("list-odd", [widget_of(b"/FT /Ch " + rest, place(i, 80, 40)) for i, rest in enumerate(lists)])
    as_kind("forms-ap-all")
    # Combo boxes with no value: a default value alone, which PDFium
    # shows; /I alone, an array and a number; and one with a value whose /DA
    # size is below 0.
    combos = [
        ten + pairs + b" /DV (e3)",
        ten + pairs + b" /I [1]",
        ten + pairs + b" /I 1",
        b"/DA (/Helv -10 Tf 0 g) " + pairs + b" /V (e2)",
    ]
    ap("combo-no-value", [widget_of(b"/FT /Ch /Ff 131072 " + rest, place(i, 100, 20))
                          for i, rest in enumerate(combos)])
    as_kind("forms-ap-all")


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
    # A beveled border's dark band: the background halved, in CMYK and RGB.
    ap("ap-border-beveled-colors", [tx(bs(b"2", b"B") + b" /MK << /BC [0] /BG [0.1 0.2 0.3 0.4] >>"),
                                    tx(bs(b"2", b"B") + b" /MK << /BC [0] /BG [0.2 0.4 0.6] >>")])

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
    # A comb field's dividers under a dashed border's own dash array.
    ap("ap-comb-border-dash-array", [tx(COMB + b" /MaxLen 3 " + bs(b"1", b"D", b" /D [2 1]") + b" /MK << /BC [0] >>",
                                        value=b"12", da=b"/Helv 10 Tf 0 g")])
    ap("ap-comb-multiline", [tx(b"/Ff 16781312 /MaxLen 5", value=b"123", da=b"/Helv 12 Tf 0 g")])
    ap("ap-comb-password", [tx(b"/Ff 16785408 /MaxLen 5", value=b"123", da=b"/Helv 12 Tf 0 g")])
    ap("ap-comb-no-maxlen", [tx(COMB, value=b"123", da=b"/Helv 12 Tf 0 g")])
    ap("ap-comb-described-auto", [tx(COMB + b" /MaxLen 5", value=b"123", da=b"/Desc 0 Tf 0 g")])
    # Cells narrower than the box is tall, where the font's box and PDFium's
    # sizes give different sizes.
    ap("ap-comb-described-auto-cells", [tx(COMB + b" /MaxLen 10 " + bs(b"0"), value=b"123", da=b"/Desc 0 Tf 0 g",
                                           rect=b"[10 10 110 40]")])

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
    # A line its glyphs fill exactly: ten of Courier's 600 across the text's
    # box at each size, whose sum in f32 passes the box at some (10 points:
    # nine fit) and not at others (12 points: ten); then words, broken before
    # the one that passes it at every size; and a password's stars, seven
    # across at 10 points, which six fill.
    sizes = (4, 5, 6, 7, 8, 9, 10, 11, 12, 14)
    ap("ap-multiline-full-line",
       [tx(MULTILINE, value=value, da=b"/Cour %d Tf 0 g" % size,
           rect=b"[%d %d %d %d]" % (x, 10 + 70 * i, x + 2 + 6 * size, 70 + 70 * i))
        for x, value in ((10, b"AAAAAAAAAAAA"), (300, b"AAAAA AAAA AAAA")) for i, size in enumerate(sizes)]
       + [tx(b"/Ff 12288", value=b"passwords", da=b"/Cour 10 Tf 0 g", rect=b"[150 10 194 70]")])

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
    # A comb field with no /MaxLen of its own takes its first widget's that
    # has one, for each of its widgets (PDFium's GetMaxLen): the second's
    # where the first has none, the first's before the second's, the
    # field's own before either, a real cut to an integer.
    kids = [(b"", b" /MaxLen 4"), (b" /MaxLen 7", b" /MaxLen 4"), (b"", b" /MaxLen 4"), (b" /MaxLen 3.7", b"")]
    ap("ap-max-len-from-a-widget",
       [b"<< /Parent %d 0 R " % (50 + i // 2) + WIDGET + b" /Rect [10 %d 210 %d]" % (10 + 30 * i, 30 + 30 * i)
        + kids[i // 2][i % 2] + b" >>" for i in range(8)],
       fonts=[b"Helv"], fields=b"50 0 R 51 0 R 52 0 R 53 0 R",
       objs={50 + f: b"<< /T (f%d) /FT /Tx " % f + COMB + (b" /MaxLen 6" if f == 2 else b"")
             + b" /V (123456789) /DA (/Helv 12 Tf 0 g) /Kids [%d 0 R %d 0 R] >>" % (5 + 2 * f, 6 + 2 * f)
             for f in range(4)})
    # A /MaxLen held by null, the field's own, through a reference (60, a
    # null object) or its parent's (56), is found as PDFium keeps a null in a
    # dictionary, and the widgets' 4 isn't read: no cells. A reference to no
    # object (61) is no entry, and the widgets' 4 makes the cells. A first
    # widget's null, or its reference to no object, is that widget's entry
    # and no number: no cells. The value is one character, which a comb
    # field with no /MaxLen draws as PDFium does (ap-comb-no-maxlen's
    # difference is in a line of more).
    own = [b" /MaxLen null", b" /MaxLen 60 0 R", b" /Parent 56 0 R", b" /MaxLen 61 0 R", b"", b""]
    first = [b" /MaxLen 4"] * 4 + [b" /MaxLen null", b" /MaxLen 61 0 R"]
    objs = {50 + f: b"<< /T (f%d) /FT /Tx " % f + COMB + own[f]
            + b" /V (1) /DA (/Helv 12 Tf 0 g) /Kids [%d 0 R %d 0 R] >>" % (5 + 2 * f, 6 + 2 * f)
            for f in range(6)}
    objs.update({56: b"<< /T (p) /MaxLen null /Kids [52 0 R] >>", 60: b"null"})
    ap("ap-max-len-held-by-null",
       [b"<< /Parent %d 0 R " % (50 + i // 2) + WIDGET + b" /Rect [10 %d 210 %d]" % (10 + 30 * i, 30 + 30 * i)
        + (first[i // 2] if i % 2 == 0 else b" /MaxLen 4") + b" >>" for i in range(12)],
       fonts=[b"Helv"], fields=b"50 0 R 51 0 R 56 0 R 53 0 R 54 0 R 55 0 R", objs=objs)
    ap("ap-value-kinds", [tx(value=b"50 0 R", da=b"/Helv 10 Tf 0 g"), tx(value=b"/Name", da=b"/Helv 10 Tf 0 g"),
                          tx(value=b"[(a) (b)]", da=b"/Helv 10 Tf 0 g"), tx(value=b"51 0 R", da=b"/Helv 10 Tf 0 g")],
       objs={50: stream(b"", b"from a stream"), 51: b"(by reference)"})

    # What /DA says oddly: a stroking color beside the text's or alone; a
    # size too small to write; a font and a color written twice.
    ap("ap-da-stroking-color", [tx(da=da) for da in (b"/Helv 8 Tf 1 0 0 rg 0 0 1 RG", b"0 0 1 RG /Helv 8 Tf",
                                                     b"/Helv 8 Tf 0 g 1 G", b"/Helv 8 Tf 0 1 1 0 K 0 0 0 1 k")],
       fonts=[b"Helv"])
    ap("ap-da-size-near-zero", [tx(da=b"/Helv %s Tf 0 g" % size) for size in (b"0.00001", b"0.00009", b"0.0004")])
    ap("ap-da-written-twice", [tx(da=da) for da in (b"/Helv 8 Tf /Helv 20 Tf 0 g", b"/Helv 8 Tf 0 g 1 0 0 rg",
                                                    b"/Helv 8 Tf 0 0 1 rg 1 0 0 rg")])

    # A descriptor's heights as given past its /FontBBox, which PDFium sets
    # a field's lines by: a TrueType font not embedded, whose ascent is above
    # its box's top.
    ap("ap-font-truetype-past-its-box", [tx(value=b"Sample 123", da=b"/TTBox 12 Tf 0 g"),
                                         tx(MULTILINE, value=b"Sample 123 and more", da=b"/TTBox 10 Tf 0 g",
                                            rect=b"[10 40 110 90]"),
                                         tx(value=b"Sample", da=b"/TTBox 0 Tf 0 g", rect=b"[10 100 110 130]")])
    # Text given decomposed, a letter and a combining accent, which the font
    # has composed and not apart.
    ap("ap-nfd-helvetica", [tx(value=utf16("Cafe\u0301"), da=b"/Helv 12 Tf 0 g"),
                            tx(MULTILINE, value=utf16("Cafe\u0301 au lait"), da=b"/Helv 12 Tf 0 g",
                               rect=b"[10 40 110 90]")])


def written():
    # Each file the crate's tests write into written/: a `forms-ap` case of
    # a TrueType program embedded as a Type 0 font in /DR (`ap-*`), or an
    # appearance made in Helvetica and a fallback program, drawn on the page,
    # read back by PDFium's text page (`back-*`).
    folder = os.path.join(os.path.dirname(os.path.abspath(__file__)), "written")
    for name in sorted(os.listdir(folder)):
        if name.endswith(".pdf"):
            kind = "forms-ap" if name.startswith("ap-") else "fonts"
            with open(os.path.join(folder, name), "rb") as f:
                ROWS.append((name[:-len(".pdf")], kind, {"pdf": f.read()}))
