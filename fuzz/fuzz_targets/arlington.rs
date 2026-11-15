//! The model's loader and its predicates' evaluator, on any bytes: read as
//! tables' files, a piece each between `\0`s, its first line the table's
//! name, loaded as a folder of them is: a model that doesn't load saying
//! which file, and a line within it; one that loads the same from its files
//! in any order, its hash of the framing its doc gives, each row where
//! `Table::row` says, each predicate in its cells parsing or saying why
//! not, one that parses printing back as a text that parses to it, and each
//! row's cells, asked over an object of its table's keys, the same through
//! a context given the model and one given none, and asked again; and read
//! as a predicate's text, three bytes of choices and a series of objects,
//! the container and the trailer first, the text parsing so, and every one
//! of the evaluator's functions, at every key and element of the container,
//! over a resolver failing the objects the input marks for the reason it
//! names, saying the same through a context given a model of the body's own
//! and one given none, each asked twice, every unknown saying why, and a key
//! a name tree holds never said to be missing where some reads fail.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    pdf_arlington::fuzz::tables(data);
    pdf_arlington::fuzz::predicates(data);
});
