//! Small files written from hand-written bodies, with no object model: what both
//! the tests of what's written and the tests against qpdf go through. Each is
//! built from outside the crate, as a caller of any model builds one, so a way in
//! that's missing doesn't compile.

#![allow(dead_code)]

use std::convert::Infallible;

use pdf_linearize::layout::Options;
use pdf_linearize::pdf_objects::ObjectId;
use pdf_linearize::{
    Encoded, Graph, Group, Kind, Layout, Limits, Page, Piece, Plan, Stream, What, assemble, plan,
};

/// One object as it's written by hand. In `text`, `{5}` is a reference to object 5
/// that a walk goes into, and `{!5}` one that it doesn't: a `/Parent`, a `/Thumb`.
#[derive(Debug, Clone)]
pub struct Object {
    pub number: u32,
    pub text: String,
    pub data: Option<Vec<u8>>,
    pub late: bool,
}

/// A file before it's planned.
#[derive(Debug, Clone)]
pub struct Doc {
    pub kind: Kind,
    /// Whether what may be goes in object streams.
    pub packed: bool,
    /// Whether the layout's own streams are written in hex, a filter of the
    /// closure's, so a reader has to go through it.
    pub hex: bool,
    pub catalog: u32,
    pub objects: Vec<Object>,
    pub pages: Vec<(u32, Vec<u32>, Option<u32>)>,
    pub first: u32,
    pub tree: Vec<u32>,
    pub open: Vec<u32>,
    pub groups: Vec<(Group, Vec<u32>)>,
    pub outlines_first: bool,
    /// The trailer's `/Info`.
    pub info: Option<u32>,
}

/// What each `{n}` in `text` is: the numbers a walk goes into, in order.
fn walked(text: &str) -> Vec<u32> {
    let mut refs = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        let end = rest[at..].find('}').expect("closed") + at;
        if let Ok(number) = rest[at + 1..end].parse() {
            refs.push(number);
        }
        rest = &rest[end + 1..];
    }
    refs
}

/// `text` with each `{n}` and `{!n}` written in the numbers `plan` gives.
fn numbered(text: &str, plan: &Plan) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        let end = rest[at..].find('}').expect("closed") + at;
        out.push_str(&rest[..at]);
        let number: u32 = rest[at + 1..end].trim_start_matches('!').parse().unwrap();
        let written = plan
            .number(ObjectId::new(number, 0))
            .expect("a reference to an object");
        out.push_str(&format!("{} 0 R", written.number));
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

impl Doc {
    /// A catalog at 1 and a page tree at 2 over `pages`, by their numbers.
    pub fn new(kind: Kind, packed: bool, pages: &[u32]) -> Doc {
        assert!(!packed || kind == Kind::Stream);
        let kids: Vec<String> = pages.iter().map(|page| format!("{{!{page}}}")).collect();
        let mut doc = Doc {
            kind,
            packed,
            hex: false,
            catalog: 1,
            objects: Vec::new(),
            pages: pages.iter().map(|&page| (page, Vec::new(), None)).collect(),
            first: 0,
            tree: vec![2],
            open: Vec::new(),
            groups: Vec::new(),
            outlines_first: false,
            info: None,
        };
        doc.object(1, "<< /Type /Catalog /Pages {!2} >>");
        doc.object(
            2,
            &format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                kids.len(),
            ),
        );
        doc
    }

    /// Object `number`.
    pub fn object(&mut self, number: u32, text: &str) -> &mut Doc {
        self.objects.push(Object {
            number,
            text: text.to_string(),
            data: None,
            late: false,
        });
        self
    }

    /// Stream `number`, `text` the entries of its dictionary but `/Length`.
    pub fn stream(&mut self, number: u32, text: &str, data: &[u8]) -> &mut Doc {
        self.objects.push(Object {
            number,
            text: text.to_string(),
            data: Some(data.to_vec()),
            late: false,
        });
        self
    }

    /// Stream `number`, read after the rest of its page: an image's or a font
    /// file's.
    pub fn late_stream(&mut self, number: u32, text: &str, data: &[u8]) -> &mut Doc {
        self.stream(number, text, data);
        self.objects.last_mut().unwrap().late = true;
        self
    }

    /// Page `number`, which has `contents` and `resources`, a dictionary's text,
    /// and the rest of `extra`.
    pub fn page(
        &mut self,
        number: u32,
        contents: &[u32],
        resources: &str,
        extra: &str,
    ) -> &mut Doc {
        let streams: Vec<String> = contents.iter().map(|at| format!("{{{at}}}")).collect();
        self.object(
            number,
            &format!(
                "<< /Type /Page /Parent {{!2}} /MediaBox [0 0 612 792] /Resources {resources} /Contents [{}] {extra} >>",
                streams.join(" "),
            ),
        );
        let page = (self.pages.iter_mut())
            .find(|page| page.0 == number)
            .unwrap();
        page.1 = contents.to_vec();
        self
    }

    /// The catalog's text.
    pub fn catalog_says(&mut self, text: &str) -> &mut Doc {
        self.objects[0].text = format!("<< /Type /Catalog /Pages {{!2}} {text} >>");
        self
    }

    /// The graph it is.
    pub fn graph(&self) -> Graph {
        let mut graph = Graph::new(ObjectId::new(self.catalog, 0), self.kind);
        for object in &self.objects {
            let refs = walked(&object.text)
                .into_iter()
                .map(|number| ObjectId::new(number, 0));
            let node = graph.object(ObjectId::new(object.number, 0), refs);
            node.packable(self.packed && object.data.is_none());
            node.late(object.late);
        }
        for (id, contents, thumb) in &self.pages {
            let mut page = Page::new(ObjectId::new(*id, 0));
            page.contents = contents.iter().map(|&at| ObjectId::new(at, 0)).collect();
            page.thumb = thumb.map(|at| ObjectId::new(at, 0));
            graph.page(page);
        }
        graph.first_page(self.first);
        self.tree
            .iter()
            .for_each(|&at| graph.tree_node(ObjectId::new(at, 0)));
        self.open
            .iter()
            .for_each(|&at| graph.open(ObjectId::new(at, 0)));
        for (group, roots) in &self.groups {
            graph.group(*group, roots.iter().map(|&at| ObjectId::new(at, 0)));
        }
        graph.outlines_first(self.outlines_first);
        graph
    }

    /// What's written for the write of `what`.
    fn body(&self, plan: &Plan, what: What, number: u32) -> Vec<u8> {
        let found = |id: ObjectId| {
            (self.objects.iter())
                .find(|o| o.number == id.number)
                .unwrap()
        };
        match what {
            What::Object(id) => {
                let object = found(id);
                let text = numbered(&object.text, plan);
                let mut out = format!("{number} 0 obj\n").into_bytes();
                match &object.data {
                    None => {
                        out.extend_from_slice(text.as_bytes());
                        out.extend_from_slice(b"\nendobj\n");
                    }
                    Some(data) => {
                        let dictionary = format!("<< {text} /Length {} >>\nstream\n", data.len(),);
                        out.extend_from_slice(dictionary.as_bytes());
                        out.extend_from_slice(data);
                        out.extend_from_slice(b"\nendstream\nendobj\n");
                    }
                }
                out
            }
            What::Container(container) => {
                let container = &plan.containers()[container];
                let mut pairs = String::new();
                let mut objects = String::new();
                for (id, written) in &container.members {
                    pairs.push_str(&format!("{written} {} ", objects.len()));
                    objects.push_str(&numbered(&found(*id).text, plan));
                    objects.push('\n');
                }
                let data = format!("{pairs}\n{objects}");
                format!(
                    "{number} 0 obj\n<< /Type /ObjStm /N {} /First {} /Length {} >>\nstream\n{data}\nendstream\nendobj\n",
                    container.members.len(),
                    pairs.len() + 1,
                    data.len(),
                )
                .into_bytes()
            }
        }
    }

    /// The file, planned, serialized and assembled, under the closure `hex` says.
    pub fn written(&self) -> Written {
        let hex = self.hex;
        self.written_with(move |stream| {
            Ok::<_, Infallible>(if hex { hexed(stream) } else { plain(stream) })
        })
        .unwrap()
    }

    /// The file, under closure `encode`.
    pub fn written_with<E>(
        &self,
        encode: impl FnMut(Stream<'_>) -> Result<Encoded, E>,
    ) -> Result<Written, pdf_linearize::layout::Error<E>> {
        let plan = plan(&self.graph(), &Limits::DEFAULT).expect("planned");
        let bodies: Vec<Vec<u8>> = (plan.writes().iter())
            .map(|write| self.body(&plan, write.what, write.number))
            .collect();
        let lengths: Vec<u64> = bodies.iter().map(|body| body.len() as u64).collect();
        let trailer = match self.info {
            Some(info) => format!(
                "/Info {} 0 R",
                plan.number(ObjectId::new(info, 0)).unwrap().number
            ),
            None => String::new(),
        };
        let mut options = Options::new((1, 7));
        options.trailer = trailer.as_bytes();
        let layout = assemble(&plan, &lengths, &options, encode)?;
        let mut bytes = Vec::with_capacity(layout.len() as usize);
        for piece in layout.pieces() {
            match piece {
                Piece::Bytes(part) => bytes.extend_from_slice(part),
                Piece::Body(at) => bytes.extend_from_slice(&bodies[at]),
            }
        }
        Ok(Written {
            bytes,
            plan,
            layout,
            lengths,
        })
    }
}

/// A file, and what made it.
#[derive(Debug)]
pub struct Written {
    pub bytes: Vec<u8>,
    pub plan: Plan,
    pub layout: Layout,
    pub lengths: Vec<u64>,
}

/// A stream as it is.
pub fn plain(stream: Stream<'_>) -> Encoded {
    Encoded::plain(stream.data.to_vec())
}

/// A stream in hex, a filter no crate's needed for.
pub fn hexed(stream: Stream<'_>) -> Encoded {
    let mut data = String::with_capacity(stream.data.len() * 2 + 1);
    for byte in stream.data {
        data.push_str(&format!("{byte:02X}"));
    }
    data.push('>');
    Encoded::new(b"/Filter /ASCIIHexDecode".to_vec(), data.into_bytes())
}

/// Two pages, sharing nothing, with a stream each.
pub fn two_pages(kind: Kind, packed: bool) -> Doc {
    let mut doc = Doc::new(kind, packed, &[3, 4]);
    doc.page(3, &[5], "<< >>", "");
    doc.page(4, &[6], "<< >>", "");
    doc.stream(5, "", b"0 0 m 1 1 l S");
    doc.stream(6, "", b"0 0 m 2 2 l S");
    doc
}

/// Three pages. Font 8 is the first page's and the second's, so it stays with the
/// first; font 7 is the second's and the third's, so it's in part 8; the second
/// has font 9 alone, and image 10 read last.
pub fn shared(kind: Kind, packed: bool) -> Doc {
    let mut doc = Doc::new(kind, packed, &[3, 4, 11]);
    let font = |n| format!("<< /Font << /F1 {{{n}}} >> >>");
    doc.page(3, &[5], &font(8), "");
    doc.page(
        4,
        &[6],
        "<< /Font << /F1 {7} /F2 {8} /F3 {9} >> /XObject << /Im0 {10} >> >>",
        "",
    );
    doc.page(11, &[12], &font(7), "");
    doc.stream(5, "", b"BT /F1 9 Tf (one) Tj ET");
    doc.stream(6, "", b"BT /F1 9 Tf (two) Tj ET");
    doc.stream(12, "", b"BT /F1 9 Tf (three) Tj ET");
    for (number, name) in [(7, "Helvetica"), (8, "Courier"), (9, "Times-Roman")] {
        doc.object(
            number,
            &format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name} >>"),
        );
    }
    doc.late_stream(
        10,
        "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray",
        b"\x80",
    );
    doc
}

/// Three pages, the second opening first by its `/OpenAction` (F.3.7). The first
/// page's sections are of different lengths, so an entry in the wrong place shows.
pub fn first_page_other(kind: Kind, packed: bool) -> Doc {
    let mut doc = Doc::new(kind, packed, &[3, 4, 7]);
    doc.page(3, &[5], "<< >>", "");
    doc.page(4, &[6], "<< >>", "");
    doc.page(7, &[8], "<< >>", "");
    doc.stream(5, "", b"0 0 m 1 1 l S");
    doc.stream(6, "", &[b' '; 100]);
    doc.stream(8, "", &[b' '; 40]);
    doc.catalog_says("/OpenAction [{!4} /Fit]");
    doc.first = 1;
    doc
}

/// A form: its dictionary at open, alone (F.3.5); its field in part 9, with a widget
/// on each page that's the field's kid.
pub fn form(kind: Kind, packed: bool) -> Doc {
    let mut doc = Doc::new(kind, packed, &[3, 4]);
    doc.page(3, &[5], "<< >>", "/Annots [{7}]");
    doc.page(4, &[6], "<< >>", "/Annots [{8}]");
    doc.stream(5, "", b"0 0 m 1 1 l S");
    doc.stream(6, "", b"0 0 m 2 2 l S");
    doc.object(
        7,
        "<< /Type /Annot /Subtype /Widget /Rect [0 0 9 9] /P {!3} >>",
    );
    doc.object(
        8,
        "<< /Type /Annot /Subtype /Widget /Rect [0 0 9 9] /P {!4} >>",
    );
    doc.object(9, "<< /Fields [{10}] >>");
    doc.object(10, "<< /FT /Tx /T (name) /Kids [{7} {8}] >>");
    doc.catalog_says("/AcroForm {!9}");
    doc.open.push(9);
    doc.groups.push((Group::Form, vec![10]));
    doc
}

/// An outline in part 9, or, read first, in part 6.
pub fn outline(kind: Kind, packed: bool, first: bool) -> Doc {
    let mut doc = two_pages(kind, packed);
    doc.object(7, "<< /Type /Outlines /First {8} /Last {8} /Count 1 >>");
    doc.object(8, "<< /Title (one) /Parent {!7} /Dest [{!4} /Fit] >>");
    let mode = if first { "/PageMode /UseOutlines" } else { "" };
    doc.catalog_says(&format!("/Outlines {{!7}} {mode}"));
    doc.groups.push((Group::Outlines, vec![7]));
    doc.outlines_first = first;
    doc
}

/// Thumbnails on the first and third of four pages, sharing a colour space, and on
/// no others.
pub fn thumbnails(kind: Kind, packed: bool) -> Doc {
    let mut doc = Doc::new(kind, packed, &[3, 4, 5, 6]);
    for (page, content, thumb) in [
        (3, 7, Some(11)),
        (4, 8, None),
        (5, 9, Some(12)),
        (6, 10, None),
    ] {
        let extra = match thumb {
            Some(thumb) => format!("/Thumb {{!{thumb}}}"),
            None => String::new(),
        };
        doc.page(page, &[content], "<< >>", &extra);
        doc.stream(content, "", b"0 0 m 1 1 l S");
        let at = (doc.pages.iter_mut()).find(|p| p.0 == page).unwrap();
        at.2 = thumb;
    }
    let image =
        "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace {13}";
    doc.stream(11, image, b"\x10");
    doc.stream(12, image, &[0x20; 8]);
    doc.object(13, "[/Indexed /DeviceGray 0 (a)]");
    doc
}

/// Every other group a table names, an embedded file reaching what the first page
/// has, and an information dictionary.
pub fn all_tables(kind: Kind, packed: bool) -> Doc {
    let mut doc = Doc::new(kind, packed, &[3, 4]);
    doc.page(3, &[5], "<< /Font << /F1 {19} >> >>", "");
    doc.page(4, &[6], "<< >>", "");
    doc.stream(5, "", b"0 0 m 1 1 l S");
    doc.stream(6, "", b"0 0 m 2 2 l S");
    doc.object(19, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    doc.object(7, "<< /Type /Thread >>");
    doc.object(8, "<< /Title (thread) >>");
    doc.object(9, "<< /Dests {10} >>");
    doc.object(10, "<< /one [{!3} /Fit] >>");
    doc.object(11, "<< /Title (info) >>");
    doc.object(12, "<< /Type /StructTreeRoot /K {13} >>");
    doc.object(13, "<< /S /P >>");
    doc.object(14, "<< /Names [(r) {15}] >>");
    doc.object(15, "<< /S /MR >>");
    doc.stream(16, "/Type /EmbeddedFile", b"file one");
    doc.stream(17, "/Type /EmbeddedFile /Params {18} /X {19}", b"file two");
    doc.object(18, "<< /Size 8 >>");
    doc.groups.extend([
        (Group::ThreadInfo, vec![8]),
        (Group::Dests, vec![9]),
        (Group::Info, vec![11]),
        (Group::Structure, vec![12]),
        (Group::Renditions, vec![14]),
        (Group::EmbeddedFile, vec![16, 17]),
    ]);
    doc.info = Some(11);
    doc
}

/// Many pages, each of a length of its own, with what they share, so a viewer
/// going by the hint tables fetches far less than the file for a page.
pub fn long(kind: Kind, packed: bool, first: u32) -> Doc {
    let count = 24;
    let pages: Vec<u32> = (0..count).map(|at| 10 + at * 2).collect();
    let mut doc = Doc::new(kind, packed, &pages);
    doc.object(3, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    doc.object(4, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>");
    for (at, &page) in pages.iter().enumerate() {
        let font = if at % 3 == 0 { 3 } else { 4 };
        let resources = format!("<< /Font << /F1 {{{font}}} >> >>");
        doc.page(page, &[page + 1], &resources, "");
        let mut content = format!("BT /F1 9 Tf ({at}) Tj ET\n").into_bytes();
        content.resize(4000 + 700 * at, b' ');
        doc.stream(page + 1, "", &content);
    }
    if first != 0 {
        doc.first = first;
        let action = format!("/OpenAction [{{!{}}} /Fit]", pages[first as usize]);
        doc.catalog_says(&action);
    }
    doc
}

/// Many pages all alike, sharing nothing, where every difference in the page table
/// is 0, so that a width of 0 would do and a reader that refuses one fetches the
/// file.
pub fn uniform(kind: Kind) -> Doc {
    let count = 24;
    let pages: Vec<u32> = (0..count).map(|at| 10 + at * 2).collect();
    let mut doc = Doc::new(kind, false, &pages);
    for &page in &pages {
        doc.page(page, &[page + 1], "<< >>", "");
        let mut content = b"BT ET\n".to_vec();
        content.resize(4000, b' ');
        doc.stream(page + 1, "", &content);
    }
    doc
}

/// Every case, each by a name of its own.
pub fn all() -> Vec<(&'static str, Doc)> {
    let hexed = |mut doc: Doc| {
        doc.hex = true;
        doc
    };
    vec![
        ("two-table", two_pages(Kind::Table, false)),
        ("two-stream", two_pages(Kind::Stream, false)),
        ("two-hex", hexed(two_pages(Kind::Stream, false))),
        ("two-packed", two_pages(Kind::Stream, true)),
        ("shared-table", shared(Kind::Table, false)),
        ("shared-stream", shared(Kind::Stream, false)),
        ("shared-packed", shared(Kind::Stream, true)),
        ("shared-packed-hex", hexed(shared(Kind::Stream, true))),
        ("first-table", first_page_other(Kind::Table, false)),
        ("first-stream", first_page_other(Kind::Stream, false)),
        ("first-packed", first_page_other(Kind::Stream, true)),
        ("form-table", form(Kind::Table, false)),
        ("form-stream", form(Kind::Stream, false)),
        ("form-packed", form(Kind::Stream, true)),
        ("outline-table", outline(Kind::Table, false, false)),
        ("outline-first", outline(Kind::Table, false, true)),
        ("outline-stream", outline(Kind::Stream, false, false)),
        ("outline-packed", outline(Kind::Stream, true, false)),
        ("outline-first-packed", outline(Kind::Stream, true, true)),
        ("thumbs-table", thumbnails(Kind::Table, false)),
        ("thumbs-stream", thumbnails(Kind::Stream, false)),
        ("thumbs-packed", thumbnails(Kind::Stream, true)),
        ("all-table", all_tables(Kind::Table, false)),
        ("all-stream", all_tables(Kind::Stream, false)),
        ("all-packed", all_tables(Kind::Stream, true)),
        ("uniform-table", uniform(Kind::Table)),
        ("uniform-stream", uniform(Kind::Stream)),
        ("long-table", long(Kind::Table, false, 0)),
        ("long-stream", long(Kind::Stream, false, 0)),
        ("long-packed", long(Kind::Stream, true, 0)),
        ("long-first-table", long(Kind::Table, false, 5)),
        ("long-first-packed", long(Kind::Stream, true, 5)),
    ]
}
