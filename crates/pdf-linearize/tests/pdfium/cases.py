"""pdf-linearize's PDFium cases: small linearized files, and broken ones, of the
kind oracle/README.md describes as `avail`: a whole file, given PDFium only as its
availability API asks for it, the answer the bytes each stage took. Every case's
bytes are made here, by a layout of its own (ISO 32000-2, Annex F), written apart
from the crate's so that neither grades the other; oracle/pdfium.py's `cases`
writes them into cases/ and cases.tsv. But for the encrypted ones, as this makes no
cipher of its own: encrypted/ has many-pages encrypted by qpdf 12.4.2 (`qpdf
--encrypt user owner 128 --use-aes=n -- --linearize --static-id`, and `256` with
`--static-aes-iv`), and those as the crate lays them out (tests/encrypted.rs writes
them)."""

import os


class Bits:
    """Bits, most significant first, each item's run ending on a byte."""

    def __init__(self):
        self.out = bytearray()
        self.acc = 0
        self.n = 0

    def put(self, value, width):
        for shift in range(width - 1, -1, -1):
            self.acc = self.acc << 1 | (value >> shift & 1)
            self.n += 1
            if self.n == 8:
                self.out.append(self.acc)
                self.acc = self.n = 0

    def align(self):
        if self.n:
            self.put(0, 8 - self.n)


def width(value):
    """How many bits say `value`, at least one, as PDFium won't take none."""
    return max(1, value.bit_length())


def stream(dictionary, data):
    return b"<< %s /Length %d >>\nstream\n" % (dictionary, len(data)) + data + b"\nendstream"


def body(number, text):
    return b"%d 0 obj\n" % number + text + b"\nendobj\n"


def linearized(pages=2, first=0, shared_later=False, sizes=None, broken=None):
    """A file of `pages` pages, each a page object and a content stream, of
    `sizes[i]` bytes or 40, all using a font that's the first page's; `first` the
    page that opens first. `shared_later` gives the later pages one object more,
    in part 8. Each page's width is its place, so an answer can name it. `broken`
    says what's wrong: a key of the hint stream's or the dictionary's, below."""
    sizes = sizes or [40] * pages
    others = [p for p in range(pages) if p != first]
    number = 1
    page_at, content_at = {}, {}
    for p in others:
        page_at[p], content_at[p] = number, number + 1
        number += 2
    shared = None
    if shared_later:
        shared, number = number, number + 1
    tree, number = number, number + 1
    dictionary = number
    catalog, page_at[first], content_at[first], font, hint = range(number + 1, number + 6)
    size = hint + 1

    def page(p):
        extra = b" /Extra %d 0 R" % shared if shared and p != first else b""
        text = (b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 %d 200] /Contents %d 0 R"
                b" /Resources << /Font << /F0 %d 0 R >> >>%s >>")
        return body(page_at[p], text % (tree, 100 + p, content_at[p], font, extra))

    def content(p):
        text = (b"BT /F0 12 Tf (%d) Tj ET " % p).ljust(sizes[p], b"%")
        return body(content_at[p], stream(b"", text))

    catalog_body = body(catalog, b"<< /Type /Catalog /Pages %d 0 R >>" % tree)
    first_bodies = [page(first), content(first),
                    body(font, b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>")]
    later = [[page(p), content(p)] for p in others]
    part8 = [body(shared, b"<< /Shared true >>")] if shared else []
    tree_body = body(tree, b"<< /Type /Pages /Count %d /Kids [%s] >>" % (
        pages, b" ".join(b"%d 0 R" % page_at[p] for p in range(pages))))

    head = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n"
    dictionary_at = len(head)
    room = 200
    first_section_at = dictionary_at + room
    # The first section's length doesn't move: /Prev is padded.
    first_rows = size - dictionary
    trailer_len = len(b"trailer\n<< /Size %d /Prev %-12d /Root %d 0 R >>\nstartxref\n0\n%%%%EOF\n"
                      % (size, 0, catalog))
    section_len = len(b"xref\n%d %d\n" % (dictionary, first_rows)) + 20 * first_rows + trailer_len
    part4_at = first_section_at + section_len
    hint_at = part4_at + len(catalog_body)

    def places(hint_len, bodies):
        at = hint_at + hint_len
        out = []
        for b in bodies:
            out.append(at)
            at += len(b)
        return out, at

    # Positions in a table are as if the hint stream weren't there: the first
    # object after it is at where the hint stream is.
    flat_first = first_bodies
    flat_later = [b for group in later for b in group]
    flat = flat_first + flat_later + part8 + [tree_body]

    def hints(hint_len):
        where, _ = places(hint_len, flat)
        less = lambda at: at - hint_len
        first_len = sum(map(len, first_bodies))
        lengths, counts, contents_at, contents_len, shares = {}, {}, {}, {}, {}
        lengths[first], counts[first] = first_len, 3
        contents_at[first] = len(first_bodies[0])
        contents_len[first] = len(first_bodies[1])
        shares[first] = []
        k = len(first_bodies)
        for p, group in zip(others, later):
            lengths[p], counts[p] = sum(map(len, group)), len(group)
            contents_at[p] = len(group[0])
            contents_len[p] = len(group[1])
            shares[p] = [0] + ([1] if shared else [])
            k += len(group)
        order = range(pages)
        min_len = min(lengths.values())
        min_obj = min(counts.values())
        min_at = min(contents_at.values())
        min_clen = min(contents_len.values())
        obj_bits = width(max(counts.values()) - min_obj)
        len_bits = width(max(lengths.values()) - min_len)
        at_bits = max(contents_at.values()) - min_at
        at_bits = at_bits.bit_length()
        clen_bits = max(len_bits, width(max(contents_len.values()) - min_clen))
        share_bits = width(max(map(len, shares.values())))
        id_bits = 1

        bits = Bits()
        for value, w in [(min_obj, 32), (less(where[0]), 32), (obj_bits, 16), (min_len, 32),
                         (len_bits, 16), (min_at, 32), (at_bits, 16), (min_clen, 32),
                         (clen_bits, 16), (share_bits, 16), (id_bits, 16), (0, 16), (4, 16)]:
            bits.put(value, w)
        if broken == "width-zero":
            # Item 3's width, which PDFium won't take at 0.
            bits.out[8:10] = b"\0\0"
        for p in order:
            bits.put(counts[p] - min_obj, obj_bits)
        bits.align()
        for p in order:
            bits.put(lengths[p] - min_len, len_bits)
        bits.align()
        for p in order:
            bits.put(len(shares[p]), share_bits)
        bits.align()
        for p in order:
            for entry in shares[p]:
                bits.put(entry, id_bits)
        bits.align()
        bits.align()  # numerators take no bits
        for p in order:
            bits.put(contents_at[p] - min_at, at_bits)
        bits.align()
        for p in order:
            bits.put(contents_len[p] - min_clen, clen_bits)
        bits.align()
        page_table = bytes(bits.out)

        # The shared table: the first page's objects, then part 8's.
        if shared:
            part8_at = less(where[flat.index(part8[0])])
            group_lengths = [first_len, len(part8[0])]
        else:
            part8_at = less(where[len(first_bodies) + len(flat_later)])
            group_lengths = [first_len]
        low = min(group_lengths)
        group_bits = width(max(group_lengths) - low)
        bits = Bits()
        # The first entry is the first page's three objects, one run; part 8's one each.
        less_ones = [len(first_bodies) - 1] + [0] * (len(group_lengths) - 1)
        count_bits = width(max(less_ones))
        for value, w in [(shared or tree, 32), (part8_at, 32), (1, 32), (len(group_lengths), 32),
                         (count_bits, 16), (low, 32), (group_bits, 16)]:
            bits.put(value, w)
        for length in group_lengths:
            bits.put(length - low, group_bits)
        bits.align()
        for _ in group_lengths:
            bits.put(0, 1)
        bits.align()
        for less_one in less_ones:
            bits.put(less_one, count_bits)
        bits.align()
        shared_table = bytes(bits.out)
        data = page_table + shared_table
        s = len(page_table)
        if broken == "hint-short":
            data = data[:20]
        if broken == "hint-shared-past":
            s = len(data) + 40
        return stream(b"/S %d" % s, data)

    # The hint stream's own length is in nothing its data says, so a second pass
    # finds nothing new, but its length is what the rest moves by.
    hint_body = body(hint, hints(0))
    hint_body = body(hint, hints(len(hint_body)))
    where, end = places(len(hint_body), flat)
    main_at = end

    numbers = {catalog: part4_at, hint: hint_at, dictionary: dictionary_at}
    for n, b in zip([page_at[first], content_at[first], font], flat):
        numbers[n] = where[flat.index(b)]
    k = len(first_bodies)
    for p in others:
        numbers[page_at[p]] = where[k]
        numbers[content_at[p]] = where[k + 1]
        k += 2
    if shared:
        numbers[shared] = where[k]
        k += 1
    numbers[tree] = where[k]

    main = b"xref\n0 %d\n0000000000 65535 f \n" % dictionary
    t = main_at + len(b"xref\n0 %d" % dictionary)
    for n in range(1, dictionary):
        main += b"%010d 00000 n \n" % numbers[n]
    main += b"trailer\n<< /Size %d >>\nstartxref\n%d\n%%%%EOF\n" % (dictionary, first_section_at)
    length = main_at + len(main)

    section = b"xref\n%d %d\n" % (dictionary, first_rows)
    for n in range(dictionary, size):
        section += b"%010d 00000 n \n" % numbers[n]
    section += b"trailer\n<< /Size %d /Prev %-12d /Root %d 0 R >>\nstartxref\n0\n%%%%EOF\n" % (
        size, main_at, catalog)
    assert len(section) == section_len

    first_end = where[len(first_bodies)]
    declared = length + 7 if broken == "length-wrong" else length
    h_at = length + 500 if broken == "hint-past" else hint_at
    extra = b" /P %d" % first if first else b""
    if broken == "first-past":
        extra = b" /P %d" % (pages + 8)
    dictionary_body = (b"%d 0 obj\n<< /Linearized 1 /L %d /H [%d %d] /O %d /E %d /N %d /T %d%s >>\nendobj\n"
                       % (dictionary, declared, h_at, len(hint_body), page_at[first], first_end,
                          pages, t, extra))
    assert len(dictionary_body) <= room
    out = (head + dictionary_body.ljust(room, b" ") + section + catalog_body + hint_body
           + b"".join(flat) + main)
    assert len(out) == length
    return out


def plain():
    """A file that isn't linearized."""
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>",
               b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
               b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>"]
    out = bytearray(b"%PDF-1.7\n")
    rows = []
    for number, text in enumerate(objects, 1):
        rows.append(len(out))
        out += body(number, text)
    at = len(out)
    out += b"xref\n0 4\n0000000000 65535 f \n"
    for row in rows:
        out += b"%010d 00000 n \n" % row
    out += b"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % at
    return bytes(out)


def encrypted(name):
    """The file of that name in encrypted/."""
    with open(os.path.join(os.path.dirname(__file__), "encrypted", name + ".pdf"), "rb") as f:
        return f.read()


def cases():
    pad = [40, 2000, 3000, 4000, 900, 8000, 40, 2500]
    return [
        ("one-page", "avail", {"pdf": linearized(pages=1)}),
        ("two-pages", "avail", {"pdf": linearized()}),
        ("two-pages-shared", "avail", {"pdf": linearized(shared_later=True)}),
        ("many-pages", "avail", {"pdf": linearized(pages=8, sizes=pad)}),
        ("many-pages-shared", "avail", {"pdf": linearized(pages=8, sizes=pad, shared_later=True)}),
        ("first-is-page-1", "avail", {"pdf": linearized(first=1)}),
        ("first-is-page-5", "avail", {"pdf": linearized(pages=8, first=5, sizes=pad)}),
        ("broken-length", "avail", {"pdf": linearized(broken="length-wrong")}),
        ("broken-width-zero", "avail", {"pdf": linearized(pages=8, sizes=pad, broken="width-zero")}),
        ("broken-hint-short", "avail", {"pdf": linearized(pages=8, sizes=pad, broken="hint-short")}),
        ("broken-hint-past", "avail", {"pdf": linearized(pages=8, sizes=pad, broken="hint-past")}),
        ("broken-shared-past", "avail", {"pdf": linearized(pages=8, sizes=pad, broken="hint-shared-past")}),
        ("broken-first-past", "avail", {"pdf": linearized(pages=1, broken="first-past")}),
        ("not-linearized", "avail", {"pdf": plain()}),
        ("encrypted-qpdf-rc4", "avail", {"pdf": encrypted("qpdf-rc4"), "password": b"user"}),
        ("encrypted-qpdf-aes-256", "avail", {"pdf": encrypted("qpdf-aes-256"), "password": b"user"}),
        ("encrypted-wrong-password", "avail", {"pdf": encrypted("qpdf-rc4"), "password": b"wrong"}),
        ("encrypted-no-password", "avail", {"pdf": encrypted("qpdf-rc4")}),
        ("encrypted-written-rc4", "avail",
         {"pdf": encrypted("written-rc4-table"), "password": b"user"}),
        ("encrypted-written-aes-256", "avail",
         {"pdf": encrypted("written-aes-256-packed"), "password": b"user"}),
    ]
