//! The spans the crate's work runs in, with the `tracing` feature: one macro,
//! so that a phase is a line where it starts, and nothing of it is compiled
//! without the feature. A phase is a call made once for a file, or a stage of
//! one: a plan, an assembly and each round its first-page section takes to
//! settle, a check; and with `family`, a document's linearizing and each of
//! its stages (the graph, each plan, each round's object streams, the bodies,
//! the bytes around them, the file laid out whole), and a file's facts and
//! each of their reads (the sections, the hint stream, the page tree, the
//! graph). An object, a page, a hint table's entry, a row and a finding have
//! none, their loops being the phases'; and nothing's told as an event:
//! findings are the sink's.

/// Enters a span at `$level` (`DEBUG`) named `$name`, held to the end of the
/// block it's in, its fields numbers alone (a length, a count), each cast to a
/// `u64`, so that no byte of a file can be one. Without the `tracing` feature
/// it's nothing, its fields not evaluated.
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
    use std::convert::Infallible;
    use std::fmt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, Once};
    use std::thread::{self, ThreadId};

    use pdf_objects::ObjectId;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use crate::Limits;
    use crate::check::{self, Facts, PageFacts};
    use crate::layout::{self, Encoded, Piece, StreamKind, assemble};
    use crate::plan::{Graph, Kind, Page, plan};

    /// A span as it was made: the module it's in, its name, its level, and each
    /// field's name and its value, where that was a number.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Made {
        module: &'static str,
        name: &'static str,
        level: Level,
        fields: Vec<(&'static str, Option<u64>)>,
    }

    /// A subscriber, written over tracing-core's trait, that keeps each of this
    /// crate's spans made on a thread `spans` is running on, and does nothing
    /// else. It's the process's default, set once by the first `spans`.
    /// Another crate's spans aren't this crate's to test, and its own feature
    /// may turn them on.
    struct Recorder {
        ids: AtomicU64,
    }

    /// The spans made so far on each thread `spans` is running on.
    static MADE: Mutex<Vec<(ThreadId, Vec<Made>)>> = Mutex::new(Vec::new());

    /// Each field of a span, and its value where it's a `u64`, as the macro
    /// makes every one: any other kind reaches `record_debug`, as the trait's
    /// defaults pass it on.
    struct Fields(Vec<(&'static str, Option<u64>)>);

    impl Visit for Fields {
        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.push((field.name(), Some(value)));
        }

        fn record_debug(&mut self, field: &Field, _: &dyn fmt::Debug) {
            self.0.push((field.name(), None));
        }
    }

    impl Subscriber for Recorder {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &Attributes<'_>) -> Id {
            let metadata = span.metadata();
            let id = Id::from_u64(self.ids.fetch_add(1, Ordering::Relaxed) + 1);
            let Some(module) = metadata.target().strip_prefix("pdf_linearize::") else {
                return id;
            };
            let mut fields = Fields(Vec::new());
            span.record(&mut fields);
            let me = thread::current().id();
            let mut threads = MADE.lock().unwrap();
            if let Some((_, made)) = threads.iter_mut().find(|(thread, _)| *thread == me) {
                made.push(Made {
                    module,
                    name: metadata.name(),
                    level: *metadata.level(),
                    fields: fields.0,
                });
            }
            id
        }

        fn record(&self, _: &Id, _: &Record<'_>) {}

        fn record_follows_from(&self, _: &Id, _: &Id) {}

        fn event(&self, _: &Event<'_>) {}

        fn enter(&self, _: &Id) {}

        fn exit(&self, _: &Id) {}
    }

    /// The spans `run` makes on this thread, in the order they're made.
    ///
    /// The recorder is the process's default rather than this thread's alone:
    /// tracing-core caches whether a callsite is wanted when it's first
    /// reached, and while one dispatcher is registered it asks the default of
    /// the thread that reached it, so a recorder set for this thread with
    /// `with_default` misses a span another test reached first on a thread
    /// with none. Every callsite's interest is worked out again before `run`,
    /// for one first reached on another thread while the default was being
    /// set.
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

    /// A span's module, its name, its level and its fields, each a number.
    type Named = (&'static str, &'static str, Level, Vec<(&'static str, u64)>);

    /// Each span's module, name, level and fields, every field a number.
    fn named(spans: &[Made]) -> Vec<Named> {
        spans
            .iter()
            .map(|made| {
                let fields = made.fields.iter().map(|&(field, value)| {
                    let value = value.unwrap_or_else(|| {
                        panic!("{}'s {field} isn't a number", made.name);
                    });
                    (field, value)
                });
                (made.module, made.name, made.level, fields.collect())
            })
            .collect()
    }

    /// A span of `module` named `name` at `debug`, with `fields`.
    fn debug(module: &'static str, name: &'static str, fields: &[(&'static str, u64)]) -> Named {
        (module, name, Level::DEBUG, fields.to_vec())
    }

    /// A catalog, 1, over a page, 2, and an object it holds, 3, listed as
    /// `kind` lists them.
    fn small(kind: Kind) -> Graph {
        let id = |number: u32| ObjectId::new(number, 0);
        let mut graph = Graph::new(id(1), kind);
        graph.object(id(1), [id(2)]);
        graph.object(id(2), [id(3)]);
        graph.object(id(3), []);
        graph.page(Page::new(id(2)));
        graph
    }

    /// The room a layout's first-page section was given: what its first piece
    /// holds past where the section starts, the section padded to it.
    fn room(layout: &layout::Layout) -> u64 {
        match layout.pieces().next() {
            Some(Piece::Bytes(head)) => head.len() as u64 - layout.first_section(),
            _ => panic!("a layout starts with its head"),
        }
    }

    /// A plan is a span, with how many objects and pages its graph has, whether
    /// it's planned or refused.
    #[test]
    fn a_plan_is_a_span() {
        let graph = small(Kind::Table);
        let made = spans(|| assert!(plan(&graph, &Limits::DEFAULT).is_ok()));
        let fields = [("objects", 3), ("pages", 1)];
        assert_eq!(named(&made), [debug("plan", "plan", &fields)]);
        let no_pages = Graph::new(ObjectId::new(1, 0), Kind::Table);
        let made = spans(|| assert!(plan(&no_pages, &Limits::DEFAULT).is_err()));
        let fields = [("objects", 0), ("pages", 0)];
        assert_eq!(named(&made), [debug("plan", "plan", &fields)]);
    }

    /// An assembly is a span, with how many writes it lays out, and each round
    /// its first-page section takes to settle is one within it, with the room
    /// that round gives the section: a table's one round, and a stream's as
    /// many as it wants more room, each giving it more, the last what it's
    /// written in.
    #[test]
    fn an_assembly_is_a_span_and_each_round_of_its_first_section_one() {
        let options = layout::Options::new((1, 7));
        for (kind, longer) in [(Kind::Table, 0), (Kind::Stream, 0), (Kind::Stream, 3)] {
            let plan = plan(&small(kind), &Limits::DEFAULT).unwrap();
            let lengths = vec![40; plan.writes().len()];
            let mut asked = 0;
            let mut layout = None;
            let made = spans(|| {
                let assembled = assemble(&plan, &lengths, &options, |stream| {
                    let mut encoded = Encoded::plain(stream.data.to_vec());
                    if stream.what == StreamKind::FirstXref {
                        asked += 1;
                        if asked <= longer {
                            encoded.data.extend(std::iter::repeat_n(0, 40 << asked));
                        }
                    }
                    Ok::<_, Infallible>(encoded)
                });
                layout = Some(assembled.unwrap());
            });
            let layout = layout.unwrap();
            let named = named(&made);
            let writes = [("writes", plan.writes().len() as u64)];
            assert_eq!(named[0], debug("layout", "assemble", &writes));
            let rooms: Vec<u64> = (named[1..].iter())
                .map(|span| match span {
                    ("layout", "round", Level::DEBUG, fields) => match fields[..] {
                        [("room", room)] => room,
                        _ => panic!("{fields:?}"),
                    },
                    _ => panic!("{span:?}"),
                })
                .collect();
            // A stream's first guess asks once before the rounds.
            let rounds = match kind {
                Kind::Table => 1,
                Kind::Stream => asked - 1,
            };
            assert_eq!(rooms.len(), rounds, "{kind:?} {longer}");
            assert!(rooms.is_sorted_by(|a, b| a < b), "{rooms:?}");
            assert_eq!(rooms.last(), Some(&room(&layout)));
            assert_eq!(rooms.len() > 1, longer > 0, "{rooms:?}");
        }
    }

    /// A check is a span, with how many pages its facts have.
    #[test]
    fn a_check_is_a_span() {
        let mut facts = Facts::new(1000);
        facts.pages = vec![PageFacts::new(None); 3];
        let made = spans(|| {
            let verdict = check::check(&facts, &Limits::DEFAULT, &mut Vec::new());
            assert!(!verdict.linearized);
        });
        assert_eq!(named(&made), [debug("check", "check", &[("pages", 3)])]);
    }

    /// The family's writes and reads.
    #[cfg(feature = "family")]
    mod family {
        use std::collections::HashSet;

        use pdf_structure::XrefKind;
        use pdf_structure::read::{self, Reading};

        use super::*;
        use crate::family::graph::Prepared;
        use crate::family::{self, Options};
        use crate::plan::What;

        /// A file of a catalog, a root node and two pages, each with resources
        /// of its own, under a table.
        fn file() -> Vec<u8> {
            let mut file = b"%PDF-1.7\n".to_vec();
            let mut rows = Vec::new();
            for (number, object) in [
                (1, "<< /Type /Catalog /Pages 2 0 R >>"),
                (2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"),
                (
                    3,
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources 5 0 R >>",
                ),
                (
                    4,
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources 6 0 R >>",
                ),
                (5, "<< /Font << >> >>"),
                (6, "<< /ProcSet [/PDF] >>"),
            ] {
                rows.push(file.len());
                file.extend_from_slice(format!("{number} 0 obj\n{object}\nendobj\n").as_bytes());
            }
            let at = file.len();
            file.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
            for row in rows {
                file.extend_from_slice(format!("{row:010} 00000 n \n").as_bytes());
            }
            let tail = format!("trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n");
            file.extend_from_slice(tail.as_bytes());
            file
        }

        /// The three kinds: a table, a stream, a stream with object streams.
        fn kinds() -> [Options<'static>; 3] {
            let mut stream = Options::DEFAULT;
            stream.kind = XrefKind::Stream;
            let mut packed = stream;
            packed.pack = true;
            [Options::DEFAULT, stream, packed]
        }

        /// A document's linearizing is a span, and each of its stages one
        /// within it, in the order they run: the graph; the plan, the engine's
        /// within it; the object streams, with how many the plan has; the
        /// bodies, with how many writes; the bytes around them, the engine's
        /// assembly within it, and its rounds; and the file laid out whole, with
        /// its length. In each kind, with what each is told the plan's own.
        #[test]
        fn a_linearize_is_a_span_and_each_stage_one_in_order() {
            let file = file();
            let reading = Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
            let doc = reading.to_document().unwrap();
            for options in kinds() {
                let (kind, pack) = match options.kind {
                    XrefKind::Stream => (Kind::Stream, options.pack),
                    _ => (Kind::Table, false),
                };
                let prepared = Prepared::of(&doc, None, &options).unwrap();
                let graph = prepared.graph(kind, pack, &HashSet::new());
                let planned = plan(&graph, &options.limits).unwrap();
                let members: usize = (planned.containers().iter())
                    .map(|container| container.members.len())
                    .sum();
                let objects = (planned.writes().iter())
                    .filter(|write| matches!(write.what, What::Object(_)))
                    .count()
                    + members;
                assert_eq!(members > 0, pack, "{options:?}");

                let mut out = Vec::new();
                let mut written = None;
                let made = spans(|| {
                    let linearized =
                        family::linearize(&mut out, &doc, None, Some(&reading), &options);
                    written = Some(linearized.unwrap());
                });
                let layout = written.unwrap().layout;
                let writes = planned.writes().len() as u64;
                let named = named(&made);
                let (rounds, stages): (Vec<_>, Vec<_>) =
                    named.into_iter().partition(|span| span.1 == "round");
                let expected = [
                    debug("family", "linearize", &[]),
                    debug("family", "graph", &[]),
                    debug("family", "plan", &[]),
                    debug("plan", "plan", &[("objects", objects as u64), ("pages", 2)]),
                    debug(
                        "family",
                        "containers",
                        &[("containers", planned.containers().len() as u64)],
                    ),
                    debug("family", "bodies", &[("writes", writes)]),
                    debug("family", "assemble", &[]),
                    debug("layout", "assemble", &[("writes", writes)]),
                    debug("family", "whole", &[("bytes", out.len() as u64)]),
                ];
                assert_eq!(stages, expected, "{options:?}");
                assert_eq!(
                    rounds.last(),
                    Some(&debug("layout", "round", &[("room", room(&layout))])),
                    "{options:?}"
                );
                // The rounds are the engine's assembly's, within the stage.
                let assembled = made.iter().position(|span| span.module == "layout");
                let whole = made.iter().position(|span| span.name == "whole");
                let within = made
                    .iter()
                    .enumerate()
                    .filter(|(_, span)| span.name == "round");
                for (at, _) in within {
                    assert!(assembled < Some(at) && Some(at) < whole);
                }
            }
        }

        /// A file's facts are a span, with its length, and each of their reads
        /// one within it: its sections, its hint stream, its page tree and its
        /// graph. A file that doesn't say it's linearized has none read. A
        /// check of a reading is its facts', then the check's, its graph's plan
        /// within it.
        #[test]
        fn a_file_s_facts_are_a_span_and_each_read_one() {
            let file = file();
            let mut out = Vec::new();
            let reading = Reading::open(&file[..], &read::Options::DEFAULT).unwrap();
            let doc = reading.to_document().unwrap();
            family::linearize(&mut out, &doc, None, Some(&reading), &Options::DEFAULT).unwrap();
            let written = Reading::open(&out[..], &read::Options::DEFAULT).unwrap();
            let bytes = [("bytes", out.len() as u64)];
            let reads = [
                debug("family::facts", "facts", &bytes),
                debug("family::facts", "sections", &[]),
                debug("family::facts", "hint", &[]),
                debug("family::facts", "pages", &[]),
                debug("family::facts", "graph", &[]),
            ];
            let made = spans(|| {
                let facts = family::facts(&written, &Limits::DEFAULT).unwrap();
                assert_eq!(facts.pages.len(), 2);
            });
            assert_eq!(named(&made), reads);

            let made = spans(|| {
                let verdict = family::check(&written, &Limits::DEFAULT, &mut ()).unwrap();
                assert!(verdict.linearized && verdict.holds && verdict.hints_read);
            });
            // The check plans the file's graph, to hold what each page reaches
            // to the tables.
            let mut checked = reads.to_vec();
            checked.push(debug("check", "check", &[("pages", 2)]));
            checked.push(debug("plan", "plan", &[("objects", 6), ("pages", 2)]));
            assert_eq!(named(&made), checked);

            let made = spans(|| {
                let facts = family::facts(&reading, &Limits::DEFAULT).unwrap();
                assert!(facts.pages.is_empty());
            });
            let bytes = [("bytes", file.len() as u64)];
            assert_eq!(named(&made), [debug("family::facts", "facts", &bytes)]);
        }
    }
}
