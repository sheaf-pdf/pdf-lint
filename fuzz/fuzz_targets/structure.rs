//! Reading a file from its bytes, on any bytes, under small limits: no
//! panic, the same twice, every place it gives inside what was read; read
//! with no faults looked for, in memory and by position, the same read,
//! keeping none; read on demand, in an order the input chooses, the whole
//! read, where neither decodes as far as the budget, and made a document
//! with the reading kept, at a point the input chooses, the reading's
//! whole read after; read by position, from a first window the input
//! chooses, the whole read, each stream's data and decoded data as in
//! memory; pruned, what it reaches the same, and an object added at a
//! number none held; and its revision before an update, and the file
//! opened by the owner's password, as the input's first bytes choose, the
//! same, and opened by a wrong password, then unlocked by the owner's, the
//! file opened by the owner's. A resolver says of each object what `get`
//! says, in memory and by position, and counts no failed read. Each
//! revision ends past a `%%EOF` and its line's end, and the file as each
//! was written, its bytes up to there, reads the same twice and by
//! position as in memory.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_structure::fuzz::read_file(data));
