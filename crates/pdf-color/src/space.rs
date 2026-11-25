//! A color space as ISO 32000-2 says it (8.6), typed: its family, the spaces it
//! holds and its colorants, each departure a [`Fault`](crate::Fault) told to a
//! [`Sink`]. Nothing here converts a color, opens a profile or evaluates a function.
//!
//! [`Space::read`] is the other reading from [`components`](crate::components),
//! which is PDFium's and takes what ISO's doesn't. The two are tied by a promise:
//! a value read with a sink that wants faults, none told, has the same count by
//! both, where [`counted`](crate::counted) meets no cap, but for a Pattern space
//! over more than 16 components, which no clause refuses and PDFium does. It's why
//! a reference to a reference is a fault, a tint transform that doesn't load, and a
//! white point that PDFium's 32-bit reading makes 0.

use std::borrow::Cow;
use std::iter::FusedIterator;

use pdf_objects::fault::Sink;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Str, Stream, Streams, Unreadable, Why};

use crate::fault::Fault;
use crate::limits::Limits;

use read::Reader;

mod read;
#[cfg(test)]
mod tests;

/// A color space's family (ISO 32000-2, 8.6.3, Table 61).
///
/// `#[non_exhaustive]`, as a later edition of the standard can name a
/// family more, so a match on it has a wildcard.
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
    /// The family this name writes in full, or none: an inline image's
    /// abbreviations (`G`, `RGB`, `CMYK` and `I`; 8.9.7), a name that only
    /// begins as a family's (PDFium tells a family by its first four bytes) and
    /// any other name are none. [`Space::read`] takes an abbreviation as its
    /// family, telling [`Fault::Abbreviated`].
    #[must_use]
    pub const fn of(name: &[u8]) -> Option<Family> {
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
    #[must_use]
    pub const fn name(self) -> &'static str {
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
    #[must_use]
    pub const fn is_device(self) -> bool {
        matches!(
            self,
            Family::DeviceGray | Family::DeviceRGB | Family::DeviceCMYK
        )
    }

    /// Whether it's a CIE-based family, ICCBased among them.
    #[must_use]
    pub const fn is_cie(self) -> bool {
        matches!(
            self,
            Family::CalGray | Family::CalRGB | Family::Lab | Family::ICCBased
        )
    }

    /// Whether it's a special family.
    #[must_use]
    pub const fn is_special(self) -> bool {
        matches!(
            self,
            Family::Indexed | Family::Pattern | Family::Separation | Family::DeviceN
        )
    }
}

/// A color space, as ISO 32000-2 says it (8.6), read by [`Space::read`]. All it
/// holds borrows from the file, a nested space boxed, and each parameter is an
/// `Option`, as reading goes on past what's wrong.
///
/// `#[non_exhaustive]` for the reason [`Family`] is, so a match on it has a
/// wildcard.
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
///
/// `#[non_exhaustive]`, as each struct of a space's parameters is: read by
/// its fields and made only by this crate, so a parameter more can be held
/// in a minor release.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Cie<'o> {
    /// Its dictionary.
    pub dict: Option<&'o Dict>,
    /// `/WhitePoint`, where it's three numbers, whatever they are.
    pub white_point: Option<[f64; 3]>,
}

/// An ICCBased space's parameters (8.6.5.5). Its profile isn't opened.
///
/// `#[non_exhaustive]`, as [`Cie`] is.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Icc<'o> {
    /// The number of its stream, the object a reference to it ended in: what its
    /// data is asked for by ([`Streams::decoded`](pdf_objects::Streams::decoded)),
    /// and what an encrypted stream's key depends on. None for a stream that isn't
    /// numbered, as only a value built in memory can be.
    pub id: Option<ObjectId>,
    /// Its stream.
    pub stream: Option<&'o Stream>,
    /// `/N`, where it's 1, 3 or 4.
    pub n: Option<u32>,
    /// `/Alternate`.
    pub alternate: Option<Box<Space<'o>>>,
}

/// An Indexed space's parameters (8.6.6.3).
///
/// `#[non_exhaustive]`, as [`Cie`] is.
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
///
/// `#[non_exhaustive]`, though these are the two forms 8.6.6.3 gives a
/// table: making it exhaustive later is a minor release, where the other
/// way round would be a breaking one. `Stream` is too, so a field can join
/// it.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Lookup<'o> {
    /// A string.
    String(&'o Str),
    /// A stream, whose data isn't read.
    #[non_exhaustive]
    Stream {
        /// The stream.
        stream: &'o Stream,
        /// Its number, the object a reference to it ended in: what its data is
        /// asked for by ([`Streams::decoded`](pdf_objects::Streams::decoded)), and
        /// what an encrypted stream's key depends on. None for a stream that isn't
        /// numbered, as only a value built in memory can be.
        id: Option<ObjectId>,
    },
}

/// A Separation space's parameters (8.6.6.4).
///
/// `#[non_exhaustive]`, as [`Cie`] is.
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
///
/// `#[non_exhaustive]`, as [`Cie`] is.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DeviceN<'o> {
    /// Its colorants, each through its references, by position, so that `names[i]`
    /// is component `i`'s: none for an element that isn't a name. Past
    /// [`Limits::space_visits`] the rest aren't kept, so it can be shorter than
    /// [`components`](DeviceN::components).
    pub names: Vec<Option<&'o [u8]>>,
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
///
/// `#[non_exhaustive]`, as [`Cie`] is.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Attributes<'o> {
    /// `/Subtype`, where it's one of the two; none written means DeviceN.
    pub subtype: Option<Subtype>,
    /// `/Colorants`: each entry a Separation space, read on asking
    /// ([`Attributes::separations`]).
    pub colorants: Option<&'o Dict>,
    /// The object `/Colorants` is written in, for what's told of it on asking.
    colorants_in: Option<ObjectId>,
    /// `/Process`.
    pub process: Option<Process<'o>>,
    /// `/MixingHints`, left to who blends (Table 72).
    pub mixing_hints: Option<&'o Dict>,
}

/// An entry of a `/Colorants` dictionary, read
/// ([`Attributes::separations`]).
///
/// `#[non_exhaustive]`, as [`Cie`] is.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ColorantSpace<'o> {
    /// The key, the colorant's name.
    pub name: &'o [u8],
    /// The space it says.
    pub space: Option<Space<'o>>,
}

/// What a DeviceN space's attributes say it's for (Table 70).
///
/// `#[non_exhaustive]`, as a later edition of the standard can name a
/// subtype more, so a match on it has a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Subtype {
    /// `DeviceN`, what's meant where it's not written.
    DeviceN,
    /// `NChannel`.
    NChannel,
}

/// A process dictionary (Table 71).
///
/// `#[non_exhaustive]`, as [`Cie`] is.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Process<'o> {
    /// `/ColorSpace`.
    pub space: Option<Box<Space<'o>>>,
    /// `/Components`, each through its references, by position: none for an
    /// element that isn't a name, and past [`Limits::space_visits`] the rest aren't
    /// kept.
    pub components: Vec<Option<&'o [u8]>>,
}

/// What a colorant's name says of it (8.6.6.4, 8.6.6.5). A name alone can't say
/// more: that's what a caller reads a space's colorants through, each by
/// [`Colorant::of`].
///
/// `#[non_exhaustive]`, as a name may come to say more, by a later edition
/// of the standard or by what a space holds beside it, so a match on it has
/// a wildcard.
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
    /// What `name` is, whole: `cyan` is a spot.
    #[must_use]
    pub const fn of(name: &[u8]) -> Colorant {
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
    #[must_use]
    pub const fn is_process(self) -> bool {
        matches!(
            self,
            Colorant::Cyan | Colorant::Magenta | Colorant::Yellow | Colorant::Black
        )
    }
}

/// A space's colorants, each a name's bytes: a Separation space's one, a DeviceN
/// space's names, none of any other ([`Space::colorants`]).
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Colorants<'a, 'o> {
    names: std::slice::Iter<'a, Option<&'o [u8]>>,
}

impl<'o> Iterator for Colorants<'_, 'o> {
    type Item = &'o [u8];

    #[inline]
    fn next(&mut self) -> Option<&'o [u8]> {
        self.names.by_ref().find_map(|name| *name)
    }

    /// At most one an element left, as an element that's no name is passed
    /// over.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.names.size_hint().1)
    }
}

impl FusedIterator for Colorants<'_, '_> {}

impl<'o> Space<'o> {
    /// The color space `value` says, a name, an array or a reference to one, as
    /// ISO 32000-2 says it (8.6.3). Each departure is told to `sink`, and none is
    /// looked for where it wants none: `()` pays only to read what a [`Space`]
    /// holds.
    ///
    /// None where there's no space to speak of: a value of no family, one that's
    /// neither a name nor an array, a reference to nothing, or where a loop, a cap
    /// or a damaged object stops the reading at the top. With `()` a caller can't
    /// tell these apart. A name that isn't a family is one of them, as outside a
    /// content stream a space isn't named: [`Spaces`](crate::Spaces) looks a name
    /// up, and a reader of `cs` applies `DefaultGray` and the like (8.6.5.6).
    ///
    /// What's read is the same whatever the sink, an `Err` included. Checks
    /// have a [`Limits::space_visits`] of their own.
    ///
    /// ```
    /// use pdf_color::pdf_objects::{Object, ObjectId, resolve_fn};
    /// use pdf_color::{Fault, Family, Limits, Space};
    ///
    /// let gray = Object::from(vec![
    ///     Object::name("Indexed"),
    ///     Object::name("DeviceGray"),
    ///     Object::from(1),
    ///     Object::bytes(*b"\x00\xFF"),
    /// ]);
    /// let nothing = resolve_fn(|_: ObjectId| None);
    /// let mut faults = Vec::new();
    /// let space = Space::read(nothing, &gray, &Limits::DEFAULT, &mut faults).unwrap().unwrap();
    /// assert_eq!(space.family(), Family::Indexed);
    /// assert_eq!(space.components(), Some(1));
    /// assert_eq!(space.base(), Some(&Space::DeviceGray));
    /// assert!(faults.is_empty());
    ///
    /// let short = Object::from(vec![
    ///     Object::name("Indexed"),
    ///     Object::name("DeviceGray"),
    ///     Object::from(1),
    ///     Object::bytes(*b"\x00"),
    /// ]);
    /// Space::read(nothing, &short, &Limits::DEFAULT, &mut faults).unwrap();
    /// assert_eq!(faults[0].1, Fault::LookupLength);
    /// ```
    ///
    /// # Errors
    ///
    /// [`Unreadable`], whole, for any failure to read what a space holds but
    /// one that's [`Why::Damaged`](pdf_objects::Why::Damaged): an object past
    /// a reader's cap, locked, or a source that didn't give its bytes. An
    /// object that's damaged isn't there, and it's told as
    /// [`Fault::Damaged`](crate::Fault::Damaged). What only a check reads
    /// gives no `Err`: a check whose read fails is left unsaid, so a sink that
    /// needs certainty reads the object itself.
    pub fn read<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        objects: R,
        value: &'o Object,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Option<Space<'o>>, Unreadable> {
        Reader::new(objects, limits, sink).space(value, 0)
    }

    /// Its family.
    #[must_use]
    pub const fn family(&self) -> Family {
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
    ///
    /// It's a count of what was read. Read with `()`, which has no fault looked
    /// for, it says nothing of whether the space loads: `/CalGray` alone counts 1
    /// here, and PDFium's 3 by [`components`](crate::components).
    #[must_use]
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
    #[must_use]
    pub fn base(&self) -> Option<&Space<'o>> {
        match self {
            Space::Indexed(indexed) => indexed.base.as_deref(),
            Space::Pattern(under) => under.as_deref(),
            _ => None,
        }
    }

    /// An ICCBased, Separation or DeviceN space's alternate space.
    #[must_use]
    pub fn alternate(&self) -> Option<&Space<'o>> {
        match self {
            Space::ICCBased(icc) => icc.alternate.as_deref(),
            Space::Separation(separation) => separation.alternate.as_deref(),
            Space::DeviceN(device_n) => device_n.alternate.as_deref(),
            _ => None,
        }
    }

    /// A Separation space's colorant, or a DeviceN space's names, those that are
    /// names: none of any other. Allocates nothing. A colorant's position among a
    /// DeviceN space's is [`DeviceN::names`]'.
    pub fn colorants(&self) -> Colorants<'_, 'o> {
        let names: &[Option<&'o [u8]>] = match self {
            Space::Separation(separation) => std::slice::from_ref(&separation.name),
            Space::DeviceN(device_n) => &device_n.names,
            _ => &[],
        };
        Colorants {
            names: names.iter(),
        }
    }

    /// Default `/Decode` intervals `(min, max)` for a sampled image with `bits_per_component`
    /// (`1..=32`) in this color space (ISO 32000-2, §8.9.5.2, Table 90).
    ///
    /// Returns `Ok(None)` when `bits_per_component` is outside `1..=32`, when `self` is a
    /// `Pattern` space (which cannot be an image's `/ColorSpace`), or when component count is
    /// unknown due to a malformed space.
    ///
    /// # Errors
    ///
    /// Propagates [`Unreadable`] (other than [`Why::Damaged`]) when resolving indirect `/Range`
    /// arrays on `Lab` or `ICCBased` spaces.
    pub fn default_decode<R: Resolve<'o>>(
        &self,
        objects: R,
        bits_per_component: u8,
        limits: &Limits,
    ) -> Result<Option<Vec<(f64, f64)>>, Unreadable> {
        if !(1..=32).contains(&bits_per_component) {
            return Ok(None);
        }
        Ok(match self {
            Space::DeviceGray | Space::CalGray(_) | Space::Separation(_) => Some(vec![(0.0, 1.0)]),
            Space::DeviceRGB | Space::CalRGB(_) => Some(vec![(0.0, 1.0); 3]),
            Space::DeviceCMYK => Some(vec![(0.0, 1.0); 4]),
            Space::Lab(cie) => {
                let [amin, amax, bmin, bmax] = cie.lab_range(objects, limits)?;
                Some(vec![(0.0, 100.0), (amin, amax), (bmin, bmax)])
            }
            Space::ICCBased(icc) => {
                let Some(n) = icc.n else {
                    return Ok(None);
                };
                match icc.range(objects, limits)? {
                    Some(pairs) => Some(pairs),
                    None => Some(vec![(0.0, 1.0); n as usize]),
                }
            }
            Space::Indexed(_) => {
                let hi = ((1_u64 << bits_per_component) - 1) as f64;
                Some(vec![(0.0, hi)])
            }
            Space::DeviceN(dn) => dn.components.map(|n| vec![(0.0, 1.0); n as usize]),
            Space::Pattern(_) => None,
        })
    }
}

impl<'o> Cie<'o> {
    /// Reads `[amin, amax, bmin, bmax]` from a `Lab` dictionary's `/Range`, falling back to
    /// `[-100.0, 100.0, -100.0, 100.0]` when absent or invalid (ISO 32000-2, Table 64).
    ///
    /// # Errors
    ///
    /// Propagates [`Unreadable`] (other than [`Why::Damaged`]) when resolving indirect references.
    pub fn lab_range<R: Resolve<'o>>(
        &self,
        objects: R,
        limits: &Limits,
    ) -> Result<[f64; 4], Unreadable> {
        const DEFAULT: [f64; 4] = [-100.0, 100.0, -100.0, 100.0];
        let Some(dict) = self.dict else {
            return Ok(DEFAULT);
        };
        let Some(entry) = dict.get(b"Range").filter(|v| !v.is_null()) else {
            return Ok(DEFAULT);
        };
        let Some(nums) = read_number_vec(&objects, entry, 4, limits)? else {
            return Ok(DEFAULT);
        };
        let [a0, a1, b0, b1] = [nums[0], nums[1], nums[2], nums[3]];
        if a0 <= a1 && b0 <= b1 {
            Ok([a0, a1, b0, b1])
        } else {
            Ok(DEFAULT)
        }
    }
}

impl<'o> Icc<'o> {
    /// Reads the `n` `(min, max)` pairs of an `ICCBased` stream's `/Range` entry when valid
    /// (`min <= max`), or `Ok(None)` when absent or malformed (ISO 32000-2, Table 65).
    ///
    /// # Errors
    ///
    /// Propagates [`Unreadable`] (other than [`Why::Damaged`]) when resolving indirect references.
    pub fn range<R: Resolve<'o>>(
        &self,
        objects: R,
        limits: &Limits,
    ) -> Result<Option<Vec<(f64, f64)>>, Unreadable> {
        let (Some(stream), Some(n)) = (self.stream, self.n) else {
            return Ok(None);
        };
        let Some(entry) = stream.dict.get(b"Range").filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let count = (n as usize) * 2;
        let Some(nums) = read_number_vec(&objects, entry, count, limits)? else {
            return Ok(None);
        };
        let mut pairs = Vec::with_capacity(n as usize);
        for &[lo, hi] in nums.as_chunks::<2>().0 {
            if lo > hi {
                return Ok(None);
            }
            pairs.push((lo, hi));
        }
        Ok(Some(pairs))
    }
}

impl<'o> Indexed<'o> {
    /// Extracts the raw palette bytes of this `Indexed` space's lookup table up to `limit` bytes,
    /// whether stored as a string or an indirect/inline stream (§8.6.6.3).
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] when the string is still encrypted ([`Why::Locked`]), when the table
    /// exceeds `limit` ([`Why::Limit`]), or when stream decoding fails for a reason other than
    /// [`Why::Damaged`].
    pub fn lookup_bytes<'a, S: Streams + ?Sized>(
        &'a self,
        streams: &'a S,
        limit: usize,
    ) -> Result<Option<Cow<'a, [u8]>>, Unreadable>
    where
        'o: 'a,
    {
        match &self.lookup {
            Some(lookup) => lookup.bytes(streams, limit),
            None => Ok(None),
        }
    }
}

impl<'o> Lookup<'o> {
    /// Extracts the decoded bytes of this lookup table up to `limit` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] with [`Why::Locked`] for an un-decrypted string, [`Why::Limit`] if
    /// the byte length exceeds `limit`, or any non-[`Why::Damaged`] error from `streams.decoded`.
    pub fn bytes<'a, S: Streams + ?Sized>(
        &'a self,
        streams: &'a S,
        limit: usize,
    ) -> Result<Option<Cow<'a, [u8]>>, Unreadable>
    where
        'o: 'a,
    {
        let zero = ObjectId::new(0, 0);
        match self {
            Lookup::String(Str::Bytes(bytes)) => {
                if bytes.len() > limit {
                    return Err(Unreadable::new(
                        zero,
                        Why::Limit,
                        "Indexed lookup string exceeds byte limit",
                    ));
                }
                Ok(Some(Cow::Borrowed(bytes)))
            }
            Lookup::String(Str::Encrypted(_)) => Err(Unreadable::new(
                zero,
                Why::Locked,
                "Indexed lookup string is encrypted",
            )),
            Lookup::Stream { id: Some(id), .. } => match streams.decoded(*id, limit) {
                Ok(data) => Ok(Some(data)),
                Err(e) if e.why == Why::Damaged => Ok(None),
                Err(e) => Err(e),
            },
            Lookup::Stream { stream, id: None } => {
                if stream.dict.has(b"Filter") {
                    return Ok(None);
                }
                let Some(bytes) = stream.data.bytes() else {
                    return Ok(None);
                };
                if bytes.len() > limit {
                    return Err(Unreadable::new(
                        zero,
                        Why::Limit,
                        "inline Indexed lookup stream exceeds byte limit",
                    ));
                }
                Ok(Some(Cow::Borrowed(bytes)))
            }
        }
    }
}

pub(crate) fn resolve_bounded<'o, R: Resolve<'o>>(
    objects: &R,
    mut obj: &'o Object,
    visits: &mut usize,
    limits: &Limits,
) -> Result<Option<(&'o Object, Option<ObjectId>)>, Unreadable> {
    let mut path = Vec::new();
    while let Some(id) = obj.as_reference() {
        if path.contains(&id) || path.len() > limits.space_depth || *visits >= limits.space_visits {
            return Ok(None);
        }
        *visits += 1;
        match objects.try_resolve(id) {
            Ok(Some(next)) => {
                path.push(id);
                obj = next;
            }
            Ok(None) => return Ok(None),
            Err(e) if e.why == Why::Damaged => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    Ok(Some((obj, path.last().copied())))
}

pub(crate) fn read_number_vec<'o, R: Resolve<'o>>(
    objects: &R,
    written: &'o Object,
    expected_len: usize,
    limits: &Limits,
) -> Result<Option<Vec<f64>>, Unreadable> {
    let mut visits = 0;
    let Some((arr_obj, _)) = resolve_bounded(objects, written, &mut visits, limits)? else {
        return Ok(None);
    };
    let Some(items) = arr_obj.as_array().filter(|a| a.len() == expected_len) else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(expected_len);
    for item in items {
        let Some((num_obj, _)) = resolve_bounded(objects, item, &mut visits, limits)? else {
            return Ok(None);
        };
        let Some(n) = num_obj.as_number().filter(|v| v.is_finite()) else {
            return Ok(None);
        };
        out.push(n);
    }
    Ok(Some(out))
}

impl<'o> Attributes<'o> {
    /// Each entry of `/Colorants`, a key written twice at its last entry, in
    /// order: a space not a Separation of that key's name is told as
    /// [`Fault::ColorantNotSeparation`](crate::Fault::ColorantNotSeparation), and
    /// given as it is.
    ///
    /// Read on asking, as [`Space::read`] holds only the dictionary: under one
    /// [`Limits::space_visits`] for the whole of it, past which the rest aren't
    /// given. A fault's [`Place::object`](pdf_objects::fault::Place::object)
    /// is the object `/Colorants` is written in, or what an entry leads to, as
    /// [`Space::read`] tells it.
    ///
    /// # Errors
    ///
    /// [`Unreadable`], whole, for any failure to read but
    /// [`Why::Damaged`](pdf_objects::Why::Damaged): an object past a reader's
    /// cap, locked, or a source that didn't give its bytes.
    pub fn separations<R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
        &self,
        objects: R,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<Vec<ColorantSpace<'o>>, Unreadable> {
        let Some(colorants) = self.colorants else {
            return Ok(Vec::new());
        };
        Reader::new(objects, limits, sink).separations(colorants, self.colorants_in)
    }
}
