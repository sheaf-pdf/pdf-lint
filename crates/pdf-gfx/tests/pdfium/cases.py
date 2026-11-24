"""pdf-gfx's PDFium oracle cases (`gfx-geom` and `gfx`).

Written using Python's standard library alone; `oracle/pdfium.py cases --write`
materializes `cases/*.pdf` and `cases.tsv`, and `oracle/pdfium.py record`
records `chromium-8152.txt`."""

ROWS = []

COURIER = b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>"
HELVETICA = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"


def stream(dict_entries, data):
    head = (b"<< " + dict_entries + b" " if dict_entries else b"<< ")
    return head + b"/Length %d >>\nstream\n" % len(data) + data + b"\nendstream"


def build_pdf(page_entries=b"/MediaBox [0 0 200 200]", pages_entries=b"", content=None, resources=b"<< >>", extra=None):
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1" + (b" " + pages_entries if pages_entries else b"") + b" >>",
    }
    page = bytearray(b"<< /Type /Page /Parent 2 0 R")
    if page_entries:
        page += b" " + page_entries
    if content is not None:
        page += b" /Contents 4 0 R"
        objects[4] = stream(b"", content)
    if resources is not None:
        page += b" /Resources " + resources
    page += b" >>"
    objects[3] = bytes(page)
    if extra:
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
        if number in offsets:
            out += b"%010d 00000 n \n" % offsets[number]
        else:
            out += b"0000000000 65535 f \n"
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (size, xref)
    return bytes(out)


def add_geom(name, page_entries=b"", pages_entries=b"", extra=None):
    pdf = build_pdf(page_entries=page_entries, pages_entries=pages_entries, content=None, resources=b"<< >>", extra=extra)
    ROWS.append((name, "gfx-geom", {"pdf": pdf}))


def add_gfx(name, content, resources=b"<< >>", extra=None, page_entries=b"/MediaBox [0 0 200 200]"):
    pdf = build_pdf(page_entries=page_entries, content=content, resources=resources, extra=extra)
    ROWS.append((name, "gfx", {"pdf": pdf}))


def add_gfx_text(
    name,
    content,
    resources=None,
    extra=None,
    page_entries=b"/MediaBox [0 0 400 800]",
    queries=None,
):
    if resources is None:
        resources = b"<< /Font << /F1 " + HELVETICA + b" /F2 " + COURIER + b" >> >>"
    files = {
        "pdf": build_pdf(
            page_entries=page_entries,
            content=content,
            resources=resources,
            extra=extra,
        )
    }
    if queries:
        files["queries"] = ("\n".join("%s\t%d" % (q, fl) for q, fl in queries) + "\n").encode("utf-8")
    ROWS.append((name, "gfx-text", files))


def geom_cases():
    add_geom("geom-letter-default", b"/MediaBox [0 0 612 792]")
    add_geom("geom-rotate-90", b"/MediaBox [0 0 200 300] /Rotate 90")
    add_geom("geom-rotate-180", b"/MediaBox [0 0 200 300] /Rotate 180")
    add_geom("geom-rotate-270", b"/MediaBox [0 0 200 300] /Rotate 270")
    add_geom("geom-rotate-neg90", b"/MediaBox [0 0 200 300] /Rotate -90")
    add_geom("geom-rotate-450", b"/MediaBox [0 0 200 300] /Rotate 450")
    add_geom("geom-rotate-45-nonquarter", b"/MediaBox [0 0 200 300] /Rotate 45")
    add_geom("geom-cropbox-inside", b"/MediaBox [0 0 400 500] /CropBox [20 30 220 330] /Rotate 270")
    add_geom("geom-cropbox-clipped", b"/MediaBox [10 20 210 320] /CropBox [-50 0 250 400] /Rotate 90")
    add_geom("geom-reversed-coords", b"/MediaBox [220 330 20 30] /Rotate 180")
    add_geom(
        "geom-inherited-media-and-rotate",
        page_entries=b"/CropBox [20 30 220 330]",
        pages_entries=b"/MediaBox [0 0 400 500] /Rotate 90",
    )
    add_geom("geom-missing-boxes-fallback", b"")
    add_geom("geom-zero-area-crop-fallback", b"/MediaBox [0 0 300 400] /CropBox [50 50 50 100]")


def path_and_state_cases():
    add_gfx(
        "ctm-compose-and-stack",
        b"q 2 0 0 3 10 20 cm 1 0 0 1 5 6 cm 1 2 3 4 re f Q 10 20 5 5 re f Q 30 40 5 5 re f",
    )
    add_gfx(
        "path-survives-save-restore",
        b"10 20 m q 2 0 0 2 50 60 cm 30 40 l Q 2 w S",
    )
    add_gfx(
        "path-curves-and-paint-modes",
        b"10 10 m 20 30 40 30 50 10 c 60 25 70 10 v 80 25 90 10 y h B "
        b"0 0 m 20 0 l 10 20 l b* "
        b"100 100 m 120 100 l 110 120 l 100 100 l b*",
    )
    add_gfx(
        "path-fast-buffer-reuse",
        b"10 20 m 1 2 3 4 30 40 c 50 l S",
    )
    add_gfx(
        "path-single-point-caps",
        b"q 4 w 1 J 50 50 m h S Q "
        b"q 4 w 0 J 60 60 m h S Q "
        b"q 4 w 2 J 70 70 m h S Q "
        b"q 4 w 1 J 80 80 m S Q",
    )
    add_gfx(
        "path-stroke-styles-dash-and-gs",
        b"q 3 w 2 J 1 j [4 2] 1 d 10 10 m 40 10 l 40 30 l S Q "
        b"q /GS1 gs 10 50 m 40 50 l S Q",
        resources=b"<< /ExtGState << /GS1 << /Type /ExtGState /LW 5 /LC 1 /LJ 2 /D [[6 3] 2] >> >> >>",
    )


def clip_cases():
    add_gfx(
        "clip-rect-and-evenodd-polygon",
        b"10 20 80 90 re W n 0 0 m 100 0 l 50 100 l h W* n 0 0 200 200 re f",
    )
    add_gfx(
        "clip-auto-merge-inner-replaces-outer",
        b"10 20 100 100 re W n 30 40 40 40 re W n 0 0 200 200 re f",
    )
    add_gfx(
        "clip-auto-merge-outer-keeps-inner",
        b"30 40 40 40 re W n 10 20 100 100 re W n 0 0 200 200 re f",
    )
    add_gfx(
        "clip-wholly-inside-single-rect",
        b"10 20 100 100 re W n 30 40 10 10 re f",
    )
    add_gfx(
        "clip-single-point-degenerate",
        b"40 50 m W n 0 0 200 200 re f",
    )
    add_gfx(
        "clip-line-zero-area",
        b"10 20 m 50 20 l W n 0 0 200 200 re f",
    )
    add_gfx(
        "clip-text-render-modes",
        b"q BT /F0 12 Tf 4 Tr 10 20 Td (Hi) Tj ET 0 0 200 200 re f Q "
        b"q BT /F0 12 Tf 7 Tr 10 60 Td ( Clip ) Tj ET 0 0 200 200 re f Q",
        resources=b"<< /Font << /F0 " + COURIER + b" >> >>",
    )


def text_cases():
    font_res = b"<< /Font << /F0 " + COURIER + b" /F1 " + HELVETICA + b" >> >>"
    add_gfx(
        "text-line-moves-and-quotes",
        b"BT /F0 10 Tf 14 TL 20 160 Td (AB) Tj 5 -12 TD (CD) Tj T* (EF) Tj (GH) ' 2 1 (IJ) \" ET",
        resources=font_res,
    )
    add_gfx(
        "text-matrix-scale-spacing-rise",
        b"BT /F0 10 Tf 1 Tc 3 Tw 150 Tz 4 Ts 1 0 0 1 15 80 Tm (A B) Tj 80 999 Tz (C) Tj ET",
        resources=font_res,
    )
    add_gfx(
        "text-tj-empty-string-quirk",
        b"BT /F0 10 Tf 10 100 Td [(A) 200 () 300 (B) 500 ()] TJ (C) Tj ET",
        resources=font_res,
    )
    add_gfx(
        "text-tj-kerning-and-empty-ops",
        b"BT /F0 10 Tf 10 100 Td [() 100 ()] TJ () Tj [(A) 50 (B) -50 (C)] TJ ET",
        resources=font_res,
    )
    add_gfx(
        "text-show-name-operands",
        b"BT /F0 10 Tf 14 TL 10 160 Td /AB Tj /CD ' 1 2 /EF \" [(G) /HI (J)] TJ /K#4CM Tj ET",
        resources=font_res,
    )
    add_gfx(
        "text-show-boolean-operands",
        b"BT true 10 Tf 14 TL 10 160 Td true Tj false ' 1 2 true \" [(A) true (B) false] TJ ET",
        resources=b"<< /Font << /true " + COURIER + b" >> >>",
    )
    add_gfx(
        "text-missing-font-uses-stock-helvetica",
        b"BT /Missing 10 Tf 10 50 Td (Hello) Tj ET",
        resources=b"<< >>",
    )
    add_gfx(
        "text-extgstate-font-and-unloaded-tf",
        b"BT /GSRef gs 10 140 Td (AB) Tj /GSName gs 0 -20 Td (CD) Tj /Bad 9 Tf 0 -20 Td (EF) Tj /GSBad gs (GH) Tj ET",
        resources=(
            b"<< /Font << /F0 " + COURIER + b" /Bad << /Type /Font /Subtype /Type0 /BaseFont /Bad >> >> "
            b"/ExtGState << /GSRef << /Font [10 0 R 14] >> /GSName << /Font [/F0 12] >> /GSBad << /Font [/Bad 8] >> >> >>"
        ),
        extra={
            10: COURIER,
        },
    )
    add_gfx(
        "text-vertical-identity-v",
        b"BT /FV 10 Tf 60 180 Td <00410042> Tj ET",
        resources=b"<< /Font << /FV 10 0 R >> >>",
        extra={
            10: (
                b"<< /Type /Font /Subtype /Type0 /BaseFont /VertFont "
                b"/Encoding /Identity-V /DescendantFonts [11 0 R] >>"
            ),
            11: (
                b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /VertFont "
                b"/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> "
                b"/DW2 [880 -1000] >>"
            ),
        },
    )


def xobject_and_marked_cases():
    shading_dict = (
        b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 10 10] "
        b"/Function << /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >> >>"
    )
    add_gfx(
        "form-nested-matrix-marks-and-shading",
        b"/Outer BMC q 1 0 0 1 5 6 cm /Fm1 Do Q EMC",
        resources=(
            b"<< /Font << /F0 " + COURIER + b" >> "
            b"/Shading << /Sh1 " + shading_dict + b" >> "
            b"/XObject << /Fm1 10 0 R >> >>"
        ),
        extra={
            10: stream(
                b"/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [2 0 0 2 10 20]",
                b"/Inner << /MCID 3 >> BDC 2 w 1 J 1 j 0 0 m 10 0 l S BT /F0 10 Tf 3 4 Td (AB) Tj ET /Sh1 sh EMC",
            ),
        },
    )
    add_gfx(
        "form-explicit-empty-resources-no-inherit",
        b"BT /F0 10 Tf 10 150 Td (MM) Tj ET /Fm1 Do",
        resources=b"<< /Font << /F0 " + COURIER + b" >> /XObject << /Fm1 10 0 R >> >>",
        extra={
            10: stream(
                b"/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Resources << >>",
                b"BT /F0 10 Tf 10 100 Td (MM) Tj ET",
            ),
        },
    )
    chain_extra = {}
    for idx in range(42):
        obj_num = 10 + idx
        if idx + 1 < 42:
            body = (
                b"0 0 2 2 re f /Fm%d Do" % (idx + 1)
            )
            res = b" /Resources << /XObject << /Fm%d %d 0 R >> >>" % (idx + 1, obj_num + 1)
        else:
            body = b"0 0 2 2 re f"
            res = b""
        chain_extra[obj_num] = stream(
            b"/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Matrix [1 0 0 1 3 0]" + res,
            body,
        )
    add_gfx(
        "form-depth-chain-cap",
        b"/Fm0 Do",
        resources=b"<< /XObject << /Fm0 10 0 R >> >>",
        extra=chain_extra,
    )
    add_gfx(
        "form-self-and-mutual-cycles",
        b"/Self Do /MutualA Do",
        resources=b"<< /XObject << /Self 10 0 R /MutualA 11 0 R /MutualB 12 0 R >> >>",
        extra={
            10: stream(
                b"/Type /XObject /Subtype /Form /BBox [0 0 200 200]",
                b"1 2 3 4 re f /Self Do",
            ),
            11: stream(
                b"/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Matrix [1 0 0 1 10 0]",
                b"5 5 2 2 re f /MutualB Do",
            ),
            12: stream(
                b"/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Matrix [1 0 0 1 0 10]",
                b"8 8 2 2 re f /MutualA Do",
            ),
        },
    )
    add_gfx(
        "images-xobject-and-inline",
        b"q 20 0 0 30 15 25 cm /Im1 Do Q "
        b"q 12 0 0 18 50 60 cm BI /W 2 /H 3 /BPC 8 /CS /G ID abcdef EI Q",
        resources=b"<< /XObject << /Im1 10 0 R >> >>",
        extra={
            10: stream(
                b"/Type /XObject /Subtype /Image /Width 4 /Height 5 /BitsPerComponent 8 /ColorSpace /DeviceGray",
                b"\x00" * 20,
            ),
        },
    )
    add_gfx(
        "images-inline-broken-filter",
        b"q 10 0 0 10 90 90 cm BI /W 1 /H 1 /BPC 8 /CS /G /F /UnknownFilter ID x EI Q",
    )
    add_gfx(
        "marks-named-props-bad-bdc-and-unclosed",
        b"/Span BMC /P /P1 BDC /Bad (notadict) BDC 10 10 5 5 re f EMC 20 20 5 5 re f EMC /Unclosed << /MCID 9 >> BDC 30 30 5 5 re f",
        resources=b"<< /Properties << /P1 << /MCID 4 >> >> >>",
    )


def text_layout_cases():
    add_gfx_text(
        "layout-two-lines-and-search",
        b"BT /F1 12 Tf 72 700 Td (Hello) Tj 0 -16 Td (World) Tj ET",
        queries=[("hello world", 0), ("Hello", 1), ("hello", 1), ("Hell", 2), ("World", 2)],
    )
    add_gfx_text(
        "layout-tj-space-threshold-below",
        b"BT /F1 10 Tf 72 700 Td [(A) -138 (B)] TJ ET",
    )
    add_gfx_text(
        "layout-tj-space-threshold-at",
        b"BT /F1 10 Tf 72 700 Td [(A) -139 (B)] TJ ET",
    )
    add_gfx_text(
        "layout-separate-tm-word-gap",
        b"BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hi) Tj 1 0 0 1 90 700 Tm (There) Tj ET",
        queries=[("hi there", 0)],
    )
    add_gfx_text(
        "layout-two-columns-table-row",
        b"BT /F1 12 Tf 1 0 0 1 72 700 Tm (Item) Tj 1 0 0 1 250 700 Tm ($10.00) Tj ET",
        queries=[("item $10.00", 0)],
    )
    add_gfx_text(
        "layout-same-line-reverse-x",
        b"BT /F1 12 Tf 1 0 0 1 120 700 Tm (World) Tj 1 0 0 1 72 700 Tm (Hello) Tj ET",
        queries=[("Hello World", 1)],
    )
    add_gfx_text(
        "layout-faux-bold-across-tj",
        b"BT /F1 12 Tf 1 0 0 1 72 700 Tm (AB) Tj 1 0 0 1 72.3 700 Tm (AB) Tj ET",
    )
    add_gfx_text(
        "layout-consecutive-spaces-collapsed",
        b"BT /F1 12 Tf 72 700 Td (A   B) Tj ET",
    )
    add_gfx_text(
        "layout-actual-text-single-and-multi-op",
        b"/Span << /ActualText (Replaced) >> BDC BT /F1 12 Tf 72 700 Td (Orig) Tj ET EMC "
        b"/Span << /ActualText (fi) >> BDC BT /F1 12 Tf 140 700 Td (f) Tj 5 0 Td (i) Tj ET EMC",
        queries=[("replaced fi", 0)],
    )
    add_gfx_text(
        "layout-hyphen-wrap-alpha-vs-digit",
        b"BT /F1 12 Tf 72 700 Td (inter-) Tj 0 -16 Td (national) Tj 0 -16 Td (2024-) Tj 0 -16 Td (2025) Tj ET",
        queries=[("inter", 0), ("national", 0), ("2024-", 0)],
    )
    add_gfx_text(
        "layout-soft-hyphen-wrap",
        b"BT /F1 12 Tf 72 700 Td (inter\\255) Tj 0 -16 Td (national) Tj ET",
    )
    add_gfx_text(
        "layout-rotated-90-two-lines",
        b"BT /F1 12 Tf 0 1 -1 0 100 200 Tm (Rot1) Tj 0 1 -1 0 116 200 Tm (Rot2) Tj ET",
        queries=[("rot1 rot2", 0)],
    )
    add_gfx_text(
        "layout-consecutive-overlap-search",
        b"BT /F1 12 Tf 72 700 Td (aaaa) Tj ET",
        queries=[("aa", 0), ("aa", 4)],
    )
    heb_cmap = stream(
        b"",
        b"1 begincodespacerange <00> <FF> endcodespacerange\n"
        b"9 beginbfchar\n"
        b"<01> <05DD>\n<02> <05D5>\n<03> <05DC>\n<04> <05E9>\n"
        b"<20> <0020>\n<31> <0031>\n<32> <0032>\n<33> <0033>\n<41> <0041>\n"
        b"endbfchar",
    )
    heb_widths = b"[" + b" ".join([b"500"] * 66) + b"]"
    heb_font = (
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Widths "
        + heb_widths
        + b" /FirstChar 0 /LastChar 65 /ToUnicode 10 0 R >>"
    )
    add_gfx_text(
        "layout-bidi-rtl-and-numbers",
        b"BT /F1 12 Tf 72 700 Td <01020304> Tj 0 -16 Td <3132332001020304> Tj 0 -16 Td <4120010203042041> Tj ET",
        resources=b"<< /Font << /F1 " + heb_font + b" >> >>",
        extra={10: heb_cmap},
        queries=[("\u05e9\u05dc\u05d5\u05dd 123", 0)],
    )


def cases():
    del ROWS[:]
    geom_cases()
    path_and_state_cases()
    clip_cases()
    text_cases()
    xobject_and_marked_cases()
    text_layout_cases()
    return list(ROWS)
