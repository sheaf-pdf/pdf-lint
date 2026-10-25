#!/usr/bin/env bash
# Checks a release folder runs as it's unpacked: pdf-lint finds the qpdf
# beside it, and decodes through it.
#
#   release/smoke.sh <folder>
set -euo pipefail

dir=${1:?a release folder}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# The bundled qpdf, not one the environment names.
unset PDF_LINT_QPDF

version=$("$dir/pdf-lint" --version)
echo "$version"
case "$version" in
*"qpdf version 12.4.2 ("*qpdf*bin*) ;;
*)
    echo "smoke.sh: pdf-lint doesn't find the bundled qpdf" >&2
    exit 1
    ;;
esac

# A file whose catalog and page tree are in an object stream, so that
# checking it clean takes decoding through qpdf.
"$dir/qpdf/bin/qpdf" --empty --object-streams=generate "$work/compressed.pdf"
"$dir/pdf-lint" "$work/compressed.pdf"
