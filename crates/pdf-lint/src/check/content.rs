//! Content, read (ISO 32000-2, 7.8.2): every content stream the walk
//! reached, as what holds it: a page's `/Contents`, its streams read as one
//! (7.7.3.3, Table 31), a form XObject's or a tiling pattern's own data
//! (8.10, 8.7.3), and each glyph description of a Type 3 font's
//! `/CharProcs` (9.6.4), each alone. What's content is what the walk reached
//! as one of those tables, the first table it reached the object as, so an
//! object the walk reached as another isn't read as content, nor one it
//! never reached.
//!
//! Each unit of content is read once: its streams, in order, the resources
//! its names are looked up in, and where it starts, a glyph at its `d0` or
//! `d1`. A stream two pages share, with the same resources, is read once.
//! The resources are a page's own or inherited `/Resources` (7.7.3.4), a
//! form's or a pattern's own, and a glyph's own, else its font's (7.8.3, as
//! its errata have it).
//!
//! The streams' data comes decoded from the source
//! ([`Source::decoded_within`]): a reader that gives none, as a document
//! alone doesn't, has no content read at all. A file's content is read
//! within a budget of decoded bytes ([`Options::content_budget`]), each
//! stream asked for no more than is left of it, what a stream that doesn't
//! decode cleanly held spent too, so that a bomb cut short spends it as one
//! read whole does; past it, no more is read. Each unit is read as PDFium
//! reads content, through pdf-content-streams, with every place ISO 32000-2
//! reads it otherwise kept as it's read; no rule is held to it here yet.
//!
//! [`Options::content_budget`]: super::Options::content_budget
//! [`Source::decoded_within`]: crate::source::Source::decoded_within

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::ptr;

use pdf_content_streams::fault::{Fault, Sink};
use pdf_content_streams::{CodedEnds, Concat, Join, Limits, ResCx, State};

use super::{Visit, Walk};
use crate::bytes;
use crate::object::{Dict, Object, ObjectId, Value};

/// How many objects are looked up for one color space an inline image is
/// drawn in, and how many subfunctions written in place are read, where
/// pdf-color reads 4,096: the cost of sizing a resources dictionary's
/// spaces is its entries times this, which a file can make its own, so the
/// checker reads with less. A space past it is sized as one that doesn't
/// load, and an unfiltered image drawn in it may end where PDFium's
/// doesn't; no honest space comes near it.
const SPACE_VISITS: usize = 256;

/// What the walk reached that holds content, kept for the content pass
/// ([`Walk::content`]), in the order the walk reached them.
pub(super) struct Holder<'a> {
    /// A page's dictionary, a form's or a pattern's stream, or a Type 3
    /// font's dictionary.
    object: &'a Object,
    kind: Kind,
}

/// What a holder's content is.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// A page's: the streams of its `/Contents`, as one.
    Page,
    /// A form XObject's or a tiling pattern's own data, the stream it is.
    Own(ObjectId),
    /// A Type 3 font's: each glyph description in its `/CharProcs`, alone.
    Glyphs,
}

/// Content to read as one: its streams, in order, and the resources its
/// names are looked up in.
struct Unit<'a> {
    streams: Vec<ObjectId>,
    resources: Option<&'a Object>,
}

/// What tells one unit from another: its streams, in order; its resources,
/// by the value they are, where the file's reader holds it, so that two
/// written alike apart are two; and where it starts.
#[derive(PartialEq, Eq, Hash)]
struct Key {
    streams: Vec<ObjectId>,
    resources: Option<*const Object>,
    start: State,
}

/// A unit's streams as read.
enum Read {
    /// Decoded, and joined.
    Joined(Concat<'static>),
    /// Not read: a stream doesn't decode cleanly, or the unit is past the
    /// budget, or past what content's reading joins.
    Unread,
    /// The source gives no stream's data.
    NoData,
}

/// Each place ISO 32000-2 reads content otherwise, as it's told, kept.
#[derive(Default)]
struct Faults<'c>(Vec<(Range<usize>, Fault<'c>)>);

impl<'c> Sink<'c> for Faults<'c> {
    fn fault(&mut self, at: Range<usize>, fault: Fault<'c>) {
        self.0.push((at, fault));
    }
}

/// What content is read under: pdf-content-streams' caps, an inline
/// image's DCT and CCITTFax data ended by decoding it, as PDFium ends it,
/// and its color space sized within [`SPACE_VISITS`].
fn limits() -> Limits {
    let mut limits = Limits::DEFAULT;
    limits.coded_ends = CodedEnds::Decoded;
    limits.color.space_visits = SPACE_VISITS;
    limits
}

/// What `table` makes `object`, reached as it, as `id`, the object it is
/// when it's indirect: a holder of content, when it's of the kind the table
/// is.
fn kind(table: &str, object: &Object, id: Option<ObjectId>) -> Option<Kind> {
    match (table, object.value()) {
        ("PageObject", Value::Dict(_)) => Some(Kind::Page),
        (
            "XObjectFormType1" | "XObjectFormPrinterMark" | "XObjectFormTrapNet" | "PatternType1",
            Value::Stream(_),
        ) => id.map(Kind::Own),
        ("FontType3", Value::Dict(_)) => Some(Kind::Glyphs),
        _ => None,
    }
}

impl<'a> Walk<'a> {
    /// Keeps `v` for the content pass when it holds content. Not in an
    /// earlier revision's trailer, whose objects are the file's where no
    /// later revision wrote them, as the walk from the trailer reaches them.
    pub(super) fn note_content(&mut self, v: &Visit<'a>) {
        if self.revision.is_some() {
            return;
        }
        if let Some(kind) = kind(&v.table.name, v.object, v.id()) {
            self.holders.push(Holder {
                object: v.object,
                kind,
            });
        }
    }

    /// Reads the content of each holder the walk reached, each unit once,
    /// in the order the walk reached their holders, within `budget` bytes
    /// decoded in all: see the module's documentation.
    pub(super) fn content(&mut self, budget: usize) {
        let holders = std::mem::take(&mut self.holders);
        let (mut units, mut seen) = (Vec::new(), HashSet::new());
        for holder in &holders {
            for (unit, start) in self.units(holder) {
                let key = Key {
                    streams: unit.streams.clone(),
                    resources: unit.resources.map(ptr::from_ref),
                    start,
                };
                if seen.insert(key) {
                    units.push(unit);
                }
            }
        }
        let limits = limits();
        let none = ResCx::none();
        let mut spaces = HashMap::new();
        let mut left = budget;
        for unit in &units {
            let content = match self.read(unit, &limits, &mut left) {
                Read::Joined(content) => content,
                Read::Unread => continue,
                Read::NoData => return,
            };
            // Only an inline image is sized by the resources' color spaces,
            // so they're read only where one may be, once for each.
            let res = match content.bytes().windows(2).any(|w| w == b"BI") {
                false => &none,
                true => {
                    let source = self.source;
                    let objects = move |id| source.get(id).ok();
                    (spaces.entry(unit.resources.map(ptr::from_ref))).or_insert_with(|| {
                        // A closure can't say why an object doesn't read,
                        // so this never fails.
                        ResCx::read(objects, unit.resources, &limits).unwrap_or_default()
                    })
                }
            };
            let mut faults = Faults::default();
            for read in content.operations(res, &limits).with_sink(&mut faults) {
                if read.is_err() {
                    break;
                }
            }
        }
    }

    /// The units of content `holder` holds, each with where it starts: a
    /// page's streams as one, with its resources, its own or inherited; a
    /// form's or a pattern's own data, with its own; each glyph of a Type 3
    /// font alone ([`Walk::glyphs`]). None with no stream.
    fn units(&self, holder: &Holder<'a>) -> Vec<(Unit<'a>, State)> {
        let Some(dict) = holder.object.as_dict() else {
            return Vec::new();
        };
        let units = match holder.kind {
            Kind::Page => {
                let streams = self.contents(dict);
                let resources = self.inheritable(dict, b"Resources");
                vec![(Unit { streams, resources }, State::ContentStream)]
            }
            Kind::Own(id) => {
                let resources = self.resources(dict);
                let streams = vec![id];
                vec![(Unit { streams, resources }, State::ContentStream)]
            }
            Kind::Glyphs => self.glyphs(dict),
        };
        (units.into_iter())
            .filter(|(unit, _)| !unit.streams.is_empty())
            .collect()
    }

    /// Each glyph description in a Type 3 font's `/CharProcs`, in order, a
    /// unit alone that starts at its `d0` or `d1`, with its own resources,
    /// else the font's; an entry that isn't a stream passed over, as what
    /// `/CharProcs` holds is checked by the walk.
    fn glyphs(&self, font: &'a Dict) -> Vec<(Unit<'a>, State)> {
        let own = self.resources(font);
        let procs =
            (font.get(b"CharProcs").map(|o| self.resolve(o))).and_then(|found| {
                match found.ok()?.1.value() {
                    Value::Dict(procs) => Some(procs),
                    _ => None,
                }
            });
        let mut glyphs = Vec::new();
        for (_, glyph) in procs.into_iter().flat_map(Dict::iter) {
            if let Ok((Some(id), glyph)) = self.resolve(glyph)
                && let Value::Stream(stream) = glyph.value()
            {
                let resources = self.resources(&stream.dict).or(own);
                let streams = vec![id];
                glyphs.push((Unit { streams, resources }, State::GlyphStart));
            }
        }
        glyphs
    }

    /// The streams of a page's `/Contents`: one, or each of an array's, a
    /// reference to either followed; an element that isn't a stream passed
    /// over, as what `/Contents` holds is checked by the walk.
    fn contents(&self, page: &'a Dict) -> Vec<ObjectId> {
        let stream = |raw: &'a Object| match self.resolve(raw) {
            Ok((Some(id), o)) if matches!(o.value(), Value::Stream(_)) => Some(id),
            _ => None,
        };
        let Some(contents) = page.get(b"Contents") else {
            return Vec::new();
        };
        if let Some(id) = stream(contents) {
            return vec![id];
        }
        match self.resolve(contents).map(|(_, o)| o.value()) {
            Ok(Value::Array(streams)) => streams.iter().filter_map(stream).collect(),
            _ => Vec::new(),
        }
    }

    /// `dict`'s own `/Resources`, its references followed; a null is none.
    fn resources(&self, dict: &'a Dict) -> Option<&'a Object> {
        let raw = dict.get(b"Resources")?;
        let (_, resources) = self.resolve(raw).ok()?;
        (!resources.is_null()).then_some(resources)
    }

    /// `unit`'s streams decoded and joined, each asked for no more than
    /// what's left of `left`, and what each held spent from it.
    fn read(&self, unit: &Unit<'a>, limits: &Limits, left: &mut usize) -> Read {
        let mut join = Join::new(limits);
        for &id in &unit.streams {
            if *left == 0 {
                return Read::Unread;
            }
            let decoded = match self.source.decoded_within(id, join.room().min(*left)) {
                None => return Read::NoData,
                Some(decoded) => decoded,
            };
            let data = match decoded {
                Ok(data) => data,
                // What decoded before a fault, or up to the limit that cut
                // it off, is spent as if it read.
                Err(bytes::Error::Decode(cut)) => {
                    *left = left.saturating_sub(cut.data.len());
                    return Read::Unread;
                }
                Err(_) => return Read::Unread,
            };
            *left = left.saturating_sub(data.len());
            // Never past the room it was asked within.
            if join.push(&data).is_err() {
                return Read::Unread;
            }
        }
        Read::Joined(join.finish())
    }
}

#[cfg(test)]
mod tests {
    //! Which streams a check asks its source for, and with how much of the
    //! budget left: each it reaches once, whatever holds it.

    use std::cell::RefCell;

    use pdf_structure::fixture;

    use super::super::{CONTENT_BUDGET, Options, check};
    use super::*;
    use crate::bytes::Opened;
    use crate::check::{Hold, Made};
    use crate::source::{Facts, Record, Revision, Source, Trailer, Unread, Warning};
    use crate::version::Version;

    /// A source that answers as the one it wraps, and keeps each stream it's
    /// asked the data of, and within how much.
    struct Counting<'s> {
        source: &'s dyn Source,
        asked: RefCell<Vec<(ObjectId, usize)>>,
    }

    impl Source for Counting<'_> {
        fn ids(&self) -> Vec<ObjectId> {
            self.source.ids()
        }

        fn get(&self, id: ObjectId) -> Result<&Object, Unread> {
            self.source.get(id)
        }

        fn trailer(&self) -> &Trailer {
            self.source.trailer()
        }

        fn header_version(&self) -> Option<Version> {
            self.source.header_version()
        }

        fn warnings(&self) -> &[Warning] {
            self.source.warnings()
        }

        fn size(&self) -> Option<usize> {
            self.source.size()
        }

        fn metadata(&self) -> Option<&[u8]> {
            self.source.metadata()
        }

        fn record(&self, id: ObjectId) -> Option<&Record> {
            self.source.record(id)
        }

        fn decoded(&self, stream: ObjectId) -> Option<&[u8]> {
            self.source.decoded(stream)
        }

        fn revisions(&self) -> &[Revision] {
            self.source.revisions()
        }

        fn facts(&self) -> Option<&Facts> {
            self.source.facts()
        }

        fn in_the_clear(&self, stream: ObjectId) -> bool {
            self.source.in_the_clear(stream)
        }

        fn decoded_within(
            &self,
            id: ObjectId,
            limit: usize,
        ) -> Option<Result<Vec<u8>, bytes::Error>> {
            self.asked.borrow_mut().push((id, limit));
            self.source.decoded_within(id, limit)
        }
    }

    /// A stream's object: its dictionary's `entries` and its `data`, with
    /// the `/Length` it has.
    fn stream(entries: &str, data: &str) -> String {
        format!(
            "<< {entries} /Length {} >>\nstream\n{data}\nendstream",
            data.len()
        )
    }

    /// A file of these objects, each its number and body, the catalog 1
    /// and the page tree 2.
    fn file(objects: &[(u32, String)]) -> Vec<u8> {
        let objects: Vec<(u32, &[u8])> = (objects.iter())
            .map(|(n, body)| (*n, body.as_bytes()))
            .collect();
        fixture::assembled("1.7", &objects, "<< /Size 100 /Root 1 0 R >>")
    }

    /// The catalog, and a page tree of `kids`, with `resources` its pages
    /// inherit.
    fn tree(kids: &str, resources: &str) -> Vec<(u32, String)> {
        let count = kids.split(" R").count() - 1;
        vec![
            (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string()),
            (
                2,
                format!("<< /Type /Pages /Kids [{kids}] /Count {count} {resources} >>"),
            ),
        ]
    }

    /// A page under the tree, with `more` in it.
    fn page(more: &str) -> String {
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {more} >>")
    }

    /// Each stream `file`'s check asks its source for, with how much it may
    /// decode to, holding what `hold` holds, within `budget`.
    fn asked(file: &[u8], hold: &Hold, budget: Option<usize>) -> Vec<(ObjectId, usize)> {
        let opened = Opened::new(file, &[], &bytes::Options::DEFAULT).unwrap();
        let counting = Counting {
            source: &opened,
            asked: RefCell::new(Vec::new()),
        };
        let options = Options {
            content_budget: budget,
            ..Options::default()
        };
        check(&counting, hold, &options);
        counting.asked.into_inner()
    }

    /// The streams of [`asked`], sorted, every object held.
    fn streams(file: &[u8]) -> Vec<u32> {
        let mut asked: Vec<u32> = (asked(file, &Hold::All(Made::Merged), None).into_iter())
            .map(|((n, _), _)| n)
            .collect();
        asked.sort_unstable();
        asked
    }

    /// Every content stream the walk reaches is asked for once: a page's
    /// one; each of an array's, direct or through a reference; one two
    /// pages share with the resources they inherit, once; a form's, a
    /// pattern's, each glyph's; and what isn't content, or isn't reached
    /// as it, never.
    #[test]
    fn each_content_stream_reached_is_asked_for_once() {
        let fonts = "/Font << /T3 30 0 R >>";
        let resources =
            format!("/Resources << /XObject << /Fm 20 0 R >> /Pattern << /P 21 0 R >> {fonts} >>");
        let mut objects = tree("3 0 R 4 0 R 5 0 R 6 0 R", &resources);
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents [11 0 R 12 0 R]")),
            (5, page("/Contents 13 0 R")),
            (6, page("/Contents 10 0 R /Thumb 15 0 R")),
            (10, stream("", "q /Fm Do Q")),
            (11, stream("", "q")),
            (12, stream("", "Q")),
            (13, "[14 0 R 99 0 R 2 0 R]".to_string()),
            (14, stream("", "/P scn")),
            (
                15,
                stream(
                    "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
                    "x",
                ),
            ),
            (
                20,
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 9 9] /Resources << >>",
                    "0 0 m 9 9 l S",
                ),
            ),
            (
                21,
                stream(
                    "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 \
                     /BBox [0 0 9 9] /XStep 9 /YStep 9 /Resources << >>",
                    "0 0 9 9 re f",
                ),
            ),
            (
                30,
                "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 9 9] /FontMatrix [1 0 0 1 0 0] \
                 /CharProcs << /a 31 0 R /b 32 0 R >> /Encoding << /Differences [97 /a /b] >> \
                 /FirstChar 97 /LastChar 98 /Widths [9 9] /Resources << >> >>"
                    .to_string(),
            ),
            (31, stream("", "9 0 d0 0 0 9 9 re f")),
            (32, stream("", "9 0 0 0 9 9 d1 0 0 9 9 re f")),
        ]);
        assert_eq!(streams(&file(&objects)), [10, 11, 12, 14, 20, 21, 31, 32]);
    }

    /// A stream two pages share is read once for each resources it's read
    /// with: once where both inherit the same, twice where one has its own.
    #[test]
    fn a_stream_shared_with_other_resources_is_asked_for_again() {
        let shared = |more: &str| {
            let mut objects = tree("3 0 R 4 0 R", "/Resources << >>");
            objects.extend([
                (3, page("/Contents 10 0 R")),
                (4, page(&format!("/Contents 10 0 R {more}"))),
                (10, stream("", "q Q")),
            ]);
            streams(&file(&objects))
        };
        assert_eq!(shared(""), [10]);
        assert_eq!(shared("/Resources << >>"), [10, 10]);
    }

    /// What isn't held is read too, as what it uses is the file's.
    #[test]
    fn a_held_page_and_an_unheld_one_are_both_read() {
        let mut objects = tree("3 0 R 4 0 R", "");
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents 11 0 R")),
            (10, stream("", "q Q")),
            (11, stream("", "q Q")),
        ]);
        let hold = Hold::Only([((3, 0), Made::Built)].into());
        let mut asked: Vec<ObjectId> = (asked(&file(&objects), &hold, None).into_iter())
            .map(|(id, _)| id)
            .collect();
        asked.sort_unstable();
        assert_eq!(asked, [(10, 0), (11, 0)]);
    }

    /// Three pages of a stream each: 10 bytes, `second`, and 30 bytes.
    fn three_pages(second: String) -> Vec<u8> {
        let mut objects = tree("3 0 R 4 0 R 5 0 R", "");
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents 11 0 R")),
            (5, page("/Contents 12 0 R")),
            (10, stream("", &"q Q ".repeat(3)[..10])),
            (11, second),
            (12, stream("", &"q Q ".repeat(8)[..30])),
        ]);
        file(&objects)
    }

    /// Each stream is asked for within what's left of the budget, and of
    /// what content's reading joins, the less; once none of the budget is
    /// left, no unit after is asked for. A stream cut off by it spends what
    /// it held, as one read whole would; and so does one that doesn't decode
    /// cleanly.
    #[test]
    fn content_is_read_within_its_budget() {
        let file = three_pages(stream("", &"q Q ".repeat(5)));
        let hold = Hold::All(Made::Merged);
        let asked_within = |budget| asked(&file, &hold, budget);
        let room = Limits::DEFAULT.content;
        let each = [((10, 0), room), ((11, 0), room), ((12, 0), room)];
        assert_eq!(asked_within(None), each);
        assert_eq!(asked_within(Some(CONTENT_BUDGET)), each);
        let all = [((10, 0), 100), ((11, 0), 90), ((12, 0), 70)];
        assert_eq!(asked_within(Some(100)), all);
        // Spent whole by the first two.
        assert_eq!(asked_within(Some(30)), [((10, 0), 30), ((11, 0), 20)]);
        // The second cut off, spending what's left.
        assert_eq!(asked_within(Some(25)), [((10, 0), 25), ((11, 0), 15)]);
        assert_eq!(asked_within(Some(10)), [((10, 0), 10)]);
        assert_eq!(asked_within(Some(0)), []);
        // Hex data without its `>`, all of it decoded, and a fault.
        let hex: String = (b"q Q ".repeat(5).iter())
            .map(|b| format!("{b:02x}"))
            .collect();
        let faulty = three_pages(stream("/Filter /ASCIIHexDecode", &hex));
        let opened = Opened::new(&faulty[..], &[], &bytes::Options::DEFAULT).unwrap();
        let Some(Err(bytes::Error::Decode(error))) = opened.decoded_within((11, 0), 100) else {
            panic!("hex data without its end decodes with a fault");
        };
        assert_eq!(error.data.len(), 20);
        assert_eq!(asked(&faulty, &hold, Some(100)), all);
    }

    /// A pass needs a source that gives streams' data: a document alone is
    /// asked for the first stream, gives none, and is asked for no more.
    #[test]
    fn a_document_alone_is_asked_for_one_stream() {
        let doc = bytes::read(&three_pages(stream("", "q Q")));
        let counting = Counting {
            source: &doc,
            asked: RefCell::new(Vec::new()),
        };
        check(&counting, &Hold::All(Made::Merged), &Options::default());
        let room = Limits::DEFAULT.content;
        assert_eq!(counting.asked.into_inner(), [((10, 0), room)]);
    }

    /// Content read finds nothing yet: what a check of a file opened beside
    /// its reading finds is what one of its document alone finds, a stream
    /// that doesn't decode, one cut off and content that doesn't read among
    /// them.
    #[test]
    fn content_read_finds_nothing_yet() {
        let mut objects = tree(
            "3 0 R 4 0 R 5 0 R",
            "/Resources << /ColorSpace << /C 5 >> >>",
        );
        objects.extend([
            (3, page("/Contents [10 0 R 11 0 R]")),
            (4, page("/Contents 12 0 R")),
            (5, page("/Contents 13 0 R")),
            (10, stream("/Filter /FlateDecode", "not zlib")),
            (11, stream("/Filter /JBIG2Decode", "x")),
            (12, stream("", "q (a string that doesn't end")),
            (13, stream("", "BI /W 1 /H 1 /CS /C /BPC 8 ID x EI q q 1 m")),
        ]);
        let file = file(&objects);
        let opened = Opened::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        for hold in [Hold::All(Made::Merged), Hold::All(Made::Built)] {
            let options = Options::default();
            assert_eq!(
                check(&opened, &hold, &options),
                check(&*opened, &hold, &options)
            );
        }
    }
}
