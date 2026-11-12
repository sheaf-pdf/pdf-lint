use pdf_fixtures::fixture::{self, Body, Xref};
use pdf_objects::{Object, Stream, Version, Why, WithStreams, dict};
use pdf_structure::pdf_crypt::Password;
use pdf_structure::pdf_crypt::writer::{Kind, Settings};
use pdf_structure::read::{Options, Reading, WithReading};
use pdf_structure::source::Document;

use super::*;
use crate::catalog;
use crate::tree::tests::ByPosition;

const A: &str = "xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"";
const UA: &str = "xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"";
const VT: &str = "xmlns:pdfvtid=\"http://www.npes.org/pdfvt/ns/id/\"";

/// Keeps what it's told.
#[derive(Default)]
struct Kept(Vec<(Place, Fault)>);

impl Sink<Fault> for Kept {
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
    let xml = reading.decoded_within(id, usize::MAX).unwrap();
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
    // first; and this one, holding an element beside its text, is mixed.
    let inside = format!("<d {A}><pdfaid:part>2<e pdfaid:part=\"3\"/></pdfaid:part></d>");
    assert_eq!(
        claims_of(&inside),
        (claims_with_part("3"), vec![Fault::XmpMixed])
    );
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
    // An element inside it makes it no text, as a value isn't markup, and
    // what's inside that is no part of it; an element with no text has an
    // empty value.
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

/// An empty element is read as one written open and closed: an empty value,
/// the first of its claim where none came before it.
#[test]
fn a_claim_s_empty_element_is_an_empty_value() {
    for empty in [
        "<pdfaid:part/>",
        "<pdfaid:part />",
        "<pdfaid:part></pdfaid:part>",
    ] {
        // First, so the value after it doesn't count.
        let xml = format!("<d {A}>{empty}<pdfaid:part>3</pdfaid:part></d>");
        assert_eq!(claims_of(&xml), (claims_with_part(""), vec![]), "{empty}");
        // After a value, by an attribute or by an element, it's passed.
        let xml = format!("<d {A} pdfaid:part=\"3\">{empty}</d>");
        assert_eq!(claims_of(&xml), (claims_with_part("3"), vec![]), "{empty}");
        let xml = format!("<d {A}><pdfaid:part>3</pdfaid:part>{empty}</d>");
        assert_eq!(claims_of(&xml), (claims_with_part("3"), vec![]), "{empty}");
    }
    // An attribute of its own is read before it, as on any tag.
    let xml = format!("<d {A}><pdfaid:part pdfaid:part=\"3\"/></d>");
    assert_eq!(part(&xml), some("3"));
    // Under any prefix, by default too, and in each namespace.
    let default = r#"<part xmlns="http://www.aiim.org/pdfa/ns/id/"/>"#;
    assert_eq!(part(default), some(""));
    let all = format!(
        "<d {A} {UA} {VT}><pdfaid:part/><pdfaid:conformance/><pdfaid:rev/>\
         <pdfuaid:part/><pdfuaid:rev/><pdfvtid:GTS_PDFVTVersion/></d>"
    );
    let empty = Claims {
        pdfa_part: some(""),
        pdfa_conformance: some(""),
        pdfa_rev: some(""),
        pdfua_part: some(""),
        pdfua_rev: some(""),
        pdfvt_version: some(""),
    };
    assert_eq!(claims_of(&all), (empty.clone(), vec![]));
    // A part or a level that says nothing is no part or level.
    assert!(empty.conformance().is_err());
    // Bound to nothing, it's no claim.
    assert_eq!(
        part("<pdfaid:part/><d><pdfaid:part>3</pdfaid:part></d>"),
        None
    );
}

/// A claim's element holding an element isn't text: with text of its own
/// beside it, its value is mixed, told from its start tag to its end tag;
/// with white space alone, or nothing, it's structured, and not told.
/// Neither is taken, so a value after it counts.
#[test]
fn a_claim_s_element_holding_an_element_isn_t_its_value() {
    let then = "<pdfaid:part>2</pdfaid:part>";
    let mixed = [
        "3<e/>",
        "<e/>3",
        "3<e/>4",
        "3<e>4</e>",
        "<e>4</e>3",
        " 3 <e/> ",
        "&#51;<e/>",
        "<e/>&amp;",
        "<![CDATA[3]]><e/>",
        "<e/><![CDATA[3]]>",
        "<!-- a -->3<?pi b?><e/>",
        "\u{a0}<e/>",
    ];
    for inside in mixed {
        let claim = format!("<pdfaid:part>{inside}</pdfaid:part>");
        let xml = format!("<d {A}>{claim}{then}</d>");
        let mut kept = Kept::default();
        let claims = read(xml.as_bytes(), None, &Limits::DEFAULT, &mut kept).unwrap();
        assert_eq!(claims, claims_with_part("2"), "{inside}");
        assert_eq!(kept.faults(), [Fault::XmpMixed], "{inside}");
        // Where it is: the claim's element, start tag to end tag.
        let mut kept = Kept::default();
        read(
            xml.as_bytes(),
            Some(ObjectId::new(4, 0)),
            &Limits::DEFAULT,
            &mut kept,
        )
        .unwrap();
        let at = kept.0[0].0.at.unwrap();
        assert_eq!(&xml[at.start..at.end], claim, "{inside}");
        // Alone, it's no value.
        let xml = format!("<d {A}>{claim}</d>");
        assert_eq!(claims_of(&xml), (Claims::default(), vec![Fault::XmpMixed]));
    }
    let structured = [
        "<e/>",
        "<e></e>",
        "<e>3</e>",
        " \r\n\t<e/>\n<e>3</e> ",
        "<![CDATA[ ]]><e/>",
        "<!-- 3 --><e/><?pi 3?>",
        "<e><f>3</f></e>",
    ];
    for inside in structured {
        let xml = format!("<d {A}><pdfaid:part>{inside}</pdfaid:part>{then}</d>");
        assert_eq!(claims_of(&xml), (claims_with_part("2"), vec![]), "{inside}");
        let xml = format!("<d {A}><pdfaid:part>{inside}</pdfaid:part></d>");
        assert_eq!(claims_of(&xml), (Claims::default(), vec![]), "{inside}");
    }
    // A comment or a processing instruction is no element.
    let xml = format!("<d {A}><pdfaid:part>3<!-- 2 --><?pi 2?></pdfaid:part></d>");
    assert_eq!(claims_of(&xml), (claims_with_part("3"), vec![]));
}

/// A claim's element in another's is read as its own, and the other holds
/// an element: mixed beside text, structured beside none.
#[test]
fn a_claim_s_element_in_another_s_is_read_as_its_own() {
    let level = |part: Option<&str>| Claims {
        pdfa_part: part.map(str::to_string),
        pdfa_conformance: some("B"),
        ..Claims::default()
    };
    let inner = "<pdfaid:conformance>B</pdfaid:conformance>";
    let shapes = [
        (format!("2{inner}"), level(None), vec![Fault::XmpMixed]),
        (format!("{inner}2"), level(None), vec![Fault::XmpMixed]),
        (format!(" {inner} "), level(None), vec![]),
        (format!("<e>{inner}</e>"), level(None), vec![]),
        (
            format!("2<e>{inner}</e>"),
            level(None),
            vec![Fault::XmpMixed],
        ),
    ];
    for (inside, claims, faults) in shapes {
        let xml = format!("<d {A}><pdfaid:part>{inside}</pdfaid:part></d>");
        assert_eq!(claims_of(&xml), (claims, faults), "{inside}");
    }
    // The same claim in itself: the innermost's value, first to close.
    let nested = format!(
        "<d {A}>{}3{}</d>",
        "<pdfaid:part>".repeat(3),
        "</pdfaid:part>".repeat(3)
    );
    assert_eq!(claims_of(&nested), (claims_with_part("3"), vec![]));
    let nested = format!("<d {A}><pdfaid:part>2<pdfaid:part>3</pdfaid:part></pdfaid:part></d>");
    assert_eq!(
        claims_of(&nested),
        (claims_with_part("3"), vec![Fault::XmpMixed])
    );
}

/// At the edges: what's in an element in a claim's is no part of its value,
/// nor read for it; a value that doesn't read is lost, told once, mixed or
/// not; a claim's element whose claim has its value isn't read; and one
/// left open isn't taken or told.
#[test]
fn a_claim_s_value_is_its_own_level_s_text() {
    let then = "<pdfaid:part>2</pdfaid:part>";
    // Inside an element in it, an `&`, or an entity, isn't read, nor told.
    for inside in [
        "<e>&</e>",
        "<e>&x;</e>",
        "<e>3&4</e>",
        "<e><![CDATA[3]]></e>",
    ] {
        let xml = format!("<d {A}><pdfaid:part>{inside}</pdfaid:part>{then}</d>");
        assert_eq!(claims_of(&xml), (claims_with_part("2"), vec![]), "{inside}");
    }
    // At its own level, before an element or after, the value's lost and
    // told so, once, and not as mixed too.
    for inside in [
        "&x;<e/>",
        "<e/>&x;",
        "<e></e>&x;",
        "3&4<e/>",
        "<e></e>3&4",
        "<e>3</e>&x;3&y;",
        "&x;<e/>&y;",
    ] {
        let xml = format!("<d {A}><pdfaid:part>{inside}</pdfaid:part>{then}</d>");
        assert_eq!(
            claims_of(&xml),
            (claims_with_part("2"), vec![Fault::XmpValue]),
            "{inside}"
        );
    }
    // A value of a claim that has one isn't read: nothing's told of it.
    for inside in ["2<e/>", "&x;", "<e/>&x;"] {
        let xml = format!("<d {A} pdfaid:part=\"3\"><pdfaid:part>{inside}</pdfaid:part></d>");
        assert_eq!(claims_of(&xml), (claims_with_part("3"), vec![]), "{inside}");
    }
    // Left open at the end, or where an error stops the read, it isn't
    // taken, mixed or not, and isn't told.
    for inside in ["3", "3<e/>", "3<e>4</e>"] {
        let xml = format!("<d {A}><pdfaid:part>{inside}");
        assert_eq!(claims_of(&xml), (Claims::default(), vec![]), "{inside}");
        let xml = format!("<d {A}><pdfaid:part>{inside}</x>{then}");
        let mut kept = Kept::default();
        let error = read(xml.as_bytes(), None, &Limits::DEFAULT, &mut kept).unwrap_err();
        assert_eq!((error.read, kept.faults()), (Claims::default(), vec![]));
    }
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
    // A mark of UTF-16 or UTF-32, either way round, or none, and nothing's
    // read (14.3.2): told at the mark, or at the first four bytes, where a
    // zero byte says it isn't UTF-8.
    let text = format!("<d {A} pdfaid:part=\"3\"/>");
    let utf16 = |be: bool| -> Vec<u8> {
        (text.encode_utf16())
            .flat_map(|unit| {
                if be {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                }
            })
            .collect()
    };
    let utf32 = |be: bool| -> Vec<u8> {
        (text.chars())
            .flat_map(|c| {
                if be {
                    u32::from(c).to_be_bytes()
                } else {
                    u32::from(c).to_le_bytes()
                }
            })
            .collect()
    };
    let shapes: [(&[u8], Vec<u8>, usize); 8] = [
        (&[0xFE, 0xFF], utf16(true), 2),
        (&[0xFF, 0xFE], utf16(false), 2),
        (&[0x00, 0x00, 0xFE, 0xFF], utf32(true), 4),
        (&[0xFF, 0xFE, 0x00, 0x00], utf32(false), 4),
        (&[], utf16(true), 4),
        (&[], utf16(false), 4),
        (&[], utf32(true), 4),
        (&[], utf32(false), 4),
    ];
    for (mark, units, end) in shapes {
        let xml = [mark, &units].concat();
        let mut kept = Kept::default();
        let stream = Some(ObjectId::new(4, 0));
        let answer = read(&xml, stream, &Limits::DEFAULT, &mut kept);
        assert_eq!(answer, Ok(Claims::default()), "{mark:x?}");
        assert_eq!(kept.faults(), [Fault::XmpNotUtf8], "{mark:x?}");
        let at = kept.0[0].0.at.unwrap();
        assert_eq!((at.start, at.end), (0, end), "{mark:x?}");
        // And a mark alone is enough.
        if !mark.is_empty() {
            let bare = [mark, text.as_bytes()].concat();
            let mut kept = Kept::default();
            let answer = read(&bare, None, &Limits::DEFAULT, &mut kept);
            assert_eq!(answer, Ok(Claims::default()), "{mark:x?}");
            assert_eq!(kept.faults(), [Fault::XmpNotUtf8], "{mark:x?}");
        }
    }
    // A zero byte anywhere among the first four, in a packet shorter than
    // that too, at what there is; past them, it's XML's to say.
    for (xml, end) in [
        (&b"\0<d/>"[..], 4),
        (b"<d\0/>", 4),
        (b"<d/\0", 4),
        (b"<\0", 2),
    ] {
        let mut kept = Kept::default();
        let stream = Some(ObjectId::new(4, 0));
        assert_eq!(
            read(xml, stream, &Limits::DEFAULT, &mut kept),
            Ok(Claims::default())
        );
        assert_eq!(kept.faults(), [Fault::XmpNotUtf8]);
        let at = kept.0[0].0.at.unwrap();
        assert_eq!((at.start, at.end), (0, end), "{xml:x?}");
    }
    let late = format!("<d {A} pdfaid:part=\"3\"/>\0");
    assert_eq!(part(&late), some("3"));
    assert_eq!(
        Fault::XmpNotUtf8.to_string(),
        "XMP metadata isn't encoded as UTF-8"
    );
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
    let dynamic: &mut dyn Sink<Fault> = &mut kept;
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
        Why::Capped
    );

    let file = with_xmp("<d/>", false);
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let objects = &reading;
    let found = catalog_of(objects, reading.trailer().dict());
    assert_eq!(found, Some(ObjectId::new(4, 0)));
    let document: Document = reading.to_document().unwrap();
    let objects = &document;
    assert_eq!(catalog_of(objects, document.trailer().dict()), found);

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

/// What [`Claim::of`] reads, [`Claim::written`] writes: each claim its own
/// name, in its namespace, under the one prefix a packet written binds the
/// namespace to, in the order [`Claims`] lists them.
#[test]
fn each_claim_is_written_as_it_s_read() {
    use quick_xml::name::QName;

    let mut names = Vec::new();
    let mut prefixes: Vec<(&str, &str)> = Vec::new();
    for (at, claim) in Claim::ALL.into_iter().enumerate() {
        let (namespace, name) = claim.written();
        let (prefix, local) = name.split_once(':').unwrap();
        let bound = ResolveResult::Bound(Namespace(namespace.as_bytes()));
        let read = Claim::of(bound, QName(local.as_bytes()).local_name());
        assert_eq!(read, Some(claim), "{name}");
        match prefixes.iter().find(|(bound, _)| *bound == namespace) {
            Some((_, was)) => assert_eq!(*was, prefix, "{name}"),
            None => prefixes.push((namespace, prefix)),
        }
        assert!(!names.contains(&name), "{name}");
        names.push(name);
        // Its value is where `Claims` lists it.
        let mut claims = Claims::default();
        claims.set(claim, "x");
        let values = [
            &claims.pdfa_part,
            &claims.pdfa_conformance,
            &claims.pdfa_rev,
            &claims.pdfua_part,
            &claims.pdfua_rev,
            &claims.pdfvt_version,
        ];
        let set: Vec<usize> = (values.iter().enumerate())
            .filter(|(_, value)| value.is_some())
            .map(|(at, _)| at)
            .collect();
        assert_eq!(set, [at], "{name}");
        assert_eq!(claims.value(claim), Some("x"), "{name}");
    }
    assert_eq!(
        prefixes,
        [(PDFA, "pdfaid"), (PDFUA, "pdfuaid"), (PDFVT, "pdfvtid")]
    );
    // A part's error names it as it's written.
    let parts = Claims {
        pdfa_part: some("x"),
        ..Claims::default()
    };
    assert!(matches!(
        parts.conformance(),
        Err(ClaimError::Part { claim, .. }) if claim == Claim::APart.written().1
    ));
    let parts = Claims {
        pdfua_part: some("x"),
        ..Claims::default()
    };
    assert!(matches!(
        parts.conformance(),
        Err(ClaimError::Part { claim, .. }) if claim == Claim::UaPart.written().1
    ));
}

/// Every claim, as [`write`]'s tests give them.
fn every_claim() -> Claims {
    Claims {
        pdfa_part: some("3"),
        pdfa_conformance: some("B"),
        pdfa_rev: some("2012"),
        pdfua_part: some("1"),
        pdfua_rev: some("2014"),
        pdfvt_version: some("PDF/VT-2"),
    }
}

/// `claims` written, and read back with nothing told: the packet.
fn written_back(claims: &Claims) -> String {
    let packet = write(claims).unwrap_or_else(|refused| panic!("{claims:?}: {refused}"));
    let mut kept = Kept::default();
    let read_back = read(packet.as_bytes(), None, &Limits::DEFAULT, &mut kept);
    assert_eq!(read_back.as_ref(), Ok(claims), "{packet}");
    assert_eq!(kept.faults(), [], "{packet}");
    // And with each empty element given open and closed, as a reader may.
    let mut expanded = Kept::default();
    let read_back = read_as(
        packet.as_bytes(),
        None,
        &Limits::DEFAULT,
        &mut expanded,
        true,
    );
    assert_eq!(read_back.as_ref(), Ok(claims), "{packet}");
    packet
}

/// Each claim alone, every claim together and none: read back as written,
/// each value an element in its namespace, the namespaces of the claims
/// given bound and no other.
#[test]
fn a_packet_written_reads_back_as_its_claims() {
    let every = every_claim();
    let packet = written_back(&every);
    for claim in Claim::ALL {
        let (namespace, name) = claim.written();
        let value = every.value(claim).unwrap();
        assert!(
            packet.contains(&format!("<{name}>{value}</{name}>")),
            "{name}"
        );
        assert!(packet.contains(namespace), "{name}");
        let mut alone = Claims::default();
        alone.set(claim, value);
        let packet = written_back(&alone);
        assert_eq!(packet.matches("xmlns:").count(), 3, "{packet}");
        for other in [PDFA, PDFUA, PDFVT] {
            assert_eq!(packet.contains(other), other == namespace, "{packet}");
        }
        // An empty value, written open and closed.
        let mut empty = Claims::default();
        empty.set(claim, "");
        let packet = written_back(&empty);
        assert!(packet.contains(&format!("<{name}></{name}>")), "{packet}");
    }
    assert_eq!(packet.matches("xmlns:").count(), 5, "{packet}");
    let none = written_back(&Claims::default());
    assert_eq!(none.matches("xmlns:").count(), 2, "{none}");
    // Every claim empty.
    let mut empty = Claims::default();
    for claim in Claim::ALL {
        empty.set(claim, "");
    }
    written_back(&empty);
}

/// A value's text reads back as it's given: what's markup escaped, a
/// carriage return by a reference, the rest as it is.
#[test]
fn a_value_written_reads_back_as_its_text() {
    let values = [
        "a&b",
        "&amp;",
        "<pdfaid:part>9</pdfaid:part>",
        "a<b",
        "a>b",
        "]]>",
        "<![CDATA[3]]>",
        "<!-- 3 -->",
        "a\rb",
        "a\r\nb",
        "a\n\rb",
        "a\nb",
        "a\tb",
        "\"quoted\" 'and'",
        "&#51;",
        "é, \u{a0}, \u{85}, \u{2028}, \u{fffd}, \u{10000}, \u{10ffff}",
        "\u{a0}3\u{a0}",
        "PDF/VT-2",
    ];
    for value in values {
        for claim in Claim::ALL {
            let mut claims = Claims::default();
            claims.set(claim, value);
            assert_eq!(claims.value(claim), Some(value));
            let packet = written_back(&claims);
            // Text XML 1.0 holds never has `]]>` in it.
            assert!(!packet.contains("]]>"), "{packet}");
        }
    }
    // Each in the packet as it's escaped.
    let mut claims = Claims::default();
    claims.set(Claim::APart, "a&b<c>d\re\r\nf\tg");
    let packet = written_back(&claims);
    assert!(
        packet.contains("<pdfaid:part>a&amp;b&lt;c&gt;d&#13;e&#13;\nf\tg</pdfaid:part>"),
        "{packet}"
    );
}

/// One packet's bytes, as written.
#[test]
fn a_packet_written_is_xmp_s_wrapper_round_one_description() {
    let claims = Claims {
        pdfa_part: some("3"),
        pdfa_conformance: some("B"),
        pdfua_part: some("1"),
        ..Claims::default()
    };
    let packet = written_back(&claims);
    let expected = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
        <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
        <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
        <rdf:Description rdf:about=\"\"\n    \
        xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"\n    \
        xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\">\n   \
        <pdfaid:part>3</pdfaid:part>\n   \
        <pdfaid:conformance>B</pdfaid:conformance>\n   \
        <pdfuaid:part>1</pdfuaid:part>\n  \
        </rdf:Description>\n \
        </rdf:RDF>\n\
        </x:xmpmeta>\n\
        <?xpacket end=\"w\"?>";
    assert_eq!(packet, expected);
    // The begin attribute's value is the mark's character, in UTF-8.
    assert!(
        packet
            .as_bytes()
            .starts_with(b"<?xpacket begin=\"\xEF\xBB\xBF\"")
    );
    let none = written_back(&Claims::default());
    assert!(
        none.contains("<rdf:Description rdf:about=\"\">\n  </rdf:Description>"),
        "{none}"
    );
}

/// A value that wouldn't read back as it is, the first in `Claims`' order,
/// is refused, and nothing's written: white space round it, which reading
/// trims, or a character XML can't hold.
#[test]
fn a_value_that_wouldn_t_read_back_is_refused() {
    // Set as given, not trimmed as a read sets it.
    let refused = |claim: Claim, value: &str| {
        let mut claims = Claims::default();
        *claims.slot(claim) = Some(value.to_string());
        write(&claims).unwrap_err()
    };
    for claim in Claim::ALL {
        let name = claim.written().1;
        for value in [
            " 3", "3 ", "\t3", "3\n", "\r3", "3\r", "\u{c}3", "3\u{c}", " ", "\n",
        ] {
            assert_eq!(
                refused(claim, value),
                WriteError::Spaced { claim: name },
                "{value:?}"
            );
        }
        for (value, at, character) in [
            ("\u{0}", 0, '\u{0}'),
            ("3\u{1}", 1, '\u{1}'),
            ("a\u{8}b", 1, '\u{8}'),
            ("a\u{b}b", 1, '\u{b}'),
            ("a\u{c}b", 1, '\u{c}'),
            ("a\u{e}b", 1, '\u{e}'),
            ("é\u{1f}", 2, '\u{1f}'),
            ("a\u{fffe}", 1, '\u{fffe}'),
            ("a\u{ffff}b\u{1}", 1, '\u{ffff}'),
        ] {
            assert_eq!(
                refused(claim, value),
                WriteError::Character {
                    claim: name,
                    at,
                    character
                },
                "{value:?}"
            );
        }
    }
    // The first refused, as `Claims` lists them.
    let mut claims = every_claim();
    claims.pdfua_rev = some("\u{1}");
    claims.pdfa_rev = some("2012 ");
    claims.pdfvt_version = some(" PDF/VT-2");
    assert_eq!(
        write(&claims),
        Err(WriteError::Spaced {
            claim: "pdfaid:rev"
        })
    );
    claims.pdfa_rev = some("2012");
    assert_eq!(
        write(&claims),
        Err(WriteError::Character {
            claim: "pdfuaid:rev",
            at: 0,
            character: '\u{1}'
        })
    );
    // Each said in its words.
    assert_eq!(
        refused(Claim::VtVersion, " a").to_string(),
        "pdfvtid:GTS_PDFVTVersion's value has white space round it, which reading it back trims"
    );
    assert_eq!(
        refused(Claim::AConformance, "b\u{fffe}").to_string(),
        "pdfaid:conformance's value holds U+FFFE at byte 1, which XML can't hold"
    );
    // White space within, and every other character XML has, is written.
    let claims = Claims {
        pdfa_part: some("3\t\r\n4"),
        pdfa_conformance: some("\u{d7ff}\u{e000}\u{fffd}\u{10000}\u{10ffff}\u{20}x"),
        ..Claims::default()
    };
    written_back(&claims);
    // A value read that XML can't hold, by a reference, isn't written.
    let read = part(&format!("<d {A}><pdfaid:part>&#1;</pdfaid:part></d>"));
    assert_eq!(read, some("\u{1}"));
    assert!(matches!(
        write(&claims_with_part("\u{1}")),
        Err(WriteError::Character { at: 0, .. })
    ));
}

/// A metadata stream's dictionary.
fn metadata() -> Dict {
    dict! { "Type" => "Metadata", "Subtype" => "XML" }
}

/// A packet claiming PDF/A-2b, an attribute of which doesn't read, so that
/// it tells a fault with its place.
fn told_packet() -> String {
    packet(&format!(
        "<rdf:Description rdf:about=\"\" {A} pdfaid:part=\"2\" oops=1>\
         <pdfaid:conformance>B</pdfaid:conformance></rdf:Description>"
    ))
}

/// A file of `objects`, its catalog object 1, written with a table.
fn file_of(objects: &[(u32, Body)]) -> Vec<u8> {
    fixture::file("1.7", objects, root(), Xref::Table)
}

/// A trailer whose `/Root` is object 1.
fn root() -> Dict {
    dict! { "Root" => Object::reference(1) }
}

/// A catalog whose `/Metadata` is `metadata`.
fn catalog_with(metadata: Object) -> Body {
    Body::from(dict! { "Type" => "Catalog", "Metadata" => metadata })
}

/// The objects of a file whose catalog's `/Metadata` is stream 4, `xml`,
/// deflated where `deflate` says.
fn with_packet(xml: &[u8], deflate: bool) -> Vec<(u32, Body)> {
    let (mut dict, data) = (metadata(), xml.to_vec());
    let data = match deflate {
        true => {
            dict.set("Filter", "FlateDecode");
            fixture::zlib(&data)
        }
        false => data,
    };
    vec![
        (1, catalog_with(Object::reference(4))),
        (4, Body::Stream(dict, data)),
    ]
}

/// What [`read_from`] gave, and what it told.
type ReadFrom = (Result<Option<Claims>, Error>, Vec<(Place, Fault)>);

/// What [`read_from`] reads through `objects` from the catalog the
/// trailer leads to, and what it told; the same with `()`.
fn read_through<'o, O: Resolve<'o> + Streams + Copy>(
    objects: O,
    trailer: &'o Dict,
    limits: &Limits,
) -> ReadFrom {
    let catalog = catalog(objects, trailer).unwrap().unwrap();
    let mut kept = Kept::default();
    let read = read_from(objects, catalog, limits, &mut kept);
    assert_eq!(read_from(objects, catalog, limits, &mut ()), read);
    (read, kept.0)
}

/// What [`read_from`] reads of `file`, opened by `passwords`, through a
/// reading in memory, one by position, and the document read beside its
/// reading, which all say the same.
fn read_each(file: &[u8], passwords: &[Password], limits: &Limits) -> ReadFrom {
    let mut options = Options::DEFAULT;
    options.passwords = passwords;
    let reading = Reading::open(file, &options).unwrap();
    let in_memory = read_through(&reading, reading.trailer().dict(), limits);
    let source = ByPosition::new(file);
    let positioned = Reading::open(&source, &options).unwrap();
    let by_position = read_through(&positioned, positioned.trailer().dict(), limits);
    assert_eq!(by_position, in_memory, "in memory and by position");
    let beside = WithReading::new(file, &options).unwrap();
    let document = beside.document();
    let with_reading = read_through(&beside, document.trailer().dict(), limits);
    assert_eq!(with_reading, in_memory, "a reading and its document");
    in_memory
}

/// A document in memory of `objects`, each stream's data held as bytes.
fn document_of(objects: Vec<(u32, Body)>) -> Document {
    let mut document = Document::new(Version::new(1, 7), root());
    for (n, body) in objects {
        match body {
            Body::Object(object) => document.insert(n, object),
            Body::Stream(dict, data) => document.insert(n, Stream::new(dict, data)),
        }
    }
    document
}

/// A file's packet is read through what gives its objects and its
/// streams' data, plain or deflated, each fault placed in its stream, as
/// [`read`] reads the stream's data: over a reading in memory, by position
/// and a document read beside its reading, and over a document holding the
/// data.
#[test]
fn a_file_s_packet_is_read_through_its_streams() {
    let xml = told_packet();
    let claims = Claims {
        pdfa_part: some("2"),
        pdfa_conformance: some("B"),
        ..Claims::default()
    };
    let stream = ObjectId::new(4, 0);
    let mut kept = Kept::default();
    let read_alone = read(xml.as_bytes(), Some(stream), &Limits::DEFAULT, &mut kept);
    assert_eq!(read_alone, Ok(claims.clone()));
    let told = kept.0;
    assert_eq!(told.len(), 1);
    let (place, fault) = told[0];
    assert_eq!((place.object, fault), (Some(stream), Fault::XmpAttribute));
    assert_eq!(place.at.and_then(|at| at.stream), Some(stream));
    for deflate in [false, true] {
        let objects = with_packet(xml.as_bytes(), deflate);
        let (read, faults) = read_each(&file_of(&objects), &[], &Limits::DEFAULT);
        assert_eq!(read, Ok(Some(claims.clone())), "deflated: {deflate}");
        assert_eq!(faults, told, "deflated: {deflate}");
        let document = document_of(objects);
        let held = read_through(&document, document.trailer().dict(), &Limits::DEFAULT);
        assert_eq!(held, (Ok(Some(claims.clone())), told.clone()));
    }

    // A document read whole alone holds no stream's data: it says so.
    let file = file_of(&with_packet(xml.as_bytes(), false));
    let alone = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let alone = alone.to_document().unwrap();
    let (read, faults) = read_through(&alone, alone.trailer().dict(), &Limits::DEFAULT);
    let error = read.unwrap_err();
    assert!(
        matches!(&error.why, XmpWhy::Unreadable(unread) if unread.why == Why::Detached && unread.id == stream),
        "{error:?}"
    );
    assert_eq!(
        (error.at, error.read, faults),
        (0, Claims::default(), vec![])
    );
}

/// The way to the stream as [`metadata_stream`] finds it: by a chain, its
/// faults placed in the stream it comes to; none where there's no
/// `/Metadata`, or it's a reference to nothing or leads to no stream; and
/// what doesn't read on the way said, nothing read.
#[test]
fn the_way_to_a_file_s_packet_is_metadata_stream_s() {
    let xml = told_packet();
    let chain = [
        (1, catalog_with(Object::reference(4))),
        (4, Body::from(Object::reference(5))),
        (5, Body::Stream(metadata(), xml.clone().into_bytes())),
    ];
    let (read, faults) = read_each(&file_of(&chain), &[], &Limits::DEFAULT);
    assert_eq!(read.unwrap().unwrap().pdfa_part, some("2"));
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].0.object, Some(ObjectId::new(5, 0)));
    let nothing = |metadata: Option<Object>| {
        let catalog = match metadata {
            Some(metadata) => catalog_with(metadata),
            None => Body::from(dict! { "Type" => "Catalog" }),
        };
        let objects = [
            (1, catalog),
            (5, Body::from(dict! { "Type" => "Metadata" })),
        ];
        read_each(&file_of(&objects), &[], &Limits::DEFAULT)
    };
    for metadata in [
        None,
        Some(Object::reference(9)),
        Some(Object::reference(5)),
        Some(Object::from(3)),
        Some(Object::null()),
    ] {
        assert_eq!(
            nothing(metadata.clone()),
            (Ok(None), vec![]),
            "{metadata:?}"
        );
    }

    // A loop of references, past the hops followed.
    let looped = [
        (1, catalog_with(Object::reference(4))),
        (4, Body::from(Object::reference(4))),
    ];
    let (read, faults) = read_each(&file_of(&looped), &[], &Limits::DEFAULT);
    let error = read.unwrap_err();
    assert!(matches!(&error.why, XmpWhy::Unreadable(unread) if unread.why == Why::Capped));
    assert_eq!(
        (error.at, &error.read, faults),
        (0, &Claims::default(), vec![])
    );
    assert!(
        (error.to_string()).starts_with("XMP metadata reads no further than byte 0: 4 0 R is past"),
        "{error}"
    );
}

/// An encrypted file's packet is read decrypted where a password opened
/// it, and locked where none did; under `/EncryptMetadata false` it's read
/// as written either way.
#[test]
fn an_encrypted_file_s_packet_is_read_as_its_reader_gives_it() {
    let xml = told_packet();
    let objects = with_packet(xml.as_bytes(), false);
    let user = [Password("user")];
    for encrypt_metadata in [true, false] {
        let settings = Settings {
            kind: Kind::Aes128,
            user: b"user",
            owner: b"owner",
            p: -4,
            id: b"0123456789abcdef",
            encrypt_metadata,
        };
        let file = fixture::encrypted(&settings, &objects, root());
        let (read, faults) = read_each(&file, &user, &Limits::DEFAULT);
        assert_eq!(read.unwrap().unwrap().pdfa_part, some("2"));
        assert_eq!(faults.len(), 1);
        let (read, faults) = read_each(&file, &[], &Limits::DEFAULT);
        match encrypt_metadata {
            false => {
                assert_eq!(read.unwrap().unwrap().pdfa_part, some("2"));
                assert_eq!(faults.len(), 1);
            }
            true => {
                let error = read.unwrap_err();
                assert!(
                    matches!(&error.why, XmpWhy::Unreadable(unread) if unread.why == Why::Locked && unread.id == ObjectId::new(4, 0)),
                    "{error:?}"
                );
                assert_eq!(
                    (error.at, error.read, faults),
                    (0, Claims::default(), vec![])
                );
            }
        }
    }
}

/// The data is asked for under the cap, so a packet as long as the cap is
/// read and one longer has nothing read, through any reader; and data
/// longer than was asked for is past the cap too, whatever gave it.
#[test]
fn a_packet_past_the_cap_has_nothing_read_through_its_streams() {
    let xml = told_packet();
    let len = xml.len();
    let objects = with_packet(xml.as_bytes(), true);
    let file = file_of(&objects);
    let document = document_of(objects);
    let mut limits = Limits::DEFAULT;
    let too_long = Error {
        read: Claims::default(),
        at: 0,
        why: XmpWhy::TooLong,
    };
    for (cap, past) in [(len, false), (len - 1, true), (0, true)] {
        limits.xmp_bytes = cap;
        let read = read_each(&file, &[], &limits);
        let held = read_through(&document, document.trailer().dict(), &limits);
        assert_eq!(held, read, "{cap}");
        match past {
            true => assert_eq!(read, (Err(too_long.clone()), vec![]), "{cap}"),
            false => assert_eq!(read.0.unwrap().unwrap().pdfa_part, some("2"), "{cap}"),
        }
    }
    // `read`, given the bytes, reads as far as the cap.
    limits.xmp_bytes = len - 1;
    let cut = read(xml.as_bytes(), None, &limits, &mut ()).unwrap_err();
    assert_eq!(
        (cut.at, cut.why, cut.read.pdfa_part),
        (len - 1, XmpWhy::TooLong, some("2"))
    );
    assert_eq!(
        too_long.to_string(),
        "XMP metadata reads no further than byte 0: it's longer than is read"
    );
}

/// Over a resolver and a function as one: the data asked for under the
/// cap; what the function gives read, more than it was asked for being
/// past the cap, and why it gives none said as it says it, past the limit
/// being past the cap; what the resolver can't give on the way, as it says.
#[test]
fn a_packet_is_read_through_any_resolver_and_its_streams() {
    let xml = told_packet();
    let stream = Object::stream(metadata());
    let objects = |id: ObjectId| (id == ObjectId::new(4, 0)).then_some(&stream);
    let catalog = dict! { "Type" => "Catalog", "Metadata" => Object::reference(4) };
    let len = xml.len();
    let mut limits = Limits::DEFAULT;
    let through = |given: &dyn Fn(ObjectId, usize) -> Result<Vec<u8>, Unreadable>,
                   limits: &Limits| {
        let with = WithStreams::new(objects, |id: ObjectId, limit: usize| {
            assert_eq!(limit, limits.xmp_bytes, "asked for under the cap");
            given(id, limit)
        });
        let mut kept = Kept::default();
        let read = read_from(with, &catalog, limits, &mut kept);
        (read, kept.0)
    };
    let all = |_: ObjectId, _: usize| Ok(xml.clone().into_bytes());
    let within = |id: ObjectId, limit: usize| match len > limit {
        true => Err(Unreadable::new(id, Why::Limit, "past the limit")),
        false => Ok(xml.clone().into_bytes()),
    };
    let mut kept = Kept::default();
    let read_alone = read(
        xml.as_bytes(),
        Some(ObjectId::new(4, 0)),
        &limits,
        &mut kept,
    );
    for given in [&all as &dyn Fn(_, _) -> _, &within] {
        assert_eq!(
            through(given, &limits),
            (read_alone.clone().map(Some), kept.0.clone())
        );
    }
    limits.xmp_bytes = len - 1;
    let too_long = Error {
        read: Claims::default(),
        at: 0,
        why: XmpWhy::TooLong,
    };
    for given in [&all as &dyn Fn(_, _) -> _, &within] {
        assert_eq!(through(given, &limits), (Err(too_long.clone()), vec![]));
    }
    limits = Limits::DEFAULT;
    for why in [
        Why::Damaged,
        Why::Capped,
        Why::Locked,
        Why::Filter,
        Why::Source,
        Why::Detached,
    ] {
        let unread = Unreadable::new(ObjectId::new(4, 0), why, "it doesn't read");
        let failing = |_: ObjectId, _: usize| Err(unread.clone());
        let (read, faults) = through(&failing, &limits);
        let error = read.unwrap_err();
        assert_eq!(error.why, XmpWhy::Unreadable(unread.clone()), "{why:?}");
        assert_eq!(
            (error.at, &error.read, faults),
            (0, &Claims::default(), vec![])
        );
        assert_eq!(
            error.to_string(),
            "XMP metadata reads no further than byte 0: it doesn't read"
        );
    }

    // A resolver that says why it gives no object, on the way.
    let locked = |id: ObjectId| -> Result<Option<&Object>, Unreadable> {
        match id.number {
            4 => Err(Unreadable::new(id, Why::Locked, "it takes a password")),
            _ => Ok(None),
        }
    };
    let failing = Failing(locked);
    let with = WithStreams::new(&failing, all);
    let error = read_from(with, &catalog, &limits, &mut ()).unwrap_err();
    assert!(matches!(&error.why, XmpWhy::Unreadable(unread) if unread.why == Why::Locked));
    // No `/Metadata`, or one to nothing: none, and nothing asked.
    let asked = |_: ObjectId, _: usize| -> Result<Vec<u8>, Unreadable> { panic!("asked") };
    let catalogs = [
        dict! { "Type" => "Catalog" },
        dict! { "Metadata" => Object::reference(9) },
    ];
    for catalog in &catalogs {
        let with = WithStreams::new(objects, asked);
        assert_eq!(read_from(with, catalog, &limits, &mut ()), Ok(None));
    }
}
