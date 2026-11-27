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
import zlib

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


def f32(x):
    """A Python float rounded to IEEE-754 binary32, as PDFium's C++ floats are."""
    return struct.unpack("<f", struct.pack("<f", float(x)))[0]


def mat_mul(l, r):
    a, b, c, d, e, f = l
    ra, rb, rc, rd, re, rf = r
    return (
        f32(f32(a * ra) + f32(b * rc)),
        f32(f32(a * rb) + f32(b * rd)),
        f32(f32(c * ra) + f32(d * rc)),
        f32(f32(c * rb) + f32(d * rd)),
        f32(f32(f32(e * ra) + f32(f * rc)) + re),
        f32(f32(f32(e * rb) + f32(f * rd)) + rf),
    )


def mat_apply(m, x, y):
    a, b, c, d, e, f = m
    x, y = f32(x), f32(y)
    return (
        f32(f32(f32(a * x) + f32(c * y)) + e),
        f32(f32(f32(b * x) + f32(d * y)) + f),
    )


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


def contents_pdf(contents, objects):
    """A one-page PDF whose page's /Contents is `contents` as written, and
    whose objects from 4 on are `objects`: each as written between its
    `n 0 obj` and `endobj`, each followed by a line `endobj`."""
    page = (b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents " + contents
            + b" /Resources << >> >>")
    bodies = objects.split(b"\nendobj\n")
    if bodies[-1] != b"":
        raise SystemExit("a contents case's objects each end in a line `endobj`")
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", page] + bodies[:-1]
    out = bytearray(b"%PDF-1.7\n")
    offsets = []
    for i, o in enumerate(objs, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    for o in offsets:
        out += b"%010d 00000 n \n" % o
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
    return bytes(out)


class Ref(tuple):
    """An indirect reference read by `pdf_value`: (number, generation)."""


class Name(bytes):
    """A name read by `pdf_value` or `content_tokens`, without its `/`."""


class Op(bytes):
    """An operator read by `content_tokens`."""


WHITE = b"\0\t\n\f\r "
DELIMS = b"()<>[]{}/%"
STRING_ESCAPES = {ord("n"): b"\n", ord("r"): b"\r", ord("t"): b"\t", ord("b"): b"\b", ord("f"): b"\f",
                  ord("("): b"(", ord(")"): b")", ord("\\"): b"\\"}


def _skip(data, i):
    while i < len(data):
        if data[i] in WHITE:
            i += 1
        elif data[i] == 0x25:
            while i < len(data) and data[i] not in b"\r\n":
                i += 1
        else:
            break
    return i


def _literal(data, i):
    """The literal string opening at `data[i]`, and where it ends."""
    out, depth = bytearray(), 1
    i += 1
    while True:
        if i >= len(data):
            raise SystemExit("forms-ap: a string doesn't end")
        b = data[i]
        if b == 0x5C:
            i += 1
            c = data[i]
            if c in STRING_ESCAPES:
                out += STRING_ESCAPES[c]
                i += 1
            elif 0x30 <= c <= 0x37:
                j = i
                while j < min(i + 3, len(data)) and 0x30 <= data[j] <= 0x37:
                    j += 1
                out.append(int(data[i:j], 8) & 0xFF)
                i = j
            elif c in b"\r\n":
                i += 2 if data[i:i + 2] == b"\r\n" else 1
            else:
                out.append(c)
                i += 1
            continue
        if b == 0x28:
            depth += 1
        elif b == 0x29:
            depth -= 1
            if depth == 0:
                return bytes(out), i + 1
        out.append(b)
        i += 1


def _word(data, i):
    j = i
    while j < len(data) and data[j] not in WHITE and data[j] not in DELIMS:
        j += 1
    return data[i:j], j


def _number(word):
    try:
        return float(word)
    except ValueError:
        return None


def pdf_value(data, i):
    """The object at `data[i]` as PDFium saves one (a dictionary, an array, a
    name, a number, a string, a reference, a keyword), and where it ends.
    Anything else stops the recording."""
    i = _skip(data, i)
    b = data[i:i + 2]
    if b == b"<<":
        out, i = {}, i + 2
        while True:
            i = _skip(data, i)
            if data[i:i + 2] == b">>":
                return out, i + 2
            key, i = pdf_value(data, i)
            if not isinstance(key, Name):
                raise SystemExit("forms-ap: a dictionary's key isn't a name")
            out[bytes(key)], i = pdf_value(data, i)
    if b[:1] == b"[":
        out, i = [], i + 1
        while True:
            i = _skip(data, i)
            if data[i:i + 1] == b"]":
                return out, i + 1
            value, i = pdf_value(data, i)
            out.append(value)
    if b[:1] == b"/":
        word, i = _word(data, i + 1)
        return Name(word), i
    if b[:1] == b"(":
        return _literal(data, i)
    if b[:1] == b"<":
        j = data.index(b">", i)
        return bytes.fromhex(re.sub(rb"\s", b"", data[i + 1:j]).decode("ascii")), j + 1
    word, j = _word(data, i)
    if word in (b"true", b"false", b"null"):
        return word.decode("ascii"), j
    n = _number(word)
    if n is None:
        raise SystemExit("forms-ap: %r isn't an object" % word)
    # `n g R`, a reference, or a number alone.
    m = re.match(rb"\s*(\d+)\s+R(?=[\s/<>\[\]()%]|$)", data[j:j + 24])
    if m and word.isdigit():
        return Ref((int(word), int(m.group(1)))), j + m.end()
    return n, j


def saved_objects(data):
    """Each object of a file PDFium saved, by its number: its value, and its
    stream's bytes (decoded where it says /FlateDecode) or None. The file is
    held to the form PDFium writes it in: no object streams, each object at a
    line's start."""
    out = {}
    for m in re.finditer(rb"(?m)^(\d+) 0 obj\s*", data):
        value, i = pdf_value(data, m.end())
        stream = None
        s = re.match(rb"\s*stream\r?\n", data[i:i + 16])
        if s:
            if not isinstance(value, dict) or not isinstance(value.get(b"Length"), float):
                raise SystemExit("forms-ap: a saved stream's /Length isn't a direct number")
            start = i + s.end()
            stream = data[start:start + int(value[b"Length"])]
            filters = value.get(b"Filter")
            if filters == Name(b"FlateDecode"):
                stream = zlib.decompress(stream)
            elif filters is not None:
                raise SystemExit("forms-ap: a saved stream's filter isn't /FlateDecode alone")
        out[int(m.group(1))] = (value, stream)
    return out


def content_tokens(data):
    """A content stream's operands and operators in order: numbers as floats,
    names, strings as bytes, arrays as lists, operators as `Op`."""
    out, i = [], 0
    stack = []
    while True:
        i = _skip(data, i)
        if i >= len(data):
            break
        b = data[i:i + 1]
        if b == b"[":
            stack.append(out)
            out = []
            i += 1
            continue
        if b == b"]":
            if not stack:
                raise SystemExit("forms-ap: `]` with no `[`")
            inner, out = out, stack.pop()
            out.append(inner)
            i += 1
            continue
        if b in (b"/", b"(", b"<"):
            value, i = pdf_value(data, i)
            out.append(value)
            continue
        word, i = _word(data, i)
        if not word:
            raise SystemExit("forms-ap: %r in a content stream" % data[i:i + 1])
        n = _number(word)
        out.append(n if n is not None else Op(word))
    if stack:
        raise SystemExit("forms-ap: `[` with no `]`")
    return out


COLOR_OPS = {b"g": 1, b"rg": 3, b"k": 4, b"G": 1, b"RG": 3, b"K": 4}
SEGMENT_OPS = {b"re": 4, b"m": 2, b"l": 2, b"c": 6, b"h": 0}
PAINT_OPS = {b"f": "fill", b"F": "fill", b"f*": "eofill", b"S": "stroke"}


def ap_record(content):
    """A made appearance's content as the record's words from `paint` on:
    each path painted before `/Tx BMC`, in order, with its color (and a
    stroke's width, cap, join and dash) and its segments; then `tx none`
    where there is no `/Tx BMC`, or the clip in it and its text, each `Tj` by
    its font, size, origin (the `Td`s summed in f32) and codes. An operator
    PDFium doesn't write in a text field's appearance stops the recording."""
    def fail(why):
        raise SystemExit("forms-ap: %s in %r" % (why, content[:200]))

    state = {"fill": (b"g", [0.0]), "stroke": (b"G", [0.0]), "w": 1.0, "dash": ([], 0.0), "cap": 0, "join": 0}
    saved, path, operands = [], [], []
    paint, runs, colors = [], [], set()
    clip, tx, in_tx, in_bt, pending_clip = None, False, False, False, False
    font, size, x, y = None, None, 0.0, 0.0

    def nums(count):
        if len(operands) != count or not all(isinstance(v, float) for v in operands):
            fail("%d numbers wanted" % count)
        return [f32(v) for v in operands]

    for token in content_tokens(content):
        if not isinstance(token, Op):
            operands.append(token)
            continue
        op = bytes(token)
        if op == b"q":
            nums(0)
            saved.append(dict(state))
        elif op == b"Q":
            nums(0)
            if not saved:
                fail("`Q` with no `q`")
            state = saved.pop()
        elif op in COLOR_OPS:
            state["fill" if op.islower() else "stroke"] = (op, nums(COLOR_OPS[op]))
        elif op == b"w":
            state["w"] = nums(1)[0]
        elif op == b"J":
            state["cap"] = int(nums(1)[0])
        elif op == b"j":
            state["join"] = int(nums(1)[0])
        elif op == b"d":
            if len(operands) != 2 or not isinstance(operands[0], list) or not isinstance(operands[1], float):
                fail("`d` wants an array and a number")
            state["dash"] = ([f32(v) for v in operands[0]], f32(operands[1]))
        elif op in SEGMENT_OPS:
            if in_bt:
                fail("a path in a text object")
            path.append((op, nums(SEGMENT_OPS[op])))
        elif op in PAINT_OPS:
            nums(0)
            if tx or not path or pending_clip:
                fail("a path painted in /Tx, a clip painted, or no path")
            kind = PAINT_OPS[op]
            color = state["stroke" if kind == "stroke" else "fill"]
            words = [kind, color[0].decode("ascii")] + [num(v) for v in color[1]]
            if kind == "stroke":
                dash, phase = state["dash"]
                words += ["w", num(state["w"]), "cap", str(state["cap"]), "join", str(state["join"]), "dash"]
                words += ([num(v) for v in dash] or ["none"]) + ["phase", num(phase)]
            for seg, args in path:
                words += [seg.decode("ascii")] + [num(v) for v in args]
            paint.append(" ".join(words))
            path = []
        elif op == b"W":
            nums(0)
            pending_clip = True
        elif op == b"n":
            nums(0)
            if pending_clip:
                if not in_tx or clip is not None or len(path) != 1 or path[0][0] != b"re":
                    fail("a clip other than one rectangle in /Tx")
                clip = path[0][1]
            path, pending_clip = [], False
        elif op == b"BMC":
            if operands != [Name(b"Tx")] or tx:
                fail("a marked sequence other than one /Tx")
            operands = []
            tx = in_tx = True
        elif op == b"EMC":
            nums(0)
            if not in_tx:
                fail("`EMC` with no `BMC`")
            in_tx = False
        elif op == b"BT":
            nums(0)
            if in_bt or not in_tx:
                fail("`BT` outside /Tx, or within a text object")
            in_bt, x, y = True, 0.0, 0.0
        elif op == b"ET":
            nums(0)
            in_bt = False
        elif op == b"Td":
            if not in_bt:
                fail("`Td` outside a text object")
            dx, dy = nums(2)
            x, y = f32(x + dx), f32(y + dy)
        elif op == b"Tf":
            if len(operands) != 2 or not isinstance(operands[0], Name) or not isinstance(operands[1], float):
                fail("`Tf` wants a name and a number")
            font, size = bytes(operands[0]), f32(operands[1])
        elif op == b"Tj":
            if not in_bt or len(operands) != 1 or not isinstance(operands[0], bytes) \
                    or isinstance(operands[0], Name):
                fail("`Tj` outside a text object, or with no string")
            colors.add((state["fill"][0], tuple(state["fill"][1])))
            # A `Tj` before any `Tf`, which PDFium writes for a box with no
            # room, is shown in no font: `%`, the empty word, at size 0.
            runs.append("font %s size %s at %s %s codes %s" % (esc(font or b""), num(size or 0.0), num(x),
                                                              num(y), esc(operands[0])))
        else:
            fail("operator %r" % op)
        if op not in (b"BMC",):
            operands = []
    if operands or saved or in_tx or in_bt or path:
        fail("an unfinished stream")
    words = ["paint"] + (paint or ["none"])
    if not tx:
        return " ".join(words + ["tx", "none"])
    words += ["clip"] + ([num(v) for v in clip] if clip else ["none"])
    if not runs:
        return " ".join(words + ["text", "none"])
    if len(colors) != 1:
        fail("text in more than one color")
    op, comps = colors.pop()
    return " ".join(words + ["color", op.decode("ascii")] + [num(v) for v in comps] + runs)


IDENTITY = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)


def ap_items(content):
    """A made appearance's content as the record's words from `items` on:
    each item in the order the content has it, paths and text in any order,
    in or out of `/Tx`. A path painted, with its color (and a stroke's
    width, cap, join and dash) and its segments, as `ap_record` writes one;
    `clip` and its segments where a path is made the clip, and `]clip` at
    the `Q` that ends it; `tx[` and `]tx` where the `/Tx` sequence begins
    and ends; and `text`, each `Tj` with its own color, its font, size,
    origin (the `Td`s summed in f32) and codes. A `cm` is applied to what it
    moves, so every point is the form's own (a rectangle under a turn as its
    four corners); a stroke's width and a text's size are scaled with it,
    which only a turn, a move or an even scale may be. `items none` where
    the content draws nothing. An operator PDFium doesn't write in a
    button's or a choice's appearance stops the recording."""
    def fail(why):
        raise SystemExit("forms-ap-all: %s in %r" % (why, content[:200]))

    state = {"fill": (b"g", [0.0]), "stroke": (b"G", [0.0]), "w": 1.0, "dash": ([], 0.0), "cap": 0, "join": 0,
             "ctm": IDENTITY, "clips": 0, "font": None, "size": None}
    saved, path, operands, items = [], [], [], []
    in_tx, in_bt, pending_clip = False, False, False
    x, y = 0.0, 0.0

    def nums(count):
        if len(operands) != count or not all(isinstance(v, float) for v in operands):
            fail("%d numbers wanted" % count)
        return [f32(v) for v in operands]

    def scale():
        """How far the CTM stretches a length: a turn's or an even scale's
        factor, and nothing else."""
        a, b, c, d, _, _ = state["ctm"]
        if not (abs(a - d) < 1e-6 and abs(b + c) < 1e-6 or abs(a + d) < 1e-6 and abs(b - c) < 1e-6):
            fail("a `cm` that isn't a turn, a move or an even scale")
        return f32(abs(a * d - b * c) ** 0.5)

    def segment_words():
        ctm = state["ctm"]
        words = []
        for seg, args in path:
            if ctm == IDENTITY:
                words += [seg.decode("ascii")] + [num(v) for v in args]
            elif seg == b"re":
                x0, y0, w, h = args
                corners = [(x0, y0), (f32(x0 + w), y0), (f32(x0 + w), f32(y0 + h)), (x0, f32(y0 + h))]
                for i, (px, py) in enumerate(corners):
                    words += ["m" if i == 0 else "l"] + [num(v) for v in mat_apply(ctm, px, py)]
                words.append("h")
            else:
                moved = []
                for i in range(0, len(args), 2):
                    moved += mat_apply(ctm, args[i], args[i + 1])
                words += [seg.decode("ascii")] + [num(v) for v in moved]
        return words

    def color_words(color):
        return [color[0].decode("ascii")] + [num(v) for v in color[1]]

    for token in content_tokens(content):
        if not isinstance(token, Op):
            operands.append(token)
            continue
        op = bytes(token)
        if op == b"q":
            nums(0)
            saved.append(dict(state))
        elif op == b"Q":
            nums(0)
            if not saved:
                fail("`Q` with no `q`")
            if in_bt:
                fail("`Q` in a text object")
            clips = state["clips"]
            state = saved.pop()
            items += ["]clip"] * (clips - state["clips"])
        elif op == b"cm":
            state["ctm"] = mat_mul(nums(6), state["ctm"])
        elif op in COLOR_OPS:
            state["fill" if op.islower() else "stroke"] = (op, nums(COLOR_OPS[op]))
        elif op == b"w":
            state["w"] = nums(1)[0]
        elif op == b"J":
            state["cap"] = int(nums(1)[0])
        elif op == b"j":
            state["join"] = int(nums(1)[0])
        elif op == b"d":
            if len(operands) != 2 or not isinstance(operands[0], list) or not isinstance(operands[1], float):
                fail("`d` wants an array and a number")
            state["dash"] = ([f32(v) for v in operands[0]], f32(operands[1]))
        elif op in SEGMENT_OPS:
            if in_bt:
                fail("a path in a text object")
            path.append((op, nums(SEGMENT_OPS[op])))
        elif op in PAINT_OPS:
            nums(0)
            if not path or pending_clip:
                fail("a clip painted, or no path")
            kind = PAINT_OPS[op]
            words = [kind] + color_words(state["stroke" if kind == "stroke" else "fill"])
            if kind == "stroke":
                k = scale()
                dash, phase = state["dash"]
                words += ["w", num(f32(state["w"] * k)), "cap", str(state["cap"]), "join", str(state["join"]),
                          "dash"]
                words += ([num(f32(v * k)) for v in dash] or ["none"]) + ["phase", num(f32(phase * k))]
            items += words + segment_words()
            path = []
        elif op == b"W":
            nums(0)
            pending_clip = True
        elif op == b"n":
            nums(0)
            if pending_clip:
                if not path:
                    fail("a clip of no path")
                items += ["clip"] + segment_words()
                state["clips"] += 1
            path, pending_clip = [], False
        elif op == b"BMC":
            if operands != [Name(b"Tx")] or in_tx:
                fail("a marked sequence other than one /Tx at a time")
            operands = []
            in_tx = True
            items.append("tx[")
        elif op == b"EMC":
            nums(0)
            if not in_tx:
                fail("`EMC` with no `BMC`")
            in_tx = False
            items.append("]tx")
        elif op == b"BT":
            nums(0)
            if in_bt or path:
                fail("`BT` within a text object or a path")
            in_bt, x, y = True, 0.0, 0.0
        elif op == b"ET":
            nums(0)
            if not in_bt:
                fail("`ET` with no `BT`")
            in_bt = False
        elif op == b"Td":
            if not in_bt:
                fail("`Td` outside a text object")
            dx, dy = nums(2)
            x, y = f32(x + dx), f32(y + dy)
        elif op == b"Tf":
            if len(operands) != 2 or not isinstance(operands[0], Name) or not isinstance(operands[1], float):
                fail("`Tf` wants a name and a number")
            state["font"], state["size"] = bytes(operands[0]), f32(operands[1])
        elif op == b"Tj":
            if not in_bt or len(operands) != 1 or not isinstance(operands[0], bytes) \
                    or isinstance(operands[0], Name):
                fail("`Tj` outside a text object, or with no string")
            at = (x, y) if state["ctm"] == IDENTITY else mat_apply(state["ctm"], x, y)
            size = state["size"] or 0.0
            if state["ctm"] != IDENTITY:
                size = f32(size * scale())
            # A `Tj` before any `Tf` is shown in no font: `%`, the empty
            # word, at size 0, as `ap_record` writes it.
            items += ["text"] + color_words(state["fill"]) + [
                "font", esc(state["font"] or b""), "size", num(size), "at", num(at[0]), num(at[1]),
                "codes", esc(operands[0])]
        else:
            fail("operator %r" % op)
        if op != b"BMC":
            operands = []
    if operands or saved or in_tx or in_bt or path or pending_clip:
        fail("an unfinished stream")
    return " ".join(["items"] + (items or ["none"]))


def name_text(name):
    """A name as PDFium saves it, its `#XX` escapes read."""
    return re.sub(rb"#([0-9A-Fa-f]{2})", lambda m: bytes([int(m.group(1), 16)]), bytes(name))


def value_word(value):
    """A field's `/V` or a widget's `/AS` as one word: `/` and the name, `(`
    the string `)`, `none` where there is none, `other` for any other kind
    of value."""
    if value is None:
        return "none"
    if isinstance(value, Name):
        return "/" + esc(name_text(value))
    if isinstance(value, bytes):
        return "(" + esc(value) + ")"
    return "other"


class FileWrite(ctypes.Structure):
    pass


FileWrite._fields_ = [("version", ctypes.c_int),
                      ("WriteBlock", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.POINTER(FileWrite),
                                                      ctypes.c_void_p, ctypes.c_ulong))]


class M(ctypes.Structure):
    _fields_ = [(n, ctypes.c_float) for n in "abcdef"]


class FSRect(ctypes.Structure):
    _fields_ = [(n, ctypes.c_float) for n in ("left", "top", "right", "bottom")]


class FSPoint(ctypes.Structure):
    _fields_ = [("x", ctypes.c_float), ("y", ctypes.c_float)]


class FSQuadPoints(ctypes.Structure):
    _fields_ = [(n, ctypes.c_float) for n in ("x1", "y1", "x2", "y2", "x3", "y3", "x4", "y4")]


class FileAvail(ctypes.Structure):
    pass


FileAvail._fields_ = [("version", ctypes.c_int),
                      ("IsDataAvail", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.POINTER(FileAvail),
                                                       ctypes.c_size_t, ctypes.c_size_t))]


class FileAccess(ctypes.Structure):
    _fields_ = [("len", ctypes.c_ulong),
                ("GetBlock", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_ulong,
                                              ctypes.POINTER(ctypes.c_ubyte), ctypes.c_ulong)),
                ("param", ctypes.c_void_p)]


class DownloadHints(ctypes.Structure):
    pass


DownloadHints._fields_ = [("version", ctypes.c_int),
                          ("AddSegment", ctypes.CFUNCTYPE(None, ctypes.POINTER(DownloadHints),
                                                          ctypes.c_size_t, ctypes.c_size_t))]


# FPDF_FORMFILLINFO's callbacks at version 2 (fpdf_formfill.h), in order;
# `forms-press` gives the first ones each a function that does nothing, or
# answers the one page it has, and leaves the rest null as `forms` does.
FORM_CALLBACKS = {
    "Release": (None, [ctypes.c_void_p]),
    "FFI_Invalidate": (None, [ctypes.c_void_p, ctypes.c_void_p] + [ctypes.c_double] * 4),
    "FFI_OutputSelectedRect": (None, [ctypes.c_void_p, ctypes.c_void_p] + [ctypes.c_double] * 4),
    "FFI_SetCursor": (None, [ctypes.c_void_p, ctypes.c_int]),
    "FFI_SetTimer": (ctypes.c_int, [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p]),
    "FFI_KillTimer": (None, [ctypes.c_void_p, ctypes.c_int]),
    # Returns a structure, which a ctypes callback can't; PDFium's form
    # filler never asks for it (only its tests' stub names it), so it's null.
    "FFI_GetLocalTime": None,
    "FFI_OnChange": (None, [ctypes.c_void_p]),
    "FFI_GetPage": (ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int]),
    "FFI_GetCurrentPage": (ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_void_p]),
    "FFI_GetRotation": (ctypes.c_int, [ctypes.c_void_p, ctypes.c_void_p]),
    "FFI_ExecuteNamedAction": (None, [ctypes.c_void_p, ctypes.c_char_p]),
    "FFI_SetTextFieldFocus": (None, [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int]),
    "FFI_DoURIAction": (None, [ctypes.c_void_p, ctypes.c_char_p]),
    "FFI_DoGoToAction": (None, [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_int]),
}


class FormFillInfo(ctypes.Structure):
    _fields_ = ([("version", ctypes.c_int)] + [(name, ctypes.c_void_p) for name in FORM_CALLBACKS]
                + [("m_pJsPlatform", ctypes.c_void_p), ("xfa_disabled", ctypes.c_int)]
                # Version 2's callbacks past xfa_disabled, each null.
                + [("v2_%d" % i, ctypes.c_void_p) for i in range(17)])


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
            ("FPDF_PageToDevice", I, [P, I, I, I, I, I, ctypes.c_double, ctypes.c_double, pi, pi]),
            ("FPDF_GetMetaText", U, [P, ctypes.c_char_p, P, U]),
            ("FPDFPage_CountObjects", I, [P]),
            ("FPDFPage_GetObject", P, [P, I]),
            ("FPDFPageObj_GetType", I, [P]),
            ("FPDFFormObj_CountObjects", I, [P]),
            ("FPDFFormObj_GetObject", P, [P, U]),
            ("FPDFPageObj_GetMatrix", I, [P, ctypes.POINTER(M)]),
            ("FPDFPageObj_GetStrokeWidth", I, [P, pf]),
            ("FPDFPageObj_GetLineCap", I, [P]),
            ("FPDFPageObj_GetLineJoin", I, [P]),
            ("FPDFPageObj_GetClipPath", P, [P]),
            ("FPDFClipPath_CountPaths", I, [P]),
            ("FPDFClipPath_CountPathSegments", I, [P, I]),
            ("FPDFClipPath_GetPathSegment", P, [P, I, I]),
            ("FPDFPageObj_CountMarks", I, [P]),
            ("FPDFPageObj_GetMark", P, [P, U]),
            ("FPDFPageObjMark_GetName", I, [P, P, U, ctypes.POINTER(U)]),
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
            ("FPDFText_CountChars", I, [P]),
            ("FPDFText_GetUnicode", ctypes.c_uint, [P, I]),
            ("FPDFText_IsGenerated", I, [P, I]),
            ("FPDFText_IsHyphen", I, [P, I]),
            ("FPDFText_CountRects", I, [P, I, I]),
            ("FPDFText_FindStart", P, [P, ctypes.POINTER(ctypes.c_ushort), U, I]),
            ("FPDFText_FindNext", I, [P]),
            ("FPDFText_GetSchResultIndex", I, [P]),
            ("FPDFText_GetSchCount", I, [P]),
            ("FPDFText_FindClose", None, [P]),
            ("FPDFText_GetCharOrigin", I, [P, I, ctypes.POINTER(ctypes.c_double), ctypes.POINTER(ctypes.c_double)]),
            ("FPDFText_GetLooseCharBox", I, [P, I, ctypes.POINTER(FSRect)]),
            ("FPDFText_GetCharBox", I, [P, I, ctypes.POINTER(ctypes.c_double), ctypes.POINTER(ctypes.c_double),
                                        ctypes.POINTER(ctypes.c_double), ctypes.POINTER(ctypes.c_double)]),
            ("FPDFTextObj_GetText", U, [P, P, P, U]),
            ("FPDFTextObj_GetFontSize", I, [P, pf]),
            ("FPDFTextObj_GetTextRenderMode", I, [P]),
            ("FPDFTextObj_GetFont", P, [P]),
            ("FPDFFont_GetBaseFontName", S, [P, P, S]),
            ("FPDFImageObj_GetImageDataRaw", U, [P, P, U]),
            ("FPDFImageObj_GetImagePixelSize", I, [P, ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint)]),
            ("FPDFAvail_Create", P, [ctypes.POINTER(FileAvail), ctypes.POINTER(FileAccess)]),
            ("FPDFAvail_Destroy", None, [P]),
            ("FPDFAvail_IsLinearized", I, [P]),
            ("FPDFAvail_IsDocAvail", I, [P, ctypes.POINTER(DownloadHints)]),
            ("FPDFAvail_GetDocument", P, [P, ctypes.c_char_p]),
            ("FPDFAvail_GetFirstPageNum", I, [P]),
            ("FPDFAvail_IsPageAvail", I, [P, I, ctypes.POINTER(DownloadHints)]),
            ("FPDFPage_GetAnnotCount", I, [P]),
            ("FPDFPage_GetAnnot", P, [P, I]),
            ("FPDFPage_CloseAnnot", None, [P]),
            ("FPDFAnnot_GetSubtype", I, [P]),
            ("FPDFAnnot_GetFlags", I, [P]),
            ("FPDFDOC_InitFormFillEnvironment", P, [P, P]),
            ("FPDFDOC_ExitFormFillEnvironment", None, [P]),
            ("FORM_OnAfterLoadPage", None, [P, P]),
            ("FORM_OnBeforeClosePage", None, [P, P]),
            ("FORM_OnMouseMove", I, [P, P, I, ctypes.c_double, ctypes.c_double]),
            ("FORM_OnLButtonDown", I, [P, P, I, ctypes.c_double, ctypes.c_double]),
            ("FORM_OnLButtonUp", I, [P, P, I, ctypes.c_double, ctypes.c_double]),
            ("FORM_ForceToKillFocus", I, [P]),
            ("FPDFAnnot_GetFormFieldType", I, [P, P]),
            ("FPDFAnnot_GetFormFieldFlags", I, [P, P]),
            ("FPDFAnnot_GetFormFieldName", U, [P, P, P, U]),
            ("FPDFAnnot_GetFormFieldValue", U, [P, P, P, U]),
            ("FPDFAnnot_GetAP", U, [P, I, P, U]),
            ("FPDF_SaveAsCopy", I, [P, ctypes.POINTER(FileWrite), U]),
            ("FPDFAnnot_GetRect", I, [P, ctypes.POINTER(FSRect)]),
            ("FPDFAnnot_GetBorder", I, [P, pf, pf, pf]),
            ("FPDFAnnot_GetStringValue", U, [P, ctypes.c_char_p, P, U]),
            ("FPDFAnnot_HasAttachmentPoints", I, [P]),
            ("FPDFAnnot_CountAttachmentPoints", S, [P]),
            ("FPDFAnnot_GetAttachmentPoints", I, [P, S, ctypes.POINTER(FSQuadPoints)]),
            ("FPDFAnnot_GetLine", I, [P, ctypes.POINTER(FSPoint), ctypes.POINTER(FSPoint)]),
            ("FPDFAnnot_GetVertices", U, [P, ctypes.POINTER(FSPoint), U]),
            ("FPDFAnnot_GetInkListCount", U, [P]),
            ("FPDFAnnot_GetInkListPath", U, [P, U, ctypes.POINTER(FSPoint), U]),
            ("FPDFAnnot_GetLink", P, [P]),
            ("FPDFLink_GetDest", P, [P, P]),
            ("FPDFLink_GetAction", P, [P]),
            ("FPDFDest_GetDestPageIndex", I, [P, P]),
            ("FPDFDest_GetView", U, [P, ctypes.POINTER(U), pf]),
            ("FPDFDest_GetLocationInPage", I, [P, pi, pi, pi, pf, pf, pf]),
            ("FPDFAction_GetType", U, [P]),
            ("FPDFAction_GetURIPath", U, [P, P, P, U]),
            ("FPDFAction_GetFilePath", U, [P, P, U]),
            ("FPDFPage_Flatten", I, [P, I]),
            ("FPDFBookmark_GetFirstChild", P, [P, P]),
            ("FPDFBookmark_GetNextSibling", P, [P, P]),
            ("FPDFBookmark_GetTitle", U, [P, P, U]),
            ("FPDFBookmark_GetCount", I, [P]),
            ("FPDFBookmark_GetDest", P, [P, P]),
            ("FPDFBookmark_GetAction", P, [P]),
            ("FPDFBookmark_GetColor", I, [P, pf, pf, pf]),
            ("FPDFBookmark_GetStyle", I, [P]),
            ("FPDF_GetPageLabel", U, [P, I, P, U]),
            ("FPDFDoc_GetAttachmentCount", I, [P]),
            ("FPDFDoc_GetAttachment", P, [P, I]),
            ("FPDFAttachment_GetName", U, [P, P, U]),
            ("FPDFAttachment_GetDescription", U, [P, P, U]),
            ("FPDFAttachment_GetSubtype", U, [P, P, U]),
            ("FPDFAttachment_GetAFRelationship", U, [P, P, U]),
            ("FPDFAttachment_GetFile", I, [P, P, U, ctypes.POINTER(U)]),
            ("FPDF_VIEWERREF_GetPrintScaling", I, [P]),
            ("FPDF_VIEWERREF_GetNumCopies", I, [P]),
            ("FPDF_VIEWERREF_GetDuplex", I, [P]),
            ("FPDF_VIEWERREF_GetPrintPageRange", P, [P]),
            ("FPDF_VIEWERREF_GetPrintPageRangeCount", S, [P]),
            ("FPDF_VIEWERREF_GetPrintPageRangeElement", I, [P, S]),
            ("FPDF_VIEWERREF_GetName", U, [P, ctypes.c_char_p, ctypes.c_char_p, U]),
            ("FPDFCatalog_GetLanguage", U, [P, P, U]),
            ("FPDF_GetFormType", I, [P]),
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

    def image_sizes(self, data):
        """The raw size of each of the page's images, those in its forms
        among them, in order; None where the page doesn't load."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        out = []

        def walk(o):
            kind = TYPES.get(lib.FPDFPageObj_GetType(o))
            if kind == "image":
                out.append(lib.FPDFImageObj_GetImageDataRaw(o, None, 0))
            elif kind == "form":
                for i in range(lib.FPDFFormObj_CountObjects(o)):
                    walk(lib.FPDFFormObj_GetObject(o, i))

        for i in range(lib.FPDFPage_CountObjects(page)):
            walk(lib.FPDFPage_GetObject(page, i))
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return out

    def text_chars(self, data, heights, boxes=False):
        """The page's characters as its text page has them, each its character
        and origin, which is from (0, 100), the fonts' cases' start; with
        `heights`, how far the first's loose box reaches from its origin; and with
        `boxes`, after each its box and its loose box from its origin. None where
        the page doesn't load."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        tp = lib.FPDFText_LoadPage(page)
        out = []
        first_y = 0
        for i in range(lib.FPDFText_CountChars(tp)):
            x, y = ctypes.c_double(), ctypes.c_double()
            lib.FPDFText_GetCharOrigin(tp, i, ctypes.byref(x), ctypes.byref(y))
            first_y = y.value if i == 0 else first_y
            line = "%04x %s %s" % (lib.FPDFText_GetUnicode(tp, i), num(x.value), num(y.value - 100))
            if boxes:
                left, right, bottom, top = (ctypes.c_double() for _ in range(4))
                lib.FPDFText_GetCharBox(tp, i, ctypes.byref(left), ctypes.byref(right),
                                        ctypes.byref(bottom), ctypes.byref(top))
                loose = FSRect()
                lib.FPDFText_GetLooseCharBox(tp, i, ctypes.byref(loose))
                line += " box %s %s %s %s loose %s %s %s %s" % (
                    num(left.value - x.value), num(bottom.value - y.value),
                    num(right.value - x.value), num(top.value - y.value),
                    num(loose.left - x.value), num(loose.bottom - y.value),
                    num(loose.right - x.value), num(loose.top - y.value))
            out.append(line)
        if heights and out:
            box = FSRect()
            lib.FPDFText_GetLooseCharBox(tp, 0, ctypes.byref(box))
            out.append("top %s bottom %s" % (num(box.top - first_y), num(box.bottom - first_y)))
        lib.FPDFText_ClosePage(tp)
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return out

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

    def gfx_geom(self, data):
        """Page 0's effective view dimensions, quarter-turn rotation, and
        FPDF_PageToDevice coordinates at four probe points; None where the
        page doesn't load."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        w = lib.FPDF_GetPageWidthF(page)
        h = lib.FPDF_GetPageHeightF(page)
        rot = lib.FPDFPage_GetRotation(page)
        iw, ih = max(int(round(w)), 1), max(int(round(h)), 1)
        out = ["size", num(w), num(h), "rot", str(rot), "dev"]
        for px, py in ((0.0, 0.0), (50.0, 80.0), (120.0, 180.0), (220.0, 330.0)):
            dx, dy = ctypes.c_int(), ctypes.c_int()
            lib.FPDF_PageToDevice(page, 0, 0, iw, ih, 0, px, py, ctypes.byref(dx), ctypes.byref(dy))
            out += [str(dx.value), str(dy.value)]
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return " ".join(out)

    def gfx(self, data):
        """Page 0's flattened leaf page objects (recursing into Form XObjects,
        transforming path segments and image quads into PageSpace, with stroke
        style, top-level clip paths, and nested marked-content stacks) followed
        by its text page's placed character origins; None where the page
        doesn't load."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        items = []

        def obj_marks(o):
            res = []
            for k in range(max(lib.FPDFPageObj_CountMarks(o), 0)):
                mk = lib.FPDFPageObj_GetMark(o, k)
                ln = ctypes.c_ulong()
                tag = b""
                if lib.FPDFPageObjMark_GetName(mk, None, 0, ctypes.byref(ln)) and ln.value >= 2:
                    raw = ctypes.create_string_buffer(ln.value)
                    lib.FPDFPageObjMark_GetName(mk, raw, ln.value, ctypes.byref(ln))
                    tag = raw.raw[:ln.value - 2].decode("utf-16le", "replace").encode("utf-8")
                v = ctypes.c_int()
                mcid = str(v.value) if lib.FPDFPageObjMark_GetParamIntValue(mk, b"MCID", ctypes.byref(v)) else "-"
                res.append("%s:%s" % (esc(tag), mcid))
            return res

        def top_clips(o):
            cp = lib.FPDFPageObj_GetClipPath(o)
            n = lib.FPDFClipPath_CountPaths(cp) if cp else -1
            if n < 0:
                return []
            words = ["clips", str(n)]
            for pi in range(n):
                words.append("[")
                for si in range(max(lib.FPDFClipPath_CountPathSegments(cp, pi), 0)):
                    s = lib.FPDFClipPath_GetPathSegment(cp, pi, si)
                    x, y = ctypes.c_float(), ctypes.c_float()
                    lib.FPDFPathSegment_GetPoint(s, ctypes.byref(x), ctypes.byref(y))
                    words += [SEGMENTS.get(lib.FPDFPathSegment_GetType(s), "?"), num(x.value), num(y.value)]
                    if lib.FPDFPathSegment_GetClose(s):
                        words.append("h")
                words.append("]")
            return words

        def walk(o, parent_m, parent_marks, top):
            t = TYPES.get(lib.FPDFPageObj_GetType(o), "unknown")
            m = M()
            lib.FPDFPageObj_GetMatrix(o, ctypes.byref(m))
            own_m = (m.a, m.b, m.c, m.d, m.e, m.f)
            marks = parent_marks + obj_marks(o)
            if t == "form":
                world = mat_mul(own_m, parent_m)
                for i in range(max(lib.FPDFFormObj_CountObjects(o), 0)):
                    walk(lib.FPDFFormObj_GetObject(o, i), world, marks, False)
                return
            world = mat_mul(own_m, parent_m)
            words = [t]
            if t == "path":
                fill, stroke = ctypes.c_int(), ctypes.c_int()
                lib.FPDFPath_GetDrawMode(o, ctypes.byref(fill), ctypes.byref(stroke))
                st = int(bool(stroke.value))
                words += ["fill", str(fill.value), "stroke", str(st)]
                if st:
                    w = ctypes.c_float()
                    lib.FPDFPageObj_GetStrokeWidth(o, ctypes.byref(w))
                    words += ["w", num(w.value), "cap", str(lib.FPDFPageObj_GetLineCap(o)),
                              "join", str(lib.FPDFPageObj_GetLineJoin(o))]
                    dn = lib.FPDFPageObj_GetDashCount(o)
                    if dn > 0:
                        dash = (ctypes.c_float * dn)()
                        lib.FPDFPageObj_GetDashArray(o, dash, dn)
                        phase = ctypes.c_float()
                        lib.FPDFPageObj_GetDashPhase(o, ctypes.byref(phase))
                        words += ["dash"] + [num(x) for x in dash] + ["phase", num(phase.value)]
                for i in range(lib.FPDFPath_CountSegments(o)):
                    s = lib.FPDFPath_GetPathSegment(o, i)
                    x, y = ctypes.c_float(), ctypes.c_float()
                    lib.FPDFPathSegment_GetPoint(s, ctypes.byref(x), ctypes.byref(y))
                    px, py = mat_apply(world, x.value, y.value)
                    words += [SEGMENTS.get(lib.FPDFPathSegment_GetType(s), "?"), num(px), num(py)]
                    if lib.FPDFPathSegment_GetClose(s):
                        words.append("h")
            elif t == "image":
                pw, ph = ctypes.c_uint(), ctypes.c_uint()
                lib.FPDFImageObj_GetImagePixelSize(o, ctypes.byref(pw), ctypes.byref(ph))
                words += ["px", str(pw.value), str(ph.value), "quad"]
                for ux, uy in ((0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)):
                    qx, qy = mat_apply(world, ux, uy)
                    words += [num(qx), num(qy)]
            elif t == "text":
                sz = ctypes.c_float()
                lib.FPDFTextObj_GetFontSize(o, ctypes.byref(sz))
                words += ["size", num(sz.value), "mode", str(lib.FPDFTextObj_GetTextRenderMode(o))]
            if top:
                words += top_clips(o)
            if marks:
                words += ["marks"] + marks
            items.append(" ".join(words))

        ident = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
        for i in range(lib.FPDFPage_CountObjects(page)):
            walk(lib.FPDFPage_GetObject(page, i), ident, [], True)
        tp = lib.FPDFText_LoadPage(page)
        for i in range(lib.FPDFText_CountChars(tp)):
            if lib.FPDFText_IsGenerated(tp, i) == 1:
                continue
            x, y = ctypes.c_double(), ctypes.c_double()
            lib.FPDFText_GetCharOrigin(tp, i, ctypes.byref(x), ctypes.byref(y))
            items.append("char %04x %s %s" % (lib.FPDFText_GetUnicode(tp, i), num(x.value), num(y.value)))
        lib.FPDFText_ClosePage(tp)
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return items

    def gfx_text(self, data, queries=None):
        """A page's CPDF_TextPage reading-order characters (collapsing generated
        CRLF pairs into 000a:1:0 and marking soft-wrapped hyphens as 002d:0:1),
        merged selection rect count (`FPDFText_CountRects`), and optional
        `FPDFText_FindStart` search matches in logical character indices."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        tp = lib.FPDFText_LoadPage(page)
        n = lib.FPDFText_CountChars(tp)
        tokens = []
        raw_to_logical = [0] * (max(n, 0) + 1)
        i = 0
        while i < n:
            u = lib.FPDFText_GetUnicode(tp, i)
            gen = 1 if lib.FPDFText_IsGenerated(tp, i) == 1 else 0
            hyp = 1 if lib.FPDFText_IsHyphen(tp, i) == 1 else 0
            idx = len(tokens)
            if (gen == 1 and u == 0x0D and i + 1 < n
                    and lib.FPDFText_GetUnicode(tp, i + 1) == 0x0A
                    and lib.FPDFText_IsGenerated(tp, i + 1) == 1):
                raw_to_logical[i] = idx
                raw_to_logical[i + 1] = idx
                tokens.append("000a:1:0")
                i += 2
            elif hyp == 1:
                raw_to_logical[i] = idx
                tokens.append("002d:0:1")
                i += 1
            else:
                raw_to_logical[i] = idx
                tokens.append("%04x:%d:0" % (u, gen))
                i += 1
        raw_to_logical[max(n, 0)] = len(tokens)
        rc = max(lib.FPDFText_CountRects(tp, 0, -1), 0)
        parts = ["chars " + (" ".join(tokens) if tokens else "none"), "rects %d" % rc]
        if queries:
            for line in queries.decode("utf-8").splitlines():
                if not line:
                    continue
                q_str, fl_str = line.split("\t")
                fl = int(fl_str)
                u16_bytes = q_str.encode("utf-16-le")
                u16 = [u16_bytes[k] | (u16_bytes[k + 1] << 8) for k in range(0, len(u16_bytes), 2)] + [0]
                arr = (ctypes.c_ushort * len(u16))(*u16)
                h = lib.FPDFText_FindStart(tp, arr, fl, 0)
                hits = []
                if h:
                    while lib.FPDFText_FindNext(h):
                        rs = lib.FPDFText_GetSchResultIndex(h)
                        rcnt = lib.FPDFText_GetSchCount(h)
                        if rcnt > 0:
                            ls = raw_to_logical[rs]
                            le = raw_to_logical[rs + rcnt - 1] + 1
                            hits.append("%d..%d" % (ls, le))
                    lib.FPDFText_FindClose(h)
                parts.append("find %s %d %s" % (esc(q_str.encode("utf-8")), fl,
                                                " ".join(hits) if hits else "none"))
        lib.FPDFText_ClosePage(tp)
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return parts

    def forms(self, data):
        """Page 0's widgets as PDFium's form-fill environment reads their
        fields (fpdf_formfill.h, fpdf_annot.h): each one's field type, its
        flags, its fully qualified name, and a text field's value."""
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        # FPDF_FORMFILLINFO at version 2, every callback null: reading fields
        # calls none of them.
        info = ctypes.create_string_buffer(1024)
        ctypes.cast(info, ctypes.POINTER(ctypes.c_int))[0] = 2
        form = lib.FPDFDOC_InitFormFillEnvironment(doc, info)
        page = lib.FPDF_LoadPage(doc, 0)
        if not page or not form:
            if form:
                lib.FPDFDOC_ExitFormFillEnvironment(form)
            lib.FPDF_CloseDocument(doc)
            return None
        lib.FORM_OnAfterLoadPage(page, form)

        def text(get, a):
            size = get(form, a, None, 0)
            if size <= 2:
                return b""
            sbuf = ctypes.create_string_buffer(size)
            get(form, a, sbuf, size)
            return sbuf.raw[:size - 2].decode("utf-16le", "replace").encode("utf-8")

        out = []
        for i in range(lib.FPDFPage_GetAnnotCount(page)):
            a = lib.FPDFPage_GetAnnot(page, i)
            if not a:
                continue
            kind = lib.FPDFAnnot_GetFormFieldType(form, a)
            if kind < 0:
                out.append("idx %d none" % i)
            else:
                words = ["idx", str(i), "type", str(kind),
                         "flags", str(lib.FPDFAnnot_GetFormFieldFlags(form, a) & 0xFFFFFFFF),
                         "name", esc(text(lib.FPDFAnnot_GetFormFieldName, a))]
                if kind == 6:
                    words += ["value", esc(text(lib.FPDFAnnot_GetFormFieldValue, a))]
                out.append(" ".join(words))
            lib.FPDFPage_CloseAnnot(a)
        lib.FORM_OnBeforeClosePage(page, form)
        lib.FPDF_ClosePage(page)
        lib.FPDFDOC_ExitFormFillEnvironment(form)
        lib.FPDF_CloseDocument(doc)
        return out

    def forms_kind(self, data):
        """The kind of form the document holds, as `FPDF_GetFormType` numbers
        it (fpdfview.h's FORMTYPE_*: 0 none, 1 an AcroForm, 2 XFA that is the
        whole form, 3 XFA beside an AcroForm); None where it doesn't load."""
        buf, doc = self._doc(data)
        if not doc:
            return None
        kind = self.lib.FPDF_GetFormType(doc)
        self.lib.FPDF_CloseDocument(doc)
        return kind

    def forms_ap(self, data):
        """The normal appearance PDFium makes for each of page 0's widgets
        that has none, under the form-fill environment `forms` sets up
        (fpdf_formfill.h): each widget's `/AP /N` as the file PDFium saves
        after (FPDF_SaveAsCopy, fpdf_save.h) has it, its `/BBox`, its
        `/Matrix` and its content read by `ap_record`; `idx N none` where
        PDFium made none. The content FPDFAnnot_GetAP gives is held to the
        saved stream's where both are ASCII, so a widget is never read with
        another's appearance."""
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        info = ctypes.create_string_buffer(1024)
        ctypes.cast(info, ctypes.POINTER(ctypes.c_int))[0] = 2
        form = lib.FPDFDOC_InitFormFillEnvironment(doc, info)
        page = lib.FPDF_LoadPage(doc, 0)
        if not page or not form:
            if form:
                lib.FPDFDOC_ExitFormFillEnvironment(form)
            lib.FPDF_CloseDocument(doc)
            return None
        lib.FORM_OnAfterLoadPage(page, form)
        given = []
        for i in range(lib.FPDFPage_GetAnnotCount(page)):
            a = lib.FPDFPage_GetAnnot(page, i)
            given.append(self._u16(lib.FPDFAnnot_GetAP, a, 0) if a else None)
            if a:
                lib.FPDFPage_CloseAnnot(a)
        chunks = []

        def write(_, block, size):
            chunks.append(ctypes.string_at(block, size))
            return 1

        writer = FileWrite(1, FileWrite._fields_[1][1](write))
        saved = lib.FPDF_SaveAsCopy(doc, ctypes.byref(writer), 0)
        lib.FORM_OnBeforeClosePage(page, form)
        lib.FPDF_ClosePage(page)
        lib.FPDFDOC_ExitFormFillEnvironment(form)
        lib.FPDF_CloseDocument(doc)
        if not saved:
            raise SystemExit("forms-ap: PDFium didn't save the file")
        objects = saved_objects(b"".join(chunks))

        def get(value):
            while isinstance(value, Ref):
                value = objects.get(value[0], (None, None))[0]
            return value

        pages = [v for v, _ in objects.values() if isinstance(v, dict) and v.get(b"Type") == Name(b"Page")]
        if len(pages) != 1:
            raise SystemExit("forms-ap: a case's file has %d pages, not one" % len(pages))
        annots = get(pages[0].get(b"Annots")) or []
        if len(annots) != len(given):
            raise SystemExit("forms-ap: %d annotations saved, %d read" % (len(annots), len(given)))
        out = []
        for i, entry in enumerate(annots):
            ap = get((get(entry) or {}).get(b"AP"))
            n = ap.get(b"N") if isinstance(ap, dict) else None
            if not isinstance(n, Ref) or not isinstance(objects.get(n[0], (None, None))[1], bytes):
                out.append("idx %d none" % i)
                continue
            dictionary, content = objects[n[0]]
            if given[i] is not None and max(content, default=0) < 0x80 and given[i] != content:
                raise SystemExit("forms-ap: annotation %d's saved appearance isn't the one PDFium gave" % i)
            bbox, matrix = get(dictionary.get(b"BBox")), get(dictionary.get(b"Matrix"))
            if not (isinstance(bbox, list) and len(bbox) == 4 and isinstance(matrix, list) and len(matrix) == 6
                    and all(isinstance(v, float) for v in bbox + matrix)):
                raise SystemExit("forms-ap: annotation %d's /BBox or /Matrix isn't as PDFium writes them" % i)
            out.append(" ".join(["idx", str(i), "bbox"] + [num(f32(v)) for v in bbox] + ["matrix"]
                                + [num(f32(v)) for v in matrix]) + " " + ap_record(content))
        return out

    def _form_saved(self, data, kind, info=None, act=None):
        """A file loaded under a form-fill environment (fpdf_formfill.h),
        page 0 loaded into it, `act(page, form)` run where given, and the
        file saved (FPDF_SaveAsCopy, fpdf_save.h): the saved file's objects,
        its page's `/Annots` as read from them, a function that follows a
        reference among them, and each annotation's normal appearance as
        FPDFAnnot_GetAP gave it before the save. `info` is the environment's
        FPDF_FORMFILLINFO, else `forms`' buffer, every callback null. None
        where the file or its page doesn't load."""
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        if info is None:
            info = ctypes.create_string_buffer(1024)
            ctypes.cast(info, ctypes.POINTER(ctypes.c_int))[0] = 2
        form = lib.FPDFDOC_InitFormFillEnvironment(doc, ctypes.byref(info))
        page = lib.FPDF_LoadPage(doc, 0)
        if not page or not form:
            if form:
                lib.FPDFDOC_ExitFormFillEnvironment(form)
            lib.FPDF_CloseDocument(doc)
            return None
        lib.FORM_OnAfterLoadPage(page, form)
        if act is not None:
            act(page, form)
        given = []
        for i in range(lib.FPDFPage_GetAnnotCount(page)):
            a = lib.FPDFPage_GetAnnot(page, i)
            given.append(self._u16(lib.FPDFAnnot_GetAP, a, 0) if a else None)
            if a:
                lib.FPDFPage_CloseAnnot(a)
        chunks = []

        def write(_, block, size):
            chunks.append(ctypes.string_at(block, size))
            return 1

        writer = FileWrite(1, FileWrite._fields_[1][1](write))
        saved = lib.FPDF_SaveAsCopy(doc, ctypes.byref(writer), 0)
        lib.FORM_OnBeforeClosePage(page, form)
        lib.FPDF_ClosePage(page)
        lib.FPDFDOC_ExitFormFillEnvironment(form)
        lib.FPDF_CloseDocument(doc)
        if not saved:
            raise SystemExit("%s: PDFium didn't save the file" % kind)
        objects = saved_objects(b"".join(chunks))

        def get(value):
            seen = set()
            while isinstance(value, Ref) and value[0] not in seen:
                seen.add(value[0])
                value = objects.get(value[0], (None, None))[0]
            return None if isinstance(value, Ref) else value

        pages = [v for v, _ in objects.values() if isinstance(v, dict) and v.get(b"Type") == Name(b"Page")]
        if len(pages) != 1:
            raise SystemExit("%s: a case's file has %d pages, not one" % (kind, len(pages)))
        annots = get(pages[0].get(b"Annots")) or []
        if len(annots) != len(given):
            raise SystemExit("%s: %d annotations saved, %d read" % (kind, len(annots), len(given)))
        return objects, [get(entry) or {} for entry in annots], get, given

    def forms_ap_all(self, data):
        """Every appearance of each of page 0's widgets once PDFium has made
        what it makes, under the form-fill environment `forms_ap` sets up,
        read back from the file it saves: the widget's `/AS` (`as` and
        `value_word`'s word), then each of `/AP`'s `/N`, `/D` and `/R` there
        is, in that order, as `ap N` where it is a stream, or `ap N/STATE`
        for each state of a state dictionary in the order the file has them
        (the name escaped); each a stream's `/BBox`, `/Matrix` (the identity
        where it has none) and content by `ap_items`, or `none` where it is
        no stream. `ap none` where the widget has no `/AP`. The content
        FPDFAnnot_GetAP gives is held to the saved stream of `/N` (or of
        `/N`'s state `/AS` names) where both are ASCII."""
        found = self._form_saved(data, "forms-ap-all")
        if found is None:
            return None
        objects, annots, get, given = found

        def stream(value, where):
            if not isinstance(value, Ref) or not isinstance(objects.get(value[0], (None, None))[1], bytes):
                return None
            dictionary, content = objects[value[0]]
            bbox, matrix = get(dictionary.get(b"BBox")), get(dictionary.get(b"Matrix"))
            if matrix is None:
                matrix = list(IDENTITY)
            if not (isinstance(bbox, list) and len(bbox) == 4 and isinstance(matrix, list) and len(matrix) == 6
                    and all(isinstance(v, float) for v in bbox + matrix)):
                raise SystemExit("forms-ap-all: %s's /BBox or /Matrix isn't four and six numbers" % where)
            return content, " ".join(["bbox"] + [num(f32(v)) for v in bbox] + ["matrix"]
                                     + [num(f32(v)) for v in matrix]) + " " + ap_items(content)

        out = []
        for i, annot in enumerate(annots):
            state = get(annot.get(b"AS"))
            words = ["idx", str(i), "as", value_word(state)]
            ap = get(annot.get(b"AP"))
            entries = []
            normal = None
            if isinstance(ap, dict):
                for key in (b"N", b"D", b"R"):
                    if key not in ap:
                        continue
                    where = "annotation %d's %s" % (i, key.decode("ascii"))
                    made = stream(ap[key], where)
                    if made is not None:
                        entries.append("ap %s %s" % (key.decode("ascii"), made[1]))
                        normal = made[0] if key == b"N" else normal
                        continue
                    states = get(ap[key])
                    if not isinstance(states, dict):
                        entries.append("ap %s none" % key.decode("ascii"))
                        continue
                    for name, value in states.items():
                        label = "%s/%s" % (key.decode("ascii"), esc(name_text(name)))
                        made = stream(value, "annotation %d's %s" % (i, label))
                        entries.append("ap %s %s" % (label, made[1] if made else "none"))
                        if key == b"N" and made and isinstance(state, Name) and bytes(name) == bytes(state):
                            normal = made[0]
            if given[i] is not None and normal is not None and max(normal, default=0) < 0x80 \
                    and given[i] != normal:
                raise SystemExit("forms-ap-all: annotation %d's saved appearance isn't the one PDFium gave" % i)
            out.append(" ".join(words + (entries or ["ap", "none"])))
        return out

    def forms_press(self, data, presses):
        """Each of page 0's widgets' `/AS`, and each field's `/V`, in the file
        PDFium saves after the widgets `presses` gives (page 0's annotation
        indices, in order) are pressed under a form-fill environment whose
        first callbacks each do nothing (or answer page 0): by the pointer,
        `FORM_OnMouseMove`, `FORM_OnLButtonDown` and `FORM_OnLButtonUp` at the
        widget's centre. `idx N as` and the widget's `/AS`, then
        `field`, each field's full name from `/Fields` down, `v` and its own
        `/V`, each as `value_word` writes it. A press PDFium doesn't take
        stops the recording."""
        lib = self.lib
        info = FormFillInfo()
        info.version = 2
        held = {}
        page_of = []

        def nothing(*_):
            return None

        answers = {
            "FFI_SetTimer": lambda *_: 0,
            "FFI_GetPage": lambda _this, _doc, index: page_of[0] if index == 0 and page_of else None,
            "FFI_GetCurrentPage": lambda *_: page_of[0] if page_of else None,
            "FFI_GetRotation": lambda *_: 0,
        }
        for name, signature in FORM_CALLBACKS.items():
            if signature is None or name == "Release":
                continue
            res, args = signature
            held[name] = ctypes.CFUNCTYPE(res, *args)(answers.get(name, nothing))
            setattr(info, name, ctypes.cast(held[name], ctypes.c_void_p))
        taken = []

        def act(page, form):
            page_of.append(page)
            for index in presses:
                a = lib.FPDFPage_GetAnnot(page, index)
                if not a:
                    raise SystemExit("forms-press: no annotation %d to press" % index)
                rect = FSRect()
                lib.FPDFAnnot_GetRect(a, ctypes.byref(rect))
                x, y = (rect.left + rect.right) / 2.0, (rect.top + rect.bottom) / 2.0
                lib.FORM_OnMouseMove(form, page, 0, x, y)
                down = lib.FORM_OnLButtonDown(form, page, 0, x, y)
                up = lib.FORM_OnLButtonUp(form, page, 0, x, y)
                taken.append(down and up)
                lib.FPDFPage_CloseAnnot(a)
            lib.FORM_ForceToKillFocus(form)

        found = self._form_saved(data, "forms-press", info, act)
        if found is None:
            return None
        if not all(taken):
            raise SystemExit("forms-press: PDFium didn't take the press on %s"
                             % " ".join(str(p) for p, t in zip(presses, taken) if not t))
        objects, annots, get, _ = found
        out = ["idx %d as %s" % (i, value_word(get(annot.get(b"AS")))) for i, annot in enumerate(annots)]
        catalog = [v for v, _ in objects.values() if isinstance(v, dict) and v.get(b"Type") == Name(b"Catalog")]
        form = get(catalog[0].get(b"AcroForm")) if len(catalog) == 1 else None
        seen = set()

        def walk(refs, prefix):
            for ref in get(refs) or []:
                node = get(ref)
                if not isinstance(node, dict) or (isinstance(ref, Ref) and ref[0] in seen):
                    continue
                if isinstance(ref, Ref):
                    seen.add(ref[0])
                part = get(node.get(b"T"))
                if isinstance(part, Name) or not isinstance(part, bytes):
                    part = None
                name = prefix if part is None else (prefix + b"." if prefix else b"") + part
                if part is not None:
                    out.append("field %s v %s" % (esc(name), value_word(get(node.get(b"V")))))
                walk(node.get(b"Kids"), name)

        if isinstance(form, dict):
            walk(form.get(b"Fields"), b"")
        return out

    def annots(self, data):
        """Page 0's annotations, links, destinations, and actions as PDFium
        exposes them via fpdf_annot.h and fpdf_doc.h."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        out = []
        count = lib.FPDFPage_GetAnnotCount(page)
        for i in range(count):
            a = lib.FPDFPage_GetAnnot(page, i)
            if not a:
                continue
            r = FSRect()
            lib.FPDFAnnot_GetRect(a, ctypes.byref(r))
            sub = lib.FPDFAnnot_GetSubtype(a)
            flags = lib.FPDFAnnot_GetFlags(a) & 0xFFFFFFFF
            words = ["idx", str(i), "sub", str(sub), "flags", str(flags),
                     "rect", num(r.left), num(r.bottom), num(r.right), num(r.top)]
            hr, vr, bw = ctypes.c_float(), ctypes.c_float(), ctypes.c_float()
            if lib.FPDFAnnot_GetBorder(a, ctypes.byref(hr), ctypes.byref(vr), ctypes.byref(bw)):
                words += ["border", num(hr.value), num(vr.value), num(bw.value)]
            for key, label in ((b"Contents", "contents"), (b"T", "author"), (b"NM", "nm")):
                slen = lib.FPDFAnnot_GetStringValue(a, key, None, 0)
                if slen > 2:
                    sbuf = ctypes.create_string_buffer(slen)
                    lib.FPDFAnnot_GetStringValue(a, key, sbuf, slen)
                    txt = sbuf.raw[:slen - 2].decode("utf-16le", "replace").encode("utf-8")
                    if txt:
                        words += [label, esc(txt)]
            if lib.FPDFAnnot_HasAttachmentPoints(a):
                qc = lib.FPDFAnnot_CountAttachmentPoints(a)
                if qc > 0:
                    words += ["quads", str(qc)]
                    for qi in range(qc):
                        q = FSQuadPoints()
                        lib.FPDFAnnot_GetAttachmentPoints(a, qi, ctypes.byref(q))
                        words += [num(q.x1), num(q.y1), num(q.x2), num(q.y2),
                                  num(q.x3), num(q.y3), num(q.x4), num(q.y4)]
            sp, ep = FSPoint(), FSPoint()
            if lib.FPDFAnnot_GetLine(a, ctypes.byref(sp), ctypes.byref(ep)):
                words += ["line", num(sp.x), num(sp.y), num(ep.x), num(ep.y)]
            vc = lib.FPDFAnnot_GetVertices(a, None, 0)
            if vc > 0:
                vbuf = (FSPoint * vc)()
                lib.FPDFAnnot_GetVertices(a, vbuf, vc)
                words += ["verts", str(vc)]
                for k in range(vc):
                    words += [num(vbuf[k].x), num(vbuf[k].y)]
            ic = lib.FPDFAnnot_GetInkListCount(a)
            if ic > 0:
                words += ["ink", str(ic)]
                for si in range(ic):
                    pc = lib.FPDFAnnot_GetInkListPath(a, si, None, 0)
                    pbuf = (FSPoint * max(pc, 1))()
                    if pc > 0:
                        lib.FPDFAnnot_GetInkListPath(a, si, pbuf, pc)
                    words += ["stroke", str(pc)]
                    for k in range(pc):
                        words += [num(pbuf[k].x), num(pbuf[k].y)]
            lnk = lib.FPDFAnnot_GetLink(a)
            if lnk:
                d = lib.FPDFLink_GetDest(doc, lnk)
                if d:
                    pidx = lib.FPDFDest_GetDestPageIndex(doc, d)
                    np = ctypes.c_ulong()
                    params = (ctypes.c_float * 4)()
                    vw = lib.FPDFDest_GetView(d, ctypes.byref(np), params)
                    words += ["dest", str(pidx), str(vw)]
                    if vw == 1:
                        hx, hy, hz = ctypes.c_int(), ctypes.c_int(), ctypes.c_int()
                        vx, vy, vz = ctypes.c_float(), ctypes.c_float(), ctypes.c_float()
                        lib.FPDFDest_GetLocationInPage(d, ctypes.byref(hx), ctypes.byref(hy), ctypes.byref(hz),
                                                       ctypes.byref(vx), ctypes.byref(vy), ctypes.byref(vz))
                        words += [num(vx.value) if hx.value else "null",
                                  num(vy.value) if hy.value else "null",
                                  num(vz.value) if hz.value else "null"]
                    elif vw in (3, 4, 7, 8):
                        words += [num(params[0]) if np.value >= 1 else "none"]
                    elif vw == 5 and np.value >= 4:
                        r0, r1 = min(params[0], params[2]), min(params[1], params[3])
                        r2, r3 = max(params[0], params[2]), max(params[1], params[3])
                        words += [num(r0), num(r1), num(r2), num(r3)]
                act = lib.FPDFLink_GetAction(lnk)
                if act:
                    at = lib.FPDFAction_GetType(act)
                    words += ["act", str(at)]
                    if at == 3:
                        ulen = lib.FPDFAction_GetURIPath(doc, act, None, 0)
                        if ulen > 1:
                            ubuf = ctypes.create_string_buffer(ulen)
                            lib.FPDFAction_GetURIPath(doc, act, ubuf, ulen)
                            words += ["uri", esc(ubuf.raw[:ulen - 1])]
                    elif at in (2, 4):
                        flen = lib.FPDFAction_GetFilePath(act, None, 0)
                        if flen > 1:
                            fbuf = ctypes.create_string_buffer(flen)
                            lib.FPDFAction_GetFilePath(act, fbuf, flen)
                            words += ["file", esc(fbuf.raw[:flen - 1])]
            lib.FPDFPage_CloseAnnot(a)
            out.append(" ".join(words))
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return out

    def annots_flatten(self, data, print_flag):
        """Flattens page 0 via `FPDFPage_Flatten(page, flag)` (`0` = display,
        `1` = print), reloads page 0, and returns the flatten status, remaining
        `/Annots` count, and world-space path segments of every flattened object."""
        lib = self.lib
        buf, doc = self._doc(data)
        page = lib.FPDF_LoadPage(doc, 0) if doc else None
        if not page:
            if doc:
                lib.FPDF_CloseDocument(doc)
            return None
        status = lib.FPDFPage_Flatten(page, 1 if print_flag else 0)
        lib.FPDF_ClosePage(page)
        page = lib.FPDF_LoadPage(doc, 0)
        rem = lib.FPDFPage_GetAnnotCount(page)

        def mat_mul(a, b):
            return (
                a[0] * b[0] + a[1] * b[2],
                a[0] * b[1] + a[1] * b[3],
                a[2] * b[0] + a[3] * b[2],
                a[2] * b[1] + a[3] * b[3],
                a[4] * b[0] + a[5] * b[2] + b[4],
                a[4] * b[1] + a[5] * b[3] + b[5],
            )

        def mat_apply(m, x, y):
            return (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])

        items = ["status %d rem %d" % (status, rem)]

        def walk(o, parent_m):
            t = TYPES.get(lib.FPDFPageObj_GetType(o), "unknown")
            m = M()
            lib.FPDFPageObj_GetMatrix(o, ctypes.byref(m))
            world = mat_mul((m.a, m.b, m.c, m.d, m.e, m.f), parent_m)
            if t == "form":
                for i in range(max(lib.FPDFFormObj_CountObjects(o), 0)):
                    walk(lib.FPDFFormObj_GetObject(o, i), world)
                return
            if t == "path":
                fill, stroke = ctypes.c_int(), ctypes.c_int()
                lib.FPDFPath_GetDrawMode(o, ctypes.byref(fill), ctypes.byref(stroke))
                words = ["path", "fill", str(fill.value), "stroke", str(int(bool(stroke.value)))]
                for i in range(lib.FPDFPath_CountSegments(o)):
                    s = lib.FPDFPath_GetPathSegment(o, i)
                    x, y = ctypes.c_float(), ctypes.c_float()
                    lib.FPDFPathSegment_GetPoint(s, ctypes.byref(x), ctypes.byref(y))
                    px, py = mat_apply(world, x.value, y.value)
                    words += [SEGMENTS.get(lib.FPDFPathSegment_GetType(s), "?"), num(px), num(py)]
                    if lib.FPDFPathSegment_GetClose(s):
                        words.append("h")
                items.append(" ".join(words))

        ident = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
        for i in range(lib.FPDFPage_CountObjects(page)):
            walk(lib.FPDFPage_GetObject(page, i), ident)
        lib.FPDF_ClosePage(page)
        lib.FPDF_CloseDocument(doc)
        return items

    def _u16(self, fn, *args):
        n = fn(*args, None, 0)
        if n == 0:
            return None
        b = ctypes.create_string_buffer(max(n, 2))
        fn(*args, b, n)
        return b.raw[:max(n - 2, 0)].decode("utf-16le", "replace").encode("utf-8")

    def outlines(self, data):
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        out = []
        seen = set()

        def walk_siblings(parent, depth):
            if depth >= 32:
                return
            bm = lib.FPDFBookmark_GetFirstChild(doc, parent)
            while bm and bm not in seen and len(out) < 256:
                seen.add(bm)
                title = self._u16(lib.FPDFBookmark_GetTitle, bm) or b""
                count = lib.FPDFBookmark_GetCount(bm)
                style = lib.FPDFBookmark_GetStyle(bm)
                words = ["bm", "depth", str(depth), "title", esc(title), "count", str(count), "style", str(style)]
                r, g, b = ctypes.c_float(), ctypes.c_float(), ctypes.c_float()
                if lib.FPDFBookmark_GetColor(bm, ctypes.byref(r), ctypes.byref(g), ctypes.byref(b)):
                    words += ["color", num(r.value), num(g.value), num(b.value)]
                d = lib.FPDFBookmark_GetDest(doc, bm)
                if d:
                    words += ["dest_page", str(lib.FPDFDest_GetDestPageIndex(doc, d))]
                act = lib.FPDFBookmark_GetAction(bm)
                if act:
                    at = lib.FPDFAction_GetType(act)
                    words += ["act", str(at)]
                    if at == 3:
                        ulen = lib.FPDFAction_GetURIPath(doc, act, None, 0)
                        if ulen > 1:
                            ubuf = ctypes.create_string_buffer(ulen)
                            lib.FPDFAction_GetURIPath(doc, act, ubuf, ulen)
                            words += ["uri", esc(ubuf.raw[:ulen - 1])]
                out.append(" ".join(words))
                walk_siblings(bm, depth + 1)
                bm = lib.FPDFBookmark_GetNextSibling(doc, bm)

        walk_siblings(None, 0)
        lib.FPDF_CloseDocument(doc)
        return out

    def labels(self, data):
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        count = lib.FPDF_GetPageCount(doc)
        each = []
        for idx in range(min(count, PAGES) + 1):
            lbl = self._u16(lib.FPDF_GetPageLabel, doc, idx)
            each.append("null" if lbl is None else esc(lbl))
        lib.FPDF_CloseDocument(doc)
        return "count %d : %s" % (count, " ; ".join(each))

    def attachments(self, data):
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        n = lib.FPDFDoc_GetAttachmentCount(doc)
        out = []
        for i in range(max(n, 0)):
            att = lib.FPDFDoc_GetAttachment(doc, i)
            if not att:
                out.append("att %d null" % i)
                continue
            name = self._u16(lib.FPDFAttachment_GetName, att) or b""
            desc = self._u16(lib.FPDFAttachment_GetDescription, att) or b""
            sub = self._u16(lib.FPDFAttachment_GetSubtype, att) or b""
            rel = self._u16(lib.FPDFAttachment_GetAFRelationship, att) or b""
            olen = ctypes.c_ulong(0)
            if lib.FPDFAttachment_GetFile(att, None, 0, ctypes.byref(olen)):
                fbuf = ctypes.create_string_buffer(max(olen.value, 1))
                lib.FPDFAttachment_GetFile(att, fbuf, olen.value, ctypes.byref(olen))
                fword = esc(fbuf.raw[:olen.value])
            else:
                fword = "unreadable"
            out.append("att %d name %s desc %s sub %s rel %s data %s" % (
                i, esc(name), esc(desc), esc(sub), esc(rel), fword))
        lib.FPDF_CloseDocument(doc)
        return out

    def viewer(self, data):
        lib = self.lib
        buf, doc = self._doc(data)
        if not doc:
            return None
        scaling = lib.FPDF_VIEWERREF_GetPrintScaling(doc)
        copies = lib.FPDF_VIEWERREF_GetNumCopies(doc)
        duplex = lib.FPDF_VIEWERREF_GetDuplex(doc)
        pr = lib.FPDF_VIEWERREF_GetPrintPageRange(doc)
        pr_elems = []
        if pr:
            prc = lib.FPDF_VIEWERREF_GetPrintPageRangeCount(pr)
            pr_elems = [str(lib.FPDF_VIEWERREF_GetPrintPageRangeElement(pr, i)) for i in range(prc)]
        words = ["scaling", str(scaling), "copies", str(copies), "duplex", str(duplex),
                 "range", ",".join(pr_elems) if pr_elems else "none"]
        for key in (b"Direction", b"NonFullScreenPageMode", b"ViewArea", b"PrintArea"):
            nb = lib.FPDF_VIEWERREF_GetName(doc, key, None, 0)
            if nb > 1:
                cbuf = ctypes.create_string_buffer(nb)
                lib.FPDF_VIEWERREF_GetName(doc, key, cbuf, nb)
                words += [key.decode("ascii"), esc(cbuf.raw[:nb - 1])]
        lang = self._u16(lib.FPDFCatalog_GetLanguage, doc)
        words += ["lang", "null" if lang is None else esc(lang)]
        lib.FPDF_CloseDocument(doc)
        return " ".join(words)

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

    def avail(self, data, password=None):
        """A file given PDFium only as its availability API asks, its first
        1,024 bytes to start with, opened by `password` if there is one: what each
        stage took, the words `avail_answer` writes."""
        lib = self.lib
        have = bytearray(len(data))
        got = [0]
        wanted = []

        def give(at, length):
            length = min(length, len(data) - at)
            if length > 0:
                got[0] += length - sum(have[at:at + length])
                have[at:at + length] = b"\1" * length

        def is_avail(_, at, length):
            return 1 if at + length <= len(data) and all(have[at:at + length]) else 0

        def block(_, at, buffer, length):
            if at + length > len(data):
                return 0
            ctypes.memmove(buffer, bytes(data[at:at + length]), length)
            return 1

        def segment(_, at, length):
            wanted.append((at, length))

        # The callbacks are kept for as long as PDFium holds them.
        avail = FileAvail(1, FileAvail._fields_[1][1](is_avail))
        access = FileAccess(len(data), FileAccess._fields_[1][1](block), None)
        hints = DownloadHints(1, DownloadHints._fields_[1][1](segment))
        give(0, 1024)
        handle = lib.FPDFAvail_Create(ctypes.byref(avail), ctypes.byref(access))

        def until(stage):
            """Giving PDFium what it asks until the stage says other than not yet:
            what it says, or `stuck` where it asks for nothing."""
            for _ in range(4096):
                del wanted[:]
                said = stage()
                if said != 0:
                    return said
                if not wanted:
                    return "stuck"
                for at, length in wanted:
                    give(at, length)
            return "stuck"

        out = ["linearized", str(lib.FPDFAvail_IsLinearized(handle))]
        said = until(lambda: lib.FPDFAvail_IsDocAvail(handle, ctypes.byref(hints)))
        out += ["doc", str(said), "open", str(got[0])]
        doc = lib.FPDFAvail_GetDocument(handle, password) if said == 1 else None
        if not doc:
            lib.FPDFAvail_Destroy(handle)
            return out + ["no-document", "got", str(got[0]), "of", str(len(data))]
        count = lib.FPDF_GetPageCount(doc)
        first = lib.FPDFAvail_GetFirstPageNum(doc)
        out += ["count", str(count), "first", str(first)]
        for index in [first] + [p for p in range(min(count, PAGES)) if p != first]:
            before = got[0]
            said = until(lambda: lib.FPDFAvail_IsPageAvail(handle, index, ctypes.byref(hints)))
            page = lib.FPDF_LoadPage(doc, index)
            size = "null"
            if page:
                size = num(lib.FPDF_GetPageWidthF(page))
                lib.FPDF_ClosePage(page)
            out += [";", "page", str(index), str(said), "took", str(got[0] - before), "width", size]
        out += [";", "got", str(got[0]), "of", str(len(data))]
        lib.FPDF_CloseDocument(doc)
        lib.FPDFAvail_Destroy(handle)
        return out


def content_answer(objects):
    if objects is None:
        return "no-page"
    return " ; ".join(objects) or "none"


def size_answer(size):
    return "no-page" if size is None else "%s %s" % (num(size[0]), num(size[1]))


def spaces_answer(sizes):
    """Each image's components, as 8 samples of 8 bits each took: `none` for
    the 1 byte of an image with no color space."""
    if sizes is None:
        return "no-page"
    words = ["none" if n == 1 else str(n // 8) if n % 8 == 0 else "raw:%d" % n for n in sizes]
    return " ".join(words) or "no-images"


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
    if kind == "contents":
        return content_answer(pdfium.objects(contents_pdf(files["contents"], files["objects"])))
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
    if kind == "avail":
        return " ".join(pdfium.avail(files["pdf"], files.get("password")))
    if kind == "spaces":
        return spaces_answer(pdfium.image_sizes(files["pdf"]))
    if kind in ("fonts", "fonts-heights", "fonts-boxes"):
        return content_answer(pdfium.text_chars(files["pdf"], kind == "fonts-heights", kind == "fonts-boxes"))
    if kind == "gfx-geom":
        geom = pdfium.gfx_geom(files["pdf"])
        return "no-page" if geom is None else geom
    if kind == "gfx":
        return content_answer(pdfium.gfx(files["pdf"]))
    if kind == "gfx-text":
        return content_answer(pdfium.gfx_text(files["pdf"], files.get("queries")))
    if kind == "annots":
        return content_answer(pdfium.annots(files["pdf"]))
    if kind == "forms":
        return content_answer(pdfium.forms(files["pdf"]))
    if kind == "forms-kind":
        found = pdfium.forms_kind(files["pdf"])
        return "no-document" if found is None else "kind %d" % found
    if kind == "forms-ap":
        return content_answer(pdfium.forms_ap(files["pdf"]))
    if kind == "forms-ap-all":
        return content_answer(pdfium.forms_ap_all(files["pdf"]))
    if kind == "forms-press":
        return content_answer(pdfium.forms_press(files["pdf"], [int(w) for w in files["press"].split()]))
    if kind in ("annots-flatten-display", "annots-flatten-print"):
        return content_answer(pdfium.annots_flatten(files["pdf"], kind == "annots-flatten-print"))
    if kind == "outlines":
        ans = pdfium.outlines(files["pdf"])
        return "no-document" if ans is None else (" ; ".join(ans) or "none")
    if kind == "labels":
        ans = pdfium.labels(files["pdf"])
        return "no-document" if ans is None else ans
    if kind == "attachments":
        ans = pdfium.attachments(files["pdf"])
        return "no-document" if ans is None else (" ; ".join(ans) or "none")
    if kind == "viewer":
        ans = pdfium.viewer(files["pdf"])
        return "no-document" if ans is None else ans
    raise SystemExit("unknown kind %r" % kind)


def named_answer(pdfium, folder, name, kind, files):
    """`answer`, with a recording that stops naming the case it stopped at."""
    try:
        return answer(pdfium, kind, files)
    except SystemExit as stop:
        raise SystemExit("%s: %s: %s" % (folder, name, stop.code)) from None


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


def extra_crate_pins(crates_dir, checked_crates, build):
    """Hold every other workspace crate's `as of chromium/N` mentions in
    README.md, src/, and tests/ (including pdf-raster-oracle) to `build`, and
    require crates that emulate or score against PDFium (`pdf-images`,
    `pdf-raster`, `pdf-raster-oracle`, `pdfs`) to name `as of chromium/<build>`
    explicitly so a pin bump cannot leave them stale."""
    wrong = []
    if not os.path.isdir(crates_dir):
        return wrong
    must_name_readme = {"pdf-images", "pdf-raster", "pdfs"}
    must_name_any = {"pdf-raster-oracle"}
    for entry in sorted(os.listdir(crates_dir)):
        crate = os.path.join(crates_dir, entry)
        if not os.path.isdir(crate) or os.path.abspath(crate) in checked_crates:
            continue
        paths = []
        readme = os.path.join(crate, "README.md")
        if os.path.isfile(readme):
            paths.append(readme)
        for sub in ("src", "tests"):
            sub_dir = os.path.join(crate, sub)
            if os.path.isdir(sub_dir):
                for root, _, files in os.walk(sub_dir):
                    paths += [os.path.join(root, f) for f in sorted(files) if f.endswith(".rs")]
        named_in_readme = False
        named_anywhere = False
        for path in paths:
            with open(path, encoding="utf-8") as f:
                text = f.read()
            words = re.sub(r"\s*\n\s*(//[/!]?)?\s*", " ", text)
            for n in re.findall(r"\bas of chromium/(\d+)", words):
                if n != build:
                    wrong.append("%s: %s names chromium/%s, where the pin is chromium/%s"
                                 % (crate, os.path.relpath(path, crate), n, build))
                else:
                    named_anywhere = True
                    if path == readme:
                        named_in_readme = True
        if entry in must_name_readme and not named_in_readme:
            wrong.append("%s: README.md doesn't say it reads as of chromium/%s" % (crate, build))
        if entry in must_name_any and not named_anywhere:
            wrong.append("%s: doesn't name as of chromium/%s" % (crate, build))
    return wrong


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
        if rest:
            crates_dir = os.path.dirname(os.path.dirname(os.path.abspath(rest[0])))
            checked = {os.path.dirname(os.path.dirname(os.path.abspath(f))) for f in rest}
            build_pin = os.path.basename(recorded_file(rest[0]))[len("chromium-"):-len(".txt")]
            for line in extra_crate_pins(crates_dir, checked, build_pin):
                print(line)
                failed += 1
        sys.exit(1 if failed else 0)

    if command not in ("run", "check", "record") or not rest:
        raise SystemExit(__doc__)
    pdfium = Pdfium(rest[0])
    failed = 0
    for folder in rest[1:]:
        cases = read_cases(folder)
        answers = [(name, named_answer(pdfium, folder, name, kind, files)) for name, kind, files in cases]
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
