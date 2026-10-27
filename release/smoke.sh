#!/usr/bin/env bash
# Checks a release folder runs as it's unpacked: pdf-lint says what it is,
# and reads files another writer made by itself, one compressed into object
# streams and one encrypted (the tests' fixtures, which qpdf wrote); on
# Windows, without the Visual C++ runtime's DLL.
#
#   release/smoke.sh <folder>
set -euo pipefail

dir=${1:?a release folder}
files=$(cd "$(dirname "$0")/../crates/pdf-lint/tests/files" && pwd)

if [ -f "$dir/pdf-lint.exe" ] && grep -a -q -i 'vcruntime140' "$dir/pdf-lint.exe"; then
    echo "smoke.sh: pdf-lint.exe needs VCRUNTIME140.dll: build it with +crt-static" >&2
    exit 1
fi

version=$("$dir/pdf-lint" --version)
echo "$version"
case "$version" in
"pdf-lint "*"Arlington PDF Model "*) ;;
*)
    echo "smoke.sh: pdf-lint --version doesn't say what it is" >&2
    exit 1
    ;;
esac

# Clean, compressed; encrypted, its findings on qpdf's revision 6 in a PDF
# 1.7 file, exit 1, but read whole.
"$dir/pdf-lint" "$files/qpdf-compressed.pdf"
code=0
out=$("$dir/pdf-lint" "$files/qpdf-aes-256.pdf") || code=$?
echo "$out" | head -n 1
if [ "$code" -gt 1 ] || echo "$out" | grep -q unresolved; then
    echo "smoke.sh: the encrypted file doesn't read whole" >&2
    exit 1
fi
