//! Page labels (ISO 32000-2, §12.4.2, Table 161).
//!
//! [`read`] decodes a catalog's `/PageLabels` number tree into sorted
//! [`LabelRange`] spans and formats any 0-based page index via
//! [`PageLabels::format`], matching PDFium (`FPDF_GetPageLabel` as of
//! chromium/8152) while reporting ISO 32000-2 departures as [`Fault`]s.
//! [`write()`] serializes label ranges back to a `/PageLabels` number-tree root
//! dictionary.

use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Str, Unreadable, Value, Why, dict, text};

use crate::info::Len;
use crate::phases::phase;
use crate::tree::{self, numbers, write_numbers};
use crate::{ByRef, Fault, HOPS, Limits, too_long};

/// Numbering style for the numeric portion of a page label (`/S`, ISO 32000-2
/// §12.4.2 Table 161).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelStyle {
    /// `/D`: decimal Arabic numerals (`1`, `2`, `3`, …).
    Decimal,
    /// `/R`: uppercase Roman numerals (`I`, `II`, `III`, …).
    UpperRoman,
    /// `/r`: lowercase Roman numerals (`i`, `ii`, `iii`, …).
    LowerRoman,
    /// `/A`: uppercase letters (`A`..`Z`, `AA`..`ZZ`, `AAA`..`ZZZ`, …).
    UpperAlpha,
    /// `/a`: lowercase letters (`a`..`z`, `aa`..`zz`, `aaa`..`zzz`, …).
    LowerAlpha,
}

impl LabelStyle {
    /// Parses a `/S` name entry.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"D" => Some(Self::Decimal),
            b"R" => Some(Self::UpperRoman),
            b"r" => Some(Self::LowerRoman),
            b"A" => Some(Self::UpperAlpha),
            b"a" => Some(Self::LowerAlpha),
            _ => None,
        }
    }

    /// Returns the PDF `/S` name token.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::Decimal => "D",
            Self::UpperRoman => "R",
            Self::LowerRoman => "r",
            Self::UpperAlpha => "A",
            Self::LowerAlpha => "a",
        }
    }
}

/// One contiguous page-label numbering range starting at `start_page` (0-based;
/// negative keys tolerated under `Rules::Pdfium` after emitting
/// [`Fault::PageLabelNegativeKey`]).
///
/// Its `Debug` implementation reports `prefix`'s byte length rather than raw
/// prefix text.
#[derive(Clone, PartialEq, Eq)]
pub struct LabelRange {
    /// 0-based page index where this range begins (`>= 0` in ISO 32000-2).
    pub start_page: i64,
    /// Numbering style (`/S`). When `None`, labels in this range consist solely
    /// of [`Self::prefix`].
    pub style: Option<LabelStyle>,
    /// Static label prefix (`/P`, empty by default).
    pub prefix: String,
    /// Value of the numeric portion for the first page in the range (`/St`,
    /// `1` by default; `>= 1` in ISO 32000-2).
    pub start_number: i64,
}

impl fmt::Debug for LabelRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LabelRange")
            .field("start_page", &self.start_page)
            .field("style", &self.style)
            .field("prefix", &Len(self.prefix.len()))
            .field("start_number", &self.start_number)
            .finish()
    }
}

impl LabelRange {
    /// Creates a range starting at 0-based `start_page` with `style`, empty
    /// prefix, and `start_number = 1`.
    #[must_use]
    pub const fn new(start_page: usize, style: Option<LabelStyle>) -> Self {
        Self {
            start_page: if start_page > i64::MAX as usize {
                i64::MAX
            } else {
                start_page as i64
            },
            style,
            prefix: String::new(),
            start_number: 1,
        }
    }

    /// Sets `/P` prefix text.
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Sets `/St` starting value.
    #[must_use]
    pub const fn with_start_number(mut self, start_number: i64) -> Self {
        self.start_number = start_number;
        self
    }

    fn format_offset(&self, offset: i64) -> String {
        let mut out = self.prefix.clone();
        let Some(style) = self.style else {
            return out;
        };
        let num = self.start_number.saturating_add(offset);
        match style {
            LabelStyle::Decimal => {
                use std::fmt::Write as _;
                let _ = write!(out, "{num}");
            }
            LabelStyle::UpperRoman => push_roman(&mut out, num, true),
            LabelStyle::LowerRoman => push_roman(&mut out, num, false),
            LabelStyle::UpperAlpha => push_alpha(&mut out, num, true),
            LabelStyle::LowerAlpha => push_alpha(&mut out, num, false),
        }
        out
    }
}

/// Formats Roman numerals matching PDFium's `MakeRoman` (`num %= 1_000_000`,
/// empty for `<= 0`, bounded to at most ~1,015 characters).
fn push_roman(out: &mut String, raw_num: i64, upper: bool) {
    if raw_num <= 0 {
        return;
    }
    let mut num = (raw_num % 1_000_000) as u32;
    if num == 0 {
        return;
    }
    const UPPER_TABLE: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    const LOWER_TABLE: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let table = if upper { &UPPER_TABLE } else { &LOWER_TABLE };
    for &(val, sym) in table {
        while num >= val {
            out.push_str(sym);
            num -= val;
        }
    }
}

/// Formats repeated alphabetic numerals matching ISO 32000-2 §12.4.2 Table 161
/// and PDFium's `MakeLetters` (`A`..`Z`, `AA`..`ZZ`, with repetition count
/// modulo `1_000` so untrusted `/St` values never allocate more than 999 bytes).
fn push_alpha(out: &mut String, raw_num: i64, upper: bool) {
    if raw_num <= 0 {
        return;
    }
    let zero_based = (raw_num - 1) as u64;
    let count = (((zero_based / 26) + 1) % 1000) as usize;
    if count == 0 {
        return;
    }
    let letter_offset = (zero_based % 26) as u8;
    let base = if upper { b'A' } else { b'a' };
    let ch = char::from(base + letter_offset);
    out.reserve(count);
    for _ in 0..count {
        out.push(ch);
    }
}

/// Decoded `/PageLabels` table for a document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageLabels {
    ranges: Vec<LabelRange>,
}

impl PageLabels {
    /// Constructs a [`PageLabels`] table from ranges, sorting by `start_page`
    /// (keeping the last entry on duplicate `start_page` keys).
    #[must_use]
    pub fn from_ranges(ranges: impl IntoIterator<Item = LabelRange>) -> Self {
        let mut vec: Vec<LabelRange> = ranges.into_iter().collect();
        vec.sort_by_key(|r| r.start_page);
        // Deduplicate keeping the last entry for equal `start_page`.
        let mut deduped: Vec<LabelRange> = Vec::with_capacity(vec.len());
        for range in vec {
            if let Some(last) = deduped.last_mut()
                && last.start_page == range.start_page
            {
                *last = range;
            } else {
                deduped.push(range);
            }
        }
        Self { ranges: deduped }
    }

    /// Sorted page-label ranges.
    #[must_use]
    pub fn ranges(&self) -> &[LabelRange] {
        &self.ranges
    }

    /// Returns `true` if no ranges were decoded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Formats the page label for 0-based `page_index`.
    ///
    /// If the first range starts after page `0`, pages before it receive
    /// 1-based decimal labels (`"1"`, `"2"`, …), matching PDFium's
    /// `FPDF_GetPageLabel`.
    #[must_use]
    pub fn format(&self, page_index: usize) -> String {
        let p = i64::try_from(page_index).unwrap_or(i64::MAX);
        let idx = self.ranges.partition_point(|r| r.start_page <= p);
        if idx == 0 {
            return (page_index.saturating_add(1)).to_string();
        }
        let range = &self.ranges[idx - 1];
        range.format_offset(p.saturating_sub(range.start_page))
    }
}

/// Reads catalog `/PageLabels` (ISO 32000-2 §12.4.2).
///
/// Returns `Ok(None)` when `/PageLabels` is absent, `null`, a dangling
/// reference, or not a dictionary.
///
/// # Errors
///
/// Returns [`Unreadable`] on reader caps, locked objects/strings, or reference
/// loops.
pub fn read<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Option<PageLabels>, Unreadable> {
    let Some(raw_root) = catalog.get(b"PageLabels") else {
        return Ok(None);
    };
    let Some((root_obj, root_id)) = reached(&objects, raw_root, None, sink)? else {
        return Ok(None);
    };
    if root_obj.as_dict().is_none() {
        say(sink, root_id, root_obj.at(), Fault::PageLabelsInvalid);
        return Ok(None);
    }

    let mut ranges = Vec::new();
    let mut tree_faults: Vec<(Place, Fault)> = Vec::new();
    let wants = sink.wants();
    {
        let walk = numbers(
            ByRef(&objects),
            raw_root,
            limits,
            TreeCollector {
                wants,
                out: &mut tree_faults,
            },
        );
        for item in walk {
            let (key, val) = item?;
            if key < 0 {
                say(sink, root_id, val.at(), Fault::PageLabelNegativeKey);
                if matches!(rules, Rules::Iso) {
                    continue;
                }
            }
            let Some((val_obj, val_id)) = reached(&objects, val, root_id, sink)? else {
                say(sink, root_id, val.at(), Fault::PageLabelsInvalid);
                continue;
            };
            let Some(dict) = val_obj.as_dict() else {
                say(sink, val_id, val_obj.at(), Fault::PageLabelsInvalid);
                continue;
            };
            if let Some(range) = read_range(&objects, dict, val_id, key, rules, sink)? {
                ranges.push(range);
            }
        }
    }
    if wants {
        for (place, fault) in tree_faults {
            sink.fault(place, fault);
        }
    }

    let labels = PageLabels::from_ranges(ranges);
    if wants && !labels.ranges().iter().any(|r| r.start_page == 0) {
        say(sink, root_id, root_obj.at(), Fault::PageLabelsInvalid);
    }
    phase!(DEBUG, "labels::read", ranges = labels.ranges().len());
    Ok(Some(labels))
}

struct TreeCollector<'a> {
    wants: bool,
    out: &'a mut Vec<(Place, Fault)>,
}

impl Sink<Fault> for TreeCollector<'_> {
    fn wants(&self) -> bool {
        self.wants
    }

    fn fault(&mut self, place: Place, fault: Fault) {
        self.out.push((place, fault));
    }
}

fn read_range<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    dict: &'o Dict,
    within: Option<ObjectId>,
    start_page: i64,
    rules: Rules,
    sink: &mut S,
) -> Result<Option<LabelRange>, Unreadable> {
    let style = match dict.get(b"S") {
        Some(raw) => match reached(objects, raw, within, sink)? {
            Some((obj, sid)) => match obj.as_name() {
                Some(name) => match LabelStyle::from_name(name) {
                    Some(st) => Some(st),
                    None => {
                        say(sink, sid, obj.at(), Fault::PageLabelWrongType { key: "S" });
                        None
                    }
                },
                None => {
                    say(sink, sid, obj.at(), Fault::PageLabelWrongType { key: "S" });
                    None
                }
            },
            None => None,
        },
        None => None,
    };

    let prefix = match dict.get(b"P") {
        Some(raw) => match reached(objects, raw, within, sink)? {
            Some((obj, pid)) => match obj.value() {
                Value::String(Str::Bytes(bytes), _) => text::decode_str(bytes, rules),
                Value::String(Str::Encrypted(_), _) => {
                    let oid = pid.unwrap_or(ObjectId::new(0, 0));
                    return Err(Unreadable::new(
                        oid,
                        Why::Locked,
                        format!("{oid} R holds /P as ciphertext: it takes a password"),
                    ));
                }
                _ => {
                    say(sink, pid, obj.at(), Fault::PageLabelWrongType { key: "P" });
                    String::new()
                }
            },
            None => String::new(),
        },
        None => String::new(),
    };

    let start_number = match dict.get(b"St") {
        Some(raw) => match reached(objects, raw, within, sink)? {
            Some((obj, stid)) => match obj.as_integer() {
                Some(n) => {
                    if n < 1 {
                        say(sink, stid, obj.at(), Fault::PageLabelInvalidStart);
                        if matches!(rules, Rules::Iso) { 1 } else { n }
                    } else {
                        n
                    }
                }
                None => {
                    say(
                        sink,
                        stid,
                        obj.at(),
                        Fault::PageLabelWrongType { key: "St" },
                    );
                    1
                }
            },
            None => 1,
        },
        None => 1,
    };

    Ok(Some(LabelRange {
        start_page,
        style,
        prefix,
        start_number,
    }))
}

fn say<S: Sink<Fault> + ?Sized>(
    sink: &mut S,
    id: Option<ObjectId>,
    at: Option<Span>,
    fault: Fault,
) {
    if sink.wants() {
        sink.fault(Place::new(id, at), fault);
    }
}

fn reached<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: &R,
    value: &'o Object,
    within: Option<ObjectId>,
    sink: &mut S,
) -> Result<Option<(&'o Object, Option<ObjectId>)>, Unreadable> {
    let (mut at, mut last) = (value, None);
    for _ in 0..HOPS {
        match at.value() {
            Value::Ref(id) => {
                last = Some(*id);
                match objects.try_resolve(*id) {
                    Ok(Some(obj)) => at = obj,
                    Ok(None) => return Ok(None),
                    Err(u) if u.why == Why::Damaged => {
                        say(sink, within, value.at(), Fault::Damaged { id: *id });
                        return Ok(None);
                    }
                    Err(u) => return Err(u),
                }
            }
            Value::Null => return Ok(None),
            _ => return Ok(Some((at, last.or(within)))),
        }
    }
    Err(too_long(last.unwrap_or(ObjectId::new(0, 0))))
}

/// Reason [`write()`] refused a page-label table.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// A range had `start_page < 0` (forbidden by ISO 32000-2 §12.4.2).
    #[error("a page-label range has negative start_page {start_page}")]
    NegativeStartPage {
        /// Offending 0-based page index.
        start_page: i64,
    },
    /// A range had `start_number < 1` (forbidden by ISO 32000-2 §12.4.2 Table 161).
    #[error("a page-label range has start_number {start_number} (< 1)")]
    InvalidStartNumber {
        /// Offending `/St` value.
        start_number: i64,
    },
    /// Two ranges shared the same `start_page` index.
    #[error("two page-label ranges start at page {start_page}")]
    DuplicateStartPage {
        /// Repeated 0-based page index.
        start_page: usize,
    },
}

/// Serializes `ranges` into a flat `/PageLabels` number-tree root dictionary.
///
/// # Errors
///
/// Returns [`WriteError`] if any range has `start_page < 0`, `start_number < 1`,
/// or duplicate `start_page` indices.
pub fn write(ranges: &[LabelRange]) -> Result<Dict, WriteError> {
    let mut entries = Vec::with_capacity(ranges.len());
    for range in ranges {
        if range.start_page < 0 {
            return Err(WriteError::NegativeStartPage {
                start_page: range.start_page,
            });
        }
        if range.start_number < 1 {
            return Err(WriteError::InvalidStartNumber {
                start_number: range.start_number,
            });
        }
        let mut d = dict! {
            "Type" => Object::name("PageLabel"),
        };
        if let Some(style) = range.style {
            d.set("S", Object::name(style.as_name()));
        }
        if !range.prefix.is_empty() {
            d.set("P", Object::text(&range.prefix));
        }
        if range.start_number != 1 {
            d.set("St", range.start_number);
        }
        entries.push((range.start_page, Object::from(d)));
    }
    write_numbers(entries).map_err(|err| match err {
        tree::WriteError::RepeatedNumber { key } => WriteError::DuplicateStartPage {
            start_page: usize::try_from(key).unwrap_or(0),
        },
        _ => WriteError::DuplicateStartPage { start_page: 0 },
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId, dict};

    use super::{LabelRange, LabelStyle, read, write};
    use crate::Limits;

    #[test]
    fn round_trip_page_labels_and_formatting() {
        let ranges = vec![
            LabelRange::new(0, Some(LabelStyle::LowerRoman)),
            LabelRange::new(4, Some(LabelStyle::Decimal)),
            LabelRange::new(7, Some(LabelStyle::Decimal))
                .with_prefix("A-")
                .with_start_number(8),
            LabelRange::new(10, Some(LabelStyle::UpperAlpha)).with_start_number(26),
            LabelRange::new(13, None).with_prefix("Cover"),
        ];
        let tree_dict = write(&ranges).unwrap();
        let catalog = dict! { "PageLabels" => tree_dict };
        let objects: BTreeMap<ObjectId, Object> = BTreeMap::new();

        let mut faults = Vec::new();
        let labels = read(
            &objects,
            &catalog,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap()
        .unwrap();
        assert!(faults.is_empty(), "{faults:?}");
        let got: Vec<String> = (0..15).map(|i| labels.format(i)).collect();
        assert_eq!(
            got,
            [
                "i", "ii", "iii", "iv", "1", "2", "3", "A-8", "A-9", "A-10", "Z", "AA", "BB",
                "Cover", "Cover"
            ]
        );

        let dbg = format!("{ranges:?}");
        assert!(!dbg.contains("Cover"), "{dbg}");
    }
}
