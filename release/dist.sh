#!/usr/bin/env bash
# Builds the release archive for one platform: the binary, the README,
# changelog and license, and the NOTICE.txt of the Arlington model it
# embeds, with the cells it corrects in it and the rows it adds
# (model/corrections.tsv, model/additions); checks it runs
# (release/smoke.sh); and packs it. The layout, pdf-lint-<version>-
# <platform>/pdf-lint[.exe] and a .sha256 beside the archive, is what
# setup-pdf-lint unpacks: keep it.
#
#   release/dist.sh <platform> <binary> <folder>
#
# Writes <folder>/pdf-lint-<version>-<platform>.tar.gz, a .zip for
# Windows, and a .sha256 beside it.
set -euo pipefail

platform=${1:?a platform}
binary=${2:?a binary}
out=${3:?a folder}
here=$(cd "$(dirname "$0")" && pwd)
crate=$(dirname "$here")/crates/pdf-lint

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$crate/Cargo.toml" | head -n 1)
name=pdf-lint-$version-$platform

mkdir -p "$out"
rm -rf "${out:?}/$name"
mkdir "$out/$name"
cp "$binary" "$out/$name/"
cp "$crate/README.md" "$crate/CHANGELOG.md" "$crate/LICENSE-APACHE" "$out/$name/"
cp "$crate/model/NOTICE.txt" "$out/$name/ARLINGTON-NOTICE.txt"
cp "$crate/model/corrections.tsv" "$out/$name/ARLINGTON-CORRECTIONS.tsv"
cp -r "$crate/model/additions" "$out/$name/ARLINGTON-ADDITIONS"
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
