use pdf_objects::dict;
use pdf_structure::bytes::{Options, Reading};
use pdf_structure::fixture::{self, Body, Xref};
use pdf_structure::source::Document;

use super::*;
use crate::{Object, Why, catalog};

const A: &str = "xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"";
const UA: &str = "xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"";
const VT: &str = "xmlns:pdfvtid=\"http://www.npes.org/pdfvt/ns/id/\"";

/// Keeps what it's told.
#[derive(Default)]
struct Kept(Vec<(Place, Fault)>);

impl Sink for Kept {
    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.push((at, fault));
    }
}

impl Kept {
    fn faults(&self) -> Vec<Fault> {
        self.0.iter().map(|(_, fault)| *fault).collect()
    }
}

/// What a packet claims, and what was said reading it.
fn claims_of(xml: &str) -> (Claims, Vec<Fault>) {
    let mut kept = Kept::default();
    let claims = read(xml.as_bytes(), None, &Limits::DEFAULT, &mut kept).expect("it reads");
    (claims, kept.faults())
}

/// The PDF/A part a packet claims.
fn part(xml: &str) -> Option<String> {
    claims_of(xml).0.pdfa_part
}

/// What a packet claiming that PDF/A part and nothing else claims.
fn claims_with_part(value: &str) -> Claims {
    Claims {
        pdfa_part: some(value),
        ..Claims::default()
    }
}

fn some(value: &str) -> Option<String> {
    Some(value.to_string())
}

/// A packet holding `description`, its `rdf:Description`s.
fn packet(description: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         {description}</rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    )
}

/// A file whose catalog's `/Metadata` holds `xml`, deflated when `deflate`.
fn with_xmp(xml: &str, deflate: bool) -> Vec<u8> {
    let mut metadata = dict! { "Type" => "Metadata", "Subtype" => "XML" };
    let data = if deflate {
        metadata.set("Filter", "FlateDecode");
        fixture::zlib(xml.as_bytes())
    } else {
        xml.as_bytes().to_vec()
    };
    let catalog = dict! { "Type" => "Catalog", "Metadata" => Object::reference(4) };
    let objects = [(1, Body::from(catalog)), (4, Body::Stream(metadata, data))];
    let trailer = dict! { "Root" => Object::reference(1) };
    fixture::file("1.7", &objects, trailer, Xref::Table)
}

/// What the file's packet claims, read as a reader of a file reads it: its
/// catalog, the stream that names, that stream decoded.
fn conformance_of(file: &[u8]) -> Result<Conformance, ClaimError> {
    let reading = Reading::open(file, &Options::DEFAULT).unwrap();
    let objects = &reading;
    let catalog = catalog(objects, reading.trailer().dict()).unwrap().unwrap();
    let id = metadata_stream(objects, catalog).unwrap();
    let Some(id) = id else {
        return Ok(Conformance::default());
    };
    let xml = reading.decoded(id).unwrap();
    read(&xml, None, &Limits::DEFAULT, &mut ())
        .unwrap()
        .conformance()
}

#[test]
fn conformance_reads_pdfa_and_pdfua_claims() {
    let claims =
        |description: &str, deflate| conformance_of(&with_xmp(&packet(description), deflate));
    // Attributes.
    let attributes = "<rdf:Description rdf:about=\"\" \
         xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"2\" pdfaid:conformance=\"b\"/>";
    let pdfa_2b = Conformance {
        pdfa: Some(PdfA {
            part: 2,
            level: Some('B'),
        }),
        pdfua: None,
    };
    assert_eq!(claims(attributes, false).unwrap(), pdfa_2b);
    assert_eq!(claims(attributes, true).unwrap(), pdfa_2b);
    assert_eq!(pdfa_2b.pdfa.unwrap().to_string(), "PDF/A-2b");
    // Elements, under any prefix, and PDF/UA beside PDF/A.
    let elements = "<rdf:Description rdf:about=\"\" xmlns:a=\"http://www.aiim.org/pdfa/ns/id/\">\
         <a:part>3</a:part><a:conformance>A</a:conformance></rdf:Description>\
         <rdf:Description rdf:about=\"\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\">\
         <pdfuaid:part> 1 </pdfuaid:part></rdf:Description>";
    assert_eq!(
        claims(elements, false).unwrap(),
        Conformance {
            pdfa: Some(PdfA {
                part: 3,
                level: Some('A'),
            }),
            pdfua: Some(PdfUa::new(1)),
        }
    );
    // PDF/A-4 gives no level; a part in another namespace is no claim.
    let pdfa_4 = "<rdf:Description xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" \
         xmlns:o=\"http://example.com/other/\" pdfaid:part=\"4\" o:part=\"9\"><o:part>8</o:part>\
         </rdf:Description>";
    assert_eq!(
        claims(pdfa_4, false).unwrap(),
        Conformance {
            pdfa: Some(PdfA {
                part: 4,
                level: None,
            }),
            pdfua: None,
        }
    );
    let named = PdfA::new(4, None);
    assert_eq!(named.to_string(), "PDF/A-4");
    assert_eq!(PdfA::new(2, Some('b')).level, Some('B'));
    assert_eq!(PdfA::new(2, Some('b')).to_string(), "PDF/A-2b");
    assert_eq!(PdfUa::new(1).to_string(), "PDF/UA-1");
    assert_eq!(claims("", false).unwrap(), Conformance::default());
    let unreadable =
        "<rdf:Description xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"two\"/>";
    assert!(claims(unreadable, false).is_err());
    // No /Metadata, no claims.
    let catalog = dict! { "Type" => "Catalog" };
    let trailer = dict! { "Root" => Object::reference(1) };
    let file = fixture::file("1.7", &[(1, Body::from(catalog))], trailer, Xref::Table);
    assert_eq!(conformance_of(&file).unwrap(), Conformance::default());
}

#[test]
fn every_claim_is_read_from_an_attribute_or_an_element() {
    let attributes = format!(
        "<d {A} {UA} {VT} pdfaid:part=\"3\" pdfaid:conformance=\"B\" pdfaid:rev=\"2012\" \
         pdfuaid:part=\"1\" pdfuaid:rev=\"2014\" pdfvtid:GTS_PDFVTVersion=\"PDF/VT-2\"/>"
    );
    let elements = format!(
        "<d {A} {UA} {VT}><pdfaid:part>3</pdfaid:part><pdfaid:conformance>B</pdfaid:conformance>\
         <pdfaid:rev>2012</pdfaid:rev><pdfuaid:part>1</pdfuaid:part><pdfuaid:rev>2014</pdfuaid:rev>\
         <pdfvtid:GTS_PDFVTVersion>PDF/VT-2</pdfvtid:GTS_PDFVTVersion></d>"
    );
    let all = Claims {
        pdfa_part: some("3"),
        pdfa_conformance: some("B"),
        pdfa_rev: some("2012"),
        pdfua_part: some("1"),
        pdfua_rev: some("2014"),
        pdfvt_version: some("PDF/VT-2"),
    };
    assert_eq!(claims_of(&attributes), (all.clone(), vec![]));
    assert_eq!(claims_of(&elements), (all, vec![]));
    // PDF/UA's namespace has no conformance, nor PDF/A's a version.
    let neither = format!("<d {A} {UA} pdfuaid:conformance=\"A\" pdfaid:GTS_PDFVTVersion=\"2\"/>");
    assert_eq!(claims_of(&neither), (Claims::default(), vec![]));
}

/// The namespace says what a name is: the prefix it's written with says
/// nothing.
#[test]
fn a_prefix_counts_by_its_binding() {
    // Bound to nothing, or another namespace.
    assert_eq!(part(r#"<d pdfaid:part="3"/>"#), None);
    assert_eq!(part("<pdfaid:part>3</pdfaid:part>"), None);
    let other = r#"<d xmlns:pdfaid="http://example.com/" pdfaid:part="3"/>"#;
    assert_eq!(part(other), None);
    assert_eq!(part(&format!("<d {A} part=\"3\"/>")), None);
    // Bound on a parent, under another name, or by default for an element,
    // which an attribute doesn't take.
    assert_eq!(
        part(&format!("<d {A}><e pdfaid:part=\"3\"/></d>")),
        some("3")
    );
    let another = r#"<d xmlns:p="http://www.aiim.org/pdfa/ns/id/" p:part="3"/>"#;
    assert_eq!(part(another), some("3"));
    let default = r#"<part xmlns="http://www.aiim.org/pdfa/ns/id/">3</part>"#;
    assert_eq!(part(default), some("3"));
    let default = r#"<d xmlns="http://www.aiim.org/pdfa/ns/id/" part="3"/>"#;
    assert_eq!(part(default), None);
    // Out of scope once its element ends.
    assert_eq!(
        part(&format!("<d><e {A}/><e pdfaid:part=\"3\"/></d>")),
        None
    );
    // A prefix bound again means the new one.
    let again = format!("<d {A}><e xmlns:pdfaid=\"http://example.com/\" pdfaid:part=\"3\"/></d>");
    assert_eq!(part(&again), None);
}

#[test]
fn the_first_value_of_each_counts() {
    let cases = [
        format!("<d {A} pdfaid:part=\"3\"/><d {A} pdfaid:part=\"2\"/>"),
        format!("<d {A} pdfaid:part=\"3\"><pdfaid:part>2</pdfaid:part></d>"),
        format!("<d {A}><pdfaid:part>3</pdfaid:part><pdfaid:part>2</pdfaid:part></d>"),
        format!("<d {A}><pdfaid:part>3</pdfaid:part><e pdfaid:part=\"2\"/></d>"),
        format!("<d {A}><e pdfaid:part=\"3\"/><pdfaid:part>2</pdfaid:part></d>"),
    ];
    for xml in cases {
        assert_eq!(part(&xml), some("3"), "{xml}");
    }
    // A later one that says nothing doesn't take its place.
    let empty = format!("<d {A} pdfaid:part=\"3\"><pdfaid:part></pdfaid:part></d>");
    assert_eq!(part(&empty), some("3"));
    // An element's value is written when it closes, so what's inside it came
    // first.
    let inside = format!("<d {A}><pdfaid:part>2<e pdfaid:part=\"3\"/></pdfaid:part></d>");
    assert_eq!(part(&inside), some("3"));
    // One that says nothing is first all the same; `Claims` holds it.
    assert_eq!(
        part(&format!(
            "<d {A} pdfaid:part=\"\" /><d {A} pdfaid:part=\"3\"/>"
        )),
        some("")
    );
    // Given twice in one tag, the first, and the second said.
    let (claims, faults) = claims_of(&format!("<d {A} pdfaid:part=\"3\" pdfaid:part=\"2\"/>"));
    assert_eq!(
        (claims.pdfa_part, faults),
        (some("3"), vec![Fault::XmpAttribute])
    );
}

#[test]
fn a_value_is_trimmed_of_ascii_s_white_space_alone() {
    assert_eq!(part(&format!("<d {A} pdfaid:part=\" 3\t\n\"/>")), some("3"));
    assert_eq!(
        part(&format!("<d {A}><pdfaid:part>\r\n 3\t</pdfaid:part></d>")),
        some("3")
    );
    // A no-break space is no white space of XML's: it's in the value, and
    // a part with one in it isn't a number.
    let xml = format!("<d {A} pdfaid:part=\"\u{a0}3\u{a0}\"/>");
    assert_eq!(part(&xml), some("\u{a0}3\u{a0}"));
    assert!(claims_of(&xml).0.conformance().is_err());
    let xml = format!("<d {A}><pdfaid:part>\u{a0}3</pdfaid:part></d>");
    assert_eq!(part(&xml), some("\u{a0}3"));
}

#[test]
fn an_element_s_value_is_its_text() {
    let reads = |text: &str| part(&format!("<d {A}><pdfaid:part>{text}</pdfaid:part></d>"));
    // A character by its number, one XML predefines, and CDATA are text,
    // and a comment or a processing instruction no part of it.
    assert_eq!(reads("&#51;"), some("3"));
    assert_eq!(reads("&#x33;"), some("3"));
    assert_eq!(reads("3&amp;4"), some("3&4"));
    assert_eq!(reads("<![CDATA[3]]>"), some("3"));
    assert_eq!(reads("<!-- 2 -->3<?pi 2?>"), some("3"));
    assert_eq!(reads("<![CDATA[<3>]]>"), some("<3>"));
    // An element inside it ends it, unread, as a value isn't markup, and an
    // element with no text has an empty value.
    assert_eq!(reads("<e>3</e>"), None);
    assert_eq!(reads(""), some(""));
    // One left open at the end of the packet isn't read.
    assert_eq!(part(&format!("<d {A}><pdfaid:part>3")), None);
    // A comment, or CDATA, holding what looks like a claim, doesn't claim it.
    assert_eq!(
        part(&format!("<d {A}><!-- <pdfaid:part>3</pdfaid:part> --></d>")),
        None
    );
    assert_eq!(
        part(&format!(
            "<d {A}><![CDATA[<pdfaid:part>3</pdfaid:part>]]></d>"
        )),
        None
    );
}

#[test]
fn an_attribute_that_doesn_t_read_is_passed_and_said() {
    for bad in ["a=1", "a", "a=\"1\" a=\"2\""] {
        for xml in [
            format!("<d {A} {bad} pdfaid:part=\"3\"/>"),
            format!("<d {A} pdfaid:part=\"3\" {bad}/>"),
        ] {
            let (claims, faults) = claims_of(&xml);
            assert_eq!(claims.pdfa_part, some("3"), "{xml}");
            assert_eq!(faults, [Fault::XmpAttribute], "{xml}");
        }
    }
    // A quote left open has no end to its tag: that's XML's error, and what's
    // before it counts.
    let open = format!("<d {A} pdfaid:part=\"3\"/><e a='/>");
    let error = read(open.as_bytes(), None, &Limits::DEFAULT, &mut ()).unwrap_err();
    assert_eq!(error.read.pdfa_part, some("3"));
    // A claim elsewhere in the packet still counts, as it does when elements
    // close round it.
    let xml = format!("<d a=1/><d {A}><e a=1 pdfaid:part=\"3\"/><pdfaid:rev>1</pdfaid:rev></d>");
    let (claims, faults) = claims_of(&xml);
    assert_eq!((claims.pdfa_part, claims.pdfa_rev), (some("3"), some("1")));
    assert_eq!(faults, [Fault::XmpAttribute, Fault::XmpAttribute]);
    // A namespace bound after it isn't: the XML reader stops binding there.
    let late = format!("<d a=1 {A} pdfaid:part=\"3\"/>");
    let (claims, faults) = claims_of(&late);
    assert_eq!(
        (claims.pdfa_part, faults),
        (None, vec![Fault::XmpAttribute])
    );
    // It's said where it is: in the stream's decoded data, as the stream's given.
    let mut kept = Kept::default();
    let xml = "<e/><d a=1/><e/>";
    read(
        xml.as_bytes(),
        Some(ObjectId::new(4, 0)),
        &Limits::DEFAULT,
        &mut kept,
    )
    .unwrap();
    let (place, _) = kept.0[0];
    assert_eq!(place.object, Some(ObjectId::new(4, 0)));
    let at = place.at.unwrap();
    assert_eq!(at.stream, Some(ObjectId::new(4, 0)));
    assert_eq!(&xml[at.start..at.end], "<d a=1/>");
    // With none given, nothing says what its offsets would be of.
    let mut kept = Kept::default();
    read(xml.as_bytes(), None, &Limits::DEFAULT, &mut kept).unwrap();
    assert_eq!(kept.0[0].0, Place::new(None, None));
}

/// No DOCTYPE is read, so what it declares stands for nothing: not billions of
/// laughs, and not a file. The walk goes on past it.
#[test]
fn entities_aren_t_expanded() {
    let laughs = format!(
        "<!DOCTYPE d [<!ENTITY a \"3\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\">\
         <!ENTITY c \"&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;\">\
         <!ENTITY f SYSTEM \"file:///etc/passwd\">]>\
         <d {A} pdfaid:part=\"&a;\" pdfaid:conformance=\"&c;\" pdfaid:rev=\"&f;\">\
         <pdfaid:part>&c;</pdfaid:part><pdfuaid:part {UA}>&a;</pdfuaid:part></d>\
         <d {UA} pdfuaid:part=\"1\"/>"
    );
    let (claims, faults) = claims_of(&laughs);
    assert_eq!(
        claims,
        Claims {
            pdfua_part: some("1"),
            ..Claims::default()
        },
        "what the entities stand for is read nowhere"
    );
    assert_eq!(faults, [Fault::XmpValue; 5]);
    // A value after one that doesn't read is the first that does.
    let xml = format!("<d {A} pdfaid:part=\"&a;\"><pdfaid:part>3</pdfaid:part></d>");
    let (claims, faults) = claims_of(&xml);
    assert_eq!(
        (claims.pdfa_part, faults),
        (some("3"), vec![Fault::XmpValue])
    );
    // XML's own five, and a character by its number, do stand for theirs,
    // but a number that's no character doesn't.
    assert_eq!(part(&format!("<d {A} pdfaid:part=\"&#51;\"/>")), some("3"));
    assert_eq!(
        part(&format!("<d {A} pdfaid:part=\"a&lt;b\"/>")),
        some("a<b")
    );
    let none = format!("<d {A} pdfaid:part=\"&#0;\"><pdfaid:rev>&#0;</pdfaid:rev></d>");
    assert_eq!(
        claims_of(&none),
        (Claims::default(), vec![Fault::XmpValue; 2])
    );
}

#[test]
fn what_isn_t_utf_8_doesn_t_read() {
    // A mark of UTF-16, either way round, and nothing's read (14.3.2).
    let text = format!("<d {A} pdfaid:part=\"3\"/>");
    let be: Vec<u8> = (text.encode_utf16()).flat_map(u16::to_be_bytes).collect();
    let le: Vec<u8> = (text.encode_utf16()).flat_map(u16::to_le_bytes).collect();
    for (mark, units) in [([0xFEu8, 0xFF], be), ([0xFF, 0xFE], le)] {
        let mut kept = Kept::default();
        let xml = [&mark[..], &units].concat();
        assert_eq!(
            read(&xml, None, &Limits::DEFAULT, &mut kept),
            Ok(Claims::default())
        );
        assert_eq!(kept.faults(), [Fault::XmpNotUtf8]);
        // And a mark alone is enough.
        let mut kept = Kept::default();
        let bare = [&mark[..], text.as_bytes()].concat();
        assert_eq!(
            read(&bare, None, &Limits::DEFAULT, &mut kept),
            Ok(Claims::default())
        );
        assert_eq!(kept.faults(), [Fault::XmpNotUtf8]);
    }
    // UTF-8's, and a packet's own header, are read past.
    assert_eq!(part(&format!("\u{feff}{text}")), some("3"));
    assert_eq!(part(&packet(&text)), some("3"));
    // A value that isn't UTF-8 is none, said.
    let bad = |xml: String| {
        let mut kept = Kept::default();
        let xml = xml.replace('~', "\u{1}").into_bytes();
        let xml: Vec<u8> = xml.iter().map(|&b| if b == 1 { 0xFF } else { b }).collect();
        let claims = read(&xml, None, &Limits::DEFAULT, &mut kept).unwrap();
        (claims.pdfa_part, kept.faults())
    };
    assert_eq!(
        bad(format!("<d {A} pdfaid:part=\"~\"/>")),
        (None, vec![Fault::XmpValue])
    );
    assert_eq!(
        bad(format!("<d {A}><pdfaid:part>3~</pdfaid:part></d>")),
        (None, vec![Fault::XmpValue])
    );
    assert_eq!(
        bad(format!(
            "<d {A}><pdfaid:part><![CDATA[~]]></pdfaid:part><pdfaid:part>3</pdfaid:part></d>"
        )),
        (some("3"), vec![Fault::XmpValue])
    );
}

#[test]
fn an_error_keeps_what_was_read() {
    let xml = format!(
        "<d {A} pdfaid:part=\"3\"><pdfaid:rev>2</pdfaid:rev></e>\
         <d {UA} pdfuaid:part=\"1\"/>"
    );
    let error = read(xml.as_bytes(), None, &Limits::DEFAULT, &mut ()).unwrap_err();
    assert_eq!(error.read.pdfa_part, some("3"));
    assert_eq!(error.read.pdfa_rev, some("2"));
    assert_eq!(error.read.pdfua_part, None, "what's after it isn't read");
    assert!(matches!(error.why, XmpWhy::Xml(_)), "{error}");
    assert_eq!(error.at, xml.find("</e>").unwrap());
    let message = error.to_string();
    assert!(message.contains(&format!("byte {}", error.at)), "{message}");
    assert!(std::error::Error::source(&error).is_none());
    // Binding a prefix XML keeps for itself is such an error too.
    let xml = format!("<d {A} pdfaid:part=\"3\"/><e xmlns:xml=\"u\"/>");
    let error = read(xml.as_bytes(), None, &Limits::DEFAULT, &mut ()).unwrap_err();
    assert_eq!(error.read.pdfa_part, some("3"));
    // A tag not closed at the end is one; an element left open isn't.
    let error = read(
        format!("<d {A} pdfaid:part=\"3\"/><e").as_bytes(),
        None,
        &Limits::DEFAULT,
        &mut (),
    );
    let error = error.unwrap_err();
    assert_eq!(error.read.pdfa_part, some("3"));
    assert!(matches!(error.why, XmpWhy::Xml(_)), "{error}");
    assert_eq!(part(&format!("<d {A} pdfaid:part=\"3\"><e>")), some("3"));
}

#[test]
#[allow(clippy::result_large_err)]
fn past_a_cap_is_an_error_keeping_what_was_read() {
    let claim = format!("<c {A} pdfaid:part=\"3\"/>");
    let limits = |set: fn(&mut Limits)| {
        let mut limits = Limits::DEFAULT;
        set(&mut limits);
        limits
    };
    let run = |xml: &str, limits: &Limits| read(xml.as_bytes(), None, limits, &mut ());

    // Bytes: a claim that ends by the cap counts, one past it doesn't.
    let xml = format!("{claim}{}<d {UA} pdfuaid:part=\"1\"/>", " ".repeat(64));
    let at = xml.find("<d").unwrap();
    let mut small = Limits::DEFAULT;
    small.xmp_bytes = xml.len();
    assert_eq!(run(&xml, &small).unwrap().pdfua_part, some("1"));
    small.xmp_bytes = xml.len() - 1;
    let error = run(&xml, &small).unwrap_err();
    assert_eq!((error.why, error.at), (XmpWhy::TooLong, xml.len() - 1));
    assert_eq!(
        (error.read.pdfa_part, error.read.pdfua_part),
        (some("3"), None)
    );
    small.xmp_bytes = at;
    let error = run(&xml, &small).unwrap_err();
    assert_eq!((error.why, error.at), (XmpWhy::TooLong, at));
    assert_eq!(error.read.pdfa_part, some("3"));
    // Cut in a tag, or in a reference, that's what it says too.
    let in_tag = format!("{claim}<e a=\"\"/>");
    small.xmp_bytes = in_tag.len() - 3;
    assert_eq!(run(&in_tag, &small).unwrap_err().why, XmpWhy::TooLong);
    let in_reference = format!("{claim}<e>&#51;</e>");
    small.xmp_bytes = in_reference.len() - 7;
    assert_eq!(run(&in_reference, &small).unwrap_err().why, XmpWhy::TooLong);
    // By default, 16 MiB of it.
    let long = format!("{claim}{}", " ".repeat(16 << 20));
    let error = run(&long, &Limits::DEFAULT).unwrap_err();
    assert_eq!((error.why, error.at), (XmpWhy::TooLong, 16 << 20));
    assert_eq!(error.read.pdfa_part, some("3"));
    assert!(run(&long, &limits(|l| l.xmp_bytes = 17 << 20)).is_ok());

    // Depth, by default 256: an element at the level past it isn't read,
    // whichever kind.
    let nested = |levels: usize, inside: &str| format!("{claim}{}{inside}", "<a>".repeat(levels));
    let inner = format!("<i {UA} pdfuaid:part=\"1\"/>");
    let read_at = run(&nested(255, &inner), &Limits::DEFAULT).unwrap();
    assert_eq!(read_at.pdfua_part, some("1"));
    let error = run(&nested(256, &inner), &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.why, XmpWhy::TooDeep);
    assert_eq!(error.at, nested(256, "").len());
    assert_eq!(
        (error.read.pdfa_part, error.read.pdfua_part),
        (some("3"), None)
    );
    let open = run(&nested(257, ""), &Limits::DEFAULT).unwrap_err();
    assert_eq!(open.why, XmpWhy::TooDeep);
    assert_eq!(open.read.pdfa_part, some("3"));
    // What's closed is no longer deep.
    let wide = format!("{}{}", "<a></a>".repeat(1000), nested(255, &inner));
    assert_eq!(run(&wide, &Limits::DEFAULT).unwrap().pdfua_part, some("1"));
    let raised = limits(|l| l.xmp_depth = 1000);
    assert_eq!(
        run(&nested(999, &inner), &raised).unwrap().pdfua_part,
        some("1")
    );

    // Bindings, by default 128 at once, wherever they're bound.
    let binds = |from: usize, count: usize| -> String {
        (from..from + count)
            .map(|i| format!(" xmlns:q{i}=\"u\""))
            .collect()
    };
    let spread = |levels: usize, each: usize, inside: &str| -> String {
        let open: String = (0..levels)
            .map(|l| format!("<a{}>", binds(l * each, each)))
            .collect();
        format!("{claim}{open}{inside}")
    };
    // `inner` binds one, so 127 over it is 128.
    for (levels, each) in [(1, 127), (127, 1), (6, 21)] {
        let xml = spread(levels, each, &inner);
        assert_eq!(
            run(&xml, &Limits::DEFAULT).unwrap().pdfua_part,
            some("1"),
            "{levels} {each}"
        );
    }
    let error = run(&spread(1, 128, &inner), &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.why, XmpWhy::TooManyBindings);
    assert_eq!(error.at, spread(1, 128, "").len());
    assert_eq!(
        (error.read.pdfa_part, error.read.pdfua_part),
        (some("3"), None)
    );
    // One tag alone past it, the XML reader's own count, the same; raised, it
    // lets it through.
    let one = format!("{claim}<a{}/>", binds(0, 129));
    let error = run(&one, &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.why, XmpWhy::TooManyBindings);
    assert_eq!(error.read.pdfa_part, some("3"));
    assert!(run(&one, &limits(|l| l.xmp_bindings = 129)).is_ok());
    // What's closed is no longer bound.
    let closed: String = (0..300).map(|i| format!("<a{}/>", binds(i, 100))).collect();
    let closed = format!("{closed}{}", "<b xmlns:r=\"u\"></b>".repeat(300));
    assert!(run(&closed, &Limits::DEFAULT).is_ok());
    // A level's bindings go with it.
    let lowered = limits(|l| l.xmp_bindings = 1);
    assert_eq!(
        run("<a xmlns:q=\"u\"></a><a xmlns:q=\"u\"/>", &lowered),
        Ok(Claims::default())
    );
    assert_eq!(
        run("<a xmlns:q=\"u\"><a xmlns:r=\"u\"/></a>", &lowered)
            .unwrap_err()
            .why,
        XmpWhy::TooManyBindings
    );
    // The default caps are the documented ones.
    let default = Limits::DEFAULT;
    assert_eq!(
        (default.xmp_bytes, default.xmp_depth, default.xmp_bindings),
        (16 << 20, 256, 128)
    );
}

/// A bare `&` in a packet's text ends nothing, and a claim's value with one
/// doesn't read, as an attribute's doesn't.
#[test]
fn a_bare_ampersand_is_read_past() {
    // In a title, or anywhere that isn't a claim, it's nothing to the claims.
    for text in ["AT&T", "&", "a & b", "&#51", "&; x", "a&b&c"] {
        let xml = format!("<d {A}><t>{text}</t><pdfaid:part>3</pdfaid:part></d>");
        assert_eq!(claims_of(&xml), (claims_with_part("3"), vec![]), "{text}");
    }
    // In a claim's, it's said, the value's lost, and a later one counts.
    for value in ["3&4", "&", "3&", "&#51"] {
        let xml =
            format!("<d {A}><pdfaid:part>{value}</pdfaid:part><pdfaid:part>2</pdfaid:part></d>");
        assert_eq!(
            claims_of(&xml),
            (claims_with_part("2"), vec![Fault::XmpValue]),
            "{value}"
        );
    }
    // A packet that ends on one has read all there is.
    assert_eq!(part(&format!("<d {A} pdfaid:part=\"3\"/>&")), some("3"));
    assert_eq!(
        claims_of(&format!("<d {A}><pdfaid:part>&#51")),
        (Claims::default(), vec![Fault::XmpValue])
    );
}

/// The XML reader counts levels in 16 bits, so a cap raised past that stops
/// where it can still count, however deep the packet.
#[test]
fn a_cap_of_depth_raised_past_the_xml_reader_s_stops_there() {
    let mut limits = Limits::DEFAULT;
    limits.xmp_depth = 100_000;
    let open = "<b>".repeat(70_000);
    let xml = format!("<d {A} pdfaid:part=\"3\">{open}<pdfaid:conformance>B</pdfaid:conformance>");
    let error = read(xml.as_bytes(), None, &limits, &mut ()).unwrap_err();
    assert_eq!(error.why, XmpWhy::TooDeep);
    assert_eq!(error.read, claims_with_part("3"));
    assert_eq!(error.at, xml.find("<b>").unwrap() + 3 * 65_533);
}

/// Each cap's worst shape, read as any other is, to the end: what it costs
/// isn't a unit test's to time.
#[test]
fn the_worst_shapes_under_the_caps_read_to_the_end() {
    let mut limits = Limits::DEFAULT;
    limits.xmp_bytes = 1 << 20;
    // The most bound at once: over levels, one to a level but the first's two,
    // and on one tag.
    for (depth, each, more) in [(32, 4, 0), (127, 1, 1), (1, 128, 0)] {
        let mut xml = String::new();
        for level in 0..depth {
            xml += "<a";
            let each = if level == 0 { each + more } else { each };
            for i in 0..each {
                xml += &format!(" xmlns:q{level}x{i}=\"u\"");
            }
            xml += ">";
        }
        while xml.len() < limits.xmp_bytes {
            xml += "<q0x0:part q0x0:part=\"\"/>";
        }
        let error = read(xml.as_bytes(), None, &limits, &mut ()).unwrap_err();
        assert_eq!(error.why, XmpWhy::TooLong);
        assert_eq!(error.read, Claims::default());
    }
}

#[test]
fn what_s_read_is_the_same_whatever_the_sink() {
    let packets = [
        format!("<d {A} a=1 pdfaid:part=\"3\" pdfaid:rev=\"&x;\"/>"),
        format!("<d {A} a=1 pdfaid:part=\"3\"></e>"),
        "\u{feff}<d/>".to_string(),
    ];
    for xml in &packets {
        let mut kept = Kept::default();
        let told = read(xml.as_bytes(), None, &Limits::DEFAULT, &mut kept);
        let quiet = read(xml.as_bytes(), None, &Limits::DEFAULT, &mut ());
        assert_eq!(told, quiet, "{xml}");
    }
    let mut kept = Kept::default();
    read(packets[0].as_bytes(), None, &Limits::DEFAULT, &mut kept).unwrap();
    assert_eq!(kept.faults(), [Fault::XmpAttribute, Fault::XmpValue]);
    // Through `&mut dyn` it's told the same.
    let dynamic: &mut dyn Sink = &mut kept;
    read(packets[0].as_bytes(), None, &Limits::DEFAULT, dynamic).unwrap();
    assert_eq!(kept.faults().len(), 4);
}

#[test]
fn a_claim_that_doesn_t_say_what_it_s_for_is_an_error_of_its_own() {
    let of = |pdfa_part: Option<&str>, pdfa_conformance: Option<&str>, pdfua_part: Option<&str>| {
        Claims {
            pdfa_part: pdfa_part.map(String::from),
            pdfa_conformance: pdfa_conformance.map(String::from),
            pdfua_part: pdfua_part.map(String::from),
            ..Claims::default()
        }
        .conformance()
    };
    assert_eq!(
        of(Some("two"), None, None),
        Err(ClaimError::Part {
            claim: "pdfaid:part",
            value: "two".to_string()
        })
    );
    assert_eq!(
        of(None, None, Some("")),
        Err(ClaimError::Part {
            claim: "pdfuaid:part",
            value: String::new()
        })
    );
    assert_eq!(
        of(Some("2"), Some(""), None),
        Err(ClaimError::Level {
            value: String::new()
        })
    );
    assert_eq!(
        of(Some("2"), Some("ab"), None),
        Err(ClaimError::Level {
            value: "ab".to_string()
        })
    );
    assert_eq!(
        of(Some("2"), Some("1"), None),
        Err(ClaimError::Level {
            value: "1".to_string()
        })
    );
    assert_eq!(
        ClaimError::Part {
            claim: "pdfaid:part",
            value: "two".to_string()
        }
        .to_string(),
        "pdfaid:part \"two\" isn't a number"
    );
    assert_eq!(
        ClaimError::Level {
            value: "ab".to_string()
        }
        .to_string(),
        "pdfaid:conformance \"ab\" isn't a letter"
    );
    // A level without a part is no claim, though it has to be one.
    assert_eq!(of(None, Some("b"), None), Ok(Conformance::default()));
    assert!(of(None, Some("bb"), None).is_err());
    // Claims still holds what the others say.
    let claims = claims_of(&format!(
        "<d {A} pdfaid:part=\"3\" pdfaid:conformance=\"\"/>"
    ))
    .0;
    assert!(claims.conformance().is_err());
    assert_eq!(claims.pdfa_part, some("3"));
    // What's upper or lower case comes out upper.
    let read = of(Some("2"), Some("u"), Some("1")).unwrap();
    assert_eq!(
        read.pdfa,
        Some(PdfA {
            part: 2,
            level: Some('U')
        })
    );
    assert_eq!(read.pdfua, Some(PdfUa::new(1)));
}

#[test]
fn the_catalog_s_metadata_stream_is_found_or_says_why_not() {
    let stream = Object::stream(dict! { "Type" => "Metadata" });
    let chain = std::collections::BTreeMap::from([
        (ObjectId::new(4, 0), Object::reference(5)),
        (ObjectId::new(5, 0), stream),
        (ObjectId::new(6, 0), Object::from(dict! {})),
    ]);
    let resolve = |id: ObjectId| chain.get(&id);
    let of = |metadata: Option<Object>| {
        let mut catalog = dict! {};
        if let Some(metadata) = metadata {
            catalog.set("Metadata", metadata);
        }
        metadata_stream(resolve, Box::leak(Box::new(catalog)))
    };
    assert_eq!(of(None), Ok(None));
    assert_eq!(
        of(Some(Object::reference(5))),
        Ok(Some(ObjectId::new(5, 0)))
    );
    assert_eq!(
        of(Some(Object::reference(4))),
        Ok(Some(ObjectId::new(5, 0))),
        "the stream it comes to"
    );
    assert_eq!(of(Some(Object::reference(6))), Ok(None), "no stream");
    assert_eq!(of(Some(Object::reference(9))), Ok(None), "no object");
    assert_eq!(of(Some(Object::from(3))), Ok(None));

    // A loop of references is a limit; the same over a reading and its
    // document, whose stream it reads by number.
    let looped = std::collections::BTreeMap::from([(ObjectId::new(4, 0), Object::reference(4))]);
    let resolve = |id: ObjectId| looped.get(&id);
    let catalog = dict! { "Metadata" => Object::reference(4) };
    assert_eq!(
        metadata_stream(resolve, &catalog).unwrap_err().why,
        Why::Limit
    );

    let file = with_xmp("<d/>", false);
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let objects = &reading;
    let found = catalog_of(objects, reading.trailer().dict());
    assert_eq!(found, Some(ObjectId::new(4, 0)));
    let document: Document = reading.to_document().unwrap();
    let objects = &document;
    assert_eq!(catalog_of(objects, document.trailer.dict()), found);

    // An object that doesn't read says so, and isn't taken for none.
    let locked = |id: ObjectId| -> Result<Option<&Object>, Unreadable> {
        Err(Unreadable::new(id, Why::Locked, "it takes a password"))
    };
    let failing = Failing(locked);
    let catalog = dict! { "Metadata" => Object::reference(4) };
    assert_eq!(
        metadata_stream(&failing, &catalog).unwrap_err().why,
        Why::Locked
    );
}

/// The metadata stream of the catalog a trailer leads to.
fn catalog_of<'o, R: Resolve<'o> + Copy>(objects: R, trailer: &'o Dict) -> Option<ObjectId> {
    metadata_stream(objects, catalog(objects, trailer).unwrap()?).unwrap()
}

/// A resolver that says why, by a function.
struct Failing<F>(F);

impl<'o, F: Fn(ObjectId) -> Result<Option<&'o Object>, Unreadable>> Resolve<'o> for &Failing<F> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (self.0)(id).ok().flatten()
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (self.0)(id)
    }
}
