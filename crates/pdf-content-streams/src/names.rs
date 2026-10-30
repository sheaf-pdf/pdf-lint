//! The resource names content uses (ISO 32000-2, 7.8.3): what a page's or
//! a form's `/Resources` must have for it, by category; and what content
//! keeps once ranges of it are taken out, its balance and the names it
//! still uses, beside those only the ranges used, in one pass ([`kept`]).

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::balance::Balance;
use crate::error::Error;
use crate::inline::{ResCx, expanded};
use crate::limits::Limits;
use crate::operations::Operation;
use crate::page::{Concat, SpanAcross};

/// A resource category: a key of `/Resources` (7.8.3, Table 34) whose
/// dictionary maps the names content uses to what they name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResKind {
    /// Color spaces: `CS`'s, `cs`'s and an inline image's.
    ColorSpace,
    /// Graphics state parameter dictionaries: `gs`'s.
    ExtGState,
    /// Fonts: `Tf`'s.
    Font,
    /// Patterns: `SCN`'s and `scn`'s.
    Pattern,
    /// Marked content's property lists: `BDC`'s and `DP`'s.
    Properties,
    /// Shadings: `sh`'s.
    Shading,
    /// External objects, images and forms: `Do`'s.
    XObject,
}

impl ResKind {
    /// Every category, in order.
    pub const ALL: [ResKind; 7] = [
        ResKind::ColorSpace,
        ResKind::ExtGState,
        ResKind::Font,
        ResKind::Pattern,
        ResKind::Properties,
        ResKind::Shading,
        ResKind::XObject,
    ];

    /// Its key in `/Resources`.
    pub fn key(self) -> &'static [u8] {
        match self {
            ResKind::ColorSpace => b"ColorSpace",
            ResKind::ExtGState => b"ExtGState",
            ResKind::Font => b"Font",
            ResKind::Pattern => b"Pattern",
            ResKind::Properties => b"Properties",
            ResKind::Shading => b"Shading",
            ResKind::XObject => b"XObject",
        }
    }
}

/// Resource names, each in its category, decoded.
///
/// It's filled an operation at a time ([`take`](NameSet::take)), so a
/// caller stepping through content for more than its names finds them in
/// the same pass; [`names_used`] fills one from a page's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameSet {
    names: BTreeSet<(ResKind, Vec<u8>)>,
}

impl NameSet {
    /// Whether `name`, decoded, without its `/`, is among those of `kind`.
    pub fn contains(&self, kind: ResKind, name: &[u8]) -> bool {
        self.names.contains(&(kind, name.to_vec()))
    }

    /// The names of `kind`, in order.
    pub fn of(&self, kind: ResKind) -> impl Iterator<Item = &[u8]> {
        self.names
            .range((kind, Vec::new())..)
            .take_while(move |(k, _)| *k == kind)
            .map(|(_, name)| name.as_slice())
    }

    /// Every name, each in its category, in order.
    pub fn iter(&self) -> impl Iterator<Item = (ResKind, &[u8])> {
        self.names
            .iter()
            .map(|(kind, name)| (*kind, name.as_slice()))
    }

    /// How many names there are, each category's counted.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// `name` of `kind` taken out, if it's there.
    pub fn remove(&mut self, kind: ResKind, name: &[u8]) {
        self.names.remove(&(kind, name.to_vec()));
    }

    /// The name `op`, content's next operation, uses, if it uses one
    /// ([`Operation::resource`]), put in.
    pub fn take(&mut self, op: &Operation) {
        if let Some((kind, name)) = op.resource() {
            self.names.insert((kind, name.into_owned()));
        }
    }
}

impl<'a> Operation<'a> {
    /// The resource it names, by category, and its name, decoded, without
    /// its `/`: what a `/Resources` it's drawn with must have under that
    /// category ([`names_used`] lists each operator's). None for an
    /// operation that names none.
    ///
    /// ```
    /// use pdf_content_streams::{Limits, ResCx, ResKind, operations};
    ///
    /// let content = b"/F1 12 Tf /DeviceRGB cs /Im#201 Do";
    /// let named: Vec<_> = operations(content, &ResCx::none(), &Limits::DEFAULT)
    ///     .map(|op| op.unwrap().resource())
    ///     .collect();
    /// assert_eq!(named[0], Some((ResKind::Font, b"F1"[..].into())));
    /// // A device space is no resource.
    /// assert_eq!(named[1], None);
    /// assert_eq!(named[2], Some((ResKind::XObject, b"Im 1"[..].into())));
    /// ```
    pub fn resource(&self) -> Option<(ResKind, Cow<'a, [u8]>)> {
        let name = |i| self.arg(i).and_then(|o| o.name());
        let (kind, name) = match self.name() {
            b"Tf" => (ResKind::Font, name(1)),
            b"Do" => (ResKind::XObject, name(0)),
            b"gs" => (ResKind::ExtGState, name(0)),
            b"sh" => (ResKind::Shading, name(0)),
            b"cs" | b"CS" => (ResKind::ColorSpace, name(0).filter(|n| !device(n))),
            b"scn" | b"SCN" => (ResKind::Pattern, name(0)),
            b"BDC" | b"DP" => (ResKind::Properties, name(0)),
            _ => {
                // By the name PDFium looks it up by: an abbreviation read
                // as the full name, so `/I` as `/Indexed`.
                let space = self.inline_image()?.entry(b"ColorSpace");
                let space = space
                    .and_then(|s| s.name())
                    .map(|n| expanded(&n).map_or(n, Cow::Borrowed))
                    .filter(|n| !device(n));
                (ResKind::ColorSpace, space)
            }
        };
        Some((kind, name?))
    }
}

/// The resource names `content` uses, its operations read with `res`
/// under `limits`, but for those only operations inside `except` use
/// (ranges to be taken out, whole streams among them):
/// - `Tf`'s font, `Do`'s XObject, `gs`'s ExtGState and `sh`'s shading;
/// - `cs` and `CS`'s color space, and an inline image's, but for the
///   device spaces and `/Pattern`, which aren't resources: an inline
///   image's as PDFium reads its dictionary
///   ([`InlineImage::entry`](crate::InlineImage::entry)), by the name it
///   looks it up by, an abbreviation read as the full name (8.9.7, Table
///   92), so `/G` is none and `/I` is `/Indexed`;
/// - `scn` and `SCN`'s pattern, when the last operand is a name;
/// - `BDC` and `DP`'s property list, when it's named.
///
/// An operation is inside a range when everything from its first operand
/// to its operator is.
///
/// ```
/// use pdf_content_streams::{Concat, Limits, ResCx, ResKind, names_used};
///
/// let page = Concat::new(&[b"/GS0 gs /F1 9 Tf", b"/Artifact BMC /F2 9 Tf EMC"]);
/// let names = names_used(&page, &ResCx::none(), &Limits::DEFAULT, &[]).unwrap();
/// let fonts: Vec<_> = names.of(ResKind::Font).collect();
/// assert_eq!(fonts, [b"F1", b"F2"]);
///
/// // The second stream to be taken out.
/// let second = page.span(page.range(1));
/// let names = names_used(&page, &ResCx::none(), &Limits::DEFAULT, &[second]).unwrap();
/// let fonts: Vec<_> = names.of(ResKind::Font).collect();
/// assert_eq!(fonts, [b"F1"]);
/// assert!(names.contains(ResKind::ExtGState, b"GS0"));
/// ```
pub fn names_used(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    except: &[SpanAcross],
) -> Result<NameSet, Error> {
    Ok(kept(content, res, limits, except)?.names)
}

/// What's left of content once ranges are taken out, read in one pass
/// ([`kept`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kept {
    /// How its `q`s and `Q`s balance.
    pub balance: Balance,
    /// The resource names it uses ([`names_used`]).
    pub names: NameSet,
    /// The names only what's taken out uses, which the resources of what's
    /// kept can go without.
    pub dropped: NameSet,
}

/// What `content`, its operations read with `res` under `limits`, keeps
/// once the operations inside `drop` (ranges to be taken out, whole
/// streams among them) are taken out: an operation is inside a range when
/// everything from its first operand to its operator is.
pub fn kept(
    content: &Concat,
    res: &ResCx,
    limits: &Limits,
    drop: &[SpanAcross],
) -> Result<Kept, Error> {
    let drop: Vec<_> = drop.iter().filter_map(|&s| content.offsets(s)).collect();
    let mut kept = Kept::default();
    let mut inside = NameSet::default();
    for op in content.operations(res, limits) {
        let op = op?;
        let range = op.range();
        let dropped = drop
            .iter()
            .any(|d| d.start <= range.start && range.end <= d.end);
        match dropped {
            true => inside.take(&op),
            false => {
                kept.names.take(&op);
                kept.balance.take(&op);
            }
        }
    }
    kept.dropped.names = (inside.names)
        .into_iter()
        .filter(|n| !kept.names.names.contains(n))
        .collect();
    Ok(kept)
}

/// Whether a color space name is a device space or `/Pattern`: no
/// resource.
fn device(name: &[u8]) -> bool {
    matches!(
        name,
        b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK" | b"Pattern"
    )
}
