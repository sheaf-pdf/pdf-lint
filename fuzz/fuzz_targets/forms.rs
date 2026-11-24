//! The field tree read from arbitrary bytes (`pdf_forms::fuzz::check`),
//! checking fault-sink invariance, the tree's shape, its caps, and `Debug`
//! hygiene, reading its fields' options and selections, pressing each button
//! field's widgets, and making the appearance of each widget of its text,
//! button, and choice fields; the same bytes cut into a button group and its
//! presses, and into a check box, a radio button, a push button, a combo
//! box, and a list box, each made (`pdf_forms::fuzz::check_group`,
//! `check_buttons`, and `check_choices`, which `check` calls);
//! the same bytes cut into a text field's text, flags, box, border, colors,
//! rotation, and `/DA`, its appearance made in each standard 14 font, and,
//! where the bytes carry a TrueType program that opens, in each font and
//! the program as a fallback (`pdf_forms::fuzz::check_appearance`, which
//! `check` calls); and the form read from a PDF file's catalog
//! (`pdf_forms::fuzz::over`).

#![no_main]

use pdf_forms::fuzz::{check, over};
use pdf_structure::fuzz::options;
use pdf_structure::read::Reading;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    check(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        over(&reading, reading.trailer().dict());
    }
});
