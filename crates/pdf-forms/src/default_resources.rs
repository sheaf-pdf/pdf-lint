//! A form's default resources (`/AcroForm /DR`, ISO 32000-2, 12.7.3), as a
//! writer adds to them: the name a font is put under, so that a default
//! appearance string finds it (12.7.4.3).

use std::borrow::Cow;
use std::collections::HashMap;

use pdf_objects::{Dict, Object, ObjectId};

/// The name to put the font object `font` under in `fonts`, the font
/// dictionary of a form's default resources (`/AcroForm /DR /Font`), for a
/// default appearance string to name it by (ISO 32000-2, 12.7.4.3).
///
/// It's `wanted` where `fonts` has no entry by that name, or one that's
/// `null` (as an entry that isn't there reads, 7.3.7), or a reference to
/// `font` itself; else `wanted` with a number after it, the first from 1
/// whose name is free so (`Helv1` for `Helv`, `Helv2` where that's taken
/// too). With no font dictionary yet, it's `wanted`. A name a writer puts a
/// font under this way never takes another font's place, so a field or an
/// annotation whose `/DA` names that one still draws in it.
///
/// It reads `fonts` and changes nothing: the caller sets the entry, and
/// where the name isn't `wanted`, names it in the default appearance string
/// it writes. pdfs' `Document::add_annot` does both for an annotation built
/// in a font of its own. The work is linear in `fonts`, the keys read once
/// however many names are tried: of the first `fonts.len() + 1` numbers one
/// is free.
///
/// ```
/// use pdf_forms::default_resources::font_name;
/// use pdf_forms::pdf_objects::{Object, ObjectId, dict};
///
/// let (helvetica, embedded) = (ObjectId::new(7, 0), ObjectId::new(12, 0));
/// let fonts = dict! { "Helv" => helvetica, "Helv1" => Object::reference(8) };
/// // `Helv` and `Helv1` are other fonts': the next number.
/// assert_eq!(font_name(Some(&fonts), b"Helv", embedded), &b"Helv2"[..]);
/// // A name free, or the font's own already, is kept.
/// assert_eq!(font_name(Some(&fonts), b"F1", embedded), &b"F1"[..]);
/// assert_eq!(font_name(Some(&fonts), b"Helv", helvetica), &b"Helv"[..]);
/// assert_eq!(font_name(None, b"Helv", embedded), &b"Helv"[..]);
/// ```
#[must_use]
pub fn font_name<'a>(fonts: Option<&Dict>, wanted: &'a [u8], font: ObjectId) -> Cow<'a, [u8]> {
    let free = |entry: Option<&Object>| {
        entry.is_none_or(|entry| entry.is_null() || entry.as_reference() == Some(font))
    };
    let Some(fonts) = fonts else {
        return Cow::Borrowed(wanted);
    };
    if free(fonts.get(wanted)) {
        return Cow::Borrowed(wanted);
    }
    // Each key once, by its last value, as a lookup reads it: one lookup a
    // name tried, and the loop ends within `fonts.len() + 1` of them.
    let taken: HashMap<&[u8], &Object> = fonts.iter().collect();
    let mut name = wanted.to_vec();
    let mut n: u64 = 0;
    loop {
        n += 1;
        name.truncate(wanted.len());
        name.extend_from_slice(n.to_string().as_bytes());
        if free(taken.get(&name[..]).copied()) {
            return Cow::Owned(name);
        }
    }
}
