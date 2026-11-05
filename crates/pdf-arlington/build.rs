//! Embeds the model: every table's name and its file's text, so that
//! `cargo install` needs no checkout of the model; with the corrections in
//! `model/corrections.tsv` made to the embedded copy, each a cell where
//! the model contradicts ISO 32000-2 or the reference it cites; and the
//! rows and tables in `model/additions` added to it, each where the model
//! lacks what a reference defines. The files in `model/tsv/latest` stay as
//! upstream ships them. A correction whose cell upstream no longer has as
//! it was, or an addition upstream now has, stops the build, to be looked
//! at again.

use std::fmt::Write as _;
use std::path::Path;
use std::{env, fs};

/// A correction's columns: the table, the row's key, the column, the
/// cell as upstream has it, as corrected, and why.
const COLUMNS: [&str; 6] = ["Table", "Key", "Column", "Upstream", "Corrected", "Reason"];

fn main() {
    let model = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("model");
    let dir = model.join("tsv/latest");
    let corrections = model.join("corrections.tsv");
    let additions = model.join("additions");
    println!("cargo::rerun-if-changed={}", dir.display());
    println!("cargo::rerun-if-changed={}", corrections.display());
    println!("cargo::rerun-if-changed={}", additions.display());
    let mut names = tables(&dir);
    let header = fs::read_to_string(dir.join("Catalog.tsv")).unwrap();
    let header = header.lines().next().unwrap().to_string();
    // The tables changed, by name, with their text.
    let mut changed: Vec<(String, String)> = Vec::new();

    let text = fs::read_to_string(&corrections)
        .unwrap_or_else(|e| panic!("reading {}: {e}", corrections.display()));
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some(COLUMNS.join("\t").as_str()),
        "{}: its header",
        corrections.display()
    );
    let mut corrected = String::new();
    for (i, line) in lines.enumerate() {
        let at = format!("{}: line {}", corrections.display(), i + 2);
        let cells: Vec<&str> = line.split('\t').collect();
        let [table, key, column, upstream, correction, reason] = cells[..] else {
            panic!("{at}: not {} cells", COLUMNS.len());
        };
        assert!(names.iter().any(|n| n == table), "{at}: no table {table}");
        let text = text_of(&mut changed, &dir, table);
        *text = correct(text, key, column, upstream, correction)
            .unwrap_or_else(|why| panic!("{at}: {table}: {why}"));
        writeln!(
            corrected,
            "    super::Correction {{ table: {table:?}, key: {key:?}, column: {column:?}, \
             upstream: {upstream:?}, corrected: {correction:?}, reason: {reason:?} }},"
        )
        .unwrap();
    }

    let mut added = String::new();
    let mut new = Vec::new();
    for name in tables(&additions) {
        let path = additions.join(format!("{name}.tsv"));
        let text = fs::read_to_string(&path).unwrap();
        let at = path.display();
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some(header.as_str()),
            "{at}: upstream's header"
        );
        let rows: Vec<&str> = lines.filter(|line| !line.is_empty()).collect();
        assert!(!rows.is_empty(), "{at}: no rows");
        let upstream = names.contains(&name);
        for row in &rows {
            let cells: Vec<&str> = row.split('\t').collect();
            assert_eq!(cells.len(), 12, "{at}: {row:?} isn't 12 cells");
            let (key, note) = (cells[0], cells[11]);
            assert!(
                !note.is_empty(),
                "{at}: {key}'s Note, which says why, is empty"
            );
            writeln!(
                added,
                "    super::Addition {{ table: {name:?}, key: {key:?}, table_added: {}, \
                 note: {note:?} }},",
                !upstream
            )
            .unwrap();
        }
        if upstream {
            let table = text_of(&mut changed, &dir, &name);
            *table = add(table, &rows).unwrap_or_else(|why| {
                panic!("{at}: upstream's {name} {why}; drop or redo the addition")
            });
        } else {
            changed.push((name.clone(), text));
            new.push(name);
        }
    }
    names.extend(new);
    names.sort();

    let out_dir = Path::new(&env::var("OUT_DIR").unwrap()).to_path_buf();
    // A table an earlier build changed, and this one doesn't, goes.
    for name in tables(&out_dir) {
        fs::remove_file(out_dir.join(format!("{name}.tsv"))).unwrap();
    }
    let mut out = String::from(
        "/// Every table's name and its file's text, in order of name, with the\n\
         /// corrections made and the additions added.\n\
         pub static TABLES: &[(&str, &str)] = &[\n",
    );
    for name in &names {
        let path = match changed.iter().find(|(n, _)| n == name) {
            Some((_, text)) => {
                let path = out_dir.join(format!("{name}.tsv"));
                fs::write(&path, text).unwrap();
                path
            }
            None => dir.join(format!("{name}.tsv")),
        };
        writeln!(
            out,
            "    ({name:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    out.push_str(
        "];\n\n/// The corrections made, in their file's order.\n\
         pub static CORRECTIONS: &[super::Correction] = &[\n",
    );
    out.push_str(&corrected);
    out.push_str(
        "];\n\n/// The rows added, by table, in each file's order.\n\
         pub static ADDITIONS: &[super::Addition] = &[\n",
    );
    out.push_str(&added);
    out.push_str("];\n");
    fs::write(out_dir.join("model.rs"), out).unwrap();
}

/// The text of the table `name` among `changed`, upstream's from `dir`
/// put there first if it isn't.
fn text_of<'a>(changed: &'a mut Vec<(String, String)>, dir: &Path, name: &str) -> &'a mut String {
    let i = match changed.iter().position(|(n, _)| n == name) {
        Some(i) => i,
        None => {
            let text = fs::read_to_string(dir.join(format!("{name}.tsv"))).unwrap();
            changed.push((name.to_string(), text));
            changed.len() - 1
        }
    };
    &mut changed[i].1
}

/// The names of the tables in `dir`, a `.tsv` file each, in order.
fn tables(dir: &Path) -> Vec<String> {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "tsv"))
        .map(|path| {
            path.file_stem()
                .unwrap()
                .to_str()
                .expect("a UTF-8 name")
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// `text`, a table's file, with the cell in `column` of `key`'s row
/// `upstream` made `correction`: an error when the table has no such
/// column or row, or the cell isn't `upstream`.
fn correct(
    text: &str,
    key: &str,
    column: &str,
    upstream: &str,
    correction: &str,
) -> Result<String, String> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    let header: Vec<&str> = lines[0].split('\t').collect();
    let c = (header.iter().position(|h| *h == column)).ok_or(format!("no column {column}"))?;
    let rows: Vec<usize> = (1..lines.len())
        .filter(|&i| lines[i].split('\t').next() == Some(key))
        .collect();
    let [row] = rows[..] else {
        return Err(format!("{} rows {key}", rows.len()));
    };
    let mut cells: Vec<&str> = lines[row].split('\t').collect();
    if cells.get(c) != Some(&upstream) {
        let found = cells.get(c).copied().unwrap_or("nothing");
        return Err(format!(
            "{key}'s {column} is {found:?}, not {upstream:?}: upstream changed it; drop or redo the correction"
        ));
    }
    cells[c] = correction;
    let line = cells.join("\t");
    lines[row] = &line;
    Ok(lines.join("\n"))
}

/// `text`, a table's file, with `rows` added: before its wildcard row,
/// when its last is one, else last. An error when it has a row of one's
/// key already, or its rows are an array's elements.
fn add(text: &str, rows: &[&str]) -> Result<String, String> {
    let mut lines: Vec<&str> = text.lines().collect();
    let keys: Vec<&str> = lines[1..]
        .iter()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    if keys
        .iter()
        .any(|k| k.starts_with(|c: char| c.is_ascii_digit()))
    {
        return Err("is an array's".to_string());
    }
    for row in rows {
        let key = row.split('\t').next().unwrap();
        if keys.contains(&key) {
            return Err(format!("has {key} now"));
        }
    }
    let at = match keys.last() {
        Some(&"*") => lines.len() - 1,
        _ => lines.len(),
    };
    lines.splice(at..at, rows.iter().copied());
    Ok(lines.join("\n") + "\n")
}
