# Changelog

## 0.1.0 (unreleased)

- **The filters.** `decode` decodes data by a list of `Filter`s, as a stream's `/Filter` lists them: `AsciiHex`, `Ascii85`, `Lzw` (with `/EarlyChange`), `Flate` (zlib, through miniz_oxide) and `RunLength`, each given what the one before gave, no filter's output over the caller's limit.
- **Predictors.** An LZW or Flate filter's `Predictor`: TIFF's (`/Predictor 2`), at 1, 2, 4, 8 or 16 bits a sample, and PNG's (10 to 15), each row by its own tag, undone in place; a `Layout` says `/Colors`, `/BitsPerComponent` and `/Columns`.
- **Faults.** An `Error` has what decoded, through every filter, and each `Fault` with its filter's index, the first of each kind a filter: a byte a filter doesn't take, no end-of-data marker, a broken `~>`, a `z` inside a group, a group over 2³² − 1, a last group of one character, an LZW code not in the table, a full table, no zlib header, corrupt or truncated deflate data, a checksum that doesn't match, a PNG row tag that isn't one, a short last row, a layout that isn't one, the limit. Where the spec leaves a reading open it's qpdf's; where a fault stops a filter what decoded before it is kept.
- **Tests.** Unit tests for each filter and fault, the spec's LZW example, a test LZW encoder over every code width, and a fuzz target (`just fuzz filters`) whose body two unit tests run on random and damaged inputs.
