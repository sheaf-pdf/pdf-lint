"""pdf-structure's PDFium cases: files whose cross-reference sections or the
objects they list aren't where they say, or that don't give their pages as
written, read as PDFium reads a file, by the kind oracle/README.md
describes: a whole file, asked for its count and each page's size and
turns. A page's width is its object's number, so an answer names the pages
it found, and its height tells two objects of one number apart. Every
case's bytes are written here, from literals; oracle/pdfium.py's `cases`
writes them into cases/ and cases.tsv."""

HEADER = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n"


def page(number, height=200, parent=2):
    """A page whose width is its number."""
    return b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 %d %d] >>" % (parent, number, height)


def pages(kids, count=None):
    """A page tree root listing `kids`, with a /Count of `count` (theirs, by
    default)."""
    listed = b" ".join(b"%d 0 R" % kid for kid in kids)
    count = len(kids) if count is None else count
    return b"<< /Type /Pages /Kids [%s] /Count %d >>" % (listed, count)


def catalog(root=2):
    return b"<< /Type /Catalog /Pages %d 0 R >>" % root


def stream(dict_entries, data):
    """A stream's body: its dictionary, `/Length` given, and its data."""
    return b"<< %s /Length %d >>\nstream\n%s\nendstream" % (dict_entries, len(data), data)


def object_stream(members):
    """An object stream's body, unfiltered, of `members`, [(number, body)]."""
    offsets, data = [], b""
    for number, text in members:
        offsets.append(b"%d %d" % (number, len(data)))
        data += text + b"\n"
    head = b" ".join(offsets) + b"\n"
    return stream(b"/Type /ObjStm /N %d /First %d" % (len(members), len(head)), head + data)


class File:
    """A file written in order: objects, each where it's written, and
    sections, each where it starts."""

    def __init__(self, header=HEADER):
        self.out = bytearray(header)
        self.at = {}

    def obj(self, number, text, generation=0):
        self.at[number] = len(self.out)
        self.out += b"%d %d obj\n" % (number, generation) + text + b"\nendobj\n"
        return self.at[number]

    def raw(self, data):
        self.out += data

    def table(self, rows, trailer, start=0, startxref=None):
        """A section of one subsection from `start`, a row for each of
        `rows`, [offset, or None for a free row], then `trailer`'s entries,
        `startxref` (its own place, by default) and `%%EOF`."""
        xref = len(self.out)
        self.out += b"xref\n%d %d\n" % (start, len(rows))
        for row in rows:
            if row is None:
                self.out += b"0000000000 65535 f \n"
            else:
                offset, generation = row if isinstance(row, tuple) else (row, 0)
                self.out += b"%010d %05d n \n" % (offset, generation)
        given = xref if startxref is None else startxref
        self.out += b"trailer\n<< %s >>\nstartxref\n%d\n%%%%EOF\n" % (trailer, given)
        return xref

    def rows(self, top, moved=None):
        """A row for each number from 0 below `top`: object 0's, then each
        object's place, or free; `moved` gives some another place."""
        moved = moved or {}
        rows = [None]
        for number in range(1, top):
            rows.append(moved.get(number, self.at.get(number)))
        return rows

    def bytes(self):
        return bytes(self.out)


def plain():
    """A file as it should be: three pages."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4, 5]))
    for n in (3, 4, 5):
        f.obj(n, page(n))
    f.table(f.rows(6), b"/Size 6 /Root 1 0 R")
    return f.bytes()


def startxref_past_its_section():
    """startxref 3 bytes past `xref`."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4, 5]))
    for n in (3, 4, 5):
        f.obj(n, page(n))
    xref = len(f.out)
    f.table(f.rows(6), b"/Size 6 /Root 1 0 R", startxref=xref + 3)
    return f.bytes()


def cut(rows_after=True):
    """Three bytes cut out of page 3 after its section was written: the
    pages after it, and the section, 3 bytes before where it says. Where
    `rows_after` isn't, the section is written after the cut, where startxref
    puts it, and only its rows for pages 4 and 5 are 3 bytes past their
    objects."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4, 5]))
    at = f.obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200]    >>")
    f.obj(4, page(4))
    f.obj(5, page(5))
    rows = f.rows(6)
    pad = at + f.out[at:].index(b"    >>")
    if rows_after:
        f.table(rows, b"/Size 6 /Root 1 0 R")
        del f.out[pad:pad + 3]
    else:
        del f.out[pad:pad + 3]
        f.table(rows, b"/Size 6 /Root 1 0 R")
    return f.bytes()


def updated(prev=None, loop=False, hidden=False, xrefstm=None):
    """A file of pages 3 and 4, then an update that adds page 6 to its tree:
    the update's /Prev `prev` (the first section's place, by default); and,
    where `loop`, the first section's /Prev the update's own place. Where
    `hidden`, page 6 is written inside stream 7's data, where the update's
    section lists it: a scan doesn't find it. Where `xrefstm` is given, the
    update's trailer has it for its /XRefStm."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.obj(4, page(4))
    trailer = b"/Size 5 /Root 1 0 R" + (b" /Prev ##########" if loop else b"")
    first = f.table(f.rows(5), trailer)
    f.obj(2, pages([3, 4, 6]))
    if hidden:
        at = f.obj(7, stream(b"", b"6 0 obj\n" + page(6) + b"\nendobj\n"))
        f.at[6] = at + f.out[at:].index(b"6 0 obj")
    else:
        f.obj(6, page(6))
    xref = len(f.out)
    f.out += b"xref\n0 1\n0000000000 65535 f \n2 1\n%010d 00000 n \n6 1\n%010d 00000 n \n" % (f.at[2], f.at[6])
    stm = b"" if xrefstm is None else b" /XRefStm " + xrefstm
    f.out += b"trailer\n<< /Size 7 /Root 1 0 R /Prev %d%s >>\nstartxref\n%d\n%%%%EOF\n" % (
        first if prev is None else prev, stm, xref)
    return f.bytes().replace(b"##########", b"%010d" % xref)


PAD = b" /Pad (" + b"." * 80 + b")"


def first_object_elsewhere():
    """Object 1, a page written long, listed where page 2 is: its own `n g
    obj` more than 64 bytes before. The catalog is object 5."""
    f = File()
    f.obj(1, b"<< /Type /Page /Parent 4 0 R /MediaBox [0 0 1 200]" + PAD + b" >>")
    f.obj(2, page(2, parent=4))
    f.obj(3, page(3, parent=4))
    f.obj(4, pages([1, 2, 3]))
    f.obj(5, catalog(4))
    f.table(f.rows(6, {1: f.at[2]}), b"/Size 6 /Root 5 0 R")
    return f.bytes()


def first_object_elsewhere_and_xrefstm_unread():
    """Object 1, the catalog, listed where object 2 is, its own `n g obj`
    within 64 bytes before; then an update that writes the page tree anew,
    its kids 3 and 6 and no /Count, and lists page 6 inside stream 7's
    data, where a scan doesn't find it; then an update whose /XRefStm is
    far from any section."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3]))
    f.obj(3, page(3))
    f.obj(4, b"<< /Producer (cases) >>")
    first = f.table(f.rows(5, {1: f.at[2]}), b"/Size 5 /Root 1 0 R")
    f.obj(2, b"<< /Type /Pages /Kids [3 0 R 6 0 R] >>")
    at = f.obj(7, stream(b"", b"6 0 obj\n" + page(6) + b"\nendobj\n"))
    inside = at + f.out[at:].index(b"6 0 obj")
    middle = len(f.out)
    f.out += b"xref\n0 1\n0000000000 65535 f \n2 1\n%010d 00000 n \n6 2\n%010d 00000 n \n%010d 00000 n \n" % (
        f.at[2], inside, f.at[7])
    f.out += b"trailer\n<< /Size 8 /Root 1 0 R /Prev %d >>\nstartxref\n%d\n%%%%EOF\n" % (first, middle)
    f.obj(5, b"<< /Note (newest) >>")
    newest = len(f.out)
    f.out += b"xref\n0 1\n0000000000 65535 f \n5 1\n%010d 00000 n \n" % f.at[5]
    f.out += b"trailer\n<< /Size 8 /Root 1 0 R /Prev %d /XRefStm 9999999 >>\nstartxref\n%d\n%%%%EOF\n" % (
        middle, newest)
    return f.bytes()


def hidden_in_a_stream():
    """Page 1 listed 3 bytes before its object, and page 6 written inside
    stream 4's data, where the section lists it: a scan doesn't find it."""
    f = File()
    f.obj(1, page(1))
    f.obj(2, pages([1, 6]))
    f.obj(3, catalog())
    hidden = b"6 0 obj\n" + page(6) + b"\nendobj\n"
    at = f.obj(4, stream(b"", hidden))
    inside = at + f.out[at:].index(b"6 0 obj")
    f.table(f.rows(7, {1: f.at[1] - 3, 6: inside}), b"/Size 7 /Root 3 0 R")
    return f.bytes()


def prev_zero():
    """One section, its trailer's /Prev 0, the header's place, where no
    section is; page 6 written inside stream 4's data, where the section
    lists it: a scan doesn't find it."""
    f = File()
    f.obj(1, page(1))
    f.obj(2, pages([1, 6]))
    f.obj(3, catalog())
    hidden = b"6 0 obj\n" + page(6) + b"\nendobj\n"
    at = f.obj(4, stream(b"", hidden))
    inside = at + f.out[at:].index(b"6 0 obj")
    f.table(f.rows(7, {6: inside}), b"/Size 7 /Root 3 0 R /Prev 0")
    return f.bytes()


def catalog_elsewhere():
    """The catalog, object 5, listed where object 4 is, its own `n g obj`
    more than 64 bytes after: no object of the section's first or last
    is out of place."""
    f = File()
    f.obj(1, pages([2, 3]))
    f.obj(2, page(2, parent=1))
    f.obj(3, page(3, parent=1))
    f.obj(4, b"<< /Note (" + b"." * 90 + b") >>")
    f.obj(5, catalog(1))
    f.obj(6, b"<< /Producer (cases) >>")
    f.table(f.rows(7, {5: f.at[4]}), b"/Size 7 /Root 5 0 R /Info 6 0 R")
    return f.bytes()


def pages_elsewhere():
    """The page tree's root, object 2, listed where object 3 is, its own
    more than 64 bytes before; the catalog where it's listed."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, b"<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2" + PAD + b" >>")
    f.obj(3, b"<< /Note (three) >>")
    f.obj(4, page(4))
    f.obj(5, page(5))
    f.obj(6, b"<< /Producer (cases) >>")
    f.table(f.rows(7, {2: f.at[3]}), b"/Size 7 /Root 1 0 R /Info 6 0 R")
    return f.bytes()


def no_startxref():
    """No startxref: a file found by scanning."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.obj(4, page(4))
    f.raw(b"trailer\n<< /Size 5 /Root 1 0 R >>\n%%EOF\n")
    return f.bytes()


def stream_without_endstream():
    """No startxref, and a stream without `endstream` before the catalog and
    its pages: its data ends at its `endobj`, as PDFium reads it."""
    f = File()
    f.raw(b"9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendobj\n")
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.obj(4, page(4))
    f.raw(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n")
    return f.bytes()


def stream_with_no_end():
    """No startxref, and a stream with neither `endstream` nor `endobj` after
    its data, then pages 3 and 4 without `endobj` either: PDFium finds no
    end to its data, reads on through it from where it starts, and finds
    both pages."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.raw(b"trailer\n<< /Size 10 /Root 1 0 R >>\n")
    f.raw(b"9 0 obj\n<< /Length 999 >>\nstream\nq Q\n")
    f.raw(b"3 0 obj\n" + page(3) + b"\n4 0 obj\n" + page(4) + b"\n")
    return f.bytes()


def endstream_run_into_endobj():
    """No startxref, and a stream whose data ends `endstreamendobj`, neither a
    word of its own, before page 4: PDFium ends its data at page 4's
    `endobj`, the first word of its own after it, and doesn't find page 4."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.raw(b"9 0 obj\n<< /Length 999 >>\nstream\nq Q\nendstreamendobj\n")
    f.obj(4, page(4))
    f.raw(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n")
    return f.bytes()


def stream_of_no_object():
    """No startxref, and a stream no `n g obj` comes before, page 4 in its
    data before `endobj` and `endstream`: PDFium's scan reads through it as
    through any words but an object's, and finds page 4."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.raw(b"<< /Length 999 >>\nstream\n")
    f.obj(4, page(4))
    f.raw(b"endstream\ntrailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n")
    return f.bytes()


def data_on_the_stream_line():
    """No startxref, and object stream 9, page 4 its member, its data and
    `endstream` on its `stream` keyword's line, `endobj` on the next, its
    /Length past its data: PDFium starts the data on the next line, finds
    no `endstream` after it, and reads no member."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    head = b" 4 0 "
    f.raw(b"9 0 obj\n<< /Type /ObjStm /N 1 /First %d /Length 999 >>\nstream%s%s endstream\nendobj\n"
          % (len(head), head, page(4)))
    f.raw(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n")
    return f.bytes()


def unclosed(where):
    """No startxref, and a literal string that doesn't close before the
    catalog and its pages: as object 9's dictionary's value, between
    objects, or where object 9's dictionary has a key. PDFium reads it to
    the file's end where it reads it as a string, a value or a `(` between
    objects, and finds no catalog; where a key goes it passes over it."""
    f = File()
    if where == "value":
        f.raw(b"9 0 obj\n<< /Title (open /Producer (x) >>\nendobj\n")
    elif where == "between":
        f.obj(9, b"<< /Title (closed) >>")
        f.raw(b"(open\n")
    else:
        f.raw(b"9 0 obj\n<< /Title (closed) ( /Producer (x) >>\nendobj\n")
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.obj(4, page(4))
    f.raw(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n")
    return f.bytes()


def unclosed_later_copy():
    """No startxref, and the catalog, its pages and the trailer, then a
    literal string that doesn't close between objects and page 4 again, its
    dictionary cut short. PDFium reads the string to the file's end, and
    page 4 where it's first written."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.obj(4, page(4))
    f.raw(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n")
    f.raw(b"(open\n4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 4 300]\n")
    return f.bytes()


def closed_late():
    """No startxref, and object 9's /Title opening a literal string that
    lacks its `)`, so that it closes late, at the second `)` of object 5's
    /Author, closed twice, past page 4, whose `n g obj` starts a line inside
    it; then the catalog, its pages and page 3. PDFium reads the string as
    the dictionary's value, page 4 among its bytes, and doesn't find page
    4."""
    f = File()
    f.raw(b"9 0 obj\n<< /Title (nine >>\nendobj\n")
    f.obj(4, page(4))
    f.obj(5, b"<< /Author (five)) >>")
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.raw(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n")
    return f.bytes()


def member_and_object(member_first):
    """No startxref, and page 3 twice: at an offset, 300 high, and in object
    stream 9, 200 high, the stream first where `member_first`."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(4, page(4))
    if member_first:
        f.obj(9, object_stream([(3, page(3))]))
        f.obj(3, page(3, 300))
    else:
        f.obj(3, page(3, 300))
        f.obj(9, object_stream([(3, page(3))]))
    f.raw(b"trailer\n<< /Size 10 /Root 1 0 R >>\n%%EOF\n")
    return f.bytes()


def xref_stream_body(rows, size, index):
    """A cross-reference stream's body, unfiltered: `rows`, each (type,
    second field, third field), by /W [1 2 1]."""
    data = b"".join(bytes([t]) + b.to_bytes(2, "big") + bytes([c]) for t, b, c in rows)
    entries = b"/Type /XRef /W [1 2 1] /Size %d /Index [%s]" % (size, b" ".join(b"%d" % i for i in index))
    return stream(entries, data)


def xrefstm_unread():
    """An update whose table is a hybrid's, its /XRefStm far from any
    section: page 6, which only the stream lists, in object stream 7."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.obj(4, page(4))
    first = f.table(f.rows(5), b"/Size 5 /Root 1 0 R")
    f.obj(2, pages([3, 4, 6]))
    f.obj(7, object_stream([(6, page(6))]))
    f.obj(8, xref_stream_body([(2, 7, 0), (1, f.at[7], 0)], 9, [6, 2]))
    xref = len(f.out)
    f.out += b"xref\n0 1\n0000000000 65535 f \n2 1\n%010d 00000 n \n6 3\n0000000000 00000 f \n%010d 00000 n \n%010d 00000 n \n" % (
        f.at[2], f.at[7], f.at[8])
    f.out += b"trailer\n<< /Size 9 /Root 1 0 R /Prev %d /XRefStm 9999999 >>\nstartxref\n%d\n%%%%EOF\n" % (first, xref)
    return f.bytes()


def misnumbered():
    """A section numbered from 1 that starts with object 0's free row, so
    each row is one number past its object's; the catalog, object 1, has
    no row of its own but that free one. Objects are of lengths apart, so
    no one shift moves them all."""
    f = File()
    f.obj(1, catalog())
    f.obj(2, pages([3, 4]))
    f.obj(3, page(3))
    f.obj(4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 4 200] /Note (four) >>")
    rows = [None, f.at[1], f.at[2], f.at[3], f.at[4]]
    f.table(rows, b"/Size 6 /Root 1 0 R", start=1)
    return f.bytes()


CASES = {
    "plain": plain(),
    # Sections not where they're said.
    "startxref-past-its-section": startxref_past_its_section(),
    "cut": cut(),
    "rows-past-a-cut": cut(rows_after=False),
    "no-startxref": no_startxref(),
    "stream-without-endstream": stream_without_endstream(),
    "stream-with-no-end": stream_with_no_end(),
    "endstream-run-into-endobj": endstream_run_into_endobj(),
    "stream-of-no-object": stream_of_no_object(),
    "stream-data-on-its-stream-line": data_on_the_stream_line(),
    # A string that doesn't close, in a file found by scanning.
    "unclosed-string-value": unclosed("value"),
    "unclosed-string-between": unclosed("between"),
    "unclosed-string-key": unclosed("key"),
    "unclosed-string-later-copy": unclosed_later_copy(),
    # A string closed late, in a file found by scanning.
    "string-closed-late": closed_late(),
    # A later section that doesn't read, or comes back on itself.
    "update": updated(),
    "update-prev-past-the-end": updated(prev=999999),
    "update-prev-past-the-end-hidden": updated(prev=999999, hidden=True),
    "update-first-prev-back-to-it": updated(loop=True),
    "update-xrefstm-unread": xrefstm_unread(),
    # A /Prev or /XRefStm of 0, the header's place, where no section is.
    "prev-zero": prev_zero(),
    "update-xrefstm-zero": updated(hidden=True, xrefstm=b"0"),
    # Objects not where the sections put them.
    "first-object-elsewhere": first_object_elsewhere(),
    "first-object-elsewhere-and-xrefstm-unread": first_object_elsewhere_and_xrefstm_unread(),
    "hidden-in-a-stream": hidden_in_a_stream(),
    "catalog-elsewhere": catalog_elsewhere(),
    "pages-elsewhere": pages_elsewhere(),
    "misnumbered-subsection": misnumbered(),
    # A scan's object streams.
    "member-after-an-object": member_and_object(member_first=False),
    "object-after-a-member": member_and_object(member_first=True),
}


def cases():
    return [("pages-" + name, "pages", {"pdf": data}) for name, data in CASES.items()]
