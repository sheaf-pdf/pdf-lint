//! A color space as ISO 32000-2 says it (8.6), typed: its family, the spaces it
//! holds and its colorants, each departure a [`Fault`] told to a [`Sink`]. Nothing
//! here converts a color, opens a profile or evaluates a function.
//!
//! [`Space::read`] is the other reading from [`components`](crate::components),
//! which is PDFium's and takes what ISO's doesn't. The two are tied by a promise:
//! a value that gives a space with no fault told has the same count by both, but
//! for a Pattern space over more than 16 components, which no clause refuses and
//! PDFium does. It's why a reference to a reference is a fault, and a tint
//! transform that doesn't load.

use pdf_objects::{Dict, Object, Resolve, Str, Stream, Unreadable};

use crate::fault::Sink;
use crate::limits::Limits;

use read::Reader;

mod read;
#[cfg(test)]
mod tests;

/// A color space's family (ISO 32000-2, 8.6.3, Table 61).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Family {
    /// `DeviceGray`.
    DeviceGray,
    /// `DeviceRGB`.
    DeviceRGB,
    /// `DeviceCMYK`.
    DeviceCMYK,
    /// `CalGray`.
    CalGray,
    /// `CalRGB`.
    CalRGB,
    /// `Lab`.
    Lab,
    /// `ICCBased`.
    ICCBased,
    /// `Indexed`.
    Indexed,
    /// `Pattern`.
    Pattern,
    /// `Separation`.
    Separation,
    /// `DeviceN`.
    DeviceN,
}

impl Family {
    /// The family of this name, whole: an abbreviation is an inline image's
    /// (8.9.7), and the first four bytes are PDFium's.
    pub fn of(name: &[u8]) -> Option<Family> {
        Some(match name {
            b"DeviceGray" => Family::DeviceGray,
            b"DeviceRGB" => Family::DeviceRGB,
            b"DeviceCMYK" => Family::DeviceCMYK,
            b"CalGray" => Family::CalGray,
            b"CalRGB" => Family::CalRGB,
            b"Lab" => Family::Lab,
            b"ICCBased" => Family::ICCBased,
            b"Indexed" => Family::Indexed,
            b"Pattern" => Family::Pattern,
            b"Separation" => Family::Separation,
            b"DeviceN" => Family::DeviceN,
            _ => return None,
        })
    }

    /// Its name, as a file writes it.
    pub fn name(self) -> &'static str {
        match self {
            Family::DeviceGray => "DeviceGray",
            Family::DeviceRGB => "DeviceRGB",
            Family::DeviceCMYK => "DeviceCMYK",
            Family::CalGray => "CalGray",
            Family::CalRGB => "CalRGB",
            Family::Lab => "Lab",
            Family::ICCBased => "ICCBased",
            Family::Indexed => "Indexed",
            Family::Pattern => "Pattern",
            Family::Separation => "Separation",
            Family::DeviceN => "DeviceN",
        }
    }

    /// Whether it's a device family.
    pub fn is_device(self) -> bool {
        matches!(
            self,
            Family::DeviceGray | Family::DeviceRGB | Family::DeviceCMYK
        )
    }

    /// Whether it's a CIE-based family, ICCBased among them.
    pub fn is_cie(self) -> bool {
        matches!(
            self,
            Family::CalGray | Family::CalRGB | Family::Lab | Family::ICCBased
        )
    }

    /// Whether it's a special family.
    pub fn is_special(self) -> bool {
        matches!(
            self,
            Family::Indexed | Family::Pattern | Family::Separation | Family::DeviceN
        )
    }
}

/// A color space, as ISO 32000-2 says it (8.6), read by [`Space::read`]. All it
/// holds borrows from the file, a nested space boxed, and each parameter is an
/// `Option`, as reading goes on past what's wrong.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Space<'o> {
    /// DeviceGray (8.6.4.2).
    DeviceGray,
    /// DeviceRGB (8.6.4.3).
    DeviceRGB,
    /// DeviceCMYK (8.6.4.4).
    DeviceCMYK,
    /// CalGray (8.6.5.2).
    CalGray(Cie<'o>),
    /// CalRGB (8.6.5.3).
    CalRGB(Cie<'o>),
    /// Lab (8.6.5.4).
    Lab(Cie<'o>),
    /// ICCBased (8.6.5.5).
    ICCBased(Icc<'o>),
    /// Indexed (8.6.6.3).
    Indexed(Indexed<'o>),
    /// Separation (8.6.6.4).
    Separation(Separation<'o>),
    /// DeviceN (8.6.6.5).
    DeviceN(DeviceN<'o>),
    /// Pattern (8.6.6.2), with its underlying space if it has one (8.7.3.3).
    Pattern(Option<Box<Space<'o>>>),
}

/// A CalGray, CalRGB or Lab space's parameters (8.6.5.2 to 8.6.5.4). Only the
/// white point is read to be held: `/BlackPoint`, `/Gamma`, `/Matrix` and
/// `/Range` are checked for a sink that wants faults, and left in the dictionary,
/// as nothing here converts.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Cie<'o> {
    /// Its dictionary.
    pub dict: Option<&'o Dict>,
    /// `/WhitePoint`, where it's three numbers, whatever they are.
    pub white_point: Option<[f64; 3]>,
}

/// An ICCBased space's parameters (8.6.5.5). Its profile isn't opened.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Icc<'o> {
    /// Its stream.
    pub stream: Option<&'o Stream>,
    /// `/N`, where it's 1, 3 or 4.
    pub n: Option<u32>,
    /// `/Alternate`.
    pub alternate: Option<Box<Space<'o>>>,
}

/// An Indexed space's parameters (8.6.6.3).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Indexed<'o> {
    /// Its base space.
    pub base: Option<Box<Space<'o>>>,
    /// Its high value, where it's from 0 to 255.
    pub hival: Option<u8>,
    /// Its table.
    pub lookup: Option<Lookup<'o>>,
}

/// An Indexed space's table (8.6.6.3).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Lookup<'o> {
    /// A string.
    String(&'o Str),
    /// A stream, whose data isn't read.
    Stream(&'o Stream),
}

/// A Separation space's parameters (8.6.6.4).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Separation<'o> {
    /// Its colorant, through its references.
    pub name: Option<&'o [u8]>,
    /// Its alternate space.
    pub alternate: Option<Box<Space<'o>>>,
    /// Its tint transform, as written: it's looked at only for a sink that wants
    /// faults, and nothing here evaluates it.
    pub tint: Option<&'o Object>,
}

/// A DeviceN space's parameters (8.6.6.5).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DeviceN<'o> {
    /// Its colorants, each through its references. An element that isn't a name
    /// has none here, and past [`Limits::space_visits`] the rest have none.
    pub names: Vec<&'o [u8]>,
    /// How many components it has: as many as its array of names has elements,
    /// whatever they are, as PDFium counts them; none where it has none.
    pub components: Option<u32>,
    /// Its alternate space.
    pub alternate: Option<Box<Space<'o>>>,
    /// Its tint transform, as written: it's looked at only for a sink that wants
    /// faults, and nothing here evaluates it.
    pub tint: Option<&'o Object>,
    /// Its attributes dictionary, read.
    pub attributes: Option<Attributes<'o>>,
}

/// A DeviceN space's attributes dictionary (8.6.6.5, Table 70).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Attributes<'o> {
    /// `/Subtype`, where it's one of the two; none written means DeviceN.
    pub subtype: Option<Subtype>,
    /// `/Colorants`: each entry a Separation space, read on asking
    /// ([`Attributes::separations`]).
    pub colorants: Option<&'o Dict>,
    /// `/Process`.
    pub process: Option<Process<'o>>,
    /// `/MixingHints`, left to who blends (Table 72).
    pub mixing_hints: Option<&'o Dict>,
}

/// An entry of a `/Colorants` dictionary, read
/// ([`Attributes::separations`]).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ColorantSpace<'o> {
    /// The key, the colorant's name.
    pub name: &'o [u8],
    /// The space it says.
    pub space: Option<Space<'o>>,
}

/// What a DeviceN space's attributes say it's for (Table 70).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Subtype {
    /// `DeviceN`, what's meant where it's not written.
    DeviceN,
    /// `NChannel`.
    NChannel,
}

/// A process dictionary (Table 71).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Process<'o> {
    /// `/ColorSpace`.
    pub space: Option<Box<Space<'o>>>,
    /// `/Components`, each through its references.
    pub components: Vec<&'o [u8]>,
}

/// What a colorant's name says of it (8.6.6.4, 8.6.6.5). A name alone can't say
/// more: that's what a caller reads a space's colorants through, each by
/// [`Colorant::of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Colorant {
    /// `Cyan`, one of the four that name a CMYK device's process colorants.
    Cyan,
    /// `Magenta`, one of the four.
    Magenta,
    /// `Yellow`, one of the four.
    Yellow,
    /// `Black`, one of the four.
    Black,
    /// `All`: every colorant the device has.
    All,
    /// `None`: nothing painted.
    None,
    /// Any other, which names a spot colorant.
    Spot,
}

impl Colorant {
    /// What `name` is.
    pub fn of(name: &[u8]) -> Colorant {
        match name {
            b"Cyan" => Colorant::Cyan,
            b"Magenta" => Colorant::Magenta,
            b"Yellow" => Colorant::Yellow,
            b"Black" => Colorant::Black,
            b"All" => Colorant::All,
            b"None" => Colorant::None,
            _ => Colorant::Spot,
        }
    }

    /// Whether it's one of the four that name a CMYK device's process colorants.
    pub fn is_process(self) -> bool {
        matches!(
            self,
            Colorant::Cyan | Colorant::Magenta | Colorant::Yellow | Colorant::Black
        )
    }
}

/// A space's colorants, each a name's bytes: a Separation space's one, a DeviceN
/// space's names, none of any other ([`Space::colorants`]).
#[derive(Debug, Clone)]
pub struct Colorants<'a, 'o> {
    names: std::slice::Iter<'a, &'o [u8]>,
}

impl<'o> Iterator for Colorants<'_, 'o> {
    type Item = &'o [u8];

    fn next(&mut self) -> Option<&'o [u8]> {
        self.names.next().copied()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.names.size_hint()
    }
}

impl ExactSizeIterator for Colorants<'_, '_> {}

impl<'o> Space<'o> {
    /// The color space `value` says, a name, an array or a reference to one, as
    /// ISO 32000-2 says it (8.6.3). Each departure is told to `sink`, and none is
    /// looked for where it wants none: `()` pays only to read what a [`Space`]
    /// holds.
    ///
    /// None where there's no space to speak of: a value of no family, or a
    /// reference to nothing. A name that isn't a family is one of them, as outside
    /// a content stream a space isn't named: [`Spaces`](crate::Spaces) looks a
    /// name up, and a reader of `cs` applies `DefaultGray` and the like (8.6.5.6).
    ///
    /// What's read is the same whatever the sink, an `Err` included. An object
    /// that's [`Why::Damaged`](crate::Why::Damaged) isn't there, and it's told as
    /// [`Fault::Damaged`](crate::Fault::Damaged); any other failure to read what a
    /// space holds is the `Err`, whole. What only a check reads gives no `Err`: a
    /// check whose read fails is left unsaid, so a sink that needs certainty reads
    /// the object itself. Checks have a [`Limits::space_visits`] of their own.
    ///
    /// ```
    /// use pdf_color::{Fault, Family, Limits, Object, ObjectId, Space};
    ///
    /// let gray = Object::from(vec![Object::from("Indexed"), Object::from("DeviceGray"), Object::from(1), Object::bytes(*b"\x00\xFF")]);
    /// let nothing = |_: ObjectId| None;
    /// let mut faults = Vec::new();
    /// let space = Space::read(nothing, &gray, &Limits::DEFAULT, &mut faults).unwrap().unwrap();
    /// assert_eq!(space.family(), Family::Indexed);
    /// assert_eq!(space.components(), Some(1));
    /// assert_eq!(space.base(), Some(&Space::DeviceGray));
    /// assert!(faults.is_empty());
    ///
    /// let short = Object::from(vec![Object::from("Indexed"), Object::from("DeviceGray"), Object::from(1), Object::bytes(*b"\x00")]);
    /// Space::read(nothing, &short, &Limits::DEFAULT, &mut faults).unwrap();
    /// assert_eq!(faults[0].1, Fault::LookupLength);
    /// ```
    pub fn read<R: Resolve<'o>, S: Sink + ?Sized>(
        objects: R,
        value: &'o Object,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Option<Space<'o>>, Unreadable> {
        Reader::new(objects, limits, sink).space(value, 0)
    }

    /// Its family.
    pub fn family(&self) -> Family {
        match self {
            Space::DeviceGray => Family::DeviceGray,
            Space::DeviceRGB => Family::DeviceRGB,
            Space::DeviceCMYK => Family::DeviceCMYK,
            Space::CalGray(_) => Family::CalGray,
            Space::CalRGB(_) => Family::CalRGB,
            Space::Lab(_) => Family::Lab,
            Space::ICCBased(_) => Family::ICCBased,
            Space::Indexed(_) => Family::Indexed,
            Space::Separation(_) => Family::Separation,
            Space::DeviceN(_) => Family::DeviceN,
            Space::Pattern(_) => Family::Pattern,
        }
    }

    /// How many values a color in it takes, by 8.6: 1, 3 or 4 for a device
    /// space; 1 for CalGray; 3 for CalRGB and Lab; `/N`, where it's 1, 3 or 4, for
    /// ICCBased; 1 for Indexed and Separation; a DeviceN space's
    /// [`components`](DeviceN::components); and for Pattern one more than its
    /// underlying space's, for the pattern's name, or 1 (8.7.3.3, 8.6.8). None
    /// where what it's read from is at fault.
    pub fn components(&self) -> Option<u32> {
        match self {
            Space::DeviceGray | Space::CalGray(_) => Some(1),
            Space::DeviceRGB | Space::CalRGB(_) | Space::Lab(_) => Some(3),
            Space::DeviceCMYK => Some(4),
            Space::ICCBased(icc) => icc.n,
            Space::Indexed(_) | Space::Separation(_) => Some(1),
            Space::DeviceN(device_n) => device_n.components,
            Space::Pattern(None) => Some(1),
            Space::Pattern(Some(under)) => under.components()?.checked_add(1),
        }
    }

    /// An Indexed space's base, or a Pattern space's underlying space.
    pub fn base(&self) -> Option<&Space<'o>> {
        match self {
            Space::Indexed(indexed) => indexed.base.as_deref(),
            Space::Pattern(under) => under.as_deref(),
            _ => None,
        }
    }

    /// An ICCBased, Separation or DeviceN space's alternate space.
    pub fn alternate(&self) -> Option<&Space<'o>> {
        match self {
            Space::ICCBased(icc) => icc.alternate.as_deref(),
            Space::Separation(separation) => separation.alternate.as_deref(),
            Space::DeviceN(device_n) => device_n.alternate.as_deref(),
            _ => None,
        }
    }

    /// A Separation space's colorant, or a DeviceN space's names: none of any
    /// other. Allocates nothing.
    pub fn colorants(&self) -> Colorants<'_, 'o> {
        let names: &[&'o [u8]] = match self {
            Space::Separation(Separation {
                name: Some(name), ..
            }) => std::slice::from_ref(name),
            Space::DeviceN(device_n) => &device_n.names,
            _ => &[],
        };
        Colorants {
            names: names.iter(),
        }
    }
}

impl<'o> Attributes<'o> {
    /// Each entry of `/Colorants`, a key written twice at its last entry, in
    /// order: a space not a Separation of that key's name is told as
    /// [`Fault::ColorantNotSeparation`](crate::Fault::ColorantNotSeparation), and
    /// given as it is.
    ///
    /// Read on asking, as [`Space::read`] holds only the dictionary: under one
    /// [`Limits::space_visits`] for the whole of it, past which the rest aren't
    /// given. Any failure to read but [`Why::Damaged`](crate::Why::Damaged) is the
    /// `Err`.
    pub fn separations<R: Resolve<'o>, S: Sink + ?Sized>(
        &self,
        objects: R,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Vec<ColorantSpace<'o>>, Unreadable> {
        let Some(colorants) = self.colorants else {
            return Ok(Vec::new());
        };
        Reader::new(objects, limits, sink).separations(colorants)
    }
}
