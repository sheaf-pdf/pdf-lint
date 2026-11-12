//! pdf-semantics' writers timed on inputs made here, so that a change to
//! one can be judged before and after it, by `cargo bench -p pdf-semantics
//! --bench write`:
//!
//! - `xmp-write`: `xmp::write` of one claim (`one-claim`); of every claim
//!   (`every-claim`); of a claim whose value is about 64 KiB of text with
//!   nothing to escape (`plain-64k`); and of one as long, nearly half its
//!   characters escaped, `&`, `<`, `>` and carriage returns (`escaped-64k`).
//! - `pages-write`: `pages::write` of a tree under nodes of `pages::FANOUT`
//!   over 7,000 pages (`balanced-7000`), and over 1,048,574, the most PDFium
//!   counts (`balanced-1048574`), its nodes numbered past the pages; each
//!   tree written dropped in the time. So each write after the first reuses
//!   memory the one before freed, and takes half the time of a process's
//!   first write, or less.
//!
//! Throughput is in bytes of the packet written, and in pages. Each input is
//! held to what it gives before it's timed: its packet, read back by
//! `xmp::read`, to the claims written, with no fault; its tree, read back by
//! `pages::pages` by both rules, each page `/Type /Page` with its `/Parent`
//! and a `/MediaBox`, with no fault, every page once and in order. Nothing
//! is read from disk, and the inputs are the same on every run.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdf_semantics::pages::{self, FANOUT, pages};
use pdf_semantics::pdf_objects::fault::{Place, Sink};
use pdf_semantics::pdf_objects::pdf_tokenizer::Rules;
use pdf_semantics::pdf_objects::{Object, ObjectId, dict};
use pdf_semantics::xmp::{Claims, read, write};
use pdf_semantics::{Fault, Limits};

/// What reading a packet back tells, kept.
#[derive(Default)]
struct Faults(Vec<(Place, Fault)>);

impl Sink<Fault> for Faults {
    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.push((at, fault));
    }
}

/// A value of about 64 KiB, of text `piece` again and again, with no
/// white space round it, which a value written can't have.
fn long(piece: &str) -> String {
    let text = piece.repeat((64 << 10) / piece.len());
    text.trim_ascii().to_string()
}

/// The claims written, by name.
fn claims() -> Vec<(&'static str, Claims)> {
    let mut one = Claims::default();
    one.pdfa_part = Some("3".into());
    let mut every = Claims::default();
    every.pdfa_part = Some("2".into());
    every.pdfa_conformance = Some("B".into());
    every.pdfa_rev = Some("2011".into());
    every.pdfua_part = Some("1".into());
    every.pdfua_rev = Some("2014".into());
    every.pdfvt_version = Some("PDF/VT-2".into());
    let mut plain = Claims::default();
    plain.pdfvt_version = Some(long("plain text, nothing to escape. "));
    let mut escaped = Claims::default();
    escaped.pdfvt_version = Some(long("a&b<c>d\re\r\nf&g<h>"));
    vec![
        ("one-claim", one),
        ("every-claim", every),
        ("plain-64k", plain),
        ("escaped-64k", escaped),
    ]
}

fn xmp_write_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("xmp-write");
    for (name, claims) in claims() {
        let packet = write(&claims).unwrap_or_else(|e| panic!("xmp-write/{name}: {e}"));
        let mut faults = Faults::default();
        let read_back = read(packet.as_bytes(), None, &Limits::DEFAULT, &mut faults);
        let read_back = read_back.unwrap_or_else(|e| panic!("xmp-write/{name}: {e}"));
        assert_eq!(read_back, claims, "xmp-write/{name}: read back");
        assert!(faults.0.is_empty(), "xmp-write/{name}: no fault");
        group.throughput(Throughput::Bytes(packet.len() as u64));
        group.bench_function(name, |b| b.iter(|| write(black_box(&claims))));
    }
    group.finish();
}

/// `count` pages, objects 1 on, and the tree `pages::write` writes over
/// them under nodes of `FANOUT`, its nodes numbered from past the last page.
fn tree_over(count: u32) -> (Vec<ObjectId>, pages::Written) {
    let pages: Vec<ObjectId> = (1..=count).map(|n| ObjectId::new(n, 0)).collect();
    let mut next = count + 1;
    let ids = || {
        next += 1;
        ObjectId::new(next - 1, 0)
    };
    let written = pages::write(&pages, FANOUT, &Limits::DEFAULT, ids);
    (
        pages,
        written.unwrap_or_else(|e| panic!("pages-write/{count}: {e}")),
    )
}

/// The tree `written` over `listed`, read back by both rules, with no fault:
/// every page once and in order.
fn held_tree(name: &str, listed: &[ObjectId], written: &pages::Written) {
    let mut objects: BTreeMap<ObjectId, Object> = (written.nodes.iter())
        .map(|(id, node)| (*id, Object::from(node.clone())))
        .collect();
    for (&page, &parent) in listed.iter().zip(&written.parents) {
        let media = vec![
            Object::from(0),
            Object::from(0),
            Object::from(612),
            Object::from(792),
        ];
        let dict = dict! { "Type" => "Page", "Parent" => parent, "MediaBox" => media };
        objects.insert(page, Object::from(dict));
    }
    let resolve = |id: ObjectId| objects.get(&id);
    let catalog = dict! { "Pages" => written.root };
    for rules in [Rules::Iso, Rules::Pdfium] {
        let mut faults = Faults::default();
        let read = pages(resolve, &catalog, rules, &Limits::DEFAULT, &mut faults);
        let read = read.unwrap_or_else(|e| panic!("pages-write/{name}: {rules:?}: {e}"));
        assert!(
            faults.0.is_empty(),
            "pages-write/{name}: {rules:?}: no fault"
        );
        assert_eq!(
            read.counted(),
            listed.len(),
            "pages-write/{name}: {rules:?}"
        );
        assert!(
            read.iter()
                .map(|page| page.id)
                .eq(listed.iter().copied().map(Some)),
            "pages-write/{name}: {rules:?}: every page once, in order"
        );
    }
}

fn pages_write_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("pages-write");
    for count in [7000, 1_048_574] {
        let name = format!("balanced-{count}");
        let (listed, written) = tree_over(count);
        held_tree(&name, &listed, &written);
        drop(written);
        group.throughput(Throughput::Elements(u64::from(count)));
        group.bench_function(&name, |b| {
            b.iter(|| {
                let mut next = count + 1;
                let ids = || {
                    next += 1;
                    ObjectId::new(next - 1, 0)
                };
                pages::write(black_box(&listed), FANOUT, &Limits::DEFAULT, ids)
            })
        });
    }
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    xmp_write_group(&mut criterion);
    pages_write_group(&mut criterion);
    criterion.final_summary();
}
