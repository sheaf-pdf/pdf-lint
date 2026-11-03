//! The standard 14 fonts: every PDF reader has them, so a file may draw in them
//! without embedding anything (ISO 32000-2, 9.6.2.2). Every width and box is
//! Adobe's, from the font's AFM. A file's font names one by any of the names
//! PDFium takes for it ([`Std14::named`]).

use std::sync::OnceLock;

use crate::afm::Afm;
use crate::encoding::Encoding;
use crate::font::{FontMetrics, Rect};

/// A standard 14 font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Std14 {
    /// `Courier`.
    Courier,
    /// `Courier-Bold`.
    CourierBold,
    /// `Courier-Oblique`.
    CourierOblique,
    /// `Courier-BoldOblique`.
    CourierBoldOblique,
    /// `Helvetica`.
    Helvetica,
    /// `Helvetica-Bold`.
    HelveticaBold,
    /// `Helvetica-Oblique`.
    HelveticaOblique,
    /// `Helvetica-BoldOblique`.
    HelveticaBoldOblique,
    /// `Times-Roman`.
    TimesRoman,
    /// `Times-Bold`.
    TimesBold,
    /// `Times-Italic`.
    TimesItalic,
    /// `Times-BoldItalic`.
    TimesBoldItalic,
    /// `Symbol`.
    Symbol,
    /// `ZapfDingbats`.
    ZapfDingbats,
}

impl Std14 {
    /// Adobe's notice for the metrics, unmodified, as in `assets/afm/MustRead.html`.
    /// Each AFM carries it on its third line. A binary's list of notices takes it
    /// from here, with each font's own copyright (`Std14::afm_text`).
    pub const NOTICE: &'static str = "This file and the 14 PostScript(R) AFM files it accompanies may be used, copied, and distributed for any purpose and without charge, with or without modification, provided that all copyright notices are retained; that the AFM files are not distributed without this file; that all modifications to this file or any of the AFM files are prominently noted in the modified file(s); and that this paragraph is not modified. Adobe Systems has no responsibility or obligation to support the use of the AFM files.";

    /// All 14.
    pub const ALL: [Std14; 14] = [
        Std14::Courier,
        Std14::CourierBold,
        Std14::CourierOblique,
        Std14::CourierBoldOblique,
        Std14::Helvetica,
        Std14::HelveticaBold,
        Std14::HelveticaOblique,
        Std14::HelveticaBoldOblique,
        Std14::TimesRoman,
        Std14::TimesBold,
        Std14::TimesItalic,
        Std14::TimesBoldItalic,
        Std14::Symbol,
        Std14::ZapfDingbats,
    ];

    /// Its `/BaseFont`.
    pub fn name(self) -> &'static str {
        match self {
            Std14::Courier => "Courier",
            Std14::CourierBold => "Courier-Bold",
            Std14::CourierOblique => "Courier-Oblique",
            Std14::CourierBoldOblique => "Courier-BoldOblique",
            Std14::Helvetica => "Helvetica",
            Std14::HelveticaBold => "Helvetica-Bold",
            Std14::HelveticaOblique => "Helvetica-Oblique",
            Std14::HelveticaBoldOblique => "Helvetica-BoldOblique",
            Std14::TimesRoman => "Times-Roman",
            Std14::TimesBold => "Times-Bold",
            Std14::TimesItalic => "Times-Italic",
            Std14::TimesBoldItalic => "Times-BoldItalic",
            Std14::Symbol => "Symbol",
            Std14::ZapfDingbats => "ZapfDingbats",
        }
    }

    /// The standard 14 font `base_font`, a font's `/BaseFont`, names, as
    /// PDFium draws one (`CFX_StandardFont::GetStandardFontIndex`, in its font
    /// mapper): after a subset's tag (`ABCDEF+`), one of the 14 names or an
    /// alternate PDFium takes for one (`Arial,Bold`, `TimesNewRomanPSMT`), in
    /// any case. None for any other name.
    pub fn named(base_font: &[u8]) -> Option<Std14> {
        let name = match base_font.iter().position(|&c| c == b'+') {
            Some(6) => &base_font[7..],
            _ => base_font,
        };
        (NAMES.iter())
            .find(|(alias, _)| alias.as_bytes().eq_ignore_ascii_case(name))
            .map(|&(_, font)| font)
    }

    /// How far its glyphs reach, per 1000 em, from its AFM: its
    /// `Ascender` and `Descender`, or, for Symbol and ZapfDingbats, which
    /// give neither, its `FontBBox`'s top and bottom; and its `FontBBox`.
    pub fn metrics(self) -> FontMetrics {
        let afm = self.afm();
        let [left, bottom, right, top] = afm.bbox.map(f32::from);
        FontMetrics {
            ascent: afm.ascender.map_or(top, f32::from),
            descent: afm.descender.map_or(bottom, f32::from),
            bbox: Some(Rect::new(left, bottom, right, top)),
        }
    }

    /// Glyph `name`'s advance, per 1000 em, from its AFM; None for a glyph
    /// it hasn't.
    pub fn glyph_width(self, name: &str) -> Option<f32> {
        self.afm().width(name).map(f32::from)
    }

    /// The box glyph `name`'s outline fits in, from its origin, per 1000
    /// em, from its AFM; None for a glyph it hasn't.
    pub fn glyph_box(self, name: &str) -> Option<Rect> {
        let [left, bottom, right, top] = self.afm().glyph_box(name)?.map(f32::from);
        Some(Rect::new(left, bottom, right, top))
    }

    /// Whether glyph `name`'s width, where a file draws the font without
    /// a program or `/Widths`, is the face's PDFium draws it in rather
    /// than Adobe's. PDFium's own faces (Foxit's, measured as of chromium/8125)
    /// haven't the glyphs Adobe's metrics added in their second
    /// version, and draw each as the space, and draw a few
    /// others at widths of their own; a system's face may draw any of
    /// them otherwise again. Courier's are 600 whatever draws them, and
    /// Symbol's and ZapfDingbats' are Adobe's. The Euro isn't among these, though
    /// its faces draw it at widths of their own: Adobe's metrics give it no box,
    /// which says the same of it.
    pub fn drawn_otherwise(self, name: &str) -> bool {
        let own: &[&str] = match self {
            Std14::Helvetica => &["guillemotleft", "guillemotright", "guilsinglright"],
            Std14::HelveticaOblique => &["at", "guilsinglleft"],
            Std14::HelveticaBold => &["at"],
            Std14::HelveticaBoldOblique => &["at", "five", "oslash"],
            _ => &[],
        };
        let latin = !matches!(
            self,
            Std14::Courier
                | Std14::CourierBold
                | Std14::CourierOblique
                | Std14::CourierBoldOblique
                | Std14::Symbol
                | Std14::ZapfDingbats
        );
        (latin && ADDED.contains(&name)) || own.contains(&name)
    }

    /// The encoding of its own a font of its name is read in, where nothing says
    /// otherwise: StandardEncoding for the twelve Latin ones, Symbol's and
    /// ZapfDingbats' their own.
    pub(crate) fn own(self) -> Encoding {
        match self {
            Std14::Symbol => Encoding::Symbol,
            Std14::ZapfDingbats => Encoding::ZapfDingbats,
            _ => Encoding::Standard,
        }
    }

    /// Its AFM, as Adobe wrote it but for what its second line says is removed,
    /// with Adobe's notice ([`Std14::NOTICE`]) on its third.
    pub fn afm_text(self) -> &'static str {
        match self {
            Std14::Courier => include_str!("../assets/afm/Courier.afm"),
            Std14::CourierBold => include_str!("../assets/afm/Courier-Bold.afm"),
            Std14::CourierOblique => include_str!("../assets/afm/Courier-Oblique.afm"),
            Std14::CourierBoldOblique => {
                include_str!("../assets/afm/Courier-BoldOblique.afm")
            }
            Std14::Helvetica => include_str!("../assets/afm/Helvetica.afm"),
            Std14::HelveticaBold => include_str!("../assets/afm/Helvetica-Bold.afm"),
            Std14::HelveticaOblique => include_str!("../assets/afm/Helvetica-Oblique.afm"),
            Std14::HelveticaBoldOblique => {
                include_str!("../assets/afm/Helvetica-BoldOblique.afm")
            }
            Std14::TimesRoman => include_str!("../assets/afm/Times-Roman.afm"),
            Std14::TimesBold => include_str!("../assets/afm/Times-Bold.afm"),
            Std14::TimesItalic => include_str!("../assets/afm/Times-Italic.afm"),
            Std14::TimesBoldItalic => include_str!("../assets/afm/Times-BoldItalic.afm"),
            Std14::Symbol => include_str!("../assets/afm/Symbol.afm"),
            Std14::ZapfDingbats => include_str!("../assets/afm/ZapfDingbats.afm"),
        }
    }

    /// Its metrics, read once.
    pub(crate) fn afm(self) -> &'static Afm {
        static AFMS: [OnceLock<Afm>; 14] = [const { OnceLock::new() }; 14];
        AFMS[self as usize].get_or_init(|| Afm::parse(self.afm_text()))
    }
}

/// The glyphs Adobe's metrics for the Helvetica and Times fonts added in
/// their second version, the letters of Central European languages and
/// MacRoman's mathematical signs, which PDFium's own faces haven't
/// ([`Std14::drawn_otherwise`]).
const ADDED: [&str; 86] = [
    "Abreve",
    "Amacron",
    "Aogonek",
    "Cacute",
    "Ccaron",
    "Dcaron",
    "Dcroat",
    "Delta",
    "Ecaron",
    "Edotaccent",
    "Emacron",
    "Eogonek",
    "Gbreve",
    "Gcommaaccent",
    "Idotaccent",
    "Imacron",
    "Iogonek",
    "Kcommaaccent",
    "Lacute",
    "Lcaron",
    "Lcommaaccent",
    "Nacute",
    "Ncaron",
    "Ncommaaccent",
    "Ohungarumlaut",
    "Omacron",
    "Racute",
    "Rcaron",
    "Rcommaaccent",
    "Sacute",
    "Scedilla",
    "Scommaaccent",
    "Tcaron",
    "Tcommaaccent",
    "Uhungarumlaut",
    "Umacron",
    "Uogonek",
    "Uring",
    "Zacute",
    "Zdotaccent",
    "abreve",
    "amacron",
    "aogonek",
    "cacute",
    "ccaron",
    "commaaccent",
    "dcaron",
    "dcroat",
    "ecaron",
    "edotaccent",
    "emacron",
    "eogonek",
    "gbreve",
    "gcommaaccent",
    "greaterequal",
    "imacron",
    "iogonek",
    "kcommaaccent",
    "lacute",
    "lcaron",
    "lcommaaccent",
    "lessequal",
    "lozenge",
    "nacute",
    "ncaron",
    "ncommaaccent",
    "notequal",
    "ohungarumlaut",
    "omacron",
    "partialdiff",
    "racute",
    "radical",
    "rcaron",
    "rcommaaccent",
    "sacute",
    "scedilla",
    "scommaaccent",
    "summation",
    "tcaron",
    "tcommaaccent",
    "uhungarumlaut",
    "umacron",
    "uogonek",
    "uring",
    "zacute",
    "zdotaccent",
];

/// The names PDFium reads as the standard 14 fonts: the 14 and the
/// alternates it takes for them, from `cfx_standardfont.cpp`'s
/// `kAltFontNames` (as of chromium/8125).
const NAMES: [(&str, Std14); 89] = [
    ("Arial", Std14::Helvetica),
    ("Arial,Bold", Std14::HelveticaBold),
    ("Arial,BoldItalic", Std14::HelveticaBoldOblique),
    ("Arial,Italic", Std14::HelveticaOblique),
    ("Arial-Bold", Std14::HelveticaBold),
    ("Arial-BoldItalic", Std14::HelveticaBoldOblique),
    ("Arial-BoldItalicMT", Std14::HelveticaBoldOblique),
    ("Arial-BoldMT", Std14::HelveticaBold),
    ("Arial-Italic", Std14::HelveticaOblique),
    ("Arial-ItalicMT", Std14::HelveticaOblique),
    ("ArialBold", Std14::HelveticaBold),
    ("ArialBoldItalic", Std14::HelveticaBoldOblique),
    ("ArialItalic", Std14::HelveticaOblique),
    ("ArialMT", Std14::Helvetica),
    ("ArialMT,Bold", Std14::HelveticaBold),
    ("ArialMT,BoldItalic", Std14::HelveticaBoldOblique),
    ("ArialMT,Italic", Std14::HelveticaOblique),
    ("ArialRoundedMTBold", Std14::HelveticaBold),
    ("Courier", Std14::Courier),
    ("Courier,Bold", Std14::CourierBold),
    ("Courier,BoldItalic", Std14::CourierBoldOblique),
    ("Courier,Italic", Std14::CourierOblique),
    ("Courier-Bold", Std14::CourierBold),
    ("Courier-BoldOblique", Std14::CourierBoldOblique),
    ("Courier-Oblique", Std14::CourierOblique),
    ("CourierBold", Std14::CourierBold),
    ("CourierBoldItalic", Std14::CourierBoldOblique),
    ("CourierItalic", Std14::CourierOblique),
    ("CourierNew", Std14::Courier),
    ("CourierNew,Bold", Std14::CourierBold),
    ("CourierNew,BoldItalic", Std14::CourierBoldOblique),
    ("CourierNew,Italic", Std14::CourierOblique),
    ("CourierNew-Bold", Std14::CourierBold),
    ("CourierNew-BoldItalic", Std14::CourierBoldOblique),
    ("CourierNew-Italic", Std14::CourierOblique),
    ("CourierNewBold", Std14::CourierBold),
    ("CourierNewBoldItalic", Std14::CourierBoldOblique),
    ("CourierNewItalic", Std14::CourierOblique),
    ("CourierNewPS-BoldItalicMT", Std14::CourierBoldOblique),
    ("CourierNewPS-BoldMT", Std14::CourierBold),
    ("CourierNewPS-ItalicMT", Std14::CourierOblique),
    ("CourierNewPSMT", Std14::Courier),
    ("CourierStd", Std14::Courier),
    ("CourierStd-Bold", Std14::CourierBold),
    ("CourierStd-BoldOblique", Std14::CourierBoldOblique),
    ("CourierStd-Oblique", Std14::CourierOblique),
    ("Helvetica", Std14::Helvetica),
    ("Helvetica,Bold", Std14::HelveticaBold),
    ("Helvetica,BoldItalic", Std14::HelveticaBoldOblique),
    ("Helvetica,Italic", Std14::HelveticaOblique),
    ("Helvetica-Bold", Std14::HelveticaBold),
    ("Helvetica-BoldItalic", Std14::HelveticaBoldOblique),
    ("Helvetica-BoldOblique", Std14::HelveticaBoldOblique),
    ("Helvetica-Italic", Std14::HelveticaOblique),
    ("Helvetica-Oblique", Std14::HelveticaOblique),
    ("HelveticaBold", Std14::HelveticaBold),
    ("HelveticaBoldItalic", Std14::HelveticaBoldOblique),
    ("HelveticaItalic", Std14::HelveticaOblique),
    ("Symbol", Std14::Symbol),
    ("SymbolMT", Std14::Symbol),
    ("Times-Bold", Std14::TimesBold),
    ("Times-BoldItalic", Std14::TimesBoldItalic),
    ("Times-Italic", Std14::TimesItalic),
    ("Times-Roman", Std14::TimesRoman),
    ("TimesBold", Std14::TimesBold),
    ("TimesBoldItalic", Std14::TimesBoldItalic),
    ("TimesItalic", Std14::TimesItalic),
    ("TimesNewRoman", Std14::TimesRoman),
    ("TimesNewRoman,Bold", Std14::TimesBold),
    ("TimesNewRoman,BoldItalic", Std14::TimesBoldItalic),
    ("TimesNewRoman,Italic", Std14::TimesItalic),
    ("TimesNewRoman-Bold", Std14::TimesBold),
    ("TimesNewRoman-BoldItalic", Std14::TimesBoldItalic),
    ("TimesNewRoman-Italic", Std14::TimesItalic),
    ("TimesNewRomanBold", Std14::TimesBold),
    ("TimesNewRomanBoldItalic", Std14::TimesBoldItalic),
    ("TimesNewRomanItalic", Std14::TimesItalic),
    ("TimesNewRomanPS", Std14::TimesRoman),
    ("TimesNewRomanPS-Bold", Std14::TimesBold),
    ("TimesNewRomanPS-BoldItalic", Std14::TimesBoldItalic),
    ("TimesNewRomanPS-BoldItalicMT", Std14::TimesBoldItalic),
    ("TimesNewRomanPS-BoldMT", Std14::TimesBold),
    ("TimesNewRomanPS-Italic", Std14::TimesItalic),
    ("TimesNewRomanPS-ItalicMT", Std14::TimesItalic),
    ("TimesNewRomanPSMT", Std14::TimesRoman),
    ("TimesNewRomanPSMT,Bold", Std14::TimesBold),
    ("TimesNewRomanPSMT,BoldItalic", Std14::TimesBoldItalic),
    ("TimesNewRomanPSMT,Italic", Std14::TimesItalic),
    ("ZapfDingbats", Std14::ZapfDingbats),
];
