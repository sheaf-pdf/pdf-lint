#!/usr/bin/env bash
# Builds the release archive for one platform: the binary, the README,
# changelog and license, and the NOTICE.txt of the Arlington model it
# embeds, with the cells it corrects in it and the rows it adds
# (pdf-arlington's model/corrections.tsv and model/additions); the
# notices of what's compiled in (below); checks it runs
# (release/smoke.sh); and packs it.
# The layout, pdf-lint-<version>-<platform>/pdf-lint[.exe] and a .sha256
# beside the archive, is what setup-pdf-lint unpacks: keep it.
#
#   release/dist.sh <platform> <target> <binary> <folder>
#
# <binary> is pdf-lint built for the Rust <target> by the rustc on the
# PATH, whose notices for the standard library go with it. Writes
# <folder>/pdf-lint-<version>-<platform>.tar.gz, a .zip for Windows, and
# a .sha256 beside it.
set -euo pipefail

platform=${1:?a platform}
target=${2:?a target}
binary=${3:?a binary}
out=${4:?a folder}
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
crate=$root/crates/pdf-lint
model=$root/crates/pdf-arlington/model

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$crate/Cargo.toml" | head -n 1)
name=pdf-lint-$version-$platform

mkdir -p "$out"
rm -rf "${out:?}/$name"
mkdir "$out/$name"
cp "$binary" "$out/$name/"
cp "$crate/README.md" "$crate/CHANGELOG.md" "$root/LICENSE-APACHE" "$out/$name/"
cp "$model/NOTICE.txt" "$out/$name/ARLINGTON-NOTICE.txt"
cp "$model/corrections.tsv" "$out/$name/ARLINGTON-CORRECTIONS.tsv"
cp -r "$model/additions" "$out/$name/ARLINGTON-ADDITIONS"

# Paths as bash takes them, on Windows too.
unix() {
    if command -v cygpath >/dev/null; then cygpath -u "$1"; else echo "$1"; fi
}
sum() {
    if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi
}
fail() {
    echo "dist.sh: $1" >&2
    exit 1
}
sysroot=$(unix "$(rustc --print sysroot)")

# The Rust standard library's notices, and its dependencies', from the
# toolchain (its rustc component).
rust=$sysroot/share/doc/rust/COPYRIGHT-library.html
[ -f "$rust" ] || fail "the toolchain has no $rust"
cp "$rust" "$out/$name/RUST-NOTICES.html"

# musl, linked in statically on Linux: its COPYRIGHT for the version the
# toolchain has (release/musl-<version>-COPYRIGHT, from its release
# tarball). A toolchain with another musl stops here until it's added.
musl=
case "$platform" in
linux-*)
    libc=$sysroot/lib/rustlib/$target/lib/self-contained/libc.a
    [ -f "$libc" ] || fail "the toolchain has no $libc"
    musl=$(tr '\0' '\n' <"$libc" | grep -m 1 -xE '1\.[0-9]+\.[0-9]+' || true)
    [ -f "$here/musl-$musl-COPYRIGHT" ] || fail "no release/musl-$musl-COPYRIGHT for $libc"
    cp "$here/musl-$musl-COPYRIGHT" "$out/$name/MUSL-COPYRIGHT"
    ;;
esac

# The crates compiled in for the target, as Cargo.lock has them, so the
# list can't drift: not a proc-macro crate, or what only it depends on,
# which runs while building and isn't compiled in. Then each one's
# licence files as its package has them, each text once.
cargo fetch --locked --target "$target" --quiet
registry=$(unix "${CARGO_HOME:-$HOME/.cargo}")/registry/src
crates=$(cargo tree --locked -e normal,no-proc-macro --target "$target" -p pdf-lint --prefix none \
    --format '{p}|{l}' | sed 's/ (\*)$//' | sort -u |
    grep -vE '^(pdf-lint|pdf-tokenizer|pdf-filters|pdf-crypt|pdf-objects|pdf-structure|pdf-arlington) v')
texts=$(mktemp)
table=
while IFS='|' read -r package licence; do
    case "$package" in
    *' ('*) fail "$package isn't from crates.io" ;;
    esac
    crate=${package% v*}
    v=${package##* v}
    dir=$(ls -d "$registry"/*/"$crate-$v" 2>/dev/null | head -n 1)
    [ -n "$dir" ] || fail "$crate $v's source isn't in $registry"
    found=
    for file in "$dir"/*; do
        case $(basename "$file" | tr '[:upper:]' '[:lower:]') in
        licen[cs]e* | copying* | copyright* | notice*)
            printf '%s\t%s\t%s\n' "$(sum "$file" | cut -d ' ' -f 1)" "$crate $v" "$file" >>"$texts"
            found=1
            ;;
        esac
    done
    [ -n "$found" ] || fail "$crate $v has no licence file"
    table=$table$(printf '  %-15s %-9s %s' "$crate" "$v" "$licence")$'\n'
done <<<"$crates"
{
    cat <<END
pdf-lint $version for $platform

What's compiled into the binary besides this workspace's own crates
(pdf-tokenizer, pdf-filters, pdf-crypt, pdf-objects and pdf-structure,
MIT OR Apache-2.0; pdf-arlington, Apache-2.0, with the Arlington model,
whose NOTICE.txt is ARLINGTON-NOTICE.txt):

- These crates from crates.io, each under the licence it names. Each
  offers the Apache License 2.0, whose text is LICENSE-APACHE beside
  this file; each package's licence files follow the list.

$table
- The Rust standard library, and what it's built from: RUST-NOTICES.html.
END
    if [ -n "$musl" ]; then
        echo "- musl $musl, the C library, linked in statically: MUSL-COPYRIGHT."
    fi
    sort "$texts" |
        awk -F '\t' -v OFS='\t' '
            $1 != hash { if (hash != "") print path, crates; hash = $1; path = $3; crates = $2; next }
            { crates = crates ", " $2 }
            END { if (hash != "") print path, crates }' |
        sort -t $'\t' -k 2 |
        while IFS=$'\t' read -r path crates; do
            printf '\n\n==== %s, of %s\n\n' "$(basename "$path")" "$crates"
            cat "$path"
        done
} >"$out/$name/THIRD-PARTY-NOTICES.txt"
rm -f "$texts"

"$here/smoke.sh" "$out/$name"

cd "$out"
case "$platform" in
windows-*)
    archive=$name.zip
    rm -f "$archive"
    7z a -tzip -bd "$archive" "$name" >/dev/null
    ;;
*)
    archive=$name.tar.gz
    tar -czf "$archive" "$name"
    ;;
esac
if command -v sha256sum >/dev/null; then
    sha256sum "$archive" >"$archive.sha256"
else
    shasum -a 256 "$archive" >"$archive.sha256"
fi
echo "dist.sh: $out/$archive"
