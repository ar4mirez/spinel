#!/usr/bin/env bash
#
# Delete iCloud sync-conflict copies from the working tree.
#
#   scripts/clean-sync-conflicts.sh          delete the duplicates, report the rest
#   scripts/clean-sync-conflicts.sh --list   report only, delete nothing
#   scripts/clean-sync-conflicts.sh --self-test   check the rule, in a temp tree
#
# iCloud Drive resolves a two-device edit by keeping both, naming the loser
# `callcache 2.rs` beside `callcache.rs`. They arrive untracked, so `git status`
# lists them and nothing else objects — until `cargo test`, which reads
# `tests/*.rs` off the filesystem rather than out of git and dies on the space:
#
#     error: invalid character ' ' in crate name: `inline_cache 2`
#
# That names neither the file nor the cause, and `cargo build` is unaffected, so
# it reads as a broken test harness. `.gitignore` does not help: Cargo discovers
# targets by walking the directory, not by asking git.
#
# Only a copy that is *byte-identical* to the file it shadows is deleted. There
# is nothing in it to lose, and no judgement to get wrong. A copy that differs,
# or one whose original is gone, is a real edit from the other device: it is
# reported and left alone, for a human to diff and merge.
#
# Directories are not handled. iCloud can conflict one, but the failure is not
# silent the way this one is, and a directory is not something to delete on a
# byte comparison.

set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# The one thing here that can lose a file is the delete, so the rule it turns on
# gets a check: identical goes, different stays, orphan stays, `target/` is not
# walked. Runs in a temp tree and touches nothing else.
#
# The script is *copied* into that tree rather than called in place, because
# `root` is resolved from `$0` and a run from anywhere cleans the repo this
# script lives in — which is the behaviour wanted in earnest and the one thing
# that would make this test silently pass against the wrong directory.
self_test() {
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  mkdir -p "$tmp/scripts" "$tmp/src" "$tmp/target"
  cp "$0" "$tmp/scripts/clean-sync-conflicts.sh"
  printf 'same\n' > "$tmp/src/a.rs";   printf 'same\n'   > "$tmp/src/a 2.rs"
  printf 'orig\n' > "$tmp/src/b.rs";   printf 'EDITED\n' > "$tmp/src/b 2.rs"
  printf 'x\n'    > "$tmp/src/orphan 2.rs"
  printf 'junk\n' > "$tmp/target/c.rs"; printf 'junk\n' > "$tmp/target/c 2.rs"

  "$tmp/scripts/clean-sync-conflicts.sh" > /dev/null

  fail=0
  want() { eval "$1" || { echo "self-test: $2" >&2; fail=1; }; }
  want '[ ! -e "$tmp/src/a 2.rs" ]'    "an identical copy survived"
  want '[ -f "$tmp/src/a.rs" ]'        "the original was deleted"
  want '[ -f "$tmp/src/b 2.rs" ]'      "a differing copy was deleted"
  want '[ -f "$tmp/src/orphan 2.rs" ]' "an orphan was deleted"
  want '[ -f "$tmp/target/c 2.rs" ]'   "target/ was walked"

  [ "$fail" -eq 0 ] || exit 1
  echo "self-test: ok"
  exit 0
}

list_only=false
case "${1-}" in
  --list) list_only=true ;;
  --self-test) self_test ;;
  "") ;;
  *) echo "usage: $0 [--list|--self-test]" >&2; exit 2 ;;
esac

deleted=0
kept=0

# `-print0` and a null-delimited read, because the names contain spaces by
# construction. `target/` and the corpus submodule are skipped: neither is ours
# to clean, and `target/` alone is tens of thousands of files.
while IFS= read -r -d '' copy; do
  # `dir/stem N.ext` -> `dir/stem.ext`, and `dir/stem N` -> `dir/stem`.
  original=$(printf '%s\n' "$copy" | sed -E 's/ [0-9]+(\.[^./]*)?$/\1/')
  [ "$original" = "$copy" ] && continue

  if [ ! -f "$original" ]; then
    # Nothing to compare against, so this is indistinguishable from a file
    # someone meant to call `notes 2.md`. Named, never touched, never counted.
    printf 'orphan  %s\n        no %s beside it; left alone\n' "$copy" "$original"
  elif cmp -s "$copy" "$original"; then
    if $list_only; then
      printf 'dupe    %s\n' "$copy"
    else
      rm -- "$copy"
      printf 'deleted %s\n' "$copy"
    fi
    deleted=$((deleted + 1))
  else
    printf 'kept    %s\n        differs from %s — diff it, this is a real edit\n' \
      "$copy" "$original"
    kept=$((kept + 1))
  fi
done < <(
  find . \
    \( -name .git -o -name target -o -path ./spec/ruby \) -prune -o \
    -type f \( -name '* [0-9]' -o -name '* [0-9].*' \) -print0
)

verb=$($list_only && echo "duplicate(s)" || echo "deleted")
echo "$deleted $verb, $kept needing a human"
