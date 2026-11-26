"""Extract or verify PDFium's 9x9x9x9 unprofiled DeviceCMYK -> sRGB table
(`crates/pdf-color/src/cmyk_lut.rs`) against a live `libpdfium` binary using
Python's standard library (`ctypes` + `zlib`) alone.

Usage:
    python3 -I -B oracle/cmyk_lut.py check /path/to/libpdfium.so
    python3 -I -B oracle/cmyk_lut.py print /path/to/libpdfium.so
"""

import ctypes
import re
import sys
import zlib

sys.dont_write_bytecode = True

FPDF_REVERSE_BYTE_ORDER = 0x10
FPDF_RENDER_NO_SMOOTHPATH = 0x4000
FPDFBitmap_BGRA = 4


def build_swatch_pdf(side, coords):
    """Build a single-page PDF (`side x side` points) painting `len(coords)`
    1x1 pt vector rectangles in `DeviceCMYK` (`k` + `re f`), row 0 at top."""
    lines = []
    for idx, (c, m, y, k) in enumerate(coords):
        col = idx % side
        row = idx // side
        # PDF PageSpace y=0 is bottom; bitmap row=0 is top (`y = side - 1 - row`).
        py = side - 1 - row
        lines.append(f"{c:.9g} {m:.9g} {y:.9g} {k:.9g} k {col} {py} 1 1 re f")
    raw = ("\n".join(lines) + "\n").encode("ascii")
    comp = zlib.compress(raw, 6)

    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        f"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 {side} {side}] >>".encode("ascii"),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
        f"<< /Filter /FlateDecode /Length {len(comp)} >>\nstream\n".encode("ascii")
        + comp
        + b"\nendstream",
    ]
    out = bytearray(b"%PDF-1.7\n")
    offsets = []
    for i, body in enumerate(objs, 1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n".encode("ascii") + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode("ascii")
    for off in offsets:
        out += f"{off:010d} 00000 n \n".encode("ascii")
    out += (
        f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode(
            "ascii"
        )
    )
    return bytes(out)


def render_pdfium_grid(lib_path, side, coords):
    lib = ctypes.CDLL(lib_path)
    lib.FPDF_InitLibrary()
    lib.FPDF_LoadMemDocument.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_char_p]
    lib.FPDF_LoadMemDocument.restype = ctypes.c_void_p
    lib.FPDF_CloseDocument.argtypes = [ctypes.c_void_p]
    lib.FPDF_LoadPage.argtypes = [ctypes.c_void_p, ctypes.c_int]
    lib.FPDF_LoadPage.restype = ctypes.c_void_p
    lib.FPDF_ClosePage.argtypes = [ctypes.c_void_p]
    lib.FPDFBitmap_CreateEx.argtypes = [
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_void_p,
        ctypes.c_int,
    ]
    lib.FPDFBitmap_CreateEx.restype = ctypes.c_void_p
    lib.FPDFBitmap_Destroy.argtypes = [ctypes.c_void_p]
    lib.FPDF_RenderPageBitmap.argtypes = [
        ctypes.c_void_p,
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
    ]

    pdf_bytes = build_swatch_pdf(side, coords)
    pdf_buf = ctypes.create_string_buffer(pdf_bytes, len(pdf_bytes))
    doc = lib.FPDF_LoadMemDocument(pdf_buf, len(pdf_bytes), None)
    if not doc:
        raise SystemExit("FPDF_LoadMemDocument failed")
    try:
        page = lib.FPDF_LoadPage(doc, 0)
        if not page:
            raise SystemExit("FPDF_LoadPage failed")
        try:
            pix_buf = (ctypes.c_ubyte * (side * side * 4))()
            bmp = lib.FPDFBitmap_CreateEx(
                side, side, FPDFBitmap_BGRA, ctypes.cast(pix_buf, ctypes.c_void_p), side * 4
            )
            if not bmp:
                raise SystemExit("FPDFBitmap_CreateEx failed")
            try:
                lib.FPDF_RenderPageBitmap(
                    bmp,
                    page,
                    0,
                    0,
                    side,
                    side,
                    0,
                    FPDF_REVERSE_BYTE_ORDER | FPDF_RENDER_NO_SMOOTHPATH,
                )
            finally:
                lib.FPDFBitmap_Destroy(bmp)
            raw = bytes(pix_buf)
            return [[raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2]] for i in range(len(coords))]
        finally:
            lib.FPDF_ClosePage(page)
    finally:
        lib.FPDF_CloseDocument(doc)
        lib.FPDF_DestroyLibrary()


def parse_rust_lut(path="crates/pdf-color/src/cmyk_lut.rs"):
    with open(path, "r", encoding="utf-8") as fh:
        text = fh.read()
    entries = re.findall(r"\[(\d+),\s*(\d+),\s*(\d+)\]", text)
    return [[int(r), int(g), int(b)] for r, g, b in entries]


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in ("check", "print"):
        raise SystemExit(__doc__.strip())
    mode, lib_path = sys.argv[1], sys.argv[2]
    coords = [
        (ic / 8.0, im / 8.0, iy / 8.0, ik / 8.0)
        for ic in range(9)
        for im in range(9)
        for iy in range(9)
        for ik in range(9)
    ]
    table = render_pdfium_grid(lib_path, 81, coords)
    if mode == "print":
        for row_start in range(0, len(table), 8):
            chunk = ", ".join(f"[{r}, {g}, {b}]" for r, g, b in table[row_start : row_start + 8])
            print(f"    {chunk},")
        return

    recorded = parse_rust_lut()
    if len(recorded) != 6561:
        raise SystemExit(f"expected 6561 entries in cmyk_lut.rs, found {len(recorded)}")
    mismatches = [i for i, (a, b) in enumerate(zip(recorded, table)) if a != b]
    if mismatches:
        first = mismatches[0]
        raise SystemExit(
            f"{len(mismatches)} grid entries differ (first at {first}: rust={recorded[first]} vs pdfium={table[first]})"
        )
    print("crates/pdf-color/src/cmyk_lut.rs: 6561 grid entries match PDFium exactly")


if __name__ == "__main__":
    main()
