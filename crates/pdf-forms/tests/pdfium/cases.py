"""pdf-forms's PDFium oracle cases (`forms`).

Written using Python's standard library alone; `oracle/pdfium.py cases --write`
materializes `cases/*.pdf` and `cases.tsv`, and `oracle/pdfium.py record`
records `chromium-8152.txt`."""

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

    return list(ROWS)
