//! The model's tables as the walk looks a dictionary's keys up in them:
//! each table's rows by their keys, made the first time the walk asks for
//! the table and kept for the run, every file and thread of it, as the
//! model is ([`grammar`]). What a row says of a file, its version and the
//! extensions it's checked under, is never kept here, only what its text
//! says alone.

use std::sync::OnceLock;

use crate::grammar::{Row, Table, Wrapped, grammar};
use crate::object::{Dict, Object};

/// A table's rows by their keys.
pub(super) struct Rows {
    table: &'static Table,
    /// Each row's key and place in the table, by the key's first byte, an
    /// empty key's first, then in the table's order: a table has a row a
    /// key at most, as the loader holds it to.
    keys: Box<[(&'static [u8], usize)]>,
    /// Where each first byte's keys start in `keys`, an empty key's at 0
    /// and byte `b`'s at `b + 1`, and where the last's end.
    starts: Box<[u32; 258]>,
    /// The wildcard's place, `*`, where the table has one.
    wildcard: Option<usize>,
    /// Each row's links' tables, of each of its types, in the order the
    /// row lists them: found the first time they're asked for.
    links: Box<[OnceLock<Links>]>,
}

/// A row's links' tables, of each of its types.
type Links = Box<[Box<[&'static Table]>]>;

/// How many rows a table has at most for [`Rows::entries`] to give their
/// entries without allocating: more than any of the model's has.
const FEW: usize = 64;

/// Each of a table's rows' values in a dictionary, by the row's place: its
/// key's last entry's, as [`Dict::get`] gives it.
pub(super) struct Entries<'o> {
    /// A table's of [`FEW`] rows or fewer, on the stack.
    few: [Option<&'o Object>; FEW],
    /// A table's of more, where `few` is unused.
    many: Vec<Option<&'o Object>>,
}

impl<'o> Entries<'o> {
    /// The value under the key of the row at `place`, if the dictionary
    /// has the key.
    pub(super) fn get(&self, place: usize) -> Option<&'o Object> {
        match self.many.is_empty() {
            true => self.few[place],
            false => self.many[place],
        }
    }
}

impl Rows {
    fn of(table: &'static Table) -> Rows {
        let rows = &table.rows;
        let mut starts = Box::new([0; 258]);
        for row in rows {
            starts[first(row.key.as_bytes()) + 1] += 1;
        }
        for b in 1..starts.len() {
            starts[b] += starts[b - 1];
        }
        // Each in its first byte's place, in the table's order.
        let mut next = *starts;
        let mut keys = vec![(&b""[..], 0); rows.len()];
        for (place, row) in rows.iter().enumerate() {
            let key = row.key.as_bytes();
            let at = &mut next[first(key)];
            keys[*at as usize] = (key, place);
            *at += 1;
        }
        Rows {
            table,
            keys: keys.into_boxed_slice(),
            starts,
            wildcard: rows.iter().position(|row| row.key == "*"),
            links: rows.iter().map(|_| OnceLock::new()).collect(),
        }
    }

    /// The place of the row with `key`, if the table has one.
    fn place(&self, key: &[u8]) -> Option<usize> {
        let b = first(key);
        let keys = &self.keys[self.starts[b] as usize..self.starts[b + 1] as usize];
        keys.iter()
            .find(|(k, _)| *k == key)
            .map(|&(_, place)| place)
    }

    /// The row for the value under `key`: the row with the key, else the
    /// wildcard, as [`Table::row`] finds it.
    pub(super) fn row(&self, key: &[u8]) -> Option<&'static Row> {
        let place = self.place(key).or(self.wildcard);
        place.map(|place| &self.table.rows[place])
    }

    /// The tables `row`'s `i`th type links to, in the order it lists them,
    /// found by their names the first time the row's are asked for. `row`
    /// is one of the table's.
    pub(super) fn links(&self, row: &'static Row, i: usize) -> &[&'static Table] {
        let place = (self.place(row.key.as_bytes()))
            .filter(|&place| std::ptr::eq(&self.table.rows[place], row))
            .expect("a row of the table");
        let each = self.links[place].get_or_init(|| {
            let table = |link: &Wrapped<String>| {
                grammar()
                    .table(&link.item)
                    .expect("the loader checks that every link names a table")
            };
            (row.link.iter())
                .map(|links| links.iter().map(table).collect())
                .collect()
        });
        &each[i]
    }

    /// Each of the rows' values in `dict`, by the row's place, walking its
    /// entries once: a key written twice gives its last entry's.
    pub(super) fn entries<'o>(&self, dict: &'o Dict) -> Entries<'o> {
        let n = self.table.rows.len();
        let mut entries = Entries {
            few: [None; FEW],
            many: match n {
                n if n <= FEW => Vec::new(),
                n => vec![None; n],
            },
        };
        let slots = match n {
            n if n <= FEW => &mut entries.few[..],
            _ => &mut entries.many[..],
        };
        for entry in dict.entries() {
            if let Some(place) = self.place(&entry.key) {
                slots[place] = Some(&entry.value);
            }
        }
        entries
    }
}

/// Where `key`'s first byte's keys are in [`Rows::keys`], an empty key's
/// first.
fn first(key: &[u8]) -> usize {
    key.first().map_or(0, |&b| usize::from(b) + 1)
}

/// Every table of the model, by where it's held, with its rows by key once
/// they're asked for.
struct Tables {
    /// Where each is held, in order.
    at: Box<[usize]>,
    /// Each one's rows by key, in that order.
    rows: Box<[OnceLock<Rows>]>,
}

/// `table`'s rows by their keys: made the first time they're asked for, and
/// kept for the run. `table` is one of the model's, [`grammar`]'s, as every
/// table the walk checks against is.
pub(super) fn rows(table: &'static Table) -> &'static Rows {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        let mut at: Box<[usize]> = grammar().tables().map(address).collect();
        at.sort_unstable();
        let rows = at.iter().map(|_| OnceLock::new()).collect();
        Tables { at, rows }
    });
    let i = (tables.at.binary_search(&address(table))).expect("the walk's tables are the model's");
    tables.rows[i].get_or_init(|| Rows::of(table))
}

/// Where `table` is held, which names one of the model's for the run.
fn address(table: &Table) -> usize {
    std::ptr::from_ref(table).addr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::At;

    /// Every table's rows by key are its rows, each found under its key as
    /// the table finds it, and a key of no row is the wildcard's; and each
    /// row's links, of each type, are the tables they name, in order.
    #[test]
    fn every_tables_rows_are_found_by_their_keys_as_the_table_finds_them() {
        for table in grammar().tables() {
            let rows = rows(table);
            assert!(std::ptr::eq(rows.table, table), "{}", table.name);
            assert_eq!(rows.keys.len(), table.rows.len(), "{}", table.name);
            assert_eq!(
                rows.starts[257] as usize,
                table.rows.len(),
                "{}",
                table.name
            );
            for row in &table.rows {
                let key = row.key.as_bytes();
                let found = rows.row(key).map(std::ptr::from_ref);
                assert_eq!(found, Some(std::ptr::from_ref(row)), "{}", table.name);
                let by_table = table.row(At::Key(key)).map(std::ptr::from_ref);
                assert_eq!(found, by_table, "{}/{}", table.name, row.key);
            }
            for (row, i) in
                (table.rows.iter()).flat_map(|row| (0..row.link.len()).map(move |i| (row, i)))
            {
                let named = (row.link[i].iter())
                    .map(|link| std::ptr::from_ref(grammar().table(&link.item).unwrap()));
                let found = rows
                    .links(row, i)
                    .iter()
                    .map(|&table| std::ptr::from_ref(table));
                assert!(found.eq(named), "{}/{} {i}", table.name, row.key);
            }
            for key in [&b"NoSuchKey"[..], b"", b"*", b"0", b"1*"] {
                let found = rows.row(key).map(std::ptr::from_ref);
                let by_table = table.row(At::Key(key)).map(std::ptr::from_ref);
                assert_eq!(found, by_table, "{} {key:?}", table.name);
            }
        }
    }
}
