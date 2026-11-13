//! A plan of any graph: the input's bytes as a graph, of few objects and pages
//! with references, stops and groups among them, refused for what it has or
//! planned, each object placed once and numbered as Annex F has it, each page's
//! list of shared objects what a plain walk of it finds, and each cap holding at
//! its edge. Then assembled, with its streams as they are and with a first-page
//! stream wanting more room: its positions what its pieces add up to, its hint
//! tables, read back, what the plan says. And the same bytes as a hint stream, read
//! under small caps at any positions: an error, or tables that write and read the
//! same. And the same bytes as the facts of a file: the same verdict whatever the
//! sink, and of what they say wasn't read, one finding that says so and nothing
//! judged that depends on it. And the same bytes as a file, read and laid out in
//! the three kinds, in the clear, under its own encryption as it is and told to
//! reuse RC4's keystreams, and now and then under a new one, then read back:
//! pages and objects the same under the numbering, a direct `/Info` an object
//! where it's encrypted, the check of it clean, and each object listed as written
//! at a keystream used one it wrote, or `out` as it was. Under its own encryption,
//! what's refused at a keystream used is written when told to reuse them, and
//! listed, and what both write is the same bytes. And the same file under caps
//! on its trees the input picks: refused where its page tree is past them, and
//! what it wrote in the clear checked under them, its page tree told as not
//! judged where it's past them, and read as at the defaults where it isn't.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    pdf_linearize::fuzz::graphs(data);
    pdf_linearize::fuzz::tables(data);
    pdf_linearize::fuzz::facts(data);
    pdf_linearize::fuzz::over(data);
});
