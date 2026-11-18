#!/usr/bin/env bash
# Gives this checkout's pdf-lint the version of its release candidate <n>,
# the crate's version with -rc.<n> (0.2.0-rc.1), in crates/pdf-lint's
# Cargo.toml and in Cargo.lock, and prints it: what release.yml builds a
# release candidate of main with, so that the archives, the folders in them
# and `pdf-lint --version` all say it. Commits nothing: release.yml commits
# it once the archives are built, in a commit the tag alone holds. A crate
# whose version is a pre-release already is refused: its release candidates
# are of a release.
#
#   release/prerelease.sh <n>
set -euo pipefail

n=${1:?the number of the release candidate}
root=$(cd "$(dirname "$0")/.." && pwd)
manifest=$root/crates/pdf-lint/Cargo.toml
lock=$root/Cargo.lock

fail() {
    echo "prerelease.sh: $1" >&2
    exit 1
}

[[ $n =~ ^[1-9][0-9]*$ ]] || fail "$n isn't a release candidate's number, 1 or more"
version=$(tr -d '\r' <"$manifest" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1)
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "pdf-lint's version is $version, not a release's"
rc=$version-rc.$n

# `version = "<version>"` rewritten once in <file>: the first, or the first
# on the line after <after>. Written by awk to a file beside it, as sed -i
# differs between GNU's and macOS', and with each line's ending as it was, a
# CR too, whatever line ends a checkout gave it.
rewrite() {
    local file=$1 after=$2
    awk -v from="version = \"$version\"" -v to="version = \"$rc\"" -v after="$after" '
        { line = $0; cr = sub(/\r$/, "", line) }
        !done && line == from && (after == "" || prev == after) {
            print to (cr ? "\r" : ""); done = 1; prev = line; next
        }
        { print; prev = line }
        END { exit !done }' "$file" >"$file.rc" || {
        rm -f "$file.rc"
        fail "$file has no $version to give $rc"
    }
    mv "$file.rc" "$file"
}
rewrite "$manifest" ""
rewrite "$lock" 'name = "pdf-lint"'
echo "$rc"
