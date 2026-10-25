# arlington-pdf-model

The PDF Association's Arlington PDF Model: the grammar of PDF's objects
(ISO 32000-2 with its errata), as TSV, a table per object.

- From: https://github.com/pdf-association/arlington-pdf-model at d148fee5a0cb6dcacadf58c09ed842e32847024d (2026-10-21, "Resolve #180")
- Fetched: 2026-10-24, from https://codeload.github.com/pdf-association/arlington-pdf-model/tar.gz/d148fee5a0cb6dcacadf58c09ed842e32847024d
- Taken: `LICENSE`, `NOTICE.txt`, `tsv/latest/*.tsv` (613 files, 493,082 bytes): one grammar for every PDF version, whose SinceVersion and DeprecatedIn columns and version predicates say which version each key, type and value belongs to
- Left out: `tsv/1.0` to `tsv/2.0` (the same grammar cut per version), `TestGrammar`, `scripts`, `gcxml`, `xml`, `Jupyter`, `3dvisualize`, `resources`, the docs (`README.md`, `INTERNAL_GRAMMAR.md` and the rest) and the spreadsheets
- License: Apache-2.0, as `LICENSE` and `NOTICE.txt` say (`CITATION.cff` too). `NOTICE.txt` puts the repository's other documentation under CC-BY-4.0: none of it is here
- Used by: `pdf-lint`, whose `build.rs` embeds every table in the library, so `cargo install` needs no checkout. `--model <DIR>` checks against a newer `tsv/latest` checkout instead

The files are as the tarball has them, byte for byte (`.gitattributes`
keeps every checkout from changing their line ends).

## Corrections and additions

`corrections.tsv`, beside `tsv/`, lists the cells where the model
contradicts ISO 32000-2, or the reference it cites, a line each: the
table, the row's key, the column, the cell as upstream has it, as
corrected, and why; a corrected predicate may call a function pdf-lint
defines, which the reason says (`fn:IsFullyQualifiedFieldName`).

`additions/` has what the model lacks that a reference defines, as
files in upstream's own format, a table each: one named for a table
upstream has adds its rows (before its wildcard row, if it ends with
one), and one named for a table upstream hasn't adds that table. Each
added row's Note says why, beginning `pdf-lint:`.

`build.rs` makes both to the copy it embeds, never to the files here,
and stops the build when a corrected cell isn't as the line says
upstream has it, or upstream has an added row's key, or a table of an
added table's name, itself. So the embedded model's hash
(`Grammar::hash`) is of the corrected and added tables, not upstream's
at the same commit; `--model` reads a checkout as it is; and a release
archive carries them as `ARLINGTON-CORRECTIONS.tsv` and
`ARLINGTON-ADDITIONS/`. Each goes once upstream fixes the model.

To update: fetch a newer commit the same way, into a scratch directory;
replace `LICENSE`, `NOTICE.txt` and `tsv/latest` with its own; build, and
for each correction or addition the build stops on, drop it if upstream
fixed the model, or redo it if upstream changed it otherwise; update the
commit here and in `pdf_lint::COMMIT`, and the two hashes in
`grammar.rs`'s `the_models_hashes_are_the_references`, upstream's tables'
and the embedded ones' (`Grammar::hash` says how to work them out); rerun
`just test`; and record in the changelog what moved.
