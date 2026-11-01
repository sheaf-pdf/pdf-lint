# Development tasks. CI runs these same recipes, so a local `just ci` checks
# what CI checks.

# The oldest Rust pdf-tokenizer, pdf-filters, pdf-crypt, pdf-objects,
# pdf-structure, pdf-content-streams, pdf-semantics, pdf-linearize and
# pdf-arlington support, from their Cargo.toml. Only the libraries promise one:
# the checker ships as a binary built with stable Rust.
tokenizer_msrv := `grep -m1 '^rust-version' crates/pdf-tokenizer/Cargo.toml | cut -d'"' -f2`
filters_msrv := `grep -m1 '^rust-version' crates/pdf-filters/Cargo.toml | cut -d'"' -f2`
crypt_msrv := `grep -m1 '^rust-version' crates/pdf-crypt/Cargo.toml | cut -d'"' -f2`
objects_msrv := `grep -m1 '^rust-version' crates/pdf-objects/Cargo.toml | cut -d'"' -f2`
structure_msrv := `grep -m1 '^rust-version' crates/pdf-structure/Cargo.toml | cut -d'"' -f2`
content_msrv := `grep -m1 '^rust-version' crates/pdf-content-streams/Cargo.toml | cut -d'"' -f2`
semantics_msrv := `grep -m1 '^rust-version' crates/pdf-semantics/Cargo.toml | cut -d'"' -f2`
linearize_msrv := `grep -m1 '^rust-version' crates/pdf-linearize/Cargo.toml | cut -d'"' -f2`
arlington_msrv := `grep -m1 '^rust-version' crates/pdf-arlington/Cargo.toml | cut -d'"' -f2`
# pdf-content-streams' `image-ends` feature needs a newer one than the crate:
# its fax decoder, hayro-ccitt, uses Rust 1.87's library.
content_image_ends_msrv := "1.87"

# The nightly the fuzz targets build on: `nightly`, or the toolchain
# FUZZ_TOOLCHAIN names, as CI's fuzzing pins one.
fuzz_toolchain := env("FUZZ_TOOLCHAIN", "nightly")

# The PDFium build the readers that follow PDFium's rules are held to
# (`_pdfium`, oracle/README.md).
pdfium_build := "8125"

# The crates' PDFium cases, each a crate's tests/pdfium.
pdfium_sets := "crates/pdf-tokenizer/tests/pdfium crates/pdf-objects/tests/pdfium crates/pdf-content-streams/tests/pdfium crates/pdf-semantics/tests/pdfium"

# List the recipes.
default:
    @just --list --unsorted

# Everything CI runs.
ci: fmt clippy test test-qpdf test-pdfium doc fuzzing dist structure-cross tokenizer-no-std tokenizer-core-only tokenizer-msrv filters-msrv crypt-msrv objects-msrv structure-msrv content-msrv semantics-msrv linearize-msrv arlington-msrv

# Check formatting; `cargo fmt --all` fixes it.
fmt:
    cargo fmt --all -- --check

# Lint every crate, with its tests and binaries; then the libraries and the
# binary as they're built for use, without the features only their tests
# turn on; then the filters with zlib-rs their only Flate backend; then the
# content streams with `image-ends`, which no other crate turns on; then
# pdf-semantics without its clock, as pdf-lint takes it.
clippy:
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo clippy --locked --workspace --lib --bins -- -D warnings
    cargo clippy --locked -p pdf-filters --no-default-features --features zlib-rs --all-targets -- -D warnings
    cargo clippy --locked -p pdf-content-streams --features image-ends --all-targets -- -D warnings
    cargo clippy --locked -p pdf-semantics --no-default-features --all-targets -- -D warnings

# Unit tests and doctests: the workspace's, where the filters have both
# Flate backends, as pdf-structure's tests turn zlib-rs on; then the
# filters' with zlib-rs alone; then the content streams' with `image-ends`;
# then pdf-semantics' without its clock.
test:
    cargo test --locked --workspace
    cargo test --locked -p pdf-filters --no-default-features --features zlib-rs
    cargo test --locked -p pdf-content-streams --features image-ends
    cargo test --locked -p pdf-semantics --no-default-features

# qpdf, the tests' oracle: its official Linux x86_64 zip at 12.4.2, at a
# pinned SHA-256 (from the release's qpdf-12.4.2.sha256), kept in target/.
_qpdf:
    #!/usr/bin/env bash
    set -euo pipefail
    version=12.4.2
    if [ "$(rustc -vV | sed -n 's/^host: //p')" != x86_64-unknown-linux-gnu ]; then
        echo "qpdf $version is fetched for Linux x86_64 only: name another in PDF_LINT_QPDF" >&2
        exit 2
    fi
    dir=$PWD/target/qpdf-$version
    if [ ! -x "$dir/bin/qpdf" ]; then
        zip=qpdf-$version-bin-linux-x86_64.zip
        work=$(mktemp -d)
        trap 'rm -rf "$work"' EXIT
        curl -fsSL --retry 3 -o "$work/$zip" \
            "https://github.com/qpdf/qpdf/releases/download/v$version/$zip"
        echo "db367d897829f22c4198ce1094143c9d467bd6ee7dfabc44ba6f02056b24f8b1  $work/$zip" \
            | sha256sum --check --quiet
        rm -rf "$dir"
        mkdir -p "$dir"
        unzip -q "$work/$zip" -d "$dir"
    fi

# The tests that set the reader, and what the writer writes, beside qpdf,
# pdf-structure's and then the checker's in pdf-lint, which `just test`
# leaves out as ignored, against the pinned qpdf (the corpus tests aside).
test-qpdf: _qpdf
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-structure --lib qpdf::tests -- --ignored --skip a_corpus_ --skip the_fixtures_are_written
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-lint --test qpdf -- --ignored --skip a_corpus_

# PDFium, the oracle of the readers that follow PDFium's rules: a
# development snapshot, PDFium as of chromium/8125, pdfium-binaries' Linux
# x86_64 build at that tag, at a pinned SHA-256 and, where `gh` can,
# its build provenance attested; kept in target/. To bump it, as
# oracle/README.md says: verify the new build's attestation, take its
# SHA-256, ask every case of it (`PDF_LINT_PDFIUM=... just test-pdfium`),
# read each answer that moved against PDFium's changes, then the tag and
# checksum here and `just pdfium-answers`.
_pdfium:
    #!/usr/bin/env bash
    set -euo pipefail
    build={{ pdfium_build }}
    if [ "$(rustc -vV | sed -n 's/^host: //p')" != x86_64-unknown-linux-gnu ]; then
        echo "PDFium chromium/$build is fetched for Linux x86_64 only: name another in PDF_LINT_PDFIUM" >&2
        exit 2
    fi
    dir=$PWD/target/pdfium-$build
    if [ ! -f "$dir/lib/libpdfium.so" ]; then
        tgz=pdfium-linux-x64.tgz
        mkdir -p target
        work=$(mktemp -d "$PWD/target/pdfium-fetch.XXXXXX")
        trap 'rm -rf "$work"' EXIT
        curl -fsSL --retry 3 -o "$work/$tgz" \
            "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/$build/$tgz"
        echo "e1c3ce6e270cb5e0c20816b379ad9f2c1309972697c505f508de92bf5e7d6659  $work/$tgz" \
            | sha256sum --check --quiet
        if gh attestation verify --help >/dev/null 2>&1 \
            && { [ -n "${GH_TOKEN:-}" ] || gh auth status >/dev/null 2>&1; }; then
            gh attestation verify "$work/$tgz" --repo bblanchon/pdfium-binaries >/dev/null
        else
            echo "no gh to verify its attestation: chromium/$build checked by its SHA-256 alone" >&2
        fi
        rm -rf "$dir"
        mkdir -p "$dir"
        tar -xzf "$work/$tgz" -C "$dir"
    fi

# The crates' PDFium cases: each generator writing the cases committed
# beside it, then every case asked of PDFium, its answer held to the one
# recorded at the pin, and each that moved listed. The library is the one
# PDF_LINT_PDFIUM names, or the pinned build. A token for gh, which the
# fetch alone uses, is kept from the process that loads the library.
test-pdfium:
    #!/usr/bin/env bash
    set -euo pipefail
    python3 -I -B oracle/pdfium.py cases {{ pdfium_sets }}
    if [ -n "${PDF_LINT_PDFIUM:-}" ]; then
        env -u GH_TOKEN -u GITHUB_TOKEN python3 -I -B oracle/pdfium.py check "$PDF_LINT_PDFIUM" {{ pdfium_sets }}
    else
        "{{ just_executable() }}" _pdfium
        env -u GH_TOKEN -u GITHUB_TOKEN python3 -I -B oracle/pdfium.py check \
            "$PWD/target/pdfium-{{ pdfium_build }}/lib/libpdfium.so" {{ pdfium_sets }} --build {{ pdfium_build }}
    fi

# Write the crates' PDFium cases anew from their generators, and their
# answers from the pinned build: when cases are added, and at a bump, once
# each answer that moved has been read.
pdfium-answers: _pdfium
    python3 -I -B oracle/pdfium.py cases --write {{ pdfium_sets }}
    env -u GH_TOKEN -u GITHUB_TOKEN python3 -I -B oracle/pdfium.py record \
        "$PWD/target/pdfium-{{ pdfium_build }}/lib/libpdfium.so" {{ pdfium_sets }} --build {{ pdfium_build }}

# Write the files qpdf makes from the tests' clean file anew, into
# crates/pdf-structure/tests/files, which the tests read: only when the
# clean file or the kinds change.
fixtures: _qpdf
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-structure --lib qpdf::tests::the_fixtures_are_written -- --ignored --exact

# Build the fuzz targets as cargo-fuzz does (`--cfg fuzzing`), on stable,
# so that what only they compile, the fuzz module and the test writer,
# can't break unseen; and lint the libraries so with no feature, as a crate
# that depends on one and fuzzes itself builds them. pdf-lint's fuzz body
# needs the test writers the fuzz crate turns on, so only that build has it.
fuzzing:
    RUSTFLAGS="--cfg fuzzing" cargo check --locked --manifest-path fuzz/Cargo.toml --target-dir target/fuzzing
    RUSTFLAGS="--cfg fuzzing" cargo clippy --locked --workspace --exclude pdf-lint --lib --target-dir target/fuzzing -- -D warnings

# Build the API docs, failing on warnings: with every feature on, then
# as they're published, then pdf-semantics without its clock, as pdf-lint
# takes it, which the workspace's unified features would hide.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --all-features
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps
    RUSTDOCFLAGS="-D warnings" cargo doc --locked -p pdf-semantics --no-deps --no-default-features

# Build this platform's release archive in dist/, and check it runs as
# it's unpacked, as the release workflow does each
# platform's. On Linux the binary is static, for musl, whose target must be
# installed (`rustup target add x86_64-unknown-linux-musl`).
dist:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(rustc -vV | sed -n 's/^host: //p')" in
    x86_64-unknown-linux-gnu) platform=linux-x86_64 target=x86_64-unknown-linux-musl ;;
    aarch64-unknown-linux-gnu) platform=linux-aarch64 target=aarch64-unknown-linux-musl ;;
    aarch64-apple-darwin) platform=macos-arm64 target=aarch64-apple-darwin ;;
    x86_64-apple-darwin) platform=macos-x86_64 target=x86_64-apple-darwin ;;
    x86_64-pc-windows-msvc) platform=windows-x86_64 target=x86_64-pc-windows-msvc ;;
    *) echo "no release archive for this platform" >&2; exit 2 ;;
    esac
    exe=
    if [ "$platform" = windows-x86_64 ]; then
        exe=.exe
        export RUSTFLAGS="-C target-feature=+crt-static"
    fi
    cargo build --release --locked -p pdf-lint --target "$target"
    release/dist.sh "$platform" "$target" "target/$target/release/pdf-lint$exe" dist

# Lint pdf-structure, its tests too, for Windows, whose reads of a file by
# position are code of its own, and whose file identity is same-file's,
# through winapi-util: the only build of that code; and its library for a
# target whose `usize` is 32 bits and which has no files
# (wasm32-unknown-unknown). Both targets must be installed (`rustup target
# add x86_64-pc-windows-gnu wasm32-unknown-unknown`); every other recipe
# builds for this platform alone.
structure-cross:
    cargo clippy --locked -p pdf-structure --all-targets --target x86_64-pc-windows-gnu -- -D warnings
    cargo clippy --locked -p pdf-structure --lib --target wasm32-unknown-unknown -- -D warnings

# Build the tokenizer for a target without std, which must be installed
# (`rustup target add thumbv6m-none-eabi`).
tokenizer-no-std:
    cargo build --locked -p pdf-tokenizer --target thumbv6m-none-eabi

# Build the tokenizer against core alone, built from source on nightly,
# whose rust-src must be installed (`rustup component add rust-src
# --toolchain nightly`): the target ships alloc beside core, so only this
# build shows the tokenizer needs neither std nor alloc.
tokenizer-core-only:
    cargo +nightly build --locked -p pdf-tokenizer --target thumbv6m-none-eabi -Z build-std=core

# Test the tokenizer on its oldest supported Rust, which must be installed.
tokenizer-msrv:
    cargo +{{ tokenizer_msrv }} test --locked -p pdf-tokenizer

# Test the filters on their oldest supported Rust, which must be installed,
# with both Flate backends.
filters-msrv:
    cargo +{{ filters_msrv }} test --locked -p pdf-filters --all-features

# Test pdf-crypt on its oldest supported Rust, which must be installed.
crypt-msrv:
    cargo +{{ crypt_msrv }} test --locked -p pdf-crypt

# Test pdf-objects on its oldest supported Rust, which must be installed.
objects-msrv:
    cargo +{{ objects_msrv }} test --locked -p pdf-objects

# Test pdf-structure on its oldest supported Rust, which must be installed.
structure-msrv:
    cargo +{{ structure_msrv }} test --locked -p pdf-structure

# Test pdf-content-streams on its oldest Rust, then with `image-ends` on
# that feature's, each of which must be installed.
content-msrv:
    cargo +{{ content_msrv }} test --locked -p pdf-content-streams
    cargo +{{ content_image_ends_msrv }} test --locked -p pdf-content-streams --features image-ends

# Test pdf-semantics on its oldest supported Rust, which must be installed.
semantics-msrv:
    cargo +{{ semantics_msrv }} test --locked -p pdf-semantics

# Test pdf-linearize on its oldest supported Rust, which must be installed.
linearize-msrv:
    cargo +{{ linearize_msrv }} test --locked -p pdf-linearize

# Test pdf-arlington on its oldest supported Rust, which must be installed.
arlington-msrv:
    cargo +{{ arlington_msrv }} test --locked -p pdf-arlington

# Fuzz a target for a while, on nightly with cargo-fuzz: `tokenize`, the
# tokenizer, by ISO 32000-2's rules and PDFium's; `filters`, the filters,
# decoding and encoding; `crypt`, pdf-crypt's security handler, new files'
# dictionaries and a password's forms; `objects`, pdf-objects' parser, by
# ISO 32000-2's rules and PDFium's, its writer on what it read, and text
# strings; `structure`, pdf-structure's reader; `write`, pdf-structure's writer, on
# what its reader read; `content`, pdf-content-streams' tokens, operations,
# faults, operator table, the balance, marked content and names over them,
# its writer, content written again and the calls, and its splices and
# lists written again; `semantics`, pdf-semantics' name and number trees,
# page tree, dates and XMP claims; `linearize`, pdf-linearize's plan of any
# graph; or `check`, the reader and the checker.
# `structure`, `write` and `check` begin from the seeds pdf-lint's fuzz module
# makes, `write`'s with the third byte its input has; `content`, `semantics` and
# `linearize` from those their own crates' fuzz modules make. The nightly is
# `fuzz_toolchain`.
# cargo-fuzz otherwise builds for the target it was itself built for,
# which may be musl, where the sanitizer can't run.
fuzz target="tokenize" seconds="60":
    #!/usr/bin/env bash
    set -euo pipefail
    host=$(rustc -vV | sed -n 's/^host: //p')
    case "{{ target }}" in
    tokenize)
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" tokenize -- -dict=fuzz/pdf.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    filters)
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" filters -- -max_total_time={{ seconds }} -timeout=10
        ;;
    crypt)
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" crypt -- -max_total_time={{ seconds }} -timeout=10 -len_control=0
        ;;
    objects)
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" objects -- -dict=fuzz/pdf.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    structure)
        PDF_LINT_FUZZ_SEEDS="$PWD/fuzz/seeds/check" cargo test --locked -q -p pdf-lint --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/structure
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" structure fuzz/corpus/structure fuzz/seeds/check -- -dict=fuzz/check.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    write)
        PDF_LINT_FUZZ_SEEDS="$PWD/fuzz/seeds/write" cargo test --locked -q -p pdf-lint --lib fuzz::tests::the_write_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/write
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" write fuzz/corpus/write fuzz/seeds/write -- -dict=fuzz/check.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    content)
        PDF_CONTENT_STREAMS_FUZZ_SEEDS="$PWD/fuzz/seeds/content" cargo test --locked -q -p pdf-content-streams --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/content
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" content fuzz/corpus/content fuzz/seeds/content -- -dict=fuzz/content.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    semantics)
        PDF_SEMANTICS_FUZZ_SEEDS="$PWD/fuzz/seeds/semantics" cargo test --locked -q -p pdf-semantics --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/semantics
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" semantics fuzz/corpus/semantics fuzz/seeds/semantics -- -dict=fuzz/semantics.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    linearize)
        PDF_LINEARIZE_FUZZ_SEEDS="$PWD/fuzz/seeds/linearize" cargo test --locked -q -p pdf-linearize --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/linearize
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" linearize fuzz/corpus/linearize fuzz/seeds/linearize -- -max_total_time={{ seconds }} -timeout=10
        ;;
    check)
        PDF_LINT_FUZZ_SEEDS="$PWD/fuzz/seeds/check" cargo test --locked -q -p pdf-lint --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/check
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" check fuzz/corpus/check fuzz/seeds/check -- -dict=fuzz/check.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    *)
        echo "just fuzz: tokenize, filters, crypt, objects, structure, write, content, semantics, linearize or check" >&2
        exit 2
        ;;
    esac

# Cut a fuzz target's corpus in fuzz/corpus down to the inputs that keep
# what it covers, as CI's scheduled fuzzing does before keeping it.
fuzz-cmin target="tokenize":
    cargo +{{ fuzz_toolchain }} fuzz cmin --target "$(rustc -vV | sed -n 's/^host: //p')" {{ target }}
