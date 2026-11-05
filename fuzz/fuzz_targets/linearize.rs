//! A plan of any graph: the input's bytes as a graph, of few objects and pages
//! with references, stops and groups among them, refused for what it has or
//! planned, each object placed once and numbered as Annex F has it, each page's
//! list of shared objects what a plain walk of it finds, and each cap holding at
//! its edge. Then assembled, with its streams as they are and with a first-page
//! stream wanting more room: its positions what its pieces add up to, its hint
//! tables, read back, what the plan says. And the same bytes as a hint stream, read
//! under small caps at any positions: an error, or tables that write and read the
//! same. And the same bytes as the facts of a file: the same verdict whatever the
//! sink. And the same bytes as a file, read, laid out in the three kinds and read
//! back: pages and objects the same under the numbering, the check of it clean, or
//! `out` as it was.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    pdf_linearize::fuzz::graphs(data);
    pdf_linearize::fuzz::tables(data);
    pdf_linearize::fuzz::facts(data);
    pdf_linearize::fuzz::over(data);
});
