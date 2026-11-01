//! A plan of any graph: the input's bytes as a graph, of few objects and pages
//! with references, stops and groups among them, refused for what it has or
//! planned, each object placed once and numbered as Annex F has it, each page's
//! list of shared objects what a plain walk of it finds, and each cap holding at
//! its edge.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    pdf_linearize::fuzz::graphs(data);
});
