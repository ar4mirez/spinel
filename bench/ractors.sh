#!/usr/bin/env bash
#
# A CPU-bound benchmark split across Ractors (#119): `bench/ractors/benchmark.rb`
# run with no Ractor, then with its work cut into 1, 2 and 4 Ractors, on
# Spinel and on CRuby, the time falling as Ractors are added when they run in
# parallel.
#
#   bench/ractors.sh             measure and rewrite bench/ractors/results.tsv
#                                and the table in bench/README.md
#   bench/ractors.sh --table     re-render the table from results.tsv
#   bench/ractors.sh --check     fail if the table is not results.tsv, a case
#                                is missing, or Spinel's time does not fall
#                                from 1 Ractor to 2 to 4
#   RUNS=9 bench/ractors.sh      more runs of each case (default 5)
#   UNITS=480 bench/ractors.sh   another amount of work (default 960)
#   RUBY=ruby-4.0.9 bench/ractors.sh   which CRuby (default `ruby`)
#
# Each case's time is the program's own, from the first Ractor made to the
# last one's value, so boot is not part of it; the median of RUNS runs is kept.
# Every run's checksum must be the same, or the run fails: cut any way, the
# work is the same work.
#
# Ractors running in parallel need as many idle CPUs as there are Ractors, so
# the numbers move with whatever else the machine runs. The header says how
# many CPUs it has and its 1-minute load average before and after, so a
# reading taken under load is marked as one.
#
# Numbers live in `bench/` (docs/roadmap.md, rule 7): results.tsv is written
# here and the table is rendered from it, so neither is ever typed.
# `crates/spinel-cli/tests/ractor_bench.rs` runs `--check`.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
program="$repo_root/bench/ractors/benchmark.rb"
results="$repo_root/bench/ractors/results.tsv"
readme="$repo_root/bench/README.md"
begin='<!-- ractors table: written by bench/ractors.sh; do not edit -->'
end='<!-- end ractors table -->'
# The cases, by how many Ractors: 0 is the work run by the main Ractor alone.
cases=(0 1 2 4)

# A results file -> the Markdown table, on standard output.
render() {
  LC_ALL=C awk -F'\t' '
    $1 == "spinel" { spinel = $2 }
    $1 == "ruby" { ruby = $2 }
    $1 == "machine" { machine = $2 }
    $1 == "runs" { runs = $2 }
    $1 == "case" { n++; k[n] = $2; sp[n] = $3; rb[n] = $4; if ($2 == 1) { sp1 = $3; rb1 = $4 } }
    END {
      print "Spinel at `" spinel "`, against `" ruby "`. " runs "; " machine "."
      print ""
      print "| Ractors | Spinel | Spinel speedup | CRuby | CRuby speedup |"
      print "|---|---:|---:|---:|---:|"
      for (i = 1; i <= n; i++) {
        label = k[i] == 0 ? "none (main only)" : k[i]
        if (k[i] == 0) { s = "–"; r = "–" }
        else { s = sprintf("%.2f×", sp1 / sp[i]); r = sprintf("%.2f×", rb1 / rb[i]) }
        printf "| %s | %.1f ms | %s | %.1f ms | %s |\n", label, sp[i], s, rb[i], r
      }
    }
  ' "$1"
}

# bench/README.md with the text between the markers replaced by the table.
spliced() {
  # The table goes through the environment: `awk -v` refuses a newline on macOS.
  TABLE="$(render "$results")" awk -v begin="$begin" -v end="$end" '
    $0 == begin { print; print ENVIRON["TABLE"]; skipping = 1; found = 1; next }
    $0 == end { skipping = 0 }
    !skipping { print }
    END { if (!found) exit 3 }
  ' "$readme"
}

write_table() {
  local out
  out="$(spliced)" || { echo "bench/ractors.sh: no table markers in $readme" >&2; return 1; }
  printf '%s\n' "$out" > "$readme"
}

check() {
  [[ -f "$results" ]] || { echo "bench/ractors.sh: no $results; run bench/ractors.sh" >&2; return 1; }
  local want have
  want="${cases[*]}"
  have="$(awk -F'\t' '$1 == "case" { print $2 }' "$results" | tr '\n' ' ' | sed 's/ $//')"
  if [[ "$want" != "$have" ]]; then
    echo "bench/ractors.sh: results.tsv measures Ractors [$have], not [$want]; run bench/ractors.sh" >&2
    return 1
  fi
  # Scaling is the point of the benchmark (#119): 1 Ractor, then 2, then 4,
  # each faster than the one before.
  if ! LC_ALL=C awk -F'\t' '
    $1 == "case" && $2 > 0 { if (seen && $3 >= last) bad = 1; last = $3; seen = 1 }
    END { exit bad }
  ' "$results"; then
    echo "bench/ractors.sh: Spinel's time in results.tsv does not fall from 1 Ractor to 2 to 4" >&2
    return 1
  fi
  if ! diff -u "$readme" <(spliced) >&2; then
    echo "bench/ractors.sh: the ractors table in bench/README.md is not what results.tsv renders to; run bench/ractors.sh --table, never edit it by hand" >&2
    return 1
  fi
}

case "${1:-}" in
  --table) write_table; exit ;;
  --check) check; exit ;;
esac

runs="${RUNS:-5}"
units="${UNITS:-960}"
ruby="${RUBY:-ruby}"
spinel="$repo_root/target/release/spinel"
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo "bench/ractors.sh: RUNS must be a positive number" >&2; exit 1; }
[[ "$units" =~ ^[1-9][0-9]*$ && $((units % 4)) -eq 0 ]] || {
  echo "bench/ractors.sh: UNITS must be a positive multiple of 4" >&2
  exit 1
}
command -v "$ruby" > /dev/null || { echo "bench/ractors.sh: no $ruby on PATH" >&2; exit 2; }

(cd "$repo_root" && cargo build --release -p spinel-cli --quiet)

load() { cut -d' ' -f1 /proc/loadavg 2> /dev/null || uptime | sed 's/.*average[s]*: *//; s/,.*//'; }
cpus() { getconf _NPROCESSORS_ONLN 2> /dev/null || nproc; }

checksum=""
# The median time, in milliseconds, of `runs` runs of one engine on one case.
measure() {
  local ractors="$1"; shift
  local out sum ms times=()
  for _ in $(seq "$runs"); do
    out="$("$@" "$program" "$ractors" "$units")" || {
      echo "bench/ractors.sh: failed: $* $program $ractors $units" >&2
      echo "$out" >&2
      return 1
    }
    sum="$(sed -n 's/^checksum: //p' <<< "$out")"
    ms="$(sed -n 's/^ms: //p' <<< "$out")"
    [[ -n "$checksum" ]] || checksum="$sum"
    if [[ "$sum" != "$checksum" ]]; then
      echo "bench/ractors.sh: $* with $ractors Ractors answered checksum $sum, not $checksum" >&2
      return 1
    fi
    times+=("$ms")
  done
  printf '%s\n' "${times[@]}" | LC_ALL=C sort -n | LC_ALL=C awk '
    { v[NR] = $1 }
    END { printf "%.1f", (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2 }
  '
}

rev="$(git -C "$repo_root" rev-parse --short HEAD)"
# Only the engine decides the numbers; an edit under bench/ does not.
git -C "$repo_root" diff --quiet HEAD -- crates core lib stdlib || rev="$rev with local changes"
before="$(load)"
out="$(mktemp "${TMPDIR:-/tmp}/spinel-bench-ractors.XXXXXX")"
trap 'rm -f -- "$out"' EXIT
rows=()
for k in "${cases[@]}"; do
  echo "== $k Ractors" >&2
  s="$(measure "$k" "$spinel" run)"
  r="$(measure "$k" "$ruby")"
  echo "   spinel $s ms, ruby $r ms" >&2
  rows+=("$(printf 'case\t%s\t%s\t%s' "$k" "$s" "$r")")
done
after="$(load)"
{
  echo "# Written by bench/ractors.sh. Medians in milliseconds; do not edit."
  printf 'spinel\t%s\n' "$rev"
  printf 'ruby\t%s\n' "$("$ruby" -e 'print RUBY_DESCRIPTION')"
  printf 'machine\t%s CPUs, 1-minute load average %s before and %s after\n' "$(cpus)" "$before" "$after"
  printf 'runs\t%s units of work, the median of %s runs each\n' "$units" "$runs"
  printf '#\tractors\tspinel\truby\n'
  printf '%s\n' "${rows[@]}"
} > "$out"
render "$out"
cp "$out" "$results"
write_table
