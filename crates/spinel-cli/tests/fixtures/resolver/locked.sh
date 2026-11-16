#!/usr/bin/env bash
#
# Lock each case under `locked/` with Bundler, one step at a time, each step
# with the lockfile the step before wrote in place (#79):
#
#   crates/spinel-cli/tests/fixtures/resolver/locked.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/resolver/locked
#
# A case is `case.env` (platforms, the first the local one; the Ruby and
# RubyGems to resolve for, which `bundler_oracle.rb` makes Bundler see as its
# own) and its steps, `1.Gemfile`, `2.Gemfile`, ... Step 1 is locked with no
# lockfile; each later step with the previous step's `Gemfile.lock` in place
# and no update asked for, as `bundle install` locks after a Gemfile edit.
# `bundle lock` writes `<n>.Gemfile.lock`, the source spelled `@SOURCE@`.
# `index=lockfile` in `case.env` resolves against the Rails-sized snapshot in
# `../lockfile/index/` instead of this one.
# A later step with a `<n>.update` file is locked as an update (#87): `bundle
# lock --update` with the gems the file names, separated by spaces, or with
# none when it is empty, as `bundle update` unlocks them.
# Bundler leaves a lockfile it would only rewrite with a new `BUNDLED WITH`
# or `RUBY VERSION` alone, so a step that changes nothing records the same
# lockfile again. Everything resolves against a snapshot served on
# 127.0.0.1. `crates/spinel-cli/tests/locked.rs` replays the steps
# with `spinel resolve --locked` and compares each lockfile.
#
# Written with Bundler 4.0.20 on Ruby 4.0.9; Python 3 serves the files.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-locked.XXXXXX")"
servers=()
cleanup() {
  for server in "${servers[@]}"; do kill "$server" 2>/dev/null || true; done
  rm -rf "$work"
}
trap cleanup EXIT

# Serve the directory `$2` and print its URL.
serve() {
  local log="$work/server-$1.log"
  python3 -u -m http.server --bind 127.0.0.1 0 --directory "$2" >"$log" 2>&1 &
  servers+=($!)
  local port=""
  for _ in $(seq 50); do
    port="$(sed -n 's/.* port \([0-9]*\) .*/\1/p' "$log" | head -1)"
    [[ -n "$port" ]] && break
    sleep 0.1
  done
  [[ -n "$port" ]] || { cat "$log" >&2; exit 1; }
  echo "http://127.0.0.1:$port"
}
serve resolver "$here/index" >"$work/url-resolver"
serve lockfile "$here/../lockfile/index" >"$work/url-lockfile"

for dir in "$here"/locked/*/; do
  name="$(basename "$dir")"
  platforms="$(sed -n 's/^platforms=//p' "$dir/case.env")"
  read -r -a platform_list <<<"$platforms"
  index="$(sed -n 's/^index=//p' "$dir/case.env")"
  source_url="$(cat "$work/url-${index:-resolver}")"
  rm -f "$dir"/*.Gemfile.lock
  mkdir -p "$work/$name"
  step=1
  while [[ -f "$dir/$step.Gemfile" ]]; do
    cp "$dir/$step.Gemfile" "$work/$name/Gemfile"
    extra=()
    if [[ "$step" == 1 ]]; then
      for platform in "${platform_list[@]:1}"; do
        extra+=(--add-platform "$platform")
      done
    fi
    if [[ -f "$dir/$step.update" ]]; then
      read -r -a gems <"$dir/$step.update" || true
      extra+=(--update "${gems[@]}")
    fi
    (
      cd "$work/$name"
      env -i PATH="$PATH" HOME="$work/home" \
        BUNDLE_GEMFILE="$work/$name/Gemfile" \
        BUNDLE_USER_HOME="$work/bundle" \
        BUNDLE_APP_CONFIG="$work/$name/.bundle" \
        SPINEL_RESOLVER_SOURCE="$source_url" \
        ORACLE_PLATFORM="${platform_list[0]}" \
        ORACLE_RUBY="$(sed -n 's/^ruby=//p' "$dir/case.env")" \
        ORACLE_RUBYGEMS="$(sed -n 's/^rubygems=//p' "$dir/case.env")" \
        RUBYOPT="-r$here/bundler_oracle.rb" \
        bundle lock "${extra[@]}" >"$work/$name/out-$step.txt" 2>&1
    ) || { cat "$work/$name/out-$step.txt" >&2; exit 1; }
    sed "s|$source_url|@SOURCE@|g" "$work/$name/Gemfile.lock" >"$dir/$step.Gemfile.lock"
    echo "$name: step $step locked"
    step=$((step + 1))
  done
done
