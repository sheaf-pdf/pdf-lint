//! The predefined CMaps a Type 0 font's `/Encoding` names (ISO 32000-2,
//! 9.7.5.2): how PDFium reads each one, from `cpdf_cmap.cpp`'s
//! `kPredefinedCMaps`, how its codes split, which its codespace says but
//! hayro-cmap keeps to itself, its collection, and whether its codes are their
//! own text; and, with the `predefined-cmaps` feature, Adobe's, from hayro-cmap's
//! bundle, each parsed once, for each code's CID and a CID's text
//! (Adobe-*-UCS2).
//!
//! hayro-cmap only ever parses its own bundle here, never a file's bytes:
//! an embedded CMap is this crate's to read, within its limits.

#[cfg(feature = "predefined-cmaps")]
use std::sync::OnceLock;

#[cfg(feature = "predefined-cmaps")]
pub(crate) use hayro_cmap::{BfString, CMap};
#[cfg(feature = "predefined-cmaps")]
use hayro_cmap::{CMapName, load_embedded};

/// No CMap, in a build without the feature: nothing can be one, so a font's
/// reading has no code of its own for which it is.
#[cfg(not(feature = "predefined-cmaps"))]
#[derive(Debug)]
pub(crate) enum CMap {}

#[cfg(not(feature = "predefined-cmaps"))]
impl CMap {
    pub(crate) fn lookup_cid_code(&self, _: u32, _: u8) -> Option<u32> {
        match *self {}
    }

    pub(crate) fn lookup_bf_string(&self, _: u32) -> Option<BfString> {
        match *self {}
    }
}

/// What a CID's text is, in the shape hayro-cmap gives it.
#[cfg(not(feature = "predefined-cmaps"))]
#[allow(dead_code)]
pub(crate) enum BfString {
    Char(char),
    String(String),
}

/// A CID character collection PDFium maps CIDs to text in (its
/// `CIDSet`), Adobe's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Collection {
    Gb1,
    Cns1,
    Japan1,
    Korea1,
}

impl Collection {
    /// The collection a CIDFont's `/CIDSystemInfo` `/Ordering` names, as
    /// PDFium reads it (`CharsetFromOrdering`): by the ordering alone, its
    /// registry unread.
    pub(crate) fn ordered(ordering: &[u8]) -> Option<Collection> {
        match ordering {
            b"GB1" => Some(Collection::Gb1),
            b"CNS1" => Some(Collection::Cns1),
            b"Japan1" => Some(Collection::Japan1),
            b"Korea1" => Some(Collection::Korea1),
            _ => None,
        }
    }

    /// The CMap from its CIDs to their text: `Adobe-<collection>-UCS2`.
    #[cfg(feature = "predefined-cmaps")]
    pub(crate) fn to_unicode(self) -> Option<&'static CMap> {
        bundled(match self {
            Collection::Gb1 => b"Adobe-GB1-UCS2",
            Collection::Cns1 => b"Adobe-CNS1-UCS2",
            Collection::Japan1 => b"Adobe-Japan1-UCS2",
            Collection::Korea1 => b"Adobe-Korea1-UCS2",
        })
    }

    /// None, in a build without the feature.
    #[cfg(not(feature = "predefined-cmaps"))]
    pub(crate) fn to_unicode(self) -> Option<&'static CMap> {
        None
    }
}

/// How a predefined CMap's strings split into codes, as PDFium splits
/// them (`CPDF_CMap::GetNextChar`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Split {
    /// Two bytes each.
    TwoBytes,
    /// A byte each, but two where the first is in one of these ranges,
    /// each its first and last leading byte.
    Lead(&'static [(u8, u8)]),
}

impl Split {
    /// Whether a code that begins with `byte` is of two bytes.
    pub(crate) fn leads(self, byte: u8) -> bool {
        match self {
            Split::TwoBytes => true,
            Split::Lead(ranges) => ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&byte)),
        }
    }
}

/// A predefined CMap as PDFium reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scheme {
    pub split: Split,
    pub collection: Collection,
    /// Whether each code is its own text: a UCS2 or UTF16 CMap, its codes
    /// Unicode's.
    pub unicode: bool,
}

/// PDFium's `kPredefinedCMaps`, by name without `-H` or `-V`.
const SCHEMES: [(&[u8], Scheme); 32] = {
    use Collection::*;
    const GB: Split = Split::Lead(&[(0x81, 0xfe)]);
    const RKSJ: Split = Split::Lead(&[(0x81, 0x9f), (0xe0, 0xfc)]);
    const UHC: Split = Split::Lead(&[(0x81, 0xfe)]);
    const fn mixed(split: Split, collection: Collection) -> Scheme {
        Scheme {
            split,
            collection,
            unicode: false,
        }
    }
    const fn unicode(collection: Collection) -> Scheme {
        Scheme {
            split: Split::TwoBytes,
            collection,
            unicode: true,
        }
    }
    [
        (b"GB-EUC", mixed(Split::Lead(&[(0xa1, 0xfe)]), Gb1)),
        (b"GBpc-EUC", mixed(Split::Lead(&[(0xa1, 0xfc)]), Gb1)),
        (b"GBK-EUC", mixed(GB, Gb1)),
        (b"GBKp-EUC", mixed(GB, Gb1)),
        (b"GBK2K-EUC", mixed(GB, Gb1)),
        (b"GBK2K", mixed(GB, Gb1)),
        (b"UniGB-UCS2", unicode(Gb1)),
        (b"UniGB-UTF16", unicode(Gb1)),
        (b"B5pc", mixed(Split::Lead(&[(0xa1, 0xfc)]), Cns1)),
        (b"HKscs-B5", mixed(Split::Lead(&[(0x88, 0xfe)]), Cns1)),
        (b"ETen-B5", mixed(Split::Lead(&[(0xa1, 0xfe)]), Cns1)),
        (b"ETenms-B5", mixed(Split::Lead(&[(0xa1, 0xfe)]), Cns1)),
        (b"UniCNS-UCS2", unicode(Cns1)),
        (b"UniCNS-UTF16", unicode(Cns1)),
        (b"83pv-RKSJ", mixed(RKSJ, Japan1)),
        (b"90ms-RKSJ", mixed(RKSJ, Japan1)),
        (b"90msp-RKSJ", mixed(RKSJ, Japan1)),
        (b"90pv-RKSJ", mixed(RKSJ, Japan1)),
        (b"Add-RKSJ", mixed(RKSJ, Japan1)),
        (
            b"EUC",
            mixed(Split::Lead(&[(0x8e, 0x8e), (0xa1, 0xfe)]), Japan1),
        ),
        (b"H", mixed(Split::TwoBytes, Japan1)),
        (b"V", mixed(Split::TwoBytes, Japan1)),
        (b"Ext-RKSJ", mixed(RKSJ, Japan1)),
        (b"UniJIS-UCS2", unicode(Japan1)),
        (b"UniJIS-UCS2-HW", unicode(Japan1)),
        (b"UniJIS-UTF16", unicode(Japan1)),
        (b"KSC-EUC", mixed(Split::Lead(&[(0xa1, 0xfe)]), Korea1)),
        (b"KSCms-UHC", mixed(UHC, Korea1)),
        (b"KSCms-UHC-HW", mixed(UHC, Korea1)),
        (b"KSCpc-EUC", mixed(Split::Lead(&[(0xa1, 0xfd)]), Korea1)),
        (b"UniKS-UCS2", unicode(Korea1)),
        (b"UniKS-UTF16", unicode(Korea1)),
    ]
};

/// How PDFium reads the predefined CMap `name` (`GetPredefinedCMap`): by
/// its name without its last two bytes, its `-H` or `-V`, where it has
/// more than two. None for a name it doesn't know, which it reads as no
/// CMap: CNS-EUC-H and -V among those ISO 32000-2 names.
pub(crate) fn scheme(name: &[u8]) -> Option<Scheme> {
    let id = match name.len() {
        0..=2 => name,
        n => &name[..n - 2],
    };
    SCHEMES.iter().find(|(n, _)| *n == id).map(|(_, s)| *s)
}

/// The CMaps hayro-cmap bundles: the 61 ISO 32000-2 names, and Adobe-*-UCS2.
#[cfg(feature = "predefined-cmaps")]
const BUNDLED: [&[u8]; 65] = [
    b"83pv-RKSJ-H",
    b"90ms-RKSJ-H",
    b"90ms-RKSJ-V",
    b"90msp-RKSJ-H",
    b"90msp-RKSJ-V",
    b"90pv-RKSJ-H",
    b"Add-RKSJ-H",
    b"Add-RKSJ-V",
    b"Adobe-CNS1-UCS2",
    b"Adobe-GB1-UCS2",
    b"Adobe-Japan1-UCS2",
    b"Adobe-Korea1-UCS2",
    b"B5pc-H",
    b"B5pc-V",
    b"CNS-EUC-H",
    b"CNS-EUC-V",
    b"ETen-B5-H",
    b"ETen-B5-V",
    b"ETenms-B5-H",
    b"ETenms-B5-V",
    b"EUC-H",
    b"EUC-V",
    b"Ext-RKSJ-H",
    b"Ext-RKSJ-V",
    b"GB-EUC-H",
    b"GB-EUC-V",
    b"GBK-EUC-H",
    b"GBK-EUC-V",
    b"GBK2K-H",
    b"GBK2K-V",
    b"GBKp-EUC-H",
    b"GBKp-EUC-V",
    b"GBpc-EUC-H",
    b"GBpc-EUC-V",
    b"H",
    b"HKscs-B5-H",
    b"HKscs-B5-V",
    b"Identity-H",
    b"Identity-V",
    b"KSC-EUC-H",
    b"KSC-EUC-V",
    b"KSCms-UHC-H",
    b"KSCms-UHC-HW-H",
    b"KSCms-UHC-HW-V",
    b"KSCms-UHC-V",
    b"KSCpc-EUC-H",
    b"UniCNS-UCS2-H",
    b"UniCNS-UCS2-V",
    b"UniCNS-UTF16-H",
    b"UniCNS-UTF16-V",
    b"UniGB-UCS2-H",
    b"UniGB-UCS2-V",
    b"UniGB-UTF16-H",
    b"UniGB-UTF16-V",
    b"UniJIS-UCS2-H",
    b"UniJIS-UCS2-HW-H",
    b"UniJIS-UCS2-HW-V",
    b"UniJIS-UCS2-V",
    b"UniJIS-UTF16-H",
    b"UniJIS-UTF16-V",
    b"UniKS-UCS2-H",
    b"UniKS-UCS2-V",
    b"UniKS-UTF16-H",
    b"UniKS-UTF16-V",
    b"V",
];

/// Each bundled CMap, parsed the first time it's asked for. The first
/// decompresses the whole bundle, about 460 KB, which stays.
#[cfg(feature = "predefined-cmaps")]
static PARSED: [OnceLock<Option<CMap>>; 65] = [const { OnceLock::new() }; 65];

/// The bundled CMap `name`, over the bundled ones it uses: None where
/// hayro-cmap doesn't bundle it.
#[cfg(feature = "predefined-cmaps")]
pub(crate) fn bundled(name: &[u8]) -> Option<&'static CMap> {
    let i = BUNDLED.iter().position(|n| *n == name)?;
    let parse = || CMap::parse(load_embedded(CMapName::from_bytes(name))?, load_embedded);
    PARSED[i].get_or_init(parse).as_ref()
}

/// None, in a build without the feature.
#[cfg(not(feature = "predefined-cmaps"))]
pub(crate) fn bundled(_: &[u8]) -> Option<&'static CMap> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every bundled name is hayro-cmap's and parses, writing vertically
    /// where it ends in V; and each one PDFium knows is bundled, both its
    /// -H and its -V, but those ISO 32000-2 doesn't name: GBK2K-EUC's, and the
    /// -V of 83pv-RKSJ, 90pv-RKSJ and KSCpc-EUC.
    #[cfg(feature = "predefined-cmaps")]
    #[test]
    fn every_bundled_cmap_parses() {
        for name in BUNDLED {
            let map = bundled(name).unwrap_or_else(|| panic!("{}", name.escape_ascii()));
            let vertical = map.metadata().writing_mode == Some(hayro_cmap::WritingMode::Vertical);
            assert_eq!(vertical, name.ends_with(b"V"), "{}", name.escape_ascii());
        }
        assert!(bundled(b"Adobe-Korea1-2").is_none());
        let names = SCHEMES.iter().flat_map(|(id, _)| match *id {
            b"H" | b"V" => vec![id.to_vec()],
            _ => vec![[id, &b"-H"[..]].concat(), [id, &b"-V"[..]].concat()],
        });
        for name in names {
            let unbundled = name.starts_with(b"GBK2K-EUC")
                || [&b"83pv-RKSJ-V"[..], b"90pv-RKSJ-V", b"KSCpc-EUC-V"].contains(&&name[..]);
            let known = bundled(&name).is_some() && scheme(&name).is_some();
            assert_eq!(known, !unbundled, "{}", name.escape_ascii());
        }
    }

    /// PDFium's names: without their last two bytes, but H and V; CNS-EUC
    /// unknown.
    #[test]
    fn schemes_by_pdfium_s_names() {
        let rksj = scheme(b"90ms-RKSJ-V").unwrap();
        assert_eq!((rksj.collection, rksj.unicode), (Collection::Japan1, false));
        assert!(rksj.split.leads(0x81) && rksj.split.leads(0xfc));
        assert!(!rksj.split.leads(0x80) && !rksj.split.leads(0xa0));
        assert!(rksj.split.leads(0x9f) && rksj.split.leads(0xe0));
        assert!(!rksj.split.leads(0xdf) && !rksj.split.leads(0xfd));
        assert_eq!(scheme(b"H").unwrap().split, Split::TwoBytes);
        assert!(scheme(b"UniJIS-UCS2-HW-H").unwrap().unicode);
        assert_eq!(scheme(b"CNS-EUC-H"), None);
        assert_eq!(scheme(b"Identity-H"), None);
        assert_eq!(Collection::ordered(b"Korea1"), Some(Collection::Korea1));
        assert_eq!(Collection::ordered(b"Identity"), None);
    }
}
