//! A file's cross-reference sections each on its own, as written: what a checker
//! wants and the reader doesn't keep, as a reading merges them into one list of
//! entries. Read again from the source when asked ([`Reading::sections`]), and
//! kept for nobody, so that a reading that isn't asked pays nothing for it. So is
//! the file's first object ([`Reading::first_object`]), which a reading opens only
//! to ask of it what [`Facts::linearized`](crate::source::Facts::linearized) says.

use pdf_filters::Fault;
use pdf_objects::object::{Object, ObjectId, Value};
use pdf_tokenizer::{Kind, Tokenizer, is_white_space};

use super::{Cx, Reader, Reading, Unwritten, XrefEntry, offset_of, predictor_row, rfind};
use crate::crypt::is_xref;
use crate::read::reading::{ByteSource, Error};
use crate::read::window::{Bytes, InMemory, Window};
use crate::source::{Trailer, XrefKind};

/// One row of a cross-reference section, as written.
/// Not exhaustive, so that it can come to say more of a row; made only by the
/// reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SectionRow {
    /// The number it's for.
    pub number: u32,
    /// Its generation: a free row's next generation, 0 for a row in an object
    /// stream, and `u16::MAX` where what's written doesn't fit.
    pub generation: u16,
    /// What it says, or none for a stream's row of a type no reader knows, which
    /// reads as null (ISO 32000-2, 7.5.8.3). A table's row with an offset or a
    /// generation out of range reads as free, as the reader reads it.
    pub entry: Option<XrefEntry>,
}

/// One cross-reference section of a file, as written, none of it merged with
/// another's.
/// Not exhaustive, so that it can come to say more of a section; made only by
/// the reader.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SectionRows {
    /// Where it starts: its `xref`, or its stream's `n g obj`.
    pub at: usize,
    /// How it's written.
    pub kind: XrefKind,
    /// Its subsections, each's first number and how many rows it says it has: a
    /// table's headers, or a stream's `/Index` or what stands for it.
    pub subsections: Vec<(u32, u32)>,
    /// Its rows, in the order they're written.
    pub rows: Vec<SectionRow>,
    /// Where what it lists first is: a table's first row, or its stream's `n g obj`,
    /// after where the white-space before that starts. None for a table with no rows.
    pub first_row: Option<(usize, usize)>,
    /// Its trailer: the dictionary after `trailer`, or the stream's.
    pub trailer: Trailer,
    /// Its `/Prev`, as the reader reads it: none where it isn't an offset.
    pub prev: Option<usize>,
    /// The number and generation of the stream it's written as.
    pub stream: Option<ObjectId>,
    /// What a hybrid table's `/XRefStm` lists (7.5.8.4), as a section of its own.
    pub hybrid: Option<Box<SectionRows>>,
    /// The offset the `startxref` after it gives, where one follows it.
    pub startxref: Option<usize>,
}

/// A file's sections along `/Prev`, the newest first: as many as read, and what
/// stopped it short of the last, if anything did.
/// Not exhaustive, so that it can come to say more of the sections; made only
/// by the reader.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Sections {
    /// The sections, in the order `/Prev` gives them.
    pub sections: Vec<SectionRows>,
    /// Why there are no more than that, where the chain doesn't end by itself.
    pub stopped: Option<SectionFault>,
}

/// Why no more of a file's sections are read ([`Reading::sections`]): the
/// chain's own reasons, each with the byte it was found at where there's one,
/// or a section that doesn't read ([`SectionFault::Section`]), why in
/// [`Unwritten`]'s words, as a reading's trailer and its faults say it. So one
/// match over [`Unwritten`] reads a section's reason wherever it's given.
/// Not exhaustive, as reasons are found as the reader learns.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SectionFault {
    /// The file has no `startxref`.
    #[error("no startxref")]
    NoStartxref,
    /// `startxref` is part of a longer word.
    #[error("startxref at byte {at} is inside another word")]
    StartxrefInWord {
        /// Where.
        at: usize,
    },
    /// What follows `startxref` isn't an offset.
    #[error("startxref at byte {at} gives no offset")]
    StartxrefNoOffset {
        /// Where.
        at: usize,
    },
    /// `/Prev` comes back to a section read before.
    #[error("the /Prev chain comes back to byte {at}")]
    Loop {
        /// Where it comes back to.
        at: usize,
    },
    /// More sections than [`Limits::sections`](super::Limits::sections).
    #[error("more than {limit} cross-reference sections")]
    TooManySections {
        /// The cap.
        limit: usize,
    },
    /// More rows than the reader reads ([`Limits::entries`](super::Limits::entries)).
    #[error("more than {limit} cross-reference rows")]
    TooManyRows {
        /// The cap.
        limit: usize,
    },
    /// The section at byte `at`, which `startxref`, a `/Prev` or a
    /// `/XRefStm` gives, doesn't read, and why: as the trailer a reading
    /// makes instead says it ([`Trailer::unwritten`](crate::Trailer::unwritten)),
    /// and a fault on a later section ([`Fault::Section`](super::Fault::Section)).
    #[error("the section at byte {at} doesn't read: {why}")]
    Section {
        /// Where it is.
        at: usize,
        /// Why it doesn't read.
        why: Box<Unwritten>,
    },
}

impl SectionFault {
    /// The section at `at`, which doesn't read for `why`.
    fn section(at: usize, why: Unwritten) -> SectionFault {
        let why = Box::new(why);
        SectionFault::Section { at, why }
    }
}

/// The first object in a file, after its header, as written.
/// Not exhaustive, so that it can come to say more of the object; made only by
/// the reader.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FirstObject {
    /// Its number and generation.
    pub id: ObjectId,
    /// Where its `n g obj` starts.
    pub at: usize,
    /// Where it ends: past its `endobj`, or where it ends without one.
    pub end: usize,
    /// Its value, a stream's as its dictionary and data's place, nothing decrypted.
    pub object: Object,
}

impl<S: ByteSource> Reading<S> {
    /// The file's cross-reference sections from the last `startxref` along their
    /// `/Prev` chain (or from [`Options::section`](super::Options::section), as it
    /// was opened), each on its own, as written, which a reading doesn't keep: read
    /// again from the source each time it's asked, under the reading's
    /// [`Limits`](super::Limits), and kept for nobody. What doesn't read ends the
    /// list, and [`Sections::stopped`] says why; a file with no section that reads
    /// has none, though its reading found its objects by scanning. So what's here
    /// can differ from [`Reading::entries`], which is what a reader made of them.
    /// Positions are from the file's first byte, as a reading reads them. A stream's
    /// rows are decoded from its own data, and that's never encrypted (ISO 32000-2,
    /// 7.5.8.2). Its data is found as the reading found it, so the rows are the
    /// reading's whatever it has read: a `/Length` that's a reference isn't followed,
    /// as a cross-reference stream's entries are direct (7.5.8.2), and the open reads
    /// them before it lists any object.
    ///
    /// # Errors
    ///
    /// It fails only when the source doesn't give its bytes ([`Error::Io`]).
    pub fn sections(&self) -> Result<Sections, Error> {
        match self.source().whole() {
            Some(file) => {
                let Ok(sections) = self.reader.sections_again(InMemory(file));
                Ok(sections)
            }
            None => (self.reader.sections_again(self.by_position())).map_err(Error::Io),
        }
    }

    /// The file's first object, from its header on, as written: none with no header,
    /// or where what's first doesn't read as an object. It isn't kept. Its strings
    /// aren't decrypted, a linearization dictionary having none that a reader wants.
    /// A stream's `/Length` in an object stream isn't read from it, as the open reads
    /// none: such a stream's data is found by `endstream`, whatever was read before.
    ///
    /// # Errors
    ///
    /// It fails only when the source doesn't give its bytes ([`Error::Io`]).
    pub fn first_object(&self) -> Result<Option<FirstObject>, Error> {
        match self.source().whole() {
            Some(file) => {
                let Ok(first) = self.reader.first_object(InMemory(file));
                Ok(first)
            }
            None => (self.reader.first_object(self.by_position())).map_err(Error::Io),
        }
    }
}

/// How far back the white-space before a section's stream is looked for.
const SPACE: usize = 64;

/// The value `field`'s bytes hold, high-order first.
fn field(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |n, &b| (n << 8) | u64::from(b))
}

impl Reader {
    /// [`Reading::first_object`], of `file`.
    fn first_object<'f, F: Bytes<'f>>(&self, file: F) -> Result<Option<FirstObject>, F::Error> {
        let Some(header) = self.facts.header_offset else {
            return Ok(None);
        };
        let mut cx = Cx::opening(false);
        Ok(self
            .indirect(file, &mut cx, header, None)?
            .ok()
            .map(|(id, object, record)| FirstObject {
                id,
                at: record.at.start,
                end: record.at.end,
                object,
            }))
    }

    /// [`Reading::sections`], of `file`.
    fn sections_again<'f, F: Bytes<'f>>(&self, file: F) -> Result<Sections, F::Error> {
        let mut sections = Vec::new();
        let start = match self.as_of {
            Some(at) => Ok(at),
            None => last_startxref(file)?,
        };
        let mut next = match start {
            Ok(at) => Some(at),
            Err(fault) => {
                return Ok(Sections {
                    sections,
                    stopped: Some(fault),
                });
            }
        };
        let mut seen = Vec::new();
        let mut left = self.listable;
        let mut stopped = None;
        while let Some(at) = next {
            if seen.contains(&at) {
                stopped = Some(SectionFault::Loop { at });
                break;
            }
            if seen.len() >= self.limits.sections {
                let limit = self.limits.sections;
                stopped = Some(SectionFault::TooManySections { limit });
                break;
            }
            seen.push(at);
            match self.section_again(file, at, &mut left)? {
                Ok(section) => {
                    next = section.prev;
                    sections.push(section);
                }
                Err(fault) => {
                    stopped = Some(fault);
                    break;
                }
            }
        }
        Ok(Sections { sections, stopped })
    }

    /// The section at `at`, its rows taken from the `left` that may still be read.
    fn section_again<'f, F: Bytes<'f>>(
        &self,
        file: F,
        at: usize,
        left: &mut usize,
    ) -> Result<Result<SectionRows, SectionFault>, F::Error> {
        let mut cx = Cx::opening(false);
        let what = file.read_from(&mut cx, at, 0, |w, cx| {
            let mut t = w.tokenizer(at);
            Ok(match w.significant(&mut t) {
                Some(token) if token.is_keyword(b"xref") => {
                    let left = *left;
                    Some(self.table_again(w, cx, &mut t, at, left))
                }
                Some(token) if token.kind == Kind::Integer => None,
                _ => Some(Err(SectionFault::section(at, Unwritten::Neither { at }))),
            })
        })?;
        let mut section = match what {
            None => return self.stream_again(file, at, left),
            Some(Err(fault)) => return Ok(Err(fault)),
            Some(Ok(section)) => section,
        };
        *left -= section.rows.len();
        let stream = section.trailer.dict().get(b"XRefStm").and_then(offset_of);
        if let Some(stream) = stream {
            match self.stream_again(file, stream, left)? {
                Ok(rows) => section.hybrid = Some(Box::new(rows)),
                Err(fault) => return Ok(Err(fault)),
            }
        }
        Ok(Ok(section))
    }

    /// A table's rows, after its `xref`, as they're written, and its trailer.
    fn table_again<'w>(
        &self,
        w: &Window<'w>,
        cx: &mut Cx,
        t: &mut Tokenizer<'w>,
        section: usize,
        left: usize,
    ) -> Result<SectionRows, SectionFault> {
        let integer = |t: &mut Tokenizer<'w>| {
            (w.significant(t))
                .filter(|token| token.kind == Kind::Integer)
                .and_then(|token| token.integer().ok())
        };
        let mut subsections = Vec::new();
        let mut rows = Vec::new();
        let mut first_row = None;
        loop {
            let at = w.place(t.offset());
            match w.significant(&mut t.clone()) {
                Some(token) if token.is_keyword(b"trailer") => {
                    w.significant(t);
                    break;
                }
                Some(token) if token.kind == Kind::Integer => {
                    let start = integer(t).and_then(|n| u32::try_from(n).ok());
                    let count = integer(t).and_then(|n| u32::try_from(n).ok());
                    let (Some(start), Some(count)) = (start, count) else {
                        return Err(SectionFault::section(section, Unwritten::Subsection { at }));
                    };
                    subsections.push((start, count));
                    let header_end = w.place(t.offset());
                    for i in 0..count {
                        let row =
                            (w.significant(&mut t.clone())).map_or(t.offset(), |token| token.start);
                        let row = w.place(row);
                        if first_row.is_none() {
                            first_row = Some((header_end, row));
                        }
                        if rows.len() >= left {
                            return Err(SectionFault::TooManyRows {
                                limit: self.listable,
                            });
                        }
                        let row_fault = |why| SectionFault::section(section, why);
                        let Some(number) = start.checked_add(i) else {
                            return Err(row_fault(Unwritten::RowNumber { at: row }));
                        };
                        let offset = integer(t);
                        let generation = integer(t);
                        let kind = w.significant(t);
                        let (Some(offset), Some(generation), Some(kind)) =
                            (offset, generation, kind.as_ref().map(|k| k.bytes))
                        else {
                            return Err(row_fault(Unwritten::Row { at: row }));
                        };
                        let fits = usize::try_from(offset)
                            .ok()
                            .zip(u16::try_from(generation).ok());
                        let row_read = match (fits, kind) {
                            (Some((offset, generation)), b"n") => SectionRow {
                                number,
                                generation,
                                entry: Some(XrefEntry::InUse { offset, generation }),
                            },
                            (_, b"n" | b"f") => SectionRow {
                                number,
                                generation: u16::try_from(generation)
                                    .ok()
                                    .filter(|_| kind == b"f")
                                    .unwrap_or(u16::MAX),
                                entry: Some(XrefEntry::Free),
                            },
                            _ => return Err(row_fault(Unwritten::Row { at: row })),
                        };
                        rows.push(row_read);
                    }
                }
                _ => return Err(SectionFault::section(section, Unwritten::NoTrailer { at })),
            }
        }
        let at = w.place(t.offset());
        let mut parser = w.parser(at).limits(self.limits.parser);
        let trailer = w.parsed(parser.value(&mut cx.parsed(None)));
        let fault = |why| SectionFault::section(section, why);
        let trailer = trailer.map_err(|why| fault(Unwritten::Trailer(why)))?;
        let Some(dict) = trailer.as_dict() else {
            return Err(fault(Unwritten::TrailerNotADictionary));
        };
        let prev = dict.get(b"Prev").and_then(offset_of);
        let startxref = startxref_at(w, parser.offset());
        Ok(SectionRows {
            at: section,
            kind: XrefKind::Table,
            subsections,
            rows,
            first_row,
            trailer: Trailer::new(trailer),
            prev,
            stream: None,
            hybrid: None,
            startxref,
        })
    }

    /// The cross-reference stream at `at`, as a section.
    fn stream_again<'f, F: Bytes<'f>>(
        &self,
        file: F,
        at: usize,
        left: &mut usize,
    ) -> Result<Result<SectionRows, SectionFault>, F::Error> {
        let mut cx = Cx::opening(false);
        let (id, object, record) = match self.indirect(file, &mut cx, at, None)? {
            Ok(read) => read,
            Err(why) => return Ok(Err(SectionFault::section(at, Unwritten::Unread(why)))),
        };
        let fault = |why| Ok(Err(SectionFault::section(at, why)));
        let Value::Stream(stream) = object.value() else {
            return fault(Unwritten::NotAStream { id, at });
        };
        if !is_xref(&stream.dict) {
            return fault(Unwritten::NotXref { id, at });
        }
        let written = stream.data.at().map(|at| file.get(at.start..at.end));
        let written = written.transpose()?.flatten().unwrap_or_default();
        let end = record.at.end;
        let startxref = file.read_from(&mut cx, end, 0, |w, _| Ok(startxref_at(w, end)))?;
        // As many bytes before it as a writer has ever been seen to put there.
        let before = file.get(at.saturating_sub(SPACE)..at)?.unwrap_or_default();
        let space = (before.iter().rev())
            .take_while(|&&b| is_white_space(b))
            .count();
        let rows = self.stream_rows(id, &object, &written, at, *left);
        let (subsections, rows) = match rows {
            Ok(rows) => rows,
            Err(fault) => return Ok(Err(fault)),
        };
        *left -= rows.len();
        let dict = object.as_dict().expect("it's a stream");
        let prev = dict.get(b"Prev").and_then(offset_of);
        let trailer = Trailer {
            stream: Some(id),
            object: object.clone(),
            unwritten: None,
        };
        Ok(Ok(SectionRows {
            at,
            kind: XrefKind::Stream,
            subsections,
            rows,
            first_row: Some((at - space, at)),
            trailer,
            prev,
            stream: Some(id),
            hybrid: None,
            startxref,
        }))
    }

    /// A stream's subsections and rows, by its `/W` and `/Index` (7.5.8.2), from its
    /// own data as `written`: stream `id`, at `at`.
    #[allow(clippy::type_complexity)]
    fn stream_rows(
        &self,
        id: ObjectId,
        object: &Object,
        written: &[u8],
        at: usize,
        left: usize,
    ) -> Result<(Vec<(u32, u32)>, Vec<SectionRow>), SectionFault> {
        let fault = |why| SectionFault::section(at, why);
        let Value::Stream(stream) = object.value() else {
            return Err(fault(Unwritten::Streamless { id }));
        };
        let dict = &stream.dict;
        let widths: Option<Vec<usize>> = match dict.get(b"W").map(Object::value) {
            Some(Value::Array(w)) if w.len() == 3 => (w.iter())
                .map(|n| {
                    n.as_integer()
                        .and_then(|n| usize::try_from(n).ok())
                        .filter(|&n| n <= 8)
                })
                .collect(),
            _ => None,
        };
        let widths = widths.ok_or(fault(Unwritten::Widths))?;
        let size = dict.get(b"Size").and_then(Object::as_integer);
        let subsections: Vec<(u32, u32)> = match dict.get(b"Index").map(Object::value) {
            Some(Value::Array(index)) => (index.chunks(2))
                .map(|pair| match pair {
                    [start, count] => Some((
                        u32::try_from(start.as_integer()?).ok()?,
                        u32::try_from(count.as_integer()?).ok()?,
                    )),
                    _ => None,
                })
                .collect::<Option<_>>()
                .ok_or(fault(Unwritten::Index))?,
            _ => {
                let size = size.and_then(|s| u32::try_from(s).ok());
                vec![(0, size.ok_or(fault(Unwritten::NoSize))?)]
            }
        };
        let row = widths.iter().sum::<usize>();
        let count = (subsections.iter()).try_fold(0usize, |all, &(_, count)| {
            all.checked_add(usize::try_from(count).ok()?)
        });
        let size = count.and_then(|count| count.checked_mul(row));
        let filters = super::filters(dict, None).map_err(|why| fault(Unwritten::Filters(why)))?;
        let decoded;
        let data = match filters.is_empty() {
            true => written,
            false => {
                let limit = size.map_or(self.limits.stream, |size| {
                    size.saturating_add(predictor_row(&filters))
                        .min(self.limits.stream)
                });
                decoded = match pdf_filters::decode(written, &filters, limit, self.flate) {
                    Ok(data) => data,
                    // What's past its rows, and a cut there, aren't its rows' fault.
                    Err(error) if size.is_some_and(|size| error.data.len() >= size) => error.data,
                    Err(error) => {
                        let first = (error.faults.first()).map_or(Fault::Limit, |&(_, f)| f);
                        return Err(fault(Unwritten::Decode { fault: first }));
                    }
                };
                &decoded[..]
            }
        };
        let mut rows_read = data.chunks_exact(row.max(1));
        let mut rows = Vec::new();
        for &(start, count) in &subsections {
            for i in 0..count {
                if rows.len() >= left {
                    return Err(SectionFault::TooManyRows {
                        limit: self.listable,
                    });
                }
                let bytes = rows_read.next().ok_or(fault(Unwritten::DataShort))?;
                let (one, rest) = bytes.split_at(widths[0]);
                let (two, three) = rest.split_at(widths[1]);
                let kind = if widths[0] == 0 { 1 } else { field(one) };
                let (two, three) = (field(two), field(three));
                let number = start.checked_add(i).ok_or(fault(Unwritten::IndexNumber))?;
                let generation = u16::try_from(three).unwrap_or(u16::MAX);
                let (generation, entry) = match kind {
                    0 => (generation, Some(XrefEntry::Free)),
                    1 => {
                        let offset = usize::try_from(two).unwrap_or(usize::MAX);
                        let entry = XrefEntry::InUse { offset, generation };
                        (generation, Some(entry))
                    }
                    2 => {
                        let stream = u32::try_from(two).unwrap_or(u32::MAX);
                        let index = u32::try_from(three).unwrap_or(u32::MAX);
                        (0, Some(XrefEntry::Compressed { stream, index }))
                    }
                    _ => (0, None),
                };
                rows.push(SectionRow {
                    number,
                    generation,
                    entry,
                });
            }
        }
        Ok((subsections, rows))
    }
}

/// Where the last `startxref` says the newest section is, read back from the
/// file's end.
fn last_startxref<'f, F: Bytes<'f>>(file: F) -> Result<Result<usize, SectionFault>, F::Error> {
    file.read_back(|w| {
        let Some(i) = rfind(w.bytes, b"startxref") else {
            w.touch();
            return Err(SectionFault::NoStartxref);
        };
        let at = w.place(i);
        let mut t = Tokenizer::at(w.bytes, i);
        if !t.next().is_some_and(|token| token.is_keyword(b"startxref")) {
            return Err(SectionFault::StartxrefInWord { at });
        }
        t.next_significant()
            .filter(|token| token.kind == Kind::Integer)
            .and_then(|token| usize::try_from(token.integer().ok()?).ok())
            .ok_or(SectionFault::StartxrefNoOffset { at })
    })
}

/// The offset a `startxref` at `at` gives, if one's there.
fn startxref_at(w: &Window<'_>, at: usize) -> Option<usize> {
    let mut t = w.tokenizer(at);
    let keyword = w.significant(&mut t);
    if !keyword.is_some_and(|token| token.is_keyword(b"startxref")) {
        return None;
    }
    (w.significant(&mut t))
        .filter(|token| token.kind == Kind::Integer)
        .and_then(|token| usize::try_from(token.integer().ok()?).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::{Limits, Options, Unfiltered};

    /// A file of a catalog, its table at the end as `table` writes it, its trailer
    /// `trailer`, and its `startxref` at the table unless the file gives its own.
    fn table_file(table: &str, trailer: &str) -> Vec<u8> {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        let at = file.len();
        file.extend_from_slice(table.as_bytes());
        file.extend_from_slice(trailer.as_bytes());
        file.extend_from_slice(format!("startxref\n{at}\n%%EOF\n").as_bytes());
        file
    }

    const ROWS: &str = "xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n";
    const TRAILER: &str = "trailer\n<< /Size 2 /Root 1 0 R >>\n";

    /// A file of a catalog, a cross-reference stream at the end whose dictionary
    /// is `dict`, unfiltered unless it says, with `data`, by `startxref`.
    fn stream_file(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        let at = file.len();
        let head = format!("2 0 obj\n<< {dict} /Length {} >>\nstream\n", data.len());
        file.extend_from_slice(head.as_bytes());
        file.extend_from_slice(data);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        file.extend_from_slice(format!("startxref\n{at}\n%%EOF\n").as_bytes());
        file
    }

    const XREF: &str = "/Type /XRef /Size 3 /W [1 2 1] /Root 1 0 R";

    /// The three rows of [`stream_file`]'s `data` that say what it is, the stream
    /// at `at`.
    fn three_rows(at: u16) -> Vec<u8> {
        let [high, low] = at.to_be_bytes();
        vec![0, 0, 0, 255, 1, 0, 9, 0, 1, high, low, 0]
    }

    fn sections(file: &[u8]) -> Sections {
        Reading::open(file, &Options::DEFAULT)
            .unwrap()
            .sections()
            .unwrap()
    }

    fn stopped(file: &[u8]) -> SectionFault {
        sections(file).stopped.expect("it stops")
    }

    /// That the sections of `file` stop at the section at `at`, which
    /// doesn't read for `why`, in `words`, as a fault on a section says it;
    /// and, where the reading reads it `alike`, that the trailer it made
    /// instead gives the same reason.
    fn section_stops(file: &[u8], at: usize, why: Unwritten, words: &str, alike: bool) {
        let fault = stopped(file);
        let want = SectionFault::Section {
            at,
            why: Box::new(why.clone()),
        };
        assert_eq!(fault, want);
        let said = format!("the section at byte {at} doesn't read: {words}");
        assert_eq!(fault.to_string(), said);
        let on_a_section = super::super::Fault::Section {
            at,
            why: Box::new(why.clone()),
        };
        assert_eq!(on_a_section.to_string(), said);
        let reading = Reading::open(file, &Options::DEFAULT).unwrap();
        let unwritten = reading.trailer().unwritten.clone();
        assert_eq!(unwritten == Some(why), alike, "{unwritten:?}");
    }

    #[test]
    fn a_tables_rows_are_as_written() {
        let table =
            "xref\n3 1\n0000000009 00002 n \n0 2\n0000000000 65535 f \n0000000009 00000 n \n";
        let file = table_file(table, "trailer\n<< /Size 4 /Prev 2 /Root 1 0 R >>\n");
        let read = sections(&file);
        // Its /Prev goes nowhere: what's there is said, with the section read.
        let why = Box::new(Unwritten::Neither { at: 2 });
        assert_eq!(read.stopped, Some(SectionFault::Section { at: 2, why }));
        let [section] = &read.sections[..] else {
            panic!("one section");
        };
        assert_eq!(section.kind, XrefKind::Table);
        assert_eq!(section.at, 45);
        assert_eq!(section.subsections, [(3, 1), (0, 2)]);
        let numbers: Vec<u32> = section.rows.iter().map(|row| row.number).collect();
        assert_eq!(numbers, [3, 0, 1], "as they're written");
        assert_eq!(section.rows[0].generation, 2);
        assert_eq!(
            section.rows[0].entry,
            Some(XrefEntry::InUse {
                offset: 9,
                generation: 2
            })
        );
        assert_eq!(section.rows[1].entry, Some(XrefEntry::Free));
        assert_eq!(section.rows[1].generation, 65535);
        // A free row's generation is its own, and an in-use row's past its width.
        assert_eq!(section.prev, Some(2));
        assert_eq!(section.stream, None);
        // The white-space after "3 1", and its first row.
        assert_eq!(section.first_row, Some((45 + 8, 45 + 9)));
    }

    #[test]
    fn a_free_rows_generation_is_as_written() {
        let table = "xref\n0 1\n0000000000 00007 f \n";
        let read = sections(&table_file(table, TRAILER));
        assert_eq!(read.sections[0].rows[0].generation, 7);
    }

    #[test]
    fn an_in_use_row_at_no_place_reads_as_free_of_no_generation() {
        let table = "xref\n0 1\n-000000001 00003 n \n";
        let read = sections(&table_file(table, TRAILER));
        assert_eq!(read.sections[0].rows[0].entry, Some(XrefEntry::Free));
        assert_eq!(read.sections[0].rows[0].generation, u16::MAX);
    }

    #[test]
    fn a_tables_row_out_of_range_reads_as_free() {
        let table = "xref\n0 1\n0000000009 70000 n \n";
        let read = sections(&table_file(table, TRAILER));
        assert_eq!(read.sections[0].rows[0].entry, Some(XrefEntry::Free));
        assert_eq!(read.sections[0].rows[0].generation, u16::MAX);
    }

    #[test]
    fn a_streams_rows_are_as_written_and_it_says_its_stream() {
        let file = stream_file(XREF, &three_rows(45));
        let read = sections(&file);
        assert_eq!(read.stopped, None);
        let [section] = &read.sections[..] else {
            panic!("one section");
        };
        assert_eq!(section.kind, XrefKind::Stream);
        assert_eq!(section.at, 45);
        assert_eq!(section.stream, Some(ObjectId::new(2, 0)));
        assert_eq!(section.subsections, [(0, 3)]);
        assert_eq!(section.first_row, Some((44, 45)));
        assert_eq!(section.startxref, Some(45));
        assert_eq!(
            section.rows[2].entry,
            Some(XrefEntry::InUse {
                offset: 45,
                generation: 0
            })
        );
        assert!(section.trailer.stream.is_some());
    }

    #[test]
    fn a_streams_row_of_no_known_type_is_said() {
        let mut data = three_rows(45);
        data[0] = 7;
        let read = sections(&stream_file(XREF, &data));
        assert_eq!(read.sections[0].rows[0].entry, None);
    }

    #[test]
    fn a_compressed_row_is_its_stream_and_place() {
        let mut data = three_rows(45);
        data[4..8].copy_from_slice(&[2, 0, 9, 3]);
        let read = sections(&stream_file(XREF, &data));
        assert_eq!(
            read.sections[0].rows[1].entry,
            Some(XrefEntry::Compressed {
                stream: 9,
                index: 3
            })
        );
    }

    #[test]
    fn a_filtered_stream_is_decoded_as_far_as_its_rows() {
        let hex: String = (three_rows(45).iter())
            .map(|byte| format!("{byte:02X}"))
            .collect();
        let dict = format!("{XREF} /Filter /ASCIIHexDecode");
        let read = sections(&stream_file(&dict, format!("{hex}>").as_bytes()));
        assert_eq!(read.stopped, None);
        assert_eq!(read.sections[0].rows.len(), 3);
    }

    #[test]
    fn a_startxref_after_a_section_is_read() {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let at = file.len();
        file.extend_from_slice(ROWS.as_bytes());
        file.extend_from_slice(TRAILER.as_bytes());
        file.extend_from_slice(b"startxref\n0\n%%EOF\n");
        file.extend_from_slice(format!("startxref\n{at}\n%%EOF\n").as_bytes());
        assert_eq!(sections(&file).sections[0].startxref, Some(0));
    }

    #[test]
    fn a_chain_is_read_newest_first_each_on_its_own() {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let first = file.len();
        file.extend_from_slice(ROWS.as_bytes());
        file.extend_from_slice(TRAILER.as_bytes());
        let second = file.len();
        file.extend_from_slice(b"xref\n5 1\n0000000009 00000 n \n");
        let trailer = format!("trailer\n<< /Size 6 /Prev {first} >>\n");
        file.extend_from_slice(trailer.as_bytes());
        file.extend_from_slice(format!("startxref\n{second}\n%%EOF\n").as_bytes());
        let read = sections(&file);
        let ats: Vec<usize> = read.sections.iter().map(|s| s.at).collect();
        assert_eq!(ats, [second, first]);
        assert_eq!(read.sections[0].rows.len(), 1);
        assert_eq!(read.sections[1].rows.len(), 2);
    }

    #[test]
    fn a_hybrid_tables_stream_is_a_section_of_its_own_inside_it() {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let stream = file.len();
        let data = three_rows(stream as u16);
        let head = format!("2 0 obj\n<< {XREF} /Length {} >>\nstream\n", data.len());
        file.extend_from_slice(head.as_bytes());
        file.extend_from_slice(&data);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let table = file.len();
        file.extend_from_slice(ROWS.as_bytes());
        let trailer = format!("trailer\n<< /Size 3 /Root 1 0 R /XRefStm {stream} >>\n");
        file.extend_from_slice(trailer.as_bytes());
        file.extend_from_slice(format!("startxref\n{table}\n%%EOF\n").as_bytes());
        let read = sections(&file);
        assert_eq!(read.stopped, None);
        let hybrid = read.sections[0].hybrid.as_ref().expect("it has a stream");
        assert_eq!(hybrid.at, stream);
        assert_eq!(hybrid.rows.len(), 3);
    }

    #[test]
    fn a_reading_as_of_a_section_starts_there() {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let first = file.len();
        file.extend_from_slice(ROWS.as_bytes());
        file.extend_from_slice(TRAILER.as_bytes());
        file.extend_from_slice(format!("startxref\n{first}\n%%EOF\n").as_bytes());
        let last = file.len();
        file.extend_from_slice(ROWS.as_bytes());
        let trailer = format!("trailer\n<< /Size 2 /Prev {first} /Root 1 0 R >>\n");
        file.extend_from_slice(trailer.as_bytes());
        file.extend_from_slice(format!("startxref\n{last}\n%%EOF\n").as_bytes());
        let mut options = Options::DEFAULT;
        options.section = Some(first);
        let reading = Reading::open(&file[..], &options).unwrap();
        let ats: Vec<usize> = (reading.sections().unwrap().sections.iter())
            .map(|s| s.at)
            .collect();
        assert_eq!(ats, [first]);
    }

    #[test]
    fn it_reads_the_same_by_position() {
        let file = stream_file(XREF, &three_rows(45));
        let by_position = crate::fuzz::Counted::new(&file, 8);
        let reading = Reading::open(&by_position, &Options::DEFAULT).unwrap();
        assert_eq!(reading.sections().unwrap(), sections(&file));
        let in_memory = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        assert_eq!(
            reading.first_object().unwrap(),
            in_memory.first_object().unwrap()
        );
    }

    // One test per way a section doesn't read.

    #[test]
    fn no_startxref_is_said() {
        let file = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n";
        assert_eq!(stopped(file), SectionFault::NoStartxref);
    }

    #[test]
    fn startxref_inside_a_word_is_said() {
        let file = b"%PDF-1.7\nstartxrefs\n10\n";
        assert_eq!(stopped(file), SectionFault::StartxrefInWord { at: 9 });
    }

    #[test]
    fn startxref_with_no_offset_is_said() {
        let file = b"%PDF-1.7\nstartxref\nnone\n";
        assert_eq!(stopped(file), SectionFault::StartxrefNoOffset { at: 9 });
    }

    // A section that doesn't read is said as a fault on a later section
    // says it, its reason an `Unwritten`, which the trailer the reading
    // made instead gives too, but where only the sections one by one say it.

    #[test]
    fn neither_xref_nor_an_object_is_said() {
        let file = b"%PDF-1.7\nstartxref\n0\n%%EOF\n";
        let words = "byte 0, which startxref or /Prev gives, is at neither `xref` nor an object";
        section_stops(file, 0, Unwritten::Neither { at: 0 }, words, true);
    }

    #[test]
    fn a_subsection_header_that_doesnt_read_is_said() {
        let file = table_file("xref\n0 x\n", TRAILER);
        let words = "a subsection's header at byte 49 doesn't read";
        section_stops(&file, 45, Unwritten::Subsection { at: 49 }, words, true);
    }

    #[test]
    fn a_row_that_doesnt_read_is_said() {
        let file = table_file("xref\n0 2\n0000000000 65535 q \n", TRAILER);
        let words = "the table's row at byte 54 doesn't read";
        section_stops(&file, 45, Unwritten::Row { at: 54 }, words, true);
    }

    #[test]
    fn a_row_past_the_numbers_is_said() {
        let file = table_file(
            "xref\n4294967295 2\n0000000000 65535 f \n0000000000 65535 f \n",
            TRAILER,
        );
        let words = "the table's row at byte 83 numbers no object";
        section_stops(&file, 45, Unwritten::RowNumber { at: 83 }, words, true);
    }

    #[test]
    fn a_table_with_no_trailer_is_said() {
        let file = table_file(ROWS, "");
        // Past the last row's `n`, where `trailer` should be.
        let at = 45 + ROWS.len() - " \n".len();
        let words = format!("the table at byte {at} has no trailer");
        section_stops(&file, 45, Unwritten::NoTrailer { at }, &words, true);
    }

    #[test]
    fn a_trailer_that_doesnt_read_is_said() {
        let file = table_file(ROWS, "trailer\n<< /Size");
        let SectionFault::Section { at: 45, why } = stopped(&file) else {
            panic!("{:?}", stopped(&file));
        };
        let Unwritten::Trailer(error) = *why else {
            panic!("{why:?}");
        };
        let words = error.to_string();
        section_stops(&file, 45, Unwritten::Trailer(error), &words, true);
    }

    #[test]
    fn a_trailer_that_isnt_a_dictionary_is_said() {
        let file = table_file(ROWS, "trailer\n12\n");
        let words = "the trailer isn't a dictionary";
        section_stops(&file, 45, Unwritten::TrailerNotADictionary, words, true);
    }

    #[test]
    fn an_object_that_doesnt_read_is_said() {
        let mut file = b"%PDF-1.7\n".to_vec();
        file.extend_from_slice(b"7 oops\nstartxref\n9\n%%EOF\n");
        let SectionFault::Section { at: 9, why } = stopped(&file) else {
            panic!("{:?}", stopped(&file));
        };
        let Unwritten::Unread(unread) = *why else {
            panic!("{why:?}");
        };
        let words = unread.to_string();
        section_stops(&file, 9, Unwritten::Unread(unread), &words, true);
    }

    #[test]
    fn an_object_that_isnt_a_stream_is_said() {
        let file = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\nstartxref\n9\n%%EOF\n";
        let id = ObjectId::new(1, 0);
        let words = "1 0 R, at byte 9, isn't a stream";
        section_stops(file, 9, Unwritten::NotAStream { id, at: 9 }, words, true);
    }

    #[test]
    fn a_stream_of_another_type_is_said() {
        let file = stream_file("/Type /ObjStm /Size 3 /W [1 2 1]", &three_rows(45));
        let id = ObjectId::new(2, 0);
        let words = "2 0 R, at byte 45, isn't /Type /XRef";
        section_stops(&file, 45, Unwritten::NotXref { id, at: 45 }, words, true);
    }

    #[test]
    fn widths_that_arent_are_said() {
        let file = stream_file("/Type /XRef /Size 3 /W [1 9 1]", &three_rows(45));
        let words = "its /W isn't three widths of 8 bytes or fewer";
        section_stops(&file, 45, Unwritten::Widths, words, true);
    }

    #[test]
    fn an_index_of_no_pairs_is_said() {
        let file = stream_file(&format!("{XREF} /Index [0 3 1]"), &three_rows(45));
        let words = "its /Index isn't pairs of numbers";
        section_stops(&file, 45, Unwritten::Index, words, true);
    }

    #[test]
    fn an_index_past_the_numbers_is_said() {
        let dict = "/Type /XRef /Size 3 /W [1 2 1] /Root 1 0 R /Index [4294967294 3]";
        let file = stream_file(dict, &three_rows(45));
        let words = "its /Index numbers no object";
        section_stops(&file, 45, Unwritten::IndexNumber, words, true);
    }

    #[test]
    fn no_size_for_an_index_is_said() {
        let file = stream_file("/Type /XRef /W [1 2 1]", &three_rows(45));
        let words = "it has no /Size";
        section_stops(&file, 45, Unwritten::NoSize, words, true);
    }

    #[test]
    fn a_filter_that_isnt_one_is_said() {
        let dict = format!("{XREF} /Filter /NoSuchFilter");
        let file = stream_file(&dict, &three_rows(45));
        let why = Unfiltered::Unsupported {
            filter: b"NoSuchFilter".to_vec(),
        };
        let words = "its cross-reference stream doesn't decode: its filter /NoSuchFilter isn't one the reader decodes";
        section_stops(&file, 45, Unwritten::Filters(why), words, true);
    }

    /// Only the sections one by one say it: a reading tells the decode's
    /// faults on the stream, and its rows end where they decode.
    #[test]
    fn data_that_doesnt_decode_as_far_as_its_rows_is_said() {
        let dict = format!("{XREF} /Filter /ASCIIHexDecode");
        let file = stream_file(&dict, b"00zz");
        let SectionFault::Section { at: 45, why } = stopped(&file) else {
            panic!("{:?}", stopped(&file));
        };
        let Unwritten::Decode { fault } = *why else {
            panic!("{why:?}");
        };
        let words = format!("its data doesn't decode as far as its rows: {fault}");
        section_stops(&file, 45, Unwritten::Decode { fault }, &words, false);
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        assert_eq!(reading.trailer().unwritten, Some(Unwritten::DataShort));
    }

    #[test]
    fn data_short_of_its_index_is_said() {
        let file = stream_file(XREF, &three_rows(45)[..8]);
        let words = "its data ends before its /Index does";
        section_stops(&file, 45, Unwritten::DataShort, words, true);
    }

    /// A hybrid table's `/XRefStm` that doesn't read stops the sections at
    /// the stream, its reason the stream's.
    #[test]
    fn a_hybrid_tables_stream_that_doesnt_read_is_said() {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let table = file.len();
        file.extend_from_slice(ROWS.as_bytes());
        file.extend_from_slice(b"trailer\n<< /Size 2 /Root 1 0 R /XRefStm 9 >>\n");
        file.extend_from_slice(format!("startxref\n{table}\n%%EOF\n").as_bytes());
        let id = ObjectId::new(1, 0);
        let why = Box::new(Unwritten::NotAStream { id, at: 9 });
        assert_eq!(stopped(&file), SectionFault::Section { at: 9, why });
    }

    #[test]
    fn a_prev_that_comes_back_is_said() {
        let file = table_file(ROWS, "trailer\n<< /Size 2 /Prev 45 >>\n");
        let read = sections(&file);
        assert_eq!(read.sections.len(), 1);
        assert_eq!(read.stopped, Some(SectionFault::Loop { at: 45 }));
    }

    #[test]
    fn too_many_sections_are_said() {
        let file = table_file(ROWS, "trailer\n<< /Size 2 /Prev 45 >>\n");
        let mut options = Options::DEFAULT;
        let mut limits = Limits::DEFAULT;
        limits.sections = 0;
        options.limits = limits;
        let reading = Reading::open(&file[..], &options).unwrap();
        let read = reading.sections().unwrap();
        assert_eq!(read.sections, []);
        assert_eq!(
            read.stopped,
            Some(SectionFault::TooManySections { limit: 0 })
        );
    }

    #[test]
    fn too_many_rows_are_said_of_a_table_and_a_stream() {
        let mut limits = Limits::DEFAULT;
        limits.entries = Some(1);
        let mut options = Options::DEFAULT;
        options.limits = limits;
        let file = table_file(ROWS, TRAILER);
        let reading = Reading::open(&file[..], &options).unwrap();
        let read = reading.sections().unwrap();
        assert_eq!(read.stopped, Some(SectionFault::TooManyRows { limit: 1 }));
        let file = stream_file(XREF, &three_rows(45));
        let reading = Reading::open(&file[..], &options).unwrap();
        let read = reading.sections().unwrap();
        assert_eq!(read.stopped, Some(SectionFault::TooManyRows { limit: 1 }));
    }

    #[test]
    fn rows_are_counted_across_sections() {
        let mut file = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let first = file.len();
        file.extend_from_slice(ROWS.as_bytes());
        file.extend_from_slice(TRAILER.as_bytes());
        let second = file.len();
        file.extend_from_slice(ROWS.as_bytes());
        let trailer = format!("trailer\n<< /Size 2 /Prev {first} >>\n");
        file.extend_from_slice(trailer.as_bytes());
        file.extend_from_slice(format!("startxref\n{second}\n%%EOF\n").as_bytes());
        let mut limits = Limits::DEFAULT;
        limits.entries = Some(3);
        let mut options = Options::DEFAULT;
        options.limits = limits;
        let reading = Reading::open(&file[..], &options).unwrap();
        let read = reading.sections().unwrap();
        assert_eq!(read.sections.len(), 1);
        assert_eq!(read.stopped, Some(SectionFault::TooManyRows { limit: 3 }));
    }

    #[test]
    fn its_first_object_is_as_written() {
        let file = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n3 1 obj\n<< /Linearized 1 >>\nendobj\nmore";
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let first = reading.first_object().unwrap().expect("it has one");
        assert_eq!(first.id, ObjectId::new(3, 1));
        assert_eq!(first.at, 15);
        assert_eq!(
            &file[first.at..first.end],
            b"3 1 obj\n<< /Linearized 1 >>\nendobj"
        );
        assert!(first.object.as_dict().is_some_and(|d| d.has(b"Linearized")));
    }

    #[test]
    fn a_first_object_past_some_junk_is_from_the_header() {
        let file = b"junk%PDF-1.7\n3 0 obj\n<< >>\nendobj\n";
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        assert_eq!(reading.first_object().unwrap().unwrap().at, 13);
    }

    #[test]
    fn no_first_object_is_none() {
        for file in [&b"no header"[..], b"%PDF-1.7\n", b"%PDF-1.7\nxref\n"] {
            let reading = Reading::open(file, &Options::DEFAULT).unwrap();
            assert_eq!(reading.first_object().unwrap(), None, "{file:?}");
        }
    }
}
