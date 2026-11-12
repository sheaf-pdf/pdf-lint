# Development tasks. CI runs these same recipes, so a local `just ci` checks
# what CI checks, but pdf-structure's tests on Windows (CI's
# `structure-windows` job), which no recipe runs.

# The oldest Rust pdf-tokenizer, pdf-filters, pdf-crypt, pdf-objects,
# pdf-structure, pdf-content-streams, pdf-semantics, pdf-color, pdf-fonts,
# pdf-linearize and pdf-arlington support, from their Cargo.toml. Only the libraries promise one:
# the checker ships as a binary built with stable Rust.
tokenizer_msrv := `grep -m1 '^rust-version' crates/pdf-tokenizer/Cargo.toml | cut -d'"' -f2`
filters_msrv := `grep -m1 '^rust-version' crates/pdf-filters/Cargo.toml | cut -d'"' -f2`
crypt_msrv := `grep -m1 '^rust-version' crates/pdf-crypt/Cargo.toml | cut -d'"' -f2`
objects_msrv := `grep -m1 '^rust-version' crates/pdf-objects/Cargo.toml | cut -d'"' -f2`
structure_msrv := `grep -m1 '^rust-version' crates/pdf-structure/Cargo.toml | cut -d'"' -f2`
content_msrv := `grep -m1 '^rust-version' crates/pdf-content-streams/Cargo.toml | cut -d'"' -f2`
semantics_msrv := `grep -m1 '^rust-version' crates/pdf-semantics/Cargo.toml | cut -d'"' -f2`
color_msrv := `grep -m1 '^rust-version' crates/pdf-color/Cargo.toml | cut -d'"' -f2`
fonts_msrv := `grep -m1 '^rust-version' crates/pdf-fonts/Cargo.toml | cut -d'"' -f2`
linearize_msrv := `grep -m1 '^rust-version' crates/pdf-linearize/Cargo.toml | cut -d'"' -f2`
arlington_msrv := `grep -m1 '^rust-version' crates/pdf-arlington/Cargo.toml | cut -d'"' -f2`

# The nightly the fuzz targets build on: `nightly`, or the toolchain
# FUZZ_TOOLCHAIN names, as CI's fuzzing pins one.
fuzz_toolchain := env("FUZZ_TOOLCHAIN", "nightly")

# The PDFium build the readers that follow PDFium's rules are held to
# (`_pdfium`, oracle/README.md).
pdfium_build := "8125"

# The crates' PDFium cases, each a crate's tests/pdfium.
pdfium_sets := "crates/pdf-tokenizer/tests/pdfium crates/pdf-objects/tests/pdfium crates/pdf-structure/tests/pdfium crates/pdf-content-streams/tests/pdfium crates/pdf-semantics/tests/pdfium crates/pdf-color/tests/pdfium crates/pdf-fonts/tests/pdfium crates/pdf-linearize/tests/pdfium"

# List the recipes.
default:
    @just --list --unsorted

# Everything CI runs, but the `-semver` recipes, which need cargo-semver-checks
# installed, and pdf-structure's tests on Windows, which CI's
# `structure-windows` job runs.
ci: fmt clippy test test-qpdf test-pdfium doc fuzzing dist structure-cross tokenizer-no-std tokenizer-core-only tokenizer-package objects-package filters-package crypt-package color-package structure-package content-streams-package semantics-package fonts-package linearize-package tokenizer-msrv filters-msrv crypt-msrv objects-msrv structure-msrv content-msrv semantics-msrv color-msrv fonts-msrv fonts-cmaps-msrv linearize-msrv arlington-msrv

# Check formatting; `cargo fmt --all` fixes it.
fmt:
    cargo fmt --all -- --check

# Lint every crate, with its tests and binaries; then the libraries and the
# binary as they're built for use, without the features only their tests
# turn on; then the filters with their default alone, miniz_oxide, the
# build a dependent that names no feature gets, which the workspace's
# unified features hide (its other members turn zlib-rs on), and with
# zlib-rs their only Flate backend; then the content streams without
# `image-ends`, which pdf-lint turns on in every workspace build, and with
# it, `simd` and its spans (`tracing`), which nothing else turns on; then
# pdf-semantics without its clock, as pdf-lint takes it, and with its spans
# (`tracing`), which nothing else turns on; then pdf-linearize's engine,
# without its adapter, and with `timing`, which nothing else turns on, and
# the lopdf adapter's library alone, where pdf-linearize is that engine
# without `family`, which pdf-lint turns on in every workspace build; then
# pdf-fonts with Adobe's predefined CMaps and its spans (`tracing`), which
# nothing else turns on, and with no feature at all; then pdf-structure
# with its spans (`tracing`), which nothing else turns on.
clippy:
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo clippy --locked --workspace --lib --bins -- -D warnings
    cargo clippy --locked -p pdf-filters --all-targets -- -D warnings
    cargo clippy --locked -p pdf-filters --no-default-features --features zlib-rs --all-targets -- -D warnings
    cargo clippy --locked -p pdf-content-streams --all-targets -- -D warnings
    cargo clippy --locked -p pdf-content-streams --features simd,image-ends,tracing --all-targets -- -D warnings
    cargo clippy --locked -p pdf-semantics --no-default-features --all-targets -- -D warnings
    cargo clippy --locked -p pdf-semantics --features tracing --all-targets -- -D warnings
    cargo clippy --locked -p pdf-linearize --no-default-features --all-targets -- -D warnings
    cargo clippy --locked -p pdf-linearize --features timing --all-targets -- -D warnings
    cargo clippy --locked -p pdf-linearize-lopdf --lib -- -D warnings
    cargo clippy --locked -p pdf-fonts --features predefined-cmaps,tracing --all-targets -- -D warnings
    cargo clippy --locked -p pdf-fonts --no-default-features --all-targets -- -D warnings
    cargo clippy --locked -p pdf-structure --features tracing --all-targets -- -D warnings

# Unit tests and doctests: the workspace's, where the filters have both
# Flate backends, as pdf-structure's tests turn zlib-rs on, and the content
# streams `image-ends`, as pdf-lint turns it on; then the filters' with
# their default alone, miniz_oxide, the build a dependent that names no
# feature gets, which the workspace's unified features hide, and with
# zlib-rs alone; then the content streams' without `image-ends`, and with
# it, `simd`, the dependencies' SIMD, and its spans, which nothing else
# turns on and only its tests read; then pdf-semantics' without its clock,
# and with its spans, which only its tests read; then pdf-linearize's
# engine, without its adapter; then pdf-fonts' with
# Adobe's predefined CMaps and its spans, which only its tests read, and
# with no feature at all; then pdf-structure's with its spans, which only
# those tests read.
test:
    cargo test --locked --workspace
    cargo test --locked -p pdf-filters
    cargo test --locked -p pdf-filters --no-default-features --features zlib-rs
    cargo test --locked -p pdf-content-streams
    cargo test --locked -p pdf-content-streams --features simd,image-ends,tracing
    cargo test --locked -p pdf-semantics --no-default-features
    cargo test --locked -p pdf-semantics --features tracing
    cargo test --locked -p pdf-linearize --no-default-features
    cargo test --locked -p pdf-fonts --features predefined-cmaps,tracing
    cargo test --locked -p pdf-fonts --no-default-features
    cargo test --locked -p pdf-structure --features tracing

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

# The tests of qpdf as the tests run it, pdf-fixtures'; then the tests that
# set the reader, and what the writer writes, beside qpdf, pdf-structure's,
# then the page trees pdf-semantics writes, then the checker's in pdf-lint,
# then what pdf-linearize lays out, and its lopdf adapter, which `just test`
# leaves out as ignored, against the pinned qpdf (the corpus tests aside).
test-qpdf: _qpdf
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-fixtures -- --ignored
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-structure --lib qpdf::tests -- --ignored --skip a_corpus_ --skip the_fixtures_are_written
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-semantics --test qpdf -- --ignored
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-lint --test qpdf -- --ignored --skip a_corpus_
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-linearize --test qpdf -- --ignored
    PDF_LINT_QPDF="$PWD/target/qpdf-12.4.2/bin/qpdf" cargo test --locked -p pdf-linearize-lopdf --test qpdf -- --ignored

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
# so that what only they compile, the fuzz modules, can't break unseen; and
# lint the libraries so with no feature, as a crate that depends on one and
# fuzzes itself builds them, pdf-lint's among them: the seeds that use the
# tests' file writer are compiled for the tests alone. Then pdf-crypt,
# pdf-structure and pdf-linearize alone, without the test writer, which
# pdf-fixtures turns on in every workspace build: what of their fuzz bodies
# needs it is compiled with it alone.
fuzzing:
    RUSTFLAGS="--cfg fuzzing" cargo check --locked --manifest-path fuzz/Cargo.toml --target-dir target/fuzzing
    RUSTFLAGS="--cfg fuzzing" cargo clippy --locked --workspace --lib --target-dir target/fuzzing -- -D warnings
    RUSTFLAGS="--cfg fuzzing" cargo clippy --locked -p pdf-crypt -p pdf-structure -p pdf-linearize --lib --target-dir target/fuzzing -- -D warnings

# Build the API docs, failing on warnings: with every feature on, then
# as they're published, then pdf-content-streams without `image-ends`,
# which pdf-lint turns on, and pdf-semantics without its clock, as pdf-lint
# takes it, which the workspace's unified features would hide, then
# pdf-linearize's engine, without its adapter, then pdf-fonts with no feature.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --all-features
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps
    RUSTDOCFLAGS="-D warnings" cargo doc --locked -p pdf-content-streams --no-deps
    RUSTDOCFLAGS="-D warnings" cargo doc --locked -p pdf-semantics --no-deps --no-default-features
    RUSTDOCFLAGS="-D warnings" cargo doc --locked -p pdf-linearize --no-deps --no-default-features
    RUSTDOCFLAGS="-D warnings" cargo doc --locked -p pdf-fonts --no-deps --no-default-features

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
# (wasm32-unknown-unknown), where getrandom has no generator unless a build
# picks one: its `unsupported` backend, which refuses every draw at run
# time. Both targets must be installed (`rustup target add
# x86_64-pc-windows-gnu wasm32-unknown-unknown`); every other recipe builds
# for this platform alone.
structure-cross:
    cargo clippy --locked -p pdf-structure --all-targets --target x86_64-pc-windows-gnu -- -D warnings
    RUSTFLAGS='--cfg getrandom_backend="unsupported"' cargo clippy --locked -p pdf-structure --lib --target wasm32-unknown-unknown -- -D warnings

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

# Package a crate as a publish would, in a target directory of its own,
# target/<the crate's name without `pdf-`>-package, made fresh each time (a
# stale one passes or fails falsely), and check what the package holds: its
# sources, README, CHANGELOG, manifest and lock, and nothing else, no
# benchmark among them, but the files a crate's own list below names, each of
# which it must hold. The benchmarks' target is declared in the manifest
# while `benches/` is outside `include`, so cargo drops it from the
# packaged manifest with a warning, which is expected, as is one for each
# test outside it; the check reads that manifest. The dry run asks the
# registry, so it needs the network.
_package crate *beside:
    #!/usr/bin/env bash
    set -euo pipefail
    crate={{ crate }}
    dir=$PWD/target/${crate#pdf-}-package
    rm -rf "$dir"
    # A crate of the family the package depends on at a version not yet on
    # crates.io (an alpha, named exactly) is packaged beside it, as a publish
    # in order would: cargo verifies the package against that one, through a
    # registry of its own, instead of asking crates.io for it.
    packages=(-p "$crate")
    for b in {{ beside }}; do packages+=(-p "$b"); done
    CARGO_TARGET_DIR="$dir" cargo publish --dry-run --locked "${packages[@]}"
    CARGO_TARGET_DIR="$dir" cargo package --list --locked -p "$crate" > "$dir/files.txt"
    # What a crate's package holds beside those, file by file, so that a file
    # added beside them still fails: pdf-fonts' data, each file with its
    # terms (Adobe's metrics and their notice, the two glyph lists), and the
    # FreeType Project License.
    extra=()
    case "$crate" in
    pdf-fonts)
        extra=(
            assets/afm/Courier.afm assets/afm/Courier-Bold.afm
            assets/afm/Courier-BoldOblique.afm assets/afm/Courier-Oblique.afm
            assets/afm/Helvetica.afm assets/afm/Helvetica-Bold.afm
            assets/afm/Helvetica-BoldOblique.afm assets/afm/Helvetica-Oblique.afm
            assets/afm/Symbol.afm assets/afm/Times-Bold.afm
            assets/afm/Times-BoldItalic.afm assets/afm/Times-Italic.afm
            assets/afm/Times-Roman.afm assets/afm/ZapfDingbats.afm
            assets/afm/MustRead.html assets/glyphlist.txt assets/zapfdingbats.txt
            FTL.TXT
        ) ;;
    esac
    : > "$dir/extra.txt"
    if [ "${#extra[@]}" -gt 0 ]; then printf '%s\n' "${extra[@]}" > "$dir/extra.txt"; fi
    if grep -v -E '^(src/.*\.rs|README\.md|CHANGELOG\.md|Cargo\.toml|Cargo\.toml\.orig|Cargo\.lock|\.cargo_vcs_info\.json)$' "$dir/files.txt" | grep -v -x -F -f "$dir/extra.txt"; then
        echo "the package holds more than its sources, README, CHANGELOG, manifest and lock, and the files its list names (above)" >&2
        exit 1
    fi
    if grep -v -x -F -f "$dir/files.txt" "$dir/extra.txt" | grep .; then
        echo "the package lacks files its list names (above)" >&2
        exit 1
    fi
    manifest=("$dir"/package/"$crate"-*/Cargo.toml)
    if [ "${#manifest[@]}" -ne 1 ] || [ ! -f "${manifest[0]}" ]; then
        echo "no packaged manifest under $dir/package to read" >&2
        exit 1
    fi
    if grep -q '^\[\[bench\]\]' "${manifest[0]}"; then
        echo "the packaged manifest declares a benchmark the package doesn't hold" >&2
        exit 1
    fi

# Package the tokenizer as a publish would; `_package` says what's checked.
tokenizer-package: (_package "pdf-tokenizer")

# Package pdf-objects as a publish would; `_package` says what's checked.
objects-package: (_package "pdf-objects")

# Package the filters as a publish would; `_package` says what's checked.
filters-package: (_package "pdf-filters")

# Package pdf-crypt as a publish would; `_package` says what's checked.
crypt-package: (_package "pdf-crypt")

# Package pdf-color as a publish would, pdf-objects beside it while the
# version it names isn't published; `_package` says what's checked.
color-package: (_package "pdf-color" "pdf-objects")

# Package pdf-structure as a publish would, pdf-crypt and pdf-filters beside
# it while the versions it names aren't published; `_package` says what's
# checked.
structure-package: (_package "pdf-structure" "pdf-crypt" "pdf-filters")

# Package the content streams as a publish would, pdf-filters beside them
# while the version they name isn't published; `_package` says what's
# checked.
content-streams-package: (_package "pdf-content-streams" "pdf-filters")

# Package pdf-semantics as a publish would; `_package` says what's checked.
semantics-package: (_package "pdf-semantics")

# Package pdf-fonts as a publish would, pdf-content-streams and pdf-filters
# beside it while the versions it and they name aren't published; `_package`
# says what's checked, and lists the data the package holds with its terms.
fonts-package: (_package "pdf-fonts" "pdf-content-streams" "pdf-filters")

# Package pdf-linearize as a publish would, pdf-structure, pdf-semantics and
# pdf-filters beside it while the versions it and they name aren't
# published; `_package` says what's checked.
linearize-package: (_package "pdf-linearize" "pdf-structure" "pdf-semantics" "pdf-filters")

# Check the tokenizer's public API against the version published on
# crates.io, with cargo-semver-checks, which must be installed (CI installs
# a pinned build): a break the version doesn't allow fails.
tokenizer-semver:
    cargo semver-checks check-release -p pdf-tokenizer

# An alpha's public API against the version crates.io has, with
# cargo-semver-checks (installed as `tokenizer-semver` needs it). While the
# manifest's version is a published one, nothing of its API may move: a
# change that would need a new version fails, as `--release-type patch` holds
# it (by itself the tool takes a pre-release to allow anything, and checks
# nothing of one). A version not on crates.io yet is the next alpha, which
# may change anything, its CHANGELOG saying what: nothing holds it here.
_semver crate:
    #!/usr/bin/env bash
    set -euo pipefail
    crate={{ crate }}
    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "crates/$crate/Cargo.toml" | head -n 1)
    index=$(curl -fs "https://index.crates.io/${crate:0:2}/${crate:2:2}/$crate")
    if grep -qF "\"vers\":\"$version\"" <<< "$index"; then
        cargo semver-checks check-release -p "$crate" --baseline-version "$version" --release-type patch
    else
        echo "$crate $version isn't on crates.io yet: the next alpha, whose CHANGELOG says what changed"
    fi

# Check pdf-objects' public API against its version on crates.io; `_semver`
# says how an alpha is held.
objects-semver: (_semver "pdf-objects")

# Check the filters' public API against their version on crates.io; `_semver`
# says how an alpha is held.
filters-semver: (_semver "pdf-filters")

# Check pdf-crypt's public API against its version on crates.io; `_semver`
# says how an alpha is held.
crypt-semver: (_semver "pdf-crypt")

# Check pdf-color's public API against its version on crates.io; `_semver`
# says how an alpha is held.
color-semver: (_semver "pdf-color")

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

# Test pdf-structure on its oldest supported Rust, which must be installed,
# without its spans and with them.
structure-msrv:
    cargo +{{ structure_msrv }} test --locked -p pdf-structure
    cargo +{{ structure_msrv }} test --locked -p pdf-structure --features tracing

# Test pdf-content-streams on its oldest Rust, then with `image-ends` on
# that feature's, and with it, `simd` and `tracing`, each of which must be
# installed.
content-msrv:
    cargo +{{ content_msrv }} test --locked -p pdf-content-streams
    cargo +{{ content_msrv }} test --locked -p pdf-content-streams --features image-ends
    cargo +{{ content_msrv }} test --locked -p pdf-content-streams --features image-ends,simd,tracing

# Test pdf-semantics on its oldest supported Rust, which must be installed,
# as it's published, without its clock and with its spans.
semantics-msrv:
    cargo +{{ semantics_msrv }} test --locked -p pdf-semantics
    cargo +{{ semantics_msrv }} test --locked -p pdf-semantics --no-default-features
    cargo +{{ semantics_msrv }} test --locked -p pdf-semantics --features tracing

# Test pdf-color on its oldest supported Rust, which must be installed.
color-msrv:
    cargo +{{ color_msrv }} test --locked -p pdf-color

# Test pdf-fonts on its oldest supported Rust, as it's published, skrifa among it,
# with no feature, and with its spans, which must be installed.
fonts-msrv:
    cargo +{{ fonts_msrv }} test --locked -p pdf-fonts
    cargo +{{ fonts_msrv }} test --locked -p pdf-fonts --no-default-features
    cargo +{{ fonts_msrv }} test --locked -p pdf-fonts --features tracing

# Test pdf-fonts with `predefined-cmaps` on that feature's oldest Rust, which must
# be installed.
fonts-cmaps-msrv:
    cargo +{{ fonts_msrv }} test --locked -p pdf-fonts --features predefined-cmaps

# Test pdf-linearize on its oldest supported Rust, which must be installed,
# with its adapter and without.
linearize-msrv:
    cargo +{{ linearize_msrv }} test --locked -p pdf-linearize
    cargo +{{ linearize_msrv }} test --locked -p pdf-linearize --no-default-features

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
# its writer, content written again and the calls, its splices and lists
# written again, and an image's color space counted alike in its dictionary
# and in the resources; `semantics`, pdf-semantics' name and number trees,
# page tree, dates and XMP claims, each read and written, and a file's packet
# read through its streams; `color`, pdf-color's count of a space's
# components, its typed spaces and what a resources dictionary names; `fonts`,
# pdf-fonts' scan of a Type 1 program, under any caps, its reading of names, and its
# reading of a font, its program among them, on any parts and in a file;
# `linearize`, pdf-linearize's plan of any graph, what's assembled from it, hint tables from any bytes, any facts of a file,
# and any bytes as a file, laid out, read back and checked; or `check`, the reader
# and the checker.
# `structure`, `write` and `check` begin from the seeds pdf-lint's fuzz module
# makes, `write`'s with the third byte its input has; `content`, `semantics`,
# `color`, `fonts` and `linearize` from those their own crates' fuzz modules make. The
# nightly is `fuzz_toolchain`. A third argument names features of fuzz/'s
# to build the targets with: `simd`, the content streams' dependencies'
# SIMD, which `content` reaches (`just fuzz content 300 simd`); and
# `predefined-cmaps`, Adobe's predefined CMaps from pdf-fonts' bundle,
# which `fonts` reaches (`just fuzz fonts 300 predefined-cmaps`), and which
# pdf-lint's binary, as `check` builds it, is without.
# cargo-fuzz otherwise builds for the target it was itself built for,
# which may be musl, where the sanitizer can't run.
fuzz target="tokenize" seconds="60" features="":
    #!/usr/bin/env bash
    set -euo pipefail
    host=$(rustc -vV | sed -n 's/^host: //p')
    built=()
    if [ -n "{{ features }}" ]; then built=(--features "{{ features }}"); fi
    case "{{ target }}" in
    tokenize)
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" tokenize -- -dict=fuzz/pdf.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    filters)
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" filters -- -max_total_time={{ seconds }} -timeout=10
        ;;
    crypt)
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" crypt -- -max_total_time={{ seconds }} -timeout=10 -len_control=0
        ;;
    objects)
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" objects -- -dict=fuzz/pdf.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    structure)
        PDF_LINT_FUZZ_SEEDS="$PWD/fuzz/seeds/check" cargo test --locked -q -p pdf-lint --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/structure
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" structure fuzz/corpus/structure fuzz/seeds/check -- -dict=fuzz/check.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    write)
        PDF_LINT_FUZZ_SEEDS="$PWD/fuzz/seeds/write" cargo test --locked -q -p pdf-lint --lib fuzz::tests::the_write_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/write
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" write fuzz/corpus/write fuzz/seeds/write -- -dict=fuzz/check.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    content)
        PDF_CONTENT_STREAMS_FUZZ_SEEDS="$PWD/fuzz/seeds/content" cargo test --locked -q -p pdf-content-streams --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/content
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" content fuzz/corpus/content fuzz/seeds/content -- -dict=fuzz/content.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    semantics)
        PDF_SEMANTICS_FUZZ_SEEDS="$PWD/fuzz/seeds/semantics" cargo test --locked -q -p pdf-semantics --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/semantics
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" semantics fuzz/corpus/semantics fuzz/seeds/semantics -- -dict=fuzz/semantics.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    color)
        PDF_COLOR_FUZZ_SEEDS="$PWD/fuzz/seeds/color" cargo test --locked -q -p pdf-color --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/color
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" color fuzz/corpus/color fuzz/seeds/color -- -dict=fuzz/color.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    fonts)
        PDF_FONTS_FUZZ_SEEDS="$PWD/fuzz/seeds/fonts" cargo test --locked -q -p pdf-fonts --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/fonts
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" fonts fuzz/corpus/fonts fuzz/seeds/fonts -- -dict=fuzz/fonts.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    linearize)
        PDF_LINEARIZE_FUZZ_SEEDS="$PWD/fuzz/seeds/linearize" cargo test --locked -q -p pdf-linearize --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/linearize
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" linearize fuzz/corpus/linearize fuzz/seeds/linearize -- -dict=fuzz/linearize.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    check)
        PDF_LINT_FUZZ_SEEDS="$PWD/fuzz/seeds/check" cargo test --locked -q -p pdf-lint --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/check
        cargo +{{ fuzz_toolchain }} fuzz run --target "$host" "${built[@]}" check fuzz/corpus/check fuzz/seeds/check -- -dict=fuzz/check.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    *)
        echo "just fuzz: tokenize, filters, crypt, objects, structure, write, content, semantics, color, fonts, linearize or check" >&2
        exit 2
        ;;
    esac

# Cut a fuzz target's corpus in fuzz/corpus down to the inputs that keep
# what it covers, as CI's scheduled fuzzing does before keeping it.
fuzz-cmin target="tokenize":
    cargo +{{ fuzz_toolchain }} fuzz cmin --target "$(rustc -vV | sed -n 's/^host: //p')" {{ target }}
