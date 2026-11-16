#!/usr/bin/env bash
#
# Record what Bundler writes for the `spinel update` fixtures (#87), which
# `crates/spinel-cli/tests/update.rs` compares Spinel's lockfiles with:
#
#   crates/spinel-cli/tests/fixtures/update/bundler.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/update/bundler
#
# The gems in `gems/` are served on 127.0.0.1 with a compact index written
# from their checksums, as the test serves them, for the Gemfile in
# `bundler/Gemfile`. With only the 1.0.0 versions published, `bundle lock`
# writes `installed.lock`; with every version published, `bundle lock
# --update rake` and `bundle lock --update` from it write `rake.lock` and
# `all.lock`, and `bundle lock --update nope` its error, `nope.txt`. The
# source is spelled `@SOURCE@`. `GEM_HOME` and `GEM_PATH` are empty
# directories, so Ruby's own `rake` is not among the gems Bundler finds.
# An opt-in tool, never run by CI.
#
# Written with Bundler 4.0.20 on Ruby 4.0.9; Python 3 serves the files.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="$here/bundler"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-update-bundler.XXXXXX")"
server=""
cleanup() {
  [[ -n "$server" ]] && kill "$server" 2>/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

# The gem's `/info` dependencies field, from its gemspec.
deps() {
  sed -n 's/^  s.add_dependency "\([^"]*\)", "\([^"]*\)"$/\1:\2/p' "$1" | paste -sd, -
}

# Publish the gems whose versions match `$1` (a grep pattern).
publish() {
  rm -rf "$work/index"
  mkdir -p "$work/index/info" "$work/index/gems"
  for gem in "$here"/gems/*.gem; do
    file="$(basename "$gem" .gem)"
    name="${file%-*}"
    version="${file##*-}"
    grep -qx "$1" <<<"$version" || continue
    cp "$gem" "$work/index/gems/"
    [[ -f "$work/index/info/$name" ]] || echo "---" >"$work/index/info/$name"
    echo "$version $(deps "$here/src/$file/$name.gemspec")|checksum:$(sha256sum "$gem" | cut -d' ' -f1)" \
      >>"$work/index/info/$name"
  done
  {
    echo "created_at: 2026-01-01T00:00:00Z"
    echo "---"
    for info in "$work"/index/info/*; do
      versions="$(grep -v '^---$' "$info" | cut -d' ' -f1 | paste -sd, -)"
      echo "$(basename "$info") $versions $(md5sum "$info" | cut -d' ' -f1)"
    done
  } >"$work/index/versions"
}

mkdir -p "$work/served"
publish '1\.0\.0'
mv "$work/index" "$work/served/index"
python3 -u -m http.server --bind 127.0.0.1 0 --directory "$work/served" >"$work/server.log" 2>&1 &
server=$!
port=""
for _ in $(seq 50); do
  port="$(sed -n 's/.* port \([0-9]*\) .*/\1/p' "$work/server.log" | head -1)"
  [[ -n "$port" ]] && break
  sleep 0.1
done
[[ -n "$port" ]] || { cat "$work/server.log"; exit 2; }
source_url="http://127.0.0.1:$port/index"

mkdir -p "$work/app"
sed "s|@SOURCE@|$source_url|" "$out/Gemfile" >"$work/app/Gemfile"
lock() {
  (
    cd "$work/app"
    env -i PATH="$PATH" HOME="$work/home" \
      BUNDLE_GEMFILE="$work/app/Gemfile" \
      BUNDLE_USER_HOME="$work/bundle" \
      BUNDLE_APP_CONFIG="$work/app/.bundle" \
      GEM_HOME="$work/gem_home" GEM_PATH="$work/gem_home" \
      bundle lock "$@"
  )
}
record() {
  sed "s|$source_url|@SOURCE@|g" "$work/app/Gemfile.lock" >"$out/$1"
}

lock >/dev/null
record installed.lock

publish '.*'
rm -rf "$work/served/index"
mv "$work/index" "$work/served/index"
lock --update rake >/dev/null
record rake.lock
sed "s|@SOURCE@|$source_url|g" "$out/installed.lock" >"$work/app/Gemfile.lock"
lock --update >/dev/null
record all.lock
sed "s|@SOURCE@|$source_url|g" "$out/installed.lock" >"$work/app/Gemfile.lock"
status=0
lock --update nope >"$work/nope.txt" 2>&1 || status=$?
{ cat "$work/nope.txt"; echo "exit $status"; } >"$out/nope.txt"
