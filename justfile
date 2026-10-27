# Development tasks. CI runs these same recipes, so a local `just ci` checks
# what CI checks.

# The oldest Rust pdf-tokenizer, pdf-filters and pdf-crypt support, from their
# Cargo.toml. Only the libraries promise one: the checker ships as a binary
# built with stable Rust.
tokenizer_msrv := `grep -m1 '^rust-version' crates/pdf-tokenizer/Cargo.toml | cut -d'"' -f2`
filters_msrv := `grep -m1 '^rust-version' crates/pdf-filters/Cargo.toml | cut -d'"' -f2`
crypt_msrv := `grep -m1 '^rust-version' crates/pdf-crypt/Cargo.toml | cut -d'"' -f2`

# List the recipes.
default:
    @just --list --unsorted

# Everything CI runs.
ci: fmt clippy test test-oldest-qpdf doc dist tokenizer-no-std tokenizer-msrv filters-msrv crypt-msrv

# Check formatting; `cargo fmt --all` fixes it.
fmt:
    cargo fmt --all -- --check

# Lint every crate, with its tests and binaries.
clippy:
    cargo clippy --locked --workspace --all-targets -- -D warnings

# Unit tests and doctests.
test:
    cargo test --locked --workspace

# The checker's tests against the oldest qpdf it decodes with (OLDEST in
# crates/pdf-lint/src/qpdf.rs): qpdf's official Linux x86_64 zip, at a
# pinned SHA-256, kept in target/. What works only with a newer qpdf fails
# here.
test-oldest-qpdf:
    #!/usr/bin/env bash
    set -euo pipefail
    version=11.9.0
    oldest=$(sed -n 's/^pub const OLDEST: &str = "\(.*\)";$/\1/p' crates/pdf-lint/src/qpdf.rs)
    if [ "${version%.*}" != "$oldest" ]; then
        echo "OLDEST is $oldest, but this tests qpdf $version" >&2
        exit 1
    fi
    if [ "$(rustc -vV | sed -n 's/^host: //p')" != x86_64-unknown-linux-gnu ]; then
        echo "qpdf $version is fetched for Linux x86_64 only" >&2
        exit 2
    fi
    dir=$PWD/target/qpdf-$version
    if [ ! -x "$dir/bin/qpdf" ]; then
        zip=qpdf-$version-bin-linux-x86_64.zip
        work=$(mktemp -d)
        trap 'rm -rf "$work"' EXIT
        curl -fsSL --retry 3 -o "$work/$zip" \
            "https://github.com/qpdf/qpdf/releases/download/v$version/$zip"
        # From the release's qpdf-11.9.0.sha256.
        echo "be5eeaf0032a86b573680cb12eb8499b74172d0af1b23a7d0a3648b01708569d  $work/$zip" \
            | sha256sum --check --quiet
        rm -rf "$dir"
        mkdir -p "$dir"
        unzip -q "$work/$zip" -d "$dir"
    fi
    PDF_LINT_QPDF="$dir/bin/qpdf" cargo test --locked -p pdf-lint

# Build the API docs, failing on warnings.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps

# Build this platform's release archive in dist/, with the qpdf it bundles,
# and check it runs as it's unpacked, as the release workflow does each
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
    cargo build --release --locked -p pdf-lint --target "$target"
    release/dist.sh "$platform" "target/$target/release/pdf-lint$(rustc --print cfg | grep -q 'target_os="windows"' && echo .exe)" dist

# Build the tokenizer for a target with neither std nor alloc, which must
# be installed (`rustup target add thumbv6m-none-eabi`): it needs only core.
tokenizer-no-std:
    cargo build --locked -p pdf-tokenizer --target thumbv6m-none-eabi

# Test the tokenizer on its oldest supported Rust, which must be installed.
tokenizer-msrv:
    cargo +{{ tokenizer_msrv }} test --locked -p pdf-tokenizer

# Test the filters on their oldest supported Rust, which must be installed.
filters-msrv:
    cargo +{{ filters_msrv }} test --locked -p pdf-filters

# Test pdf-crypt on its oldest supported Rust, which must be installed.
crypt-msrv:
    cargo +{{ crypt_msrv }} test --locked -p pdf-crypt

# Fuzz a target for a while, on nightly with cargo-fuzz: `tokenize`, the
# tokenizer; `filters`, the filters; `primitives`, pdf-crypt's beside
# RustCrypto's; or `check`, the reader and the checker, which begins from
# the seeds pdf-lint's fuzz module makes. cargo-fuzz otherwise builds for the
# target it was itself built for, which may be musl, where the sanitizer
# can't run.
fuzz target="tokenize" seconds="60":
    #!/usr/bin/env bash
    set -euo pipefail
    host=$(rustc -vV | sed -n 's/^host: //p')
    case "{{ target }}" in
    tokenize)
        cargo +nightly fuzz run --target "$host" tokenize -- -dict=fuzz/pdf.dict -max_total_time={{ seconds }}
        ;;
    filters)
        cargo +nightly fuzz run --target "$host" filters -- -max_total_time={{ seconds }} -timeout=10
        ;;
    primitives)
        cargo +nightly fuzz run --target "$host" primitives -- -max_total_time={{ seconds }} -timeout=10
        ;;
    check)
        PDF_LINT_FUZZ_SEEDS="$PWD/fuzz/seeds/check" cargo test --locked -q -p pdf-lint --lib fuzz::tests::the_seeds_are_written -- --ignored --exact
        mkdir -p fuzz/corpus/check
        cargo +nightly fuzz run --target "$host" check fuzz/corpus/check fuzz/seeds/check -- -dict=fuzz/check.dict -max_total_time={{ seconds }} -timeout=10
        ;;
    *)
        echo "just fuzz: tokenize, filters, primitives or check" >&2
        exit 2
        ;;
    esac
