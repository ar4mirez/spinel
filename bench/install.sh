#!/usr/bin/env bash
#
# Cold and warm `spinel install` of a real lockfile (#91, docs/package-manager.md,
# "Validation targets"), beside `curl` fetching the same `.gem` files, which is
# what the network costs, and `bundle install` of the same when `bundle` is
# found.
#
#   bench/install.sh                   measure bench/install/rails/ and rewrite
#                                      bench/install/results.tsv and the table
#                                      in bench/README.md
#   bench/install.sh path/Gemfile.lock measure another lockfile (its Gemfile
#                                      beside it) and only print the table
#   bench/install.sh --table           re-render the table from results.tsv
#   bench/install.sh --check           fail if the table is not results.tsv,
#                                      the lockfile is not the one measured, or
#                                      the warm install is not sub-second
#   RUNS=9 bench/install.sh            more runs of each case (default 5)
#
# `bench/install/rails/` is the Gemfile `rails new` writes (Rails 8.1) and the
# lockfile `bundle lock --add-checksums` resolves for it, with `json` kept to
# the major version of Spinel's built-in (its Gemfile says why).
#
# Measuring needs the network: it fetches from rubygems.org into a directory
# the script makes for the run and removes after it, so `~/.spinel` and
# `~/.bundle` are never touched. The cases, each run RUNS times, the median and
# the fastest wall time kept, with the median CPU time (user + sys):
#
#   curl          the `.gem` files `spinel install` fetched, 32 at a time
#   cold          an empty store and no index cache, a fresh project: every
#                 gem downloaded, verified, unpacked and linked
#   warm          every gem already in the store, a fresh project: links,
#                 gemspecs, bin stubs and the require map written, nothing
#                 fetched (the lockfile's CHECKSUMS name each store entry)
#   again         the same project installed a second time: nothing to change
#   bundle cold   `bundle install` with an empty BUNDLE_PATH and home of its
#                 own, extensions compiled
#   bundle again  the same, everything in place
#
# A cold install is network-bound when its wall time is close to curl's and
# its CPU time is a small part of its wall time.
#
# Numbers live in `bench/` (docs/roadmap.md, rule 7): results.tsv is written
# here and the table is rendered from it, so neither is ever typed.
# `crates/spinel-cli/tests/install_bench.rs` runs `--check`, offline.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bench_dir="$repo_root/bench/install"
default_lock="$bench_dir/rails/Gemfile.lock"
results="$bench_dir/results.tsv"
readme="$repo_root/bench/README.md"
begin='<!-- install table: written by bench/install.sh; do not edit -->'
end='<!-- end install table -->'
# The case a warm install is, which `--check` holds to under a second.
warm_case="spinel install, warm"

sha256() {
  if command -v sha256sum > /dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# A results file -> the Markdown table, on standard output.
render() {
  LC_ALL=C awk -F'\t' '
    $1 == "spinel" { spinel = $2 }
    $1 == "lockfile" { lock = $2 }
    $1 == "gems" { gems = $2 }
    $1 == "runs" { runs = $2 }
    $1 == "bundler" { bundler = $2 }
    $1 == "case" { n++; name[n] = $2; med[n] = $3; min[n] = $4; cpu[n] = $5 }
    END {
      print "Spinel at `" spinel "`, `" lock "`: " gems "; " runs "."
      if (bundler != "") print "Bundler: `" bundler "`."
      print ""
      print "| case | median wall | fastest wall | median CPU |"
      print "|---|---:|---:|---:|"
      for (i = 1; i <= n; i++)
        printf "| %s | %.3f s | %.3f s | %.3f s |\n", name[i], med[i], min[i], cpu[i]
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
  out="$(spliced)" || { echo "bench/install.sh: no table markers in $readme" >&2; return 1; }
  printf '%s\n' "$out" > "$readme"
}

check() {
  [[ -f "$results" ]] || { echo "bench/install.sh: no $results; run bench/install.sh" >&2; return 1; }
  local recorded
  recorded="$(awk -F'\t' '$1 == "lockfile" { print $3 }' "$results")"
  if [[ "$recorded" != "$(sha256 "$default_lock")" ]]; then
    echo "bench/install.sh: results.tsv measured another bench/install/rails/Gemfile.lock; run bench/install.sh" >&2
    return 1
  fi
  local warm
  warm="$(awk -F'\t' -v c="$warm_case" '$1 == "case" && $2 == c { print $3 }' "$results")"
  if [[ -z "$warm" ]]; then
    echo "bench/install.sh: results.tsv has no \"$warm_case\" case" >&2
    return 1
  fi
  if ! LC_ALL=C awk -v w="$warm" 'BEGIN { exit !(w < 1) }'; then
    echo "bench/install.sh: a warm install took $warm s, not under a second" >&2
    return 1
  fi
  if ! diff -u "$readme" <(spliced) >&2; then
    echo "bench/install.sh: the install table in bench/README.md is not what results.tsv renders to; run bench/install.sh --table, never edit it by hand" >&2
    return 1
  fi
}

case "${1:-}" in
  --table) write_table; exit ;;
  --check) check; exit ;;
esac

lock="$(cd "$(dirname "${1:-$default_lock}")" && pwd)/$(basename "${1:-$default_lock}")"
runs="${RUNS:-5}"
spinel="$repo_root/target/release/spinel"
[[ -f "$(dirname "$lock")/Gemfile" ]] || { echo "bench/install.sh: no Gemfile beside $lock" >&2; exit 1; }
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo "bench/install.sh: RUNS must be a positive number" >&2; exit 1; }

(cd "$repo_root" && cargo build --release -p spinel-cli --quiet)

work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-bench-install.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT
export SPINEL_HOME="$work/home"
app="$work/app"
mkdir "$app"
cp "$lock" "$app/Gemfile.lock"
cp "$(dirname "$lock")/Gemfile" "$app/Gemfile"
bundle_env=(BUNDLE_PATH="$work/bundle/path" BUNDLE_USER_HOME="$work/bundle/home"
  BUNDLE_APP_CONFIG="$work/bundle/config")

# `time`'s real, user and sys seconds for one command, on one line.
timed() {
  local TIMEFORMAT='%R %U %S'
  { time "$@" > /dev/null 2>&1; } 2>&1
}

spinel_cold_once() {
  rm -rf -- "$SPINEL_HOME/store" "$SPINEL_HOME/index" "$app/.spinel"
  (cd "$app" && "$spinel" install)
}

spinel_warm_once() {
  rm -rf -- "$app/.spinel"
  (cd "$app" && "$spinel" install)
}

spinel_again_once() {
  (cd "$app" && "$spinel" install)
}

curl_once() {
  rm -rf -- "$work/curl" && mkdir "$work/curl"
  local args=() i=0 url
  for url in "${urls[@]}"; do
    args+=(-o "$work/curl/$i.gem" "$url")
    i=$((i + 1))
  done
  curl --parallel --parallel-max 32 -sSfL "${args[@]}"
}

bundle_cold_once() {
  rm -rf -- "$work/bundle"
  (cd "$app" && env "${bundle_env[@]}" bundle install --quiet)
}

bundle_again_once() {
  (cd "$app" && env "${bundle_env[@]}" bundle install --quiet)
}

out="$work/results.tsv"

# Time `runs` runs of one function and append its case line: the median wall,
# the fastest wall and the median CPU, in seconds.
measure() {
  local label="$1" fn="$2" line real user sys walls=() cpus=()
  echo "== $label" >&2
  for _ in $(seq "$runs"); do
    line="$(timed "$fn")" || { echo "bench/install.sh: $label failed" >&2; "$fn" >&2 || true; return 1; }
    read -r real user sys <<< "$line"
    walls+=("$real")
    cpus+=("$(LC_ALL=C awk -v u="$user" -v s="$sys" 'BEGIN { printf "%.3f", u + s }')")
  done
  printf '%s\n' "${walls[@]}" | LC_ALL=C sort -n | LC_ALL=C awk -v label="$label" -v cpus="${cpus[*]}" '
    { v[NR] = $1 }
    END {
      k = split(cpus, c, " ")
      # The median CPU, sorted by hand: awk has no sort.
      for (i = 1; i <= k; i++) for (j = i + 1; j <= k; j++) if (c[j] + 0 < c[i] + 0) { t = c[i]; c[i] = c[j]; c[j] = t }
      wall = (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2
      cpu = (k % 2) ? c[(k + 1) / 2] : (c[k / 2] + c[k / 2 + 1]) / 2
      printf "case\t%s\t%.3f\t%.3f\t%.3f\n", label, wall, v[1], cpu
    }
  ' >> "$out"
}

# One cold install first, untimed: what it fetched is what curl fetches.
if ! spinel_cold_once > "$work/install.log" 2>&1; then
  cat "$work/install.log" >&2
  echo "bench/install.sh: spinel install of $lock failed" >&2
  exit 1
fi
urls=()
linked=0
for gem in "$app"/.spinel/gem_home/gems/*; do
  linked=$((linked + 1))
  # A gem from the store, not a built-in: `<full name>` is its `.gem`'s name.
  if [[ "$(readlink "$gem")" == "$SPINEL_HOME/store/gems/"* ]]; then
    urls+=("https://rubygems.org/gems/$(basename "$gem").gem")
  fi
done
bytes="$(( $(du -sk "$SPINEL_HOME/store/gems" | cut -f1) * 1024 ))"

rev="$(git -C "$repo_root" rev-parse --short HEAD)"
# Only the installer decides the numbers; an edit under bench/ does not.
git -C "$repo_root" diff --quiet HEAD -- crates core lib stdlib || rev="$rev with local changes"
relative="${lock#"$repo_root"/}"
{
  echo "# Written by bench/install.sh. Seconds; do not edit."
  printf 'spinel\t%s\n' "$rev"
  printf 'lockfile\t%s\t%s\n' "$relative" "$(sha256 "$lock")"
  printf 'gems\t%s gems installed, %s of them fetched (%s MB unpacked)\n' \
    "$linked" "${#urls[@]}" "$((bytes / 1000000))"
  printf 'runs\tthe median and the fastest of %s runs each\n' "$runs"
  if command -v bundle > /dev/null; then
    printf 'bundler\t%s, %s\n' "$(bundle --version)" "$(ruby -e 'print RUBY_DESCRIPTION')"
  fi
  printf '#\tcase\tmedian wall\tfastest wall\tmedian cpu\n'
} > "$out"

measure "curl, the same .gem files" curl_once
measure "spinel install, cold" spinel_cold_once
measure "$warm_case" spinel_warm_once
measure "spinel install, again" spinel_again_once
if command -v bundle > /dev/null; then
  measure "bundle install, cold" bundle_cold_once
  measure "bundle install, again" bundle_again_once
fi

render "$out"
if [[ "$lock" == "$default_lock" ]]; then
  cp "$out" "$results"
  write_table
fi
