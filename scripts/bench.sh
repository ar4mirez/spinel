#!/usr/bin/env bash
#
# Run the benchmarks in `bench/`.
#
#   scripts/bench.sh                 every benchmark
#   scripts/bench.sh regex_captures  one of them
#   scripts/bench.sh io_gets         a Ruby one, `bench/io_gets.rb`, on Spinel
#   scripts/bench.sh --ruby          the same regex patterns on system ruby
#   scripts/bench.sh --yjit-bench    the yjit-bench subset on Spinel and ruby
#                                    (#63); rewrites the table in bench/README.md
#   scripts/bench.sh --yjit-bench-table  re-render that table from results.tsv
#   scripts/bench.sh --yjit-bench-check  fail if the table is not results.tsv
#
# Numbers go in the PR, not into the repo — `docs/roadmap.md`, rule 7. The two
# exceptions are generated and committed on purpose: `bench/spec-status.md`,
# by `scripts/spec-status.sh`, is the progress bar, and the yjit-bench table in
# `bench/README.md`, by `--yjit-bench` here, is the interpreter's baseline. Both
# are written by a script and checked against its output, never typed.
#
# `--ruby` times `bench/regex-patterns.txt` on the system Ruby with YJIT, which
# is the comparison `CLAUDE.md` asks engine benchmarks to make. Both sides read
# that one file, so neither can quietly measure a different pattern.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
patterns="$repo_root/bench/regex-patterns.txt"

# --- the yjit-bench subset (#63) -------------------------------------------
#
# `bench/yjit-bench/benchmarks/` holds benchmarks copied verbatim from
# ruby-bench (formerly yjit-bench) at the commit named in
# `bench/yjit-bench/README.md`; `lib/harness/loader.rb` beside them is ours. A
# run times each one on the release `spinel`, on `ruby --disable-yjit` and on
# `ruby --yjit`, through that one harness, writes the medians to
# `results.tsv`, and renders the table in `bench/README.md` from that file.
# The table is only ever written here, and `--yjit-bench-check` (run by
# `crates/spinel-cli/tests/yjit_bench.rs`) fails when it is not what
# `results.tsv` renders to.

yb_dir="$repo_root/bench/yjit-bench"
yb_results="$yb_dir/results.tsv"
yb_readme="$repo_root/bench/README.md"
yb_begin='<!-- yjit-bench table: written by scripts/bench.sh --yjit-bench; do not edit -->'
yb_end='<!-- end yjit-bench table -->'

# Every vendored benchmark as `name<TAB>path`: `fib.rb` is `fib`, and
# `nbody/benchmark.rb` is `nbody`, as ruby-bench names them.
yb_list() {
  local f
  for f in "$yb_dir"/benchmarks/*.rb "$yb_dir"/benchmarks/*/benchmark.rb; do
    [[ -f "$f" ]] || continue
    if [[ "$(basename "$f")" == benchmark.rb ]]; then
      printf '%s\t%s\n' "$(basename "$(dirname "$f")")" "$f"
    else
      printf '%s\t%s\n' "$(basename "$f" .rb)" "$f"
    fi
  done | LC_ALL=C sort
}

# Pin runs to one CPU with address randomisation off, as far as this machine
# has working tools for each, and cap their memory. `yb_pinning` names the
# pinning that applied, for the `iterations` line.
yb_pin=() yb_cap=() yb_pinning=""
yb_probe() {
  local cpu="${BENCH_CPU:-2}" arch
  arch="$(uname -m)"
  if command -v taskset >/dev/null && taskset -c "$cpu" true 2>/dev/null; then
    yb_pin+=(taskset -c "$cpu"); yb_pinning+=", one CPU"
  fi
  if command -v setarch >/dev/null && setarch "$arch" -R true 2>/dev/null; then
    yb_pin+=(setarch "$arch" -R); yb_pinning+=", ASLR off"
  fi
  if command -v systemd-run >/dev/null && systemd-run --user --scope -q true 2>/dev/null; then
    yb_cap=(systemd-run --user --scope -q -p MemoryMax=1500M -p MemorySwapMax=0)
  fi
}

# Run a command as `yb_probe` set up, under a time limit.
yb_pinned() {
  local limit="$1"; shift
  local cmd=(${yb_pin[@]+"${yb_pin[@]}"} "$@")
  if command -v timeout >/dev/null; then cmd=(timeout -k 5 "$limit" "${cmd[@]}"); fi
  ${yb_cap[@]+"${yb_cap[@]}"} "${cmd[@]}"
}

# The median a run printed, from the harness's last line.
yb_median() {
  local out median
  out="$(yb_pinned "$@")" || { echo "bench.sh: failed: $*" >&2; echo "$out" >&2; return 1; }
  median="$(echo "$out" | sed -n 's/^median_ms: //p' | tail -1)"
  if [[ ! "$median" =~ ^[0-9]+(\.[0-9]+)?$ ]]; then
    echo "bench.sh: no median_ms from: $*" >&2; echo "$out" >&2; return 1
  fi
  echo "$median"
}

# results.tsv -> the Markdown table, on standard output.
yb_render() {
  # The C locale, so a decimal point is a point wherever this runs.
  LC_ALL=C awk -F'\t' '
    $1 == "spinel" { spinel = $2 }
    $1 == "ruby" { ruby = $2 }
    $1 == "iterations" { itrs = $2 }
    $1 == "bench" { n++; name[n] = $2; sp[n] = $3; interp[n] = $4; yjit[n] = $5 }
    END {
      print "Spinel at `" spinel "`, against `" ruby "`. " itrs "."
      print ""
      print "| benchmark | Spinel | `ruby --disable-yjit` | Spinel ÷ interpreter | within 1.5× | `ruby --yjit` |"
      print "|---|---:|---:|---:|:---:|---:|"
      for (i = 1; i <= n; i++) {
        r = sprintf("%.2f", sp[i] / interp[i])
        printf "| %s | %.1f ms | %.1f ms | %s× | %s | %.1f ms |\n", name[i], sp[i], interp[i], r, (r + 0 <= 1.5 ? "yes" : "no"), yjit[i]
      }
    }
  ' "$yb_results"
}

# bench/README.md with the text between the markers replaced by `yb_render`.
yb_spliced() {
  # The table goes through the environment: `awk -v` refuses a newline on macOS.
  YB_TABLE="$(yb_render)" awk -v begin="$yb_begin" -v end="$yb_end" '
    $0 == begin { print; print ENVIRON["YB_TABLE"]; skipping = 1; found = 1; next }
    $0 == end { skipping = 0 }
    !skipping { print }
    END { if (!found) exit 3 }
  ' "$yb_readme"
}

yb_check() {
  [[ -f "$yb_results" ]] || { echo "bench.sh: no $yb_results; run --yjit-bench" >&2; return 1; }
  local want have
  want="$(yb_list | cut -f1)"
  have="$(awk -F'\t' '$1 == "bench" { print $2 }' "$yb_results")"
  if [[ "$want" != "$have" ]]; then
    echo "bench.sh: results.tsv measures [$(echo $have)], bench/yjit-bench has [$(echo $want)]; run --yjit-bench" >&2
    return 1
  fi
  if ! diff -u "$yb_readme" <(yb_spliced) >&2; then
    echo "bench.sh: the yjit-bench table in bench/README.md is not what results.tsv renders to; run --yjit-bench-table, never edit it by hand" >&2
    return 1
  fi
}

yb_write_table() {
  local spliced
  spliced="$(yb_spliced)" || { echo "bench.sh: no table markers in $yb_readme" >&2; return 1; }
  printf '%s\n' "$spliced" > "$yb_readme"
}

yb_run() {
  command -v ruby >/dev/null || { echo "bench.sh: no ruby on PATH" >&2; return 2; }
  cargo build --release -p spinel-cli
  local spinel="$repo_root/target/release/spinel"
  local rev
  rev="$(git -C "$repo_root" rev-parse --short HEAD)"
  # Only the engine decides the numbers; an edit under bench/ does not.
  git -C "$repo_root" diff --quiet HEAD -- crates core lib stdlib || rev="$rev with local changes"
  local warmup="${WARMUP_ITRS:-1}" timed="${BENCH_ITRS:-5}"
  export WARMUP_ITRS="$warmup" BENCH_ITRS="$timed"
  local tmp="$yb_results.tmp"
  yb_probe
  {
    echo "# Written by scripts/bench.sh --yjit-bench. Medians in milliseconds; do not edit."
    printf 'spinel\t%s\n' "$rev"
    printf 'ruby\t%s\n' "$(ruby --disable-yjit -e 'print RUBY_DESCRIPTION')"
    printf 'iterations\t%s warmup and the median of %s timed iterations each%s\n' "$warmup" "$timed" "$yb_pinning"
    printf '#\tbenchmark\tspinel\truby --disable-yjit\truby --yjit\n'
  } > "$tmp"
  local name path s i y
  while IFS=$'\t' read -r name path; do
    echo "== $name" >&2
    s="$(yb_median 600 "$spinel" run "$path")" || return 1
    i="$(yb_median 120 ruby --disable-yjit "$path")" || return 1
    y="$(yb_median 120 ruby --yjit "$path")" || return 1
    echo "   spinel $s ms, ruby --disable-yjit $i ms, ruby --yjit $y ms" >&2
    printf 'bench\t%s\t%s\t%s\t%s\n' "$name" "$s" "$i" "$y" >> "$tmp"
  done < <(yb_list)
  mv "$tmp" "$yb_results"
  yb_write_table
}

case "${1:-}" in
  --yjit-bench) yb_run; exit ;;
  --yjit-bench-table) yb_write_table; exit ;;
  --yjit-bench-check) yb_check; exit ;;
esac

if [[ "${1:-}" == "--ruby" ]]; then
  command -v ruby >/dev/null || { echo "bench.sh: no ruby on PATH" >&2; exit 2; }
  ruby --yjit -e '
    iterations = 20_000
    puts "system #{RUBY_DESCRIPTION}"
    puts "  %-20s  %12s  %7s" % ["pattern", "per match", "groups"]
    File.readlines(ARGV[0]).each do |line|
      next if line.start_with?("#") || line.strip.empty?
      name, source, subject = line.chomp("\n").split("\t", 3)
      re = Regexp.new(source)
      # Min of five, matching `time()` in the Rust benchmarks: the quantity is
      # the time with nothing else interfering.
      best = (1..5).map do
        started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
        iterations.times { re.match(subject) }
        (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) / iterations
      end.min
      per = best < 1e-6 ? "%.0fns" % (best * 1e9) : "%.3fµs" % (best * 1e6)
      puts "  %-20s  %12s  %7d" % [name, per, re.names.size + source.scan(/(?<!\\)\((?!\?)/).size]
    end
  ' "$patterns"
  exit 0
fi

# A `bench/*.rb` runs on the release `spinel`, and on `ruby --yjit` by hand:
# its header says how.
run_ruby_bench() {
  echo "== $1"
  "$repo_root/target/release/spinel" run "$1"
}

# `cargo bench` with `harness = false` just runs each `main`, so a name selects
# one the way `--bench` does.
if [[ $# -gt 0 ]]; then
  if [[ -f "$repo_root/bench/$1.rb" ]]; then
    cargo build --release -p spinel-cli
    run_ruby_bench "$repo_root/bench/$1.rb"
    exit 0
  fi
  exec cargo bench --bench "$1"
fi
cargo bench --benches
cargo build --release -p spinel-cli
for bench in "$repo_root"/bench/*.rb; do
  run_ruby_bench "$bench"
done
