//! What [`check`](crate::check()) judges, read from a [`Reading`]: its
//! sections, its first object, its hint stream, its pages, and the graph of its
//! objects.

use std::collections::HashSet;

use pdf_objects::object::{Dict, Object, ObjectId, Value};
use pdf_semantics::pages::pages;
use pdf_semantics::{Limits as TreeLimits, Rules, catalog};
use pdf_structure::read::{
    self, ByteSource, Reading, SectionRow, SectionRows, WithReading, XrefEntry,
};
use pdf_structure::{Document, Encryption, XrefKind};

use super::graph::Prepared;
use super::{Error, FirstPage, Options};
use crate::Limits;
use crate::check::{
    self, Attribute, Extent, Facts, FirstObject, HintData, HintFacts, PageFacts, Raw, RowEntry,
    RowFacts, SectionFacts, Sink, Verdict,
};
use crate::plan::Kind;

/// `value` as a check takes it: an array as a whole, unless it has a reference
/// in it, which is what a reader can't read without the file's sections.
fn raw(value: Option<&Object>) -> Raw {
    let Some(value) = value else {
        return Raw::Absent;
    };
    match value.value() {
        Value::Integer(n) => Raw::Integer(*n),
        Value::Real(n) => Raw::Real(*n),
        Value::Ref(id) => Raw::Reference(*id),
        _ => match (value.references().next(), value.value()) {
            (Some(id), _) => Raw::Reference(id),
            (None, Value::Array(_)) => Raw::Array,
            _ => Raw::Other,
        },
    }
}

/// The dictionary's keys, `/H` being an array whatever's in it.
fn first_object(read: read::FirstObject) -> FirstObject {
    let (at, end) = (read.at as u64, read.end as u64);
    let mut first = FirstObject::new(at, end, read.id.number, read.id.generation);
    first.stream = matches!(read.object.value(), Value::Stream(_));
    let Some(dict) = read.object.as_dict() else {
        return first;
    };
    first.linearized = raw(dict.get(b"Linearized"));
    first.l = raw(dict.get(b"L"));
    first.o = raw(dict.get(b"O"));
    first.e = raw(dict.get(b"E"));
    first.n = raw(dict.get(b"N"));
    first.t = raw(dict.get(b"T"));
    first.p = raw(dict.get(b"P"));
    match dict.get(b"H").map(Object::value) {
        Some(Value::Array(items)) => {
            first.h = Raw::Array;
            first.h_items = items.iter().map(|item| raw(Some(item))).collect();
        }
        _ => first.h = raw(dict.get(b"H")),
    }
    first
}

fn row(row: &SectionRow) -> RowFacts {
    let entry = match row.entry {
        Some(XrefEntry::InUse { offset, .. }) => RowEntry::InUse(offset as u64),
        Some(XrefEntry::Compressed { stream, index }) => RowEntry::Compressed { stream, index },
        Some(_) => RowEntry::Free,
        None => RowEntry::Null,
    };
    RowFacts::new(row.number, row.generation, entry)
}

fn section(read: &SectionRows) -> SectionFacts {
    let kind = match read.kind {
        XrefKind::Table => Kind::Table,
        _ => Kind::Stream,
    };
    let mut section = SectionFacts::new(read.at as u64, kind);
    section.subsections = read.subsections.clone();
    section.rows = read.rows.iter().map(row).collect();
    section.first_row = (read.first_row).map(|(space, first)| (space as u64, first as u64));
    section.stream = read.stream.map(|id| id.number);
    let dict = read.trailer.dict();
    section.size = raw(dict.get(b"Size"));
    section.prev = raw(dict.get(b"Prev"));
    section.root = raw(dict.get(b"Root"));
    section.startxref = read.startxref.map(|at| at as u64);
    if let Some(hybrid) = &read.hybrid {
        section.hybrid = hybrid.rows.iter().map(row).collect();
    }
    section
}

/// Where the row `at` in any of `sections` is for, and its generation.
fn listed_at(sections: &[SectionRows], at: u64) -> Option<ObjectId> {
    let rows = sections.iter().flat_map(|section| {
        let hybrid = section.hybrid.iter().flat_map(|hybrid| &hybrid.rows);
        section.rows.iter().chain(hybrid)
    });
    rows.into_iter().find_map(|row| match row.entry {
        Some(XrefEntry::InUse { offset, .. }) if offset as u64 == at => {
            Some(ObjectId::new(row.number, row.generation))
        }
        _ => None,
    })
}

/// Where object `id` ends, past its `endobj`.
fn end_of<S: ByteSource>(reading: &Reading<S>, id: ObjectId) -> Result<Option<u64>, Error> {
    match reading.get(id) {
        Ok(_) => Ok(reading.record(id).map(|record| record.at.end as u64)),
        Err(read::Error::Io(why)) => Err(Error::Read(read::Error::Io(why))),
        Err(_) => Ok(None),
    }
}

/// The primary hint stream, by what's listed at `/H`'s first offset, which
/// counts from the header (ISO 32000-2, 7.5.2), so is `header` bytes into the
/// file past where it says, as the rows' offsets are.
fn hint<S: ByteSource>(
    reading: &Reading<S>,
    items: &[Raw],
    sections: &[SectionRows],
    header: u64,
) -> Result<Option<HintFacts>, Error> {
    let offset = |raw: Option<&Raw>| match raw {
        Some(&Raw::Integer(at)) => u64::try_from(at).ok().map(|at| at.saturating_add(header)),
        _ => None,
    };
    let Some(at) = offset(items.first()) else {
        return Ok(None);
    };
    let Some(id) = listed_at(sections, at) else {
        return Ok(None);
    };
    let Some(mut end) = end_of(reading, id)? else {
        let data = HintData::NotAStream;
        return Ok(Some(HintFacts::new(at, at, id.number, data)));
    };
    let primary = reading.get(id).map_err(Error::Read)?;
    let Value::Stream(stream) = primary.value() else {
        let data = HintData::NotAStream;
        return Ok(Some(HintFacts::new(at, end, id.number, data)));
    };
    // An indirect /Length follows the stream, and /H's length covers it (Table
    // F.1).
    if let Some(length) = stream.dict.get(b"Length").and_then(Object::as_reference)
        && let Some(after) = end_of(reading, length)?
    {
        end = end.max(after);
    }
    let mut data = decoded(reading, id)?;
    let mut overflow = None;
    if let Some(second) = offset(items.get(2))
        && let Some(id) = listed_at(sections, second)
    {
        let more = decoded(reading, id)?;
        data = match (data, more) {
            (HintData::Decoded(mut data), HintData::Decoded(more)) => {
                data.extend_from_slice(&more);
                HintData::Decoded(data)
            }
            (HintData::Decoded(_), more) => more,
            (data, _) => data,
        };
        overflow = end_of(reading, id)?.map(|end| (second, end));
    }
    let mut hint = HintFacts::new(at, end, id.number, data);
    hint.overflow = overflow;
    hint.keys = (stream.dict.entries().iter())
        .map(|entry| (entry.key.to_vec(), raw(Some(&entry.value))))
        .collect();
    Ok(Some(hint))
}

/// Stream `id`'s data, as far as it decodes, as any reader would take it.
fn decoded<S: ByteSource>(reading: &Reading<S>, id: ObjectId) -> Result<HintData, Error> {
    Ok(match reading.decoded_within(id, reading.limits().stream) {
        Ok(data) => HintData::Decoded(data),
        Err(read::Error::Io(why)) => return Err(Error::Read(read::Error::Io(why))),
        Err(read::Error::NotAStream { .. }) => HintData::NotAStream,
        Err(read::Error::Locked | read::Error::Encrypted | read::Error::Unsupported) => {
            HintData::Locked
        }
        Err(read::Error::Decode(error)) if !error.data.is_empty() => HintData::Decoded(error.data),
        Err(_) => HintData::Undecodable,
    })
}

/// The inheritable attributes a node may hold, with their keys.
const INHERITED: [(&[u8], Attribute); 4] = [
    (b"Resources", Attribute::Resources),
    (b"MediaBox", Attribute::MediaBox),
    (b"CropBox", Attribute::CropBox),
    (b"Rotate", Attribute::Rotate),
];

/// The pages by ISO's rules, and what the nodes on their way hold.
fn page_tree<S: ByteSource>(
    reading: &Reading<S>,
    limits: &Limits,
) -> (Vec<PageFacts>, Vec<(u32, Attribute)>) {
    let mut tree_limits = TreeLimits::DEFAULT;
    tree_limits.pages = tree_limits.pages.max(limits.pages);
    let trailer = reading.trailer().dict();
    let Ok(Some(root)) = catalog(reading, trailer) else {
        return Default::default();
    };
    let Ok(table) = pages(reading, root, Rules::Iso, &tree_limits, &mut ()) else {
        return Default::default();
    };
    let own = |dict: Option<&Dict>, key: &[u8]| dict.is_some_and(|dict| dict.has(key));
    let listed = table.iter().map(|page| {
        let mut facts = PageFacts::new(page.id);
        facts.resources = own(page.dict, b"Resources");
        facts.media_box = own(page.dict, b"MediaBox");
        facts
    });
    let listed = listed.collect();
    let page_ids: HashSet<ObjectId> = table.iter().filter_map(|page| page.id).collect();
    let mut seen = HashSet::new();
    let mut nodes = Vec::new();
    for page in table.iter() {
        for parent in page.parents() {
            if !seen.insert(parent) {
                break;
            }
            let dict = reading.get(parent).ok().and_then(Object::as_dict);
            // What a page's /Parent names is a node only if it isn't a page,
            // and holds kids or says so, as the writer has it: a page rightly
            // holds these, and a broken file's /Parent can name its /Resources.
            let node = |dict: &&Dict| dict.has(b"Kids") || dict.has_type(b"Pages");
            let Some(dict) = dict.filter(node).filter(|_| !page_ids.contains(&parent)) else {
                continue;
            };
            let held = INHERITED.iter().filter(|(key, _)| dict.has(key));
            nodes.extend(held.map(|&(_, attribute)| (parent.number, attribute)));
        }
    }
    (listed, nodes)
}

/// The facts of the file `reading` reads, for [`check`](crate::check()), under
/// `limits`, kept for nobody and read by whoever asks: its first object, each
/// section's rows, its hint stream's data, decrypted where it's encrypted and a
/// password opened it, its page tree and the graph of its objects, by a whole
/// read where the file says it's linearized. A file that doesn't, or whose `/L`
/// isn't its length, has no more read than it takes to say so. It fails only
/// when the source doesn't give its bytes ([`Error::Read`]): what's wrong with
/// a file is a fact. A caller that holds the file read whole beside its reading
/// gives that ([`facts_with_reading`]), so it isn't read whole again.
pub fn facts<S: ByteSource>(reading: &Reading<S>, limits: &Limits) -> Result<Facts, Error> {
    facts_of(reading, None, limits)
}

/// The facts of the file `read` holds, as [`facts`] reads them of its reading
/// ([`WithReading::reading`]), but the graph of its objects, which is made of
/// the document read whole beside it ([`WithReading::document`]) rather than
/// of a whole read made again: the same facts, as that document is the
/// reading's whole read.
pub fn facts_with_reading<S: ByteSource>(
    read: &WithReading<S>,
    limits: &Limits,
) -> Result<Facts, Error> {
    facts_of(read.reading(), Some(read.document()), limits)
}

/// [`facts`], the graph made of `whole` where it's given, `reading`'s whole
/// read, else of a whole read made now.
fn facts_of<S: ByteSource>(
    reading: &Reading<S>,
    whole: Option<&Document>,
    limits: &Limits,
) -> Result<Facts, Error> {
    let mut facts = Facts::new(reading.size() as u64);
    facts.header = reading.facts().header_offset.unwrap_or(0) as u64;
    let first = reading.first_object().map_err(Error::Read)?;
    facts.first_object = first.map(first_object);
    if reading.update().is_some() {
        facts.update = reading.facts().newest.map(|(at, _)| at as u64);
    }
    if !check::check(&facts, limits, &mut ()).linearized {
        return Ok(facts);
    }
    let read = reading.sections().map_err(Error::Read)?.sections;
    let mut listed = read.iter();
    facts.first = listed.next().map(section);
    facts.main = listed.next().map(section);
    let items = facts.first_object.as_ref().map_or(&[][..], |f| &f.h_items);
    facts.hint = hint(reading, items, &read, facts.header)?;
    let (pages, nodes) = page_tree(reading, limits);
    facts.pages = pages;
    facts.nodes = nodes;
    facts.objects = first_group_end(reading, &facts)?;
    facts.graph = graph(reading, whole, &facts, limits)?;
    Ok(facts)
}

/// Where the first group's last object ends, which is what `/E` is held to.
fn first_group_end<S: ByteSource>(
    reading: &Reading<S>,
    facts: &Facts,
) -> Result<Vec<Extent>, Error> {
    let Some(first) = &facts.first else {
        return Ok(Vec::new());
    };
    let hint = facts.hint.as_ref().map(|hint| hint.number);
    let last = (first.rows.iter())
        .filter(|row| Some(row.number) != hint)
        .filter_map(|row| match row.entry {
            RowEntry::InUse(at) => Some((at, row.number, row.generation)),
            _ => None,
        })
        .max();
    let Some((at, number, generation)) = last else {
        return Ok(Vec::new());
    };
    let end = end_of(reading, ObjectId::new(number, generation))?;
    Ok(end
        .map(|end| Extent::new(number, at, end))
        .into_iter()
        .collect())
}

/// The file's objects as a plan takes them, in the numbers they're written at,
/// where they read: none where no password opened it or its page tree can't be
/// laid out, which says nothing of the file as a linearized one. Of `whole`,
/// `reading`'s whole read, where it's given, else of one made now.
fn graph<S: ByteSource>(
    reading: &Reading<S>,
    whole: Option<&Document>,
    facts: &Facts,
    limits: &Limits,
) -> Result<Option<crate::Graph>, Error> {
    match reading.encryption() {
        Encryption::None | Encryption::Opened { .. } => {}
        _ => return Ok(None),
    }
    let made;
    let doc = match whole {
        Some(doc) => doc,
        None => {
            made = reading.to_document().map_err(Error::Read)?;
            &made
        }
    };
    let mut options = Options::DEFAULT;
    options.limits = *limits;
    let first_page = facts
        .first_object
        .as_ref()
        .map_or(0, |first| match first.p {
            Raw::Integer(p) => u32::try_from(p).unwrap_or(0),
            _ => 0,
        });
    options.first_page = FirstPage::Index(first_page);
    Ok((Prepared::of(doc, None, &options).ok())
        .map(|prepared| prepared.graph(Kind::Table, false, &HashSet::new())))
}

/// What [`facts`] reads of `reading`, [`check`](crate::check()) judged: a whole
/// read where the file says it's linearized, so paid only by who asks. It fails
/// only where [`facts`] does.
pub fn check<S: ByteSource, K: Sink + ?Sized>(
    reading: &Reading<S>,
    limits: &Limits,
    sink: &mut K,
) -> Result<Verdict, Error> {
    let facts = facts(reading, limits)?;
    Ok(check::check(&facts, limits, sink))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::io;
    use std::ops::Range;
    use std::sync::atomic::{AtomicBool, Ordering};

    use pdf_structure::read::Options as Open;

    use super::*;

    /// A file of a table whose trailer gives an `/XRefStm`.
    fn hybrid() -> Vec<u8> {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        let stream = file.len();
        let rows = [0, 0, 0, 255, 1, 0, 9, 0, 2, 0, 9, 3];
        let head = format!(
            "2 0 obj\n<< /Type /XRef /Size 3 /W [1 2 1] /Length {} >>\nstream\n",
            rows.len()
        );
        file.extend_from_slice(head.as_bytes());
        file.extend_from_slice(&rows);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let table = file.len();
        file.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n");
        let trailer = format!("trailer\n<< /Size 3 /Root 1 0 R /XRefStm {stream} >>\n");
        file.extend_from_slice(trailer.as_bytes());
        file.extend_from_slice(format!("startxref\n{table}\n%%EOF\n").as_bytes());
        file
    }

    #[test]
    fn a_section_is_as_read() {
        let file = hybrid();
        let reading = Reading::open(&file[..], &Open::DEFAULT).unwrap();
        let read = reading.sections().unwrap().sections;
        let table = section(&read[0]);
        assert_eq!(table.kind, Kind::Table);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.hybrid.len(), 3);
        assert_eq!(
            table.hybrid[2].entry,
            RowEntry::Compressed {
                stream: 9,
                index: 3
            }
        );
        assert_eq!(table.root, Raw::Reference(ObjectId::new(1, 0)));
        assert_eq!(table.size, Raw::Integer(3));
        assert!(table.first_row.is_some());
        assert_eq!(table.stream, None);
        let stream = section(read[0].hybrid.as_ref().unwrap());
        assert_eq!(stream.kind, Kind::Stream);
        assert_eq!(stream.stream, Some(2));
        assert_eq!(stream.rows[0].entry, RowEntry::Free);
    }

    #[test]
    fn a_startxref_after_a_section_is_said() {
        let file = hybrid();
        let reading = Reading::open(&file[..], &Open::DEFAULT).unwrap();
        let read = reading.sections().unwrap().sections;
        assert_eq!(
            section(&read[0]).startxref,
            (read[0].startxref).map(|at| at as u64)
        );
        assert!(section(&read[0]).startxref.is_some());
    }

    /// A stream object and, after it, what it says its length is.
    fn indirect_length() -> (Vec<u8>, u64, u64) {
        let mut file = b"%PDF-1.7\n".to_vec();
        let at = file.len() as u64;
        file.extend_from_slice(
            b"2 0 obj\n<< /S 0 /Length 3 0 R >>\nstream\nab\nendstream\nendobj\n",
        );
        let length = file.len();
        file.extend_from_slice(b"3 0 obj\n2\nendobj\n");
        let end = file.len() as u64 - 1;
        let table = file.len();
        let row = |at: usize| format!("{at:010} 00000 n \n");
        let text = format!(
            "xref\n0 1\n0000000000 65535 f \n2 2\n{}{}trailer\n<< /Size 4 >>\nstartxref\n{table}\n%%EOF\n",
            row(at as usize),
            row(length),
        );
        file.extend_from_slice(text.as_bytes());
        (file, at, end)
    }

    #[test]
    fn a_hint_stream_ends_past_an_indirect_length_that_follows_it() {
        let (file, at, end) = indirect_length();
        let reading = Reading::open(&file[..], &Open::DEFAULT).unwrap();
        let read = reading.sections().unwrap().sections;
        let items = [Raw::Integer(at as i64), Raw::Integer(0)];
        let hint = hint(&reading, &items, &read, 0)
            .unwrap()
            .expect("it's listed");
        assert_eq!(hint.at, at);
        assert_eq!(hint.number, 2);
        assert_eq!(hint.end, end);
        assert_eq!(hint.data, HintData::Decoded(b"ab".to_vec()));
        assert!(
            (hint.keys.iter())
                .any(|(key, raw)| key == b"Length" && *raw == Raw::Reference(ObjectId::new(3, 0)))
        );
    }

    #[test]
    fn an_overflow_stream_goes_after_the_primary() {
        let (file, at, _) = indirect_length();
        let reading = Reading::open(&file[..], &Open::DEFAULT).unwrap();
        let read = reading.sections().unwrap().sections;
        let at = at as i64;
        let items = [
            Raw::Integer(at),
            Raw::Integer(0),
            Raw::Integer(at),
            Raw::Integer(0),
        ];
        let hint = hint(&reading, &items, &read, 0)
            .unwrap()
            .expect("it's listed");
        assert_eq!(hint.data, HintData::Decoded(b"abab".to_vec()));
        assert!(hint.overflow.is_some());
    }

    /// A source that fails once it's told to.
    struct Failing<'a>(&'a [u8], AtomicBool);

    impl ByteSource for Failing<'_> {
        fn len(&self) -> usize {
            self.0.len()
        }

        fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
            match self.1.load(Ordering::Relaxed) {
                true => Err(io::Error::other("gone")),
                false => Ok(Cow::Borrowed(&self.0[range])),
            }
        }
    }

    #[test]
    fn a_source_that_fails_reading_its_data_is_an_error() {
        let (file, _, _) = indirect_length();
        let source = Failing(&file, AtomicBool::new(false));
        let reading = Reading::open(&source, &Open::DEFAULT).unwrap();
        // The stream is read and kept, and only its data is asked for after.
        reading.get(ObjectId::new(2, 0)).unwrap();
        source.1.store(true, Ordering::Relaxed);
        assert!(matches!(
            decoded(&reading, ObjectId::new(2, 0)),
            Err(Error::Read(read::Error::Io(_)))
        ));
    }

    #[test]
    fn what_doesnt_decrypt_or_isnt_a_stream_is_said_of_the_data() {
        let (file, _, _) = indirect_length();
        let reading = Reading::open(&file[..], &Open::DEFAULT).unwrap();
        assert_eq!(
            decoded(&reading, ObjectId::new(3, 0)).unwrap(),
            HintData::NotAStream
        );
        assert_eq!(
            decoded(&reading, ObjectId::new(9, 0)).unwrap(),
            HintData::Undecodable
        );
    }
}
