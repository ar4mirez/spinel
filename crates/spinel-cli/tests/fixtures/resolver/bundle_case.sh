#!/usr/bin/env bash
#
# What Bundler resolves for a synthetic compact index (#78): an opt-in tool
# for checking the resolver's unit-test expectations, never run by CI.
#
#   crates/spinel-cli/tests/fixtures/resolver/bundle_case.sh oracle/needs_pre
#
# The case directory holds `info/<gem>` (`/info` lines, no header needed)
# and `Gemfile.gems` (the Gemfile's `gem` lines, no `source`). `/versions`
# is made from the info files, the lot is served on 127.0.0.1, and `bundle
# lock` resolves it through `bundler_oracle.rb` for the Ruby, RubyGems and
# platform in ORACLE_RUBY (4.0.0), ORACLE_RUBYGEMS (4.0.16) and
# ORACLE_PLATFORM (x86_64-linux). Prints the lockfile's specs, one per line,
# or Bundler's explanation, and exits 1 when Bundler could not resolve.
# THOR_COLUMNS keeps Bundler's UI from rewrapping the explanation's long
# lines at 80 columns (`Bundler::UI::Shell#word_wrap`).
#
# Written with Bundler 4.0.20 on Ruby 4.0.9; Python 3 serves the files.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
case_dir="$(cd "${1:?usage: bundle_case.sh <case dir>}" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-bundle-case.XXXXXX")"
server=""
cleanup() {
  [[ -n "$server" ]] && kill "$server" 2>/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

mkdir -p "$work/index/info"
cp "$case_dir"/info/* "$work/index/info/"
{
  echo "created_at: 2026-01-01T00:00:00Z"
  echo "---"
  for info in "$work"/index/info/*; do
    gem="$(basename "$info")"
    versions="$(grep -v '^---$' "$info" | cut -d' ' -f1 | paste -sd, -)"
    echo "$gem $versions $(md5sum "$info" | cut -d' ' -f1)"
  done
} >"$work/index/versions"

python3 -u -m http.server --bind 127.0.0.1 0 --directory "$work/index" >"$work/server.log" 2>&1 &
server=$!
port=""
for _ in $(seq 50); do
  port="$(sed -n 's/.* port \([0-9]*\) .*/\1/p' "$work/server.log" | head -1)"
  [[ -n "$port" ]] && break
  sleep 0.1
done
[[ -n "$port" ]] || { cat "$work/server.log"; exit 2; }

mkdir -p "$work/app"
{ echo "source \"http://127.0.0.1:$port\""; cat "$case_dir/Gemfile.gems"; } >"$work/app/Gemfile"
if (
  cd "$work/app"
  env -i PATH="$PATH" HOME="$work/home" \
    BUNDLE_GEMFILE="$work/app/Gemfile" \
    BUNDLE_USER_HOME="$work/bundle" \
    BUNDLE_APP_CONFIG="$work/app/.bundle" \
    ORACLE_PLATFORM="${ORACLE_PLATFORM:-x86_64-linux}" \
    ORACLE_RUBY="${ORACLE_RUBY:-4.0.0}" \
    ORACLE_RUBYGEMS="${ORACLE_RUBYGEMS:-4.0.16}" \
    RUBYOPT="-r$here/bundler_oracle.rb" \
    THOR_COLUMNS=100000 \
    bundle lock >"$work/out.txt" 2>&1
); then
  sed -n '/^  specs:/,/^$/p' "$work/app/Gemfile.lock" | grep '^    [^ ]' | sed 's/^    //'
else
  grep -av '^Fetching \|^Resolving dependencies\|^Writing lockfile' "$work/out.txt" |
    sed "s|http://127.0.0.1:$port|@SOURCE@|g"
  exit 1
fi
