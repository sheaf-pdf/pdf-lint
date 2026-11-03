#!/usr/bin/env bash
# Builds the release archive for one platform: the binary, the README,
# changelog and license, and the NOTICE.txt of the Arlington model it
# embeds, with the cells it corrects in it and the rows it adds
# (pdf-arlington's model/corrections.tsv and model/additions); the
# notices of what's compiled in (below), Adobe's for the font metrics
# among them; checks it runs
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
    grep -vE '^(pdf-lint|pdf-tokenizer|pdf-filters|pdf-crypt|pdf-objects|pdf-structure|pdf-content-streams|pdf-color|pdf-fonts|pdf-semantics|pdf-arlington) v')

# pdf-fonts as pdf-lint's build compiles it, by the features it's built
# with: neither `programs`, whose code is in part FreeType's, under the
# FTL, and which brings in skrifa, nor `predefined-cmaps`, which brings in
# hayro-cmap's CMaps. Their notices aren't taken here, so a build that
# turns either on stops here until they are.
features=$(cargo tree --locked -e normal --target "$target" -p pdf-lint -i pdf-fonts --depth 0 --format '{f}') ||
    fail "pdf-fonts isn't in pdf-lint's build"
case ",$features," in
*,programs,* | *,predefined-cmaps,*)
    fail "pdf-lint's build turns on pdf-fonts' features $features, whose notices (FreeType's, skrifa's, hayro-cmap's) aren't taken" ;;
esac

# pdf-fonts' data, compiled in whole: Adobe's Core 14 font metrics, which
# go with the file Adobe distributes them with, as its paragraph, which
# each carries on its third line too, has it; and the Adobe Glyph List and
# the ITC Zapf Dingbats Glyph List, under the BSD 3-Clause license each
# one's header holds, the same text in both, said once. Each read as text,
# whatever line ends a checkout gave it.
assets=$root/crates/pdf-fonts/assets
cp "$assets/afm/MustRead.html" "$out/$name/ADOBE-AFM-NOTICE.html"
paragraph() {
    sed -n '3s/^Comment //p' "$1" | tr -d '\r'
}
afm=$(paragraph "$assets/afm/Helvetica.afm")
for file in "$assets"/afm/*.afm; do
    [ "$(paragraph "$file")" = "$afm" ] || fail "$file doesn't carry Adobe's paragraph on its third line"
done
grep -qF "$afm" "$assets/afm/MustRead.html" || fail "the metrics' MustRead.html doesn't hold their paragraph"
# A glyph list's licence: its header's lines between its first two rules.
licence() {
    awk '{ sub(/\r$/, "") } /^# -+$/ { if (++rules == 2) exit; next } rules == 1 { sub(/^# ?/, ""); print }' "$1"
}
glyphs=$(licence "$assets/glyphlist.txt")
[ -n "$glyphs" ] && [ "$glyphs" = "$(licence "$assets/zapfdingbats.txt")" ] ||
    fail "the glyph lists' headers don't hold one licence"
texts=$(mktemp)
table=
# The names' column as wide as the longest name, 15 at least.
width=$(sed 's/ v[^ ]*|.*$//' <<<"$crates" | awk '{ if (length > w) w = length } END { print (w > 15 ? w : 15) }')
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
    table=$table$(printf "  %-${width}s %-9s %s" "$crate" "$v" "$licence")$'\n'
done <<<"$crates"
{
    cat <<END
pdf-lint $version for $platform

What's compiled into the binary besides this workspace's own crates
(pdf-tokenizer, pdf-filters, pdf-crypt, pdf-objects, pdf-structure,
pdf-content-streams, pdf-color, pdf-fonts and pdf-semantics, MIT OR
Apache-2.0, pdf-fonts as it's compiled in here, without the code its
package's FTL is for, which only its programs feature compiles;
pdf-arlington, Apache-2.0, with the Arlington model, whose NOTICE.txt is
ARLINGTON-NOTICE.txt):

- These crates from crates.io, each under the licence it names, its
  package's licence files after the list; the Apache License 2.0, which
  most offer, is LICENSE-APACHE beside this file.

$table
- Adobe's Core 14 font metrics, in pdf-fonts, whole, with their
  copyright notices, on the terms of Adobe's paragraph, which goes with
  them as ADOBE-AFM-NOTICE.html, the file Adobe distributes them with:

$afm

- The Adobe Glyph List and the ITC Zapf Dingbats Glyph List, in
  pdf-fonts, under the BSD 3-Clause license, its text after the list.
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
    printf '\n\n==== %s\n\n%s\n' "The licence in glyphlist.txt's and zapfdingbats.txt's headers, of pdf-fonts" "$glyphs"
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
