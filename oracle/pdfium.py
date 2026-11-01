"""PDFium as the crates' oracle: each case of a set asked of a PDFium
library through ctypes, its answer one line, held to the answer recorded
at the pinned build. The standard library alone; see README.md.

    pdfium.py run LIB SET...               print each case's answer
    pdfium.py check LIB SET... [--build N] each answer against the recorded one
    pdfium.py record LIB SET... --build N  write each set's chromium-N.txt
    pdfium.py cases SET... [--write]       each set's generator against its cases

A set is a crate's tests/pdfium folder: cases.py, the generator, writes
the cases (cases/ and cases.tsv); chromium-N.txt holds PDFium's answers
at the pinned build, chromium/N. Run it as `python3 -I -B`."""

import ctypes
import importlib.util
import os
import re
import struct
import sys
import tempfile

sys.dont_write_bytecode = True

# A case's text or data longer than this is written by its length and
# FNV-1a 64 hash, not byte by byte.
LONG = 64

COURIER = b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>"
FONTS = b"<< /Font << /F0 " + COURIER + b" >> >>"
# How many pages of a file are asked for, whatever count it gives.
PAGES = 64

TYPES = {1: "text", 2: "path", 3: "image", 4: "shading", 5: "form"}
SEGMENTS = {0: "l", 1: "c", 2: "m"}


def fnv(data):
    h = 0xCBF29CE484222325
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return "%016x" % h


def esc(data):
    """Bytes as one word: printable ASCII as it is but `%`, any other byte
    `%XX`; none, `%` alone; past LONG bytes, `#` its length `:` its hash."""
    if not data:
        return "%"
    if len(data) > LONG:
        return "#%d:%s" % (len(data), fnv(data))
    return "".join(chr(b) if 0x21 <= b <= 0x7E and b != 0x25 else "%%%02X" % b for b in data)


def num(x):
    """A number as nine significant digits, which read back to the same
    f32."""
    return "%.9g" % x


def pdf(content=None, resources=b"<< >>", mediabox=b"[0 0 200 200]", info=None, kids=b"3 0 R"):
    """A one-page PDF: `content` its content stream (none if None), `info`
    the trailer's /Info dictionary (none if None), `kids` the page tree's
    /Kids as written, its page object 3."""
    page = (b"<< /Type /Page /Parent 2 0 R /MediaBox " + mediabox
            + (b" /Contents 4 0 R" if content is not None else b"")
            + b" /Resources " + resources + b" >>")
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [" + kids + b"] /Count 1 >>", page]
    if content is not None:
        objs.append(b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream")
    if info is not None:
        objs.append(info)
    out = bytearray(b"%PDF-1.7\n")
    offsets = []
    for i, o in enumerate(objs, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    for o in offsets:
        out += b"%010d 00000 n \n" % o
    trailer = b" /Info %d 0 R" % len(objs) if info is not None else b""
    out += b"trailer\n<< /Size %d /Root 1 0 R%s >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objs) + 1, trailer, xref)
    return bytes(out)


class M(ctypes.Structure):
    _fields_ = [(n, ctypes.c_float) for n in "abcdef"]


class Pdfium:
    """A PDFium library, loaded by its path."""

    def __init__(self, path):
        lib = self.lib = ctypes.CDLL(os.path.abspath(path))
        P, I, U, F, S = ctypes.c_void_p, ctypes.c_int, ctypes.c_ulong, ctypes.c_float, ctypes.c_size_t
        pf, pi = ctypes.POINTER(F), ctypes.POINTER(I)
        for name, res, args in [
            ("FPDF_InitLibrary", None, []),
            ("FPDF_LoadMemDocument", P, [P, I, ctypes.c_char_p]),
            ("FPDF_CloseDocument", None, [P]),
            ("FPDF_LoadPage", P, [P, I]),
            ("FPDF_ClosePage", None, [P]),
            ("FPDF_GetPageWidthF", F, [P]),
            ("FPDF_GetPageHeightF", F, [P]),
            ("FPDF_GetPageCount", I, [P]),
            ("FPDFPage_GetRotation", I, [P]),
            ("FPDF_GetMetaText", U, [P, ctypes.c_char_p, P, U]),
            ("FPDFPage_CountObjects", I, [P]),
            ("FPDFPage_GetObject", P, [P, I]),
            ("FPDFPageObj_GetType", I, [P]),
            ("FPDFPageObj_GetMatrix", I, [P, ctypes.POINTER(M)]),
            ("FPDFPageObj_CountMarks", I, [P]),
            ("FPDFPageObj_GetMark", P, [P, U]),
            ("FPDFPageObjMark_GetParamIntValue", I, [P, ctypes.c_char_p, pi]),
            ("FPDFPageObj_GetDashCount", I, [P]),
            ("FPDFPageObj_GetDashArray", I, [P, pf, S]),
            ("FPDFPageObj_GetDashPhase", I, [P, pf]),
            ("FPDFPath_CountSegments", I, [P]),
            ("FPDFPath_GetPathSegment", P, [P, I]),
            ("FPDFPathSegment_GetPoint", I, [P, pf, pf]),
            ("FPDFPathSegment_GetType", I, [P]),
            ("FPDFPathSegment_GetClose", I, [P]),
            ("FPDFPath_GetDrawMode", I, [P, pi, pi]),
            ("FPDFText_LoadPage", P, [P]),
            ("FPDFText_ClosePage", None, [P]),
            ("FPDFTextObj_GetText", U, [P, P, P, U]),
            ("FPDFTextObj_GetFontSize", I, [P, pf]),
            ("FPDFTextObj_GetFont", P, [P]),
            ("FPDFFont_GetBaseFontName", S, [P, P, S]),
            ("FPDFImageObj_GetImageDataRaw", U, [P, P, U]),
        ]:
            f = getattr(lib, name)
            f.restype, f.argtypes = res, args
        lib.FPDF_InitLibrary()

    def _doc(self, data):
        buf = ctypes.create_string_buffer(data, len(data))
        return buf, self.lib.FPDF_LoadMemDocument(buf, len(data), None)

    def objects(self, data):
        """The page's objects in order, each one line; None where the page
        doesn't load."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        tp = lib.FPDFText_LoadPage(page)
        out = [self._object(lib.FPDFPage_GetObject(page, i), tp)
               for i in range(lib.FPDFPage_CountObjects(page))]
        lib.FPDFText_ClosePage(tp)
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return out

    def _object(self, o, tp):
        lib = self.lib
        t = TYPES.get(lib.FPDFPageObj_GetType(o), "unknown")
        m = M()
        lib.FPDFPageObj_GetMatrix(o, ctypes.byref(m))
        matrix = ["matrix"] + [num(getattr(m, n)) for n in "abcdef"]
        out = [t]
        if t == "path":
            fill, stroke = ctypes.c_int(), ctypes.c_int()
            lib.FPDFPath_GetDrawMode(o, ctypes.byref(fill), ctypes.byref(stroke))
            out += ["fill", str(fill.value), "stroke", str(int(bool(stroke.value)))] + matrix
            for i in range(lib.FPDFPath_CountSegments(o)):
                s = lib.FPDFPath_GetPathSegment(o, i)
                x, y = ctypes.c_float(), ctypes.c_float()
                lib.FPDFPathSegment_GetPoint(s, ctypes.byref(x), ctypes.byref(y))
                out += [SEGMENTS.get(lib.FPDFPathSegment_GetType(s), "?"), num(x.value), num(y.value)]
                if lib.FPDFPathSegment_GetClose(s):
                    out.append("h")
            n = lib.FPDFPageObj_GetDashCount(o)
            if n > 0:
                dash = (ctypes.c_float * n)()
                lib.FPDFPageObj_GetDashArray(o, dash, n)
                phase = ctypes.c_float()
                lib.FPDFPageObj_GetDashPhase(o, ctypes.byref(phase))
                out += ["dash"] + [num(x) for x in dash] + ["phase", num(phase.value)]
        elif t == "image":
            n = lib.FPDFImageObj_GetImageDataRaw(o, None, 0)
            raw = ctypes.create_string_buffer(max(n, 1))
            lib.FPDFImageObj_GetImageDataRaw(o, raw, n)
            out += matrix + ["raw", esc(raw.raw[:n])]
        elif t == "text":
            size = ctypes.c_float()
            lib.FPDFTextObj_GetFontSize(o, ctypes.byref(size))
            font = lib.FPDFTextObj_GetFont(o)
            k = lib.FPDFFont_GetBaseFontName(font, None, 0) if font else 0
            name = ctypes.create_string_buffer(max(k, 1))
            if k:
                lib.FPDFFont_GetBaseFontName(font, name, k)
            n = lib.FPDFTextObj_GetText(o, tp, None, 0)
            text = ctypes.create_string_buffer(max(n, 2))
            lib.FPDFTextObj_GetText(o, tp, text, n)
            shown = text.raw[:max(n - 2, 0)].decode("utf-16le", "replace").encode("utf-8")
            out += ["font", esc(name.value), "size", num(size.value), "text", esc(shown)]
        for k in range(lib.FPDFPageObj_CountMarks(o)):
            v = ctypes.c_int()
            ok = lib.FPDFPageObjMark_GetParamIntValue(lib.FPDFPageObj_GetMark(o, k), b"MCID", ctypes.byref(v))
            out += ["mcid", str(v.value) if ok else "-"]
        return " ".join(out)

    def size(self, data):
        """The page's width and height; None where it doesn't load."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        out = (lib.FPDF_GetPageWidthF(page), lib.FPDF_GetPageHeightF(page))
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return out

    def pages(self, data):
        """What a file's page tree comes to: the count PDFium gives, then, in
        order, each page to two past it, its width, height and quarter turns,
        or None where it doesn't load. None for a file that doesn't open."""
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        count = lib.FPDF_GetPageCount(doc)
        pages = []
        for index in range(min(count, PAGES) + 2):
            page = lib.FPDF_LoadPage(doc, index)
            if not page:
                pages.append(None)
                continue
            pages.append((lib.FPDF_GetPageWidthF(page), lib.FPDF_GetPageHeightF(page),
                          lib.FPDFPage_GetRotation(page)))
            lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return count, pages

    def meta(self, data, tag):
        """The document's /Info entry `tag` as text, UTF-8."""
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        n = lib.FPDF_GetMetaText(doc, tag, None, 0)
        out = ctypes.create_string_buffer(max(n, 2))
        lib.FPDF_GetMetaText(doc, tag, out, n)
        lib.FPDF_CloseDocument(doc)
        return out.raw[:max(n - 2, 0)].decode("utf-16le", "replace").encode("utf-8")


def content_answer(objects):
    if objects is None:
        return "no-page"
    return " ; ".join(objects) or "none"


def size_answer(size):
    return "no-page" if size is None else "%s %s" % (num(size[0]), num(size[1]))


def pages_answer(pages):
    if pages is None:
        return "no-document"
    count, each = pages
    return "count %d : " % count + " ; ".join(
        "null" if p is None else "%s %s %d" % (num(p[0]), num(p[1]), p[2]) for p in each)


def answer(pdfium, kind, files):
    """What PDFium answers for one case, as one line. A case's kind says
    how it's asked (README.md)."""
    if kind == "content":
        return content_answer(pdfium.objects(pdf(files["content"], files.get("res", b"<< >>"))))
    if kind in ("number", "hex", "literal", "name"):
        word = files["word"]
        if kind == "number":
            # The word as `re`'s second: its first point (1, the word),
            # where PDFium reads four numbers; where it reads the word as a
            # keyword, which drops the 1, (0, 0).
            content = b"1 " + word + b" 10 10 re f"
        elif kind == "name":
            content = b"BT " + word + b" 10 Tf 0 0 Td (ab) Tj ET"
        else:
            content = b"BT /F0 10 Tf 0 0 Td " + word + b" Tj ET"
        objects = pdfium.objects(pdf(content, FONTS))
        if not objects:
            return content_answer(objects)
        words = objects[0].split()
        if kind == "number":
            x, y = words[words.index("m") + 1:words.index("m") + 3]
            return y if float(x) == 1 else "not-a-number"
        if kind == "name":
            # F0 is Courier; a name it can't find, PDFium's stock font.
            return "found" if words[words.index("font") + 1] == "Courier" else "not-found"
        return "text " + words[words.index("size") + 3]
    if kind == "mediabox":
        return size_answer(pdfium.size(pdf(None, mediabox=files["obj"])))
    if kind == "kids":
        return size_answer(pdfium.size(pdf(None, mediabox=b"[0 0 50 60]", kids=files["obj"])))
    if kind == "info":
        text = pdfium.meta(pdf(None, info=files["obj"]), files["tag"])
        return "no-document" if text is None else "text " + esc(text)
    if kind == "pages":
        return pages_answer(pdfium.pages(files["pdf"]))
    raise SystemExit("unknown kind %r" % kind)


def load_generator(folder):
    path = os.path.join(folder, "cases.py")
    spec = importlib.util.spec_from_file_location("cases", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.cases()


def read_file(path):
    with open(path, "rb") as f:
        return f.read()


def write_cases(folder, out):
    """The generator's cases written into `out`, cases/NAME.EXT and
    cases.tsv, in place of any there."""
    cases = load_generator(folder)
    names = [c[0] for c in cases]
    if len(set(names)) != len(names):
        raise SystemExit("%s: a case's name written twice" % folder)
    # A file system that ignores case (macOS's, Windows') would keep one
    # file for both.
    if len({n.lower() for n in names}) != len(names):
        raise SystemExit("%s: two cases' names alike but for case" % folder)
    os.makedirs(os.path.join(out, "cases"), exist_ok=True)
    for file in os.listdir(os.path.join(out, "cases")):
        os.remove(os.path.join(out, "cases", file))
    lines = ["# name\tkind\tfiles"]
    for name, kind, files in cases:
        written = []
        for ext in sorted(files):
            file = "%s.%s" % (name, ext)
            with open(os.path.join(out, "cases", file), "wb") as f:
                f.write(files[ext])
            written.append(file)
        lines.append("%s\t%s\t%s" % (name, kind, ",".join(written)))
    with open(os.path.join(out, "cases.tsv"), "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


def read_cases(folder):
    """A set's cases, as cases.tsv lists them: (name, kind, {ext: bytes})."""
    out = []
    with open(os.path.join(folder, "cases.tsv"), encoding="utf-8") as f:
        for line in f:
            if line.startswith("#") or not line.strip():
                continue
            name, kind, files = line.rstrip("\n").split("\t")
            read = {}
            for file in files.split(","):
                with open(os.path.join(folder, "cases", file), "rb") as g:
                    read[file[len(name) + 1:]] = g.read()
            out.append((name, kind, read))
    return out


def recorded_file(folder):
    """The one chromium-N.txt in a set."""
    found = [f for f in os.listdir(folder) if f.startswith("chromium-") and f.endswith(".txt")]
    if len(found) != 1:
        raise SystemExit("%s: %d recorded answer files, not one" % (folder, len(found)))
    return os.path.join(folder, found[0])


def pin_named(folder):
    """Where the docs of a set's crate name another build than the one its
    answers are recorded at: each "as of chromium/N" in its README.md and
    its sources' comments, the claim of which build it reads as, held to
    the N of the set's chromium-N.txt. A list of what's wrong."""
    build = os.path.basename(recorded_file(folder))[len("chromium-"):-len(".txt")]
    crate = os.path.dirname(os.path.dirname(os.path.abspath(folder)))
    paths = [os.path.join(crate, "README.md")]
    for root, _, files in os.walk(os.path.join(crate, "src")):
        paths += [os.path.join(root, f) for f in sorted(files) if f.endswith(".rs")]
    wrong = []
    named = False
    for path in paths:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        # Comments' markers taken out, so a claim across two lines reads
        # as one.
        words = re.sub(r"\s*\n\s*(//[/!]?)?\s*", " ", text)
        for n in re.findall(r"\bas of chromium/(\d+)", words):
            if n != build:
                wrong.append("%s names chromium/%s" % (os.path.relpath(path, crate), n))
            elif path.endswith("README.md"):
                named = True
    if not named:
        wrong.append("README.md doesn't say it reads as of chromium/%s" % build)
    return ["%s: %s, where its answers are chromium/%s's" % (crate, w, build) for w in wrong]


def read_answers(path):
    out = {}
    if not os.path.exists(path):
        return out
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#") or not line.strip():
                continue
            name, text = line.rstrip("\n").split("\t", 1)
            out[name] = text
    return out


def words_agree(a, b):
    """Two answers alike, word by word, numbers as f32s."""
    a, b = a.split(), b.split()
    if len(a) != len(b):
        return False
    for x, y in zip(a, b):
        if x == y:
            continue
        try:
            fx, fy = (struct.unpack("<f", struct.pack("<f", float(v)))[0] for v in (x, y))
        except (ValueError, OverflowError):
            return False
        if not (fx == fy or (fx != fx and fy != fy)):
            return False
    return True


def main(argv):
    if len(argv) < 2:
        raise SystemExit(__doc__)
    command, rest = argv[1], argv[2:]
    build = None
    if "--build" in rest:
        i = rest.index("--build")
        build = rest[i + 1]
        rest = rest[:i] + rest[i + 2:]
    write = "--write" in rest
    rest = [r for r in rest if r != "--write"]

    if command == "cases":
        failed = 0
        for folder in rest:
            if write:
                write_cases(folder, folder)
                continue
            wrong = 0
            with tempfile.TemporaryDirectory() as tmp:
                write_cases(folder, tmp)
                made = sorted(os.listdir(os.path.join(tmp, "cases")))
                kept = sorted(os.listdir(os.path.join(folder, "cases")))
                for file in sorted(set(made) | set(kept)):
                    a = os.path.join(tmp, "cases", file)
                    b = os.path.join(folder, "cases", file)
                    if not (os.path.exists(a) and os.path.exists(b)) or read_file(a) != read_file(b):
                        print("%s: cases/%s isn't what cases.py writes" % (folder, file))
                        wrong += 1
                if read_file(os.path.join(tmp, "cases.tsv")) != read_file(os.path.join(folder, "cases.tsv")):
                    print("%s: cases.tsv isn't what cases.py writes" % folder)
                    wrong += 1
            for line in pin_named(folder):
                print(line)
                wrong += 1
            if wrong:
                print("%s: %d files not as cases.py writes them, or naming another build"
                      % (folder, wrong))
            else:
                print("%s: %d cases, as cases.py writes them" % (folder, len(read_cases(folder))))
            failed += wrong
        sys.exit(1 if failed else 0)

    if command not in ("run", "check", "record") or not rest:
        raise SystemExit(__doc__)
    pdfium = Pdfium(rest[0])
    failed = 0
    for folder in rest[1:]:
        cases = read_cases(folder)
        answers = [(name, answer(pdfium, kind, files)) for name, kind, files in cases]
        if command == "run":
            print("# %s" % folder)
            for name, text in answers:
                print("%s\t%s" % (name, text))
        elif command == "record":
            if not build:
                raise SystemExit("record: --build names the build the answers are PDFium's at")
            # A case the build's .differs lists is PDFium's regression at
            # it: it keeps the answer recorded before (README.md).
            known = read_answers(os.path.join(folder, "chromium-%s.differs" % build))
            before = read_answers(recorded_file(folder)) if known else {}
            for name in sorted(set(known) - set(before)):
                raise SystemExit("%s: %s is in chromium-%s.differs with no answer recorded before"
                                 % (folder, name, build))
            answers = [(name, before[name] if name in known else text) for name, text in answers]
            for f in os.listdir(folder):
                if f.startswith("chromium-") and f.endswith(".txt"):
                    os.remove(os.path.join(folder, f))
            with open(os.path.join(folder, "chromium-%s.txt" % build), "w", newline="\n", encoding="utf-8") as f:
                f.write("# PDFium's answer for each case, as chromium/%s gives it "
                        "(oracle/pdfium.py record)\n" % build)
                for name, text in answers:
                    f.write("%s\t%s\n" % (name, text))
            print("%s: %d answers recorded, %d kept as chromium-%s.differs lists"
                  % (folder, len(answers), len(known), build))
        else:
            if build and not recorded_file(folder).endswith("chromium-%s.txt" % build):
                print("%s: answers recorded at another build than chromium/%s" % (folder, build))
                failed += 1
            recorded = read_answers(recorded_file(folder))
            # A case whose answer a build gives otherwise, kept at its
            # recorded answer, as PDFium's to fix (README.md).
            known = read_answers(os.path.join(folder, "chromium-%s.differs" % build)) if build else {}
            moved = 0
            for name, text in answers:
                if name not in recorded:
                    print("%s: %s has no recorded answer" % (folder, name))
                    moved += 1
                elif name in known and words_agree(known[name], text):
                    print("%s: %s answered as chromium-%s.differs lists" % (folder, name, build))
                elif not words_agree(recorded[name], text):
                    print("%s: %s\n  recorded: %s\n  now:      %s" % (folder, name, recorded[name], text))
                    moved += 1
            for name in sorted(set(recorded) - {n for n, _ in answers}):
                print("%s: %s is recorded but no case" % (folder, name))
                moved += 1
            print("%s: %d cases, %d answered otherwise than recorded" % (folder, len(answers), moved))
            failed += moved
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main(sys.argv)
