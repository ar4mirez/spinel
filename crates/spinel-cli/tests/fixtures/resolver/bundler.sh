#!/usr/bin/env bash
#
# Resolve every case under `cases/` with Bundler, against the snapshot in
# `index/` served on 127.0.0.1, and record what it chose (#78):
#
#   crates/spinel-cli/tests/fixtures/resolver/bundler.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/resolver/cases
#
# Each `cases/<name>/` holds a Gemfile and `case.env`: the resolution
# platforms (the first is the local one, the rest are added with
# `--add-platform`), and the Ruby and RubyGems versions to resolve for, which
# `bundler_oracle.rb` makes Bundler see as its own. `bundle lock` writes
# `Gemfile.lock` there, its remote spelled `@SOURCE@`; a case Bundler cannot
# resolve gets `bundler.txt` instead, Bundler's explanation, which the PR
# shows beside Spinel's. A case Bundler fails or retries also gets
# `relax.log`, what each try's `find_names_to_relax` returned, the final
# failing try's included, as `relax_hook.rb` logs it.
# `crates/spinel-cli/tests/resolver.rs` compares Spinel's resolution of each
# case with these.
#
# Written with Bundler 4.0.20 on Ruby 4.0.9; Python 3 serves the files.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-resolver.XXXXXX")"
server=""
cleanup() {
  [[ -n "$server" ]] && kill "$server" 2>/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

python3 -u -m http.server --bind 127.0.0.1 0 --directory "$here/index" >"$work/server.log" 2>&1 &
server=$!
port=""
for _ in $(seq 50); do
  port="$(sed -n 's/.* port \([0-9]*\) .*/\1/p' "$work/server.log" | head -1)"
  [[ -n "$port" ]] && break
  sleep 0.1
done
[[ -n "$port" ]] || { cat "$work/server.log"; exit 1; }
source_url="http://127.0.0.1:$port"

for dir in "$here"/cases/*/; do
  name="$(basename "$dir")"
  platforms="$(sed -n 's/^platforms=//p' "$dir/case.env")"
  read -r -a platform_list <<<"$platforms"
  rm -f "$dir/Gemfile.lock" "$dir/bundler.txt" "$dir/relax.log"
  mkdir -p "$work/$name"
  cp "$dir/Gemfile" "$work/$name/Gemfile"
  extra=()
  for platform in "${platform_list[@]:1}"; do
    extra+=(--add-platform "$platform")
  done
  if (
    cd "$work/$name"
    env -i PATH="$PATH" HOME="$work/home" \
      BUNDLE_GEMFILE="$work/$name/Gemfile" \
      BUNDLE_USER_HOME="$work/bundle" \
      BUNDLE_APP_CONFIG="$work/$name/.bundle" \
      SPINEL_RESOLVER_SOURCE="$source_url" \
      ORACLE_PLATFORM="${platform_list[0]}" \
      ORACLE_RUBY="$(sed -n 's/^ruby=//p' "$dir/case.env")" \
      ORACLE_RUBYGEMS="$(sed -n 's/^rubygems=//p' "$dir/case.env")" \
      RUBYOPT="-r$here/bundler_oracle.rb -r$here/relax_hook.rb" \
      RELAX_LOG="$dir/relax.log" \
      bundle lock "${extra[@]}" >"$work/$name/out.txt" 2>&1
  ); then
    sed "s|$source_url|@SOURCE@|g" "$work/$name/Gemfile.lock" >"$dir/Gemfile.lock"
    echo "$name: resolved"
  else
    # Bundler's progress lines ("Fetching gem metadata ...", "Writing
    # lockfile to ...") are not part of
    # its explanation.
    grep -av '^Fetching \|^Resolving dependencies\|^Writing lockfile' "$work/$name/out.txt" |
      sed "s|$source_url|@SOURCE@|g" >"$dir/bundler.txt"
    echo "$name: failed"
  fi
done
