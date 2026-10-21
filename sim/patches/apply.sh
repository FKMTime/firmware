#!/bin/sh
# Copies the exact crate version from the cargo registry into target/sim-patches/<name>-<ver>/ and
# applies <name>-<ver>.patch from this folder. Idempotent: a crate is re-copied only when its patch
# changed. Prints the patched path.
#   sim/patches/apply.sh esp-hal-1.2.2
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$HERE/../.."
OUT="$ROOT/target/sim-patches"
NAMEVER="$1"
PATCH="$HERE/$NAMEVER.patch"
DIR="$OUT/$NAMEVER"
[ -f "$PATCH" ] || { echo "no $PATCH (the lockfile's version has no sim patch)" >&2; exit 1; }
STAMP="$(shasum -a 256 "$PATCH" | cut -c1-16)"
if [ "$(cat "$DIR/.patched" 2>/dev/null)" != "$STAMP" ]; then
    find_src() { ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/"$NAMEVER" 2>/dev/null | head -1; }
    SRC="$(find_src)"
    [ -n "$SRC" ] || { (cd "$ROOT" && cargo fetch -q) && SRC="$(find_src)"; }
    [ -n "$SRC" ] || { echo "$NAMEVER not in the cargo registry" >&2; exit 1; }
    rm -rf "$DIR" && mkdir -p "$DIR"
    (cd "$SRC" && tar cf - .) | (cd "$DIR" && tar xf -)
    patch -s -d "$DIR" -p1 <"$PATCH"
    echo "$STAMP" >"$DIR/.patched"
fi
echo "$(cd "$DIR" && pwd)"
