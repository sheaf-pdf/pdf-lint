//! The spans the calls that read or write a whole page run in, with the
//! `tracing` feature: one macro, so that a phase is a line where it starts,
//! and nothing of it is compiled without the feature. A phase is a call a
//! page's content is read, spliced or written in once: a resources
//! dictionary's color spaces counted, a page's balance, marked content and
//! names, a splice, `splice_each` and each of its checks, a list written
//! again, what an edit makes in a page's streams, the names a resources
//! dictionary binds, and the writer's names.
//! No token, operation or image has one, as their loop is the caller's,
//! and no fault is told as an event: faults are the sink's.

/// Enters a span at `$level` (`DEBUG`, or `TRACE` for what a call makes
/// many of) named `$name`, held to the end of the block it's in, its
/// fields numbers alone (a length, a count), each cast to a `u64`, so that
/// no byte of content can be one. Without the `tracing` feature it's
/// nothing, its fields not evaluated.
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

    use pdf_objects::{Object, ObjectId, dict};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use crate::*;

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

    /// The content the tests read: a page of two streams, its balance
    /// across them, and two marked ranges, the first of which a splice may
    /// take out, and the second not, as it sets a color past it.
    fn page() -> Concat<'static> {
        Concat::new(&[
            b"q /A BMC q 0 g Q EMC /F1 9 Tf",
            b"/B <</K 1>> BDC 1 g EMC Q",
        ])
    }

    /// A page's balance, marked content, names and what's kept are a span
    /// each, `names_used`'s `kept`'s within it, each with its fields: the
    /// content's length and its streams, and the ranges taken out. Its
    /// tokens and operations are none.
    #[test]
    fn a_pages_passes_are_spans() {
        let (page, none, limits) = (page(), ResCx::none(), Limits::DEFAULT);
        let props = Properties::default();
        let fields = |made: &Made| made.fields.iter().map(|(f, _)| *f).collect::<Vec<_>>();
        let made = spans(|| {
            balance(&page, &none, &limits).unwrap();
        });
        assert_eq!(named(&made), [("balance", Level::DEBUG)]);
        assert_eq!(fields(&made[0]), ["len", "streams"]);
        let made = spans(|| {
            marked(&page, &props, &none, &limits).unwrap();
        });
        assert_eq!(named(&made), [("marked", Level::DEBUG)]);
        let made = spans(|| {
            names_used(&page, &none, &limits, &[]).unwrap();
        });
        let used = [("names_used", Level::DEBUG), ("kept", Level::DEBUG)];
        assert_eq!(named(&made), used);
        assert_eq!(fields(&made[1]), ["len", "streams", "ranges"]);
        let made = spans(|| {
            tokens(page.bytes(), &none, &limits).for_each(drop);
            page.operations(&none, &limits).for_each(drop);
        });
        assert_eq!(named(&made), []);
    }

    /// A splice is a span, and so is `splice_each`, each check within it
    /// one at `trace`; a list written again is one, and so is what's taken
    /// out made with it.
    #[test]
    fn a_splice_and_its_checks_are_spans() {
        let (page, none, limits) = (page(), ResCx::none(), Limits::DEFAULT);
        let props = Properties::default();
        let found = marked(&page, &props, &none, &limits).unwrap();
        let (a, b) = (found[0].span, found[1].span);
        let made = spans(|| {
            splice(&page, &none, &limits, &[a]).unwrap();
        });
        assert_eq!(named(&made), [("splice", Level::DEBUG)]);
        // The second sets a color outside a `q`...`Q` of its own: refused
        // together, so each is checked, and the one taken again.
        let groups = [vec![a], vec![b]];
        let made = spans(|| {
            let taken = splice_each(&page, &none, &limits, &groups).unwrap();
            assert_eq!(taken.taken, [true, false]);
        });
        let check = ("check", Level::TRACE);
        let each = [("splice_each", Level::DEBUG), check, check, check, check];
        assert_eq!(named(&made), each);
        let keys: [&[u8]; 1] = [b"K"];
        let made = spans(|| {
            let rewrite = without_keys(&page, &none, &limits, b, &keys)
                .unwrap()
                .unwrap();
            let nothing = Spliced::default();
            nothing.streams(&page, &none, &limits, &[rewrite]).unwrap();
        });
        let written = [("without_keys", Level::DEBUG), ("streams", Level::DEBUG)];
        assert_eq!(named(&made), written);
    }

    /// A resources dictionary's color spaces counted, the names one binds,
    /// and the writer's names are a span each.
    #[test]
    fn resources_and_the_writers_names_are_spans() {
        let limits = Limits::DEFAULT;
        let resources = Object::from(dict! {
            "ColorSpace" => dict! { "CS0" => "DeviceRGB" },
            "Font" => dict! { "F1" => Object::reference(1) },
        });
        let nothing = |_: ObjectId| None;
        let made = spans(|| {
            ResCx::read(nothing, Some(&resources), &limits).unwrap();
        });
        assert_eq!(named(&made), [("spaces", Level::DEBUG)]);
        let made = spans(|| {
            let _ = Reserved::read(nothing, Some(&resources), &limits);
        });
        assert_eq!(named(&made), [("reserved", Level::DEBUG)]);
        let mut w = ContentWriter::new();
        let font = Res::dict(dict! { "Type" => "Font" }).unwrap();
        w.font(font, 12.0);
        let made = spans(|| {
            let (_, needs) = w.finish(&Reserved::default());
            assert_eq!(needs.0.len(), 1);
        });
        assert_eq!(named(&made), [("named", Level::DEBUG)]);
        assert_eq!(made[0].fields, [("len", true), ("needs", true)]);
    }
}
