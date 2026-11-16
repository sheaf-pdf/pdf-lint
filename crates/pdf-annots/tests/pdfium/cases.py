"""pdf-annots's PDFium oracle cases (`annots`).

Written using Python's standard library alone; `oracle/pdfium.py cases --write`
materializes `cases/*.pdf` and `cases.tsv`, and `oracle/pdfium.py record`
records `chromium-8152.txt`."""

ROWS = []


def build_doc(annots=None, catalog_extra=b"", extra_objs=None, page_count=2, page_extra=b""):
    pages_kids = b" ".join(b"%d 0 R" % (3 + i) for i in range(page_count))
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R" + (b" " + catalog_extra if catalog_extra else b"") + b" >>",
        2: b"<< /Type /Pages /Kids [" + pages_kids + b"] /Count %d >>" % page_count,
    }
    p0 = bytearray(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792]")
    if page_extra:
        p0 += b" " + page_extra
    if annots is not None:
        p0 += b" /Annots " + annots
    p0 += b" >>"
    objects[3] = bytes(p0)
    for i in range(1, page_count):
        objects[3 + i] = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>"
    if extra_objs:
        objects.update(extra_objs)

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


def add(name, annots=None, catalog_extra=b"", extra_objs=None, page_count=2, kind="annots", page_extra=b""):
    ROWS.append((
        name,
        kind,
        {"pdf": build_doc(annots=annots, catalog_extra=catalog_extra, extra_objs=extra_objs, page_count=page_count, page_extra=page_extra)},
    ))


def stream_obj(header, body):
    return header + b" /Length %d >>\nstream\n" % len(body) + body + b"\nendstream"


def cases():
    del ROWS[:]

    # 1. Empty / missing / non-array / malformed /Annots slots
    add("annots-absent", annots=None)
    add("annots-empty-array", annots=b"[]")
    add("annots-not-array", annots=b"<< /Foo 1 >>")
    add(
        "annots-skip-null-scalar-and-dangling",
        annots=b"[null 42 99 0 R 5 0 R]",
        extra_objs={
            5: b"<< /Subtype /Text /Rect [10 20 30 40] /Contents (Survives) >>",
        },
    )
    add(
        "annots-indirect-array-and-duplicate-ref",
        annots=b"5 0 R",
        extra_objs={
            5: b"[6 0 R 6 0 R]",
            6: b"<< /Subtype /Stamp /Rect [50 60 150 110] /NM (dup-1) >>",
        },
    )

    # 2. Subtypes, flags, inverted/missing /Rect, and /Border
    add(
        "subtypes-core-catalog",
        annots=(
            b"[<< /Subtype /Text /Rect [10 10 30 30] /F 4 >> "
            b"<< /Subtype /FreeText /Rect [40 10 120 40] /F 68 >> "
            b"<< /Subtype /Square /Rect [10 50 60 90] >> "
            b"<< /Subtype /Circle /Rect [70 50 120 90] >> "
            b"<< /Subtype /Caret /Rect [130 50 145 70] >> "
            b"<< /Subtype /Popup /Rect [150 50 250 120] >> "
            b"<< /Subtype /FileAttachment /Rect [10 100 30 120] >> "
            b"<< /Subtype /Widget /Rect [40 100 140 125] >> "
            b"<< /Subtype /Redact /Rect [150 100 240 125] >> "
            b"<< /Subtype /CustomUnknown /Rect [0 0 5 5] >>]"
        ),
    )
    add(
        "rect-inverted-missing-and-short",
        annots=(
            b"[<< /Subtype /Highlight /Rect [200 300 50 80] >> "
            b"<< /Subtype /Text >> "
            b"<< /Subtype /Text /Rect [10 20 30] >> "
            b"<< /Rect [1 2 3 4] >>]"
        ),
    )
    add(
        "border-legacy-three-four-and-short",
        annots=(
            b"[<< /Subtype /Link /Rect [10 20 80 40] /Border [2 4 1.5] >> "
            b"<< /Subtype /Link /Rect [10 50 80 70] /Border [0 0 2 [4 2]] >> "
            b"<< /Subtype /Link /Rect [10 80 80 100] /Border [1 2] >>]"
        ),
    )

    # 3. Text decoding (/Contents, /T, /NM: ASCII, UTF-16BE BOM, PDFDocEncoding, Name)
    add(
        "strings-ascii-utf16be-and-name",
        annots=(
            b"[<< /Subtype /Text /Rect [10 10 30 30] /T (Alice) /NM (note-1) "
            b"/Contents <FEFF00480069002020140020004F004B> >> "
            b"<< /Subtype /Text /Rect [40 10 60 30] /T /NameAuthor /Contents (Plain note) >>]"
        ),
    )

    # 4. Text-markup and Link QuadPoints (including trailing incomplete elements)
    add(
        "quadpoints-text-markup-and-link",
        annots=(
            b"[<< /Subtype /Highlight /Rect [72 700 200 720] "
            b"/QuadPoints [72 720 200 720 72 700 200 700] >> "
            b"<< /Subtype /Underline /Rect [72 660 200 695] "
            b"/QuadPoints [72 695 200 695 72 680 200 680 72 675 150 675 72 660 150 660 999] >> "
            b"<< /Subtype /Squiggly /Rect [72 640 140 655] "
            b"/QuadPoints [72 655 140 655 72 640 140 640] >> "
            b"<< /Subtype /StrikeOut /Rect [72 620 140 635] "
            b"/QuadPoints [72 635 140 635 72 620 140 620] >> "
            b"<< /Subtype /Link /Rect [72 590 160 610] "
            b"/QuadPoints [72 610 160 610 72 590 160 590] /Dest [4 0 R /Fit] >>]"
        ),
    )

    # 5. Line, Polygon, PolyLine, and multi-stroke Ink geometry
    add(
        "geom-line-present-and-missing",
        annots=(
            b"[<< /Subtype /Line /Rect [10 20 110 80] /L [15 25 105 75] >> "
            b"<< /Subtype /Line /Rect [10 20 110 80] >>]"
        ),
    )
    add(
        "geom-polygon-and-polyline-vertices",
        annots=(
            b"[<< /Subtype /Polygon /Rect [10 10 90 90] /Vertices [10 10 90 10 50 85] >> "
            b"<< /Subtype /PolyLine /Rect [10 100 90 160] /Vertices [10 100 40 150 70 110 90 160] >>]"
        ),
    )
    add(
        "geom-ink-multiple-strokes",
        annots=(
            b"[<< /Subtype /Ink /Rect [0 0 200 200] "
            b"/InkList [[10 10 20 25 35 20] [50 60 80 90]] >>]"
        ),
    )

    # 6. Link explicit destinations across all fit modes + integer page target
    add(
        "link-explicit-dest-fit-modes",
        annots=(
            b"[<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /XYZ 36 720 1.25] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /XYZ null 500 null] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /Fit] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /FitH 640] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /FitV 72] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /FitR 100 200 10 20] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /FitB] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /FitBH 600] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [4 0 R /FitBV 48] >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /Dest [2 /Fit] >>]"
        ),
        page_count=3,
    )

    # 7. Named destinations in /Names/Dests and /Dests, including precedence & cross-lookup
    add(
        "link-named-dests-tree-and-dict",
        annots=(
            b"[<< /Subtype /Link /Rect [0 0 10 10] /Dest (chap1) >> "
            b"<< /Subtype /Link /Rect [0 0 10 10] /Dest /legacy_sec >> "
            b"<< /Subtype /Link /Rect [0 0 10 10] /Dest (shared) >> "
            b"<< /Subtype /Link /Rect [0 0 10 10] /Dest /in_tree_by_name >> "
            b"<< /Subtype /Link /Rect [0 0 10 10] /Dest (in_dict_by_str) >> "
            b"<< /Subtype /Link /Rect [0 0 10 10] /Dest (missing_dest) >>]"
        ),
        catalog_extra=(
            b"/Names << /Dests << /Names ["
            b"(chap1) << /D [4 0 R /XYZ 0 700 null] >> "
            b"(in_tree_by_name) [4 0 R /FitB] "
            b"(shared) [4 0 R /Fit]"
            b"] >> >> "
            b"/Dests << "
            b"/legacy_sec [5 0 R /FitH 500] "
            b"/shared [5 0 R /Fit] "
            b"/in_dict_by_str << /D [5 0 R /FitV 36] >> "
            b">>"
        ),
        page_count=3,
    )

    # 8. Actions (/URI, /GoTo, /GoToR, /Launch, /Named)
    add(
        "link-actions-uri-goto-gotor-launch-named",
        annots=(
            b"[<< /Subtype /Link /Rect [0 0 20 20] /A << /S /URI /URI (https://example.org/doc?a=1&b=2) >> >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /A << /S /GoTo /D [4 0 R /XYZ 10 20 0] >> >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /A << /S /GoToR /F (other.pdf) /D [0 /Fit] >> >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /A << /S /Launch /F << /Type /Filespec /UF (report.pdf) /F (rep.pdf) >> >> >> "
            b"<< /Subtype /Link /Rect [0 0 20 20] /A << /S /Named /N /NextPage >> >>]"
        ),
    )

    # 9. Page flattening (`FPDFPage_Flatten` display vs print, BBox + Matrix placement,
    #    graphics-state isolation over unclosed page CTM, and stateful /AP /AS selection)
    flatten_objs = {
        # Existing page content leaves CTM scaled by 3x without `Q`: flattening must isolate it!
        10: stream_obj(b"<<", b"3 0 0 3 0 0 cm 0 0 10 10 re f"),
        # Form with BBox [0 0 20 10] and Matrix [2 0 0 2 10 20] placed onto Rect [100 200 180 240]
        11: stream_obj(
            b"<< /Type /XObject /Subtype /Form /BBox [0 0 20 10] /Matrix [2 0 0 2 10 20]",
            b"0 0 20 10 re f",
        ),
        # Screen-only annotation (`F = 0`, no Print bit)
        12: stream_obj(
            b"<< /Type /XObject /Subtype /Form /BBox [0 0 15 15]",
            b"0 0 15 15 re S",
        ),
        # Hidden annotation (`F = 2`, Hidden bit: drawn in neither Display nor Print)
        13: stream_obj(
            b"<< /Type /XObject /Subtype /Form /BBox [0 0 10 10]",
            b"0 0 10 10 re f",
        ),
        # Stateful checkbox `/AP /N << /Off 14 0 R /Yes 15 0 R >>` with `/AS /Yes` and `F = 4` (Print)
        14: stream_obj(
            b"<< /Type /XObject /Subtype /Form /BBox [0 0 12 12]",
            b"0 0 12 12 re S",
        ),
        15: stream_obj(
            b"<< /Type /XObject /Subtype /Form /BBox [0 0 12 12]",
            b"2 2 8 8 re f",
        ),
    }
    flatten_annots = (
        b"[<< /Subtype /Stamp /Rect [100 200 180 240] /F 4 /AP << /N 11 0 R >> >> "
        b"<< /Subtype /Square /Rect [300 300 330 330] /F 0 /AP << /N 12 0 R >> >> "
        b"<< /Subtype /Circle /Rect [400 400 420 420] /F 2 /AP << /N 13 0 R >> >> "
        b"<< /Subtype /Widget /Rect [50 400 74 424] /F 4 /AS /Yes /AP << /N << /Off 14 0 R /Yes 15 0 R >> >> >>]"
    )
    add(
        "flatten-display-matrix-isolation-and-state",
        annots=flatten_annots,
        extra_objs=flatten_objs,
        page_extra=b"/Contents 10 0 R",
        page_count=1,
        kind="annots-flatten-display",
    )
    add(
        "flatten-print-flags-and-state",
        annots=flatten_annots,
        extra_objs=flatten_objs,
        page_extra=b"/Contents 10 0 R",
        page_count=1,
        kind="annots-flatten-print",
    )

    return list(ROWS)
