//! The tables both ways, each held to bits laid out here by hand, one field at
//! a time with no bit reader or writer, so neither way grades the other.

use super::*;

/// Bits by hand: each field in its width, high-order bit first, and `Pad` to go
/// on to the next byte.
enum Bit {
    F(u64, u32),
    Pad,
}
use Bit::{F, Pad};

fn by_hand(fields: &[Bit]) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::new();
    for field in fields {
        match field {
            F(value, width) => {
                assert!(*width == 64 || value >> width == 0, "{value} in {width}");
                bits.extend((0..*width).rev().map(|bit| value >> bit & 1 == 1));
            }
            Pad => bits.resize(bits.len().next_multiple_of(8), false),
        }
    }
    bits.resize(bits.len().next_multiple_of(8), false);
    bits.chunks(8)
        .map(|byte| byte.iter().fold(0, |sum, bit| sum << 1 | u8::from(*bit)))
        .collect()
}

/// The table's bytes by hand, its first header fields given, its widths as
/// here: the objects 3 bits, lengths 6, counts 2, identifiers 3, numerators 2,
/// content offsets 4, content lengths 5.
fn page_table_bytes() -> Vec<u8> {
    by_hand(&[
        F(3, 32),   // the least objects
        F(777, 32), // the first page
        F(3, 16),
        F(100, 32), // the least length
        F(6, 16),
        F(9, 32), // the least content offset
        F(4, 16),
        F(20, 32), // the least content length
        F(5, 16),
        F(2, 16),
        F(3, 16),
        F(2, 16),
        F(4, 16), // the denominator
        // Each item's run ends on a byte boundary.
        F(0, 3),
        F(5, 3),
        F(2, 3),
        Pad,
        F(40, 6),
        F(0, 6),
        F(63, 6),
        Pad,
        F(0, 2),
        F(2, 2),
        F(1, 2),
        Pad,
        F(6, 3),
        F(1, 3),
        F(7, 3),
        Pad,
        F(3, 2),
        F(0, 2),
        F(1, 2),
        Pad,
        F(15, 4),
        F(0, 4),
        F(1, 4),
        Pad,
        F(31, 5),
        F(0, 5),
        F(1, 5),
    ])
}

fn page_table() -> PageOffsets {
    let entry = |objects, length, shared: &[(u32, u32)], at, content| PageEntry {
        objects,
        length,
        shared: shared.to_vec(),
        content_at: at,
        content_length: content,
    };
    PageOffsets {
        header: PageHeader {
            min_objects: 3,
            first_page_at: 777,
            objects_bits: 3,
            min_length: 100,
            length_bits: 6,
            min_content_at: 9,
            content_at_bits: 4,
            min_content_length: 20,
            content_length_bits: 5,
            count_bits: 2,
            id_bits: 3,
            numerator_bits: 2,
            denominator: 4,
        },
        pages: vec![
            entry(3, 140, &[], 24, 51),
            entry(8, 100, &[(6, 3), (1, 0)], 9, 20),
            entry(5, 163, &[(7, 1)], 10, 21),
        ],
    }
}

#[test]
fn the_page_table_reads_and_writes_as_laid_out_by_hand() {
    let bytes = page_table_bytes();
    let table = PageOffsets::read(&bytes, 3, &Limits::DEFAULT).unwrap();
    assert_eq!(table, page_table());
    let mut written = Vec::new();
    table.write(&mut written).unwrap();
    assert_eq!(written, bytes);
}

/// `of` makes the header from the entries: each width just what the difference
/// needs, a width of 0 only where readers take one, and the least of each.
#[test]
fn a_page_tables_header_is_as_narrow_as_readers_take() {
    let entry = |objects, length, at, content| PageEntry {
        objects,
        length,
        shared: Vec::new(),
        content_at: at,
        content_length: content,
    };
    // Differences of 4, 7, 8 and 3 take 3, 3, 4 and 2 bits, and no more.
    let table = PageOffsets::of(50, vec![entry(2, 90, 1, 5), entry(6, 97, 9, 8)], 3, 4);
    assert_eq!(
        table.header,
        PageHeader {
            min_objects: 2,
            first_page_at: 50,
            objects_bits: 3,
            min_length: 90,
            length_bits: 3,
            min_content_at: 1,
            content_at_bits: 4,
            min_content_length: 5,
            // A page's length's, which readers take to be the least it is.
            content_length_bits: 3,
            // PDFium refuses a width of 0 for these, so they have a bit.
            count_bits: 1,
            id_bits: 3,
            numerator_bits: 0,
            denominator: 4,
        }
    );
    // A width that's exactly what the greatest takes.
    let table = PageOffsets::of(50, vec![entry(1, 1, 0, 0), entry(8, 256, 15, 1)], 1, 4);
    assert_eq!(table.header.objects_bits, 3);
    assert_eq!(table.header.length_bits, 8);
    assert_eq!(table.header.content_at_bits, 4);
    assert_eq!(table.header.content_length_bits, 8);
    // With nothing to tell pages apart, a bit anyway, and every entry's still
    // read.
    let table = PageOffsets::of(50, vec![entry(2, 9, 0, 0), entry(2, 9, 0, 0)], 0, 4);
    assert_eq!(table.header.objects_bits, 1);
    assert_eq!(table.header.length_bits, 1);
    assert_eq!(table.header.count_bits, 1);
    assert_eq!(table.header.id_bits, 1);
    assert_eq!(table.header.content_at_bits, 0);
    let mut bytes = Vec::new();
    table.write(&mut bytes).unwrap();
    assert_eq!(PageOffsets::read(&bytes, 2, &Limits::DEFAULT), Ok(table));
}

#[test]
fn a_width_of_nothing_reads_nothing_and_32_reads_all() {
    let bytes = by_hand(&[
        F(u64::from(u32::MAX), 32),
        F(0, 32),
        F(0, 16),
        F(u64::from(u32::MAX), 32),
        F(32, 16),
        F(0, 32),
        F(0, 16),
        F(0, 32),
        F(0, 16),
        F(0, 16),
        F(0, 16),
        F(0, 16),
        F(0, 16),
        F(u64::from(u32::MAX), 32),
    ]);
    let table = PageOffsets::read(&bytes, 1, &Limits::DEFAULT).unwrap();
    assert_eq!(table.pages[0].objects, u64::from(u32::MAX));
    // The least and a difference of all 32 bits, which don't fit 32 bits.
    assert_eq!(table.pages[0].length, 2 * u64::from(u32::MAX));
    let mut written = Vec::new();
    table.write(&mut written).unwrap();
    assert_eq!(written, bytes);
}

#[test]
fn the_shared_table_reads_and_writes_as_laid_out_by_hand() {
    let bytes = by_hand(&[
        F(9, 32),   // part 8's first object
        F(500, 32), // and where it is
        F(1, 32),   // entries of the first page
        F(3, 32),   // entries in all
        F(2, 16),
        F(30, 32), // the least length
        F(5, 16),
        F(0, 5),
        F(31, 5),
        F(1, 5),
        Pad,
        F(0, 1),
        F(1, 1),
        F(0, 1),
        Pad,
        F(0xAA, 8),
        F(0xBB, 8),
        F(0, 56),
        F(0, 56),
        F(2, 2),
        F(0, 2),
        F(1, 2),
    ]);
    let mut signature = [0; 16];
    signature[0] = 0xAA;
    signature[1] = 0xBB;
    let table = SharedObjects::read(&bytes, &Limits::DEFAULT).unwrap();
    assert_eq!(
        table,
        SharedObjects {
            header: SharedHeader {
                first_object: 9,
                first_at: 500,
                first_page_entries: 1,
                entries: 3,
                objects_bits: 2,
                min_length: 30,
                length_bits: 5,
            },
            groups: vec![
                GroupEntry {
                    length: 30,
                    signature: None,
                    objects: 3,
                },
                GroupEntry {
                    length: 61,
                    signature: Some(signature),
                    objects: 1,
                },
                GroupEntry {
                    length: 31,
                    signature: None,
                    objects: 2,
                },
            ],
        }
    );
    let mut written = Vec::new();
    table.write(&mut written).unwrap();
    assert_eq!(written, bytes);
}

/// `of` has the count's width that of the greatest count, not of it less one,
/// and a bit for a length at least.
#[test]
fn a_shared_tables_header_is_as_table_f5_has_it() {
    let entry = |length, objects| GroupEntry {
        length,
        signature: None,
        objects,
    };
    let table = SharedObjects::of(4, 8, 1, vec![entry(10, 4), entry(13, 1)]);
    assert_eq!(table.header.objects_bits, 3);
    assert_eq!(table.header.min_length, 10);
    assert_eq!(table.header.length_bits, 2);
    assert_eq!(table.header.entries, 2);
    let table = SharedObjects::of(4, 8, 1, vec![entry(10, 1), entry(10, 1)]);
    assert_eq!(table.header.objects_bits, 1);
    assert_eq!(table.header.length_bits, 1);
    let mut bytes = Vec::new();
    table.write(&mut bytes).unwrap();
    assert_eq!(SharedObjects::read(&bytes, &Limits::DEFAULT), Ok(table));
}

#[test]
fn a_generic_and_an_extended_table_read_and_write_as_laid_out_by_hand() {
    let bytes = by_hand(&[F(1, 32), F(2, 32), F(3, 32), F(4, 32)]);
    let generic = Generic {
        first_object: 1,
        first_at: 2,
        objects: 3,
        length: 4,
    };
    assert_eq!(Generic::read(&bytes, &Limits::DEFAULT), Ok(generic));
    let mut written = Vec::new();
    generic.write(&mut written).unwrap();
    assert_eq!(written, bytes);

    let bytes = by_hand(&[
        F(1, 32),
        F(2, 32),
        F(3, 32),
        F(4, 32),
        F(3, 32),
        F(5, 16),
        F(16, 5),
        F(1, 5),
        F(0, 5),
    ]);
    let table = Extended::of(generic, vec![16, 1, 0]);
    assert_eq!(table.id_bits, 5);
    assert_eq!(Extended::read(&bytes, &Limits::DEFAULT), Ok(table.clone()));
    let mut written = Vec::new();
    table.write(&mut written).unwrap();
    assert_eq!(written, bytes);
    assert_eq!(Extended::of(generic, Vec::new()).id_bits, 0);
}

#[test]
fn the_thumbnail_table_reads_and_writes_as_laid_out_by_hand() {
    let bytes = by_hand(&[
        F(70, 32),
        F(900, 32),
        F(2, 32),
        F(2, 16),
        F(50, 32),
        F(3, 16),
        F(1, 32),
        F(1, 16),
        F(90, 32),
        F(950, 32),
        F(4, 32),
        F(44, 32),
        // Gaps, then objects, then lengths, each on a byte boundary.
        F(0, 2),
        F(3, 2),
        Pad,
        F(0, 1),
        F(1, 1),
        Pad,
        F(0, 3),
        F(7, 3),
    ]);
    let pages = vec![
        ThumbEntry {
            gap: 0,
            objects: 1,
            length: 50,
        },
        ThumbEntry {
            gap: 3,
            objects: 2,
            length: 57,
        },
    ];
    let shared = Generic {
        first_object: 90,
        first_at: 950,
        objects: 4,
        length: 44,
    };
    let table = Thumbnails::of(70, 900, pages, shared);
    assert_eq!(
        Thumbnails::read(&bytes, &Limits::DEFAULT),
        Ok(table.clone())
    );
    let mut written = Vec::new();
    table.write(&mut written).unwrap();
    assert_eq!(written, bytes);
}

/// Identifiers are in the page offset table's width, as this table says none of
/// its own (F.4.7).
#[test]
fn the_embedded_file_table_reads_and_writes_as_laid_out_by_hand() {
    let bytes = by_hand(&[
        F(60, 32),
        F(800, 32),
        F(2, 32),
        F(6, 16),
        F(2, 16),
        F(5, 16),
        F(2, 16),
        F(60, 6),
        F(60, 6),
        Pad,
        F(2, 2),
        F(0, 2),
        Pad,
        F(31, 5),
        F(0, 5),
        Pad,
        F(0, 2),
        F(2, 2),
        Pad,
        F(5, 4),
        F(9, 4),
    ]);
    let table = EmbeddedFiles::of(
        60,
        800,
        vec![
            FileEntry {
                number: 60,
                objects: 2,
                length: 31,
                shared: Vec::new(),
            },
            FileEntry {
                number: 60,
                objects: 0,
                length: 0,
                shared: vec![5, 9],
            },
        ],
    );
    assert_eq!(
        EmbeddedFiles::read(&bytes, 4, &Limits::DEFAULT),
        Ok(table.clone())
    );
    let mut written = Vec::new();
    table.write(&mut written, 4).unwrap();
    assert_eq!(written, bytes);
    // In another width they're other values, so it's that one that's used.
    assert_ne!(EmbeddedFiles::read(&bytes, 3, &Limits::DEFAULT), Ok(table));
}

/// All of them, from where the keys say, each on a byte boundary.
#[test]
fn tables_reads_each_from_where_its_key_says() {
    let pages = page_table();
    let shared = SharedObjects::of(
        9,
        500,
        1,
        vec![GroupEntry {
            length: 7,
            signature: None,
            objects: 1,
        }],
    );
    let generic = |first_object| Generic {
        first_object,
        first_at: 2,
        objects: 3,
        length: 4,
    };
    let thumbnails = Thumbnails::of(1, 2, Vec::new(), Generic::default());
    let extended = |first| Extended::of(generic(first), vec![first]);
    let files = EmbeddedFiles::of(
        1,
        2,
        vec![FileEntry {
            number: 1,
            objects: 1,
            length: 1,
            shared: vec![3],
        }],
    );

    let mut data = Vec::new();
    pages.write(&mut data).unwrap();
    let mut keys = HintKeys::new(data.len() as u64);
    shared.write(&mut data).unwrap();
    let key = |data: &Vec<u8>| Some(data.len() as u64);
    keys.thumbnails = key(&data);
    thumbnails.write(&mut data).unwrap();
    keys.outlines = key(&data);
    generic(10).write(&mut data).unwrap();
    keys.threads = key(&data);
    generic(11).write(&mut data).unwrap();
    keys.dests = key(&data);
    generic(12).write(&mut data).unwrap();
    keys.form = key(&data);
    extended(13).write(&mut data).unwrap();
    keys.info = key(&data);
    generic(14).write(&mut data).unwrap();
    keys.structure = key(&data);
    extended(15).write(&mut data).unwrap();
    keys.labels = key(&data);
    generic(16).write(&mut data).unwrap();
    keys.renditions = key(&data);
    extended(17).write(&mut data).unwrap();
    keys.files = key(&data);
    files.write(&mut data, pages.header.id_bits).unwrap();

    let read = tables(&data, &keys, 3, &Limits::DEFAULT).unwrap();
    assert_eq!(read.pages, pages);
    assert_eq!(read.shared, shared);
    assert_eq!(read.thumbnails, Some(thumbnails));
    assert_eq!(read.outlines, Some(generic(10)));
    assert_eq!(read.threads, Some(generic(11)));
    assert_eq!(read.dests, Some(generic(12)));
    assert_eq!(read.form, Some(extended(13)));
    assert_eq!(read.info, Some(generic(14)));
    assert_eq!(read.structure, Some(extended(15)));
    assert_eq!(read.labels, Some(generic(16)));
    assert_eq!(read.renditions, Some(extended(17)));
    assert_eq!(read.files, Some(files));

    // What isn't asked for isn't there.
    let mut few = HintKeys::new(keys.shared);
    few.outlines = keys.outlines;
    let read = tables(&data, &few, 3, &Limits::DEFAULT).unwrap();
    assert_eq!(read.thumbnails, None);
    assert_eq!(read.outlines, Some(generic(10)));
}

#[test]
fn a_position_is_as_if_the_hint_stream_werent_there() {
    // A hint stream of 50 bytes at 100: before it as it is, after it less it.
    for (at, reads) in [(0, 0), (99, 99), (150, 100), (151, 101), (1000, 950)] {
        assert_eq!(position(at, 100, 50), reads);
        assert_eq!(located(reads, 100, 50), at);
    }
}

/// Reading past the data says where; each table's every field does.
#[test]
fn reading_past_the_data_is_an_error_at_the_bit() {
    let bytes = page_table_bytes();
    for len in 0..bytes.len() {
        let read = PageOffsets::read(&bytes[..len], 3, &Limits::DEFAULT);
        assert!(
            matches!(read, Err(Error::PastEnd { .. })),
            "{len}: {read:?}"
        );
    }
    assert_eq!(
        PageOffsets::read(&bytes[..3], 3, &Limits::DEFAULT),
        Err(Error::PastEnd {
            what: "the fewest objects a page has",
            bit: 0,
            bits: 24,
        })
    );
    // Two fields in: the third starts at bit 64.
    assert_eq!(
        PageOffsets::read(&bytes[..9], 3, &Limits::DEFAULT),
        Err(Error::PastEnd {
            what: "a page's number of objects",
            bit: 64,
            bits: 72,
        })
    );
    let mut bytes = Vec::new();
    Generic::default().write(&mut bytes).unwrap();
    for len in 0..bytes.len() {
        let read = Generic::read(&bytes[..len], &Limits::DEFAULT);
        assert!(matches!(read, Err(Error::PastEnd { .. })), "{len}");
    }
}

/// A key past the data is refused by which it is, whichever table's.
#[test]
fn a_table_past_the_data_is_an_error() {
    let mut bytes = Vec::new();
    page_table().write(&mut bytes).unwrap();
    let len = bytes.len() as u64;
    let keys = HintKeys::new(len);
    assert_eq!(
        tables(&bytes, &keys, 3, &Limits::DEFAULT),
        Err(Error::TableAt {
            table: "/S",
            at: len,
            len,
        })
    );
    assert!(matches!(
        tables(&[], &keys, 3, &Limits::DEFAULT),
        Err(Error::TableAt {
            table: "page offset",
            ..
        })
    ));
    // Shared at 0, as the shared table's reading anything; it's the optional
    // one.
    let mut keys = HintKeys::new(0);
    keys.labels = Some(u64::MAX);
    assert!(matches!(
        tables(&[0; 200], &keys, 0, &Limits::DEFAULT),
        Err(Error::TableAt {
            table: "/L",
            at: u64::MAX,
            ..
        })
    ));
}

/// A width past 32 is refused where it's read, whichever it is, not used.
#[test]
fn a_width_past_32_is_an_error_before_its_used() {
    // The third field of each's header, where the first width is.
    let bytes = by_hand(&[F(1, 32), F(1, 32), F(33, 16), F(0, 64), F(0, 64), F(0, 64)]);
    assert_eq!(
        PageOffsets::read(&bytes, 1, &Limits::DEFAULT),
        Err(Error::Width {
            what: "a page's number of objects",
            width: 33,
        })
    );
    let bytes = by_hand(&[F(0, 64), F(0, 64), F(40_000, 16), F(0, 64)]);
    assert_eq!(
        SharedObjects::read(&bytes, &Limits::DEFAULT),
        Err(Error::Width {
            what: "a shared entry's count of objects",
            width: 40_000,
        })
    );
    let table = EmbeddedFiles::of(0, 0, Vec::new());
    let mut bytes = Vec::new();
    table.write(&mut bytes, 1).unwrap();
    assert_eq!(
        EmbeddedFiles::read(&bytes, 33, &Limits::DEFAULT),
        Err(Error::Width {
            what: "an embedded file's identifier",
            width: 33,
        })
    );
}

/// `/N` above the cap, and a count past the data, are refused by that check:
/// not by a read that comes to the data's end, so nothing's been made room for.
/// That a page's entries read nothing doesn't get past the charge.
#[test]
fn a_count_is_checked_before_theres_room_for_it() {
    assert_eq!(
        PageOffsets::read(&[], u32::MAX, &Limits::DEFAULT),
        Err(Error::TooManyPages {
            pages: u64::from(u32::MAX),
            limit: 0xFFFFE,
        })
    );
    let widths = |objects| {
        by_hand(&[
            F(1, 32),
            F(1, 32),
            F(objects, 16),
            F(1, 32),
            F(0, 16),
            F(0, 32),
            F(0, 16),
            F(0, 32),
            F(0, 16),
            F(0, 16),
            F(0, 16),
            F(0, 16),
            F(0, 16),
        ])
    };
    assert_eq!(
        PageOffsets::read(&widths(1), 0xFFFFE, &Limits::DEFAULT),
        Err(Error::PastEnd {
            what: "the page offset table's entries",
            bit: 288,
            bits: 288,
        })
    );
    let mut limits = Limits::DEFAULT;
    limits.hint_entries = 1000;
    assert_eq!(
        PageOffsets::read(&widths(0), 1001, &limits),
        Err(Error::TooManyEntries {
            entries: 1001,
            limit: 1000,
        })
    );
    assert!(PageOffsets::read(&widths(0), 1000, &limits).is_ok());

    // A page's count of shared objects asks for four billion, with widths of 0
    // for them too.
    let mut bytes = by_hand(&[
        F(1, 32),
        F(1, 32),
        F(0, 16),
        F(1, 32),
        F(0, 16),
        F(0, 32),
        F(0, 16),
        F(0, 32),
        F(0, 16),
        F(32, 16),
        F(0, 16),
        F(0, 16),
        F(0, 16),
    ]);
    bytes.extend_from_slice(&[0xFF; 4]);
    assert_eq!(
        PageOffsets::read(&bytes, 1, &Limits::DEFAULT),
        Err(Error::TooManyEntries {
            entries: 1 + u64::from(u32::MAX),
            limit: 1 << 24,
        })
    );

    // Each other table's count.
    let shared = by_hand(&[
        F(0, 64),
        F(0, 32),
        F(u64::from(u32::MAX), 32),
        F(0, 16),
        F(0, 32),
        F(0, 16),
    ]);
    assert_eq!(
        SharedObjects::read(&shared, &Limits::DEFAULT),
        Err(Error::PastEnd {
            what: "the shared object table's entries",
            bit: 192,
            bits: 192,
        })
    );
    let extended = by_hand(&[F(0, 64), F(0, 64), F(u64::from(u32::MAX), 32), F(1, 16)]);
    assert_eq!(
        Extended::read(&extended, &Limits::DEFAULT),
        Err(Error::PastEnd {
            what: "an extended table's identifiers",
            bit: 176,
            bits: 176,
        })
    );
    let extended = by_hand(&[F(0, 64), F(0, 64), F(1 << 25, 32), F(0, 16)]);
    assert!(matches!(
        Extended::read(&extended, &Limits::DEFAULT),
        Err(Error::TooManyEntries { .. })
    ));
    let thumbnails = by_hand(&[
        F(0, 64),
        F(u64::from(u32::MAX), 32),
        F(1, 16),
        F(0, 32),
        F(0, 16),
        F(0, 32),
        F(0, 16),
        F(0, 64),
        F(0, 64),
    ]);
    assert!(matches!(
        Thumbnails::read(&thumbnails, &Limits::DEFAULT),
        Err(Error::PastEnd {
            what: "the thumbnail table's entries",
            ..
        })
    ));
    let files = by_hand(&[F(0, 64), F(u64::from(u32::MAX), 32), F(1, 16), F(0, 48)]);
    assert!(matches!(
        EmbeddedFiles::read(&files, 1, &Limits::DEFAULT),
        Err(Error::PastEnd {
            what: "the embedded file table's entries",
            ..
        })
    ));
}

/// What's charged goes on from table to table, so a stream can't ask for the
/// cap from each.
#[test]
fn one_count_of_entries_covers_all_the_tables() {
    let mut data = Vec::new();
    PageOffsets::default().write(&mut data).unwrap();
    SharedObjects::default().write(&mut data).unwrap();
    let mut keys = HintKeys::new(36);
    let at = data.len() as u64;
    let empty = by_hand(&[F(0, 64), F(0, 64), F(600, 32), F(0, 16)]);
    data.extend_from_slice(&empty);
    keys.form = Some(at);
    keys.structure = Some(at);
    let mut limits = Limits::DEFAULT;
    limits.hint_entries = 1200;
    assert!(tables(&data, &keys, 0, &limits).is_ok());
    limits.hint_entries = 1199;
    assert_eq!(
        tables(&data, &keys, 0, &limits),
        Err(Error::TooManyEntries {
            entries: 1200,
            limit: 1199,
        })
    );
}

/// What can't be written is refused, in its table's widths: a value that
/// doesn't fit, one under the least.
#[test]
fn a_value_its_widths_cant_hold_isnt_written() {
    let mut table = page_table();
    table.pages[0].objects = 2;
    assert_eq!(
        table.write(&mut Vec::new()),
        Err(Error::Value {
            what: "a page's number of objects",
            value: 2,
            width: 3,
        })
    );
    let mut table = page_table();
    table.pages[0].objects = 3 + 8;
    assert_eq!(
        table.write(&mut Vec::new()),
        Err(Error::Value {
            what: "a page's number of objects",
            value: 8,
            width: 3,
        })
    );
    let mut table = page_table();
    table.pages[0].shared.push((8, 0));
    assert!(table.write(&mut Vec::new()).is_err());
    let mut table = page_table();
    table.header.id_bits = 33;
    assert!(table.write(&mut Vec::new()).is_err());
}

/// A position past a stream the far end of what a `u64` holds is no position
/// past it, and doesn't overflow either way.
#[test]
fn a_position_at_the_far_end_of_u64_is_itself() {
    assert_eq!(position(10, u64::MAX, 2), 10);
    assert_eq!(position(10, 2, u64::MAX), 10);
    assert_eq!(located(10, 2, u64::MAX), u64::MAX);
}
