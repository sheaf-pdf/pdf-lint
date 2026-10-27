#!/usr/bin/env bash
# Checks a release folder runs as it's unpacked: pdf-lint runs, and reads
# a file whose objects are in an object stream by itself.
#
#   release/smoke.sh <folder>
set -euo pipefail

dir=${1:?a release folder}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

version=$("$dir/pdf-lint" --version)
echo "$version"
case "$version" in
"pdf-lint "*"Arlington PDF Model "*) ;;
*)
    echo "smoke.sh: pdf-lint --version doesn't say what it is" >&2
    exit 1
    ;;
esac

# A file whose catalog and page tree are in an object stream, written by
# the bundled qpdf, which pdf-lint, finding no qpdf, decodes itself.
"$dir/qpdf/bin/qpdf" --empty --object-streams=generate "$work/compressed.pdf"
PDF_LINT_QPDF=/nonexistent/qpdf "$dir/pdf-lint" "$work/compressed.pdf"
