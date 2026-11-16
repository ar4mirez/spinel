#!/usr/bin/env bash
#
# Cold and warm fetch of a lockfile's gems into the global store (#81,
# docs/package-manager.md, "Global store"), beside the same `.gem` files
# fetched by `curl` alone, which is what the network costs; then a warm
# install (#82): every gem already stored, linked into a project's GEM_HOME.
#
#   bench/install.sh                  the Sinatra fixture's Gemfile.lock
#   bench/install.sh path/Gemfile.lock
#   RUNS=9 bench/install.sh           more runs (default 5); medians are printed
#
# Needs the network: it fetches from rubygems.org, into a Spinel home made for
# the run and removed after it, so `~/.spinel` is never touched. The index is
# brought up to date first and kept across runs, so each timed run is the
# store alone: download, verify, unpack, rename. A cold run starts from an
# empty store; a warm run finds every gem stored. A cold install is
# network-bound when its wall time is close to curl's and its CPU time is a
# small part of its wall time. A warm install is `spinel fetch --gem-home`
# with the store full: into an empty GEM_HOME (every link, gemspec and bin
# stub written) and into one already linked (nothing written). Both still
# bring the index up to date first, one request.
#
# `spinel install` (#84) is what will time a whole install against
# `bundle install`; until it lands this is the store and the linking.
#
# Numbers go in the pull request, not into the repo (docs/roadmap.md, rule 7).

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock="${1:-$repo_root/crates/spinel-cli/tests/fixtures/sinatra-lock/Gemfile.lock}"
runs="${RUNS:-5}"
source_url="https://rubygems.org"
spinel="$repo_root/target/release/spinel"

(cd "$repo_root" && cargo build --release -p spinel-cli --quiet)

# The GEM section's specs, `    name (version[-platform])`, as `name@version`.
mapfile -t gems < <(awk '
  /^GEM$/ { in_gem = 1; next }
  /^[A-Z]/ { in_gem = 0 }
  in_gem && /^    [^ ]+ \([^)]*\)$/ { gsub(/[()]/, "", $2); print $1 "@" $2 }
' "$lock")
if [[ ${#gems[@]} -eq 0 ]]; then
  echo "bench/install.sh: no gems in $lock" >&2
  exit 1
fi
names=()
urls=()
for gem in "${gems[@]}"; do
  names+=("${gem%@*}")
  urls+=("$source_url/gems/${gem%@*}-${gem#*@}.gem")
done

work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-bench-install.XXXXXX")"
trap 'rm -rf "$work"' EXIT
export SPINEL_HOME="$work/home"
"$spinel" index --source "$source_url" "${names[@]}" > /dev/null

# `time`'s real, user and sys seconds for one command, on one line.
timed() {
  local TIMEFORMAT='%R %U %S'
  { time "$@" > /dev/null 2>&1; } 2>&1
}

median() {
  sort -n | awk '{ v[NR] = $1 } END { print (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2 }'
}

curl_once() {
  rm -rf "$work/curl" && mkdir "$work/curl"
  local args=()
  local i=0
  for url in "${urls[@]}"; do
    args+=(-o "$work/curl/$i.gem" "$url")
    i=$((i + 1))
  done
  curl --parallel --parallel-max 32 -sSfL "${args[@]}"
}

cold_once() {
  rm -rf "$SPINEL_HOME/store"
  "$spinel" fetch --source "$source_url" "${gems[@]}"
}

warm_once() {
  "$spinel" fetch --source "$source_url" "${gems[@]}"
}

install_fresh_once() {
  rm -rf "$work/project/.spinel/gem_home"
  "$spinel" fetch --source "$source_url" --gem-home "$work/project/.spinel/gem_home" "${gems[@]}"
}

install_again_once() {
  "$spinel" fetch --source "$source_url" --gem-home "$work/project/.spinel/gem_home" "${gems[@]}"
}

# Report the medians of `runs` runs of one function: wall, and user + sys.
report() {
  local label="$1" fn="$2" walls=() cpus=() line real user sys
  for _ in $(seq "$runs"); do
    line="$(timed "$fn")"
    read -r real user sys <<< "$line"
    walls+=("$real")
    cpus+=("$(awk -v u="$user" -v s="$sys" 'BEGIN { printf "%.3f", u + s }')")
  done
  printf '  %-26s wall %6.3f s   cpu %6.3f s\n' "$label" \
    "$(printf '%s\n' "${walls[@]}" | median)" "$(printf '%s\n' "${cpus[@]}" | median)"
}

cold_once > /dev/null
bytes="$(du -sb "$SPINEL_HOME/store/gems" | cut -f1)"
echo "== $(basename "$(dirname "$lock")")/$(basename "$lock"): ${#gems[@]} gems, $bytes bytes unpacked; median of $runs"
report "curl (the network alone)" curl_once
report "spinel fetch, cold" cold_once
report "spinel fetch, warm" warm_once
install_fresh_once > /dev/null
report "warm install, empty home" install_fresh_once
report "warm install, linked home" install_again_once
