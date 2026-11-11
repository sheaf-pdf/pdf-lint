//! pdf-semantics' writers timed on inputs made here, so that a change to
//! one can be judged before and after it, by `cargo bench -p pdf-semantics
//! --bench write`:
//!
//! - `xmp-write`: `xmp::write` of one claim (`one-claim`); of every claim
//!   (`every-claim`); of a claim whose value is about 64 KiB of text with
//!   nothing to escape (`plain-64k`); and of one as long, nearly half its
//!   characters escaped, `&`, `<`, `>` and carriage returns (`escaped-64k`).
//!
//! Throughput is in bytes of the packet written. Each input is held to
//! what it gives before it's timed: its packet, read back by `xmp::read`,
//! to the claims written, with no fault. Nothing is read from disk, and
//! the inputs are the same on every run.

use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdf_semantics::pdf_objects::fault::{Place, Sink};
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

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    xmp_write_group(&mut criterion);
    criterion.final_summary();
}
