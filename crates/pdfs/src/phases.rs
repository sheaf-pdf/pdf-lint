//! The spans the facade's calls run in, with the `tracing` feature: one
//! macro, so that a phase is a line where it starts, and nothing of it is
//! compiled without the feature. A phase is a call that reads a part of a
//! file whole: a file opened, its page tree walked, a page's content joined,
//! its fonts read and its text, its document information dictionary or its
//! packet read, and its linearization checked; or that writes one: a file read
//! whole to edit, and a document saved whole, linearized or as an update. A
//! lookup has none (the catalog, the version, a stream's data), nor has a page
//! added or a resource written, and no fault is told as an event: faults are
//! the sink's.
//! The crates under it make their own spans inside these, where they have
//! the feature.

/// Enters a span at `$level` (`DEBUG`) named `$name`, held to the end of
/// the block it's in, its fields numbers alone (a length, which rules), each
/// cast to a `u64`, so that no byte of a file can be one. Without the
/// `tracing` feature it's nothing, its fields not evaluated.
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

    use pdf_objects::ObjectId;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    /// A span as it was made: its name, its level, and each field's name
    /// and whether its value was a number.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Made {
        name: &'static str,
        level: Level,
        fields: Vec<(&'static str, bool)>,
    }

    /// A subscriber, written over tracing-core's trait, that keeps each of
    /// this crate's spans made on a thread `spans` is running on, and does
    /// nothing else: the crates under it make their own, which their tests
    /// hold. It's the process's default, set once by the first `spans`.
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
            let metadata = span.metadata();
            let me = thread::current().id();
            let ours = metadata.target().split("::").next() == Some("pdfs");
            let mut threads = MADE.lock().unwrap();
            if let Some((_, made)) = threads.iter_mut().find(|(thread, _)| *thread == me)
                && ours
            {
                let mut fields = Fields(Vec::new());
                span.record(&mut fields);
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

    /// This crate's spans `run` makes on this thread, in the order they're
    /// made.
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

    /// Each span's name, level and fields' names, every one's fields
    /// numbers alone.
    fn named(spans: &[Made]) -> Vec<(&'static str, Level, Vec<&'static str>)> {
        for made in spans {
            for &(field, number) in &made.fields {
                assert!(number, "{}'s {field} isn't a number", made.name);
            }
        }
        let fields = |made: &Made| made.fields.iter().map(|(field, _)| *field).collect();
        (spans.iter())
            .map(|made| (made.name, made.level, fields(made)))
            .collect()
    }

    /// A file of one page, stream 4 its content, as written by hand.
    const FILE: &[u8] = b"%PDF-1.7\n\
        1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
        2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>\nendobj\n\
        3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n\
        4 0 obj\n<< /Length 27 >>\nstream\n0 0 1 rg 72 72 144 144 re f\nendstream\nendobj\n\
        xref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n\
        0000000139 00000 n \n0000000202 00000 n \n\
        trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n279\n%%EOF\n";

    /// A file opened, its pages walked, a page's content joined, its fonts
    /// and its text read (the text's content within it), what it says and
    /// claims read, its linearization checked, and its document edited and
    /// saved each way are each a span, at `debug`, their fields numbers; its
    /// catalog, version and a stream's data, each a lookup, are none.
    #[test]
    fn each_part_read_whole_is_a_span_and_a_lookup_none() {
        const DEBUG: Level = Level::DEBUG;
        let mut pdf = None;
        let made = spans(|| pdf = Some(crate::from_bytes(FILE).unwrap()));
        assert_eq!(named(&made), [("open", DEBUG, vec!["bytes"])]);
        let pdf = pdf.unwrap();
        let made = spans(|| {
            let pages = pdf.pages().unwrap();
            pages.get(0).unwrap().content().unwrap();
        });
        let walked = [("pages", DEBUG, vec!["pdfium"]), ("content", DEBUG, vec![])];
        assert_eq!(named(&made), walked);
        let made = spans(|| {
            let pages = pdf.pages().unwrap();
            let page = pages.get(0).unwrap();
            assert!(page.fonts().unwrap().is_empty());
            assert!(page.text().unwrap().strings.is_empty());
        });
        let read = [
            ("pages", DEBUG, vec!["pdfium"]),
            ("fonts", DEBUG, vec![]),
            ("text", DEBUG, vec![]),
            ("content", DEBUG, vec![]),
        ];
        assert_eq!(named(&made), read);
        let made = spans(|| {
            assert_eq!(pdf.info().unwrap(), None);
            assert_eq!(pdf.claims().unwrap(), None);
            assert!(!pdf.linearized().unwrap().linearized);
        });
        let read = [
            ("info", DEBUG, vec![]),
            ("claims", DEBUG, vec![]),
            ("linearized", DEBUG, vec!["bytes"]),
        ];
        assert_eq!(named(&made), read);
        let made = spans(|| {
            pdf.catalog().unwrap();
            pdf.version().unwrap();
            pdf.stream(ObjectId::new(4, 0)).unwrap();
        });
        assert_eq!(named(&made), []);
        let made = spans(|| {
            let mut doc = pdf.edit().unwrap();
            doc.save_to(&mut Vec::new()).unwrap();
            let options = doc.linearize_options();
            doc.save_linearized_to(&mut Vec::new(), &options).unwrap();
            assert!(doc.save_update_to(&mut Vec::new()).unwrap().is_none());
        });
        let saved = [
            ("edit", DEBUG, vec!["bytes"]),
            ("save", DEBUG, vec!["pages"]),
            ("save_linearized", DEBUG, vec!["pages"]),
            ("save_update", DEBUG, vec!["pages"]),
        ];
        assert_eq!(named(&made), saved);
    }
}
