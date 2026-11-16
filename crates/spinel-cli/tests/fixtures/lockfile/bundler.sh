#!/usr/bin/env bash
#
# Lock every case under `cases/` with Bundler, against the snapshot in
# `index/` served on 127.0.0.1, and record the `Gemfile.lock` it writes
# (#80):
#
#   crates/spinel-cli/tests/fixtures/lockfile/bundler.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/lockfile/cases
#
# Each `cases/<name>/` holds a Gemfile and `case.env`: the resolution
# platforms (the first is the local one, the rest are added with
# `--add-platform`), the Ruby and RubyGems versions to resolve for, which the
# resolver's `bundler_oracle.rb` makes Bundler see as its own, and
# `checksums=false` for a lockfile written without `CHECKSUMS`
# (`BUNDLE_LOCKFILE_CHECKSUMS=false`). `bundle lock` writes the case's
# `Gemfile.lock`, with what differs from run to run spelled as a placeholder:
# the source as `@SOURCE@`, a second source the `sources` case scopes gems
# to as `@SOURCE_2@`, and its git repositories as `@GIT_TOOL@` and
# `@TAGGED_TOOL@`. Those repositories are made here from `cases/sources/git/`
# with a fixed author and date, so their revisions are the same every run.
# `crates/spinel-cli/tests/lockfile.rs` reads every lockfile back and writes
# it out unchanged, and compares what Spinel writes for each case with these.
#
# Written with Bundler 4.0.20 on Ruby 4.0.9; Python 3 serves the files.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-lockfile.XXXXXX")"
servers=()
cleanup() {
  for server in "${servers[@]}"; do kill "$server" 2>/dev/null || true; done
  rm -rf "$work"
}
trap cleanup EXIT

# Serve `index/` on a port of its own on address `$2` and print its URL.
# Bundler writes a lockfile's GEM sections in the order of their remotes, so
# the second source is on 127.0.0.2, which sorts after 127.0.0.1 whatever the
# ports.
serve() {
  local log="$work/server-$1.log"
  python3 -u -m http.server --bind "$2" 0 --directory "$here/index" >"$log" 2>&1 &
  servers+=($!)
  local port=""
  for _ in $(seq 50); do
    port="$(sed -n 's/.* port \([0-9]*\) .*/\1/p' "$log" | head -1)"
    [[ -n "$port" ]] && break
    sleep 0.1
  done
  [[ -n "$port" ]] || { cat "$log" >&2; exit 1; }
  echo "http://$2:$port"
}
serve 1 127.0.0.1 >"$work/url-1"
serve 2 127.0.0.2 >"$work/url-2"
source_url="$(cat "$work/url-1")"
source_url_2="$(cat "$work/url-2")"

# A git repository of `cases/sources/git/<name>`, committed and tagged with a
# fixed identity and date.
git_repo() {
  local name="$1" repo="$work/git/$1"
  mkdir -p "$repo"
  cp -R "$here/cases/sources/git/$name/." "$repo/"
  (
    cd "$repo"
    export GIT_AUTHOR_NAME=Spinel GIT_AUTHOR_EMAIL=spinel@example.invalid
    export GIT_COMMITTER_NAME=Spinel GIT_COMMITTER_EMAIL=spinel@example.invalid
    export GIT_AUTHOR_DATE="2026-01-01T00:00:00Z" GIT_COMMITTER_DATE="2026-01-01T00:00:00Z"
    git -c init.defaultBranch=main init -q
    git add -A
    git -c commit.gpgsign=false commit -q -m "$name"
    git tag v0.2.0
  )
  echo "$repo"
}
git_tool="$(git_repo git_tool)"
tagged_tool="$(git_repo tagged_tool)"

for dir in "$here"/cases/*/; do
  name="$(basename "$dir")"
  platforms="$(sed -n 's/^platforms=//p' "$dir/case.env")"
  read -r -a platform_list <<<"$platforms"
  checksums="$(sed -n 's/^checksums=//p' "$dir/case.env")"
  rm -f "$dir/Gemfile.lock"
  mkdir -p "$work/cases"
  cp -R "$dir" "$work/cases/$name"
  rm -rf "$work/cases/$name/git"
  extra=()
  for platform in "${platform_list[@]:1}"; do
    extra+=(--add-platform "$platform")
  done
  (
    cd "$work/cases/$name"
    env -i PATH="$PATH" HOME="$work/home" \
      BUNDLE_GEMFILE="$work/cases/$name/Gemfile" \
      BUNDLE_USER_HOME="$work/bundle" \
      BUNDLE_APP_CONFIG="$work/cases/$name/.bundle" \
      ${checksums:+BUNDLE_LOCKFILE_CHECKSUMS="$checksums"} \
      SPINEL_RESOLVER_SOURCE="$source_url" \
      SPINEL_RESOLVER_SOURCE_2="$source_url_2" \
      LOCKFILE_GIT_TOOL="$git_tool" \
      LOCKFILE_TAGGED_TOOL="$tagged_tool" \
      ORACLE_PLATFORM="${platform_list[0]}" \
      ORACLE_RUBY="$(sed -n 's/^ruby=//p' "$dir/case.env")" \
      ORACLE_RUBYGEMS="$(sed -n 's/^rubygems=//p' "$dir/case.env")" \
      RUBYOPT="-r$here/../resolver/bundler_oracle.rb" \
      bundle lock "${extra[@]}" >"$work/cases/$name.out" 2>&1
  ) || { cat "$work/cases/$name.out" >&2; exit 1; }
  sed -e "s|$source_url_2|@SOURCE_2@|g" -e "s|$source_url|@SOURCE@|g" \
    -e "s|$git_tool|@GIT_TOOL@|g" -e "s|$tagged_tool|@TAGGED_TOOL@|g" \
    "$work/cases/$name/Gemfile.lock" >"$dir/Gemfile.lock"
  echo "$name: locked"
done
