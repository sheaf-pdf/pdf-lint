//! The kind of form a document holds: none, an AcroForm, or XFA beside or in
//! place of one.

use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, Resolve};

use crate::error::Error;
use crate::limits::Limits;
use crate::read::Reader;

/// The kind of interactive form a document holds, as a viewer tells them
/// apart before it offers to fill one: what PDFium's `FPDF_GetFormType`
/// answers, read from the catalog's `/AcroForm`, its `/XFA`, and the
/// catalog's `/NeedsRendering` (ISO 32000-1 §12.7.8, deprecated by
/// ISO 32000-2).
///
/// A host says different things for each. An [`AcroForm`](Self::AcroForm) is
/// filled through [`Form`](crate::Form). With
/// [`XfaForeground`](Self::XfaForeground) the fields are there as an AcroForm
/// too, and a change made to them alone leaves the XFA's copy of the data
/// behind. With [`XfaFull`](Self::XfaFull) the pages are a placeholder
/// ("Please wait...") for a form only an XFA processor draws, so there's
/// nothing to fill as an AcroForm.
///
/// ```
/// use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
/// use std::collections::HashMap;
///
/// use pdf_forms::pdf_objects::{Object, ObjectId, dict};
/// use pdf_forms::{FormKind, Limits};
///
/// let none = HashMap::<ObjectId, Object>::new();
/// let read = |catalog| FormKind::read(&none, catalog, Rules::Pdfium, &Limits::DEFAULT).unwrap();
///
/// let bare = dict! { "Type" => Object::name("Catalog") };
/// assert_eq!(read(&bare), FormKind::None);
/// let acro = dict! { "AcroForm" => dict! { "Fields" => Vec::<Object>::new() } };
/// assert_eq!(read(&acro), FormKind::AcroForm);
/// let beside = dict! { "AcroForm" => dict! { "XFA" => Vec::<Object>::new() } };
/// assert_eq!(read(&beside), FormKind::XfaForeground);
/// let whole = dict! {
///     "AcroForm" => dict! { "XFA" => Vec::<Object>::new() },
///     "NeedsRendering" => true,
/// };
/// assert_eq!(read(&whole), FormKind::XfaFull);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum FormKind {
    /// No form: the catalog has no `/AcroForm` dictionary. PDFium's
    /// `FORMTYPE_NONE`.
    #[default]
    None,
    /// An AcroForm and no XFA. PDFium's `FORMTYPE_ACRO_FORM`.
    AcroForm,
    /// XFA beside an AcroForm: the form dictionary has `/XFA`, and the pages
    /// and fields are a PDF's own. PDFium's `FORMTYPE_XFA_FOREGROUND`.
    XfaForeground,
    /// XFA that is the whole form: the form dictionary has `/XFA` and the
    /// catalog's `/NeedsRendering` is true, so the pages are to be made from
    /// the XFA and what the file holds of them is a placeholder. PDFium's
    /// `FORMTYPE_XFA_FULL`.
    XfaFull,
}

impl FormKind {
    /// The kind of form the document whose catalog is `catalog` holds.
    ///
    /// The form dictionary is found as [`Form::read`](crate::Form::read) finds
    /// it, so this is [`FormKind::None`] exactly where that reads no form.
    /// What counts as `/XFA` and as a true `/NeedsRendering` goes by `rules`:
    ///
    /// - [`Rules::Pdfium`], as of chromium/8152: an `/XFA` entry counts
    ///   whatever it holds, `null` and a reference to no object among them;
    ///   `/NeedsRendering` counts only as the boolean `true` written in the
    ///   catalog itself, not one reached by reference.
    /// - [`Rules::Iso`]: an `/XFA` that is `null`, or a reference to `null` or
    ///   to no object, is no entry (ISO 32000-2 §7.3.9), and
    ///   `/NeedsRendering` is read through a reference as any entry is.
    ///
    /// By either, the last of an entry written twice is the one read.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unreadable`] on locked, I/O, or reader-cap failures.
    pub fn read<'o, R: Resolve<'o>>(
        objects: R,
        catalog: &'o Dict,
        rules: Rules,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let mut reader = Reader {
            objects,
            rules,
            limits,
            sink: &mut (),
        };
        let Some((form, id)) = reader.dict_entry(catalog, None, "AcroForm")? else {
            return Ok(Self::None);
        };
        let pdfium = rules == Rules::Pdfium;
        let has_xfa = if pdfium {
            form.get("XFA").is_some()
        } else {
            reader.entry(form, id, "XFA")?.is_some()
        };
        if !has_xfa {
            return Ok(Self::AcroForm);
        }
        let needs_rendering = if pdfium {
            catalog.get("NeedsRendering").and_then(Object::as_bool)
        } else {
            reader
                .entry(catalog, None, "NeedsRendering")?
                .and_then(|(object, _)| object.as_bool())
        };
        Ok(if needs_rendering == Some(true) {
            Self::XfaFull
        } else {
            Self::XfaForeground
        })
    }

    /// Whether the document holds XFA, beside an AcroForm or as the whole
    /// form.
    #[must_use]
    pub const fn has_xfa(self) -> bool {
        matches!(self, Self::XfaForeground | Self::XfaFull)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pdf_objects::{ObjectId, Unreadable, Why, dict};

    use super::*;

    fn kinds(catalog: &Dict, objects: &HashMap<ObjectId, Object>) -> (FormKind, FormKind) {
        let read = |rules| FormKind::read(objects, catalog, rules, &Limits::DEFAULT).unwrap();
        (read(Rules::Pdfium), read(Rules::Iso))
    }

    fn both(catalog: &Dict, objects: &HashMap<ObjectId, Object>) -> FormKind {
        let (pdfium, iso) = kinds(catalog, objects);
        assert_eq!(pdfium, iso);
        pdfium
    }

    fn id(n: u32) -> ObjectId {
        ObjectId::new(n, 0)
    }

    #[test]
    fn the_four_kinds_are_told_apart_by_either_rules() {
        let none = HashMap::new();
        let xfa = || dict! { "XFA" => Vec::<Object>::new() };
        assert_eq!(both(&dict! {}, &none), FormKind::None);
        assert_eq!(
            both(&dict! { "NeedsRendering" => true }, &none),
            FormKind::None
        );
        assert_eq!(
            both(&dict! { "AcroForm" => dict! {} }, &none),
            FormKind::AcroForm
        );
        assert_eq!(
            both(
                &dict! { "AcroForm" => dict! {}, "NeedsRendering" => true },
                &none
            ),
            FormKind::AcroForm
        );
        assert_eq!(
            both(&dict! { "AcroForm" => xfa() }, &none),
            FormKind::XfaForeground
        );
        assert_eq!(
            both(
                &dict! { "AcroForm" => xfa(), "NeedsRendering" => false },
                &none
            ),
            FormKind::XfaForeground
        );
        assert_eq!(
            both(
                &dict! { "AcroForm" => xfa(), "NeedsRendering" => true },
                &none
            ),
            FormKind::XfaFull
        );
        // What isn't a boolean isn't true.
        assert_eq!(
            both(&dict! { "AcroForm" => xfa(), "NeedsRendering" => 1 }, &none),
            FormKind::XfaForeground
        );
        assert!(FormKind::XfaFull.has_xfa() && FormKind::XfaForeground.has_xfa());
        assert!(!FormKind::AcroForm.has_xfa() && !FormKind::None.has_xfa());
        assert_eq!(FormKind::default(), FormKind::None);
    }

    #[test]
    fn a_form_by_reference_is_read_and_one_that_is_no_dictionary_is_none() {
        let objects = HashMap::from([
            (id(5), Object::from(dict! { "XFA" => Vec::<Object>::new() })),
            (id(6), Object::from(7)),
        ]);
        let by_reference = dict! { "AcroForm" => Object::reference(5), "NeedsRendering" => true };
        assert_eq!(both(&by_reference, &objects), FormKind::XfaFull);
        for no_form in [
            Object::reference(6),
            Object::reference(9),
            Object::null(),
            Object::from(1),
        ] {
            assert_eq!(
                both(&dict! { "AcroForm" => no_form }, &objects),
                FormKind::None
            );
        }
    }

    #[test]
    fn a_null_xfa_and_a_referenced_needs_rendering_go_by_the_rules() {
        let objects = HashMap::from([(id(5), Object::null()), (id(6), Object::from(true))]);
        // `/XFA null`, `/XFA` to null and `/XFA` to nothing: an entry to
        // PDFium, none to ISO 32000-2.
        for xfa in [Object::null(), Object::reference(5), Object::reference(9)] {
            let catalog = dict! { "AcroForm" => dict! { "XFA" => xfa }, "NeedsRendering" => true };
            assert_eq!(
                kinds(&catalog, &objects),
                (FormKind::XfaFull, FormKind::AcroForm)
            );
        }
        // `/NeedsRendering` by reference: not followed by PDFium.
        let catalog = dict! {
            "AcroForm" => dict! { "XFA" => Vec::<Object>::new() },
            "NeedsRendering" => Object::reference(6),
        };
        assert_eq!(
            kinds(&catalog, &objects),
            (FormKind::XfaForeground, FormKind::XfaFull)
        );
    }

    #[test]
    fn an_object_that_doesnt_read_is_an_error_not_no_form() {
        struct Locked<'o>(std::marker::PhantomData<&'o ()>);
        impl<'o> Resolve<'o> for Locked<'o> {
            fn resolve(&self, _: ObjectId) -> Option<&'o Object> {
                None
            }
            fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
                Err(Unreadable::new(id, Why::Locked, "it takes a password"))
            }
        }
        let catalog = dict! { "AcroForm" => Object::reference(5) };
        for rules in [Rules::Pdfium, Rules::Iso] {
            let read = FormKind::read(
                Locked(std::marker::PhantomData),
                &catalog,
                rules,
                &Limits::DEFAULT,
            );
            assert!(matches!(read, Err(Error::Unreadable(u)) if u.why == Why::Locked));
        }
    }
}
