//! The spans a font's reading runs in, with the `tracing` feature: one macro, so
//! that a phase is a line where it starts, and nothing of it is compiled without the
//! feature. A phase is a call a font, or a part of one, is read in once: a font read,
//! its program opened, an embedded CMap's chain, its `/ToUnicode` and a Type 3 font's
//! CharProcs. A part read from a stream has its span once the stream is decompressed,
//! so that its fields say how much was read, and the decompressing is its font's
//! span's. But a Type 3 font's CharProcs are decompressed within their span, whose
//! field, how many the font has, is known before any is. A font looked up where it was
//! read before has none, nor has a code, a width, a string's text or a glyph's box, as
//! their loop is the caller's, nor a glyph's name or a standard 14 font's, each a
//! lookup; and no fault is told as an event: faults are the sink's.

/// Enters a span at `$level` (`DEBUG`) named `$name`, held to the end of the block
/// it's in, its fields numbers alone (a length, a count, a flag), each cast to a
/// `u64`, so that no byte of a font can be one. Without the `tracing` feature it's
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
    use std::collections::BTreeMap;
    use std::fmt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, Once};
    use std::thread::{self, ThreadId};

    use pdf_objects::object::Stream;
    use pdf_objects::{Dict, Object, ObjectId, Unreadable, Why, WithStreams, dict};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use crate::{Encoding, Font, Fonts, Limits, Std14, glyph_text, name_code_point};

    /// A span as it was made: its name, its level, and each field's name and its
    /// value, where that was a number.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Made {
        name: &'static str,
        level: Level,
        fields: Vec<(&'static str, Option<u64>)>,
    }

    /// A subscriber, written over tracing-core's trait, that keeps each span made on
    /// a thread `spans` is running on, and does nothing else. It's the process's
    /// default, set once by the first `spans`.
    struct Recorder {
        ids: AtomicU64,
    }

    /// The spans made so far on each thread `spans` is running on.
    static MADE: Mutex<Vec<(ThreadId, Vec<Made>)>> = Mutex::new(Vec::new());

    /// Each field of a span, and its value where it's a `u64`, as the macro makes
    /// every one: any other kind reaches `record_debug`, as the trait's defaults
    /// pass it on.
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
    /// The recorder is the process's default rather than this thread's alone:
    /// tracing-core caches whether a callsite is wanted when it's first reached, and
    /// while one dispatcher is registered it asks the default of the thread that
    /// reached it, so a recorder set for this thread with `with_default` misses a span
    /// another test reached first on a thread with none. Every callsite's interest is
    /// worked out again before `run`, for one first reached on another thread while
    /// the default was being set.
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

    /// A span's name, its level and its fields, each a number.
    type Named = (&'static str, Level, Vec<(&'static str, u64)>);

    /// Each span's name, level and fields, every field a number.
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
                (made.name, made.level, fields.collect())
            })
            .collect()
    }

    /// A file of `objects`, numbered from 1, each stream's data as it's held.
    fn file(objects: Vec<Object>) -> BTreeMap<ObjectId, Object> {
        (1..).map(|n| ObjectId::new(n, 0)).zip(objects).collect()
    }

    /// `font` read over `objects`, its program where `programs` says.
    fn read<'o>(objects: &'o BTreeMap<ObjectId, Object>, font: &'o Dict, programs: bool) -> Font {
        let resolve = |id: ObjectId| objects.get(&id);
        let objects = WithStreams::new(&resolve, |id: ObjectId, _: usize| {
            let data = objects.get(&id).and_then(Object::as_stream);
            let data = data.and_then(|stream| stream.data.bytes());
            data.map(<[u8]>::to_vec)
                .ok_or_else(|| Unreadable::new(id, Why::Damaged, "no such stream"))
        });
        let mut budget = 1 << 20;
        let limits = Limits::DEFAULT;
        let read = match programs {
            true => Font::read(objects, font, &limits, &mut budget, &mut ()),
            false => Font::read_for_text(objects, font, &limits, &mut budget, &mut ()),
        };
        read.unwrap()
    }

    /// A simple font read is a span, which says whether its program was asked for;
    /// read again from what keeps it, it's none.
    #[test]
    fn a_font_read_is_a_span() {
        let helvetica = dict! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        };
        let objects = file(vec![helvetica.clone().into()]);
        for programs in [true, false] {
            let made = spans(|| {
                let font = read(&objects, &helvetica, programs);
                assert_eq!(font.width(u32::from(b'A')), 667.0);
            });
            let fields = vec![("programs", u64::from(programs))];
            assert_eq!(named(&made), [("read", Level::DEBUG, fields)]);
        }
        let resolve = |id: ObjectId| objects.get(&id);
        let objects = WithStreams::new(&resolve, |id: ObjectId, _: usize| {
            Err(Unreadable::new(id, Why::Damaged, "no streams"))
        });
        let fonts = Fonts::for_text();
        let one = Object::reference(1);
        let get = |budget: &mut usize| fonts.get(objects, &one, &Limits::DEFAULT, budget);
        let made = spans(|| assert!(get(&mut 0).unwrap().is_some()));
        assert_eq!(
            named(&made),
            [("read", Level::DEBUG, vec![("programs", 0)])]
        );
        let made = spans(|| assert!(get(&mut 0).unwrap().is_some()));
        assert_eq!(named(&made), []);
    }

    /// A Type 0 font's `/ToUnicode` is a span, with how many bytes it decompressed
    /// to, and so is its embedded CMap's chain, with how many streams it holds.
    #[test]
    fn a_type0_font_s_cmap_and_to_unicode_are_spans() {
        let to_unicode = b"1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
            1 beginbfchar <0041> <0041> endbfchar";
        let base = b"1 begincodespacerange <0000> <FFFF> endcodespacerange";
        let own = b"1 begincidrange <0041> <0042> 10 endcidrange";
        let objects = file(vec![
            Stream::new(dict! {}, to_unicode.to_vec()).into(),
            Stream::new(dict! { "Type" => "CMap" }, base.to_vec()).into(),
            Stream::new(dict! { "UseCMap" => Object::reference(2) }, own.to_vec()).into(),
            dict! { "Subtype" => "CIDFontType2", "DW" => 500 }.into(),
        ]);
        let font = dict! {
            "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Foo",
            "Encoding" => Object::reference(3), "DescendantFonts" => vec![Object::reference(4)],
            "ToUnicode" => Object::reference(1),
        };
        let made = spans(|| {
            let font = read(&objects, &font, true);
            assert_eq!(font.cid(0x42), Some(11));
            assert_eq!(font.text(b"\0A").as_deref(), Some("A"));
        });
        let expected = [
            ("read", Level::DEBUG, vec![("programs", 1)]),
            (
                "to_unicode",
                Level::DEBUG,
                vec![("bytes", to_unicode.len() as u64)],
            ),
            ("cmap", Level::DEBUG, vec![("streams", 2)]),
        ];
        assert_eq!(named(&made), expected);
    }

    /// A Type 3 font's CharProcs are a span, with how many glyphs its `/CharProcs`
    /// names, each stream read once however many name it.
    #[test]
    fn a_type3_font_s_char_procs_are_a_span() {
        let proc = b"100 0 0 0 100 100 d1 0 0 100 100 re f";
        let objects = file(vec![
            Stream::new(dict! {}, proc.to_vec()).into(),
            Stream::new(dict! {}, proc.to_vec()).into(),
        ]);
        let one = Object::reference(1);
        let matrix = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0]
            .map(Object::from)
            .to_vec();
        let differences = vec![97.into(), "a".into(), "b".into(), "c".into()];
        let font = dict! {
            "Type" => "Font", "Subtype" => "Type3", "FontMatrix" => matrix,
            "Encoding" => dict! { "Differences" => differences },
            "CharProcs" => dict! { "a" => one.clone(), "b" => one, "c" => Object::reference(2) },
        };
        let made = spans(|| {
            let font = read(&objects, &font, true);
            assert_eq!(font.width(u32::from(b'c')), 100.0);
            assert_eq!(font.cost(), 2 * proc.len());
        });
        let expected = [
            ("read", Level::DEBUG, vec![("programs", 1)]),
            ("char_procs", Level::DEBUG, vec![("glyphs", 3)]),
        ];
        assert_eq!(named(&made), expected);
    }

    /// A font's program read and opened is a span, with how many bytes it
    /// decompressed to, where a program is read: with `programs`, and in
    /// [`Font::read`] alone.
    #[test]
    fn a_program_read_is_a_span() {
        let program = crate::fixture::truetype_program();
        let objects = file(vec![
            Stream::new(dict! {}, program.clone()).into(),
            dict! { "Flags" => 32, "FontFile2" => Object::reference(1) }.into(),
        ]);
        let font = dict! {
            "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "Foo",
            "FontDescriptor" => Object::reference(2), "Encoding" => "WinAnsiEncoding",
        };
        let made = spans(|| {
            let read = read(&objects, &font, true);
            assert_eq!(read.outlined(), cfg!(feature = "programs"));
        });
        let mut expected = vec![("read", Level::DEBUG, vec![("programs", 1)])];
        if cfg!(feature = "programs") {
            expected.push((
                "program",
                Level::DEBUG,
                vec![("bytes", program.len() as u64)],
            ));
        }
        assert_eq!(named(&made), expected);
        let made = spans(|| assert!(!read(&objects, &font, false).outlined()));
        assert_eq!(
            named(&made),
            [("read", Level::DEBUG, vec![("programs", 0)])]
        );
    }

    /// What's asked of a font read has none, its loop being the caller's: its codes,
    /// widths, text and boxes, and the outlines a string draws charged; nor has a
    /// glyph's name read, an encoding or a standard 14 font named.
    #[test]
    fn what_s_asked_of_a_font_and_the_lookups_are_none() {
        let objects = file(vec![
            Stream::new(dict! {}, crate::fixture::truetype_program()).into(),
            dict! { "Flags" => 32, "FontFile2" => Object::reference(1) }.into(),
        ]);
        let font = dict! {
            "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "Foo",
            "FontDescriptor" => Object::reference(2), "Encoding" => "WinAnsiEncoding",
        };
        let font = read(&objects, &font, true);
        let made = spans(|| {
            assert_eq!(font.codes(b"AB").count(), 2);
            let _ = (
                font.width(65),
                font.cid(65),
                font.vertical(65),
                font.char_box(65),
            );
            let _ = (font.code_text(65), font.text(b"AB"), font.glyph_box(65));
            let mut budget = 1 << 20;
            assert!(font.charge_outlines(b"AB", |_| true, &mut budget).is_ok());
            assert_eq!(glyph_text("f_i", false).as_deref(), Some("fi"));
            assert_eq!(name_code_point(b"uni0041"), Some((0x41, false)));
            assert_eq!(Std14::named(b"Arial,Bold"), Some(Std14::HelveticaBold));
            assert_eq!(Encoding::named(b"WinAnsiEncoding"), Some(Encoding::WinAnsi));
        });
        assert_eq!(named(&made), []);
    }
}
