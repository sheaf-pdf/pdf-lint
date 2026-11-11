"""pdf-semantics' PDFium cases: page trees as PDFium walks them, by the kind
oracle/README.md describes: a whole file, asked for its count and each page's
size and turns. A page's width is its object's number, so an answer names the
pages it found. Every case's bytes are written here, from literals;
oracle/pdfium.py's `cases` writes them into cases/ and cases.tsv."""


def build(objects):
    """A file of `objects`, {number: its body}, with a cross-reference table,
    its catalog object 1."""
    out = bytearray(b"%PDF-1.7\n")
    offsets = {}
    for number in sorted(objects):
        offsets[number] = len(out)
        out += b"%d 0 obj\n" % number + objects[number] + b"\nendobj\n"
    xref = len(out)
    top = max(objects) + 1
    out += b"xref\n0 %d\n0000000000 65535 f \n" % top
    for number in range(1, top):
        if number in offsets:
            out += b"%010d 00000 n \n" % offsets[number]
        else:
            out += b"0000000000 00000 f \n"
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (top, xref)
    return bytes(out)


CATALOG = b"<< /Type /Catalog /Pages 2 0 R >>"


def page(number, parent=2, more=b""):
    """A page whose width is its number."""
    return b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 %d 200]%s >>" % (parent, number, more)


def bare(parent=2, more=b""):
    """A page with no box of its own."""
    return b"<< /Type /Page /Parent %d 0 R%s >>" % (parent, more)


def tree(root, **more):
    """A file whose root is `root`, with pages 3, 4 and 5."""
    objects = {1: CATALOG, 2: root, 3: page(3), 4: page(4), 5: page(5)}
    objects.update({int(k[1:]): v for k, v in more.items()})
    return objects


def one(kid, root_keys=b"", **more):
    """A file whose root lists object 3 alone, `kid`."""
    objects = {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 " + root_keys + b">>", 3: kid}
    objects.update({int(k[1:]): v for k, v in more.items()})
    return objects


def deep(levels):
    """A chain of `levels` nodes from the root down, each listing the next and
    naming no /Parent, and a page under the last."""
    objects = {1: CATALOG}
    number = 2
    for _ in range(levels):
        objects[number] = b"<< /Type /Pages /Kids [%d 0 R] /Count 1 >>" % (number + 1)
        number += 1
    objects[number] = b"<< /Type /Page /MediaBox [0 0 3 200] >>"
    return objects


def deep_past_the_count(levels):
    """A page, then a chain of `levels` nodes with no page under them, under a
    root with no /Count."""
    objects = {1: CATALOG, 2: b"<< /Kids [3 0 R 4 0 R] >>", 3: page(3)}
    number = 4
    for _ in range(levels):
        objects[number] = b"<< /Kids [%d 0 R] >>" % (number + 1)
        number += 1
    objects[number] = b"<< /Kids [] >>"
    return objects


def linearized():
    """A file whose first object is a linearization header naming two pages,
    where its tree lists three: a document opened whole doesn't read it."""
    objects = [
        (9, b"<< /Linearized 1 /L @@@@@@@@@@ /H [ 0 0 ] /O 3 /E 0 /N 2 /T ########## >>"),
        (1, CATALOG),
        (2, b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>"),
        (3, page(3)),
        (4, page(4)),
        (5, page(5)),
    ]
    out = bytearray(b"%PDF-1.7\n")
    offsets = {}
    for number, body in objects:
        offsets[number] = len(out)
        out += b"%d 0 obj\n" % number + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 10\n0000000000 65535 f \n"
    for number in range(1, 10):
        if number in offsets:
            out += b"%010d 00000 n \n" % offsets[number]
        else:
            out += b"0000000000 00000 f \n"
    out += b"trailer\n<< /Size 10 /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % xref
    out = bytes(out).replace(b"##########", b"%010d" % xref)
    return out.replace(b"@@@@@@@@@@", b"%010d" % len(out))


def written(count, fanout):
    """A file of `count` pages, objects 2 on, under a tree laid out as the
    crate's writer lays one out under nodes of `fanout` kids: every page at
    one depth, under as few levels of nodes as hold them, each node's kids
    as even as they can be, the first ones a page more. Its nodes are
    numbered from `count + 2` in the order the writer asks for their
    numbers, the root first and each node before those under it, and each
    is /Type /Pages, /Parent but the root, /Kids and /Count, in that order."""
    height, reach = 1, fanout
    while reach < count:
        height += 1
        reach *= fanout
    nodes = []  # [parent, kids, count], a kid ("node", place) or ("page", position)
    leaf = [None] * count

    def build(first, length, height, parent):
        me = len(nodes)
        nodes.append([parent, [], length])
        if height == 1:
            kids = [("page", p) for p in range(first, first + length)]
            for p in range(first, first + length):
                leaf[p] = me
        else:
            under = fanout ** (height - 1)
            groups = max(-(-length // under), 1)
            size, more = divmod(length, groups)
            kids, at = [], 0
            for g in range(groups):
                part = size + (1 if g < more else 0)
                kids.append(("node", build(first + at, part, height - 1, me)))
                at += part
        nodes[me][1] = kids
        return me

    build(0, count, height, None)

    def number(kid):
        kind, at = kid
        return count + 2 + at if kind == "node" else 2 + at

    objects = {1: b"<< /Type /Catalog /Pages %d 0 R >>" % number(("node", 0))}
    for p in range(count):
        objects[2 + p] = page(2 + p, number(("node", leaf[p])))
    for at, (parent, kids, length) in enumerate(nodes):
        up = b"" if parent is None else b" /Parent %d 0 R" % number(("node", parent))
        listed = b" ".join(b"%d 0 R" % number(kid) for kid in kids)
        objects[number(("node", at))] = b"<< /Type /Pages%s /Kids [%s] /Count %d >>" % (up, listed, length)
    return objects


ALL = b"/Type /Pages /Kids [3 0 R 4 0 R 5 0 R]"
FOO = b"<< /Type /Foo /Parent 2 0 R /MediaBox [0 0 3 200] >>"
BOX = b"/MediaBox [0 0 21 22]"
ROOT_BOX = b"/MediaBox [0 0 2 200]"
SIX = b"<< /Type /Pages /Parent 2 0 R /Kids [3 0 R 4 0 R] "

CASES = {
    "plain": tree(b"<< " + ALL + b" /Count 3 >>"),
    # The root's /Count: what PDFium counts by, and what it doesn't.
    "count-2": tree(b"<< " + ALL + b" /Count 2 >>"),
    "count-real": tree(b"<< " + ALL + b" /Count 2.9 >>"),
    "count-past-the-tree": tree(b"<< " + ALL + b" /Count 5 >>"),
    "count-0": tree(b"<< " + ALL + b" /Count 0 >>"),
    "count-negative": tree(b"<< " + ALL + b" /Count -1 >>"),
    "count-string": tree(b"<< " + ALL + b" /Count (2) >>"),
    "count-at-the-page-cap": tree(b"<< " + ALL + b" /Count 1048575 >>"),
    "count-under-the-page-cap": tree(b"<< " + ALL + b" /Count 1048574 >>"),
    "count-none": tree(b"<< " + ALL + b" >>"),
    "count-reference": tree(b"<< " + ALL + b" /Count 9 0 R >>", o9=b"1"),
    "count-reference-chain": tree(b"<< " + ALL + b" /Count 9 0 R >>", o9=b"10 0 R", o10=b"1"),
    "count-reference-to-nothing": tree(b"<< " + ALL + b" /Count 88 0 R >>"),
    "count-null": tree(b"<< " + ALL + b" /Count null >>"),
    # A kid written in place.
    "kid-direct-page": tree(b"<< /Type /Pages /Kids [3 0 R << /Type /Page /MediaBox [0 0 777 200] >> 5 0 R] /Count 3 >>"),
    "kid-direct-page-no-count": tree(b"<< /Type /Pages /Kids [3 0 R << /Type /Page /MediaBox [0 0 777 200] >> 5 0 R] >>"),
    "kid-direct-node": tree(b"<< /Type /Pages /Kids [3 0 R << /Kids [4 0 R] /Count 1 >> 5 0 R] /Count 3 >>"),
    "kid-direct-node-no-count": tree(b"<< /Type /Pages /Kids [3 0 R << /Kids [4 0 R 5 0 R] >> 5 0 R] >>"),
    "kid-direct-other-type": tree(b"<< /Type /Pages /Kids [3 0 R << /Type /Foo /MediaBox [0 0 777 200] >> 5 0 R] /Count 3 >>"),
    "kid-direct-parent-box": {1: CATALOG, 2: b"<< /Type /Pages /Kids [<< /Type /Page /Parent 2 0 R >>] /Count 1 " + BOX + b" >>"},
    "kid-direct-node-inherits-nothing": {1: CATALOG, 2: b"<< /Type /Pages /Kids [<< /Kids [3 0 R] /Count 1 /MediaBox [0 0 41 42] >>] /Count 1 >>", 3: b"<< /Type /Page >>"},
    "kid-direct-no-type": tree(b"<< /Type /Pages /Kids [3 0 R << /MediaBox [0 0 777 200] >> 5 0 R] /Count 3 >>"),
    # A kid that names no dictionary: counted out, but its place taken.
    "kid-names-an-integer": tree(b"<< /Type /Pages /Kids [3 0 R 9 0 R 5 0 R] /Count 3 >>", o9=b"1"),
    "kid-names-an-integer-no-count": tree(b"<< /Type /Pages /Kids [3 0 R 9 0 R 5 0 R] >>", o9=b"1"),
    "kid-names-nothing": tree(b"<< /Type /Pages /Kids [3 0 R 88 0 R 5 0 R] /Count 3 >>"),
    "kid-names-nothing-no-count": tree(b"<< /Type /Pages /Kids [3 0 R 88 0 R 5 0 R] >>"),
    "kid-names-null": tree(b"<< /Type /Pages /Kids [3 0 R 9 0 R 5 0 R] /Count 3 >>", o9=b"null"),
    "kid-null": tree(b"<< /Type /Pages /Kids [3 0 R null 5 0 R] /Count 3 >>"),
    "kid-null-no-count": tree(b"<< /Type /Pages /Kids [3 0 R null 5 0 R] >>"),
    "kid-integer": tree(b"<< /Type /Pages /Kids [3 0 R 7 5 0 R] /Count 3 >>"),
    "kid-name": tree(b"<< /Type /Pages /Kids [3 0 R /Page 5 0 R] /Count 3 >>"),
    "kid-by-a-chain": {1: CATALOG, 2: b"<< /Type /Pages /Kids [9 0 R] /Count 1 >>", 9: b"3 0 R", 3: page(3)},
    "kid-by-a-chain-in-the-middle": tree(b"<< /Type /Pages /Kids [3 0 R 9 0 R 5 0 R] /Count 3 >>", o9=b"4 0 R"),
    "kid-by-a-chain-no-count": tree(b"<< /Type /Pages /Kids [3 0 R 9 0 R 5 0 R] >>", o9=b"4 0 R"),
    # A reference by a wrong generation.
    "kid-wrong-generation": tree(b"<< /Type /Pages /Kids [3 5 R 4 0 R 5 0 R] /Count 3 >>"),
    "kid-wrong-generation-no-count": tree(b"<< /Type /Pages /Kids [3 5 R 4 0 R 5 0 R] >>"),
    # A leaf's /Type.
    "leaf-type-foo": one(b"<< /Type /Foo /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
    "leaf-type-page": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
    "leaf-type-pages": one(b"<< /Type /Pages /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
    "leaf-type-string": one(b"<< /Type (Page) /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
    "leaf-type-template": one(b"<< /Type /Template /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
    "leaf-type-reference-to-pages": one(b"<< /Type 9 0 R /Parent 2 0 R /MediaBox [0 0 3 200] >>", o9=b"/Pages"),
    "leaf-type-reference-to-page": one(b"<< /Type 9 0 R /Parent 2 0 R /MediaBox [0 0 3 200] >>", o9=b"/Page"),
    "leaf-type-null": one(b"<< /Type null /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
    "leaf-no-type": one(b"<< /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
    "leaf-no-type-under-no-count": {1: CATALOG, 2: b"<< /Kids [3 0 R] >>", 3: b"<< /MediaBox [0 0 3 200] >>"},
    "leaf-other-type-in-the-middle": tree(b"<< " + ALL + b" /Count 3 >>", o4=b"<< /Type /Foo /Parent 2 0 R /MediaBox [0 0 4 200] >>"),
    "leaf-other-type-in-the-middle-no-count": tree(b"<< " + ALL + b" >>", o4=b"<< /Type /Foo /Parent 2 0 R /MediaBox [0 0 4 200] >>"),
    "leaf-other-type-listed-twice": tree(b"<< /Type /Pages /Kids [4 0 R 3 0 R 4 0 R] /Count 3 >>", o4=b"<< /Type /Foo /Parent 2 0 R /MediaBox [0 0 4 200] >>"),
    "leaf-stream": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200] /Length 1 >>\nstream\nx\nendstream"),
    "leaf-type-by-a-chain": one(b"<< /Type 9 0 R /Parent 2 0 R /MediaBox [0 0 3 200] >>", o9=b"10 0 R", o10=b"/Page"),
    "leaf-type-names-nothing": one(b"<< /Type 88 0 R /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
    # With no root /Count, what the count's walk finds a page is one, whatever
    # its /Type but /Pages, and only that.
    "no-count-leaf-other-type": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: FOO, 5: page(5)},
    "no-count-leaf-string-type": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type (Pages) /MediaBox [0 0 3 200] >>", 5: page(5)},
    "no-count-leaf-null-type": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type null /MediaBox [0 0 3 200] >>", 5: page(5)},
    "no-count-leaf-type-by-reference": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type 9 0 R /MediaBox [0 0 3 200] >>", 5: page(5), 9: b"/Pages"},
    "no-count-leaf-type-pages-alone": {1: CATALOG, 2: b"<< /Kids [3 0 R] >>", 3: b"<< /Type /Pages /MediaBox [0 0 3 200] >>"},
    "no-count-leaf-type-pages-first": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type /Pages /MediaBox [0 0 3 200] >>", 5: page(5)},
    "no-count-root-count-0-leaf-other-type": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] /Count 0 >>", 3: FOO, 5: page(5)},
    "no-count-direct-leaf-other-type": {1: CATALOG, 2: b"<< /Kids [<< /Type /Foo /MediaBox [0 0 3 200] >> 4 0 R] >>", 4: page(4)},
    "no-count-node-counted-leaf-other-type": {1: CATALOG, 2: b"<< /Kids [6 0 R] >>", 6: b"<< /Kids [3 0 R] /Count 1 >>", 3: FOO},
    "no-count-node-uncounted-leaf-other-type": {1: CATALOG, 2: b"<< /Kids [6 0 R] >>", 6: b"<< /Kids [3 0 R] >>", 3: FOO},
    "count-node-uncounted-leaf-other-type": {1: CATALOG, 2: b"<< /Kids [6 0 R] /Count 1 >>", 6: b"<< /Kids [3 0 R] >>", 3: FOO},
    "no-count-leaf-under-a-counted-and-an-uncounted-node": {1: CATALOG, 2: b"<< /Kids [6 0 R 7 0 R] >>", 6: b"<< /Kids [3 0 R] /Count 1 >>", 7: b"<< /Kids [3 0 R] >>", 3: FOO},
    "no-count-leaf-other-type-listed-twice": {1: CATALOG, 2: b"<< /Kids [3 0 R 3 0 R] >>", 3: FOO},
    "no-count-type-page-kids-empty": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type /Page /Kids [] /MediaBox [0 0 3 200] >>", 5: page(5)},
    "no-count-type-page-kids-twice": {1: CATALOG, 2: b"<< /Kids [3 0 R] >>", 3: b"<< /Type /Page /Kids [4 0 R 4 0 R] >>", 4: page(4)},
    "no-count-type-page-kids-counted": {1: CATALOG, 2: b"<< /Kids [3 0 R] >>", 3: b"<< /Type /Page /Kids [4 0 R 4 0 R] /Count 9 >>", 4: page(4)},
    "no-count-other-type-kids-empty": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type /Foo /Kids [] /MediaBox [0 0 3 200] >>", 5: page(5)},
    "no-count-no-type-kids-empty": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: b"<< /Kids [] /MediaBox [0 0 3 200] >>", 5: page(5)},
    "no-count-type-page-by-reference-kids-empty": {1: CATALOG, 2: b"<< /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type 9 0 R /Kids [] /MediaBox [0 0 3 200] >>", 5: page(5), 9: b"/Page"},
    "no-count-stream-with-kids": {1: CATALOG, 2: b"<< /Kids [3 0 R] >>", 3: b"<< /Kids [4 0 R] /Length 1 >>\nstream\nx\nendstream", 4: page(4)},
    # A kid with /Kids is a node, whatever its /Type.
    "node-type-page": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", 3: b"<< /Type /Page /Parent 2 0 R /Kids [4 0 R] /Count 1 >>", 4: page(4, 3)},
    "node-kids-null": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>", 3: b"<< /Type /Page /Parent 2 0 R /Kids null /MediaBox [0 0 3 200] >>", 5: page(5)},
    "node-kids-empty": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>", 3: b"<< /Type /Pages /Parent 2 0 R /Kids [] >>", 5: page(5)},
    "node-kids-empty-no-count": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type /Pages /Parent 2 0 R /Kids [] >>", 5: page(5)},
    "node-kids-not-an-array": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>", 3: b"<< /Type /Pages /Parent 2 0 R /Kids 7 >>", 5: page(5)},
    "node-kids-not-an-array-last": {1: CATALOG, 2: b"<< /Type /Pages /Kids [5 0 R 3 0 R] /Count 2 >>", 3: b"<< /Type /Page /Parent 2 0 R /Kids 7 /MediaBox [0 0 3 200] >>", 5: page(5)},
    "node-kids-reference-to-nothing": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>", 3: b"<< /Type /Page /Parent 2 0 R /Kids 88 0 R /MediaBox [0 0 3 200] >>", 5: page(5)},
    "node-kids-by-reference": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] >>", 6: b"<< /Type /Pages /Parent 2 0 R /Kids 9 0 R >>", 9: b"[3 0 R 4 0 R]", 3: page(3, 6), 4: page(4, 6), 5: page(5)},
    "node-listed-twice": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 6 0 R] >>", 6: b"<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 >>", 3: page(3, 6)},
    "page-listed-twice": tree(b"<< /Type /Pages /Kids [3 0 R 4 0 R 3 0 R] /Count 3 >>"),
    "node-stream": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R] /Count 1 >>", 6: b"<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 /Length 1 >>\nstream\nx\nendstream", 3: page(3, 6)},
    # A node's /Count, under a root that has one.
    "node-count-low": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] /Count 3 >>", 6: SIX + b"/Count 1 >>", 3: page(3, 6), 4: page(4, 6), 5: page(5)},
    "node-count-high": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] /Count 3 >>", 6: b"<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 2 >>", 3: page(3, 6), 5: page(5)},
    "node-count-none": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] /Count 3 >>", 6: SIX + b">>", 3: page(3, 6), 4: page(4, 6), 5: page(5)},
    # And under a root that hasn't: the node's trusted.
    "no-count-node-count-high": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] >>", 6: b"<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 5 >>", 3: page(3, 6), 5: page(5)},
    "no-count-node-count-0": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] >>", 6: b"<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 0 >>", 3: page(3, 6), 5: page(5)},
    "no-count-node-count-none": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] >>", 6: SIX + b">>", 3: page(3, 6), 4: page(4, 6), 5: page(5)},
    "no-count-node-count-low": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] >>", 6: SIX + b"/Count 1 >>", 3: page(3, 6), 4: page(4, 6), 5: page(5)},
    "no-count-node-count-real": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] >>", 6: SIX + b"/Count 1.9 >>", 3: page(3, 6), 4: page(4, 6), 5: page(5)},
    "no-count-node-count-reference": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 5 0 R] >>", 6: SIX + b"/Count 9 0 R >>", 9: b"1", 3: page(3, 6), 4: page(4, 6), 5: page(5)},
    "no-count-node-listed-twice": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 6 0 R] >>", 6: SIX + b">>", 3: page(3, 6), 4: page(4, 6)},
    "no-count-at-the-page-cap": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R] >>", 6: SIX + b"/Count 1048574 >>", 3: page(3, 6), 4: page(4, 6)},
    "no-count-past-the-page-cap": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 7 0 R] >>", 6: SIX + b"/Count 1048574 >>", 7: SIX + b"/Count 1048574 >>", 3: page(3, 6), 4: page(4, 6)},
    # The root.
    "root-page": {1: CATALOG, 2: b"<< /Type /Page /MediaBox [0 0 2 200] >>"},
    "root-page-with-count": {1: CATALOG, 2: b"<< /Type /Page /MediaBox [0 0 2 200] /Count 5 >>"},
    "root-no-type-no-kids": {1: CATALOG, 2: b"<< /MediaBox [0 0 2 200] >>"},
    "root-other-type-no-kids": {1: CATALOG, 2: b"<< /Type /Foo /MediaBox [0 0 2 200] >>"},
    "root-pages-no-kids": {1: CATALOG, 2: b"<< /Type /Pages /MediaBox [0 0 2 200] >>"},
    "root-type-by-reference-to-pages": {1: CATALOG, 2: b"<< /Type 9 0 R /MediaBox [0 0 2 200] >>", 9: b"/Pages"},
    "root-type-string": {1: CATALOG, 2: b"<< /Type (Pages) /MediaBox [0 0 2 200] >>"},
    "root-type-null": {1: CATALOG, 2: b"<< /Type null /MediaBox [0 0 2 200] >>"},
    "root-is-the-catalog": {1: b"<< /Type /Catalog /Pages 1 0 R /MediaBox [0 0 2 200] >>"},
    "root-page-with-kids": {1: CATALOG, 2: b"<< /Type /Page /Kids [3 0 R] /Count 1 >>", 3: page(3)},
    "root-no-type-with-kids": {1: CATALOG, 2: b"<< /Kids [3 0 R] /Count 1 >>", 3: page(3)},
    "root-kids-empty": {1: CATALOG, 2: b"<< /Type /Pages /Kids [] /Count 0 >>"},
    "root-kids-empty-count-2": {1: CATALOG, 2: b"<< /Type /Pages /Kids [] /Count 2 >>"},
    "root-kids-not-an-array": {1: CATALOG, 2: b"<< /Type /Pages /Kids 3 /Count 1 >>"},
    "root-kids-not-an-array-no-count": {1: CATALOG, 2: b"<< /Type /Pages /Kids 3 >>"},
    "root-kids-null": {1: CATALOG, 2: b"<< /Type /Pages /Kids null >>"},
    "root-kids-by-reference": tree(b"<< /Type /Pages /Kids 9 0 R /Count 3 >>", o9=b"[3 0 R 4 0 R 5 0 R]"),
    "root-kids-by-a-chain": tree(b"<< /Type /Pages /Kids 9 0 R /Count 3 >>", o9=b"10 0 R", o10=b"[3 0 R 4 0 R 5 0 R]"),
    "root-kids-by-a-chain-no-count": tree(b"<< /Type /Pages /Kids 9 0 R >>", o9=b"10 0 R", o10=b"[3 0 R 4 0 R 5 0 R]"),
    # A root of /Type /Page, the name as written, whose /Kids doesn't come to an
    # array: page 0, under the count it's given, however /Kids is written.
    "root-page-kids-a-string": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 1 " + ROOT_BOX + b" >>"},
    "root-page-kids-a-string-count-3": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 3 " + ROOT_BOX + b" >>"},
    "root-page-kids-null": {1: CATALOG, 2: b"<< /Type /Page /Kids null /Count 1 " + ROOT_BOX + b" >>"},
    "root-page-kids-null-count-3": {1: CATALOG, 2: b"<< /Type /Page /Kids null /Count 3 " + ROOT_BOX + b" >>"},
    "root-page-kids-names-nothing": {1: CATALOG, 2: b"<< /Type /Page /Kids 88 0 R /Count 1 " + ROOT_BOX + b" >>"},
    "root-page-kids-names-nothing-count-3": {1: CATALOG, 2: b"<< /Type /Page /Kids 88 0 R /Count 3 " + ROOT_BOX + b" >>"},
    "root-page-kids-names-an-integer": {1: CATALOG, 2: b"<< /Type /Page /Kids 9 0 R /Count 1 " + ROOT_BOX + b" >>", 9: b"7"},
    "root-page-kids-names-an-integer-count-3": {1: CATALOG, 2: b"<< /Type /Page /Kids 9 0 R /Count 3 " + ROOT_BOX + b" >>", 9: b"7"},
    "root-page-kids-by-a-chain": tree(b"<< /Type /Page /Kids 9 0 R /Count 1 " + ROOT_BOX + b" >>", o9=b"10 0 R", o10=b"[3 0 R]"),
    "root-page-kids-by-a-chain-count-3": tree(b"<< /Type /Page /Kids 9 0 R /Count 3 " + ROOT_BOX + b" >>", o9=b"10 0 R", o10=b"[3 0 R]"),
    "root-page-kids-names-itself": {1: CATALOG, 2: b"<< /Type /Page /Kids 2 0 R /Count 1 " + ROOT_BOX + b" >>"},
    # As a count is one PDFium goes by.
    "root-page-kids-a-string-count-real": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 2.5 " + ROOT_BOX + b" >>"},
    "root-page-kids-a-string-count-by-reference": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 9 0 R " + ROOT_BOX + b" >>", 9: b"2"},
    "root-page-kids-a-string-count-under-the-page-cap": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 1048574 " + ROOT_BOX + b" >>"},
    "root-page-kids-a-string-no-count": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) " + ROOT_BOX + b" >>"},
    "root-page-kids-a-string-count-0": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 0 " + ROOT_BOX + b" >>"},
    "root-page-kids-a-string-count-negative": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count -1 " + ROOT_BOX + b" >>"},
    "root-page-kids-a-string-count-at-the-page-cap": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 1048575 " + ROOT_BOX + b" >>"},
    "root-page-kids-a-string-count-by-a-chain": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 9 0 R " + ROOT_BOX + b" >>", 9: b"10 0 R", 10: b"2"},
    # However the root is reached, and it's a page as any other.
    "root-page-kids-a-string-in-place": {1: b"<< /Type /Catalog /Pages << /Type /Page /Kids (x) /Count 3 " + ROOT_BOX + b" >> >>"},
    "root-page-kids-a-string-by-a-chain-from-the-catalog": {1: b"<< /Type /Catalog /Pages 9 0 R >>", 9: b"2 0 R", 2: b"<< /Type /Page /Kids (x) /Count 3 " + ROOT_BOX + b" >>"},
    "root-page-kids-a-string-stream": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 3 " + ROOT_BOX + b" /Length 1 >>\nstream\nx\nendstream"},
    "root-page-kids-a-string-inherits": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 3 /Parent 9 0 R >>", 9: b"<< /MediaBox [0 0 41 42] /Rotate 90 >>"},
    "root-page-kids-a-string-crop-and-rotate": {1: CATALOG, 2: b"<< /Type /Page /Kids (x) /Count 3 " + ROOT_BOX + b" /CropBox [0 0 1 100] /Rotate 180 >>"},
    # Any other /Type there, even one that reads as /Page, loads none: no pages.
    "root-no-type-kids-a-string": {1: CATALOG, 2: b"<< /Kids (x) /Count 1 " + ROOT_BOX + b" >>"},
    "root-other-type-kids-a-string": {1: CATALOG, 2: b"<< /Type /Foo /Kids (x) /Count 1 " + ROOT_BOX + b" >>"},
    "root-type-string-kids-a-string": {1: CATALOG, 2: b"<< /Type (Page) /Kids (x) /Count 1 " + ROOT_BOX + b" >>"},
    "root-type-by-reference-to-page-kids-a-string": {1: CATALOG, 2: b"<< /Type 9 0 R /Kids (x) /Count 1 " + ROOT_BOX + b" >>", 9: b"/Page"},
    # And /Kids that is an array is walked, whatever's in it.
    "root-page-kids-empty-count-2": {1: CATALOG, 2: b"<< /Type /Page /Kids [] /Count 2 " + ROOT_BOX + b" >>"},
    "root-page-kids-by-reference": tree(b"<< /Type /Page /Kids 9 0 R /Count 2 " + ROOT_BOX + b" >>", o9=b"[3 0 R 4 0 R]"),
    "root-page-kids-by-reference-to-empty": {1: CATALOG, 2: b"<< /Type /Page /Kids 9 0 R /Count 2 " + ROOT_BOX + b" >>", 9: b"[]"},
    "root-not-a-dictionary": {1: CATALOG, 2: b"7"},
    "root-stream": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Length 1 >>\nstream\nx\nendstream", 3: page(3)},
    # The catalog's /Pages.
    "catalog-no-type": {1: b"<< /Pages 2 0 R >>", 2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", 3: page(3)},
    "catalog-no-pages": {1: b"<< /Type /Catalog >>"},
    "catalog-pages-direct": {1: b"<< /Type /Catalog /Pages << /Type /Pages /Kids [3 0 R] /Count 1 >> >>", 3: b"<< /Type /Page /MediaBox [0 0 3 200] >>"},
    "catalog-pages-direct-no-kids": {1: b"<< /Type /Catalog /Pages << /Type /Page /MediaBox [0 0 2 200] >> >>"},
    "catalog-pages-by-a-chain": {1: b"<< /Type /Catalog /Pages 9 0 R >>", 9: b"2 0 R", 2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", 3: page(3)},
    "catalog-pages-by-a-longer-chain": {1: b"<< /Type /Catalog /Pages 9 0 R >>", 9: b"10 0 R", 10: b"2 0 R", 2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", 3: page(3)},
    "catalog-pages-names-nothing": {1: b"<< /Type /Catalog /Pages 88 0 R >>"},
    "catalog-pages-null": {1: b"<< /Type /Catalog /Pages null >>"},
    # A node listed inside itself: the walk fails here, by choice.
    "self-kid": {1: CATALOG, 2: b"<< /Type /Pages /Kids [2 0 R 3 0 R] /Count 2 >>", 3: page(3)},
    "self-kid-no-count": {1: CATALOG, 2: b"<< /Type /Pages /Kids [2 0 R 3 0 R] >>", 3: page(3)},
    "cycle": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>", 3: b"<< /Type /Pages /Parent 2 0 R /Kids [2 0 R] /Count 1 >>", 5: page(5)},
    "cycle-no-count": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R 5 0 R] >>", 3: b"<< /Type /Pages /Parent 2 0 R /Kids [2 0 R] >>", 5: page(5)},
    "cycle-past-the-count": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 1 >>", 3: page(3), 4: b"<< /Type /Pages /Parent 2 0 R /Kids [2 0 R] >>"},
    "self-kid-further-down": {1: CATALOG, 2: b"<< /Type /Pages /Kids [6 0 R 4 0 R] /Count 2 >>", 6: b"<< /Kids [6 0 R 3 0 R] >>", 3: page(3), 4: page(4)},
    "self-kid-further-down-no-count": {1: CATALOG, 2: b"<< /Kids [6 0 R 4 0 R] >>", 6: b"<< /Kids [6 0 R 3 0 R] >>", 3: page(3), 4: page(4)},
    "self-kid-past-the-count": {1: CATALOG, 2: b"<< /Kids [3 0 R 6 0 R] >>", 3: page(3), 6: b"<< /Kids [6 0 R] >>"},
    "cycle-of-two-no-count": {1: CATALOG, 2: b"<< /Kids [6 0 R 4 0 R] >>", 6: b"<< /Kids [7 0 R 3 0 R] >>", 7: b"<< /Kids [6 0 R] >>", 3: page(3), 4: page(4)},
    # Inherited up /Parent, not by the path walked.
    "inherit-box-and-rotate": one(bare(), BOX + b" /Rotate 90 "),
    "inherit-by-parent-not-path": {1: CATALOG, 2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 " + BOX + b" >>", 3: bare(4), 4: b"<< /Type /Pages /MediaBox [0 0 41 42] >>"},
    "inherit-no-parent": one(b"<< /Type /Page >>", BOX + b" "),
    "inherit-crop": one(page(3), b"/CropBox [0 0 11 12] "),
    "inherit-rotate-by-reference": one(page(3), b"/Rotate 9 0 R ", o9=b"270"),
    "inherit-rotate-by-a-chain": one(page(3), b"/Rotate 9 0 R ", o9=b"10 0 R", o10=b"270"),
    "rotate-by-a-longer-chain": one(page(3, 2, b" /Rotate 9 0 R"), o9=b"10 0 R", o10=b"11 0 R", o11=b"90"),
    "parent-direct": one(b"<< /Type /Page /Parent << /MediaBox [0 0 31 32] >> >>", BOX + b" "),
    "parent-direct-up": one(b"<< /Type /Page /Parent << /Parent 2 0 R >> >>", BOX + b" "),
    "parent-direct-up-direct": one(b"<< /Type /Page /Parent << /Parent << /Parent 2 0 R >> >> >>", BOX + b" "),
    "parent-by-a-chain": one(b"<< /Type /Page /Parent 9 0 R >>", BOX + b" ", o9=b"2 0 R"),
    "parent-by-a-chain-of-three": one(b"<< /Type /Page /Parent 9 0 R >>", BOX + b" ", o9=b"10 0 R", o10=b"2 0 R"),
    "parent-by-a-longer-chain": one(b"<< /Type /Page /Parent 9 0 R >>", BOX + b" ", o9=b"10 0 R", o10=b"11 0 R", o11=b"2 0 R"),
    "parent-of-a-parent-by-a-chain": one(b"<< /Type /Page /Parent 6 0 R >>", BOX + b" ", o6=b"<< /Parent 9 0 R >>", o9=b"2 0 R"),
    "parent-names-nothing": one(b"<< /Type /Page /Parent 88 0 R >>", BOX + b" "),
    "parent-not-a-dictionary": one(b"<< /Type /Page /Parent 7 >>", BOX + b" "),
    "parent-stream": one(b"<< /Type /Page /Parent 9 0 R >>", BOX + b" ", o9=b"<< /MediaBox [0 0 31 32] /Length 1 >>\nstream\nx\nendstream"),
    "parent-itself": one(b"<< /Type /Page /Parent 3 0 R >>", BOX + b" "),
    "parent-loop": one(b"<< /Type /Page /Parent 6 0 R >>", BOX + b" ", o6=b"<< /Parent 7 0 R >>", o7=b"<< /Parent 6 0 R >>"),
    "parent-loop-below-a-box": one(b"<< /Type /Page /Parent 6 0 R >>", BOX + b" ", o6=b"<< /Parent 7 0 R /MediaBox [0 0 41 42] >>", o7=b"<< /Parent 6 0 R >>"),
    # What stops the climb, and what doesn't.
    "box-null-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox null >>", BOX + b" "),
    "box-names-null-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox 9 0 R >>", BOX + b" ", o9=b"null"),
    "box-names-nothing-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox 88 0 R >>", BOX + b" "),
    "box-chain-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox 9 0 R >>", BOX + b" ", o9=b"10 0 R", o10=b"[0 0 31 32]"),
    "box-chain-to-nothing-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox 9 0 R >>", BOX + b" ", o9=b"88 0 R"),
    "box-not-an-array-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox 7 >>", BOX + b" "),
    "box-three-numbers-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3] >>", BOX + b" "),
    "box-five-numbers": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200 9] >>"),
    "box-empty-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 0 0] >>", BOX + b" "),
    "box-by-reference": one(b"<< /Type /Page /Parent 2 0 R /MediaBox 9 0 R >>", o9=b"[0 0 3 200]"),
    "box-number-by-reference": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 0 R 200] >>", o9=b"3"),
    "box-number-by-a-chain": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 0 R 200] >>", o9=b"10 0 R", o10=b"3"),
    "box-number-not-a-number": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 (3) 200] >>"),
    "box-number-null": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 null 200] >>"),
    "box-reversed": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [3 200 0 0] >>"),
    "box-none": one(bare()),
    "crop-null-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200] /CropBox null >>", b"/CropBox [0 0 1 2] "),
    "crop-outside-the-media-box": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200] /CropBox [700 0 900 100] >>"),
    "crop-empty": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200] /CropBox [0 0 0 0] >>"),
    "crop-past-the-media-box": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200] /CropBox [-100 -100 300 300] >>"),
    "crop-names-null-over-inherited": one(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200] /CropBox 9 0 R >>", b"/CropBox [0 0 1 2] ", o9=b"null"),
    "rotate-real": one(page(3, 2, b" /Rotate 90.5")),
    "rotate-not-a-quarter": one(page(3, 2, b" /Rotate 45")),
    "rotate-135": one(page(3, 2, b" /Rotate 135")),
    "rotate-negative": one(page(3, 2, b" /Rotate -90")),
    "rotate-450": one(page(3, 2, b" /Rotate 450")),
    "rotate-name": one(page(3, 2, b" /Rotate /R90")),
    "rotate-null-over-inherited": one(page(3, 2, b" /Rotate null"), b"/Rotate 90 "),
    "rotate-names-nothing-over-inherited": one(page(3, 2, b" /Rotate 88 0 R"), b"/Rotate 90 "),
    "rotate-name-over-inherited": one(page(3, 2, b" /Rotate /R90"), b"/Rotate 90 "),
    "user-unit-not-applied": one(page(3, 2, b" /UserUnit 2.5")),
    # Depth: 1,024 levels of nodes open, and no more.
    "depth-1024": deep(1024),
    "depth-1025": deep(1025),
    "depth-past-the-count": deep_past_the_count(1100),
    # Trees as the crate's writer writes them: 33 pages under nodes of 32,
    # leaves of 17 and 16; 64 under nodes of 3, four levels; and 1,025 under
    # nodes of 32, three levels, of which the oracle asks for the first 66.
    "written-33": written(33, 32),
    "written-64-by-3": written(64, 3),
    "written-1025": written(1025, 32),
}


def cases():
    made = [("pages-" + name, "pages", {"pdf": build(objects)}) for name, objects in CASES.items()]
    return made + [("pages-linearized-header", "pages", {"pdf": linearized()})]
