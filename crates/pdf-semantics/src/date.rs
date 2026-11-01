//! Dates (ISO 32000-2, 7.9.4): `D:` and a year, then as many of month, day,
//! hour, minute, second and an offset from UT as the date writes, each only
//! after those before it.
//!
//! [`Date::read`] takes what a date's text says, and says in
//! [`Date::quirks`] what it read past on the way ([`Quirk`]); what it can't
//! read past makes it no date ([`Error`]). A field not written is none
//! ([`Date::month`]), and what stands for it is 7.9.4's default
//! ([`Date::month_or_default`]). [`Date::write`] writes the clause's form
//! of the fields, never the bytes read.
//!
//! ```
//! use pdf_semantics::date::{Date, Offset, Quirk};
//!
//! let date = Date::read("D:20261024153000-08'00'").unwrap();
//! assert_eq!(date.month(), Some(10));
//! assert_eq!(date.offset(), Some(Offset::Behind { hours: 8, minutes: 0 }));
//! assert_eq!(date.quirks(), [Quirk::TerminatingApostrophe]);
//! assert_eq!(date.write(), "D:20261024153000-08'00");
//!
//! let date = Date::read("D:2026").unwrap();
//! assert_eq!((date.month(), date.month_or_default()), (None, 1));
//!
//! let date = Date::new(2026).unwrap().with_month(10).unwrap();
//! assert_eq!(date.write(), "D:202610");
//! ```

use std::fmt;

/// A date: its year, as many of its other fields as it has, and what reading
/// it read past. Its fields are private, so that it always writes as a date:
/// it's had from [`read`](Self::read), [`new`](Self::new) and the `with_`
/// methods, or with the `clock` feature, `now` and `from_unix`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Date {
    year: u16,
    /// Month, day, hour, minute and second, as [`FIELDS`] has them: those
    /// written come first, and all of them are there under an offset.
    fields: [Option<u8>; 5],
    offset: Option<Offset>,
    quirks: Vec<Quirk>,
}

/// How a date's local time stands to UT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Offset {
    /// `Z`: it's equal to UT.
    Utc,
    /// `+`: it's this far ahead of UT.
    Ahead {
        /// Hours, 0 to 23.
        hours: u8,
        /// Minutes, 0 to 59.
        minutes: u8,
    },
    /// `-`: it's this far behind UT.
    Behind {
        /// Hours, 0 to 23.
        hours: u8,
        /// Minutes, 0 to 59.
        minutes: u8,
    },
}

/// One of a date's fields, as an error names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Field {
    /// The year, 0 to 9999.
    Year,
    /// The month, 1 to 12.
    Month,
    /// The day, 1 to 31, and one its month has when it's being set.
    Day,
    /// The hour, 0 to 23.
    Hour,
    /// The minute, 0 to 59.
    Minute,
    /// The second, 0 to 59.
    Second,
    /// The offset as a whole, which comes after the second.
    Offset,
    /// The offset's hours, 0 to 23.
    OffsetHours,
    /// The offset's minutes, 0 to 59.
    OffsetMinutes,
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Field::Year => "year",
            Field::Month => "month",
            Field::Day => "day",
            Field::Hour => "hour",
            Field::Minute => "minute",
            Field::Second => "second",
            Field::Offset => "offset",
            Field::OffsetHours => "offset's hours",
            Field::OffsetMinutes => "offset's minutes",
        })
    }
}

/// Why a text is no date, or a field can't be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// It doesn't begin `D:`.
    #[error("a date begins D:")]
    NoPrefix,
    /// `D:` isn't followed by a year's four digits.
    #[error("a date's D: is followed by a year's four digits")]
    NoYear,
    /// A field outside its range.
    #[error("a date's {field} out of its range")]
    OutOfRange {
        /// Which.
        field: Field,
    },
    /// A field set before the one that comes before it, which 7.9.4 doesn't
    /// let a date leave out.
    #[error("a date's {field} set before the field before it")]
    OutOfOrder {
        /// Which.
        field: Field,
    },
}

/// What reading a date read past, in the order it comes. Each departs from
/// 7.9.4's form, and [`Date::write`] keeps none of them, but for
/// [`ImpossibleDay`](Quirk::ImpossibleDay).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Quirk {
    /// An offset's hours with no `Z`, `+` or `-` before them: which way isn't
    /// said, so the date has no offset.
    OffsetWithoutSign,
    /// An offset where a field before it isn't written, the fields before it
    /// then each taking its default.
    OffsetBeforeFields,
    /// `Z` followed by hours or minutes other than zero, which it can't have:
    /// the date's offset is [`Offset::Utc`].
    UtcWithOffset,
    /// An apostrophe with no hours before it, any minutes after it being the
    /// offset's.
    ApostropheWithoutHours,
    /// An offset's minutes with no apostrophe before them.
    MissingApostrophe,
    /// An apostrophe after the offset's minutes, which PDF up to 1.7 wrote.
    TerminatingApostrophe,
    /// A digit alone where a field or the offset's next two digits would be.
    LoneDigit,
    /// Something after the date.
    Trailing,
    /// A day in 7.9.4's range that its month hasn't: 30 February. The date
    /// keeps it, so reading it back says it again.
    ImpossibleDay,
}

/// Month, day, hour, minute and second: which each is, its range and its
/// default (7.9.4).
const FIELDS: [(Field, u8, u8, u8); 5] = [
    (Field::Month, 1, 12, 1),
    (Field::Day, 1, 31, 1),
    (Field::Hour, 0, 23, 0),
    (Field::Minute, 0, 59, 0),
    (Field::Second, 0, 59, 0),
];

const MONTH: usize = 0;
const DAY: usize = 1;
const HOUR: usize = 2;
const MINUTE: usize = 3;
const SECOND: usize = 4;

/// `value` if it's in its range, for `field`.
fn in_range(value: u8, low: u8, high: u8, field: Field) -> Result<u8, Error> {
    if (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(Error::OutOfRange { field })
    }
}

/// The days of a month, by the Gregorian calendar.
fn days_in(year: u16, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    }
}

/// `count` digits at the start of `text`, as a number, and what's after them.
fn digits(text: &[u8], count: usize) -> Option<(u16, &[u8])> {
    let (number, rest) = text.split_at_checked(count)?;
    number.iter().all(u8::is_ascii_digit).then(|| {
        let number = number.iter().fold(0, |n, d| n * 10 + u16::from(d - b'0'));
        (number, rest)
    })
}

/// The two digits at the start of `*text`, if they're there, moving past them.
fn two(text: &mut &[u8]) -> Option<u8> {
    let (number, rest) = digits(text, 2)?;
    *text = rest;
    Some(number as u8)
}

/// The first byte of `*text` if `wanted` says so, moving past it.
fn take(text: &mut &[u8], wanted: impl Fn(u8) -> bool) -> Option<u8> {
    let (&first, rest) = text.split_first()?;
    wanted(first).then(|| {
        *text = rest;
        first
    })
}

impl Date {
    /// Reads a date from its text, the text string that holds it decoded
    /// (7.9.2.2): `D:` and a year's four digits; then as many of month, day,
    /// hour, minute and second as follow, two digits each; then `Z`, `+` or
    /// `-`, its hours, an apostrophe and its minutes, each if it's there.
    /// Anything after that is read past. A field out of its range makes it no
    /// date, wherever it comes; every other departure from 7.9.4's form is one
    /// of its [`quirks`](Self::quirks).
    pub fn read(text: &str) -> Result<Date, Error> {
        let text = text.strip_prefix("D:").ok_or(Error::NoPrefix)?;
        let (year, mut rest) = digits(text.as_bytes(), 4).ok_or(Error::NoYear)?;
        let mut date = Date::at(year, [None; 5], None);

        for (at, &(field, low, high, _)) in FIELDS.iter().enumerate() {
            let Some(value) = two(&mut rest) else {
                break;
            };
            date.fields[at] = Some(in_range(value, low, high, field)?);
        }
        date.said_of_its_day();

        let sign = take(&mut rest, |c| b"Z+-".contains(&c));
        let hours = two(&mut rest)
            .map(|n| in_range(n, 0, 23, Field::OffsetHours))
            .transpose()?;
        let apostrophe = take(&mut rest, |c| c == b'\'').is_some();
        let minutes = two(&mut rest)
            .map(|n| in_range(n, 0, 59, Field::OffsetMinutes))
            .transpose()?;

        let (h, m) = (hours.unwrap_or(0), minutes.unwrap_or(0));
        date.offset = match sign {
            Some(b'Z') => Some(Offset::Utc),
            Some(b'+') => Some(Offset::Ahead {
                hours: h,
                minutes: m,
            }),
            Some(b'-') => Some(Offset::Behind {
                hours: h,
                minutes: m,
            }),
            _ => None,
        };

        if sign.is_none() && hours.is_some() {
            date.quirks.push(Quirk::OffsetWithoutSign);
        }
        if sign.is_some() && date.fields.contains(&None) {
            date.quirks.push(Quirk::OffsetBeforeFields);
            for (value, &(.., default)) in date.fields.iter_mut().zip(&FIELDS) {
                value.get_or_insert(default);
            }
        }
        if sign == Some(b'Z') && (h, m) != (0, 0) {
            date.quirks.push(Quirk::UtcWithOffset);
        }
        if apostrophe && hours.is_none() {
            date.quirks.push(Quirk::ApostropheWithoutHours);
        }
        if minutes.is_some() && !apostrophe {
            date.quirks.push(Quirk::MissingApostrophe);
        }
        if minutes.is_some() && rest == b"'" {
            date.quirks.push(Quirk::TerminatingApostrophe);
            rest = &[];
        }
        if rest.first().is_some_and(u8::is_ascii_digit)
            && !rest.get(1).is_some_and(u8::is_ascii_digit)
        {
            date.quirks.push(Quirk::LoneDigit);
            rest = &rest[1..];
        }
        if !rest.is_empty() {
            date.quirks.push(Quirk::Trailing);
        }
        Ok(date)
    }

    fn at(year: u16, fields: [Option<u8>; 5], offset: Option<Offset>) -> Date {
        Date {
            year,
            fields,
            offset,
            quirks: Vec::new(),
        }
    }

    /// A date of a year alone, 0 to 9999.
    pub fn new(year: u16) -> Result<Date, Error> {
        if year > 9999 {
            return Err(Error::OutOfRange { field: Field::Year });
        }
        Ok(Date::at(year, [None; 5], None))
    }

    /// Sets one of [`FIELDS`], in its range and with the one before it set.
    fn with(mut self, at: usize, value: u8) -> Result<Date, Error> {
        let (field, low, high, _) = FIELDS[at];
        let value = in_range(value, low, high, field)?;
        if at > 0 && self.fields[at - 1].is_none() {
            return Err(Error::OutOfOrder { field });
        }
        self.fields[at] = Some(value);
        self.quirks.clear();
        self.said_of_its_day();
        Ok(self)
    }

    /// Says [`Quirk::ImpossibleDay`] where a date read has such a day still: a
    /// `with_` asks only of the fields it sets, and reading what it writes back
    /// says so, so that it's the same date.
    fn said_of_its_day(&mut self) {
        if let (Some(month), Some(day)) = (self.fields[MONTH], self.fields[DAY]) {
            if day > days_in(self.year, month) {
                self.quirks.push(Quirk::ImpossibleDay);
            }
        }
    }

    /// This date in another month, 1 to 12, which must have its day if it has
    /// one. What reading it read past is left behind, as with each `with_`, but
    /// that a day its month hasn't is still said, if another `with_` kept it.
    pub fn with_month(self, month: u8) -> Result<Date, Error> {
        let date = self.with(MONTH, month)?;
        if date.fields[DAY].is_some_and(|day| day > days_in(date.year, month)) {
            return Err(Error::OutOfRange { field: Field::Day });
        }
        Ok(date)
    }

    /// This date on another day, one its month has; it must have a month.
    pub fn with_day(self, day: u8) -> Result<Date, Error> {
        let date = self.with(DAY, day)?;
        if date.fields[MONTH].is_some_and(|month| day > days_in(date.year, month)) {
            return Err(Error::OutOfRange { field: Field::Day });
        }
        Ok(date)
    }

    /// This date at another hour, 0 to 23; it must have a day.
    pub fn with_hour(self, hour: u8) -> Result<Date, Error> {
        self.with(HOUR, hour)
    }

    /// This date at another minute, 0 to 59; it must have an hour.
    pub fn with_minute(self, minute: u8) -> Result<Date, Error> {
        self.with(MINUTE, minute)
    }

    /// This date at another second, 0 to 59; it must have a minute.
    pub fn with_second(self, second: u8) -> Result<Date, Error> {
        self.with(SECOND, second)
    }

    /// This date at another offset, hours 0 to 23 and minutes 0 to 59; it
    /// must have a second.
    pub fn with_offset(mut self, offset: Offset) -> Result<Date, Error> {
        if let Offset::Ahead { hours, minutes } | Offset::Behind { hours, minutes } = offset {
            in_range(hours, 0, 23, Field::OffsetHours)?;
            in_range(minutes, 0, 59, Field::OffsetMinutes)?;
        }
        if self.fields[SECOND].is_none() {
            return Err(Error::OutOfOrder {
                field: Field::Offset,
            });
        }
        self.offset = Some(offset);
        self.quirks.clear();
        self.said_of_its_day();
        Ok(self)
    }

    /// The date in 7.9.4's form, its fields as far as it has them: an
    /// offset's sign, its hours, an apostrophe and its minutes, or `Z` alone.
    /// So reading it back gives this date's fields, and no quirks but
    /// [`Quirk::ImpossibleDay`].
    pub fn write(&self) -> String {
        use fmt::Write;
        let mut out = format!("D:{:04}", self.year);
        for field in self.fields.iter().map_while(|field| *field) {
            let _ = write!(out, "{field:02}");
        }
        match self.offset {
            None => {}
            Some(Offset::Utc) => out.push('Z'),
            Some(Offset::Ahead { hours, minutes }) => {
                let _ = write!(out, "+{hours:02}'{minutes:02}");
            }
            Some(Offset::Behind { hours, minutes }) => {
                let _ = write!(out, "-{hours:02}'{minutes:02}");
            }
        }
        out
    }

    /// The year, 0 to 9999.
    pub fn year(&self) -> u16 {
        self.year
    }

    /// The month, 1 to 12, if the date writes one.
    pub fn month(&self) -> Option<u8> {
        self.fields[MONTH]
    }

    /// The day, 1 to 31, if the date writes one.
    pub fn day(&self) -> Option<u8> {
        self.fields[DAY]
    }

    /// The hour, 0 to 23, if the date writes one.
    pub fn hour(&self) -> Option<u8> {
        self.fields[HOUR]
    }

    /// The minute, 0 to 59, if the date writes one.
    pub fn minute(&self) -> Option<u8> {
        self.fields[MINUTE]
    }

    /// The second, 0 to 59, if the date writes one.
    pub fn second(&self) -> Option<u8> {
        self.fields[SECOND]
    }

    /// The offset from UT, if the date writes one.
    pub fn offset(&self) -> Option<Offset> {
        self.offset
    }

    fn or_default(&self, at: usize) -> u8 {
        self.fields[at].unwrap_or(FIELDS[at].3)
    }

    /// The month, or what stands for one not written: 1.
    pub fn month_or_default(&self) -> u8 {
        self.or_default(MONTH)
    }

    /// The day, or what stands for one not written: 1.
    pub fn day_or_default(&self) -> u8 {
        self.or_default(DAY)
    }

    /// The hour, or what stands for one not written: 0.
    pub fn hour_or_default(&self) -> u8 {
        self.or_default(HOUR)
    }

    /// The minute, or what stands for one not written: 0.
    pub fn minute_or_default(&self) -> u8 {
        self.or_default(MINUTE)
    }

    /// The second, or what stands for one not written: 0.
    pub fn second_or_default(&self) -> u8 {
        self.or_default(SECOND)
    }

    /// The offset, or what stands for one not written: UT's own.
    pub fn offset_or_default(&self) -> Offset {
        self.offset.unwrap_or(Offset::Utc)
    }

    /// What reading it read past, in the order it comes; none for a date
    /// built, or set but for a day its month hasn't, which it says still.
    pub fn quirks(&self) -> &[Quirk] {
        &self.quirks
    }

    /// The date and time by the clock, in UT, to the second, every field
    /// written. A clock before 1970 or past 9999 reads as 1970's start.
    #[cfg(feature = "clock")]
    pub fn now() -> Date {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|since| i64::try_from(since.as_secs()).ok());
        (seconds.and_then(Date::from_unix))
            .unwrap_or_else(|| Date::at(1970, [1, 1, 0, 0, 0].map(Some), Some(Offset::Utc)))
    }

    /// The date and time this many seconds from 1970's start, in UT, every
    /// field written; none before year 0 or past 9999.
    #[cfg(feature = "clock")]
    pub fn from_unix(seconds: i64) -> Option<Date> {
        use chrono::{Datelike, Timelike};
        let at = chrono::DateTime::from_timestamp(seconds, 0)?;
        let year = u16::try_from(at.year()).ok().filter(|year| *year <= 9999)?;
        let fields = [at.month(), at.day(), at.hour(), at.minute(), at.second()];
        Some(Date::at(
            year,
            fields.map(|f| u8::try_from(f).ok()),
            Some(Offset::Utc),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ahead(hours: u8, minutes: u8) -> Offset {
        Offset::Ahead { hours, minutes }
    }

    const fn behind(hours: u8, minutes: u8) -> Offset {
        Offset::Behind { hours, minutes }
    }

    #[test]
    fn a_date_reads_as_the_checks_table_has_it() {
        for (date, valid) in [
            ("D:2026", true),
            ("D:20261024", true),
            ("D:20261024153000Z", true),
            ("D:20261024153000+05'30'", true),
            ("D:20261024153000-08'00", true),
            ("D:2026102415", true),
            ("D:202610241530007", true),
            ("D:20261324", false),
            ("D:20261000", false),
            ("D:20261024246000", false),
            ("D:20261024153000+24'00'", false),
            ("D:20261024153000+05'60'", false),
            ("D:2026102415300099", false),
            ("D:26", false),
            ("20261024", false),
        ] {
            assert_eq!(Date::read(date).is_ok(), valid, "{date}");
        }
    }

    #[test]
    fn each_field_read_is_there_and_the_rest_arent() {
        let all = Date::read("D:19991231235958Z").unwrap();
        assert_eq!(all.year(), 1999);
        assert_eq!(all.month(), Some(12));
        assert_eq!(all.day(), Some(31));
        assert_eq!(all.hour(), Some(23));
        assert_eq!(all.minute(), Some(59));
        assert_eq!(all.second(), Some(58));
        assert_eq!(all.offset(), Some(Offset::Utc));
        assert_eq!(all.quirks(), []);

        let some = Date::read("D:199912").unwrap();
        assert_eq!(some.year(), 1999);
        assert_eq!(some.month(), Some(12));
        assert_eq!(some.day(), None);
        assert_eq!(some.hour(), None);
        assert_eq!(some.minute(), None);
        assert_eq!(some.second(), None);
        assert_eq!(some.offset(), None);
        assert_eq!(some.quirks(), []);
    }

    #[test]
    fn what_isnt_written_takes_its_default() {
        let date = Date::read("D:1999").unwrap();
        assert_eq!(date.month_or_default(), 1);
        assert_eq!(date.day_or_default(), 1);
        assert_eq!(date.hour_or_default(), 0);
        assert_eq!(date.minute_or_default(), 0);
        assert_eq!(date.second_or_default(), 0);
        assert_eq!(date.offset_or_default(), Offset::Utc);

        let date = Date::read("D:19991231235958-08'30").unwrap();
        assert_eq!(date.month_or_default(), 12);
        assert_eq!(date.day_or_default(), 31);
        assert_eq!(date.hour_or_default(), 23);
        assert_eq!(date.minute_or_default(), 59);
        assert_eq!(date.second_or_default(), 58);
        assert_eq!(date.offset_or_default(), behind(8, 30));
    }

    #[test]
    fn an_offset_reads_as_its_sign_says_its_parts_each_optional() {
        for (text, offset) in [
            ("D:20261024153000Z", Offset::Utc),
            ("D:20261024153000Z00'00", Offset::Utc),
            ("D:20261024153000+", ahead(0, 0)),
            ("D:20261024153000-", behind(0, 0)),
            ("D:20261024153000+05", ahead(5, 0)),
            ("D:20261024153000-05'", behind(5, 0)),
            ("D:20261024153000+05'30", ahead(5, 30)),
            ("D:20261024153000-23'59", behind(23, 59)),
        ] {
            let date = Date::read(text).unwrap();
            assert_eq!(date.offset(), Some(offset), "{text}");
            assert_eq!(date.quirks(), [], "{text}");
        }
    }

    #[test]
    fn what_isnt_a_date_says_why() {
        use Error::{NoPrefix, NoYear, OutOfRange};
        for (text, error) in [
            ("", NoPrefix),
            ("2026", NoPrefix),
            (" D:2026", NoPrefix),
            ("d:2026", NoPrefix),
            ("D", NoPrefix),
            ("D:", NoYear),
            ("D:202", NoYear),
            ("D:20a6", NoYear),
            ("D:+026", NoYear),
            (
                "D:202600",
                OutOfRange {
                    field: Field::Month,
                },
            ),
            (
                "D:202613",
                OutOfRange {
                    field: Field::Month,
                },
            ),
            ("D:20261200", OutOfRange { field: Field::Day }),
            ("D:20261232", OutOfRange { field: Field::Day }),
            ("D:2026123124", OutOfRange { field: Field::Hour }),
            (
                "D:202612312360",
                OutOfRange {
                    field: Field::Minute,
                },
            ),
            (
                "D:20261231235960",
                OutOfRange {
                    field: Field::Second,
                },
            ),
            (
                "D:20261231235959+24",
                OutOfRange {
                    field: Field::OffsetHours,
                },
            ),
            (
                "D:20261231235959-24'00",
                OutOfRange {
                    field: Field::OffsetHours,
                },
            ),
            (
                "D:20261231235959Z24",
                OutOfRange {
                    field: Field::OffsetHours,
                },
            ),
            (
                "D:20261231235959+05'60",
                OutOfRange {
                    field: Field::OffsetMinutes,
                },
            ),
            (
                "D:20261231235959+0560",
                OutOfRange {
                    field: Field::OffsetMinutes,
                },
            ),
            (
                "D:2026123123595924",
                OutOfRange {
                    field: Field::OffsetHours,
                },
            ),
            (
                "D:2026+24",
                OutOfRange {
                    field: Field::OffsetHours,
                },
            ),
            (
                "D:2026'60",
                OutOfRange {
                    field: Field::OffsetMinutes,
                },
            ),
        ] {
            assert_eq!(Date::read(text), Err(error), "{text}");
        }
    }

    #[test]
    fn each_range_holds_at_its_edges() {
        for text in [
            "D:0000",
            "D:9999",
            "D:202601",
            "D:202612",
            "D:20260101",
            "D:20260131",
            "D:2026013100",
            "D:2026013123",
            "D:202601312300",
            "D:202601312359",
            "D:20260131235900",
            "D:20260131235959",
            "D:20260131235959+00'00",
            "D:20260131235959+23'59",
        ] {
            let date = Date::read(text).unwrap();
            assert_eq!(date.quirks(), [], "{text}");
            assert_eq!(date.write(), text);
        }
    }

    #[test]
    fn what_a_reading_reads_past_is_said() {
        use Quirk::*;
        let none = None;
        let rows: &[(&str, Option<Offset>, &[Quirk])] = &[
            // Hours with no sign: their range still holds, and no offset.
            ("D:2026102415300012", none, &[OffsetWithoutSign]),
            ("D:20261024153000 12", none, &[Trailing]),
            ("D:20261024153000123", none, &[OffsetWithoutSign, LoneDigit]),
            ("D:2026102415300012'30", none, &[OffsetWithoutSign]),
            (
                "D:202610241530001230",
                none,
                &[OffsetWithoutSign, MissingApostrophe],
            ),
            // An offset before its fields.
            ("D:2026Z", Some(Offset::Utc), &[OffsetBeforeFields]),
            ("D:202610+05'30", Some(ahead(5, 30)), &[OffsetBeforeFields]),
            ("D:2026102415-08", Some(behind(8, 0)), &[OffsetBeforeFields]),
            // Z and what follows it.
            (
                "D:20261024153000Z05'30",
                Some(Offset::Utc),
                &[UtcWithOffset],
            ),
            (
                "D:20261024153000Z00'30",
                Some(Offset::Utc),
                &[UtcWithOffset],
            ),
            // Apostrophes.
            (
                "D:20261024153000+'30",
                Some(ahead(0, 30)),
                &[ApostropheWithoutHours],
            ),
            ("D:20261024153000'", none, &[ApostropheWithoutHours]),
            ("D:2026'30", none, &[ApostropheWithoutHours]),
            (
                "D:20261024153000+0530",
                Some(ahead(5, 30)),
                &[MissingApostrophe],
            ),
            (
                "D:20261024153000+05'30'",
                Some(ahead(5, 30)),
                &[TerminatingApostrophe],
            ),
            (
                "D:20261024153000Z00'00'",
                Some(Offset::Utc),
                &[TerminatingApostrophe],
            ),
            // After the date.
            ("D:20261", none, &[LoneDigit]),
            ("D:20261024153000+5", Some(ahead(0, 0)), &[LoneDigit]),
            ("D:20261024153000+05'3", Some(ahead(5, 0)), &[LoneDigit]),
            ("D:20261x", none, &[LoneDigit, Trailing]),
            ("D:2026 ", none, &[Trailing]),
            ("D:2026x10", none, &[Trailing]),
            ("D:2026é", none, &[Trailing]),
            ("D:20261024153000+05''", Some(ahead(5, 0)), &[Trailing]),
            ("D:20261024153000+05'30''", Some(ahead(5, 30)), &[Trailing]),
            ("D:20261024153000+05'3012", Some(ahead(5, 30)), &[Trailing]),
            // A day its month hasn't.
            ("D:20260230", none, &[ImpossibleDay]),
            ("D:20250229", none, &[ImpossibleDay]),
            ("D:19000229", none, &[ImpossibleDay]),
            ("D:20260431", none, &[ImpossibleDay]),
            (
                "D:20260431Z",
                Some(Offset::Utc),
                &[ImpossibleDay, OffsetBeforeFields],
            ),
        ];
        for &(text, offset, quirks) in rows {
            let date = Date::read(text).unwrap();
            assert_eq!(date.offset(), offset, "{text}");
            assert_eq!(date.quirks(), quirks, "{text}");
        }
        // Days that are there.
        for text in [
            "D:20240229",
            "D:20000229",
            "D:00000229",
            "D:20260430",
            "D:20261231",
        ] {
            assert_eq!(Date::read(text).unwrap().quirks(), [], "{text}");
        }
    }

    #[test]
    fn an_offset_before_its_fields_fills_them_with_their_defaults() {
        let date = Date::read("D:202610+05'30").unwrap();
        assert_eq!(date.month(), Some(10));
        assert_eq!(date.day(), Some(1));
        assert_eq!(date.hour(), Some(0));
        assert_eq!(date.minute(), Some(0));
        assert_eq!(date.second(), Some(0));
        assert_eq!(date.write(), "D:20261001000000+05'30");
    }

    #[test]
    fn hours_without_a_sign_arent_an_offset_and_arent_written() {
        let date = Date::read("D:2026102415300012'30").unwrap();
        assert_eq!(date.offset(), None);
        assert_eq!(date.write(), "D:20261024153000");
    }

    #[test]
    fn utc_is_written_alone_and_an_offset_in_full() {
        let at = |offset| {
            let date = Date::read("D:20261024153000").unwrap();
            date.with_offset(offset).unwrap().write()
        };
        assert_eq!(at(Offset::Utc), "D:20261024153000Z");
        assert_eq!(at(ahead(0, 0)), "D:20261024153000+00'00");
        assert_eq!(at(ahead(5, 0)), "D:20261024153000+05'00");
        assert_eq!(at(behind(8, 30)), "D:20261024153000-08'30");
    }

    /// A date with a field left out, or an offset, of each form, and each
    /// quirk: written, then read, it gives its fields and no quirks.
    #[test]
    fn a_date_written_reads_back_as_itself() {
        let dates = [
            "D:0000",
            "D:2026",
            "D:202610",
            "D:20261024",
            "D:2026102415",
            "D:202610241530",
            "D:20261024153000",
            "D:20261024153000Z",
            "D:20261024153000+",
            "D:20261024153000-05",
            "D:20261024153000+05'",
            "D:20261024153000+05'30",
            "D:20261024153000-23'59",
            "D:2026102415300012",
            "D:2026Z",
            "D:202610+05'30",
            "D:20261024153000Z05'30",
            "D:20261024153000+'30",
            "D:2026'30",
            "D:20261024153000+0530",
            "D:20261024153000+05'30'",
            "D:20261",
            "D:2026 and so on",
        ];
        for text in dates {
            let date = Date::read(text).unwrap();
            let back = Date::read(&date.write()).unwrap();
            let mut without = date.clone();
            without.quirks.clear();
            assert_eq!(back, without, "{text}");
        }
    }

    /// Every string of up to four pieces after a year, and after a whole
    /// date's fields: whatever a reading read past, what it wrote reads back
    /// as its fields, and writes the same.
    #[test]
    fn what_any_text_reads_as_writes_and_reads_back() {
        const PIECES: [&str; 12] = [
            "01", "12", "13", "30", "60", "7", "Z", "+", "-", "'", " ", "x",
        ];
        let mut read = 0;
        for start in ["D:2026", "D:20260230", "D:20261024153000"] {
            for count in 0..=4u32 {
                for mut pick in 0..PIECES.len().pow(count) {
                    let mut text = start.to_string();
                    for _ in 0..count {
                        text.push_str(PIECES[pick % PIECES.len()]);
                        pick /= PIECES.len();
                    }
                    let Ok(date) = Date::read(&text) else {
                        continue;
                    };
                    read += 1;
                    let written = date.write();
                    let back = Date::read(&written).unwrap();
                    assert_eq!(back.write(), written, "{text}");
                    assert_eq!(
                        (back.year, back.fields, back.offset),
                        (date.year, date.fields, date.offset),
                        "{text}"
                    );
                    let impossible = date.quirks.contains(&Quirk::ImpossibleDay);
                    let said: &[Quirk] = if impossible {
                        &[Quirk::ImpossibleDay]
                    } else {
                        &[]
                    };
                    assert_eq!(back.quirks, said, "{text}");
                }
            }
        }
        assert!(read > 10_000, "{read}");
    }

    #[test]
    fn a_day_its_month_hasnt_stays_and_is_said_again() {
        let date = Date::read("D:20260230").unwrap();
        assert_eq!(date.day(), Some(30));
        let written = date.write();
        assert_eq!(written, "D:20260230");
        assert_eq!(Date::read(&written).unwrap(), date);
    }

    #[test]
    fn a_date_is_built_in_order_and_in_range() {
        use Error::{OutOfOrder, OutOfRange};
        let date = Date::new(2024).unwrap();
        assert_eq!(date.write(), "D:2024");
        assert_eq!(Date::new(0).unwrap().write(), "D:0000");
        assert_eq!(Date::new(9999).unwrap().write(), "D:9999");
        assert_eq!(Date::new(10000), Err(OutOfRange { field: Field::Year }));

        // Each before the one before it.
        let order = |field| Err(OutOfOrder { field });
        assert_eq!(date.clone().with_day(1), order(Field::Day));
        assert_eq!(date.clone().with_hour(0), order(Field::Hour));
        assert_eq!(date.clone().with_minute(0), order(Field::Minute));
        assert_eq!(date.clone().with_second(0), order(Field::Second));
        assert_eq!(date.clone().with_offset(Offset::Utc), order(Field::Offset));

        let month = date.clone().with_month(2).unwrap();
        assert_eq!(month.write(), "D:202402");
        assert_eq!(month.clone().with_hour(0), order(Field::Hour));
        let day = month.clone().with_day(29).unwrap();
        assert_eq!(day.write(), "D:20240229");
        let hour = day.clone().with_hour(23).unwrap();
        let minute = hour.clone().with_minute(59).unwrap();
        assert_eq!(
            minute.clone().with_offset(Offset::Utc),
            order(Field::Offset)
        );
        let second = minute.clone().with_second(59).unwrap();
        assert_eq!(second.write(), "D:20240229235959");
        let full = second.clone().with_offset(behind(23, 59)).unwrap();
        assert_eq!(full.write(), "D:20240229235959-23'59");

        // Each out of range.
        let range = |field| Err(OutOfRange { field });
        assert_eq!(date.clone().with_month(0), range(Field::Month));
        assert_eq!(date.clone().with_month(13), range(Field::Month));
        assert_eq!(month.clone().with_day(0), range(Field::Day));
        assert_eq!(month.clone().with_day(30), range(Field::Day));
        assert_eq!(day.clone().with_hour(24), range(Field::Hour));
        assert_eq!(hour.clone().with_minute(60), range(Field::Minute));
        assert_eq!(minute.clone().with_second(60), range(Field::Second));
        assert_eq!(
            second.clone().with_offset(ahead(24, 0)),
            range(Field::OffsetHours)
        );
        assert_eq!(
            second.clone().with_offset(behind(0, 60)),
            range(Field::OffsetMinutes)
        );

        // A month that hasn't its day; a leap day's year has been said.
        assert_eq!(day.clone().with_month(4).map(|d| d.month()), Ok(Some(4)));
        assert_eq!(
            Date::read("D:20240131").unwrap().with_month(2),
            range(Field::Day)
        );
        assert_eq!(
            Date::read("D:2023")
                .unwrap()
                .with_month(2)
                .unwrap()
                .with_day(29),
            range(Field::Day)
        );
    }

    #[test]
    fn a_date_set_leaves_what_was_read_past_behind() {
        let date = Date::read("D:20260230 and so on").unwrap();
        assert_eq!(date.quirks(), [Quirk::ImpossibleDay, Quirk::Trailing]);
        let date = date.with_month(3).unwrap();
        assert_eq!(date.quirks(), []);
        assert_eq!(date.write(), "D:20260330");

        let date = Date::read("D:20261024153000+05'30'").unwrap();
        let date = date.with_offset(Offset::Utc).unwrap();
        assert_eq!(date.quirks(), []);
        assert_eq!(date.write(), "D:20261024153000Z");

        // A day its month hasn't, kept, is still said, as reading what's
        // written back says it: the date's the same one.
        let kept = Date::read("D:20260230123000 and so on").unwrap();
        for date in [
            kept.clone().with_hour(1).unwrap(),
            kept.clone().with_minute(1).unwrap(),
            kept.clone().with_second(1).unwrap(),
            kept.clone().with_offset(Offset::Utc).unwrap(),
        ] {
            assert_eq!(date.quirks(), [Quirk::ImpossibleDay]);
            assert_eq!(Date::read(&date.write()), Ok(date));
        }
    }

    #[test]
    fn what_an_error_says_is_the_whole_message() {
        assert_eq!(Error::NoPrefix.to_string(), "a date begins D:");
        assert_eq!(
            Error::OutOfRange {
                field: Field::OffsetHours
            }
            .to_string(),
            "a date's offset's hours out of its range"
        );
        assert_eq!(
            Error::OutOfOrder { field: Field::Day }.to_string(),
            "a date's day set before the field before it"
        );
        let error: &dyn std::error::Error = &Error::NoYear;
        assert!(error.source().is_none());
    }

    #[cfg(feature = "clock")]
    #[test]
    fn seconds_read_as_a_date_in_ut() {
        for (seconds, written) in [
            (0, "D:19700101000000Z"),
            (-1, "D:19691231235959Z"),
            (951_782_400, "D:20000229000000Z"),
            (1_792_855_800, "D:20261024153000Z"),
            (253_402_300_799, "D:99991231235959Z"),
            (-62_167_219_200, "D:00000101000000Z"),
        ] {
            let date = Date::from_unix(seconds).unwrap();
            assert_eq!(date.write(), written, "{seconds}");
            assert_eq!(Date::read(written).unwrap(), date, "{seconds}");
        }
        for seconds in [253_402_300_800, -62_167_219_201, i64::MAX, i64::MIN] {
            assert_eq!(Date::from_unix(seconds), None, "{seconds}");
        }
    }

    #[cfg(feature = "clock")]
    #[test]
    fn the_clock_gives_a_date_with_every_field() {
        let now = Date::now();
        assert!(now.year() >= 2026);
        assert_eq!(now.offset(), Some(Offset::Utc));
        let written = now.write();
        assert_eq!(written.len(), "D:YYYYMMDDHHmmSSZ".len());
        assert_eq!(Date::read(&written).unwrap(), now);
    }
}
