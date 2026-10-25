#!/usr/bin/env bash
# Lays out the qpdf a release bundles, for one platform, in a folder:
# bin/qpdf and the libraries it loads, from qpdf's official zip at a pinned
# version and SHA-256, and the notices and licenses that go with them.
#
#   release/qpdf.sh <platform> <folder>
#
# <platform> is linux-x86_64, linux-aarch64, macos-arm64, macos-x86_64 or
# windows-x86_64. The release workflow bundles <folder> as qpdf/ beside the
# binary; CI runs the tests against <folder>/bin/qpdf.
set -euo pipefail

version=12.4.2

platform=${1:?a platform}
out=${2:?a folder}
here=$(cd "$(dirname "$0")" && pwd)

# From the release's qpdf-12.4.2.sha256, which matches GitHub's digests.
case "$platform" in
linux-x86_64)
    zip=qpdf-$version-bin-linux-x86_64.zip
    sha=db367d897829f22c4198ce1094143c9d467bd6ee7dfabc44ba6f02056b24f8b1
    ;;
linux-aarch64)
    zip=qpdf-$version-bin-linux-aarch64.zip
    sha=8fd9d009eb0838398180a603f2cf76531e5c68a4ecc8847598e0a1ccb1d3b600
    ;;
macos-arm64)
    zip=qpdf-$version-bin-macos-arm64.zip
    sha=62e46987a30ea167cbc530ccb22690aec3d8c812ed09979a941bcee92e504b79
    ;;
macos-x86_64)
    zip=qpdf-$version-bin-macos-x86_64.zip
    sha=dd3b01f4414d198529bb0f13bba59c2f016a328bb3506695087a96fb80fc1481
    ;;
windows-x86_64)
    zip=qpdf-$version-msvc64.zip
    sha=db87077e683630c1217e0e8f9a20a9749d952ab676e881c3689187763a5de25d
    ;;
*)
    echo "qpdf.sh: no qpdf for platform '$platform'" >&2
    exit 2
    ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl -fsSL --retry 3 -o "$work/$zip" \
    "https://github.com/qpdf/qpdf/releases/download/v$version/$zip"
if command -v sha256sum >/dev/null; then
    got=$(sha256sum "$work/$zip" | cut -d' ' -f1)
else
    got=$(shasum -a 256 "$work/$zip" | cut -d' ' -f1)
fi
if [ "$got" != "$sha" ]; then
    echo "qpdf.sh: $zip has SHA-256 $got, not the pinned $sha" >&2
    exit 1
fi

mkdir -p "$work/zip"
if command -v unzip >/dev/null; then
    unzip -q "$work/$zip" -d "$work/zip"
else
    7z x -bd -o"$work/zip" "$work/$zip" >/dev/null
fi
# The Windows zip has its files in a folder of its own.
from="$work/zip"
if [ -d "$from/qpdf-$version-msvc64" ]; then
    from="$from/qpdf-$version-msvc64"
fi

rm -rf "$out"
mkdir -p "$out/bin"
case "$platform" in
linux-*)
    cp "$from/bin/qpdf" "$out/bin/"
    mkdir -p "$out/lib"
    # Each library under the name it's loaded by; libqpdf's is a link in
    # the zip, which an archive might not keep.
    for lib in "$from"/lib/*.so.*; do
        [ -L "$lib" ] || cp "$lib" "$out/lib/"
    done
    mv "$out/lib/libqpdf.so.30."* "$out/lib/libqpdf.so.30"
    mkdir -p "$out/linux"
    cp "$here"/qpdf/linux/* "$out/linux/"
    ;;
macos-*)
    cp "$from/bin/qpdf" "$out/bin/"
    mkdir -p "$out/lib"
    cp "$from"/lib/libqpdf.30.*.dylib "$out/lib/libqpdf.30.dylib"
    cp "$here/qpdf/JPEG.md" "$here/qpdf/linux/libjpeg-turbo.copyright" "$out/"
    ;;
windows-*)
    cp "$from/bin/qpdf.exe" "$from"/bin/*.dll "$out/bin/"
    cp "$here/qpdf/JPEG.md" "$here/qpdf/linux/libjpeg-turbo.copyright" "$out/"
    ;;
esac
cp "$here/qpdf/NOTICE.md" "$here/qpdf/LICENSE.txt" "$here/qpdf/QPDF-NOTICE.md" "$out/"
echo "qpdf.sh: qpdf $version for $platform in $out"
