//! The spans the calls that read or write a whole page tree or packet run
//! in, with the `tracing` feature: one macro, so that a phase is a line
//! where it starts, and nothing of it is compiled without the feature. A
//! phase is a call a page tree or a packet is read or written in once: a
//! page tree walked or counted, a packet read, from its bytes or from a
//! file's streams, and a page tree written. A name or number tree's walk
//! has none, as its loop is the caller's, nor has a date, a reference
//! followed, a metadata stream found or a packet written, each a lookup or
//! a value; and no fault is told as an event: faults are the sink's.

/// Enters a span at `$level` (`DEBUG`) named `$name`, held to the end of
/// the block it's in, its fields numbers alone (a length, a count, which
/// rules), each cast to a `u64`, so that no byte of a file or a packet can
/// be one. Without the `tracing` feature it's nothing, its fields not
/// evaluated.
macro_rules! phase {
    ($level:ident, $name:literal $(, $field:ident = $value:expr)* $(,)?) => {
        #[cfg(feature = "tracing")]
        let _phase = ::tracing::span!(
            ::tracing::Level::$level,
            $name
            $(, $field = ($value) as u64)*
        )
        .entered();
    };
}

pub(crate) use phase;

#[cfg(all(test, feature = "tracing"))]
mod tests {
    use std::collections::BTreeMap;
    use std::fmt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, Once};
    use std::thread::{self, ThreadId};

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams, dict};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use crate::date::Date;
    use crate::pages::{self, FANOUT};
    use crate::xmp::{self, Claims};
    use crate::{Limits, catalog, follow, tree};

    /// A span as it was made: its name, its level, and each field's name
    /// and whether its value was a number.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Made {
        name: &'static str,
        level: Level,
        fields: Vec<(&'static str, bool)>,
    }

    /// A subscriber, written over tracing-core's trait, that keeps each span
    /// made on a thread `spans` is running on, and does nothing else. It's
    /// the process's default, set once by the first `spans`.
    struct Recorder {
        ids: AtomicU64,
    }

    /// The spans made so far on each thread `spans` is running on.
    static MADE: Mutex<Vec<(ThreadId, Vec<Made>)>> = Mutex::new(Vec::new());

    /// Each field of a span, and whether its value was a number: any other
    /// kind reaches `record_debug`, as the trait's defaults pass it on.
    struct Fields(Vec<(&'static str, bool)>);

    impl Visit for Fields {
        fn record_u64(&mut self, field: &Field, _: u64) {
            self.0.push((field.name(), true));
        }

        fn record_i64(&mut self, field: &Field, _: i64) {
            self.0.push((field.name(), true));
        }

        fn record_debug(&mut self, field: &Field, _: &dyn fmt::Debug) {
            self.0.push((field.name(), false));
        }
    }

    impl Subscriber for Recorder {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &Attributes<'_>) -> Id {
            let mut fields = Fields(Vec::new());
            span.record(&mut fields);
            let metadata = span.metadata();
            let me = thread::current().id();
            let mut threads = MADE.lock().unwrap();
            if let Some((_, made)) = threads.iter_mut().find(|(thread, _)| *thread == me) {
                made.push(Made {
                    name: metadata.name(),
                    level: *metadata.level(),
                    fields: fields.0,
                });
            }
            Id::from_u64(self.ids.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn record(&self, _: &Id, _: &Record<'_>) {}

        fn record_follows_from(&self, _: &Id, _: &Id) {}

        fn event(&self, _: &Event<'_>) {}

        fn enter(&self, _: &Id) {}

        fn exit(&self, _: &Id) {}
    }

    /// The spans `run` makes on this thread, in the order they're made.
    ///
    /// The recorder is the process's default rather than this thread's
    /// alone: tracing-core caches whether a callsite is wanted when it's
    /// first reached, and while one dispatcher is registered it asks the
    /// default of the thread that reached it, so a recorder set for this
    /// thread with `with_default` misses a span another test reached first
    /// on a thread with none. Every callsite's interest is worked out again
    /// before `run`, for one first reached on another thread while the
    /// default was being set.
    fn spans(run: impl FnOnce()) -> Vec<Made> {
        static SET: Once = Once::new();
        SET.call_once(|| {
            let recorder = Recorder {
                ids: AtomicU64::new(0),
            };
            tracing::subscriber::set_global_default(recorder).unwrap();
        });
        tracing::callsite::rebuild_interest_cache();
        let me = thread::current().id();
        MADE.lock().unwrap().push((me, Vec::new()));
        run();
        let mut threads = MADE.lock().unwrap();
        let mine = threads
            .iter()
            .position(|(thread, _)| *thread == me)
            .unwrap();
        threads.swap_remove(mine).1
    }

    /// Each span's name and level, every one's fields numbers alone.
    fn named(spans: &[Made]) -> Vec<(&'static str, Level)> {
        for made in spans {
            for &(field, number) in &made.fields {
                assert!(number, "{}'s {field} isn't a number", made.name);
            }
        }
        spans.iter().map(|made| (made.name, made.level)).collect()
    }

    /// Each span's fields' names.
    fn fields(made: &Made) -> Vec<&'static str> {
        made.fields.iter().map(|(field, _)| *field).collect()
    }

    /// A page tree of two pages under one node.
    fn tree() -> BTreeMap<ObjectId, Object> {
        let page = || {
            let media = [0, 0, 1, 1].map(Object::from).to_vec();
            let page =
                dict! { "Type" => "Page", "Parent" => Object::reference(2), "MediaBox" => media };
            Object::from(page)
        };
        let kids = vec![Object::reference(3), Object::reference(4)];
        let root = dict! { "Type" => "Pages", "Kids" => kids, "Count" => 2 };
        BTreeMap::from([
            (ObjectId::new(2, 0), Object::from(root)),
            (ObjectId::new(3, 0), page()),
            (ObjectId::new(4, 0), page()),
        ])
    }

    /// A page tree walked is a span by either rules, which it says, and so
    /// is one counted; a page's parents are none.
    #[test]
    fn a_page_tree_walked_or_counted_is_a_span() {
        let objects = tree();
        let resolve = |id: ObjectId| objects.get(&id);
        let catalog = dict! { "Pages" => Object::reference(2) };
        let limits = Limits::DEFAULT;
        for rules in [Rules::Iso, Rules::Pdfium] {
            let made = spans(|| {
                let table = pages::pages(resolve, &catalog, rules, &limits, &mut ()).unwrap();
                assert_eq!(table.len(), 2);
                let parents = table.iter().map(|page| page.parents().count());
                assert_eq!(parents.sum::<usize>(), 2);
            });
            assert_eq!(named(&made), [("pages", Level::DEBUG)], "{rules:?}");
            assert_eq!(fields(&made[0]), ["pdfium"]);
        }
        let made = spans(|| {
            assert_eq!(pages::counted(resolve, &catalog, &limits), Ok(2));
        });
        assert_eq!(named(&made), [("counted", Level::DEBUG)]);
        assert_eq!(fields(&made[0]), Vec::<&str>::new());
    }

    /// A page tree written is a span, with how many pages it's given.
    #[test]
    fn a_page_tree_written_is_a_span() {
        let numbered = [ObjectId::new(1, 0), ObjectId::new(2, 0)];
        let mut next = 10;
        let made = spans(|| {
            let ids = || {
                next += 1;
                ObjectId::new(next - 1, 0)
            };
            pages::write(&numbered, FANOUT, &Limits::DEFAULT, ids).unwrap();
        });
        assert_eq!(named(&made), [("write", Level::DEBUG)]);
        assert_eq!(fields(&made[0]), ["pages"]);
    }

    /// A packet read is a span, with its length; read from a file's streams,
    /// a span holds it. A packet written, and the stream that holds one
    /// found, are none.
    #[test]
    fn a_packet_read_is_a_span() {
        let packet = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="4"/>"#;
        let limits = Limits::DEFAULT;
        let made = spans(|| {
            let claims = xmp::read(packet, None, &limits, &mut ()).unwrap();
            assert_eq!(claims.pdfa_part.as_deref(), Some("4"));
        });
        assert_eq!(named(&made), [("read", Level::DEBUG)]);
        assert_eq!(fields(&made[0]), ["bytes"]);
        let stream = Object::stream(dict! { "Type" => "Metadata", "Subtype" => "XML" });
        let objects = |id: ObjectId| (id == ObjectId::new(5, 0)).then_some(&stream);
        let file = WithStreams::new(objects, |id: ObjectId, _: usize| match id.number {
            5 => Ok(packet.to_vec()),
            _ => Err(Unreadable::new(id, Why::Damaged, "no such stream")),
        });
        let catalog = dict! { "Metadata" => Object::reference(5) };
        let made = spans(|| {
            let claims = xmp::read_from(file, &catalog, &limits, &mut ()).unwrap();
            assert_eq!(claims.unwrap().pdfa_part.as_deref(), Some("4"));
        });
        let both = [("read_from", Level::DEBUG), ("read", Level::DEBUG)];
        assert_eq!(named(&made), both);
        assert_eq!(fields(&made[0]), Vec::<&str>::new());
        let made = spans(|| {
            let claims = Claims {
                pdfua_part: Some("2".into()),
                ..Claims::default()
            };
            xmp::write(&claims).unwrap();
            assert_eq!(
                xmp::metadata_stream(objects, &catalog),
                Ok(Some(ObjectId::new(5, 0)))
            );
        });
        assert_eq!(named(&made), []);
    }

    /// A name or number tree walked has none, as its loop is the caller's;
    /// nor has one written, a date, a reference followed or a catalog found.
    #[test]
    fn a_tree_walked_and_the_lookups_are_none() {
        let made = spans(|| {
            let root = Object::from(tree::write_names([(b"a".to_vec(), Object::from(1))]).unwrap());
            let nothing = |_: ObjectId| None::<&Object>;
            let limits = Limits::DEFAULT;
            assert_eq!(tree::names(nothing, &root, &limits, ()).count(), 1);
            let root = Object::from(tree::write_numbers([(1, Object::from(1))]).unwrap());
            assert_eq!(tree::numbers(nothing, &root, &limits, ()).count(), 1);
            assert_eq!(Date::read("D:2026").unwrap().write(), "D:2026");
            let one = Object::reference(1);
            assert_eq!(follow(nothing, &one), Ok(None));
            let trailer = dict! { "Root" => Object::reference(1) };
            assert_eq!(catalog(nothing, &trailer), Ok(None));
        });
        assert_eq!(named(&made), []);
    }
}
