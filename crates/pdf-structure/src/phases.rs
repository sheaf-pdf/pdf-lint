//! The spans the reader's and the writer's phases run in, with the
//! `tracing` feature: one macro, so that a phase is a line where it starts,
//! and nothing of it is compiled without the feature. A phase is a step a
//! file is read or written in once, or once an object stream: the open,
//! its sections and a scan, an object stream decoded, a whole read, an
//! earlier revision's open, a write and the object streams it builds. No
//! object, token or byte has one, and no fault is told as an event: a
//! reading's faults are its own ([`Source::faults`](crate::Source::faults)).

/// Enters a span at `$level` (`DEBUG`, or `TRACE` for what a file has many
/// of) named `$name`, held to the end of the block it's in, its fields
/// numbers alone (an object's number, an offset, a count, a length), each
/// cast to a `u64`, so that no value, key, password or plaintext can be
/// one. Without the `tracing` feature it's nothing, its fields not
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
    use std::fmt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, Once};
    use std::thread::{self, ThreadId};

    use pdf_fixtures::fixture::{self, Xref};
    use pdf_objects::dict;
    use pdf_objects::object::{Object, ObjectId};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use crate::read::{self, Options, Reading};
    use crate::source::XrefKind;
    use crate::write;

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

    /// A file of a catalog and its page tree, its section of `kind`.
    fn file(kind: Xref) -> Vec<u8> {
        let objects = [
            (
                1,
                dict! { "Type" => "Catalog", "Pages" => Object::reference(2) }.into(),
            ),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 }.into(),
            ),
        ];
        fixture::file(
            "1.7",
            &objects,
            dict! { "Root" => Object::reference(1) },
            kind,
        )
    }

    /// An open is a span, and the sections it reads one within it, each
    /// with its fields: the file's length, where the newest section starts.
    #[test]
    fn an_open_is_a_span_and_its_sections_one() {
        let file = file(Xref::Table);
        let made = spans(|| {
            Reading::open(&file[..], &Options::DEFAULT).unwrap();
        });
        let fields = |made: &Made| made.fields.iter().map(|(f, _)| *f).collect::<Vec<_>>();
        assert_eq!(fields(&made[0]), ["len"]);
        assert_eq!(fields(&made[1]), ["at"]);
        assert_eq!(
            named(&made),
            [("open", Level::DEBUG), ("sections", Level::DEBUG)]
        );
    }

    /// A file whose sections don't read, as it has no `startxref`, is
    /// scanned within its open.
    #[test]
    fn a_scan_is_a_span_within_the_open() {
        let mut file = file(Xref::Table);
        let xref = file.windows(4).position(|w| w == b"xref").unwrap();
        file.truncate(xref);
        let made = spans(|| {
            let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
            assert!(reading.trailer().unwritten.is_some());
        });
        assert_eq!(
            named(&made),
            [("open", Level::DEBUG), ("scan", Level::DEBUG)]
        );
    }

    /// A whole read is a span after the open's, and so is a document read
    /// beside its reading; an object stream decoded is one, at `trace`, in
    /// the whole read, and in a reading on demand when an object in it is
    /// first read, and not again.
    #[test]
    fn a_whole_read_and_an_object_stream_decoded_are_spans() {
        let (file, _) = crate::read::tests::compressed(false);
        let open = [("open", Level::DEBUG), ("sections", Level::DEBUG)];
        let whole = [("whole", Level::DEBUG), ("object_stream", Level::TRACE)];
        let made = spans(|| {
            assert!(read::document(&file, &Options::DEFAULT).catalog().is_some());
        });
        assert_eq!(named(&made), [&open[..], &whole[..]].concat());
        let made = spans(|| {
            let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
            reading.get(ObjectId::new(2, 0)).unwrap();
            reading.get(ObjectId::new(3, 0)).unwrap();
            reading.to_document().unwrap();
        });
        let decoded = [("object_stream", Level::TRACE), ("whole", Level::DEBUG)];
        assert_eq!(named(&made), [&open[..], &decoded[..]].concat());
        assert_eq!(made[2].fields, [("number", true)]);
        let made = spans(|| {
            let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
            reading.into_document().unwrap();
        });
        assert_eq!(named(&made), [&open[..], &whole[..]].concat());
    }

    /// An earlier revision's open is a span, its own open within it.
    #[test]
    fn a_prefix_is_a_span() {
        let file = file(Xref::Table);
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let made = spans(|| {
            reading.prefix(file.len(), reading.limits()).unwrap();
        });
        let prefix = [("prefix", Level::DEBUG), ("open", Level::DEBUG)];
        assert_eq!(
            named(&made),
            [&prefix[..], &[("sections", Level::DEBUG)]].concat()
        );
    }

    /// Each write is a span: a whole file, a document through it, the
    /// object streams it builds, and an update.
    #[test]
    fn each_write_is_a_span() {
        let file = file(Xref::Table);
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let doc = reading.to_document().unwrap();
        let made = spans(|| {
            let mut out = Vec::new();
            let trailer = doc.trailer().dict();
            let options = write::Options::DEFAULT;
            write::full(&mut out, write::bodies(&doc), trailer, None, None, &options).unwrap();
        });
        assert_eq!(named(&made), [("full", Level::DEBUG)]);
        assert_eq!(made[0].fields, [("bodies", true)]);
        let made = spans(|| {
            let mut options = write::Options::DEFAULT;
            options.kind = XrefKind::Stream;
            options.pack = Some(write::Pack::DEFAULT);
            let mut out = Vec::new();
            write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
        });
        let document = [("document", Level::DEBUG), ("full", Level::DEBUG)];
        let packed = [&document[..], &[("containers", Level::DEBUG)]].concat();
        assert_eq!(named(&made), packed);
        let made = spans(|| {
            let base = write::Base::of(&reading).unwrap();
            let mut out = file.clone();
            let page = dict! { "Type" => "Page", "Parent" => Object::reference(2) };
            let bodies = [(ObjectId::new(3, 0), write::Body::from(Object::from(page)))];
            let options = write::Options::DEFAULT;
            write::update(&mut out, &base, bodies, None, Some(&reading), &options).unwrap();
        });
        assert_eq!(named(&made), [("update", Level::DEBUG)]);
        assert_eq!(made[0].fields, [("at", true)]);
    }
}
