//! What a file lets one who opens it with the user's password do: the
//! permissions its `/P` grants (ISO 32000-2, 7.6.4.2, Table 22), and the
//! levels of printing and of changes a security dialog offers.

use std::fmt;
use std::ops::BitOr;

/// The permissions a file grants one who opens it with the user's password:
/// a set of the eight that `/P` has a bit for (ISO 32000-2, 7.6.4.2, Table
/// 22). The owner's password opens a file with every one, whatever the set
/// says (7.6.4.1).
///
/// A set is made from the constants, joined by `|`
/// ([`Permissions::union`]), or from a security dialog's levels
/// ([`Printing`] and [`Changes`], by `From`); it's read from a file's `/P`
/// by [`Permissions::from_p`], and written as one by [`Permissions::p`].
///
/// The handler says them; honouring them is a reader's. One who can open a
/// file, by the user's password or by none where that's empty, can decrypt
/// all of it, whatever the set withholds.
///
/// Its `Debug` names the bits, `Permissions(PRINT | COPY)`, and its
/// `Display` names them in words, `print, copy`; the empty set is
/// `Permissions(NONE)` and `none`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Permissions(u32);

impl Permissions {
    /// Printing (bit 3), at low resolution unless
    /// [`Permissions::PRINT_HIGH`] is granted too (from revision 3).
    pub const PRINT: Permissions = Permissions(1 << 2);
    /// Changing the document's content by what the other bits don't cover
    /// (bit 4); beside [`Permissions::ANNOTATE`], creating and changing form
    /// fields too.
    pub const MODIFY: Permissions = Permissions(1 << 3);
    /// Copying text and graphics out of the document, or taking them out
    /// any other way (bit 5). PDF 2.0 asks a reader, without binding it, to
    /// treat this as granted where it hands them to assistive technology.
    pub const COPY: Permissions = Permissions(1 << 4);
    /// Adding and changing text annotations (bit 6), and filling in form
    /// fields, which it grants too, whatever
    /// [`Permissions::FILL_FORMS`] says: a reader asking whether a form may
    /// be filled in reads either.
    pub const ANNOTATE: Permissions = Permissions(1 << 5);
    /// Filling in the form fields a document has, signature fields among
    /// them (bit 9, from revision 3), where [`Permissions::ANNOTATE`]
    /// isn't granted, which grants it too.
    pub const FILL_FORMS: Permissions = Permissions(1 << 8);
    /// Taking content out for assistive technology (bit 10). Unused from
    /// PDF 2.0, where a reader is to pass over it (and is asked, though not
    /// bound, to serve assistive technology as though
    /// [`Permissions::COPY`] were granted), and a writer to set it
    /// whatever's meant: a new file's `/P` has it
    /// ([`Handler::create`](crate::Handler::create)).
    pub const ACCESSIBILITY: Permissions = Permissions(1 << 9);
    /// Assembling the document (bit 11, from revision 3): adding, removing
    /// and turning its pages, and making its bookmarks (outline items) and
    /// page thumbnails, where [`Permissions::MODIFY`] isn't granted.
    pub const ASSEMBLE: Permissions = Permissions(1 << 10);
    /// Printing at high resolution (bit 12, from revision 3): output a true
    /// copy of the content could be made from. Without it, beside
    /// [`Permissions::PRINT`], printing is at low resolution, the pages'
    /// look perhaps coarser.
    pub const PRINT_HIGH: Permissions = Permissions(1 << 11);
    /// None of them.
    pub const NONE: Permissions = Permissions(0);
    /// Every one: what a file whose `/P` is -4 grants.
    pub const ALL: Permissions = Permissions(0xF3C);

    /// The permissions, by name, in the order of their bits.
    const NAMED: [(Permissions, &'static str, &'static str); 8] = [
        (Permissions::PRINT, "PRINT", "print"),
        (Permissions::MODIFY, "MODIFY", "modify"),
        (Permissions::COPY, "COPY", "copy"),
        (Permissions::ANNOTATE, "ANNOTATE", "annotate"),
        (Permissions::FILL_FORMS, "FILL_FORMS", "fill forms"),
        (Permissions::ACCESSIBILITY, "ACCESSIBILITY", "accessibility"),
        (Permissions::ASSEMBLE, "ASSEMBLE", "assemble"),
        (
            Permissions::PRINT_HIGH,
            "PRINT_HIGH",
            "print at high resolution",
        ),
    ];

    /// The bits `/P` has set past the eight: 7 and 8, and 13 to 32.
    const RESERVED: u32 = 0xFFFF_F0C0;

    /// The permissions `/P` grants in a dictionary of revision `r`, by that
    /// revision's meaning of its bits (ISO 32000-2, 7.6.4.2, Table 22):
    /// `/P`'s low 32 bits, so that -4 and 4294967292 read alike, and of
    /// those the eight, each as written from revision 3. Revision 2 (and
    /// below, which no handler opens) has four of them alone, each covering
    /// what a later revision has a bit of its own for: there filling in
    /// forms follows annotating, accessibility copying, assembling
    /// modifying, and printing at high resolution printing. Every other
    /// bit is ignored, as a reader ignores it.
    #[must_use]
    pub const fn from_p(p: i64, r: i64) -> Permissions {
        let bits = p as u32 & Permissions::ALL.0;
        if r >= 3 {
            return Permissions(bits);
        }
        let mut set = Permissions(bits & 0x3C);
        let covered = [
            (Permissions::ANNOTATE, Permissions::FILL_FORMS),
            (Permissions::COPY, Permissions::ACCESSIBILITY),
            (Permissions::MODIFY, Permissions::ASSEMBLE),
            (Permissions::PRINT, Permissions::PRINT_HIGH),
        ];
        let mut i = 0;
        while i < covered.len() {
            let (by, covers) = covered[i];
            if set.contains(by) {
                set = set.union(covers);
            }
            i += 1;
        }
        set
    }

    /// `/P` for these permissions, as a dictionary of revision 3 or later
    /// writes it (ISO 32000-2, 7.6.4.2, Table 22): bits 1 and 2 clear, 7, 8
    /// and 13 to 32 set, and the rest as the set has them; as a 32-bit
    /// integer, so negative. Every permission is -4. Bit 10 is written as
    /// the set has it too, though a PDF 2.0 writer is to set it whatever's
    /// meant ([`Permissions::ACCESSIBILITY`]), as
    /// [`Handler::create`](crate::Handler::create) puts it back.
    #[must_use]
    pub const fn p(self) -> i64 {
        (self.0 | Permissions::RESERVED) as i32 as i64
    }

    /// Whether it has every permission of `other`.
    #[must_use]
    pub const fn contains(self, other: Permissions) -> bool {
        self.0 & other.0 == other.0
    }

    /// The permissions of both.
    #[must_use]
    pub const fn union(self, other: Permissions) -> Permissions {
        Permissions(self.0 | other.0)
    }

    /// The permissions of this set that `other` hasn't.
    #[must_use]
    pub const fn without(self, other: Permissions) -> Permissions {
        Permissions(self.0 & !other.0)
    }

    /// How it lets a user print: at high resolution with
    /// [`Permissions::PRINT`] and [`Permissions::PRINT_HIGH`], at low
    /// resolution with the first alone, and not at all without it, as the
    /// second qualifies the first.
    #[must_use]
    pub const fn printing(self) -> Printing {
        match (
            self.contains(Permissions::PRINT),
            self.contains(Permissions::PRINT_HIGH),
        ) {
            (true, true) => Printing::High,
            (true, false) => Printing::Low,
            (false, _) => Printing::Denied,
        }
    }

    /// The permissions' names in this set, in the order of their bits.
    fn named(self) -> impl Iterator<Item = (&'static str, &'static str)> {
        (Permissions::NAMED.into_iter())
            .filter(move |(permission, ..)| self.contains(*permission))
            .map(|(_, name, words)| (name, words))
    }
}

impl BitOr for Permissions {
    type Output = Permissions;

    fn bitor(self, other: Permissions) -> Permissions {
        self.union(other)
    }
}

impl fmt::Debug for Permissions {
    /// `Permissions(PRINT | COPY)`, the bits by their constants' names;
    /// `Permissions(NONE)` for none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Permissions(")?;
        if *self == Permissions::NONE {
            f.write_str("NONE")?;
        }
        for (i, (name, _)) in self.named().enumerate() {
            if i > 0 {
                f.write_str(" | ")?;
            }
            f.write_str(name)?;
        }
        f.write_str(")")
    }
}

impl fmt::Display for Permissions {
    /// `print, copy`, the bits in words; `none` for none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Permissions::NONE {
            return f.write_str("none");
        }
        for (i, (_, words)) in self.named().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(words)?;
        }
        Ok(())
    }
}

/// How a file lets a user print, at three levels, as a security dialog
/// offers them. A level is made a set of [`Permissions`] by
/// `Permissions::from(level)`, and a set's level read back by
/// [`Permissions::printing`].
///
/// Exhaustive on purpose: the levels are designed, the two bits' three
/// meanings, and a dialog lists every one ([`Printing::ALL`]), so a level
/// more would be a breaking release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Printing {
    /// Not at all: neither [`Permissions::PRINT`] nor
    /// [`Permissions::PRINT_HIGH`].
    Denied,
    /// At low resolution: [`Permissions::PRINT`].
    Low,
    /// At high resolution: [`Permissions::PRINT`] and
    /// [`Permissions::PRINT_HIGH`].
    High,
}

impl Printing {
    /// Every level, in the order of their discriminants: `ALL[level as
    /// usize]` is `level`.
    pub const ALL: [Printing; 3] = [Printing::Denied, Printing::Low, Printing::High];
}

impl From<Printing> for Permissions {
    fn from(printing: Printing) -> Permissions {
        match printing {
            Printing::Denied => Permissions::NONE,
            Printing::Low => Permissions::PRINT,
            Printing::High => Permissions::PRINT | Permissions::PRINT_HIGH,
        }
    }
}

/// How a file lets a user change it, at five levels, as a security dialog
/// offers them, each granting what the one before it does and more, as
/// qpdf's `--modify` levels do. A level is made a set of [`Permissions`]
/// by `Permissions::from(level)`; a file's bits needn't be one of the
/// five, so none is read back from a set: [`Permissions::contains`] says
/// each.
///
/// Exhaustive on purpose: the levels are designed, and a dialog lists
/// every one ([`Changes::ALL`]), so a level more would be a breaking
/// release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Changes {
    /// None.
    Denied,
    /// Assembling the document: [`Permissions::ASSEMBLE`].
    Assemble,
    /// Filling in form fields, and assembling:
    /// [`Permissions::FILL_FORMS`] and [`Permissions::ASSEMBLE`].
    FillForms,
    /// Commenting, filling in form fields, and assembling:
    /// [`Permissions::ANNOTATE`], [`Permissions::FILL_FORMS`] and
    /// [`Permissions::ASSEMBLE`].
    CommentAndFill,
    /// Any but copying: [`Permissions::MODIFY`],
    /// [`Permissions::ANNOTATE`], [`Permissions::FILL_FORMS`] and
    /// [`Permissions::ASSEMBLE`].
    Any,
}

impl Changes {
    /// Every level, in the order of their discriminants: `ALL[level as
    /// usize]` is `level`.
    pub const ALL: [Changes; 5] = [
        Changes::Denied,
        Changes::Assemble,
        Changes::FillForms,
        Changes::CommentAndFill,
        Changes::Any,
    ];
}

impl From<Changes> for Permissions {
    fn from(changes: Changes) -> Permissions {
        let fill = Permissions::FILL_FORMS | Permissions::ASSEMBLE;
        match changes {
            Changes::Denied => Permissions::NONE,
            Changes::Assemble => Permissions::ASSEMBLE,
            Changes::FillForms => fill,
            Changes::CommentAndFill => fill | Permissions::ANNOTATE,
            Changes::Any => fill | Permissions::ANNOTATE | Permissions::MODIFY,
        }
    }
}
