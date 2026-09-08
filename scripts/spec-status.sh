#!/usr/bin/env bash
#
# Regenerate `bench/spec-status.md` — the project's progress bar.
#
#   scripts/spec-status.sh
#
# One row per ruby/spec directory, pass/fail/blocked/skip. The table is written
# by the harness itself, not assembled here: the counts and the markdown come
# out of the same run, so a row can never disagree with the total it is part of.
#
# The file is committed, and CI runs this script and then `git diff --exit-code`.
# That is the whole staleness check — there is no `--check` mode, because a
# regenerate-and-diff already is one and cannot drift from what it checks.
#
# Numbers live in `bench/` and are reproduced by script, never typed into prose:
# `docs/roadmap.md`, rule 7.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
corpus="$repo_root/spec/ruby"
out="$repo_root/bench/spec-status.md"

if [[ ! -e "$corpus/spec_helper.rb" ]]; then
  echo "spec-status.sh: ruby/spec is not checked out at spec/ruby" >&2
  echo "                git submodule update --init spec/ruby" >&2
  exit 2
fi

cargo build --release -p spec-harness --quiet

mkdir -p "$(dirname "$out")"
# Written to a temporary file first: a run that fails part way through must not
# leave a truncated progress bar behind that CI would then report as a diff.
scratch="$(mktemp)"
trap 'rm -f "$scratch"' EXIT

status=0
"$repo_root/target/release/spec-harness" --by-directory "$corpus" > "$scratch" || status=$?
if [[ $status -ne 0 ]]; then
  echo "spec-status.sh: the spec run reported failures, unreadable files, or tag" >&2
  echo "                problems. The table is only printed on a clean run — run" >&2
  echo "                scripts/spec.sh to see what went wrong." >&2
  exit "$status"
fi

# The total row is printed from its own accumulator, so a bug in the grouping
# would show up as rows that do not add up to it rather than as a wrong total.
# Cheap to check here, and it is the reconciliation the issue asks for.
awk -F'|' '
  /^\| `/      { for (c = 3; c <= 9; c++) sum[c] += $c }
  /^\| \*\*tot/ { for (c = 3; c <= 9; c++) total[c] = $c + 0 }
  END {
    for (c = 3; c <= 8; c++)
      if (sum[c] != total[c]) {
        printf "spec-status.sh: column %d rows sum to %d, total row says %d\n", \
               c - 2, sum[c], total[c] > "/dev/stderr"
        bad = 1
      }
    exit bad
  }
' "$scratch"

mv "$scratch" "$out"
trap - EXIT
echo "wrote ${out#"$repo_root"/}"
tail -1 "$out"
